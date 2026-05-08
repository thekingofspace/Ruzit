use std::cell::RefCell;
use std::io::Cursor;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use mlua::{
    AnyUserData, Lua, MultiValue, RegistryKey, Table, UserData, UserDataFields, UserDataMethods,
    Value,
};
use rodio::{Decoder, OutputStream, OutputStreamHandle, Sink, Source};

use crate::libs::shader::{AttachedShader, Params, read_param, shader_attach_spec, shader_id};
use crate::libs::signal;

pub const SOUND_EXTS: &[&str] = &["wav", "mp3", "ogg", "flac"];

thread_local! {

    static OUTPUT: RefCell<Option<OutputStreamHandle>> = const { RefCell::new(None) };
    static ACTIVE: RefCell<Vec<ActivePlayback>> = const { RefCell::new(Vec::new()) };
}

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

pub fn create(lua: &Lua) -> mlua::Result<Table> {
    let t = lua.create_table()?;

    t.set(
        "LoadSound",
        lua.create_function(|lua, data: AnyUserData| -> mlua::Result<AnyUserData> {
            let sd = data.borrow::<SoundData>()?;
            load_from_data(lua, &sd)
        })?,
    )?;

    t.set(
        "Volume",
        lua.create_function(|lua, factor: f32| lua.create_userdata(Shader::Volume(factor)))?,
    )?;
    t.set(
        "Speed",
        lua.create_function(|lua, factor: f32| lua.create_userdata(Shader::Speed(factor)))?,
    )?;
    t.set(
        "FadeIn",
        lua.create_function(|lua, secs: f64| lua.create_userdata(Shader::FadeIn(secs)))?,
    )?;
    t.set(
        "LowPass",
        lua.create_function(|lua, freq: u32| lua.create_userdata(Shader::LowPass(freq)))?,
    )?;
    t.set(
        "Delay",
        lua.create_function(|lua, secs: f64| lua.create_userdata(Shader::Delay(secs)))?,
    )?;
    t.set(
        "Repeat",
        lua.create_function(|lua, _: ()| lua.create_userdata(Shader::Repeat))?,
    )?;
    t.set(
        "FadeOut",
        lua.create_function(|lua, secs: f64| lua.create_userdata(Shader::FadeOut(secs)))?,
    )?;
    t.set(
        "HighPass",
        lua.create_function(|lua, freq: u32| lua.create_userdata(Shader::HighPass(freq)))?,
    )?;
    t.set(
        "Pan",
        lua.create_function(|lua, amount: f32| lua.create_userdata(Shader::Pan(amount)))?,
    )?;
    t.set(
        "Distortion",
        lua.create_function(|lua, amount: f32| lua.create_userdata(Shader::Distortion(amount)))?,
    )?;
    t.set(
        "Echo",
        lua.create_function(
            |lua, (delay_ms, feedback, mix): (u32, Option<f32>, Option<f32>)| {
                lua.create_userdata(Shader::Echo {
                    delay_ms,
                    feedback: feedback.unwrap_or(0.4),
                    mix: mix.unwrap_or(0.4),
                })
            },
        )?,
    )?;
    t.set(
        "Reverb",
        lua.create_function(|lua, (mix, decay): (Option<f32>, Option<f32>)| {
            lua.create_userdata(Shader::Reverb {
                mix: mix.unwrap_or(0.35),
                decay: decay.unwrap_or(0.7),
            })
        })?,
    )?;
    t.set(
        "Tremolo",
        lua.create_function(|lua, (rate, depth): (Option<f32>, Option<f32>)| {
            lua.create_userdata(Shader::Tremolo {
                rate: rate.unwrap_or(5.0),
                depth: depth.unwrap_or(0.5),
            })
        })?,
    )?;

    Ok(t)
}

pub struct SoundData {
    pub bytes: Arc<Vec<u8>>,
    pub source: String,
}

impl UserData for SoundData {
    fn add_methods<M: UserDataMethods<Self>>(m: &mut M) {
        m.add_method("Source", |_, this, _: ()| Ok(this.source.clone()));
        m.add_method("ByteCount", |_, this, _: ()| Ok(this.bytes.len() as i64));
    }
}

#[derive(Clone, Copy, Debug)]
pub enum Shader {
    Volume(f32),
    Speed(f32),
    FadeIn(f64),
    FadeOut(f64),
    LowPass(u32),
    HighPass(u32),
    Delay(f64),
    Repeat,
    Pan(f32),
    Distortion(f32),
    Echo {
        delay_ms: u32,
        feedback: f32,
        mix: f32,
    },
    Reverb {
        mix: f32,
        decay: f32,
    },
    Tremolo {
        rate: f32,
        depth: f32,
    },
}

impl Shader {
    fn kind_id(&self) -> u8 {
        match self {
            Shader::Volume(_) => 0,
            Shader::Speed(_) => 1,
            Shader::FadeIn(_) => 2,
            Shader::FadeOut(_) => 3,
            Shader::LowPass(_) => 4,
            Shader::HighPass(_) => 5,
            Shader::Delay(_) => 6,
            Shader::Repeat => 7,
            Shader::Pan(_) => 8,
            Shader::Distortion(_) => 9,
            Shader::Echo { .. } => 10,
            Shader::Reverb { .. } => 11,
            Shader::Tremolo { .. } => 12,
        }
    }
}

impl UserData for Shader {}

pub struct Sound {
    bytes: Arc<Vec<u8>>,
    source_path: String,
    started_key: Arc<RegistryKey>,
    stopped_key: Arc<RegistryKey>,
    did_loop_key: Arc<RegistryKey>,
    started_table: Table,
    stopped_table: Table,
    did_loop_table: Table,
    shaders: Mutex<Vec<Shader>>,
    attached: Mutex<Vec<AttachedShader>>,
    update_links: Mutex<Vec<UpdateLink>>,
    current_id: Mutex<Option<u64>>,
    position: Arc<Mutex<Option<SpatialPos>>>,
    looped: Arc<AtomicBool>,
    loop_count: Arc<AtomicU64>,
    total_duration: Mutex<Option<Duration>>,
    pending_offset: Mutex<Duration>,
}

#[derive(Clone, Copy, Debug)]
pub struct SpatialPos {
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub falloff: f32,
}

struct UpdateLink {
    interval: f64,
    key: Arc<RegistryKey>,
}

struct ActivePlayback {
    id: u64,
    sink: Sink,
    start: Instant,
    base_offset: Duration,
    started_fired: bool,
    started_key: Arc<RegistryKey>,
    stopped_key: Arc<RegistryKey>,
    did_loop_key: Arc<RegistryKey>,
    loop_count: Arc<AtomicU64>,
    last_loop_count: u64,
    updates: Vec<UpdateState>,
}

struct UpdateState {
    interval: f64,
    next_fire: f64,
    signal_key: Arc<RegistryKey>,
}

fn mlua_value_to_f32(v: mlua::Value) -> mlua::Result<f32> {
    match v {
        mlua::Value::Number(n) => Ok(n as f32),
        mlua::Value::Integer(n) => Ok(n as f32),
        mlua::Value::Nil => Err(mlua::Error::RuntimeError(
            "Sound.SetPosition: expected number or nil to clear".into(),
        )),
        _ => Err(mlua::Error::RuntimeError(
            "Sound.SetPosition: expected number".into(),
        )),
    }
}

fn output_handle() -> mlua::Result<OutputStreamHandle> {
    OUTPUT.with(|cell| {
        let mut borrow = cell.borrow_mut();
        if borrow.is_none() {
            let (stream, handle) = OutputStream::try_default()
                .map_err(|e| mlua::Error::RuntimeError(format!("SFX audio init: {e}")))?;

            std::mem::forget(stream);
            *borrow = Some(handle);
        }
        Ok(borrow.as_ref().unwrap().clone())
    })
}

fn load_from_data(lua: &Lua, data: &SoundData) -> mlua::Result<AnyUserData> {
    let probe = Decoder::new(Cursor::new((*data.bytes).clone())).map_err(|e| {
        mlua::Error::RuntimeError(format!("SFX.LoadSound: decode '{}': {e}", data.source))
    })?;
    let total = probe.total_duration();
    drop(probe);

    let started = signal::new_instance(lua)?;
    let stopped = signal::new_instance(lua)?;
    let did_loop = signal::new_instance(lua)?;
    let started_key = Arc::new(lua.create_registry_value(started.clone())?);
    let stopped_key = Arc::new(lua.create_registry_value(stopped.clone())?);
    let did_loop_key = Arc::new(lua.create_registry_value(did_loop.clone())?);

    lua.create_userdata(Sound {
        bytes: data.bytes.clone(),
        source_path: data.source.clone(),
        started_key,
        stopped_key,
        did_loop_key,
        started_table: started,
        stopped_table: stopped,
        did_loop_table: did_loop,
        shaders: Mutex::new(Vec::new()),
        attached: Mutex::new(Vec::new()),
        update_links: Mutex::new(Vec::new()),
        current_id: Mutex::new(None),
        position: Arc::new(Mutex::new(None)),
        looped: Arc::new(AtomicBool::new(false)),
        loop_count: Arc::new(AtomicU64::new(0)),
        total_duration: Mutex::new(total),
        pending_offset: Mutex::new(Duration::ZERO),
    })
}

impl UserData for Sound {
    fn add_fields<F: UserDataFields<Self>>(f: &mut F) {
        f.add_field_method_get("Started", |_, this| Ok(this.started_table.clone()));
        f.add_field_method_get("Stopped", |_, this| Ok(this.stopped_table.clone()));
        f.add_field_method_get("DidLoop", |_, this| Ok(this.did_loop_table.clone()));
        f.add_field_method_get("Source", |_, this| Ok(this.source_path.clone()));

        f.add_field_method_get("Looped", |_, this| Ok(this.looped.load(Ordering::Relaxed)));
        f.add_field_method_set("Looped", |_, this, value: bool| {
            this.looped.store(value, Ordering::Relaxed);
            Ok(())
        });

        f.add_field_method_get("TimePosition", |_, this| {
            let cur_id = *this.current_id.lock().unwrap();
            if let Some(id) = cur_id {
                let total = *this.total_duration.lock().unwrap();
                let looped = this.looped.load(Ordering::Relaxed);
                let secs = ACTIVE.with(|c| {
                    c.borrow()
                        .iter()
                        .find(|p| p.id == id)
                        .map(|p| {
                            let elapsed = p.sink.get_pos() + p.base_offset;
                            let mut s = elapsed.as_secs_f64();
                            if looped {
                                if let Some(t) = total {
                                    let total_secs = t.as_secs_f64();
                                    if total_secs > 0.0 {
                                        s %= total_secs;
                                    }
                                }
                            }
                            s
                        })
                        .unwrap_or(0.0)
                });
                Ok(secs)
            } else {
                Ok(this.pending_offset.lock().unwrap().as_secs_f64())
            }
        });
        f.add_field_method_set("TimePosition", |_, this, secs: f64| {
            let target = Duration::from_secs_f64(secs.max(0.0));
            let cur_id = *this.current_id.lock().unwrap();
            if cur_id.is_some() {
                play_sound_at(this, target)?;
            } else {
                *this.pending_offset.lock().unwrap() = target;
            }
            Ok(())
        });
    }

    fn add_methods<M: UserDataMethods<Self>>(m: &mut M) {
        m.add_method("Play", |_, this, _: ()| play_sound(this));
        m.add_method("Stop", |_, this, _: ()| {
            stop_sound(this);
            Ok(())
        });
        m.add_method("IsPlaying", |_, this, _: ()| {
            Ok(this.current_id.lock().unwrap().is_some())
        });
        m.add_method(
            "ApplyShader",
            |_, this, shader: AnyUserData| -> mlua::Result<()> {
                let s = *shader.borrow::<Shader>()?;
                this.shaders.lock().unwrap().push(s);
                Ok(())
            },
        );
        m.add_method("ClearShaders", |_, this, _: ()| -> mlua::Result<()> {
            this.shaders.lock().unwrap().clear();
            Ok(())
        });

        fn set_kind(this: &Sound, new_shader: Shader) {
            let mut list = this.shaders.lock().unwrap();
            let kind = new_shader.kind_id();
            list.retain(|s| s.kind_id() != kind);
            list.push(new_shader);
        }
        m.add_method("Volume", |_, this, factor: f32| -> mlua::Result<()> {
            set_kind(this, Shader::Volume(factor.max(0.0)));
            Ok(())
        });
        m.add_method("Speed", |_, this, factor: f32| -> mlua::Result<()> {
            set_kind(this, Shader::Speed(factor.max(0.0)));
            Ok(())
        });
        m.add_method("Pitch", |_, this, factor: f32| -> mlua::Result<()> {
            set_kind(this, Shader::Speed(factor.max(0.0)));
            Ok(())
        });
        m.add_method("Pan", |_, this, amount: f32| -> mlua::Result<()> {
            set_kind(this, Shader::Pan(amount));
            Ok(())
        });
        m.add_method("LowPass", |_, this, freq: u32| -> mlua::Result<()> {
            set_kind(this, Shader::LowPass(freq));
            Ok(())
        });
        m.add_method("HighPass", |_, this, freq: u32| -> mlua::Result<()> {
            set_kind(this, Shader::HighPass(freq));
            Ok(())
        });
        m.add_method("FadeIn", |_, this, secs: f64| -> mlua::Result<()> {
            set_kind(this, Shader::FadeIn(secs));
            Ok(())
        });
        m.add_method("FadeOut", |_, this, secs: f64| -> mlua::Result<()> {
            set_kind(this, Shader::FadeOut(secs));
            Ok(())
        });
        m.add_method("Delay", |_, this, secs: f64| -> mlua::Result<()> {
            set_kind(this, Shader::Delay(secs));
            Ok(())
        });
        m.add_method("Loop", |_, this, _: ()| -> mlua::Result<()> {
            set_kind(this, Shader::Repeat);
            Ok(())
        });
        m.add_method("Distortion", |_, this, amount: f32| -> mlua::Result<()> {
            set_kind(this, Shader::Distortion(amount));
            Ok(())
        });
        m.add_method(
            "Echo",
            |_,
             this,
             (delay_ms, feedback, mix): (u32, Option<f32>, Option<f32>)|
             -> mlua::Result<()> {
                set_kind(
                    this,
                    Shader::Echo {
                        delay_ms,
                        feedback: feedback.unwrap_or(0.4),
                        mix: mix.unwrap_or(0.4),
                    },
                );
                Ok(())
            },
        );
        m.add_method(
            "Reverb",
            |_, this, (mix, decay): (Option<f32>, Option<f32>)| -> mlua::Result<()> {
                set_kind(
                    this,
                    Shader::Reverb {
                        mix: mix.unwrap_or(0.35),
                        decay: decay.unwrap_or(0.7),
                    },
                );
                Ok(())
            },
        );
        m.add_method(
            "Tremolo",
            |_, this, (rate, depth): (Option<f32>, Option<f32>)| -> mlua::Result<()> {
                set_kind(
                    this,
                    Shader::Tremolo {
                        rate: rate.unwrap_or(5.0),
                        depth: depth.unwrap_or(0.5),
                    },
                );
                Ok(())
            },
        );
        m.add_method("Reset", |_, this, _: ()| -> mlua::Result<()> {
            this.shaders.lock().unwrap().clear();
            Ok(())
        });

        m.add_method(
            "SetPosition",
            |_, this, args: mlua::MultiValue| -> mlua::Result<()> {
                if args.is_empty() {
                    *this.position.lock().unwrap() = None;
                    return Ok(());
                }
                let mut iter = args.into_iter();
                let first = iter.next().unwrap_or(mlua::Value::Nil);
                if matches!(first, mlua::Value::Nil) {
                    *this.position.lock().unwrap() = None;
                    return Ok(());
                }
                let x = mlua_value_to_f32(first)?;
                let y = mlua_value_to_f32(iter.next().unwrap_or(mlua::Value::Nil))?;
                let z = mlua_value_to_f32(iter.next().unwrap_or(mlua::Value::Nil))?;
                let falloff = match iter.next() {
                    Some(mlua::Value::Number(n)) => n as f32,
                    Some(mlua::Value::Integer(n)) => n as f32,
                    _ => 20.0,
                };
                *this.position.lock().unwrap() = Some(SpatialPos {
                    x,
                    y,
                    z,
                    falloff: falloff.max(0.1),
                });
                Ok(())
            },
        );
        m.add_method("ClearPosition", |_, this, _: ()| -> mlua::Result<()> {
            *this.position.lock().unwrap() = None;
            Ok(())
        });
        m.add_method(
            "AttachShader",
            |_, this, asset: AnyUserData| -> mlua::Result<()> {
                let attached = shader_attach_spec(&asset)?;
                let mut list = this.attached.lock().unwrap();
                if list.iter().any(|e| e.id == attached.id) {
                    return Err(mlua::Error::RuntimeError(
                        "AttachShader: shader is already attached".into(),
                    ));
                }
                list.push(attached);
                Ok(())
            },
        );
        m.add_method(
            "DetachShader",
            |_, this, asset: AnyUserData| -> mlua::Result<()> {
                let id = shader_id(&asset)?;
                this.attached.lock().unwrap().retain(|e| e.id != id);
                Ok(())
            },
        );
        m.add_method(
            "SetData",
            |_, this, (asset, name, value): (AnyUserData, String, f32)| -> mlua::Result<()> {
                let id = shader_id(&asset)?;
                let list = this.attached.lock().unwrap();
                let entry = list.iter().find(|e| e.id == id).ok_or_else(|| {
                    mlua::Error::RuntimeError(
                        "SetData: shader is not attached to this sound".into(),
                    )
                })?;

                entry.params.lock().unwrap().insert(name, value);
                Ok(())
            },
        );
        m.add_method(
            "GetData",
            |_, this, (asset, name): (AnyUserData, String)| -> mlua::Result<Option<f32>> {
                let id = shader_id(&asset)?;
                let list = this.attached.lock().unwrap();
                let entry = list.iter().find(|e| e.id == id).ok_or_else(|| {
                    mlua::Error::RuntimeError(
                        "GetData: shader is not attached to this sound".into(),
                    )
                })?;
                Ok(entry.params.lock().unwrap().get(&name).copied())
            },
        );
        m.add_method(
            "LinkToUpdate",
            |lua, this, interval: f64| -> mlua::Result<Table> {
                if !(interval > 0.0) {
                    return Err(mlua::Error::RuntimeError(
                        "SFX.LinkToUpdate: interval must be > 0".into(),
                    ));
                }
                let signal = signal::new_instance(lua)?;
                let key = Arc::new(lua.create_registry_value(signal.clone())?);
                this.update_links
                    .lock()
                    .unwrap()
                    .push(UpdateLink { interval, key });
                Ok(signal)
            },
        );
    }
}

fn build_source(
    bytes: &[u8],
    shaders: &[Shader],
    attached: &[AttachedShader],
) -> mlua::Result<Box<dyn Source<Item = f32> + Send>> {
    let decoder = Decoder::new(Cursor::new(bytes.to_vec()))
        .map_err(|e| mlua::Error::RuntimeError(format!("SFX decode: {e}")))?;
    let mut source: Box<dyn Source<Item = f32> + Send> = Box::new(decoder.convert_samples::<f32>());
    for sh in shaders {
        source = match *sh {
            Shader::Volume(f) => Box::new(source.amplify(f)),
            Shader::Speed(f) => Box::new(source.speed(f)),
            Shader::FadeIn(s) => Box::new(source.fade_in(Duration::from_secs_f64(s))),
            Shader::FadeOut(s) => Box::new(FadeOut::new(source, s)),
            Shader::LowPass(freq) => Box::new(source.low_pass(freq)),
            Shader::HighPass(freq) => Box::new(source.high_pass(freq)),
            Shader::Delay(s) => Box::new(source.delay(Duration::from_secs_f64(s))),
            Shader::Repeat => Box::new(source.repeat_infinite()),
            Shader::Pan(amount) => Box::new(StaticPan::new(source, amount)),
            Shader::Distortion(amount) => Box::new(Distortion::new(source, amount)),
            Shader::Echo {
                delay_ms,
                feedback,
                mix,
            } => Box::new(Echo::new(source, delay_ms, feedback, mix)),
            Shader::Reverb { mix, decay } => Box::new(Reverb::new(source, mix, decay)),
            Shader::Tremolo { rate, depth } => Box::new(StaticTremolo::new(source, rate, depth)),
        };
    }
    for a in attached {
        source = apply_attached(source, a)?;
    }
    Ok(source)
}

fn apply_attached(
    source: Box<dyn Source<Item = f32> + Send>,
    attached: &AttachedShader,
) -> mlua::Result<Box<dyn Source<Item = f32> + Send>> {
    let params = attached.params.clone();
    match attached.kind.as_str() {
        "wobble" | "tremolo" => Ok(Box::new(Tremolo::new(source, params))),
        "volume" | "gain" => Ok(Box::new(LiveGain::new(source, params))),

        "speed" => Ok(Box::new(source.speed(read_param(&params, "factor", 1.0)))),
        "lowpass" => Ok(Box::new(
            source.low_pass(read_param(&params, "freq", 1000.0) as u32),
        )),

        "pan" => Ok(Box::new(Pan::new(source, params))),

        "distance" | "falloff" => Ok(Box::new(Distance::new(source, params))),

        "spatial" | "position" | "3d" => Ok(Box::new(Spatial::new(source, params))),

        other => Err(mlua::Error::RuntimeError(format!(
            "AttachShader: unknown audio shader kind '{other}'"
        ))),
    }
}

struct Tremolo<I> {
    inner: I,
    params: Params,
    sample_idx: u64,
}

impl<I> Tremolo<I>
where
    I: Source<Item = f32>,
{
    fn new(inner: I, params: Params) -> Self {
        Self {
            inner,
            params,
            sample_idx: 0,
        }
    }
}

impl<I> Iterator for Tremolo<I>
where
    I: Source<Item = f32>,
{
    type Item = f32;

    fn next(&mut self) -> Option<f32> {
        let s = self.inner.next()?;
        let channels = self.inner.channels().max(1) as u64;
        let sample_rate = self.inner.sample_rate().max(1) as f32;
        let rate = read_param(&self.params, "rate", 5.0).max(0.0);
        let depth = read_param(&self.params, "depth", 0.5).clamp(0.0, 1.0);
        let frame = self.sample_idx / channels;
        let t = frame as f32 / sample_rate;
        let lfo = (2.0 * std::f32::consts::PI * rate * t).sin();
        let gain = (1.0 - depth) + depth * (lfo * 0.5 + 0.5);
        self.sample_idx = self.sample_idx.wrapping_add(1);
        Some(s * gain)
    }
}

impl<I> Source for Tremolo<I>
where
    I: Source<Item = f32>,
{
    fn current_frame_len(&self) -> Option<usize> {
        self.inner.current_frame_len()
    }
    fn channels(&self) -> u16 {
        self.inner.channels()
    }
    fn sample_rate(&self) -> u32 {
        self.inner.sample_rate()
    }
    fn total_duration(&self) -> Option<Duration> {
        self.inner.total_duration()
    }
}

struct LiveGain<I> {
    inner: I,
    params: Params,
}

impl<I: Source<Item = f32>> LiveGain<I> {
    fn new(inner: I, params: Params) -> Self {
        Self { inner, params }
    }
}

impl<I: Source<Item = f32>> Iterator for LiveGain<I> {
    type Item = f32;
    fn next(&mut self) -> Option<f32> {
        let s = self.inner.next()?;
        Some(s * read_param(&self.params, "amount", 1.0))
    }
}

impl<I: Source<Item = f32>> Source for LiveGain<I> {
    fn current_frame_len(&self) -> Option<usize> {
        self.inner.current_frame_len()
    }
    fn channels(&self) -> u16 {
        self.inner.channels()
    }
    fn sample_rate(&self) -> u32 {
        self.inner.sample_rate()
    }
    fn total_duration(&self) -> Option<Duration> {
        self.inner.total_duration()
    }
}

struct Pan<I> {
    inner: I,
    params: Params,
    channel_idx: u16,
}

impl<I: Source<Item = f32>> Pan<I> {
    fn new(inner: I, params: Params) -> Self {
        Self {
            inner,
            params,
            channel_idx: 0,
        }
    }
}

impl<I: Source<Item = f32>> Iterator for Pan<I> {
    type Item = f32;
    fn next(&mut self) -> Option<f32> {
        let s = self.inner.next()?;
        let channels = self.inner.channels().max(1);
        let amount = read_param(&self.params, "amount", 0.0).clamp(-1.0, 1.0);

        let theta = (amount + 1.0) * std::f32::consts::FRAC_PI_4;
        let gain = match (channels, self.channel_idx) {
            (1, _) => 1.0,
            (_, 0) => theta.cos(),
            (_, 1) => theta.sin(),
            _ => 1.0,
        };
        self.channel_idx = (self.channel_idx + 1) % channels;
        Some(s * gain)
    }
}

impl<I: Source<Item = f32>> Source for Pan<I> {
    fn current_frame_len(&self) -> Option<usize> {
        self.inner.current_frame_len()
    }
    fn channels(&self) -> u16 {
        self.inner.channels()
    }
    fn sample_rate(&self) -> u32 {
        self.inner.sample_rate()
    }
    fn total_duration(&self) -> Option<Duration> {
        self.inner.total_duration()
    }
}

struct Distance<I> {
    inner: I,
    params: Params,
}

impl<I: Source<Item = f32>> Distance<I> {
    fn new(inner: I, params: Params) -> Self {
        Self { inner, params }
    }
}

impl<I: Source<Item = f32>> Iterator for Distance<I> {
    type Item = f32;
    fn next(&mut self) -> Option<f32> {
        let s = self.inner.next()?;
        let dist = read_param(&self.params, "distance", 1.0).max(0.0);
        let falloff = read_param(&self.params, "falloff", 1.0).max(0.0);
        let gain = 1.0 / (1.0 + falloff * dist);
        Some(s * gain)
    }
}

impl<I: Source<Item = f32>> Source for Distance<I> {
    fn current_frame_len(&self) -> Option<usize> {
        self.inner.current_frame_len()
    }
    fn channels(&self) -> u16 {
        self.inner.channels()
    }
    fn sample_rate(&self) -> u32 {
        self.inner.sample_rate()
    }
    fn total_duration(&self) -> Option<Duration> {
        self.inner.total_duration()
    }
}

struct Spatial<I> {
    inner: I,
    params: Params,

    held: f32,

    held_left: f32,
    held_right: f32,
    output_channel: u8,
}

impl<I: Source<Item = f32>> Spatial<I> {
    fn new(inner: I, params: Params) -> Self {
        Self {
            inner,
            params,
            held: 0.0,
            held_left: 0.0,
            held_right: 0.0,
            output_channel: 0,
        }
    }

    fn next_mono_frame(&mut self) -> Option<f32> {
        let channels = self.inner.channels().max(1);
        let mut sum = 0.0;
        for _ in 0..channels {
            sum += self.inner.next()?;
        }
        Some(sum / channels as f32)
    }
}

impl<I: Source<Item = f32>> Iterator for Spatial<I> {
    type Item = f32;
    fn next(&mut self) -> Option<f32> {
        if self.output_channel == 0 {
            self.held = self.next_mono_frame()?;

            let p = self.params.lock().unwrap();
            let x = *p.get("x").unwrap_or(&0.0);
            let y = *p.get("y").unwrap_or(&0.0);
            let z = *p.get("z").unwrap_or(&0.0);
            let falloff = p.get("falloff").copied().unwrap_or(1.0).max(0.0);
            drop(p);

            let dist = (x * x + y * y + z * z).sqrt();
            let attenuation = 1.0 / (1.0 + falloff * dist);

            let pan = if dist > 1e-4 {
                (x / dist).clamp(-1.0, 1.0)
            } else {
                0.0
            };
            let theta = (pan + 1.0) * std::f32::consts::FRAC_PI_4;
            self.held_left = theta.cos() * attenuation;
            self.held_right = theta.sin() * attenuation;
        }
        let out = if self.output_channel == 0 {
            self.held * self.held_left
        } else {
            self.held * self.held_right
        };
        self.output_channel = (self.output_channel + 1) % 2;
        Some(out)
    }
}

impl<I: Source<Item = f32>> Source for Spatial<I> {
    fn current_frame_len(&self) -> Option<usize> {
        None
    }
    fn channels(&self) -> u16 {
        2
    }
    fn sample_rate(&self) -> u32 {
        self.inner.sample_rate()
    }
    fn total_duration(&self) -> Option<Duration> {
        self.inner.total_duration()
    }
}

fn play_sound(this: &Sound) -> mlua::Result<()> {
    let offset = *this.pending_offset.lock().unwrap();
    play_sound_at(this, offset)
}

fn play_sound_at(this: &Sound, offset: Duration) -> mlua::Result<()> {
    let prev_id = this.current_id.lock().unwrap().take();
    if let Some(id) = prev_id {
        ACTIVE.with(|c| {
            let mut active = c.borrow_mut();
            if let Some(pos) = active.iter().position(|p| p.id == id) {
                let p = active.remove(pos);
                p.sink.stop();
            }
        });
    }

    let handle = output_handle()?;
    let sink =
        Sink::try_new(&handle).map_err(|e| mlua::Error::RuntimeError(format!("SFX sink: {e}")))?;
    let shaders = this.shaders.lock().unwrap().clone();
    let attached = this.attached.lock().unwrap().clone();
    let looper = Looper::build(
        this.bytes.clone(),
        shaders,
        attached,
        this.looped.clone(),
        this.loop_count.clone(),
    )?;
    let pos_handle = this.position.clone();
    let mut final_source: Box<dyn Source<Item = f32> + Send> =
        Box::new(CameraSpatial::new(Box::new(looper), pos_handle));
    if !offset.is_zero() {
        final_source = Box::new(final_source.skip_duration(offset));
    }
    sink.append(final_source);

    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    let updates: Vec<UpdateState> = this
        .update_links
        .lock()
        .unwrap()
        .iter()
        .map(|link| UpdateState {
            interval: link.interval,
            next_fire: link.interval,
            signal_key: link.key.clone(),
        })
        .collect();
    let last_loop_count = this.loop_count.load(Ordering::Relaxed);

    ACTIVE.with(|c| {
        c.borrow_mut().push(ActivePlayback {
            id,
            sink,
            start: Instant::now(),
            base_offset: offset,
            started_fired: false,
            started_key: this.started_key.clone(),
            stopped_key: this.stopped_key.clone(),
            did_loop_key: this.did_loop_key.clone(),
            loop_count: this.loop_count.clone(),
            last_loop_count,
            updates,
        });
    });
    *this.current_id.lock().unwrap() = Some(id);
    Ok(())
}

fn stop_sound(this: &Sound) {
    let id = this.current_id.lock().unwrap().take();
    if let Some(id) = id {
        ACTIVE.with(|c| {
            if let Some(p) = c.borrow().iter().find(|p| p.id == id) {
                p.sink.stop();
            }
        });
    }
}

pub fn pump(lua: &Lua) {
    let snapshot = ACTIVE.with(|c| std::mem::take(&mut *c.borrow_mut()));
    let mut keep: Vec<ActivePlayback> = Vec::with_capacity(snapshot.len());

    for mut p in snapshot {
        let elapsed = p.start.elapsed().as_secs_f64();

        if !p.started_fired {
            p.started_fired = true;
            if let Err(e) = fire_signal(lua, &p.started_key, MultiValue::new()) {
                eprintln!("[SFX] Started fire error: {e}");
            }
        }

        for u in p.updates.iter_mut() {
            while elapsed >= u.next_fire {
                let fire_time = u.next_fire;
                let mut args = MultiValue::new();
                args.push_back(Value::Number(fire_time));
                if let Err(e) = fire_signal(lua, &u.signal_key, args) {
                    eprintln!("[SFX] LinkToUpdate fire error: {e}");
                }
                u.next_fire += u.interval;
            }
        }

        let cur_loops = p.loop_count.load(Ordering::Relaxed);
        while p.last_loop_count < cur_loops {
            if let Err(e) = fire_signal(lua, &p.did_loop_key, MultiValue::new()) {
                eprintln!("[SFX] DidLoop fire error: {e}");
            }
            p.last_loop_count += 1;
        }

        if p.sink.empty() {
            if let Err(e) = fire_signal(lua, &p.stopped_key, MultiValue::new()) {
                eprintln!("[SFX] Stopped fire error: {e}");
            }
        } else {
            keep.push(p);
        }
    }

    ACTIVE.with(|c| {
        let mut active = c.borrow_mut();

        let added = std::mem::take(&mut *active);
        keep.extend(added);
        *active = keep;
    });
}

pub fn is_active() -> bool {
    ACTIVE.with(|c| !c.borrow().is_empty())
}

fn fire_signal(lua: &Lua, key: &RegistryKey, args: MultiValue) -> mlua::Result<()> {
    let signal: Table = lua.registry_value(key)?;
    signal::fire(lua, &signal, args)
}

struct FadeOut<I> {
    inner: I,
    sample_idx: u64,
    fade_started_at: Option<u64>,
    total_samples: Option<u64>,
}

impl<I: Source<Item = f32>> FadeOut<I> {
    fn new(inner: I, duration_secs: f64) -> Self {
        let total = inner.total_duration().map(|d| {
            (d.as_secs_f64() * inner.sample_rate() as f64 * inner.channels() as f64) as u64
        });
        let fade_samples =
            (duration_secs * inner.sample_rate() as f64 * inner.channels() as f64) as u64;
        let started = total.map(|t| t.saturating_sub(fade_samples));
        Self {
            inner,
            sample_idx: 0,
            fade_started_at: started,
            total_samples: total,
        }
    }
}

impl<I: Source<Item = f32>> Iterator for FadeOut<I> {
    type Item = f32;
    fn next(&mut self) -> Option<f32> {
        let s = self.inner.next()?;
        let i = self.sample_idx;
        self.sample_idx = self.sample_idx.wrapping_add(1);
        let gain = match (self.fade_started_at, self.total_samples) {
            (Some(start), Some(end)) if i >= start => {
                let span = (end - start).max(1) as f32;
                let prog = ((i - start) as f32 / span).clamp(0.0, 1.0);
                1.0 - prog
            }
            _ => 1.0,
        };
        Some(s * gain)
    }
}

impl<I: Source<Item = f32>> Source for FadeOut<I> {
    fn current_frame_len(&self) -> Option<usize> {
        self.inner.current_frame_len()
    }
    fn channels(&self) -> u16 {
        self.inner.channels()
    }
    fn sample_rate(&self) -> u32 {
        self.inner.sample_rate()
    }
    fn total_duration(&self) -> Option<Duration> {
        self.inner.total_duration()
    }
}

struct StaticPan<I> {
    inner: I,
    amount: f32,
    channel_idx: u16,
}

impl<I: Source<Item = f32>> StaticPan<I> {
    fn new(inner: I, amount: f32) -> Self {
        Self {
            inner,
            amount: amount.clamp(-1.0, 1.0),
            channel_idx: 0,
        }
    }
}

impl<I: Source<Item = f32>> Iterator for StaticPan<I> {
    type Item = f32;
    fn next(&mut self) -> Option<f32> {
        let s = self.inner.next()?;
        let channels = self.inner.channels().max(1);
        let theta = (self.amount + 1.0) * std::f32::consts::FRAC_PI_4;
        let gain = match (channels, self.channel_idx) {
            (1, _) => 1.0,
            (_, 0) => theta.cos(),
            (_, 1) => theta.sin(),
            _ => 1.0,
        };
        self.channel_idx = (self.channel_idx + 1) % channels;
        Some(s * gain)
    }
}

impl<I: Source<Item = f32>> Source for StaticPan<I> {
    fn current_frame_len(&self) -> Option<usize> {
        self.inner.current_frame_len()
    }
    fn channels(&self) -> u16 {
        self.inner.channels()
    }
    fn sample_rate(&self) -> u32 {
        self.inner.sample_rate()
    }
    fn total_duration(&self) -> Option<Duration> {
        self.inner.total_duration()
    }
}

struct Distortion<I> {
    inner: I,
    drive: f32,
    norm: f32,
}

impl<I: Source<Item = f32>> Distortion<I> {
    fn new(inner: I, amount: f32) -> Self {
        let drive = (1.0 + amount.max(0.0) * 9.0).max(1.0);
        let norm = drive.tanh().max(1e-3);
        Self { inner, drive, norm }
    }
}

impl<I: Source<Item = f32>> Iterator for Distortion<I> {
    type Item = f32;
    fn next(&mut self) -> Option<f32> {
        let s = self.inner.next()?;
        Some((s * self.drive).tanh() / self.norm)
    }
}

impl<I: Source<Item = f32>> Source for Distortion<I> {
    fn current_frame_len(&self) -> Option<usize> {
        self.inner.current_frame_len()
    }
    fn channels(&self) -> u16 {
        self.inner.channels()
    }
    fn sample_rate(&self) -> u32 {
        self.inner.sample_rate()
    }
    fn total_duration(&self) -> Option<Duration> {
        self.inner.total_duration()
    }
}

struct Echo<I> {
    inner: I,
    buf: Vec<f32>,
    head: usize,
    feedback: f32,
    mix: f32,
}

impl<I: Source<Item = f32>> Echo<I> {
    fn new(inner: I, delay_ms: u32, feedback: f32, mix: f32) -> Self {
        let frames = (inner.sample_rate() as u64 * delay_ms.max(1) as u64 / 1000) as usize;
        let len = frames.max(1) * inner.channels().max(1) as usize;
        Self {
            inner,
            buf: vec![0.0_f32; len],
            head: 0,
            feedback: feedback.clamp(0.0, 0.95),
            mix: mix.clamp(0.0, 1.0),
        }
    }
}

impl<I: Source<Item = f32>> Iterator for Echo<I> {
    type Item = f32;
    fn next(&mut self) -> Option<f32> {
        let dry = self.inner.next()?;
        let delayed = self.buf[self.head];
        let new_buf_value = dry + delayed * self.feedback;
        self.buf[self.head] = new_buf_value;
        self.head = (self.head + 1) % self.buf.len();
        Some(dry * (1.0 - self.mix) + delayed * self.mix)
    }
}

impl<I: Source<Item = f32>> Source for Echo<I> {
    fn current_frame_len(&self) -> Option<usize> {
        self.inner.current_frame_len()
    }
    fn channels(&self) -> u16 {
        self.inner.channels()
    }
    fn sample_rate(&self) -> u32 {
        self.inner.sample_rate()
    }
    fn total_duration(&self) -> Option<Duration> {
        self.inner.total_duration()
    }
}

struct Reverb<I> {
    inner: I,
    combs: [Comb; 4],
    allpasses: [AllPass; 2],
    mix: f32,
}

struct Comb {
    buf: Vec<f32>,
    head: usize,
    feedback: f32,
}
struct AllPass {
    buf: Vec<f32>,
    head: usize,
}

impl Comb {
    fn new(samples: usize, feedback: f32) -> Self {
        Self {
            buf: vec![0.0; samples.max(1)],
            head: 0,
            feedback,
        }
    }
    fn process(&mut self, x: f32) -> f32 {
        let y = self.buf[self.head];
        self.buf[self.head] = x + y * self.feedback;
        self.head = (self.head + 1) % self.buf.len();
        y
    }
}
impl AllPass {
    fn new(samples: usize) -> Self {
        Self {
            buf: vec![0.0; samples.max(1)],
            head: 0,
        }
    }
    fn process(&mut self, x: f32) -> f32 {
        let g = 0.5_f32;
        let buf_out = self.buf[self.head];
        let y = -x + buf_out;
        self.buf[self.head] = x + buf_out * g;
        self.head = (self.head + 1) % self.buf.len();
        y
    }
}

impl<I: Source<Item = f32>> Reverb<I> {
    fn new(inner: I, mix: f32, decay: f32) -> Self {
        let sr = inner.sample_rate() as f32;
        let ch = inner.channels().max(1) as usize;
        let fb = decay.clamp(0.05, 0.95);
        let comb_lens = [29, 37, 41, 43];
        let ap_lens = [5, 7];
        let combs = [
            Comb::new((sr * comb_lens[0] as f32 / 1000.0) as usize * ch, fb),
            Comb::new((sr * comb_lens[1] as f32 / 1000.0) as usize * ch, fb),
            Comb::new((sr * comb_lens[2] as f32 / 1000.0) as usize * ch, fb),
            Comb::new((sr * comb_lens[3] as f32 / 1000.0) as usize * ch, fb),
        ];
        let allpasses = [
            AllPass::new((sr * ap_lens[0] as f32 / 1000.0) as usize * ch),
            AllPass::new((sr * ap_lens[1] as f32 / 1000.0) as usize * ch),
        ];
        Self {
            inner,
            combs,
            allpasses,
            mix: mix.clamp(0.0, 1.0),
        }
    }
}

impl<I: Source<Item = f32>> Iterator for Reverb<I> {
    type Item = f32;
    fn next(&mut self) -> Option<f32> {
        let dry = self.inner.next()?;
        let mut wet = 0.0;
        for c in &mut self.combs {
            wet += c.process(dry);
        }
        wet *= 0.25;
        for a in &mut self.allpasses {
            wet = a.process(wet);
        }
        Some(dry * (1.0 - self.mix) + wet * self.mix)
    }
}

impl<I: Source<Item = f32>> Source for Reverb<I> {
    fn current_frame_len(&self) -> Option<usize> {
        self.inner.current_frame_len()
    }
    fn channels(&self) -> u16 {
        self.inner.channels()
    }
    fn sample_rate(&self) -> u32 {
        self.inner.sample_rate()
    }
    fn total_duration(&self) -> Option<Duration> {
        self.inner.total_duration()
    }
}

struct StaticTremolo<I> {
    inner: I,
    rate: f32,
    depth: f32,
    sample_idx: u64,
}

impl<I: Source<Item = f32>> StaticTremolo<I> {
    fn new(inner: I, rate: f32, depth: f32) -> Self {
        Self {
            inner,
            rate: rate.max(0.0),
            depth: depth.clamp(0.0, 1.0),
            sample_idx: 0,
        }
    }
}

impl<I: Source<Item = f32>> Iterator for StaticTremolo<I> {
    type Item = f32;
    fn next(&mut self) -> Option<f32> {
        let s = self.inner.next()?;
        let channels = self.inner.channels().max(1) as u64;
        let sr = self.inner.sample_rate().max(1) as f32;
        let frame = self.sample_idx / channels;
        let t = frame as f32 / sr;
        let lfo = (2.0 * std::f32::consts::PI * self.rate * t).sin();
        let gain = (1.0 - self.depth) + self.depth * (lfo * 0.5 + 0.5);
        self.sample_idx = self.sample_idx.wrapping_add(1);
        Some(s * gain)
    }
}

impl<I: Source<Item = f32>> Source for StaticTremolo<I> {
    fn current_frame_len(&self) -> Option<usize> {
        self.inner.current_frame_len()
    }
    fn channels(&self) -> u16 {
        self.inner.channels()
    }
    fn sample_rate(&self) -> u32 {
        self.inner.sample_rate()
    }
    fn total_duration(&self) -> Option<Duration> {
        self.inner.total_duration()
    }
}

struct Looper {
    inner: Box<dyn Source<Item = f32> + Send>,
    bytes: Arc<Vec<u8>>,
    shaders: Vec<Shader>,
    attached: Vec<AttachedShader>,
    looped: Arc<AtomicBool>,
    loop_count: Arc<AtomicU64>,
    sample_rate: u32,
    channels: u16,
}

impl Looper {
    fn build(
        bytes: Arc<Vec<u8>>,
        shaders: Vec<Shader>,
        attached: Vec<AttachedShader>,
        looped: Arc<AtomicBool>,
        loop_count: Arc<AtomicU64>,
    ) -> mlua::Result<Self> {
        let inner = build_source(&bytes, &shaders, &attached)?;
        let sample_rate = inner.sample_rate();
        let channels = inner.channels();
        Ok(Self {
            inner,
            bytes,
            shaders,
            attached,
            looped,
            loop_count,
            sample_rate,
            channels,
        })
    }
}

impl Iterator for Looper {
    type Item = f32;
    fn next(&mut self) -> Option<f32> {
        if let Some(s) = self.inner.next() {
            return Some(s);
        }
        if !self.looped.load(Ordering::Relaxed) {
            return None;
        }
        let new_source = build_source(&self.bytes, &self.shaders, &self.attached).ok()?;
        self.inner = new_source;
        self.loop_count.fetch_add(1, Ordering::Relaxed);
        self.inner.next()
    }
}

impl Source for Looper {
    fn current_frame_len(&self) -> Option<usize> {
        None
    }
    fn channels(&self) -> u16 {
        self.channels
    }
    fn sample_rate(&self) -> u32 {
        self.sample_rate
    }
    fn total_duration(&self) -> Option<Duration> {
        None
    }
}

struct CameraSpatial {
    inner: Box<dyn Source<Item = f32> + Send>,
    position: Arc<Mutex<Option<SpatialPos>>>,
    channel_idx: u16,
    last_left: f32,
    last_right: f32,
}

impl CameraSpatial {
    fn new(
        inner: Box<dyn Source<Item = f32> + Send>,
        position: Arc<Mutex<Option<SpatialPos>>>,
    ) -> Self {
        Self {
            inner,
            position,
            channel_idx: 0,
            last_left: 0.0,
            last_right: 0.0,
        }
    }
}

impl Iterator for CameraSpatial {
    type Item = f32;
    fn next(&mut self) -> Option<f32> {
        let s = self.inner.next()?;
        let channels = self.inner.channels().max(1);

        let pos = match *self.position.lock().unwrap() {
            Some(p) => p,
            None => return Some(s),
        };

        if self.channel_idx == 0 {
            let cam = crate::libs::renderable::camera_snapshot();
            let cx = cam.cframe.position.x;
            let cy = cam.cframe.position.y;
            let cz = cam.cframe.position.z;
            let dx = pos.x - cx;
            let dy = pos.y - cy;
            let dz = pos.z - cz;
            let dist = (dx * dx + dy * dy + dz * dz).sqrt();
            let atten = (1.0 - (dist / pos.falloff)).clamp(0.0, 1.0).powf(1.6);

            let yaw = cam.cframe.rotation.y;
            let cy_ = yaw.cos();
            let sy_ = yaw.sin();
            let local_x = cy_ * dx + sy_ * dz;
            let pan = if dist > 0.001 {
                (local_x / dist).clamp(-1.0, 1.0)
            } else {
                0.0
            };

            let amp = s * atten;
            let theta = (pan + 1.0) * std::f32::consts::FRAC_PI_4;
            self.last_left = amp * theta.cos();
            self.last_right = amp * theta.sin();

            self.channel_idx = (self.channel_idx + 1) % channels.max(2);
            Some(self.last_left)
        } else {
            let v = self.last_right;
            self.channel_idx = (self.channel_idx + 1) % channels.max(2);
            Some(v)
        }
    }
}

impl Source for CameraSpatial {
    fn current_frame_len(&self) -> Option<usize> {
        self.inner.current_frame_len()
    }
    fn channels(&self) -> u16 {
        self.inner.channels().max(2)
    }
    fn sample_rate(&self) -> u32 {
        self.inner.sample_rate()
    }
    fn total_duration(&self) -> Option<Duration> {
        self.inner.total_duration()
    }
}
