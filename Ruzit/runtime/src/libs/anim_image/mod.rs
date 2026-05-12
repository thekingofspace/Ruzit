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

#[derive(Clone, Copy)]
struct Frame {
    x: u32,
    y: u32,
    width: u32,
    height: u32,
    frame_x: i32,
    frame_y: i32,
    frame_width: u32,
    frame_height: u32,
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
            self.elapsed.min(total - 1e-6).max(0.0)
        };
        let idx = (t / frame_dur) as usize;
        self.frames.get(idx.min(self.frames.len() - 1)).copied()
    }
}

pub struct AnimatedImageTrack {
    inner: Arc<Mutex<TrackInner>>,
}

impl UserData for AnimatedImageTrack {
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
    frames: Vec<Frame>,
    name_to_index: HashMap<String, usize>,
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
    ) -> mlua::Result<Self> {
        let parsed = xml::parse_texture_atlas(xml_text)?;
        let frames: Vec<Frame> = parsed
            .frames
            .iter()
            .map(|(_, f)| Frame {
                x: f.x,
                y: f.y,
                width: f.width,
                height: f.height,
                frame_x: f.frame_x,
                frame_y: f.frame_y,
                frame_width: f.frame_width,
                frame_height: f.frame_height,
            })
            .collect();
        let name_to_index = parsed.name_to_index;

        let primitive = GuiPrimitive::new(lua, Shape::Image)?;
        let primitive_arc = primitive.state_arc();
        let initial_size = frames
            .first()
            .map(|f| Dim::new(f.frame_width as f32, f.frame_height as f32))
            .unwrap_or(Dim::new(64.0, 64.0));
        primitive_arc.lock().unwrap().size = initial_size;

        let inner = Arc::new(Mutex::new(AnimatedImageInner {
            source,
            frames,
            name_to_index,
            primitive_state: primitive_arc,
            tracks: Vec::new(),
            manual_frame: None,
            alive: true,
        }));
        ACTIVE.with(|c| c.borrow_mut().push(inner.clone()));

        let img = AnimatedImage { inner };
        if !img.inner.lock().unwrap().frames.is_empty() {
            let guard = img.inner.lock().unwrap();
            render_frame_to_primitive(&guard, 0);
        }
        Ok(img)
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
    let Some(frame) = inner.frames.get(frame_idx).copied() else {
        return;
    };
    let Some((src_bytes, src_w, src_h)) = read_source_bytes(&inner.source) else {
        return;
    };
    let pixels = build_frame_pixels(&src_bytes, src_w, src_h, &frame);
    let id = NEXT_IMAGE_ID.fetch_add(1, Ordering::Relaxed);
    {
        let mut ps = inner.primitive_state.lock().unwrap();
        ps.image = Some(Arc::new(ImageRef {
            id,
            width: frame.frame_width.max(1),
            height: frame.frame_height.max(1),
            data: Arc::new(pixels),
        }));
    }
    crate::libs::gui::bump_dirty();
}

fn build_frame_pixels(src: &[u8], src_w: u32, src_h: u32, f: &Frame) -> Vec<u8> {
    let fw = f.frame_width.max(1);
    let fh = f.frame_height.max(1);
    let mut out = vec![0u8; (fw * fh * 4) as usize];

    let src_stride = (src_w * 4) as usize;
    let dst_stride = (fw * 4) as usize;

    for row in 0..f.height {
        let sy_signed = (f.y as i64) + (row as i64);
        if sy_signed < 0 || sy_signed >= src_h as i64 {
            continue;
        }
        let dy_signed = (row as i64) - (f.frame_y as i64);
        if dy_signed < 0 || dy_signed >= fh as i64 {
            continue;
        }
        let sy = sy_signed as usize;
        let dy = dy_signed as usize;

        for col in 0..f.width {
            let sx_signed = (f.x as i64) + (col as i64);
            if sx_signed < 0 || sx_signed >= src_w as i64 {
                continue;
            }
            let dx_signed = (col as i64) - (f.frame_x as i64);
            if dx_signed < 0 || dx_signed >= fw as i64 {
                continue;
            }
            let sx = sx_signed as usize;
            let dx = dx_signed as usize;

            let s_off = sy * src_stride + sx * 4;
            let d_off = dy * dst_stride + dx * 4;
            if s_off + 4 > src.len() || d_off + 4 > out.len() {
                continue;
            }
            out[d_off..d_off + 4].copy_from_slice(&src[s_off..s_off + 4]);
        }
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

fn resolve_frames(
    name_to_index: &HashMap<String, usize>,
    frames_v: Value,
) -> mlua::Result<Vec<usize>> {
    let mut out: Vec<usize> = Vec::new();
    match frames_v {
        Value::String(s) => {
            let text = s.to_str()?.to_string();
            for part in text.split(',') {
                let p = part.trim();
                if p.is_empty() {
                    continue;
                }
                let idx = name_to_index.get(p).copied().ok_or_else(|| {
                    mlua::Error::RuntimeError(format!(
                        "AnimatedImage:CreateTrack: unknown frame '{p}'"
                    ))
                })?;
                out.push(idx);
            }
        }
        Value::Table(t) => {
            for pair in t.sequence_values::<String>() {
                let name = pair?;
                let idx = name_to_index.get(&name).copied().ok_or_else(|| {
                    mlua::Error::RuntimeError(format!(
                        "AnimatedImage:CreateTrack: unknown frame '{name}'"
                    ))
                })?;
                out.push(idx);
            }
        }
        _ => {
            return Err(mlua::Error::RuntimeError(
                "AnimatedImage:CreateTrack: frames must be a comma-separated string or an array of names".into(),
            ));
        }
    }
    if out.is_empty() {
        return Err(mlua::Error::RuntimeError(
            "AnimatedImage:CreateTrack: at least one frame name is required".into(),
        ));
    }
    Ok(out)
}

fn read_track_opts(opts_v: Value) -> mlua::Result<(f32, bool, f32)> {
    let mut fps: f32 = 12.0;
    let mut looped: bool = false;
    let mut priority: f32 = 1.0;
    match opts_v {
        Value::Nil => {}
        Value::Table(t) => {
            if let Ok(v) = t.get::<f32>("FPS") {
                fps = v.max(0.001);
            }
            if let Ok(v) = t.get::<bool>("Looped") {
                looped = v;
            }
            if let Ok(v) = t.get::<f32>("Priority") {
                priority = v;
            }
        }
        _ => {
            return Err(mlua::Error::RuntimeError(
                "AnimatedImage:CreateTrack: opts must be a table or nil".into(),
            ));
        }
    }
    Ok((fps, looped, priority))
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
        f.add_field_method_get("Rotation", |_, this| {
            Ok(this.inner.lock().unwrap().primitive_state.lock().unwrap().rotation)
        });
        f.add_field_method_set("Rotation", |_, this, deg: f32| {
            this.inner
                .lock()
                .unwrap()
                .primitive_state
                .lock()
                .unwrap()
                .rotation = deg;
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
            "CreateTrack",
            |lua, this, args: MultiValue| -> mlua::Result<AnimatedImageTrack> {
                let mut iter = args.into_iter();
                let frames_v = iter.next().ok_or_else(|| {
                    mlua::Error::RuntimeError(
                        "AnimatedImage:CreateTrack: missing frames argument".into(),
                    )
                })?;
                let opts_v = iter.next().unwrap_or(Value::Nil);
                let (fps, looped, priority) = read_track_opts(opts_v)?;
                let mut inner = this.inner.lock().unwrap();
                let resolved = resolve_frames(&inner.name_to_index, frames_v)?;
                let did_loop = signal::new_instance(lua)?;
                let ended = signal::new_instance(lua)?;
                let track_inner = Arc::new(Mutex::new(TrackInner {
                    frames: resolved,
                    fps,
                    looped,
                    priority,
                    state: TrackState::Idle,
                    elapsed: 0.0,
                    did_loop_signal: did_loop,
                    ended_signal: ended,
                }));
                inner.tracks.push(track_inner.clone());
                Ok(AnimatedImageTrack { inner: track_inner })
            },
        );
        m.add_method(
            "JumpToFrame",
            |_, this, v: Value| -> mlua::Result<()> {
                let mut inner = this.inner.lock().unwrap();
                if inner.frames.is_empty() {
                    return Ok(());
                }
                let last = inner.frames.len() - 1;
                let idx = match v {
                    Value::Integer(n) => (n.max(0) as usize).min(last),
                    Value::Number(n) => (n.max(0.0) as usize).min(last),
                    Value::String(s) => {
                        let name = s.to_str()?.to_string();
                        *inner.name_to_index.get(&name).ok_or_else(|| {
                            mlua::Error::RuntimeError(format!(
                                "AnimatedImage:JumpToFrame: unknown frame '{name}'"
                            ))
                        })?
                    }
                    _ => {
                        return Err(mlua::Error::RuntimeError(
                            "AnimatedImage:JumpToFrame expects a frame name or numeric index".into(),
                        ));
                    }
                };
                inner.manual_frame = Some(idx);
                render_frame_to_primitive(&inner, idx);
                Ok(())
            },
        );
        m.add_method("ClearJump", |_, this, _: ()| -> mlua::Result<()> {
            this.inner.lock().unwrap().manual_frame = None;
            Ok(())
        });
        m.add_method("HasFrame", |_, this, name: String| -> mlua::Result<bool> {
            Ok(this.inner.lock().unwrap().name_to_index.contains_key(&name))
        });
        m.add_method(
            "GetFrameNames",
            |lua, this, _: ()| -> mlua::Result<Table> {
                let inner = this.inner.lock().unwrap();
                let out = lua.create_table()?;
                let mut ordered: Vec<(&String, &usize)> = inner.name_to_index.iter().collect();
                ordered.sort_by_key(|(_, i)| **i);
                for (i, (name, _)) in ordered.iter().enumerate() {
                    out.set(i + 1, (*name).clone())?;
                }
                Ok(out)
            },
        );
        m.add_method(
            "GetPrimitive",
            |lua, this, _: ()| -> mlua::Result<GuiPrimitive> {
                let _ = lua;
                Ok(GuiPrimitive::from_state(
                    this.inner.lock().unwrap().primitive_state.clone(),
                ))
            },
        );
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
            "GUI.AnimatedImage: missing first argument (TextureAtlas XML string)".into(),
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
                "GUI.AnimatedImage: first argument must be a TextureAtlas XML string".into(),
            ));
        }
    };

    let source = match src_v {
        Value::UserData(ud) => {
            if let Ok(asset) = ud.borrow::<ImageAsset>() {
                SourceKind::Static {
                    bytes: asset.data.clone(),
                    width: asset.width,
                    height: asset.height,
                }
            } else if let Ok(dyn_h) = ud.borrow::<DynImgHandle>() {
                SourceKind::Dyn(dyn_h.inner.clone())
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

    AnimatedImage::new(lua, &xml_text, source)
}
