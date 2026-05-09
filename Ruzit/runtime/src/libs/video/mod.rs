use std::cell::RefCell;
use std::io::Cursor;
use std::sync::{Arc, Mutex};

use mlua::{AnyUserData, Lua, MultiValue, UserData, UserDataFields, UserDataMethods, Value};

use crate::libs::asset::{ImageAsset, next_shader_id};
use crate::libs::renderable::{PartHandle, PartTextureRef};
use crate::libs::sfx::SoundData;

pub const VIDEO_EXTS: &[&str] = &["gif", "ruzitvid"];
const RUZITVID_MAGIC: &[u8] = b"RZVD";

thread_local! {
    static LINKED_PARTS: RefCell<Vec<LinkedPart>> = const { RefCell::new(Vec::new()) };
}

struct LinkedPart {
    video_state: Arc<Mutex<VideoState>>,
    frames: Arc<Vec<Vec<u8>>>,
    width: u32,
    height: u32,
    asset_id: u64,
    frame_delay_ms: u32,
    part: Arc<Mutex<crate::libs::renderable::PartState>>,
    last_frame_index: usize,
}

pub struct VideoAsset {
    pub id: u64,
    pub width: u32,
    pub height: u32,
    pub frames: Arc<Vec<Vec<u8>>>,
    pub frame_delay_ms: u32,
    pub source: String,
    pub state: Arc<Mutex<VideoState>>,
    pub cached_image: Mutex<Option<(usize, Arc<Vec<u8>>)>>,
}

#[derive(Clone, Copy)]
pub struct VideoState {
    pub current_frame: usize,
    pub elapsed_ms: f64,
    pub is_playing: bool,
    pub looped: bool,
}

impl Default for VideoState {
    fn default() -> Self {
        Self {
            current_frame: 0,
            elapsed_ms: 0.0,
            is_playing: false,
            looped: false,
        }
    }
}

impl UserData for VideoAsset {
    fn add_fields<F: UserDataFields<Self>>(f: &mut F) {
        f.add_field_method_get("Looped", |_, this| Ok(this.state.lock().unwrap().looped));
        f.add_field_method_set("Looped", |_, this, v: bool| {
            this.state.lock().unwrap().looped = v;
            Ok(())
        });
        f.add_field_method_get("CurrentFrame", |_, this| {
            Ok(this.state.lock().unwrap().current_frame as i64)
        });
        f.add_field_method_set("CurrentFrame", |_, this, v: i64| {
            let mut s = this.state.lock().unwrap();
            let n = this.frames.len().max(1);
            s.current_frame = (v.max(0) as usize) % n;
            s.elapsed_ms = 0.0;
            Ok(())
        });
    }

    fn add_methods<M: UserDataMethods<Self>>(m: &mut M) {
        m.add_method("Width", |_, this, _: ()| Ok(this.width as i64));
        m.add_method("Height", |_, this, _: ()| Ok(this.height as i64));
        m.add_method("FrameCount", |_, this, _: ()| Ok(this.frames.len() as i64));
        m.add_method("FrameDelayMs", |_, this, _: ()| {
            Ok(this.frame_delay_ms as i64)
        });
        m.add_method("Length", |_, this, _: ()| {
            Ok(this.frames.len() as f64 * this.frame_delay_ms as f64 / 1000.0)
        });
        m.add_method("Source", |_, this, _: ()| Ok(this.source.clone()));
        m.add_method("Play", |_, this, _: ()| -> mlua::Result<()> {
            let mut s = this.state.lock().unwrap();
            s.is_playing = true;
            Ok(())
        });
        m.add_method("Pause", |_, this, _: ()| -> mlua::Result<()> {
            this.state.lock().unwrap().is_playing = false;
            Ok(())
        });
        m.add_method("Stop", |_, this, _: ()| -> mlua::Result<()> {
            let mut s = this.state.lock().unwrap();
            s.is_playing = false;
            s.current_frame = 0;
            s.elapsed_ms = 0.0;
            Ok(())
        });
        m.add_method(
            "CurrentImage",
            |lua, this, _: ()| -> mlua::Result<AnyUserData> {
                let frame_idx = this.state.lock().unwrap().current_frame;
                let frame_idx = frame_idx.min(this.frames.len().saturating_sub(1));
                let mut cached = this.cached_image.lock().unwrap();
                let data = if let Some((idx, data)) = cached.as_ref() {
                    if *idx == frame_idx {
                        data.clone()
                    } else {
                        let d = Arc::new(this.frames[frame_idx].clone());
                        *cached = Some((frame_idx, d.clone()));
                        d
                    }
                } else {
                    let d = Arc::new(this.frames[frame_idx].clone());
                    *cached = Some((frame_idx, d.clone()));
                    d
                };
                drop(cached);
                let img = ImageAsset {
                    id: next_shader_id(),
                    width: this.width,
                    height: this.height,
                    data,
                    source: format!("<video-frame:{}:{}>", this.source, frame_idx),
                };
                lua.create_userdata(img)
            },
        );

        m.add_method(
            "LinkPart",
            |_, this, part: AnyUserData| -> mlua::Result<()> {
                let handle = part.borrow::<PartHandle>().map_err(|_| {
                    mlua::Error::RuntimeError("Video:LinkPart: argument must be a BasePart".into())
                })?;
                LINKED_PARTS.with(|c| {
                    let mut reg = c.borrow_mut();
                    reg.retain(|l| !Arc::ptr_eq(&l.part, &handle.state));
                    reg.push(LinkedPart {
                        video_state: this.state.clone(),
                        frames: this.frames.clone(),
                        width: this.width,
                        height: this.height,
                        asset_id: this.id,
                        frame_delay_ms: this.frame_delay_ms,
                        part: handle.state.clone(),
                        last_frame_index: usize::MAX,
                    });
                });
                Ok(())
            },
        );

        m.add_method(
            "UnlinkPart",
            |_, _this, part: AnyUserData| -> mlua::Result<()> {
                let handle = part.borrow::<PartHandle>().map_err(|_| {
                    mlua::Error::RuntimeError(
                        "Video:UnlinkPart: argument must be a BasePart".into(),
                    )
                })?;
                LINKED_PARTS.with(|c| {
                    c.borrow_mut()
                        .retain(|l| !Arc::ptr_eq(&l.part, &handle.state));
                });
                Ok(())
            },
        );
    }
}

pub fn parse_video_bytes(
    bytes: Vec<u8>,
    source: String,
) -> mlua::Result<(VideoAsset, Option<Vec<u8>>)> {
    if bytes.starts_with(RUZITVID_MAGIC) {
        return parse_ruzitvid(bytes, source);
    }
    if looks_like_gif(&bytes) {
        return parse_gif(bytes, source).map(|v| (v, None));
    }
    Err(mlua::Error::RuntimeError(format!(
        "Video '{source}': unsupported format. Supported: animated GIF (.gif), Ruzit Video (.ruzitvid)"
    )))
}

fn looks_like_gif(b: &[u8]) -> bool {
    b.starts_with(b"GIF87a") || b.starts_with(b"GIF89a")
}

fn parse_gif(bytes: Vec<u8>, source: String) -> mlua::Result<VideoAsset> {
    use image::AnimationDecoder;
    let cursor = Cursor::new(bytes);
    let decoder = image::codecs::gif::GifDecoder::new(cursor)
        .map_err(|e| mlua::Error::RuntimeError(format!("Video '{source}' gif decode: {e}")))?;
    let frames = decoder
        .into_frames()
        .collect_frames()
        .map_err(|e| mlua::Error::RuntimeError(format!("Video '{source}' frames: {e}")))?;

    if frames.is_empty() {
        return Err(mlua::Error::RuntimeError(format!(
            "Video '{source}': GIF has no frames"
        )));
    }

    let first_buf = frames[0].buffer();
    let width = first_buf.width();
    let height = first_buf.height();
    let mut frame_data: Vec<Vec<u8>> = Vec::with_capacity(frames.len());
    let mut total_delay: u32 = 0;
    let mut delay_count: u32 = 0;
    for f in &frames {
        let buf = f.buffer();
        if buf.width() != width || buf.height() != height {
            return Err(mlua::Error::RuntimeError(format!(
                "Video '{source}': GIF frame size mismatch (engine requires uniform dimensions)"
            )));
        }
        frame_data.push(buf.as_raw().clone());
        let (num, _denom) = f.delay().numer_denom_ms();
        total_delay = total_delay.saturating_add(num);
        delay_count = delay_count.saturating_add(1);
    }
    let avg_delay = if delay_count > 0 {
        (total_delay / delay_count).max(10)
    } else {
        100
    };

    Ok(VideoAsset {
        id: next_shader_id(),
        width,
        height,
        frames: Arc::new(frame_data),
        frame_delay_ms: avg_delay,
        source,
        state: Arc::new(Mutex::new(VideoState::default())),
        cached_image: Mutex::new(None),
    })
}

fn parse_ruzitvid(bytes: Vec<u8>, source: String) -> mlua::Result<(VideoAsset, Option<Vec<u8>>)> {
    let err = |msg: &str| mlua::Error::RuntimeError(format!("Video '{source}' (.ruzitvid): {msg}"));
    if bytes.len() < 4 + 16 {
        return Err(err("file too small"));
    }
    let mut offset = 4;
    let read_u32 =
        |b: &[u8], o: usize| -> u32 { u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]]) };
    let width = read_u32(&bytes, offset);
    offset += 4;
    let height = read_u32(&bytes, offset);
    offset += 4;
    let frame_count = read_u32(&bytes, offset) as usize;
    offset += 4;
    let frame_delay_ms = read_u32(&bytes, offset);
    offset += 4;

    let frame_size = (width as usize) * (height as usize) * 4;
    if bytes.len() < offset + frame_size * frame_count + 4 {
        return Err(err("truncated payload"));
    }

    let mut frames: Vec<Vec<u8>> = Vec::with_capacity(frame_count);
    for _ in 0..frame_count {
        frames.push(bytes[offset..offset + frame_size].to_vec());
        offset += frame_size;
    }

    let audio_len = read_u32(&bytes, offset) as usize;
    offset += 4;
    if bytes.len() < offset + audio_len {
        return Err(err("audio payload truncated"));
    }
    let audio = if audio_len > 0 {
        Some(bytes[offset..offset + audio_len].to_vec())
    } else {
        None
    };

    let video = VideoAsset {
        id: next_shader_id(),
        width,
        height,
        frames: Arc::new(frames),
        frame_delay_ms: frame_delay_ms.max(1),
        source,
        state: Arc::new(Mutex::new(VideoState::default())),
        cached_image: Mutex::new(None),
    };
    Ok((video, audio))
}

pub fn create_get_video_function(
    lua: &Lua,
    fs: crate::vfs::Fs,
    owner: String,
) -> mlua::Result<mlua::Function> {
    lua.create_function(move |lua, path: String| -> mlua::Result<MultiValue> {
        let (bytes, source) = crate::libs::asset::read_video_bytes(&fs, &owner, &path)?;
        let (video, audio_bytes) = parse_video_bytes(bytes, source.clone())?;
        let mut out = MultiValue::new();
        out.push_back(Value::UserData(lua.create_userdata(video)?));
        if let Some(audio) = audio_bytes {
            let sound = SoundData {
                id: next_shader_id(),
                bytes: Arc::new(audio),
                source: format!("{source}#audio"),
            };
            out.push_back(Value::UserData(lua.create_userdata(sound)?));
        } else {
            out.push_back(Value::Nil);
        }
        Ok(out)
    })
}

pub fn tick(dt_ms: f64) {
    if dt_ms <= 0.0 {
        return;
    }
    LINKED_PARTS.with(|c| {
        let mut reg = c.borrow_mut();
        reg.retain(|l| l.part.lock().unwrap().alive);
        let mut advanced: std::collections::HashSet<usize> = std::collections::HashSet::new();
        for l in reg.iter_mut() {
            let key = Arc::as_ptr(&l.video_state) as usize;
            if advanced.insert(key) {
                advance_state(&l.video_state, l.frames.len(), l.frame_delay_ms, dt_ms);
            }
            let frame_idx = l.video_state.lock().unwrap().current_frame;
            let frame_idx = frame_idx.min(l.frames.len().saturating_sub(1));
            if frame_idx == l.last_frame_index {
                continue;
            }
            l.last_frame_index = frame_idx;
            let mut p = l.part.lock().unwrap();
            if !p.alive {
                continue;
            }
            p.texture = Some(PartTextureRef {
                id: l.asset_id,
                width: l.width,
                height: l.height,
                data: Arc::new(l.frames[frame_idx].clone()),
            });
        }
    });
}

fn advance_state(
    state_arc: &Arc<Mutex<VideoState>>,
    frame_count: usize,
    frame_delay_ms: u32,
    dt_ms: f64,
) {
    if frame_count == 0 || frame_delay_ms == 0 {
        return;
    }
    let mut s = state_arc.lock().unwrap();
    if !s.is_playing {
        return;
    }
    s.elapsed_ms += dt_ms;
    while s.elapsed_ms >= frame_delay_ms as f64 {
        s.elapsed_ms -= frame_delay_ms as f64;
        s.current_frame += 1;
        if s.current_frame >= frame_count {
            if s.looped {
                s.current_frame = 0;
            } else {
                s.current_frame = frame_count - 1;
                s.is_playing = false;
                s.elapsed_ms = 0.0;
                break;
            }
        }
    }
}
