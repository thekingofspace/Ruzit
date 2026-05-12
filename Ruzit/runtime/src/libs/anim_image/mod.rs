use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use mlua::{
    AnyUserData, Lua, MultiValue, Table, UserData, UserDataFields, UserDataMethods, Value,
};

use crate::libs::asset::ImageAsset;
use crate::libs::dynimg::{DynImgHandle, DynImgState};
use crate::libs::gui::{GuiPrimitive, ImageRef, PrimitiveState, Shape};
use crate::libs::primitives::{Color3, Dim};
use crate::libs::signal;

mod xml;

static NEXT_IMAGE_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Copy, Debug)]
struct FrameRect {
    x: u32,
    y: u32,
    w: u32,
    h: u32,
}

#[derive(Clone)]
struct AnimDef {
    frames: Vec<usize>,
    fps: f32,
    looped: bool,
}

enum SourceKind {
    Static {
        bytes: Arc<Vec<u8>>,
        width: u32,
        height: u32,
    },
    Dyn(Arc<Mutex<DynImgState>>),
}

#[derive(Clone, Copy, PartialEq)]
enum TrackState {
    Idle,
    Playing,
    Stopped,
}

struct TrackInner {
    frames: Vec<usize>,
    fps: f32,
    looped: bool,
    priority: f32,
    state: TrackState,
    elapsed: f32,
    did_loop_signal: Table,
    ended_signal: Table,
}

impl TrackInner {
    fn current_frame(&self) -> Option<usize> {
        if self.frames.is_empty() {
            return None;
        }
        let frame_dur = 1.0 / self.fps.max(0.001);
        let total = frame_dur * self.frames.len() as f32;
        let t = if self.looped {
            self.elapsed.rem_euclid(total)
        } else {
            self.elapsed.min(total - 1e-6)
        };
        let idx = (t / frame_dur) as usize;
        self.frames.get(idx.min(self.frames.len() - 1)).copied()
    }
}

pub struct AnimationTrack {
    inner: Arc<Mutex<TrackInner>>,
}

impl UserData for AnimationTrack {
    fn add_fields<F: UserDataFields<Self>>(f: &mut F) {
        f.add_field_method_get("FPS", |_, this| Ok(this.inner.lock().unwrap().fps));
        f.add_field_method_set("FPS", |_, this, v: f32| {
            this.inner.lock().unwrap().fps = v.max(0.001);
            Ok(())
        });
        f.add_field_method_get("Looped", |_, this| Ok(this.inner.lock().unwrap().looped));
        f.add_field_method_set("Looped", |_, this, v: bool| {
            this.inner.lock().unwrap().looped = v;
            Ok(())
        });
        f.add_field_method_get("Priority", |_, this| Ok(this.inner.lock().unwrap().priority));
        f.add_field_method_set("Priority", |_, this, v: f32| {
            this.inner.lock().unwrap().priority = v;
            Ok(())
        });
        f.add_field_method_get("FrameCount", |_, this| {
            Ok(this.inner.lock().unwrap().frames.len() as i64)
        });
        f.add_field_method_get("State", |_, this| {
            Ok(match this.inner.lock().unwrap().state {
                TrackState::Idle => "Idle",
                TrackState::Playing => "Playing",
                TrackState::Stopped => "Stopped",
            }
            .to_string())
        });
        f.add_field_method_get("Elapsed", |_, this| Ok(this.inner.lock().unwrap().elapsed));
        f.add_field_method_get("DidLoop", |_, this| {
            Ok(this.inner.lock().unwrap().did_loop_signal.clone())
        });
        f.add_field_method_get("Ended", |_, this| {
            Ok(this.inner.lock().unwrap().ended_signal.clone())
        });
    }

    fn add_methods<M: UserDataMethods<Self>>(m: &mut M) {
        m.add_method("Play", |_, this, _: ()| -> mlua::Result<()> {
            let mut inner = this.inner.lock().unwrap();
            if inner.state != TrackState::Playing {
                inner.elapsed = 0.0;
            }
            inner.state = TrackState::Playing;
            Ok(())
        });
        m.add_method("Stop", |_, this, _: ()| -> mlua::Result<()> {
            this.inner.lock().unwrap().state = TrackState::Stopped;
            Ok(())
        });
        m.add_method("Pause", |_, this, _: ()| -> mlua::Result<()> {
            let mut inner = this.inner.lock().unwrap();
            if inner.state == TrackState::Playing {
                inner.state = TrackState::Idle;
            }
            Ok(())
        });
        m.add_method("Resume", |_, this, _: ()| -> mlua::Result<()> {
            let mut inner = this.inner.lock().unwrap();
            if inner.state == TrackState::Idle {
                inner.state = TrackState::Playing;
            }
            Ok(())
        });
        m.add_method("Wait", |lua, this, _: ()| -> mlua::Result<()> {
            let signal_table = this.inner.lock().unwrap().ended_signal.clone();
            let wait: mlua::Function = signal_table.get("Wait")?;
            wait.call::<MultiValue>(Value::Table(signal_table))?;
            let _ = lua;
            Ok(())
        });
    }
}

struct AnimatedImageInner {
    source: SourceKind,
    frames: Vec<FrameRect>,
    animations: HashMap<String, AnimDef>,
    primitive_state: Arc<Mutex<PrimitiveState>>,
    tracks: Vec<Arc<Mutex<TrackInner>>>,
    manual_frame: Option<usize>,
    alive: bool,
}

pub struct AnimatedImage {
    inner: Arc<Mutex<AnimatedImageInner>>,
}

thread_local! {
    static ACTIVE: RefCell<Vec<Arc<Mutex<AnimatedImageInner>>>> = const { RefCell::new(Vec::new()) };
}

impl AnimatedImage {
    fn new(
        lua: &Lua,
        xml_text: &str,
        source: SourceKind,
        source_w: u32,
        source_h: u32,
    ) -> mlua::Result<Self> {
        let parsed = xml::parse_animated_xml(xml_text, source_w, source_h)?;
        let primitive = GuiPrimitive::new(lua, Shape::Image)?;
        let primitive_arc = primitive.state_arc();
        {
            let mut ps = primitive_arc.lock().unwrap();
            ps.size = Dim::new(source_w as f32, source_h as f32);
        }

        let animations = parsed
            .animations
            .into_iter()
            .map(|(k, v)| {
                let mut resolved: Vec<usize> = Vec::with_capacity(v.frames.len());
                for f in v.frames {
                    match f {
                        xml::FrameRef::Index(i) => {
                            if i < parsed.frames.len() {
                                resolved.push(i);
                            }
                        }
                        xml::FrameRef::Name(name) => {
                            if let Some(&i) = parsed.name_to_frame.get(&name) {
                                resolved.push(i);
                            }
                        }
                    }
                }
                (
                    k,
                    AnimDef {
                        frames: resolved,
                        fps: v.fps,
                        looped: v.looped,
                    },
                )
            })
            .collect();

        let inner = Arc::new(Mutex::new(AnimatedImageInner {
            source,
            frames: parsed.frames,
            animations,
            primitive_state: primitive_arc,
            tracks: Vec::new(),
            manual_frame: None,
            alive: true,
        }));
        ACTIVE.with(|c| c.borrow_mut().push(inner.clone()));

        let img = AnimatedImage { inner };
        img.render_frame(0);
        Ok(img)
    }

    fn render_frame(&self, frame_idx: usize) {
        let inner = self.inner.lock().unwrap();
        render_frame_to_primitive(&inner, frame_idx);
    }
}

fn read_source_bytes(src: &SourceKind) -> Option<(Vec<u8>, u32, u32)> {
    match src {
        SourceKind::Static {
            bytes,
            width,
            height,
        } => Some(((**bytes).clone(), *width, *height)),
        SourceKind::Dyn(state_arc) => {
            let s = state_arc.lock().unwrap();
            if !s.alive {
                return None;
            }
            let buf = s.buffer.lock().unwrap();
            Some((buf.bytes.clone(), buf.width, buf.height))
        }
    }
}

fn render_frame_to_primitive(inner: &AnimatedImageInner, frame_idx: usize) {
    let Some(rect) = inner.frames.get(frame_idx).copied() else {
        return;
    };
    let Some((src_bytes, src_w, src_h)) = read_source_bytes(&inner.source) else {
        return;
    };
    let cropped = crop_rgba(&src_bytes, src_w, src_h, rect);
    let mut ps = inner.primitive_state.lock().unwrap();
    let id = NEXT_IMAGE_ID.fetch_add(1, Ordering::Relaxed);
    ps.image = Some(Arc::new(ImageRef {
        id,
        width: rect.w,
        height: rect.h,
        data: Arc::new(cropped),
    }));
}

fn crop_rgba(src: &[u8], src_w: u32, src_h: u32, rect: FrameRect) -> Vec<u8> {
    let dst_w = rect.w.min(src_w.saturating_sub(rect.x));
    let dst_h = rect.h.min(src_h.saturating_sub(rect.y));
    let mut out = Vec::with_capacity((dst_w * dst_h * 4) as usize);
    let stride = (src_w * 4) as usize;
    for row in 0..dst_h {
        let y = (rect.y + row) as usize;
        let start = y * stride + (rect.x as usize) * 4;
        let end = start + (dst_w as usize) * 4;
        if end > src.len() {
            break;
        }
        out.extend_from_slice(&src[start..end]);
    }
    let expected = (rect.w * rect.h * 4) as usize;
    if out.len() < expected {
        out.resize(expected, 0);
    }
    out
}

pub fn tick(lua: &Lua, dt: f32) {
    let snapshot: Vec<Arc<Mutex<AnimatedImageInner>>> = ACTIVE.with(|c| {
        let mut reg = c.borrow_mut();
        reg.retain(|s| s.lock().unwrap().alive);
        reg.iter().cloned().collect()
    });

    for ai_arc in snapshot {
        let mut pending_loop: Vec<Table> = Vec::new();
        let mut pending_end: Vec<Table> = Vec::new();
        let chosen_frame: Option<usize>;

        {
            let inner = ai_arc.lock().unwrap();
            let manual = inner.manual_frame;
            let mut best: Option<(f32, usize)> = None;
            for tr_arc in &inner.tracks {
                let mut tr = tr_arc.lock().unwrap();
                if tr.state != TrackState::Playing || tr.frames.is_empty() {
                    continue;
                }
                let frame_dur = 1.0 / tr.fps.max(0.001);
                let total = frame_dur * tr.frames.len() as f32;
                let prev = tr.elapsed;
                tr.elapsed = prev + dt;

                if tr.elapsed >= total {
                    if tr.looped {
                        let laps = (tr.elapsed / total).floor() as u32;
                        for _ in 0..laps {
                            pending_loop.push(tr.did_loop_signal.clone());
                        }
                        tr.elapsed = tr.elapsed.rem_euclid(total);
                    } else {
                        tr.elapsed = total;
                        tr.state = TrackState::Stopped;
                        pending_end.push(tr.ended_signal.clone());
                        continue;
                    }
                }

                if let Some(f) = tr.current_frame() {
                    if best.map(|(p, _)| tr.priority > p).unwrap_or(true) {
                        best = Some((tr.priority, f));
                    }
                }
            }
            chosen_frame = best.map(|(_, f)| f).or(manual);
        }

        if let Some(f) = chosen_frame {
            let inner = ai_arc.lock().unwrap();
            render_frame_to_primitive(&inner, f);
        }

        for sig in pending_loop {
            let _ = signal::fire(lua, &sig, MultiValue::new());
        }
        for sig in pending_end {
            let _ = signal::fire(lua, &sig, MultiValue::new());
        }
    }
}

impl UserData for AnimatedImage {
    fn add_fields<F: UserDataFields<Self>>(f: &mut F) {
        f.add_field_method_get("Size", |_, this| {
            Ok(this.inner.lock().unwrap().primitive_state.lock().unwrap().size)
        });
        f.add_field_method_set("Size", |_, this, v: AnyUserData| {
            let d = *v.borrow::<Dim>().map_err(|_| {
                mlua::Error::RuntimeError("AnimatedImage.Size expects a Dim".into())
            })?;
            this.inner
                .lock()
                .unwrap()
                .primitive_state
                .lock()
                .unwrap()
                .size = d;
            Ok(())
        });
        f.add_field_method_get("Position", |_, this| {
            Ok(this.inner.lock().unwrap().primitive_state.lock().unwrap().position)
        });
        f.add_field_method_set("Position", |_, this, v: AnyUserData| {
            let d = *v.borrow::<Dim>().map_err(|_| {
                mlua::Error::RuntimeError("AnimatedImage.Position expects a Dim".into())
            })?;
            this.inner
                .lock()
                .unwrap()
                .primitive_state
                .lock()
                .unwrap()
                .position = d;
            Ok(())
        });
        f.add_field_method_get("ZIndex", |_, this| {
            Ok(this.inner.lock().unwrap().primitive_state.lock().unwrap().z_index as i64)
        });
        f.add_field_method_set("ZIndex", |_, this, v: i64| {
            this.inner
                .lock()
                .unwrap()
                .primitive_state
                .lock()
                .unwrap()
                .z_index = v as i32;
            Ok(())
        });
        f.add_field_method_get("Visible", |_, this| {
            Ok(this.inner.lock().unwrap().primitive_state.lock().unwrap().visible)
        });
        f.add_field_method_set("Visible", |_, this, v: bool| {
            this.inner
                .lock()
                .unwrap()
                .primitive_state
                .lock()
                .unwrap()
                .visible = v;
            Ok(())
        });
        f.add_field_method_get("Color", |_, this| {
            Ok(this.inner.lock().unwrap().primitive_state.lock().unwrap().color)
        });
        f.add_field_method_set("Color", |_, this, v: AnyUserData| {
            let c = *v.borrow::<Color3>().map_err(|_| {
                mlua::Error::RuntimeError("AnimatedImage.Color expects a Color3".into())
            })?;
            this.inner
                .lock()
                .unwrap()
                .primitive_state
                .lock()
                .unwrap()
                .color = c;
            Ok(())
        });
        f.add_field_method_get("Transparency", |_, this| {
            Ok(this.inner.lock().unwrap().primitive_state.lock().unwrap().transparency)
        });
        f.add_field_method_set("Transparency", |_, this, v: f32| {
            this.inner
                .lock()
                .unwrap()
                .primitive_state
                .lock()
                .unwrap()
                .transparency = v.clamp(0.0, 1.0);
            Ok(())
        });
        f.add_field_method_get("FrameCount", |_, this| {
            Ok(this.inner.lock().unwrap().frames.len() as i64)
        });
        f.add_field_method_get("Alive", |_, this| Ok(this.inner.lock().unwrap().alive));
    }

    fn add_methods<M: UserDataMethods<Self>>(m: &mut M) {
        m.add_method(
            "GetTrack",
            |lua, this, name: String| -> mlua::Result<Option<AnimationTrack>> {
                let mut inner = this.inner.lock().unwrap();
                let def = match inner.animations.get(&name) {
                    Some(d) => d.clone(),
                    None => return Ok(None),
                };
                let did_loop = signal::new_instance(lua)?;
                let ended = signal::new_instance(lua)?;
                let track_inner = Arc::new(Mutex::new(TrackInner {
                    frames: def.frames,
                    fps: def.fps,
                    looped: def.looped,
                    priority: 1.0,
                    state: TrackState::Idle,
                    elapsed: 0.0,
                    did_loop_signal: did_loop,
                    ended_signal: ended,
                }));
                inner.tracks.push(track_inner.clone());
                Ok(Some(AnimationTrack { inner: track_inner }))
            },
        );
        m.add_method(
            "JumpToFrame",
            |_, this, idx: i64| -> mlua::Result<()> {
                let frame_count = this.inner.lock().unwrap().frames.len();
                if frame_count == 0 {
                    return Ok(());
                }
                let clamped = (idx.max(0) as usize).min(frame_count - 1);
                this.inner.lock().unwrap().manual_frame = Some(clamped);
                render_frame_to_primitive(&this.inner.lock().unwrap(), clamped);
                Ok(())
            },
        );
        m.add_method("ClearJump", |_, this, _: ()| -> mlua::Result<()> {
            this.inner.lock().unwrap().manual_frame = None;
            Ok(())
        });
        m.add_method(
            "GetPrimitive",
            |lua, this, _: ()| -> mlua::Result<GuiPrimitive> {
                let _ = lua;
                Ok(GuiPrimitive::from_state(
                    this.inner.lock().unwrap().primitive_state.clone(),
                ))
            },
        );
        m.add_method("HasAnimation", |_, this, name: String| -> mlua::Result<bool> {
            Ok(this.inner.lock().unwrap().animations.contains_key(&name))
        });
        m.add_method("Destroy", |_, this, _: ()| -> mlua::Result<()> {
            let mut inner = this.inner.lock().unwrap();
            inner.alive = false;
            inner.primitive_state.lock().unwrap().alive = false;
            Ok(())
        });
    }
}

pub fn make_animated_image(lua: &Lua, args: MultiValue) -> mlua::Result<AnimatedImage> {
    let mut iter = args.into_iter();
    let xml_v = iter.next().ok_or_else(|| {
        mlua::Error::RuntimeError(
            "GUI.AnimatedImage: missing first argument (XML string)".into(),
        )
    })?;
    let src_v = iter.next().ok_or_else(|| {
        mlua::Error::RuntimeError(
            "GUI.AnimatedImage: missing second argument (Image asset or DynImg)".into(),
        )
    })?;

    let xml_text = match xml_v {
        Value::String(s) => s.to_str()?.to_string(),
        _ => {
            return Err(mlua::Error::RuntimeError(
                "GUI.AnimatedImage: first argument must be an XML string".into(),
            ));
        }
    };

    let (source, w, h) = match src_v {
        Value::UserData(ud) => {
            if let Ok(asset) = ud.borrow::<ImageAsset>() {
                (
                    SourceKind::Static {
                        bytes: asset.data.clone(),
                        width: asset.width,
                        height: asset.height,
                    },
                    asset.width,
                    asset.height,
                )
            } else if let Ok(dyn_h) = ud.borrow::<DynImgHandle>() {
                let (w, h) = {
                    let s = dyn_h.inner.lock().unwrap();
                    (s.width, s.height)
                };
                (SourceKind::Dyn(dyn_h.inner.clone()), w, h)
            } else {
                return Err(mlua::Error::RuntimeError(
                    "GUI.AnimatedImage: source must be an Image asset or a DynImg".into(),
                ));
            }
        }
        _ => {
            return Err(mlua::Error::RuntimeError(
                "GUI.AnimatedImage: source must be a userdata".into(),
            ));
        }
    };

    AnimatedImage::new(lua, &xml_text, source, w, h)
}
