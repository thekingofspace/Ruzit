use std::cell::RefCell;
use std::io::Cursor;
use std::sync::{Arc, Mutex};

use mlua::{AnyUserData, Lua, MultiValue, UserData, UserDataFields, UserDataMethods, Value};

use crate::libs::asset::{ImageAsset, next_shader_id};
use crate::libs::renderable::{PartHandle, PartTextureRef};
use crate::libs::sfx::SoundData;

pub const VIDEO_EXTS: &[&str] = &["gif", "mp4", "mov"];

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
    if looks_like_gif(&bytes) {
        return parse_gif(bytes, source).map(|v| (v, None));
    }
    if looks_like_mp4_mov(&bytes) {
        return parse_mp4_mov(bytes, source);
    }
    Err(mlua::Error::RuntimeError(format!(
        "Video '{source}': unsupported format. Supported: animated GIF (.gif), MP4 (.mp4), QuickTime (.mov)"
    )))
}

fn looks_like_gif(b: &[u8]) -> bool {
    b.starts_with(b"GIF87a") || b.starts_with(b"GIF89a")
}

fn looks_like_mp4_mov(b: &[u8]) -> bool {
    b.len() >= 12 && &b[4..8] == b"ftyp"
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

fn parse_mp4_mov(bytes: Vec<u8>, source: String) -> mlua::Result<(VideoAsset, Option<Vec<u8>>)> {
    use std::process::Command;
    let mkerr =
        |msg: String| mlua::Error::RuntimeError(format!("Video '{source}' (mp4/mov): {msg}"));

    if !ffmpeg_on_path() {
        return Err(mkerr(
            "ffmpeg not found on PATH. MP4/MOV decoding is delegated to ffmpeg \
             at runtime; install it (https://ffmpeg.org/download.html) and re-try, \
             or convert the source to an animated GIF for the bundled decoder."
                .into(),
        ));
    }

    let tmp_dir = std::env::temp_dir().join(format!(
        "ruzit-video-{:x}-{}",
        std::process::id(),
        next_shader_id()
    ));
    std::fs::create_dir_all(&tmp_dir)
        .map_err(|e| mkerr(format!("create temp dir {}: {e}", tmp_dir.display())))?;
    let input_path = tmp_dir.join("input.bin");
    let audio_path = tmp_dir.join("audio.wav");
    std::fs::write(&input_path, &bytes).map_err(|e| mkerr(format!("write temp input: {e}")))?;

    let probe = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-print_format",
            "json",
            "-show_streams",
            "-show_entries",
            "stream=index,codec_type,width,height,r_frame_rate,nb_frames",
        ])
        .arg(&input_path)
        .output()
        .map_err(|e| mkerr(format!("spawn ffprobe: {e}")))?;
    if !probe.status.success() {
        let stderr = String::from_utf8_lossy(&probe.stderr).to_string();
        let _ = std::fs::remove_dir_all(&tmp_dir);
        return Err(mkerr(format!("ffprobe failed: {stderr}")));
    }
    let probe_json: serde_json::Value = serde_json::from_slice(&probe.stdout)
        .map_err(|e| mkerr(format!("parse ffprobe json: {e}")))?;
    let streams = probe_json
        .get("streams")
        .and_then(|s| s.as_array())
        .ok_or_else(|| mkerr("ffprobe returned no streams".into()))?;

    let mut video_stream: Option<&serde_json::Value> = None;
    let mut has_audio = false;
    for s in streams {
        match s.get("codec_type").and_then(|v| v.as_str()) {
            Some("video") => {
                if video_stream.is_none() {
                    video_stream = Some(s);
                }
            }
            Some("audio") => has_audio = true,
            _ => {}
        }
    }

    let video_stream = video_stream.ok_or_else(|| mkerr("file has no video stream".into()))?;
    let width = video_stream
        .get("width")
        .and_then(|v| v.as_u64())
        .ok_or_else(|| mkerr("video stream missing width".into()))? as u32;
    let height = video_stream
        .get("height")
        .and_then(|v| v.as_u64())
        .ok_or_else(|| mkerr("video stream missing height".into()))? as u32;
    let fps_str = video_stream
        .get("r_frame_rate")
        .and_then(|v| v.as_str())
        .unwrap_or("30/1");
    let fps = parse_rational(fps_str).unwrap_or(30.0).max(1.0);
    let frame_delay_ms = (1000.0 / fps).round().max(10.0) as u32;

    let video_out = Command::new("ffmpeg")
        .args(["-v", "error", "-i"])
        .arg(&input_path)
        .args(["-f", "rawvideo", "-pix_fmt", "rgba", "-an", "-"])
        .output()
        .map_err(|e| mkerr(format!("spawn ffmpeg (video): {e}")))?;
    if !video_out.status.success() {
        let stderr = String::from_utf8_lossy(&video_out.stderr).to_string();
        let _ = std::fs::remove_dir_all(&tmp_dir);
        return Err(mkerr(format!("ffmpeg video decode failed: {stderr}")));
    }

    let frame_size = (width as usize) * (height as usize) * 4;
    if frame_size == 0 || video_out.stdout.is_empty() {
        let _ = std::fs::remove_dir_all(&tmp_dir);
        return Err(mkerr("ffmpeg produced no video frames".into()));
    }
    let frame_count = video_out.stdout.len() / frame_size;
    if frame_count == 0 {
        let _ = std::fs::remove_dir_all(&tmp_dir);
        return Err(mkerr(format!(
            "decoded buffer is {} bytes but a single frame is {} bytes",
            video_out.stdout.len(),
            frame_size
        )));
    }
    let mut frames: Vec<Vec<u8>> = Vec::with_capacity(frame_count);
    for i in 0..frame_count {
        let start = i * frame_size;
        let end = start + frame_size;
        frames.push(video_out.stdout[start..end].to_vec());
    }

    let audio_bytes = if has_audio {
        let audio_status = Command::new("ffmpeg")
            .args(["-v", "error", "-y", "-i"])
            .arg(&input_path)
            .args(["-vn", "-c:a", "pcm_s16le"])
            .arg(&audio_path)
            .output()
            .map_err(|e| mkerr(format!("spawn ffmpeg (audio): {e}")))?;
        if !audio_status.status.success() {
            let stderr = String::from_utf8_lossy(&audio_status.stderr).to_string();
            eprintln!(
                "[Video] '{source}': audio extraction failed, video continues without audio: {stderr}"
            );
            None
        } else {
            std::fs::read(&audio_path).ok()
        }
    } else {
        None
    };

    let _ = std::fs::remove_dir_all(&tmp_dir);

    let video = VideoAsset {
        id: next_shader_id(),
        width,
        height,
        frames: Arc::new(frames),
        frame_delay_ms,
        source,
        state: Arc::new(Mutex::new(VideoState::default())),
        cached_image: Mutex::new(None),
    };
    Ok((video, audio_bytes))
}

fn ffmpeg_on_path() -> bool {
    std::process::Command::new("ffmpeg")
        .arg("-version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

fn parse_rational(s: &str) -> Option<f32> {
    if let Some((n, d)) = s.split_once('/') {
        let num: f32 = n.parse().ok()?;
        let den: f32 = d.parse().ok()?;
        if den.abs() < 1e-6 {
            return None;
        }
        Some(num / den)
    } else {
        s.parse().ok()
    }
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
                version: 0,
                live: None,
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
