use std::cell::RefCell;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{Stream, StreamConfig};
use mlua::{AnyUserData, Lua, MultiValue, Table, UserData, UserDataFields, UserDataMethods, Value};
use opus::{Application as OpusApplication, Channels as OpusChannels, Decoder, Encoder};
use rodio::{OutputStream, OutputStreamHandle, Sink, Source};

use crate::libs::primitives::read_xyz_from_args;
use crate::libs::signal;

const SAMPLE_RATE: u32 = 48000;
const FRAME_MS: u32 = 20;
const FRAME_SAMPLES: usize = (SAMPLE_RATE * FRAME_MS / 1000) as usize;
const OUTPUT_CHANNELS: u16 = 2;

thread_local! {
    static OUTPUT: RefCell<Option<OutputStreamHandle>> = const { RefCell::new(None) };
    static CAPTURES: RefCell<Vec<CaptureRegistration>> = const { RefCell::new(Vec::new()) };
    static PENDING_RECORDS: RefCell<Vec<PendingRecord>> = const { RefCell::new(Vec::new()) };
}

struct CaptureRegistration {
    rx: std::sync::mpsc::Receiver<Vec<u8>>,
    signal_table: Table,
    alive: Arc<AtomicBool>,
}

struct PendingRecord {
    rx: std::sync::mpsc::Receiver<Vec<u8>>,
    alive: Arc<AtomicBool>,
    stream: Mutex<Option<Stream>>,
    deadline: std::time::Instant,
    packets: Vec<Vec<u8>>,
    signal_table: Table,
}

#[derive(Clone, Copy, Debug)]
pub enum VoiceShader {
    Volume(f32),
    Speed(f32),
    Spatial {
        x: f32,
        y: f32,
        z: f32,
        min_falloff: f32,
        max_falloff: f32,
    },
}

impl UserData for VoiceShader {}

pub fn create(lua: &Lua) -> mlua::Result<Table> {
    let t = lua.create_table()?;
    t.set(
        "StartCapture",
        lua.create_function(|lua, _: ()| -> mlua::Result<AnyUserData> { start_capture(lua) })?,
    )?;
    t.set(
        "CreateChannel",
        lua.create_function(|lua, _: ()| -> mlua::Result<AnyUserData> { create_channel(lua) })?,
    )?;
    t.set(
        "Volume",
        lua.create_function(|_, factor: f32| Ok(VoiceShader::Volume(factor.max(0.0))))?,
    )?;
    t.set(
        "Speed",
        lua.create_function(|_, factor: f32| Ok(VoiceShader::Speed(factor.clamp(0.25, 4.0))))?,
    )?;
    t.set(
        "Spatial",
        lua.create_function(|_, args: MultiValue| -> mlua::Result<VoiceShader> {
            let ((x, y, z), tail) = read_xyz_from_args(args)?;
            let max_falloff = match tail {
                Some(Value::Number(n)) => n as f32,
                Some(Value::Integer(n)) => n as f32,
                _ => 8.0,
            };
            Ok(VoiceShader::Spatial {
                x,
                y,
                z,
                min_falloff: 0.0,
                max_falloff: max_falloff.max(0.1),
            })
        })?,
    )?;
    t.set(
        "ListenerPosition",
        lua.create_function(|_, args: MultiValue| -> mlua::Result<()> {
            let ((x, y, z), _) = read_xyz_from_args(args)?;
            LISTENER_POS.with(|c| *c.borrow_mut() = (x, y, z));
            Ok(())
        })?,
    )?;
    t.set(
        "NewRecorder",
        lua.create_function(|lua, _: ()| -> mlua::Result<AnyUserData> {
            lua.create_userdata(Recorder {
                packets: Arc::new(Mutex::new(Vec::new())),
            })
        })?,
    )?;
    t.set(
        "LoadRecording",
        lua.create_function(|lua, data: mlua::String| -> mlua::Result<AnyUserData> {
            deserialize_recording(lua, data.as_bytes().as_ref())
        })?,
    )?;
    t.set(
        "Record",
        lua.create_function(|lua, duration: f64| -> mlua::Result<Table> {
            record_one_shot(lua, duration)
        })?,
    )?;
    t.set(
        "PlayRecording",
        lua.create_function(
            |lua, (data, opts): (Value, Option<Table>)| -> mlua::Result<AnyUserData> {
                let recording_ud = match data {
                    Value::UserData(ud) => {
                        if ud.is::<Recording>() {
                            ud
                        } else {
                            return Err(mlua::Error::RuntimeError(
                                "Voice.PlayRecording: userdata must be a Recording".into(),
                            ));
                        }
                    }
                    Value::String(s) => deserialize_recording(lua, s.as_bytes().as_ref())?,
                    _ => {
                        return Err(mlua::Error::RuntimeError(
                            "Voice.PlayRecording: expected serialized bytes or a Recording"
                                .into(),
                        ));
                    }
                };
                let channel_ud = create_channel(lua)?;
                {
                    let recording = recording_ud.borrow::<Recording>()?;
                    let channel = channel_ud.borrow::<ChannelHandle>()?;
                    let looped = opts
                        .as_ref()
                        .and_then(|t| t.get::<bool>("loop").ok())
                        .unwrap_or(false);
                    let speed: f32 = opts
                        .as_ref()
                        .and_then(|t| t.get::<f32>("speed").ok())
                        .unwrap_or(1.0)
                        .clamp(0.25, 4.0);
                    RECORDING_PLAYBACKS.with(|c| {
                        c.borrow_mut().push(RecordingPlayback {
                            packets: recording.packets.clone(),
                            index: 0,
                            accumulated_ms: 0.0,
                            last_tick: std::time::Instant::now(),
                            looped,
                            speed,
                            decoder: channel.decoder.clone(),
                            queue: channel.queue.clone(),
                        });
                    });
                    let handle = output_handle()?;
                    let sink = Sink::try_new(&handle).map_err(|e| {
                        mlua::Error::RuntimeError(format!("Voice sink: {e}"))
                    })?;
                    sink.append(VoiceSource {
                        queue: channel.queue.clone(),
                        shaders: channel.shaders.clone(),
                        position: channel.position.clone(),
                        next_channel: 0,
                        last_l: 0.0,
                        last_r: 0.0,
                        speed_phase: 0.0,
                    });
                    sink.play();
                    *channel.sink.lock().unwrap() = Some(sink);
                    channel.playing.store(true, Ordering::Relaxed);
                }
                Ok(channel_ud)
            },
        )?,
    )?;
    t.set(
        "PlayPacket",
        lua.create_function(
            |lua, (packet, opts): (mlua::String, Option<Table>)| -> mlua::Result<AnyUserData> {
                let channel_ud = create_channel(lua)?;
                {
                    let channel = channel_ud.borrow::<ChannelHandle>()?;
                    if let Some(opts) = &opts {
                        if let (Ok(x), Ok(y), Ok(z)) = (
                            opts.get::<f32>("x"),
                            opts.get::<f32>("y"),
                            opts.get::<f32>("z"),
                        ) {
                            let max_falloff =
                                opts.get::<f32>("falloff").unwrap_or(8.0).max(0.1);
                            let min_falloff = opts.get::<f32>("min_falloff").unwrap_or(0.0).max(0.0);
                            let mut shaders = channel.shaders.lock().unwrap();
                            shaders.push(VoiceShader::Spatial {
                                x,
                                y,
                                z,
                                min_falloff,
                                max_falloff,
                            });
                            *channel.position.lock().unwrap() = (x, y, z);
                        }
                    }
                    let bytes = packet.as_bytes();
                    let mut pcm = vec![0_f32; FRAME_SAMPLES * 6];
                    let n = match channel
                        .decoder
                        .lock()
                        .unwrap()
                        .decode_float(&bytes, &mut pcm, false)
                    {
                        Ok(n) => n,
                        Err(e) => {
                            eprintln!("[voice] PlayPacket decode err: {e}");
                            0
                        }
                    };
                    channel
                        .queue
                        .lock()
                        .unwrap()
                        .extend(pcm[..n].iter().copied());
                    let handle = output_handle()?;
                    let sink = Sink::try_new(&handle)
                        .map_err(|e| mlua::Error::RuntimeError(format!("Voice sink: {e}")))?;
                    sink.append(VoiceSource {
                        queue: channel.queue.clone(),
                        shaders: channel.shaders.clone(),
                        position: channel.position.clone(),
                        next_channel: 0,
                        last_l: 0.0,
                        last_r: 0.0,
                        speed_phase: 0.0,
                    });
                    sink.play();
                    *channel.sink.lock().unwrap() = Some(sink);
                    channel.playing.store(true, Ordering::Relaxed);
                }
                Ok(channel_ud)
            },
        )?,
    )?;
    Ok(t)
}

thread_local! {
    static LISTENER_POS: RefCell<(f32, f32, f32)> = const { RefCell::new((0.0, 0.0, 0.0)) };
}

fn output_handle() -> mlua::Result<OutputStreamHandle> {
    OUTPUT.with(|cell| {
        let mut borrow = cell.borrow_mut();
        if borrow.is_none() {
            let (stream, handle) = OutputStream::try_default()
                .map_err(|e| mlua::Error::RuntimeError(format!("Voice audio init: {e}")))?;
            std::mem::forget(stream);
            *borrow = Some(handle);
        }
        Ok(borrow.as_ref().unwrap().clone())
    })
}

fn start_capture(lua: &Lua) -> mlua::Result<AnyUserData> {
    let host = cpal::default_host();
    let device = host.default_input_device().ok_or_else(|| {
        mlua::Error::RuntimeError("Voice.StartCapture: no default input device".into())
    })?;
    let supported = device
        .default_input_config()
        .map_err(|e| mlua::Error::RuntimeError(format!("Voice.StartCapture: query input: {e}")))?;
    let device_rate = supported.sample_rate().0;
    let device_channels = supported.channels();
    let cfg = StreamConfig {
        channels: device_channels,
        sample_rate: cpal::SampleRate(device_rate),
        buffer_size: cpal::BufferSize::Default,
    };

    let (tx, rx) = std::sync::mpsc::channel::<Vec<u8>>();
    let alive = Arc::new(AtomicBool::new(true));
    let paused = Arc::new(AtomicBool::new(false));
    let threshold = Arc::new(Mutex::new(0.0_f32));
    let alive_cb = alive.clone();
    let paused_cb = paused.clone();
    let threshold_cb = threshold.clone();

    let mut encoder = Encoder::new(SAMPLE_RATE, OpusChannels::Mono, OpusApplication::Voip)
        .map_err(|e| mlua::Error::RuntimeError(format!("Voice opus encoder: {e}")))?;
    let mut frame_buf: Vec<f32> = Vec::with_capacity(FRAME_SAMPLES * 2);
    let mut packet_buf = vec![0u8; 4000];

    let stream = device
        .build_input_stream(
            &cfg,
            move |data: &[f32], _: &cpal::InputCallbackInfo| {
                if !alive_cb.load(Ordering::Relaxed) {
                    return;
                }
                if paused_cb.load(Ordering::Relaxed) {
                    return;
                }
                let mono: Vec<f32> = if device_channels == 1 {
                    data.to_vec()
                } else {
                    data.chunks(device_channels as usize)
                        .map(|c| c.iter().sum::<f32>() / c.len() as f32)
                        .collect()
                };
                let resampled = if device_rate == SAMPLE_RATE {
                    mono
                } else {
                    linear_resample(&mono, device_rate, SAMPLE_RATE)
                };
                frame_buf.extend(resampled);
                while frame_buf.len() >= FRAME_SAMPLES {
                    let frame: Vec<f32> = frame_buf.drain(..FRAME_SAMPLES).collect();
                    let thresh = *threshold_cb.lock().unwrap();
                    if thresh > 0.0 {
                        let peak = frame.iter().fold(0.0_f32, |acc, &s| acc.max(s.abs()));
                        if peak < thresh {
                            continue;
                        }
                    }
                    match encoder.encode_float(&frame, &mut packet_buf) {
                        Ok(n) => {
                            if tx.send(packet_buf[..n].to_vec()).is_err() {
                                return;
                            }
                        }
                        Err(e) => {
                            eprintln!("[voice] opus encode: {e}");
                        }
                    }
                }
            },
            move |err| eprintln!("[voice] capture err: {err}"),
            None,
        )
        .map_err(|e| mlua::Error::RuntimeError(format!("Voice.StartCapture: build: {e}")))?;
    stream
        .play()
        .map_err(|e| mlua::Error::RuntimeError(format!("Voice.StartCapture: play: {e}")))?;

    let signal_table = signal::new_instance(lua)?;
    CAPTURES.with(|c| {
        c.borrow_mut().push(CaptureRegistration {
            rx,
            signal_table: signal_table.clone(),
            alive: alive.clone(),
        });
    });

    let handle = CaptureHandle {
        stream: Mutex::new(Some(stream)),
        signal_table,
        alive,
        paused,
        threshold,
    };
    lua.create_userdata(handle)
}

pub struct CaptureHandle {
    stream: Mutex<Option<Stream>>,
    signal_table: Table,
    alive: Arc<AtomicBool>,
    paused: Arc<AtomicBool>,
    threshold: Arc<Mutex<f32>>,
}

unsafe impl Send for CaptureHandle {}
unsafe impl Sync for CaptureHandle {}

impl UserData for CaptureHandle {
    fn add_fields<F: UserDataFields<Self>>(f: &mut F) {
        f.add_field_method_get("OnPacket", |_, this| Ok(this.signal_table.clone()));
    }
    fn add_methods<M: UserDataMethods<Self>>(m: &mut M) {
        m.add_method("Stop", |_, this, _: ()| -> mlua::Result<()> {
            this.alive.store(false, Ordering::Relaxed);
            this.stream.lock().unwrap().take();
            Ok(())
        });
        m.add_method("IsActive", |_, this, _: ()| {
            Ok(this.alive.load(Ordering::Relaxed))
        });
        m.add_method("Pause", |_, this, _: ()| -> mlua::Result<()> {
            this.paused.store(true, Ordering::Relaxed);
            Ok(())
        });
        m.add_method("Resume", |_, this, _: ()| -> mlua::Result<()> {
            this.paused.store(false, Ordering::Relaxed);
            Ok(())
        });
        m.add_method("IsPaused", |_, this, _: ()| {
            Ok(this.paused.load(Ordering::Relaxed))
        });
        m.add_method("SetActive", |_, this, on: bool| -> mlua::Result<()> {
            this.paused.store(!on, Ordering::Relaxed);
            Ok(())
        });
        m.add_method("SetThreshold", |_, this, amp: f32| -> mlua::Result<()> {
            *this.threshold.lock().unwrap() = amp.max(0.0);
            Ok(())
        });
        m.add_method("GetThreshold", |_, this, _: ()| {
            Ok(*this.threshold.lock().unwrap())
        });
    }
}

fn create_channel(lua: &Lua) -> mlua::Result<AnyUserData> {
    let _ = output_handle()?;
    let queue = Arc::new(Mutex::new(VecDeque::<f32>::new()));
    let shaders = Arc::new(Mutex::new(Vec::<VoiceShader>::new()));
    let position = Arc::new(Mutex::new((0.0_f32, 0.0_f32, 0.0_f32)));
    let decoder = Arc::new(Mutex::new(
        Decoder::new(SAMPLE_RATE, OpusChannels::Mono)
            .map_err(|e| mlua::Error::RuntimeError(format!("Voice decoder: {e}")))?,
    ));

    let handle = ChannelHandle {
        queue,
        shaders,
        position,
        decoder,
        sink: Mutex::new(None),
        playing: Arc::new(AtomicBool::new(false)),
    };
    lua.create_userdata(handle)
}

pub struct ChannelHandle {
    queue: Arc<Mutex<VecDeque<f32>>>,
    pub(crate) shaders: Arc<Mutex<Vec<VoiceShader>>>,
    pub(crate) position: Arc<Mutex<(f32, f32, f32)>>,
    decoder: Arc<Mutex<Decoder>>,
    sink: Mutex<Option<Sink>>,
    playing: Arc<AtomicBool>,
}

unsafe impl Send for ChannelHandle {}
unsafe impl Sync for ChannelHandle {}

fn voice_get_scalar(
    shaders: &Arc<Mutex<Vec<VoiceShader>>>,
    pick: impl Fn(&VoiceShader) -> Option<f32>,
) -> Option<f32> {
    shaders.lock().unwrap().iter().rev().find_map(pick)
}

fn voice_set_scalar(shaders: &Arc<Mutex<Vec<VoiceShader>>>, new: VoiceShader) {
    let kind = std::mem::discriminant(&new);
    let mut list = shaders.lock().unwrap();
    list.retain(|s| std::mem::discriminant(s) != kind);
    list.push(new);
}

fn voice_spatial_get(
    shaders: &Arc<Mutex<Vec<VoiceShader>>>,
) -> Option<(f32, f32, f32, f32, f32)> {
    shaders.lock().unwrap().iter().rev().find_map(|s| match s {
        VoiceShader::Spatial {
            x,
            y,
            z,
            min_falloff,
            max_falloff,
        } => Some((*x, *y, *z, *min_falloff, *max_falloff)),
        _ => None,
    })
}

fn voice_spatial_update(
    shaders: &Arc<Mutex<Vec<VoiceShader>>>,
    pos_arc: &Arc<Mutex<(f32, f32, f32)>>,
    x: f32,
    y: f32,
    z: f32,
    min_falloff: f32,
    max_falloff: f32,
) {
    let mut list = shaders.lock().unwrap();
    list.retain(|s| !matches!(s, VoiceShader::Spatial { .. }));
    list.push(VoiceShader::Spatial {
        x,
        y,
        z,
        min_falloff: min_falloff.max(0.0),
        max_falloff: max_falloff.max(min_falloff.max(0.0)),
    });
    *pos_arc.lock().unwrap() = (x, y, z);
}

impl UserData for ChannelHandle {
    fn add_fields<F: UserDataFields<Self>>(f: &mut F) {
        f.add_field_method_get("IsPlaying", |_, this| {
            Ok(this.playing.load(Ordering::Relaxed))
        });

        f.add_field_method_get("Volume", |_, this| {
            Ok(voice_get_scalar(&this.shaders, |s| match s {
                VoiceShader::Volume(v) => Some(*v),
                _ => None,
            })
            .unwrap_or(1.0))
        });
        f.add_field_method_set("Volume", |_, this, v: f32| {
            voice_set_scalar(&this.shaders, VoiceShader::Volume(v.max(0.0)));
            Ok(())
        });

        f.add_field_method_get("Speed", |_, this| {
            Ok(voice_get_scalar(&this.shaders, |s| match s {
                VoiceShader::Speed(v) => Some(*v),
                _ => None,
            })
            .unwrap_or(1.0))
        });
        f.add_field_method_set("Speed", |_, this, v: f32| {
            voice_set_scalar(&this.shaders, VoiceShader::Speed(v.max(0.0)));
            Ok(())
        });

        f.add_field_method_get("Pitch", |_, this| {
            Ok(voice_get_scalar(&this.shaders, |s| match s {
                VoiceShader::Speed(v) => Some(*v),
                _ => None,
            })
            .unwrap_or(1.0))
        });
        f.add_field_method_set("Pitch", |_, this, v: f32| {
            voice_set_scalar(&this.shaders, VoiceShader::Speed(v.max(0.0)));
            Ok(())
        });

        f.add_field_method_get("Position", |_, this| -> mlua::Result<Option<crate::libs::primitives::Vector>> {
            Ok(voice_spatial_get(&this.shaders).map(|(x, y, z, _, _)| {
                crate::libs::primitives::Vector::new(x, y, z)
            }))
        });
        f.add_field_method_set("Position", |_, this, v: mlua::Value| {
            match v {
                mlua::Value::Nil => {
                    this.shaders
                        .lock()
                        .unwrap()
                        .retain(|s| !matches!(s, VoiceShader::Spatial { .. }));
                }
                other => {
                    let vec = crate::libs::primitives::value_to_vector_opt(&other)
                        .ok_or_else(|| {
                            mlua::Error::RuntimeError(
                                "VoiceChannel.Position expects a Vector or nil".into(),
                            )
                        })?;
                    let (min_f, max_f) = voice_spatial_get(&this.shaders)
                        .map(|(_, _, _, mn, mx)| (mn, mx))
                        .unwrap_or((0.0, 8.0));
                    voice_spatial_update(&this.shaders, &this.position, vec.x, vec.y, vec.z, min_f, max_f);
                }
            }
            Ok(())
        });

        f.add_field_method_get("MinFalloff", |_, this| {
            Ok(voice_spatial_get(&this.shaders)
                .map(|(_, _, _, mn, _)| mn)
                .unwrap_or(0.0))
        });
        f.add_field_method_set("MinFalloff", |_, this, v: f32| {
            let v = v.max(0.0);
            let (x, y, z, _, max_f) =
                voice_spatial_get(&this.shaders).unwrap_or((0.0, 0.0, 0.0, 0.0, 8.0));
            voice_spatial_update(&this.shaders, &this.position, x, y, z, v, max_f.max(v));
            Ok(())
        });

        f.add_field_method_get("MaxFalloff", |_, this| {
            Ok(voice_spatial_get(&this.shaders)
                .map(|(_, _, _, _, mx)| mx)
                .unwrap_or(8.0))
        });
        f.add_field_method_set("MaxFalloff", |_, this, v: f32| {
            let v = v.max(0.0);
            let (x, y, z, min_f, _) =
                voice_spatial_get(&this.shaders).unwrap_or((0.0, 0.0, 0.0, 0.0, 8.0));
            voice_spatial_update(&this.shaders, &this.position, x, y, z, min_f.min(v), v);
            Ok(())
        });
    }
    fn add_methods<M: UserDataMethods<Self>>(m: &mut M) {
        m.add_method(
            "Push",
            |_, this, packet: mlua::String| -> mlua::Result<()> {
                let bytes = packet.as_bytes();
                let mut pcm = vec![0f32; FRAME_SAMPLES * 6];
                let n = match this
                    .decoder
                    .lock()
                    .unwrap()
                    .decode_float(&bytes, &mut pcm, false)
                {
                    Ok(n) => n,
                    Err(e) => {
                        eprintln!("[voice] decode err: {e}");
                        return Ok(());
                    }
                };
                let mut q = this.queue.lock().unwrap();
                q.extend(pcm[..n].iter().copied());
                if q.len() > SAMPLE_RATE as usize * 2 {
                    let drop = q.len() - SAMPLE_RATE as usize;
                    q.drain(..drop);
                }
                Ok(())
            },
        );
        m.add_method("Play", |_, this, _: ()| -> mlua::Result<()> {
            if this.playing.load(Ordering::Relaxed) {
                return Ok(());
            }
            let handle = output_handle()?;
            let sink = Sink::try_new(&handle)
                .map_err(|e| mlua::Error::RuntimeError(format!("Voice sink: {e}")))?;
            sink.append(VoiceSource {
                queue: this.queue.clone(),
                shaders: this.shaders.clone(),
                position: this.position.clone(),
                next_channel: 0,
                last_l: 0.0,
                last_r: 0.0,
                speed_phase: 0.0,
            });
            sink.play();
            *this.sink.lock().unwrap() = Some(sink);
            this.playing.store(true, Ordering::Relaxed);
            Ok(())
        });
        m.add_method("Stop", |_, this, _: ()| -> mlua::Result<()> {
            if let Some(sink) = this.sink.lock().unwrap().take() {
                sink.stop();
            }
            this.queue.lock().unwrap().clear();
            this.playing.store(false, Ordering::Relaxed);
            Ok(())
        });
        m.add_method(
            "ApplyShader",
            |_, this, shader: AnyUserData| -> mlua::Result<()> {
                let s = *shader.borrow::<VoiceShader>()?;
                this.shaders.lock().unwrap().push(s);
                Ok(())
            },
        );
        m.add_method("ClearShaders", |_, this, _: ()| -> mlua::Result<()> {
            this.shaders.lock().unwrap().clear();
            Ok(())
        });
        m.add_method(
            "SetPosition",
            |_, this, args: MultiValue| -> mlua::Result<()> {
                let first_is_nil = args
                    .iter()
                    .next()
                    .map(|v| matches!(v, Value::Nil))
                    .unwrap_or(true);
                if args.is_empty() || first_is_nil {
                    this.shaders
                        .lock()
                        .unwrap()
                        .retain(|s| !matches!(s, VoiceShader::Spatial { .. }));
                    *this.position.lock().unwrap() = (0.0, 0.0, 0.0);
                    return Ok(());
                }
                let ((x, y, z), _) = read_xyz_from_args(args)?;
                *this.position.lock().unwrap() = (x, y, z);
                Ok(())
            },
        );
        m.add_method(
            "SetSpatial",
            |_, this, args: MultiValue| -> mlua::Result<()> {
                let first_is_nil = args
                    .iter()
                    .next()
                    .map(|v| matches!(v, Value::Nil))
                    .unwrap_or(true);
                if args.is_empty() || first_is_nil {
                    this.shaders
                        .lock()
                        .unwrap()
                        .retain(|s| !matches!(s, VoiceShader::Spatial { .. }));
                    return Ok(());
                }
                let ((x, y, z), tail) = read_xyz_from_args(args)?;
                let max_falloff = match tail {
                    Some(Value::Number(n)) => n as f32,
                    Some(Value::Integer(n)) => n as f32,
                    _ => 8.0,
                };
                let mut shaders = this.shaders.lock().unwrap();
                let prior_min = shaders.iter().find_map(|s| match s {
                    VoiceShader::Spatial { min_falloff, .. } => Some(*min_falloff),
                    _ => None,
                });
                shaders.retain(|s| !matches!(s, VoiceShader::Spatial { .. }));
                shaders.push(VoiceShader::Spatial {
                    x,
                    y,
                    z,
                    min_falloff: prior_min.unwrap_or(0.0),
                    max_falloff: max_falloff.max(0.1),
                });
                *this.position.lock().unwrap() = (x, y, z);
                Ok(())
            },
        );
    }
}

struct VoiceSource {
    queue: Arc<Mutex<VecDeque<f32>>>,
    shaders: Arc<Mutex<Vec<VoiceShader>>>,
    position: Arc<Mutex<(f32, f32, f32)>>,
    next_channel: u8,
    last_l: f32,
    last_r: f32,
    speed_phase: f32,
}

impl Iterator for VoiceSource {
    type Item = f32;
    fn next(&mut self) -> Option<f32> {
        if self.next_channel == 1 {
            self.next_channel = 0;
            return Some(self.last_r);
        }
        let raw = {
            let mut q = self.queue.lock().unwrap();
            q.pop_front().unwrap_or(0.0)
        };

        let mut volume = 1.0_f32;
        let mut speed = 1.0_f32;
        let mut pan = 0.0_f32;
        let mut atten = 1.0_f32;
        let listener_override = LISTENER_POS.with(|c| *c.borrow());
        let listener = if listener_override.0.is_finite()
            && listener_override.1.is_finite()
            && listener_override.2.is_finite()
            && (listener_override.0 != 0.0
                || listener_override.1 != 0.0
                || listener_override.2 != 0.0)
        {
            listener_override
        } else {
            let cam = crate::libs::renderable::camera_snapshot();
            (
                cam.cframe.position.x,
                cam.cframe.position.y,
                cam.cframe.position.z,
            )
        };
        let pos = *self.position.lock().unwrap();
        for s in self.shaders.lock().unwrap().iter() {
            match *s {
                VoiceShader::Volume(v) => volume *= v,
                VoiceShader::Speed(s) => speed *= s,
                VoiceShader::Spatial {
                    x,
                    y,
                    z,
                    min_falloff,
                    max_falloff,
                } => {
                    let dx = x - listener.0;
                    let dy = y - listener.1;
                    let dz = z - listener.2;
                    let dist = (dx * dx + dy * dy + dz * dz).sqrt();
                    let a = if dist <= min_falloff {
                        1.0
                    } else if dist >= max_falloff {
                        0.0
                    } else {
                        let span = (max_falloff - min_falloff).max(0.001);
                        let t = (dist - min_falloff) / span;
                        (1.0 - t).clamp(0.0, 1.0).powf(1.6)
                    };
                    atten *= a;
                    let _ = pos;
                    let cam = crate::libs::renderable::camera_snapshot();
                    let yaw = cam.cframe.rotation.y;
                    let local_x = yaw.cos() * dx + yaw.sin() * dz;
                    let dir_norm = if dist > 0.001 { local_x / dist } else { 0.0 };
                    pan = dir_norm.clamp(-1.0, 1.0);
                }
            }
        }
        let _ = speed;
        let _ = self.speed_phase;

        let amp = raw * volume * atten;
        let l = amp * ((1.0 - pan).max(0.0));
        let r = amp * ((1.0 + pan).max(0.0));
        self.last_l = l;
        self.last_r = r;
        self.next_channel = 1;
        Some(self.last_l)
    }
}

impl Source for VoiceSource {
    fn current_frame_len(&self) -> Option<usize> {
        None
    }
    fn channels(&self) -> u16 {
        OUTPUT_CHANNELS
    }
    fn sample_rate(&self) -> u32 {
        SAMPLE_RATE
    }
    fn total_duration(&self) -> Option<std::time::Duration> {
        None
    }
}

fn linear_resample(input: &[f32], from: u32, to: u32) -> Vec<f32> {
    if from == to {
        return input.to_vec();
    }
    let ratio = from as f64 / to as f64;
    let out_len = ((input.len() as f64) / ratio).floor() as usize;
    let mut out = Vec::with_capacity(out_len);
    for i in 0..out_len {
        let pos = (i as f64) * ratio;
        let lo = pos.floor() as usize;
        let hi = (lo + 1).min(input.len() - 1);
        let frac = (pos - lo as f64) as f32;
        out.push(input[lo] * (1.0 - frac) + input[hi] * frac);
    }
    out
}

pub fn is_active() -> bool {
    let captures_alive = CAPTURES.with(|c| {
        c.borrow()
            .iter()
            .any(|reg| reg.alive.load(Ordering::Relaxed))
    });
    captures_alive || recordings_active() || pending_records_active()
}

pub fn pump(lua: &Lua) {
    drip_recordings();
    pump_records(lua);
    pump_inner(lua);
}

fn pump_inner(lua: &Lua) {
    let drained: Vec<(Table, Vec<Vec<u8>>)> = CAPTURES.with(|c| {
        let mut list = c.borrow_mut();
        list.retain(|reg| reg.alive.load(Ordering::Relaxed));
        list.iter()
            .map(|reg| {
                let mut packets = Vec::new();
                while let Ok(p) = reg.rx.try_recv() {
                    packets.push(p);
                }
                (reg.signal_table.clone(), packets)
            })
            .collect()
    });
    for (sig, packets) in drained {
        for packet in packets {
            let s = match lua.create_string(&packet) {
                Ok(s) => s,
                Err(_) => continue,
            };
            let mut args = MultiValue::new();
            args.push_back(Value::String(s));
            if let Err(e) = signal::fire(lua, &sig, args) {
                eprintln!("[voice] OnPacket fire: {e}");
            }
        }
    }
}

pub struct Recorder {
    packets: Arc<Mutex<Vec<Vec<u8>>>>,
}

impl UserData for Recorder {
    fn add_methods<M: UserDataMethods<Self>>(m: &mut M) {
        m.add_method(
            "Push",
            |_, this, packet: mlua::String| -> mlua::Result<()> {
                this.packets
                    .lock()
                    .unwrap()
                    .push(packet.as_bytes().to_vec());
                Ok(())
            },
        );
        m.add_method("Clear", |_, this, _: ()| -> mlua::Result<()> {
            this.packets.lock().unwrap().clear();
            Ok(())
        });
        m.add_method("Length", |_, this, _: ()| -> mlua::Result<i64> {
            Ok(this.packets.lock().unwrap().len() as i64)
        });
        m.add_method("Duration", |_, this, _: ()| -> mlua::Result<f64> {
            Ok(this.packets.lock().unwrap().len() as f64 * (FRAME_MS as f64 / 1000.0))
        });
        m.add_method(
            "Serialize",
            |lua, this, _: ()| -> mlua::Result<mlua::String> {
                let packets = this.packets.lock().unwrap();
                let mut buf = Vec::with_capacity(4 + packets.len() * 64);
                buf.extend_from_slice(&(packets.len() as u32).to_le_bytes());
                for p in packets.iter() {
                    let len = p.len().min(u16::MAX as usize) as u16;
                    buf.extend_from_slice(&len.to_le_bytes());
                    buf.extend_from_slice(&p[..len as usize]);
                }
                lua.create_string(&buf)
            },
        );
        m.add_method(
            "ToRecording",
            |lua, this, _: ()| -> mlua::Result<AnyUserData> {
                let cloned = this.packets.lock().unwrap().clone();
                lua.create_userdata(Recording {
                    packets: Arc::new(cloned),
                })
            },
        );
    }
}

pub struct Recording {
    packets: Arc<Vec<Vec<u8>>>,
}

impl UserData for Recording {
    fn add_methods<M: UserDataMethods<Self>>(m: &mut M) {
        m.add_method(
            "PacketCount",
            |_, this, _: ()| Ok(this.packets.len() as i64),
        );
        m.add_method("Duration", |_, this, _: ()| {
            Ok(this.packets.len() as f64 * (FRAME_MS as f64 / 1000.0))
        });
        m.add_method(
            "PlayInto",
            |_, this, (channel_ud, opts): (AnyUserData, Option<Table>)| -> mlua::Result<()> {
                let channel = channel_ud.borrow::<ChannelHandle>().map_err(|_| {
                    mlua::Error::RuntimeError("Recording:PlayInto expects a VoiceChannel".into())
                })?;
                let looped = opts
                    .as_ref()
                    .and_then(|t| t.get::<bool>("loop").ok())
                    .unwrap_or(false);
                let speed: f32 = opts
                    .as_ref()
                    .and_then(|t| t.get::<f32>("speed").ok())
                    .unwrap_or(1.0)
                    .clamp(0.25, 4.0);
                RECORDING_PLAYBACKS.with(|c| {
                    c.borrow_mut().push(RecordingPlayback {
                        packets: this.packets.clone(),
                        index: 0,
                        accumulated_ms: 0.0,
                        last_tick: std::time::Instant::now(),
                        looped,
                        speed,
                        decoder: channel.decoder.clone(),
                        queue: channel.queue.clone(),
                    });
                });
                Ok(())
            },
        );
    }
}

struct RecordingPlayback {
    packets: Arc<Vec<Vec<u8>>>,
    index: usize,
    accumulated_ms: f32,
    last_tick: std::time::Instant,
    looped: bool,
    speed: f32,
    decoder: Arc<Mutex<Decoder>>,
    queue: Arc<Mutex<VecDeque<f32>>>,
}

thread_local! {
    static RECORDING_PLAYBACKS: RefCell<Vec<RecordingPlayback>> = const { RefCell::new(Vec::new()) };
}

fn drip_recordings() {
    RECORDING_PLAYBACKS.with(|c| {
        let mut list = c.borrow_mut();
        let mut i = 0;
        while i < list.len() {
            let pb = &mut list[i];
            if pb.packets.is_empty() {
                list.swap_remove(i);
                continue;
            }
            let now = std::time::Instant::now();
            let dt = now.duration_since(pb.last_tick).as_secs_f32() * 1000.0 * pb.speed;
            pb.last_tick = now;
            pb.accumulated_ms += dt;
            while pb.accumulated_ms >= FRAME_MS as f32 {
                pb.accumulated_ms -= FRAME_MS as f32;
                if pb.index >= pb.packets.len() {
                    if pb.looped {
                        pb.index = 0;
                    } else {
                        break;
                    }
                }
                let packet = &pb.packets[pb.index];
                pb.index += 1;
                let mut pcm = vec![0_f32; FRAME_SAMPLES * 6];
                let n = match pb
                    .decoder
                    .lock()
                    .unwrap()
                    .decode_float(packet, &mut pcm, false)
                {
                    Ok(n) => n,
                    Err(_) => continue,
                };
                let mut q = pb.queue.lock().unwrap();
                q.extend(pcm[..n].iter().copied());
                if q.len() > SAMPLE_RATE as usize * 2 {
                    let drop = q.len() - SAMPLE_RATE as usize;
                    q.drain(..drop);
                }
            }
            if !pb.looped && pb.index >= pb.packets.len() && pb.accumulated_ms < FRAME_MS as f32 {
                list.swap_remove(i);
            } else {
                i += 1;
            }
        }
    });
}

pub fn recordings_active() -> bool {
    RECORDING_PLAYBACKS.with(|c| !c.borrow().is_empty())
}

pub fn pending_records_active() -> bool {
    PENDING_RECORDS.with(|c| !c.borrow().is_empty())
}

fn deserialize_recording(lua: &Lua, bytes: &[u8]) -> mlua::Result<AnyUserData> {
    if bytes.len() < 4 {
        return Err(mlua::Error::RuntimeError(
            "Voice.LoadRecording: payload too short".into(),
        ));
    }
    let count = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as usize;
    let mut packets = Vec::with_capacity(count);
    let mut i = 4;
    for _ in 0..count {
        if i + 2 > bytes.len() {
            return Err(mlua::Error::RuntimeError(
                "Voice.LoadRecording: truncated length prefix".into(),
            ));
        }
        let len = u16::from_le_bytes([bytes[i], bytes[i + 1]]) as usize;
        i += 2;
        if i + len > bytes.len() {
            return Err(mlua::Error::RuntimeError(
                "Voice.LoadRecording: truncated packet".into(),
            ));
        }
        packets.push(bytes[i..i + len].to_vec());
        i += len;
    }
    lua.create_userdata(Recording {
        packets: Arc::new(packets),
    })
}

fn serialize_packets(packets: &[Vec<u8>]) -> Vec<u8> {
    let mut buf = Vec::with_capacity(4 + packets.len() * 64);
    buf.extend_from_slice(&(packets.len() as u32).to_le_bytes());
    for p in packets {
        let len = p.len().min(u16::MAX as usize) as u16;
        buf.extend_from_slice(&len.to_le_bytes());
        buf.extend_from_slice(&p[..len as usize]);
    }
    buf
}

fn record_one_shot(lua: &Lua, duration: f64) -> mlua::Result<Table> {
    if !(duration > 0.0) {
        return Err(mlua::Error::RuntimeError(
            "Voice.Record: duration must be > 0 seconds".into(),
        ));
    }
    let host = cpal::default_host();
    let device = host
        .default_input_device()
        .ok_or_else(|| mlua::Error::RuntimeError("Voice.Record: no default input device".into()))?;
    let supported = device
        .default_input_config()
        .map_err(|e| mlua::Error::RuntimeError(format!("Voice.Record: query input: {e}")))?;
    let device_rate = supported.sample_rate().0;
    let device_channels = supported.channels();
    let cfg = StreamConfig {
        channels: device_channels,
        sample_rate: cpal::SampleRate(device_rate),
        buffer_size: cpal::BufferSize::Default,
    };

    let (tx, rx) = std::sync::mpsc::channel::<Vec<u8>>();
    let alive = Arc::new(AtomicBool::new(true));
    let alive_cb = alive.clone();

    let mut encoder = Encoder::new(SAMPLE_RATE, OpusChannels::Mono, OpusApplication::Voip)
        .map_err(|e| mlua::Error::RuntimeError(format!("Voice.Record opus encoder: {e}")))?;
    let mut frame_buf: Vec<f32> = Vec::with_capacity(FRAME_SAMPLES * 2);
    let mut packet_buf = vec![0u8; 4000];

    let stream = device
        .build_input_stream(
            &cfg,
            move |data: &[f32], _: &cpal::InputCallbackInfo| {
                if !alive_cb.load(Ordering::Relaxed) {
                    return;
                }
                let mono: Vec<f32> = if device_channels == 1 {
                    data.to_vec()
                } else {
                    data.chunks(device_channels as usize)
                        .map(|c| c.iter().sum::<f32>() / c.len() as f32)
                        .collect()
                };
                let resampled = if device_rate == SAMPLE_RATE {
                    mono
                } else {
                    linear_resample(&mono, device_rate, SAMPLE_RATE)
                };
                frame_buf.extend(resampled);
                while frame_buf.len() >= FRAME_SAMPLES {
                    let frame: Vec<f32> = frame_buf.drain(..FRAME_SAMPLES).collect();
                    match encoder.encode_float(&frame, &mut packet_buf) {
                        Ok(n) => {
                            if tx.send(packet_buf[..n].to_vec()).is_err() {
                                return;
                            }
                        }
                        Err(e) => {
                            eprintln!("[voice] Record opus encode: {e}");
                        }
                    }
                }
            },
            move |err| eprintln!("[voice] Record capture err: {err}"),
            None,
        )
        .map_err(|e| mlua::Error::RuntimeError(format!("Voice.Record: build: {e}")))?;
    stream
        .play()
        .map_err(|e| mlua::Error::RuntimeError(format!("Voice.Record: play: {e}")))?;

    let signal_table = signal::new_instance(lua)?;
    let deadline =
        std::time::Instant::now() + std::time::Duration::from_secs_f64(duration.min(3600.0));
    PENDING_RECORDS.with(|cell| {
        cell.borrow_mut().push(PendingRecord {
            rx,
            alive,
            stream: Mutex::new(Some(stream)),
            deadline,
            packets: Vec::new(),
            signal_table: signal_table.clone(),
        });
    });
    Ok(signal_table)
}

fn pump_records(lua: &Lua) {
    let now = std::time::Instant::now();
    let completed: Vec<PendingRecord> = PENDING_RECORDS.with(|cell| {
        let mut list = cell.borrow_mut();
        for rec in list.iter_mut() {
            while let Ok(p) = rec.rx.try_recv() {
                rec.packets.push(p);
            }
        }
        let mut done: Vec<PendingRecord> = Vec::new();
        let mut i = 0;
        while i < list.len() {
            if now >= list[i].deadline {
                done.push(list.swap_remove(i));
            } else {
                i += 1;
            }
        }
        done
    });

    for mut rec in completed {
        rec.alive.store(false, Ordering::Relaxed);
        rec.stream.lock().unwrap().take();
        while let Ok(p) = rec.rx.try_recv() {
            rec.packets.push(p);
        }
        let serialized_bytes = serialize_packets(&rec.packets);
        let recording_ud = lua
            .create_userdata(Recording {
                packets: Arc::new(rec.packets),
            })
            .ok();
        let mut args = MultiValue::new();
        if let Some(ud) = recording_ud {
            args.push_back(Value::UserData(ud));
        } else {
            args.push_back(Value::Nil);
        }
        match lua.create_string(&serialized_bytes) {
            Ok(s) => args.push_back(Value::String(s)),
            Err(e) => {
                eprintln!("[voice] Record serialize: {e}");
                continue;
            }
        }
        if let Err(e) = signal::fire(lua, &rec.signal_table, args) {
            eprintln!("[voice] Record signal fire: {e}");
        }
    }
}
