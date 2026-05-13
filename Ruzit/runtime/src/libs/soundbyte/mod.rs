use std::cell::RefCell;
use std::collections::VecDeque;
use std::io::Cursor;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use mlua::{
    AnyUserData, Lua, MultiValue, RegistryKey, Table, UserData, UserDataFields, UserDataMethods,
    Value,
};
use rodio::{Decoder, OutputStream, OutputStreamHandle, Sink, Source};

use crate::libs::primitives::{CFrame, Vector};
use crate::libs::signal;

thread_local! {
    static OUTPUT: RefCell<Option<OutputStreamHandle>> = const { RefCell::new(None) };
    static BYTE_REGISTRY: RefCell<Vec<Arc<Mutex<ByteSinkState>>>> = const { RefCell::new(Vec::new()) };
    static PLAYER_REGISTRY: RefCell<Vec<Arc<Mutex<PlayerState>>>> = const { RefCell::new(Vec::new()) };
    static BYTE_SOURCE_REGISTRY: RefCell<Vec<Arc<Mutex<ByteSourceState>>>> = const { RefCell::new(Vec::new()) };
    #[cfg(feature = "voice")]
    static VOICE_REGISTRY: RefCell<Vec<Arc<Mutex<VoiceChannelState>>>> = const { RefCell::new(Vec::new()) };
}

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

fn output_handle() -> mlua::Result<OutputStreamHandle> {
    OUTPUT.with(|cell| {
        let mut b = cell.borrow_mut();
        if b.is_none() {
            let (stream, handle) = OutputStream::try_default()
                .map_err(|e| mlua::Error::RuntimeError(format!("SoundByte audio init: {e}")))?;
            std::mem::forget(stream);
            *b = Some(handle);
        }
        Ok(b.as_ref().unwrap().clone())
    })
}

pub fn create(lua: &Lua) -> mlua::Result<Table> {
    let t = lua.create_table()?;

    t.set("VoiceFlagEnabled", cfg!(feature = "voice"))?;

    t.set(
        "NewPlayer",
        lua.create_function(|lua, asset: AnyUserData| -> mlua::Result<Player> {
            let sd = asset.borrow::<crate::libs::sfx::SoundData>().map_err(|_| {
                mlua::Error::RuntimeError(
                    "SoundByte.NewPlayer expects a SoundData (Asset.GetAsset(\"Sound\", ...))"
                        .into(),
                )
            })?;
            Player::new(lua, &sd)
        })?,
    )?;

    t.set(
        "NewOutput",
        lua.create_function(|_, _: ()| -> mlua::Result<OutputNode> {
            Ok(OutputNode::new())
        })?,
    )?;

    t.set(
        "NewByte",
        lua.create_function(|lua, _: ()| -> mlua::Result<ByteSink> { ByteSink::new(lua) })?,
    )?;

    t.set(
        "NewByteSource",
        lua.create_function(
            |_, args: MultiValue| -> mlua::Result<ByteSource> {
                let mut iter = args.into_iter();
                let sample_rate = match iter.next() {
                    Some(Value::Nil) | None => 48000u32,
                    Some(Value::Integer(i)) => i.max(1) as u32,
                    Some(Value::Number(n)) => n.max(1.0) as u32,
                    Some(_) => {
                        return Err(mlua::Error::RuntimeError(
                            "SoundByte.NewByteSource: sampleRate must be a number".into(),
                        ))
                    }
                };
                let channels = match iter.next() {
                    Some(Value::Nil) | None => 1u16,
                    Some(Value::Integer(i)) => i.clamp(1, 8) as u16,
                    Some(Value::Number(n)) => (n as i64).clamp(1, 8) as u16,
                    Some(_) => {
                        return Err(mlua::Error::RuntimeError(
                            "SoundByte.NewByteSource: channels must be a number".into(),
                        ))
                    }
                };
                Ok(ByteSource::new(sample_rate, channels))
            },
        )?,
    )?;

    t.set(
        "GetModifier",
        lua.create_function(|_, kind: String| -> mlua::Result<Modifier> {
            let mk = ModifierKind::parse(&kind).ok_or_else(|| {
                mlua::Error::RuntimeError(format!(
                    "SoundByte.GetModifier: unknown modifier '{kind}'"
                ))
            })?;
            Ok(Modifier::new(mk))
        })?,
    )?;

    #[cfg(feature = "voice")]
    t.set(
        "GetVoiceChannel",
        lua.create_function(|_, _: ()| -> mlua::Result<VoiceChannel> {
            Ok(VoiceChannel::new())
        })?,
    )?;
    #[cfg(not(feature = "voice"))]
    t.set(
        "GetVoiceChannel",
        lua.create_function(|_, _: ()| -> mlua::Result<Value> {
            Err(mlua::Error::RuntimeError(
                "SoundByte.GetVoiceChannel: voice feature is disabled in this build (compile with --features voice)".into(),
            ))
        })?,
    )?;

    t.set("Link", lua.create_function(link)?)?;

    Ok(t)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModifierKind {
    Volume,
    Speed,
    Pitch,
    Pan,
    Distortion,
    LowPass,
    HighPass,
    BandPass,
    Echo,
    Reverb,
    Tremolo,
    Vibrato,
    FadeIn,
    FadeOut,
    Delay,
    PlaybackSpeed,
    BitCrusher,
    NoiseGate,
    Compressor,
    Limiter,
    Saturation,
    RingMod,
    Chorus,
    Flanger,
    StereoWiden,
    Wobble,
    Telephone,
    Underwater,
    Mute,
}

impl ModifierKind {
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "Volume" => Self::Volume,
            "Speed" => Self::Speed,
            "Pitch" => Self::Pitch,
            "Pan" => Self::Pan,
            "Distortion" => Self::Distortion,
            "LowPass" => Self::LowPass,
            "HighPass" => Self::HighPass,
            "BandPass" => Self::BandPass,
            "Echo" => Self::Echo,
            "Reverb" => Self::Reverb,
            "Tremolo" => Self::Tremolo,
            "Vibrato" => Self::Vibrato,
            "FadeIn" => Self::FadeIn,
            "FadeOut" => Self::FadeOut,
            "Delay" => Self::Delay,
            "PlaybackSpeed" => Self::PlaybackSpeed,
            "BitCrusher" => Self::BitCrusher,
            "NoiseGate" => Self::NoiseGate,
            "Compressor" => Self::Compressor,
            "Limiter" => Self::Limiter,
            "Saturation" => Self::Saturation,
            "RingMod" => Self::RingMod,
            "Chorus" => Self::Chorus,
            "Flanger" => Self::Flanger,
            "StereoWiden" => Self::StereoWiden,
            "Wobble" => Self::Wobble,
            "Telephone" => Self::Telephone,
            "Underwater" => Self::Underwater,
            "Mute" => Self::Mute,
            _ => return None,
        })
    }

    pub fn label(&self) -> &'static str {
        match self {
            Self::Volume => "Volume",
            Self::Speed => "Speed",
            Self::Pitch => "Pitch",
            Self::Pan => "Pan",
            Self::Distortion => "Distortion",
            Self::LowPass => "LowPass",
            Self::HighPass => "HighPass",
            Self::BandPass => "BandPass",
            Self::Echo => "Echo",
            Self::Reverb => "Reverb",
            Self::Tremolo => "Tremolo",
            Self::Vibrato => "Vibrato",
            Self::FadeIn => "FadeIn",
            Self::FadeOut => "FadeOut",
            Self::Delay => "Delay",
            Self::PlaybackSpeed => "PlaybackSpeed",
            Self::BitCrusher => "BitCrusher",
            Self::NoiseGate => "NoiseGate",
            Self::Compressor => "Compressor",
            Self::Limiter => "Limiter",
            Self::Saturation => "Saturation",
            Self::RingMod => "RingMod",
            Self::Chorus => "Chorus",
            Self::Flanger => "Flanger",
            Self::StereoWiden => "StereoWiden",
            Self::Wobble => "Wobble",
            Self::Telephone => "Telephone",
            Self::Underwater => "Underwater",
            Self::Mute => "Mute",
        }
    }

    fn default_value(&self) -> f32 {
        match self {
            Self::Volume | Self::Speed | Self::Pitch | Self::PlaybackSpeed => 1.0,
            Self::Pan | Self::Distortion | Self::FadeIn | Self::FadeOut | Self::Delay => 0.0,
            Self::LowPass => 22050.0,
            Self::HighPass => 0.0,
            Self::BandPass => 1000.0,
            Self::Echo => 0.3,
            Self::Reverb => 0.3,
            Self::Tremolo => 5.0,
            Self::Vibrato => 5.0,
            Self::BitCrusher => 8.0,
            Self::NoiseGate => 0.05,
            Self::Compressor => 0.5,
            Self::Limiter => 0.95,
            Self::Saturation => 0.3,
            Self::RingMod => 200.0,
            Self::Chorus => 0.5,
            Self::Flanger => 0.5,
            Self::StereoWiden => 0.3,
            Self::Wobble => 1.0,
            Self::Telephone => 1.0,
            Self::Underwater => 1.0,
            Self::Mute => 0.0,
        }
    }
}

pub struct ModifierState {
    pub id: u64,
    pub kind: ModifierKind,
    pub enabled: bool,
    pub min: f32,
    pub max: f32,
    pub value: f32,
}

#[derive(Clone)]
pub struct Modifier {
    pub state: Arc<Mutex<ModifierState>>,
}

impl Modifier {
    pub fn new(kind: ModifierKind) -> Self {
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        let v = kind.default_value();
        Self {
            state: Arc::new(Mutex::new(ModifierState {
                id,
                kind,
                enabled: true,
                min: v,
                max: v,
                value: v,
            })),
        }
    }
}

impl UserData for Modifier {
    fn add_fields<F: UserDataFields<Self>>(f: &mut F) {
        f.add_field_method_get("Kind", |_, this| {
            Ok(this.state.lock().unwrap().kind.label().to_string())
        });
        f.add_field_method_get("Enabled", |_, this| Ok(this.state.lock().unwrap().enabled));
        f.add_field_method_set("Enabled", |_, this, v: bool| {
            this.state.lock().unwrap().enabled = v;
            Ok(())
        });
        f.add_field_method_get("Min", |_, this| Ok(this.state.lock().unwrap().min));
        f.add_field_method_set("Min", |_, this, v: f32| {
            let mut s = this.state.lock().unwrap();
            s.min = v;
            if s.max < s.min {
                s.max = s.min;
            }
            s.value = s.value.clamp(s.min, s.max);
            Ok(())
        });
        f.add_field_method_get("Max", |_, this| Ok(this.state.lock().unwrap().max));
        f.add_field_method_set("Max", |_, this, v: f32| {
            let mut s = this.state.lock().unwrap();
            s.max = v;
            if s.min > s.max {
                s.min = s.max;
            }
            s.value = s.value.clamp(s.min, s.max);
            Ok(())
        });
        f.add_field_method_get("Value", |_, this| Ok(this.state.lock().unwrap().value));
        f.add_field_method_set("Value", |_, this, v: f32| {
            let mut s = this.state.lock().unwrap();
            s.value = v.clamp(s.min.min(s.max), s.min.max(s.max));
            Ok(())
        });
    }
}

#[derive(Clone, Copy, Debug)]
pub struct SpatialState {
    pub use_3d: bool,
    pub position: Vector,
    pub falloff_min: f32,
    pub falloff_max: f32,
    pub volume: f32,
}

pub struct OutputState {
    pub id: u64,
    pub alive: bool,
    pub sink: Mutex<Option<Sink>>,
    pub spatial: Arc<Mutex<SpatialState>>,
    pub explicit_cframe: Mutex<Option<CFrame>>,
    pub follow_target: Mutex<Option<Arc<Mutex<crate::libs::renderable::PartState>>>>,
    pub linked_links: Mutex<Vec<u64>>,
}

#[derive(Clone)]
pub struct OutputNode {
    pub state: Arc<OutputState>,
}

impl OutputNode {
    pub fn new() -> Self {
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        Self {
            state: Arc::new(OutputState {
                id,
                alive: true,
                sink: Mutex::new(None),
                spatial: Arc::new(Mutex::new(SpatialState {
                    use_3d: false,
                    position: Vector::new(0.0, 0.0, 0.0),
                    falloff_min: 0.0,
                    falloff_max: 20.0,
                    volume: 1.0,
                })),
                explicit_cframe: Mutex::new(None),
                follow_target: Mutex::new(None),
                linked_links: Mutex::new(Vec::new()),
            }),
        }
    }

}

impl UserData for OutputNode {
    fn add_fields<F: UserDataFields<Self>>(f: &mut F) {
        f.add_field_method_get("Volume", |_, this| Ok(this.state.spatial.lock().unwrap().volume));
        f.add_field_method_set("Volume", |_, this, v: f32| {
            this.state.spatial.lock().unwrap().volume = v.max(0.0);
            if let Some(sink) = this.state.sink.lock().unwrap().as_ref() {
                sink.set_volume(v.max(0.0));
            }
            Ok(())
        });

        f.add_field_method_get("FalloffMinDistance", |_, this| {
            Ok(this.state.spatial.lock().unwrap().falloff_min)
        });
        f.add_field_method_set("FalloffMinDistance", |_, this, v: f32| {
            let mut s = this.state.spatial.lock().unwrap();
            s.falloff_min = v.max(0.0);
            if s.falloff_max < s.falloff_min {
                s.falloff_max = s.falloff_min;
            }
            Ok(())
        });

        f.add_field_method_get("FalloffMaxDistance", |_, this| {
            Ok(this.state.spatial.lock().unwrap().falloff_max)
        });
        f.add_field_method_set("FalloffMaxDistance", |_, this, v: f32| {
            let mut s = this.state.spatial.lock().unwrap();
            s.falloff_max = v.max(0.0);
            if s.falloff_min > s.falloff_max {
                s.falloff_min = s.falloff_max;
            }
            Ok(())
        });

        f.add_field_method_get("CFrame", |_, this| {
            Ok(*this.state.explicit_cframe.lock().unwrap())
        });
        f.add_field_method_set("CFrame", |_, this, v: Value| {
            let mut cf_slot = this.state.explicit_cframe.lock().unwrap();
            let mut spat = this.state.spatial.lock().unwrap();
            match v {
                Value::Nil => {
                    *cf_slot = None;
                    if this.state.follow_target.lock().unwrap().is_none() {
                        spat.use_3d = false;
                    }
                }
                Value::UserData(ud) => {
                    let cf = *ud.borrow::<CFrame>().map_err(|_| {
                        mlua::Error::RuntimeError(
                            "OutputNode.CFrame expects a CFrame or nil".into(),
                        )
                    })?;
                    *cf_slot = Some(cf);
                    spat.use_3d = true;
                    spat.position = cf.position;
                }
                _ => {
                    return Err(mlua::Error::RuntimeError(
                        "OutputNode.CFrame expects a CFrame or nil".into(),
                    ));
                }
            }
            Ok(())
        });

        f.add_field_method_get("Position", |_, this| {
            Ok(this.state.spatial.lock().unwrap().position)
        });
        f.add_field_method_set("Position", |_, this, v: Value| {
            let vec = crate::libs::primitives::value_to_vector_opt(&v).ok_or_else(|| {
                mlua::Error::RuntimeError("OutputNode.Position expects a Vector".into())
            })?;
            this.state.spatial.lock().unwrap().position = vec;
            Ok(())
        });
    }

    fn add_methods<M: UserDataMethods<Self>>(m: &mut M) {
        m.add_method("Follow", |_, this, target: AnyUserData| -> mlua::Result<()> {
            let part = target
                .borrow::<crate::libs::renderable::PartHandle>()
                .map_err(|_| {
                    mlua::Error::RuntimeError(
                        "OutputNode:Follow expects a BasePart".into(),
                    )
                })?;
            *this.state.follow_target.lock().unwrap() = Some(part.state.clone());
            this.state.spatial.lock().unwrap().use_3d = true;
            Ok(())
        });
        m.add_method("Unfollow", |_, this, _: ()| -> mlua::Result<()> {
            *this.state.follow_target.lock().unwrap() = None;
            if this.state.explicit_cframe.lock().unwrap().is_none() {
                this.state.spatial.lock().unwrap().use_3d = false;
            }
            Ok(())
        });
    }
}

pub struct Route {
    pub link_id: u64,
    pub output: Option<Arc<OutputState>>,
    pub byte: Option<Arc<Mutex<ByteSinkState>>>,
    pub modifiers: Vec<Arc<Mutex<ModifierState>>>,
}

pub struct UpdateLink {
    pub interval: f64,
    pub signal_key: Arc<RegistryKey>,
    pub signal_table: Table,
    pub next_fire: f64,
}

pub struct PlayerState {
    pub id: u64,
    pub source_id: u64,
    pub bytes: Arc<Vec<u8>>,
    pub source_path: String,
    pub alive: Arc<AtomicBool>,
    pub looped: Arc<AtomicBool>,
    pub loop_count: Arc<AtomicU64>,
    pub last_loop_count: u64,
    pub playing: Arc<AtomicBool>,
    pub started_key: Arc<RegistryKey>,
    pub stopped_key: Arc<RegistryKey>,
    pub did_loop_key: Arc<RegistryKey>,
    pub started_table: Table,
    pub stopped_table: Table,
    pub did_loop_table: Table,
    pub routes: Vec<Route>,
    pub started_fired_for_active: bool,
    pub active_sink_indicator: Option<Arc<AtomicBool>>,
    pub total_duration: Option<Duration>,
    pub play_started_at: Option<std::time::Instant>,
    pub pending_offset: Duration,
    pub update_links: Vec<UpdateLink>,
}

#[derive(Clone)]
pub struct Player {
    pub state: Arc<Mutex<PlayerState>>,
}

impl Player {
    fn new(lua: &Lua, sd: &crate::libs::sfx::SoundData) -> mlua::Result<Self> {
        let started = signal::new_instance(lua)?;
        let stopped = signal::new_instance(lua)?;
        let did_loop = signal::new_instance(lua)?;
        let started_key = Arc::new(lua.create_registry_value(started.clone())?);
        let stopped_key = Arc::new(lua.create_registry_value(stopped.clone())?);
        let did_loop_key = Arc::new(lua.create_registry_value(did_loop.clone())?);

        let total_duration = Decoder::new(Cursor::new((*sd.bytes).clone()))
            .ok()
            .and_then(|d| d.total_duration());

        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        let state = Arc::new(Mutex::new(PlayerState {
            id,
            source_id: sd.id,
            bytes: sd.bytes.clone(),
            source_path: sd.source.clone(),
            alive: Arc::new(AtomicBool::new(true)),
            looped: Arc::new(AtomicBool::new(false)),
            loop_count: Arc::new(AtomicU64::new(0)),
            last_loop_count: 0,
            playing: Arc::new(AtomicBool::new(false)),
            started_key,
            stopped_key,
            did_loop_key,
            started_table: started,
            stopped_table: stopped,
            did_loop_table: did_loop,
            routes: Vec::new(),
            started_fired_for_active: false,
            active_sink_indicator: None,
            total_duration,
            play_started_at: None,
            pending_offset: Duration::ZERO,
            update_links: Vec::new(),
        }));
        PLAYER_REGISTRY.with(|r| r.borrow_mut().push(state.clone()));
        Ok(Self { state })
    }
}

impl UserData for Player {
    fn add_fields<F: UserDataFields<Self>>(f: &mut F) {
        f.add_field_method_get("Started", |_, this| {
            Ok(this.state.lock().unwrap().started_table.clone())
        });
        f.add_field_method_get("Stopped", |_, this| {
            Ok(this.state.lock().unwrap().stopped_table.clone())
        });
        f.add_field_method_get("DidLoop", |_, this| {
            Ok(this.state.lock().unwrap().did_loop_table.clone())
        });
        f.add_field_method_get("Source", |_, this| Ok(this.state.lock().unwrap().source_path.clone()));

        f.add_field_method_get("Looped", |_, this| {
            Ok(this.state.lock().unwrap().looped.load(Ordering::Relaxed))
        });
        f.add_field_method_set("Looped", |_, this, v: bool| {
            this.state.lock().unwrap().looped.store(v, Ordering::Relaxed);
            Ok(())
        });

        f.add_field_method_get("IsPlaying", |_, this| {
            Ok(this.state.lock().unwrap().playing.load(Ordering::Relaxed))
        });
        f.add_field_method_get("IsAlive", |_, this| {
            Ok(this.state.lock().unwrap().alive.load(Ordering::Relaxed))
        });
        f.add_field_method_get("Duration", |_, this| {
            Ok(this
                .state
                .lock()
                .unwrap()
                .total_duration
                .map(|d| d.as_secs_f64())
                .unwrap_or(0.0))
        });
        f.add_field_method_get("TimePosition", |_, this| -> mlua::Result<f64> {
            let s = this.state.lock().unwrap();
            if s.playing.load(Ordering::Relaxed) {
                let elapsed = s
                    .play_started_at
                    .map(|t| t.elapsed())
                    .unwrap_or(Duration::ZERO);
                let total = s.pending_offset + elapsed;
                let secs = if s.looped.load(Ordering::Relaxed) {
                    if let Some(d) = s.total_duration {
                        let dsec = d.as_secs_f64();
                        if dsec > 0.0 {
                            total.as_secs_f64() % dsec
                        } else {
                            total.as_secs_f64()
                        }
                    } else {
                        total.as_secs_f64()
                    }
                } else {
                    total.as_secs_f64()
                };
                Ok(secs)
            } else {
                Ok(s.pending_offset.as_secs_f64())
            }
        });
        f.add_field_method_set("TimePosition", |_, this, secs: f64| {
            let mut s = this.state.lock().unwrap();
            let target = Duration::from_secs_f64(secs.max(0.0));
            if s.playing.load(Ordering::Relaxed) {
                s.pending_offset = target;
                s.play_started_at = Some(std::time::Instant::now());
                drop(s);
                let _ = restart_player_at_offset(this);
            } else {
                s.pending_offset = target;
            }
            Ok(())
        });
    }

    fn add_methods<M: UserDataMethods<Self>>(m: &mut M) {
        m.add_method("Play", |_, this, _: ()| -> mlua::Result<()> {
            let s = this.state.lock().unwrap();
            if !s.alive.load(Ordering::Relaxed) {
                return Err(mlua::Error::RuntimeError(
                    "Player: source data has been dropped, this Player is no longer playable"
                        .into(),
                ));
            }
            if s.routes.is_empty() {
                return Err(mlua::Error::RuntimeError(
                    "Player:Play has no linked OutputNode or ByteSink. Call SoundByte.Link first."
                        .into(),
                ));
            }
            let bytes = s.bytes.clone();
            let looped = s.looped.clone();
            let loop_count = s.loop_count.clone();
            let offset = s.pending_offset;
            let routes: Vec<(Option<Arc<OutputState>>, Option<Arc<Mutex<ByteSinkState>>>, Vec<Arc<Mutex<ModifierState>>>)> =
                s.routes
                    .iter()
                    .map(|r| (r.output.clone(), r.byte.clone(), r.modifiers.clone()))
                    .collect();
            drop(s);

            play_routes(bytes, looped, loop_count.clone(), offset, &routes)?;

            let mut s = this.state.lock().unwrap();
            s.playing.store(true, Ordering::Relaxed);
            s.started_fired_for_active = false;
            s.last_loop_count = loop_count.load(Ordering::Relaxed);
            s.play_started_at = Some(std::time::Instant::now());
            for u in s.update_links.iter_mut() {
                u.next_fire = u.interval;
            }
            Ok(())
        });

        m.add_method("Stop", |_, this, _: ()| -> mlua::Result<()> {
            let mut s = this.state.lock().unwrap();
            for r in s.routes.iter() {
                if let Some(out) = &r.output {
                    if let Some(sink) = out.sink.lock().unwrap().as_ref() {
                        sink.stop();
                    }
                }
            }
            s.playing.store(false, Ordering::Relaxed);
            s.play_started_at = None;
            Ok(())
        });

        m.add_method("IsPlaying", |_, this, _: ()| {
            Ok(this.state.lock().unwrap().playing.load(Ordering::Relaxed))
        });

        m.add_method("Destroy", |_, this, _: ()| -> mlua::Result<()> {
            let mut s = this.state.lock().unwrap();
            s.alive.store(false, Ordering::Relaxed);
            s.playing.store(false, Ordering::Relaxed);
            for r in s.routes.iter() {
                if let Some(out) = &r.output {
                    if let Some(sink) = out.sink.lock().unwrap().as_ref() {
                        sink.stop();
                    }
                }
            }
            s.routes.clear();
            s.update_links.clear();
            s.play_started_at = None;
            Ok(())
        });

        m.add_method("LinkToUpdate", |lua, this, interval: f64| -> mlua::Result<Table> {
            if !(interval > 0.0) {
                return Err(mlua::Error::RuntimeError(
                    "Player:LinkToUpdate: interval must be > 0".into(),
                ));
            }
            let sig = signal::new_instance(lua)?;
            let key = Arc::new(lua.create_registry_value(sig.clone())?);
            let mut s = this.state.lock().unwrap();
            s.update_links.push(UpdateLink {
                interval,
                signal_key: key,
                signal_table: sig.clone(),
                next_fire: interval,
            });
            Ok(sig)
        });
    }
}

fn play_routes(
    bytes: Arc<Vec<u8>>,
    looped: Arc<AtomicBool>,
    loop_count: Arc<AtomicU64>,
    offset: Duration,
    routes: &[(
        Option<Arc<OutputState>>,
        Option<Arc<Mutex<ByteSinkState>>>,
        Vec<Arc<Mutex<ModifierState>>>,
    )],
) -> mlua::Result<()> {
    for (out_opt, byte_opt, modifiers) in routes.iter() {
        if let Some(out) = out_opt {
            let decoder = Decoder::new(Cursor::new((*bytes).clone())).map_err(|e| {
                mlua::Error::RuntimeError(format!("SoundByte decode: {e}"))
            })?;
            let mut source: Box<dyn Source<Item = f32> + Send> =
                Box::new(decoder.convert_samples::<f32>());
            source = Box::new(LoopingSource {
                bytes: bytes.clone(),
                looped: looped.clone(),
                loop_count: loop_count.clone(),
                inner: source,
            });
            source = apply_modifier_chain(source, modifiers);
            if !offset.is_zero() {
                source = Box::new(source.skip_duration(offset));
            }
            source = Box::new(SpatialAdapter {
                inner: source,
                state: out.spatial.clone(),
                chan_idx: 0,
                last_l: 0.0,
                last_r: 0.0,
            });

            let mut sink_lock = out.sink.lock().unwrap();
            if sink_lock.is_none() {
                let h = output_handle()?;
                let new_sink = Sink::try_new(&h).map_err(|e| {
                    mlua::Error::RuntimeError(format!("SoundByte sink: {e}"))
                })?;
                *sink_lock = Some(new_sink);
            }
            if let Some(sink) = sink_lock.as_ref() {
                sink.set_volume(out.spatial.lock().unwrap().volume);
                sink.append(source);
                sink.play();
            }
        } else if let Some(byte_sink) = byte_opt {
            let decoder = Decoder::new(Cursor::new((*bytes).clone())).map_err(|e| {
                mlua::Error::RuntimeError(format!("SoundByte decode (byte): {e}"))
            })?;
            let mut sample_rate = decoder.sample_rate();
            let mut channels = decoder.channels();
            let mut source: Box<dyn Source<Item = f32> + Send> =
                Box::new(decoder.convert_samples::<f32>());
            source = apply_modifier_chain(source, modifiers);
            let processed: Vec<f32> = source.collect();
            if sample_rate == 0 {
                sample_rate = 44100;
            }
            if channels == 0 {
                channels = 2;
            }
            let mut bs = byte_sink.lock().unwrap();
            bs.queue_pcm(processed, sample_rate, channels);
        }
    }
    Ok(())
}

fn restart_player_at_offset(player: &Player) -> mlua::Result<()> {
    let s = player.state.lock().unwrap();
    let bytes = s.bytes.clone();
    let looped = s.looped.clone();
    let loop_count = s.loop_count.clone();
    let offset = s.pending_offset;
    let routes: Vec<(Option<Arc<OutputState>>, Option<Arc<Mutex<ByteSinkState>>>, Vec<Arc<Mutex<ModifierState>>>)> =
        s.routes
            .iter()
            .map(|r| (r.output.clone(), r.byte.clone(), r.modifiers.clone()))
            .collect();
    for (out_opt, _, _) in routes.iter() {
        if let Some(out) = out_opt {
            if let Some(sink) = out.sink.lock().unwrap().as_ref() {
                sink.stop();
            }
            *out.sink.lock().unwrap() = None;
        }
    }
    drop(s);
    play_routes(bytes, looped, loop_count, offset, &routes)
}

struct LoopingSource {
    bytes: Arc<Vec<u8>>,
    looped: Arc<AtomicBool>,
    loop_count: Arc<AtomicU64>,
    inner: Box<dyn Source<Item = f32> + Send>,
}

impl Iterator for LoopingSource {
    type Item = f32;
    fn next(&mut self) -> Option<f32> {
        if let Some(s) = self.inner.next() {
            return Some(s);
        }
        if !self.looped.load(Ordering::Relaxed) {
            return None;
        }
        let decoder = Decoder::new(Cursor::new((*self.bytes).clone())).ok()?;
        self.inner = Box::new(decoder.convert_samples::<f32>());
        self.loop_count.fetch_add(1, Ordering::Relaxed);
        self.inner.next()
    }
}

impl Source for LoopingSource {
    fn current_frame_len(&self) -> Option<usize> {
        None
    }
    fn channels(&self) -> u16 {
        self.inner.channels()
    }
    fn sample_rate(&self) -> u32 {
        self.inner.sample_rate()
    }
    fn total_duration(&self) -> Option<Duration> {
        None
    }
}

struct SpatialAdapter {
    inner: Box<dyn Source<Item = f32> + Send>,
    state: Arc<Mutex<SpatialState>>,
    chan_idx: u16,
    last_l: f32,
    last_r: f32,
}

impl Iterator for SpatialAdapter {
    type Item = f32;
    fn next(&mut self) -> Option<f32> {
        if self.chan_idx == 1 {
            self.chan_idx = 0;
            return Some(self.last_r);
        }
        let raw = self.inner.next()?;
        let st = *self.state.lock().unwrap();
        let (atten, pan) = if st.use_3d {
            let cam = crate::libs::renderable::camera_snapshot();
            let dx = st.position.x - cam.cframe.position.x;
            let dy = st.position.y - cam.cframe.position.y;
            let dz = st.position.z - cam.cframe.position.z;
            let dist = (dx * dx + dy * dy + dz * dz).sqrt();
            let a = if dist <= st.falloff_min {
                1.0
            } else if dist >= st.falloff_max {
                0.0
            } else {
                let span = (st.falloff_max - st.falloff_min).max(0.001);
                let t = (dist - st.falloff_min) / span;
                (1.0 - t).clamp(0.0, 1.0).powf(1.6)
            };
            let yaw = cam.cframe.rotation.y;
            let local_x = yaw.cos() * dx + yaw.sin() * dz;
            let p = if dist > 0.001 { (local_x / dist).clamp(-1.0, 1.0) } else { 0.0 };
            (a, p)
        } else {
            (1.0, 0.0)
        };

        let amp = raw * atten;
        let theta = (pan + 1.0) * std::f32::consts::FRAC_PI_4;
        self.last_l = amp * theta.cos();
        self.last_r = amp * theta.sin();
        self.chan_idx = 1;
        Some(self.last_l)
    }
}

impl Source for SpatialAdapter {
    fn current_frame_len(&self) -> Option<usize> {
        self.inner.current_frame_len()
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

pub struct ByteSinkState {
    pub id: u64,
    pub alive: bool,
    pub on_packet_key: Arc<RegistryKey>,
    pub on_packet_table: Table,
    pub pending_packets: VecDeque<Vec<u8>>,
    pub sample_rate: u32,
    pub channels: u16,
    pub accum: Vec<f32>,
    pub samples_per_packet: usize,
}

pub struct ByteSink {
    pub state: Arc<Mutex<ByteSinkState>>,
}

impl ByteSinkState {
    pub fn queue_pcm(&mut self, pcm: Vec<f32>, sample_rate: u32, channels: u16) {
        self.sample_rate = sample_rate;
        self.channels = channels;
        if self.samples_per_packet == 0 {
            self.samples_per_packet =
                (sample_rate as usize / 20).max(64) * channels as usize;
        }
        self.accum.extend(pcm);
        while self.accum.len() >= self.samples_per_packet {
            let drained: Vec<f32> = self.accum.drain(..self.samples_per_packet).collect();
            let mut bytes = Vec::with_capacity(drained.len() * 4);
            for s in drained {
                bytes.extend_from_slice(&s.to_le_bytes());
            }
            self.pending_packets.push_back(bytes);
        }
    }
}

impl ByteSink {
    fn new(lua: &Lua) -> mlua::Result<Self> {
        let on_packet = signal::new_instance(lua)?;
        let on_packet_key = Arc::new(lua.create_registry_value(on_packet.clone())?);
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        let state = Arc::new(Mutex::new(ByteSinkState {
            id,
            alive: true,
            on_packet_key,
            on_packet_table: on_packet,
            pending_packets: VecDeque::new(),
            sample_rate: 0,
            channels: 0,
            accum: Vec::new(),
            samples_per_packet: 0,
        }));
        BYTE_REGISTRY.with(|r| r.borrow_mut().push(state.clone()));
        Ok(Self { state })
    }
}

impl UserData for ByteSink {
    fn add_fields<F: UserDataFields<Self>>(f: &mut F) {
        f.add_field_method_get("OnPacket", |_, this| {
            Ok(this.state.lock().unwrap().on_packet_table.clone())
        });
        f.add_field_method_get("SampleRate", |_, this| {
            Ok(this.state.lock().unwrap().sample_rate as i64)
        });
        f.add_field_method_get("Channels", |_, this| {
            Ok(this.state.lock().unwrap().channels as i64)
        });
        f.add_field_method_get("QueueLength", |_, this| {
            Ok(this.state.lock().unwrap().pending_packets.len() as i64)
        });
    }
    fn add_methods<M: UserDataMethods<Self>>(m: &mut M) {
        m.add_method("Drain", |_, this, _: ()| -> mlua::Result<i64> {
            let mut s = this.state.lock().unwrap();
            let n = s.pending_packets.len() as i64;
            s.pending_packets.clear();
            s.accum.clear();
            Ok(n)
        });
    }
}

pub struct ByteSinkRoute {
    pub link_id: u64,
    pub sink: Arc<Mutex<ByteSinkState>>,
    pub chain: Box<dyn Source<Item = f32> + Send>,
    pub queue: Arc<Mutex<VecDeque<f32>>>,
    pub sample_rate: u32,
    pub channels: u16,
}

pub struct ByteSourceState {
    pub id: u64,
    pub enabled: bool,
    pub queue: Arc<Mutex<VecDeque<f32>>>,
    pub sample_rate: u32,
    pub channels: u16,
    pub active_sinks: Mutex<Vec<(u64, Sink)>>,
    pub byte_routes: Mutex<Vec<ByteSinkRoute>>,
    pub max_buffer_samples: usize,
}

unsafe impl Send for ByteSourceState {}
unsafe impl Sync for ByteSourceState {}

pub struct ByteSource {
    pub state: Arc<Mutex<ByteSourceState>>,
}

impl ByteSource {
    pub fn new(sample_rate: u32, channels: u16) -> Self {
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        let sr = sample_rate.max(8000);
        let ch = channels.max(1);
        let state = Arc::new(Mutex::new(ByteSourceState {
            id,
            enabled: true,
            queue: Arc::new(Mutex::new(VecDeque::new())),
            sample_rate: sr,
            channels: ch,
            active_sinks: Mutex::new(Vec::new()),
            byte_routes: Mutex::new(Vec::new()),
            max_buffer_samples: (sr as usize) * (ch as usize) * 5,
        }));
        BYTE_SOURCE_REGISTRY.with(|r| r.borrow_mut().push(state.clone()));
        Self { state }
    }
}

impl UserData for ByteSource {
    fn add_fields<F: UserDataFields<Self>>(f: &mut F) {
        f.add_field_method_get("Enabled", |_, this| Ok(this.state.lock().unwrap().enabled));
        f.add_field_method_set("Enabled", |_, this, v: bool| {
            this.state.lock().unwrap().enabled = v;
            Ok(())
        });
        f.add_field_method_get("SampleRate", |_, this| {
            Ok(this.state.lock().unwrap().sample_rate as i64)
        });
        f.add_field_method_get("Channels", |_, this| {
            Ok(this.state.lock().unwrap().channels as i64)
        });
        f.add_field_method_get("QueueLength", |_, this| {
            Ok(this.state.lock().unwrap().queue.lock().unwrap().len() as i64)
        });
        f.add_field_method_get("BufferSeconds", |_, this| {
            let s = this.state.lock().unwrap();
            let q = s.queue.lock().unwrap().len() as f64;
            let denom = (s.sample_rate as f64) * (s.channels as f64).max(1.0);
            Ok(if denom > 0.0 { q / denom } else { 0.0 })
        });
    }
    fn add_methods<M: UserDataMethods<Self>>(m: &mut M) {
        m.add_method("SendInput", |_, this, packet: mlua::String| -> mlua::Result<()> {
            let s = this.state.lock().unwrap();
            if !s.enabled {
                return Ok(());
            }
            let bytes = packet.as_bytes();
            if bytes.len() % 4 != 0 {
                return Err(mlua::Error::RuntimeError(
                    "ByteSource:SendInput: byte length must be a multiple of 4 (interleaved little-endian f32)".into(),
                ));
            }
            let queue = s.queue.clone();
            let max_buf = s.max_buffer_samples;
            drop(s);
            let mut q = queue.lock().unwrap();
            let mut i = 0;
            while i + 4 <= bytes.len() {
                let sample = f32::from_le_bytes([bytes[i], bytes[i + 1], bytes[i + 2], bytes[i + 3]]);
                q.push_back(sample);
                i += 4;
            }
            if q.len() > max_buf {
                let drop_n = q.len() - max_buf;
                q.drain(..drop_n);
            }
            Ok(())
        });
        m.add_method("SendSamples", |_, this, samples: Vec<f32>| -> mlua::Result<()> {
            let s = this.state.lock().unwrap();
            if !s.enabled {
                return Ok(());
            }
            let queue = s.queue.clone();
            let max_buf = s.max_buffer_samples;
            drop(s);
            let mut q = queue.lock().unwrap();
            q.extend(samples.into_iter());
            if q.len() > max_buf {
                let drop_n = q.len() - max_buf;
                q.drain(..drop_n);
            }
            Ok(())
        });
        m.add_method("Clear", |_, this, _: ()| -> mlua::Result<()> {
            let s = this.state.lock().unwrap();
            s.queue.lock().unwrap().clear();
            Ok(())
        });
        m.add_method("Stop", |_, this, _: ()| -> mlua::Result<()> {
            let s = this.state.lock().unwrap();
            let mut sinks = s.active_sinks.lock().unwrap();
            for (_, sink) in sinks.iter() {
                sink.stop();
            }
            sinks.clear();
            drop(sinks);
            s.byte_routes.lock().unwrap().clear();
            s.queue.lock().unwrap().clear();
            Ok(())
        });
        m.add_method("Destroy", |_, this, _: ()| -> mlua::Result<()> {
            let s = this.state.lock().unwrap();
            let mut sinks = s.active_sinks.lock().unwrap();
            for (_, sink) in sinks.iter() {
                sink.stop();
            }
            sinks.clear();
            drop(sinks);
            s.byte_routes.lock().unwrap().clear();
            s.queue.lock().unwrap().clear();
            let id = s.id;
            drop(s);
            BYTE_SOURCE_REGISTRY.with(|r| {
                r.borrow_mut().retain(|arc| arc.lock().map(|st| st.id != id).unwrap_or(true));
            });
            Ok(())
        });
    }
}

struct ByteSourceQueueSource {
    queue: Arc<Mutex<VecDeque<f32>>>,
    sample_rate: u32,
    channels: u16,
}

impl Iterator for ByteSourceQueueSource {
    type Item = f32;
    fn next(&mut self) -> Option<f32> {
        let mut q = self.queue.lock().unwrap();
        Some(q.pop_front().unwrap_or(0.0))
    }
}

impl Source for ByteSourceQueueSource {
    fn current_frame_len(&self) -> Option<usize> {
        None
    }
    fn channels(&self) -> u16 {
        self.channels.max(1)
    }
    fn sample_rate(&self) -> u32 {
        self.sample_rate.max(8000)
    }
    fn total_duration(&self) -> Option<Duration> {
        None
    }
}

#[cfg(feature = "voice")]
pub struct VoiceChannelState {
    pub id: u64,
    pub enabled: bool,
    pub threshold: Arc<Mutex<f32>>,
    pub queue: Arc<Mutex<VecDeque<f32>>>,
    pub sample_rate: u32,
    pub channels: u16,
    pub stream: Mutex<Option<cpal::Stream>>,
    pub active_sinks: Mutex<Vec<(u64, Sink)>>,
    pub byte_routes: Mutex<Vec<ByteSinkRoute>>,
    pub peak_level: Arc<Mutex<f32>>,
}

#[cfg(feature = "voice")]
unsafe impl Send for VoiceChannelState {}
#[cfg(feature = "voice")]
unsafe impl Sync for VoiceChannelState {}

#[cfg(feature = "voice")]
pub struct VoiceChannel {
    pub state: Arc<Mutex<VoiceChannelState>>,
}

#[cfg(feature = "voice")]
impl VoiceChannel {
    pub fn new() -> Self {
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        let state = Arc::new(Mutex::new(VoiceChannelState {
            id,
            enabled: true,
            threshold: Arc::new(Mutex::new(0.0)),
            queue: Arc::new(Mutex::new(VecDeque::new())),
            sample_rate: 0,
            channels: 0,
            stream: Mutex::new(None),
            active_sinks: Mutex::new(Vec::new()),
            byte_routes: Mutex::new(Vec::new()),
            peak_level: Arc::new(Mutex::new(0.0)),
        }));
        VOICE_REGISTRY.with(|r| r.borrow_mut().push(state.clone()));
        Self { state }
    }
}

#[cfg(feature = "voice")]
impl UserData for VoiceChannel {
    fn add_fields<F: UserDataFields<Self>>(f: &mut F) {
        f.add_field_method_get("Enabled", |_, this| Ok(this.state.lock().unwrap().enabled));
        f.add_field_method_set("Enabled", |_, this, v: bool| {
            this.state.lock().unwrap().enabled = v;
            Ok(())
        });
        f.add_field_method_get("Threshold", |_, this| {
            Ok(*this.state.lock().unwrap().threshold.lock().unwrap())
        });
        f.add_field_method_set("Threshold", |_, this, v: f32| {
            let s = this.state.lock().unwrap();
            *s.threshold.lock().unwrap() = v.max(0.0);
            Ok(())
        });
        f.add_field_method_get("SampleRate", |_, this| {
            Ok(this.state.lock().unwrap().sample_rate as i64)
        }) ;
        f.add_field_method_get("Channels", |_, this| {
            Ok(this.state.lock().unwrap().channels as i64)
        });
        f.add_field_method_get("IsCapturing", |_, this| {
            Ok(this.state.lock().unwrap().stream.lock().unwrap().is_some())
        });
        f.add_field_method_get("VolumeLevel", |_, this| {
            Ok(*this.state.lock().unwrap().peak_level.lock().unwrap())
        });
    }
    fn add_methods<M: UserDataMethods<Self>>(m: &mut M) {
        m.add_method("Stop", |_, this, _: ()| -> mlua::Result<()> {
            let s = this.state.lock().unwrap();
            let mut sinks = s.active_sinks.lock().unwrap();
            for (_, sink) in sinks.iter() {
                sink.stop();
            }
            sinks.clear();
            drop(sinks);
            s.byte_routes.lock().unwrap().clear();
            let _ = s.stream.lock().unwrap().take();
            Ok(())
        });
        m.add_method("Destroy", |_, this, _: ()| -> mlua::Result<()> {
            let s = this.state.lock().unwrap();
            let mut sinks = s.active_sinks.lock().unwrap();
            for (_, sink) in sinks.iter() {
                sink.stop();
            }
            sinks.clear();
            drop(sinks);
            s.byte_routes.lock().unwrap().clear();
            let _ = s.stream.lock().unwrap().take();
            let id = s.id;
            drop(s);
            VOICE_REGISTRY.with(|r| {
                r.borrow_mut().retain(|arc| {
                    arc.lock().map(|st| st.id != id).unwrap_or(true)
                });
            });
            Ok(())
        });
    }
}

#[cfg(feature = "voice")]
fn voice_ensure_capture(state: &Arc<Mutex<VoiceChannelState>>) -> mlua::Result<()> {
    use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

    {
        let s = state.lock().unwrap();
        if s.stream.lock().unwrap().is_some() {
            return Ok(());
        }
    }
    let host = cpal::default_host();
    let device = host.default_input_device().ok_or_else(|| {
        mlua::Error::RuntimeError("VoiceChannel: no default input device".into())
    })?;
    let supported = device.default_input_config().map_err(|e| {
        mlua::Error::RuntimeError(format!("VoiceChannel input config: {e}"))
    })?;
    let sample_rate = supported.sample_rate().0;
    let channels = supported.channels();
    let cfg = cpal::StreamConfig {
        channels,
        sample_rate: cpal::SampleRate(sample_rate),
        buffer_size: cpal::BufferSize::Default,
    };

    let (queue, threshold_arc, peak_arc) = {
        let s = state.lock().unwrap();
        (s.queue.clone(), s.threshold.clone(), s.peak_level.clone())
    };

    let stream = device
        .build_input_stream(
            &cfg,
            move |data: &[f32], _: &cpal::InputCallbackInfo| {
                let peak = data.iter().fold(0.0_f32, |a, &x| a.max(x.abs()));
                {
                    let mut p = peak_arc.lock().unwrap();
                    *p = *p * 0.6 + peak * 0.4;
                }
                let thresh = *threshold_arc.lock().unwrap();
                if thresh > 0.0 && peak < thresh {
                    return;
                }
                let mut q = queue.lock().unwrap();
                q.extend(data.iter().copied());
                if q.len() > sample_rate as usize * 2 {
                    let drop = q.len() - sample_rate as usize;
                    q.drain(..drop);
                }
            },
            |err| eprintln!("[SoundByte voice] capture err: {err}"),
            None,
        )
        .map_err(|e| mlua::Error::RuntimeError(format!("VoiceChannel build_stream: {e}")))?;
    stream
        .play()
        .map_err(|e| mlua::Error::RuntimeError(format!("VoiceChannel start: {e}")))?;

    let mut s = state.lock().unwrap();
    s.sample_rate = sample_rate;
    s.channels = channels;
    *s.stream.lock().unwrap() = Some(stream);
    Ok(())
}

#[cfg(feature = "voice")]
struct VoiceQueueSource {
    queue: Arc<Mutex<VecDeque<f32>>>,
    sample_rate: u32,
    channels: u16,
}

#[cfg(feature = "voice")]
impl Iterator for VoiceQueueSource {
    type Item = f32;
    fn next(&mut self) -> Option<f32> {
        let mut q = self.queue.lock().unwrap();
        Some(q.pop_front().unwrap_or(0.0))
    }
}

#[cfg(feature = "voice")]
impl Source for VoiceQueueSource {
    fn current_frame_len(&self) -> Option<usize> {
        None
    }
    fn channels(&self) -> u16 {
        self.channels.max(1)
    }
    fn sample_rate(&self) -> u32 {
        self.sample_rate.max(8000)
    }
    fn total_duration(&self) -> Option<Duration> {
        None
    }
}

pub struct LinkHandle {
    pub id: u64,
    pub source_player_id: Option<u64>,
    pub source_voice_id: Option<u64>,
    pub source_byte_source_id: Option<u64>,
    pub sink_output_id: Option<u64>,
    pub sink_byte_id: Option<u64>,
    pub modifier_ids: Vec<u64>,
}

impl UserData for LinkHandle {
    fn add_methods<M: UserDataMethods<Self>>(m: &mut M) {
        m.add_method("Unlink", |_, this, _: ()| -> mlua::Result<()> {
            let link_id = this.id;
            if let Some(player_id) = this.source_player_id {
                PLAYER_REGISTRY.with(|r| {
                    for arc in r.borrow().iter() {
                        let mut s = arc.lock().unwrap();
                        if s.id != player_id {
                            continue;
                        }
                        s.routes.retain(|r| r.link_id != link_id);
                    }
                });
            }
            #[cfg(feature = "voice")]
            if let Some(_voice_id) = this.source_voice_id {
                VOICE_REGISTRY.with(|r| {
                    for arc in r.borrow().iter() {
                        let s = arc.lock().unwrap();
                        let mut sinks = s.active_sinks.lock().unwrap();
                        sinks.retain(|(id, sink)| {
                            if *id == link_id {
                                sink.stop();
                                false
                            } else {
                                true
                            }
                        });
                        let sinks_empty = sinks.is_empty();
                        drop(sinks);
                        let mut routes = s.byte_routes.lock().unwrap();
                        routes.retain(|r| r.link_id != link_id);
                        let routes_empty = routes.is_empty();
                        drop(routes);
                        if sinks_empty && routes_empty {
                            let _ = s.stream.lock().unwrap().take();
                        }
                    }
                });
            }
            if let Some(_bs_id) = this.source_byte_source_id {
                BYTE_SOURCE_REGISTRY.with(|r| {
                    for arc in r.borrow().iter() {
                        let s = arc.lock().unwrap();
                        let mut sinks = s.active_sinks.lock().unwrap();
                        sinks.retain(|(id, sink)| {
                            if *id == link_id {
                                sink.stop();
                                false
                            } else {
                                true
                            }
                        });
                        drop(sinks);
                        s.byte_routes.lock().unwrap().retain(|r| r.link_id != link_id);
                    }
                });
            }
            Ok(())
        });
    }
}

fn link(_lua: &Lua, args: MultiValue) -> mlua::Result<LinkHandle> {
    let items: Vec<Value> = args.into_iter().collect();
    if items.len() < 2 {
        return Err(mlua::Error::RuntimeError(
            "SoundByte.Link: expected at least a source and a sink (source, ..modifiers, sink)"
                .into(),
        ));
    }

    let mut source_player: Option<Arc<Mutex<PlayerState>>> = None;
    #[cfg(feature = "voice")]
    let mut source_voice: Option<Arc<Mutex<VoiceChannelState>>> = None;
    let mut source_byte_source: Option<Arc<Mutex<ByteSourceState>>> = None;
    let mut sink_output: Option<Arc<OutputState>> = None;
    let mut sink_byte: Option<Arc<Mutex<ByteSinkState>>> = None;
    let mut modifier_states: Vec<Arc<Mutex<ModifierState>>> = Vec::new();

    for (i, item) in items.iter().enumerate() {
        let ud = match item {
            Value::UserData(ud) => ud,
            _ => {
                return Err(mlua::Error::RuntimeError(format!(
                    "SoundByte.Link: argument {} must be a SoundByte userdata",
                    i + 1
                )));
            }
        };
        if let Ok(p) = ud.borrow::<Player>() {
            if source_player.is_some() || source_byte_source.is_some() {
                return Err(mlua::Error::RuntimeError(
                    "SoundByte.Link: only one source allowed per link (Player)".into(),
                ));
            }
            #[cfg(feature = "voice")]
            if source_voice.is_some() {
                return Err(mlua::Error::RuntimeError(
                    "SoundByte.Link: only one source allowed per link (Player)".into(),
                ));
            }
            source_player = Some(p.state.clone());
            continue;
        }
        #[cfg(feature = "voice")]
        if let Ok(v) = ud.borrow::<VoiceChannel>() {
            if source_player.is_some() || source_voice.is_some() || source_byte_source.is_some() {
                return Err(mlua::Error::RuntimeError(
                    "SoundByte.Link: only one source allowed per link".into(),
                ));
            }
            source_voice = Some(v.state.clone());
            continue;
        }
        if let Ok(b) = ud.borrow::<ByteSource>() {
            if source_player.is_some() || source_byte_source.is_some() {
                return Err(mlua::Error::RuntimeError(
                    "SoundByte.Link: only one source allowed per link".into(),
                ));
            }
            #[cfg(feature = "voice")]
            if source_voice.is_some() {
                return Err(mlua::Error::RuntimeError(
                    "SoundByte.Link: only one source allowed per link".into(),
                ));
            }
            source_byte_source = Some(b.state.clone());
            continue;
        }
        if let Ok(o) = ud.borrow::<OutputNode>() {
            if sink_output.is_some() || sink_byte.is_some() {
                return Err(mlua::Error::RuntimeError(
                    "SoundByte.Link: only one sink allowed per link".into(),
                ));
            }
            sink_output = Some(o.state.clone());
            continue;
        }
        if let Ok(b) = ud.borrow::<ByteSink>() {
            if sink_output.is_some() || sink_byte.is_some() {
                return Err(mlua::Error::RuntimeError(
                    "SoundByte.Link: only one sink allowed per link".into(),
                ));
            }
            sink_byte = Some(b.state.clone());
            continue;
        }
        if let Ok(m) = ud.borrow::<Modifier>() {
            modifier_states.push(m.state.clone());
            continue;
        }
        return Err(mlua::Error::RuntimeError(format!(
            "SoundByte.Link: argument {} is not a recognized SoundByte type",
            i + 1
        )));
    }

    if sink_output.is_none() && sink_byte.is_none() {
        return Err(mlua::Error::RuntimeError(
            "SoundByte.Link: missing sink (an OutputNode or ByteSink must be one of the arguments)"
                .into(),
        ));
    }

    let link_id = NEXT_ID.fetch_add(1, Ordering::Relaxed);

    if let Some(player) = source_player {
        let mut sp = player.lock().unwrap();
        sp.routes.push(Route {
            link_id,
            output: sink_output.clone(),
            byte: sink_byte.clone(),
            modifiers: modifier_states.clone(),
        });
        let player_id = sp.id;
        drop(sp);

        return Ok(LinkHandle {
            id: link_id,
            source_player_id: Some(player_id),
            source_voice_id: None,
            source_byte_source_id: None,
            sink_output_id: sink_output.as_ref().map(|o| o.id),
            sink_byte_id: sink_byte.as_ref().map(|b| b.lock().unwrap().id),
            modifier_ids: modifier_states.iter().map(|m| m.lock().unwrap().id).collect(),
        });
    }

    #[cfg(feature = "voice")]
    if let Some(voice) = source_voice {
        voice_ensure_capture(&voice)?;
        let (queue, sample_rate, channels) = {
            let s = voice.lock().unwrap();
            (s.queue.clone(), s.sample_rate.max(48000), s.channels.max(1))
        };
        let mut source: Box<dyn Source<Item = f32> + Send> = Box::new(VoiceQueueSource {
            queue: queue.clone(),
            sample_rate,
            channels,
        });
        source = apply_modifier_chain(source, &modifier_states);

        if let Some(out) = sink_output.as_ref() {
            source = Box::new(SpatialAdapter {
                inner: source,
                state: out.spatial.clone(),
                chan_idx: 0,
                last_l: 0.0,
                last_r: 0.0,
            });
            let h = output_handle()?;
            let sink = Sink::try_new(&h)
                .map_err(|e| mlua::Error::RuntimeError(format!("SoundByte voice sink: {e}")))?;
            sink.set_volume(out.spatial.lock().unwrap().volume);
            sink.append(source);
            sink.play();
            voice.lock().unwrap().active_sinks.lock().unwrap().push((link_id, sink));
        } else if let Some(byte_sink) = sink_byte.as_ref() {
            voice.lock().unwrap().byte_routes.lock().unwrap().push(ByteSinkRoute {
                link_id,
                sink: byte_sink.clone(),
                chain: source,
                queue,
                sample_rate,
                channels,
            });
        }

        let voice_id = voice.lock().unwrap().id;
        return Ok(LinkHandle {
            id: link_id,
            source_player_id: None,
            source_voice_id: Some(voice_id),
            source_byte_source_id: None,
            sink_output_id: sink_output.as_ref().map(|o| o.id),
            sink_byte_id: sink_byte.as_ref().map(|b| b.lock().unwrap().id),
            modifier_ids: modifier_states.iter().map(|m| m.lock().unwrap().id).collect(),
        });
    }

    if let Some(byte_src) = source_byte_source {
        let (queue, sample_rate, channels) = {
            let s = byte_src.lock().unwrap();
            (s.queue.clone(), s.sample_rate, s.channels)
        };
        let mut source: Box<dyn Source<Item = f32> + Send> = Box::new(ByteSourceQueueSource {
            queue: queue.clone(),
            sample_rate,
            channels,
        });
        source = apply_modifier_chain(source, &modifier_states);

        if let Some(out) = sink_output.as_ref() {
            source = Box::new(SpatialAdapter {
                inner: source,
                state: out.spatial.clone(),
                chan_idx: 0,
                last_l: 0.0,
                last_r: 0.0,
            });
            let h = output_handle()?;
            let sink = Sink::try_new(&h).map_err(|e| {
                mlua::Error::RuntimeError(format!("SoundByte ByteSource sink: {e}"))
            })?;
            sink.set_volume(out.spatial.lock().unwrap().volume);
            sink.append(source);
            sink.play();
            byte_src
                .lock()
                .unwrap()
                .active_sinks
                .lock()
                .unwrap()
                .push((link_id, sink));
        } else if let Some(byte_sink) = sink_byte.as_ref() {
            byte_src.lock().unwrap().byte_routes.lock().unwrap().push(ByteSinkRoute {
                link_id,
                sink: byte_sink.clone(),
                chain: source,
                queue,
                sample_rate,
                channels,
            });
        }

        let bs_id = byte_src.lock().unwrap().id;
        return Ok(LinkHandle {
            id: link_id,
            source_player_id: None,
            source_voice_id: None,
            source_byte_source_id: Some(bs_id),
            sink_output_id: sink_output.as_ref().map(|o| o.id),
            sink_byte_id: sink_byte.as_ref().map(|b| b.lock().unwrap().id),
            modifier_ids: modifier_states.iter().map(|m| m.lock().unwrap().id).collect(),
        });
    }

    Err(mlua::Error::RuntimeError(
        "SoundByte.Link: missing source (a Player, VoiceChannel, or ByteSource must be one of the arguments)".into(),
    ))
}

fn drain_route(route: &mut ByteSinkRoute) {
    let available = route.queue.lock().unwrap().len();
    if available == 0 {
        return;
    }
    let mut samples: Vec<f32> = Vec::with_capacity(available);
    for _ in 0..available {
        match route.chain.next() {
            Some(s) => samples.push(s),
            None => break,
        }
    }
    if samples.is_empty() {
        return;
    }
    route
        .sink
        .lock()
        .unwrap()
        .queue_pcm(samples, route.sample_rate, route.channels);
}

fn drain_byte_routes() {
    BYTE_SOURCE_REGISTRY.with(|r| {
        for arc in r.borrow().iter() {
            let s = arc.lock().unwrap();
            let mut routes = s.byte_routes.lock().unwrap();
            for route in routes.iter_mut() {
                drain_route(route);
            }
        }
    });
    #[cfg(feature = "voice")]
    VOICE_REGISTRY.with(|r| {
        for arc in r.borrow().iter() {
            let s = arc.lock().unwrap();
            let mut routes = s.byte_routes.lock().unwrap();
            for route in routes.iter_mut() {
                drain_route(route);
            }
        }
    });
}

pub fn pump(lua: &Lua) {
    let players: Vec<Arc<Mutex<PlayerState>>> = PLAYER_REGISTRY
        .with(|r| r.borrow().iter().cloned().collect());

    for p in &players {
        let outs: Vec<Arc<OutputState>> = p
            .lock()
            .unwrap()
            .routes
            .iter()
            .filter_map(|r| r.output.clone())
            .collect();
        for out in &outs {
            let follow = out.follow_target.lock().unwrap().clone();
            if let Some(part_arc) = follow {
                if let Ok(part) = part_arc.lock() {
                    if part.alive {
                        let pos = part.current_cframe().position;
                        let mut spat = out.spatial.lock().unwrap();
                        spat.use_3d = true;
                        spat.position = pos;
                    }
                }
            }
        }
    }

    for p in &players {
        let (mut started_fired, started_key, stopped_key, did_loop_key, last_loop, cur_loop, playing) = {
            let s = p.lock().unwrap();
            if !s.playing.load(Ordering::Relaxed) {
                continue;
            }
            (
                s.started_fired_for_active,
                s.started_key.clone(),
                s.stopped_key.clone(),
                s.did_loop_key.clone(),
                s.last_loop_count,
                s.loop_count.load(Ordering::Relaxed),
                s.playing.clone(),
            )
        };
        if !started_fired {
            if let Ok(sig) = lua.registry_value::<Table>(&started_key) {
                let _ = signal::fire(lua, &sig, MultiValue::new());
            }
            started_fired = true;
        }
        let mut last = last_loop;
        while last < cur_loop {
            if let Ok(sig) = lua.registry_value::<Table>(&did_loop_key) {
                let _ = signal::fire(lua, &sig, MultiValue::new());
            }
            last += 1;
        }

        let any_active = {
            let s = p.lock().unwrap();
            s.routes
                .iter()
                .filter_map(|r| r.output.as_ref())
                .any(|o| {
                    o.sink
                        .lock()
                        .unwrap()
                        .as_ref()
                        .map(|sk| !sk.empty())
                        .unwrap_or(false)
                })
        };
        if !any_active {
            playing.store(false, Ordering::Relaxed);
            if let Ok(sig) = lua.registry_value::<Table>(&stopped_key) {
                let _ = signal::fire(lua, &sig, MultiValue::new());
            }
            let mut s = p.lock().unwrap();
            s.started_fired_for_active = false;
            s.last_loop_count = cur_loop;
            continue;
        }
        let mut s = p.lock().unwrap();
        s.started_fired_for_active = started_fired;
        s.last_loop_count = last;

        let elapsed = s
            .play_started_at
            .map(|t| t.elapsed().as_secs_f64())
            .unwrap_or(0.0);
        let update_fires: Vec<(Arc<RegistryKey>, f64)> = {
            let mut fires: Vec<(Arc<RegistryKey>, f64)> = Vec::new();
            for u in s.update_links.iter_mut() {
                while elapsed >= u.next_fire {
                    fires.push((u.signal_key.clone(), u.next_fire));
                    u.next_fire += u.interval;
                }
            }
            fires
        };
        drop(s);
        for (key, t) in update_fires {
            if let Ok(sig) = lua.registry_value::<Table>(&key) {
                let mut args = MultiValue::new();
                args.push_back(Value::Number(t));
                let _ = signal::fire(lua, &sig, args);
            }
        }
    }

    drain_byte_routes();

    let bytes: Vec<Arc<Mutex<ByteSinkState>>> = BYTE_REGISTRY
        .with(|r| r.borrow().iter().cloned().collect());
    for b in &bytes {
        let (packets, sig_key) = {
            let mut s = b.lock().unwrap();
            let mut packets: Vec<Vec<u8>> = Vec::new();
            while let Some(p) = s.pending_packets.pop_front() {
                packets.push(p);
            }
            (packets, s.on_packet_key.clone())
        };
        if packets.is_empty() {
            continue;
        }
        if let Ok(sig) = lua.registry_value::<Table>(&sig_key) {
            for p in packets {
                if let Ok(s) = lua.create_string(&p) {
                    let mut args = MultiValue::new();
                    args.push_back(Value::String(s));
                    let _ = signal::fire(lua, &sig, args);
                }
            }
        }
    }
}

pub fn is_active() -> bool {
    let players_active = PLAYER_REGISTRY.with(|r| {
        r.borrow().iter().any(|p| {
            p.lock().unwrap().playing.load(Ordering::Relaxed)
        })
    });
    if players_active {
        return true;
    }
    let bs_active = BYTE_SOURCE_REGISTRY.with(|r| {
        r.borrow().iter().any(|bs| {
            let s = bs.lock().unwrap();
            !s.active_sinks.lock().unwrap().is_empty()
                || !s.byte_routes.lock().unwrap().is_empty()
        })
    });
    if bs_active {
        return true;
    }
    #[cfg(feature = "voice")]
    {
        let voice_active = VOICE_REGISTRY.with(|r| {
            r.borrow().iter().any(|v| {
                v.lock().unwrap().stream.lock().unwrap().is_some()
            })
        });
        if voice_active {
            return true;
        }
    }
    false
}

pub fn shutdown() {
    PLAYER_REGISTRY.with(|r| {
        for arc in r.borrow().iter() {
            let s = arc.lock().unwrap();
            for route in &s.routes {
                if let Some(out) = &route.output {
                    if let Some(sink) = out.sink.lock().unwrap().as_ref() {
                        sink.stop();
                    }
                }
            }
        }
    });
    BYTE_SOURCE_REGISTRY.with(|r| {
        for arc in r.borrow().iter() {
            let s = arc.lock().unwrap();
            let mut sinks = s.active_sinks.lock().unwrap();
            for (_, sink) in sinks.iter() {
                sink.stop();
            }
            sinks.clear();
            drop(sinks);
            s.byte_routes.lock().unwrap().clear();
            s.queue.lock().unwrap().clear();
        }
    });
    #[cfg(feature = "voice")]
    VOICE_REGISTRY.with(|r| {
        for arc in r.borrow().iter() {
            let s = arc.lock().unwrap();
            let mut sinks = s.active_sinks.lock().unwrap();
            for (_, sink) in sinks.iter() {
                sink.stop();
            }
            sinks.clear();
            drop(sinks);
            s.byte_routes.lock().unwrap().clear();
            let _ = s.stream.lock().unwrap().take();
        }
    });
}

fn apply_modifier_chain(
    mut src: Box<dyn Source<Item = f32> + Send>,
    modifiers: &[Arc<Mutex<ModifierState>>],
) -> Box<dyn Source<Item = f32> + Send> {
    for m in modifiers {
        let kind = m.lock().unwrap().kind;
        match kind {
            ModifierKind::Volume => {
                src = Box::new(LiveVolume { inner: src, state: m.clone() });
            }
            ModifierKind::Pan => {
                src = Box::new(LivePan { inner: src, state: m.clone(), chan_idx: 0 });
            }
            ModifierKind::Distortion => {
                src = Box::new(LiveDistortion { inner: src, state: m.clone() });
            }
            ModifierKind::Tremolo => {
                src = Box::new(LiveTremolo {
                    inner: src,
                    state: m.clone(),
                    sample_idx: 0,
                });
            }
            ModifierKind::Echo => {
                let s = m.lock().unwrap();
                let delay_ms = s.value.max(1.0) as u32;
                drop(s);
                src = Box::new(EchoMod::new(src, delay_ms, 0.4, 0.4));
            }
            ModifierKind::Reverb => {
                let v = m.lock().unwrap().value;
                src = Box::new(ReverbMod::new(src, v.clamp(0.0, 1.0), 0.7));
            }
            ModifierKind::FadeIn => {
                let v = m.lock().unwrap().value;
                let dur = Duration::from_secs_f32(v.max(0.0));
                src = Box::new(src.fade_in(dur));
            }
            ModifierKind::FadeOut => {
                let v = m.lock().unwrap().value;
                src = Box::new(FadeOutMod::new(src, v.max(0.0) as f64));
            }
            ModifierKind::Delay => {
                let v = m.lock().unwrap().value;
                let dur = Duration::from_secs_f32(v.max(0.0));
                src = Box::new(src.delay(dur));
            }
            ModifierKind::Speed | ModifierKind::Pitch | ModifierKind::PlaybackSpeed => {
                let v = m.lock().unwrap().value;
                src = Box::new(src.speed(v.max(0.0)));
            }
            ModifierKind::LowPass => {
                let v = m.lock().unwrap().value;
                src = Box::new(src.low_pass(v.max(1.0) as u32));
            }
            ModifierKind::HighPass => {
                let v = m.lock().unwrap().value;
                src = Box::new(src.high_pass(v.max(1.0) as u32));
            }
            ModifierKind::BandPass => {
                let v = m.lock().unwrap().value;
                let center = v.max(20.0) as u32;
                let high = (center as f32 / 2.0).max(20.0) as u32;
                let low = (center as f32 * 2.0) as u32;
                src = Box::new(src.high_pass(high).low_pass(low));
            }
            ModifierKind::Vibrato => {
                src = Box::new(VibratoMod::new(src, m.clone()));
            }
            ModifierKind::BitCrusher => {
                src = Box::new(BitCrusherMod { inner: src, state: m.clone() });
            }
            ModifierKind::NoiseGate => {
                src = Box::new(NoiseGateMod { inner: src, state: m.clone() });
            }
            ModifierKind::Compressor => {
                src = Box::new(CompressorMod {
                    inner: src,
                    state: m.clone(),
                    env: 0.0,
                });
            }
            ModifierKind::Limiter => {
                src = Box::new(LimiterMod { inner: src, state: m.clone() });
            }
            ModifierKind::Saturation => {
                src = Box::new(SaturationMod { inner: src, state: m.clone() });
            }
            ModifierKind::RingMod => {
                src = Box::new(RingModMod {
                    inner: src,
                    state: m.clone(),
                    sample_idx: 0,
                });
            }
            ModifierKind::Chorus => {
                src = Box::new(ChorusMod::new(src, m.clone()));
            }
            ModifierKind::Flanger => {
                src = Box::new(FlangerMod::new(src, m.clone()));
            }
            ModifierKind::StereoWiden => {
                src = Box::new(StereoWidenMod {
                    inner: src,
                    state: m.clone(),
                    chan_idx: 0,
                    last_l: 0.0,
                });
            }
            ModifierKind::Wobble => {
                src = Box::new(WobbleMod {
                    inner: src,
                    state: m.clone(),
                    sample_idx: 0,
                });
            }
            ModifierKind::Telephone => {
                src = Box::new(src.high_pass(300).low_pass(3400));
                src = Box::new(TelephoneMod { inner: src, state: m.clone() });
            }
            ModifierKind::Underwater => {
                src = Box::new(src.low_pass(600));
                src = Box::new(UnderwaterMod {
                    inner: src,
                    state: m.clone(),
                    sample_idx: 0,
                });
            }
            ModifierKind::Mute => {
                src = Box::new(MuteMod { inner: src, state: m.clone() });
            }
        }
    }
    src
}

struct LiveVolume {
    inner: Box<dyn Source<Item = f32> + Send>,
    state: Arc<Mutex<ModifierState>>,
}
impl Iterator for LiveVolume {
    type Item = f32;
    fn next(&mut self) -> Option<f32> {
        let s = self.inner.next()?;
        let g = {
            let st = self.state.lock().unwrap();
            if st.enabled { st.value.max(0.0) } else { 1.0 }
        };
        Some(s * g)
    }
}
impl Source for LiveVolume {
    fn current_frame_len(&self) -> Option<usize> { self.inner.current_frame_len() }
    fn channels(&self) -> u16 { self.inner.channels() }
    fn sample_rate(&self) -> u32 { self.inner.sample_rate() }
    fn total_duration(&self) -> Option<Duration> { self.inner.total_duration() }
}

struct LivePan {
    inner: Box<dyn Source<Item = f32> + Send>,
    state: Arc<Mutex<ModifierState>>,
    chan_idx: u16,
}
impl Iterator for LivePan {
    type Item = f32;
    fn next(&mut self) -> Option<f32> {
        let s = self.inner.next()?;
        let channels = self.inner.channels().max(1);
        let pan = {
            let st = self.state.lock().unwrap();
            if st.enabled { st.value.clamp(-1.0, 1.0) } else { 0.0 }
        };
        let theta = (pan + 1.0) * std::f32::consts::FRAC_PI_4;
        let gain = match (channels, self.chan_idx) {
            (1, _) => 1.0,
            (_, 0) => theta.cos(),
            (_, 1) => theta.sin(),
            _ => 1.0,
        };
        self.chan_idx = (self.chan_idx + 1) % channels;
        Some(s * gain)
    }
}
impl Source for LivePan {
    fn current_frame_len(&self) -> Option<usize> { self.inner.current_frame_len() }
    fn channels(&self) -> u16 { self.inner.channels() }
    fn sample_rate(&self) -> u32 { self.inner.sample_rate() }
    fn total_duration(&self) -> Option<Duration> { self.inner.total_duration() }
}

struct LiveDistortion {
    inner: Box<dyn Source<Item = f32> + Send>,
    state: Arc<Mutex<ModifierState>>,
}
impl Iterator for LiveDistortion {
    type Item = f32;
    fn next(&mut self) -> Option<f32> {
        let s = self.inner.next()?;
        let amount = {
            let st = self.state.lock().unwrap();
            if st.enabled { st.value.max(0.0) } else { 0.0 }
        };
        if amount < 1e-4 { return Some(s); }
        let drive = (1.0 + amount * 9.0).max(1.0);
        let norm = drive.tanh().max(1e-3);
        Some((s * drive).tanh() / norm)
    }
}
impl Source for LiveDistortion {
    fn current_frame_len(&self) -> Option<usize> { self.inner.current_frame_len() }
    fn channels(&self) -> u16 { self.inner.channels() }
    fn sample_rate(&self) -> u32 { self.inner.sample_rate() }
    fn total_duration(&self) -> Option<Duration> { self.inner.total_duration() }
}

struct LiveTremolo {
    inner: Box<dyn Source<Item = f32> + Send>,
    state: Arc<Mutex<ModifierState>>,
    sample_idx: u64,
}
impl Iterator for LiveTremolo {
    type Item = f32;
    fn next(&mut self) -> Option<f32> {
        let s = self.inner.next()?;
        let channels = self.inner.channels().max(1) as u64;
        let sr = self.inner.sample_rate().max(1) as f32;
        let (rate, depth, enabled) = {
            let st = self.state.lock().unwrap();
            (st.value.max(0.0), 0.5_f32, st.enabled)
        };
        if !enabled {
            self.sample_idx = self.sample_idx.wrapping_add(1);
            return Some(s);
        }
        let frame = self.sample_idx / channels;
        let t = frame as f32 / sr;
        let lfo = (2.0 * std::f32::consts::PI * rate * t).sin();
        let gain = (1.0 - depth) + depth * (lfo * 0.5 + 0.5);
        self.sample_idx = self.sample_idx.wrapping_add(1);
        Some(s * gain)
    }
}
impl Source for LiveTremolo {
    fn current_frame_len(&self) -> Option<usize> { self.inner.current_frame_len() }
    fn channels(&self) -> u16 { self.inner.channels() }
    fn sample_rate(&self) -> u32 { self.inner.sample_rate() }
    fn total_duration(&self) -> Option<Duration> { self.inner.total_duration() }
}

struct EchoMod {
    inner: Box<dyn Source<Item = f32> + Send>,
    buf: Vec<f32>,
    head: usize,
    feedback: f32,
    mix: f32,
}
impl EchoMod {
    fn new(inner: Box<dyn Source<Item = f32> + Send>, delay_ms: u32, feedback: f32, mix: f32) -> Self {
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
impl Iterator for EchoMod {
    type Item = f32;
    fn next(&mut self) -> Option<f32> {
        let dry = self.inner.next()?;
        let delayed = self.buf[self.head];
        self.buf[self.head] = dry + delayed * self.feedback;
        self.head = (self.head + 1) % self.buf.len();
        Some(dry * (1.0 - self.mix) + delayed * self.mix)
    }
}
impl Source for EchoMod {
    fn current_frame_len(&self) -> Option<usize> { self.inner.current_frame_len() }
    fn channels(&self) -> u16 { self.inner.channels() }
    fn sample_rate(&self) -> u32 { self.inner.sample_rate() }
    fn total_duration(&self) -> Option<Duration> { self.inner.total_duration() }
}

struct ReverbMod {
    inner: Box<dyn Source<Item = f32> + Send>,
    combs: [ReverbComb; 4],
    aps: [ReverbAp; 2],
    mix: f32,
}
struct ReverbComb { buf: Vec<f32>, head: usize, feedback: f32 }
struct ReverbAp { buf: Vec<f32>, head: usize }
impl ReverbComb {
    fn new(n: usize, fb: f32) -> Self { Self { buf: vec![0.0; n.max(1)], head: 0, feedback: fb } }
    fn process(&mut self, x: f32) -> f32 {
        let y = self.buf[self.head];
        self.buf[self.head] = x + y * self.feedback;
        self.head = (self.head + 1) % self.buf.len();
        y
    }
}
impl ReverbAp {
    fn new(n: usize) -> Self { Self { buf: vec![0.0; n.max(1)], head: 0 } }
    fn process(&mut self, x: f32) -> f32 {
        let g = 0.5_f32;
        let b = self.buf[self.head];
        let y = -x + b;
        self.buf[self.head] = x + b * g;
        self.head = (self.head + 1) % self.buf.len();
        y
    }
}
impl ReverbMod {
    fn new(inner: Box<dyn Source<Item = f32> + Send>, mix: f32, decay: f32) -> Self {
        let sr = inner.sample_rate() as f32;
        let ch = inner.channels().max(1) as usize;
        let fb = decay.clamp(0.05, 0.95);
        let cl = [29, 37, 41, 43];
        let al = [5, 7];
        Self {
            combs: [
                ReverbComb::new((sr * cl[0] as f32 / 1000.0) as usize * ch, fb),
                ReverbComb::new((sr * cl[1] as f32 / 1000.0) as usize * ch, fb),
                ReverbComb::new((sr * cl[2] as f32 / 1000.0) as usize * ch, fb),
                ReverbComb::new((sr * cl[3] as f32 / 1000.0) as usize * ch, fb),
            ],
            aps: [
                ReverbAp::new((sr * al[0] as f32 / 1000.0) as usize * ch),
                ReverbAp::new((sr * al[1] as f32 / 1000.0) as usize * ch),
            ],
            mix: mix.clamp(0.0, 1.0),
            inner,
        }
    }
}
impl Iterator for ReverbMod {
    type Item = f32;
    fn next(&mut self) -> Option<f32> {
        let dry = self.inner.next()?;
        let mut wet = 0.0_f32;
        for c in &mut self.combs { wet += c.process(dry); }
        wet *= 0.25;
        for a in &mut self.aps { wet = a.process(wet); }
        Some(dry * (1.0 - self.mix) + wet * self.mix)
    }
}
impl Source for ReverbMod {
    fn current_frame_len(&self) -> Option<usize> { self.inner.current_frame_len() }
    fn channels(&self) -> u16 { self.inner.channels() }
    fn sample_rate(&self) -> u32 { self.inner.sample_rate() }
    fn total_duration(&self) -> Option<Duration> { self.inner.total_duration() }
}

struct FadeOutMod {
    inner: Box<dyn Source<Item = f32> + Send>,
    sample_idx: u64,
    fade_started_at: Option<u64>,
    total_samples: Option<u64>,
}
impl FadeOutMod {
    fn new(inner: Box<dyn Source<Item = f32> + Send>, duration_secs: f64) -> Self {
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
impl Iterator for FadeOutMod {
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
impl Source for FadeOutMod {
    fn current_frame_len(&self) -> Option<usize> { self.inner.current_frame_len() }
    fn channels(&self) -> u16 { self.inner.channels() }
    fn sample_rate(&self) -> u32 { self.inner.sample_rate() }
    fn total_duration(&self) -> Option<Duration> { self.inner.total_duration() }
}

struct BitCrusherMod {
    inner: Box<dyn Source<Item = f32> + Send>,
    state: Arc<Mutex<ModifierState>>,
}
impl Iterator for BitCrusherMod {
    type Item = f32;
    fn next(&mut self) -> Option<f32> {
        let s = self.inner.next()?;
        let (bits, enabled) = {
            let st = self.state.lock().unwrap();
            (st.value.clamp(1.0, 16.0), st.enabled)
        };
        if !enabled { return Some(s); }
        let levels = (1u32 << bits as u32) as f32;
        Some((s * levels).round() / levels)
    }
}
impl Source for BitCrusherMod {
    fn current_frame_len(&self) -> Option<usize> { self.inner.current_frame_len() }
    fn channels(&self) -> u16 { self.inner.channels() }
    fn sample_rate(&self) -> u32 { self.inner.sample_rate() }
    fn total_duration(&self) -> Option<Duration> { self.inner.total_duration() }
}

struct NoiseGateMod {
    inner: Box<dyn Source<Item = f32> + Send>,
    state: Arc<Mutex<ModifierState>>,
}
impl Iterator for NoiseGateMod {
    type Item = f32;
    fn next(&mut self) -> Option<f32> {
        let s = self.inner.next()?;
        let (thresh, enabled) = {
            let st = self.state.lock().unwrap();
            (st.value.max(0.0), st.enabled)
        };
        if !enabled { return Some(s); }
        if s.abs() < thresh { Some(0.0) } else { Some(s) }
    }
}
impl Source for NoiseGateMod {
    fn current_frame_len(&self) -> Option<usize> { self.inner.current_frame_len() }
    fn channels(&self) -> u16 { self.inner.channels() }
    fn sample_rate(&self) -> u32 { self.inner.sample_rate() }
    fn total_duration(&self) -> Option<Duration> { self.inner.total_duration() }
}

struct CompressorMod {
    inner: Box<dyn Source<Item = f32> + Send>,
    state: Arc<Mutex<ModifierState>>,
    env: f32,
}
impl Iterator for CompressorMod {
    type Item = f32;
    fn next(&mut self) -> Option<f32> {
        let s = self.inner.next()?;
        let (threshold, enabled) = {
            let st = self.state.lock().unwrap();
            (st.value.clamp(0.01, 1.0), st.enabled)
        };
        if !enabled { return Some(s); }
        let abs = s.abs();
        let attack = 0.2_f32;
        let release = 0.01_f32;
        if abs > self.env {
            self.env = self.env * (1.0 - attack) + abs * attack;
        } else {
            self.env = self.env * (1.0 - release) + abs * release;
        }
        let gain = if self.env > threshold {
            let over = self.env / threshold;
            1.0 / over.powf(0.5)
        } else {
            1.0
        };
        Some(s * gain)
    }
}
impl Source for CompressorMod {
    fn current_frame_len(&self) -> Option<usize> { self.inner.current_frame_len() }
    fn channels(&self) -> u16 { self.inner.channels() }
    fn sample_rate(&self) -> u32 { self.inner.sample_rate() }
    fn total_duration(&self) -> Option<Duration> { self.inner.total_duration() }
}

struct LimiterMod {
    inner: Box<dyn Source<Item = f32> + Send>,
    state: Arc<Mutex<ModifierState>>,
}
impl Iterator for LimiterMod {
    type Item = f32;
    fn next(&mut self) -> Option<f32> {
        let s = self.inner.next()?;
        let (ceiling, enabled) = {
            let st = self.state.lock().unwrap();
            (st.value.clamp(0.01, 1.0), st.enabled)
        };
        if !enabled { return Some(s); }
        Some(s.clamp(-ceiling, ceiling))
    }
}
impl Source for LimiterMod {
    fn current_frame_len(&self) -> Option<usize> { self.inner.current_frame_len() }
    fn channels(&self) -> u16 { self.inner.channels() }
    fn sample_rate(&self) -> u32 { self.inner.sample_rate() }
    fn total_duration(&self) -> Option<Duration> { self.inner.total_duration() }
}

struct SaturationMod {
    inner: Box<dyn Source<Item = f32> + Send>,
    state: Arc<Mutex<ModifierState>>,
}
impl Iterator for SaturationMod {
    type Item = f32;
    fn next(&mut self) -> Option<f32> {
        let s = self.inner.next()?;
        let (amt, enabled) = {
            let st = self.state.lock().unwrap();
            (st.value.clamp(0.0, 1.0), st.enabled)
        };
        if !enabled || amt < 1e-4 { return Some(s); }
        let k = 1.0 + amt * 4.0;
        Some((s * k).tanh() / k.tanh().max(1e-3))
    }
}
impl Source for SaturationMod {
    fn current_frame_len(&self) -> Option<usize> { self.inner.current_frame_len() }
    fn channels(&self) -> u16 { self.inner.channels() }
    fn sample_rate(&self) -> u32 { self.inner.sample_rate() }
    fn total_duration(&self) -> Option<Duration> { self.inner.total_duration() }
}

struct RingModMod {
    inner: Box<dyn Source<Item = f32> + Send>,
    state: Arc<Mutex<ModifierState>>,
    sample_idx: u64,
}
impl Iterator for RingModMod {
    type Item = f32;
    fn next(&mut self) -> Option<f32> {
        let s = self.inner.next()?;
        let channels = self.inner.channels().max(1) as u64;
        let sr = self.inner.sample_rate().max(1) as f32;
        let (freq, enabled) = {
            let st = self.state.lock().unwrap();
            (st.value.max(0.0), st.enabled)
        };
        let idx = self.sample_idx;
        self.sample_idx = self.sample_idx.wrapping_add(1);
        if !enabled { return Some(s); }
        let frame = idx / channels;
        let t = frame as f32 / sr;
        let m = (2.0 * std::f32::consts::PI * freq * t).sin();
        Some(s * m)
    }
}
impl Source for RingModMod {
    fn current_frame_len(&self) -> Option<usize> { self.inner.current_frame_len() }
    fn channels(&self) -> u16 { self.inner.channels() }
    fn sample_rate(&self) -> u32 { self.inner.sample_rate() }
    fn total_duration(&self) -> Option<Duration> { self.inner.total_duration() }
}

struct VibratoMod {
    inner: Box<dyn Source<Item = f32> + Send>,
    state: Arc<Mutex<ModifierState>>,
    buffer: Vec<f32>,
    write: usize,
    sample_idx: u64,
    sample_rate: u32,
    channels: u16,
}
impl VibratoMod {
    fn new(inner: Box<dyn Source<Item = f32> + Send>, state: Arc<Mutex<ModifierState>>) -> Self {
        let sr = inner.sample_rate().max(1);
        let ch = inner.channels().max(1);
        let len = (sr as usize / 50).max(64) * ch as usize;
        Self {
            inner,
            state,
            buffer: vec![0.0; len],
            write: 0,
            sample_idx: 0,
            sample_rate: sr,
            channels: ch,
        }
    }
}
impl Iterator for VibratoMod {
    type Item = f32;
    fn next(&mut self) -> Option<f32> {
        let s = self.inner.next()?;
        let (rate, enabled) = {
            let st = self.state.lock().unwrap();
            (st.value.max(0.1), st.enabled)
        };
        self.buffer[self.write] = s;
        self.write = (self.write + 1) % self.buffer.len();
        let idx = self.sample_idx;
        self.sample_idx = self.sample_idx.wrapping_add(1);
        if !enabled { return Some(s); }
        let t = (idx / self.channels.max(1) as u64) as f32 / self.sample_rate as f32;
        let depth_samples = (self.sample_rate as f32 * 0.005) * self.channels as f32;
        let offset =
            depth_samples * (2.0 * std::f32::consts::PI * rate * t).sin();
        let len = self.buffer.len();
        let read_f = (self.write as f32 - (len as f32) * 0.5 + offset).rem_euclid(len as f32);
        let r0 = read_f.floor() as usize % len;
        let r1 = (r0 + self.channels as usize) % len;
        let frac = read_f - read_f.floor();
        Some(self.buffer[r0] * (1.0 - frac) + self.buffer[r1] * frac)
    }
}
impl Source for VibratoMod {
    fn current_frame_len(&self) -> Option<usize> { None }
    fn channels(&self) -> u16 { self.channels }
    fn sample_rate(&self) -> u32 { self.sample_rate }
    fn total_duration(&self) -> Option<Duration> { self.inner.total_duration() }
}

struct ChorusMod {
    inner: Box<dyn Source<Item = f32> + Send>,
    state: Arc<Mutex<ModifierState>>,
    buffer: Vec<f32>,
    write: usize,
    sample_idx: u64,
    sample_rate: u32,
    channels: u16,
}
impl ChorusMod {
    fn new(inner: Box<dyn Source<Item = f32> + Send>, state: Arc<Mutex<ModifierState>>) -> Self {
        let sr = inner.sample_rate().max(1);
        let ch = inner.channels().max(1);
        let len = (sr as usize / 20).max(128) * ch as usize;
        Self {
            inner,
            state,
            buffer: vec![0.0; len],
            write: 0,
            sample_idx: 0,
            sample_rate: sr,
            channels: ch,
        }
    }
}
impl Iterator for ChorusMod {
    type Item = f32;
    fn next(&mut self) -> Option<f32> {
        let s = self.inner.next()?;
        let (mix, enabled) = {
            let st = self.state.lock().unwrap();
            (st.value.clamp(0.0, 1.0), st.enabled)
        };
        self.buffer[self.write] = s;
        self.write = (self.write + 1) % self.buffer.len();
        let idx = self.sample_idx;
        self.sample_idx = self.sample_idx.wrapping_add(1);
        if !enabled { return Some(s); }
        let t = (idx / self.channels.max(1) as u64) as f32 / self.sample_rate as f32;
        let base_delay = self.sample_rate as f32 * 0.015 * self.channels as f32;
        let depth = self.sample_rate as f32 * 0.005 * self.channels as f32;
        let offset = base_delay + depth * (2.0 * std::f32::consts::PI * 1.2 * t).sin();
        let len = self.buffer.len();
        let read_f = (self.write as f32 - offset).rem_euclid(len as f32);
        let r0 = read_f.floor() as usize % len;
        let r1 = (r0 + self.channels as usize) % len;
        let frac = read_f - read_f.floor();
        let wet = self.buffer[r0] * (1.0 - frac) + self.buffer[r1] * frac;
        Some(s * (1.0 - mix * 0.5) + wet * mix)
    }
}
impl Source for ChorusMod {
    fn current_frame_len(&self) -> Option<usize> { None }
    fn channels(&self) -> u16 { self.channels }
    fn sample_rate(&self) -> u32 { self.sample_rate }
    fn total_duration(&self) -> Option<Duration> { self.inner.total_duration() }
}

struct FlangerMod {
    inner: Box<dyn Source<Item = f32> + Send>,
    state: Arc<Mutex<ModifierState>>,
    buffer: Vec<f32>,
    write: usize,
    sample_idx: u64,
    sample_rate: u32,
    channels: u16,
}
impl FlangerMod {
    fn new(inner: Box<dyn Source<Item = f32> + Send>, state: Arc<Mutex<ModifierState>>) -> Self {
        let sr = inner.sample_rate().max(1);
        let ch = inner.channels().max(1);
        let len = (sr as usize / 100).max(64) * ch as usize;
        Self {
            inner,
            state,
            buffer: vec![0.0; len],
            write: 0,
            sample_idx: 0,
            sample_rate: sr,
            channels: ch,
        }
    }
}
impl Iterator for FlangerMod {
    type Item = f32;
    fn next(&mut self) -> Option<f32> {
        let s = self.inner.next()?;
        let (mix, enabled) = {
            let st = self.state.lock().unwrap();
            (st.value.clamp(0.0, 1.0), st.enabled)
        };
        self.buffer[self.write] = s + self.buffer[self.write] * 0.5 * mix;
        self.write = (self.write + 1) % self.buffer.len();
        let idx = self.sample_idx;
        self.sample_idx = self.sample_idx.wrapping_add(1);
        if !enabled { return Some(s); }
        let t = (idx / self.channels.max(1) as u64) as f32 / self.sample_rate as f32;
        let max_delay = (self.buffer.len() - self.channels as usize) as f32;
        let offset = (max_delay * 0.5) * (1.0 + (2.0 * std::f32::consts::PI * 0.5 * t).sin()) * 0.5
            + (self.channels as f32 * 4.0);
        let len = self.buffer.len();
        let read_f = (self.write as f32 - offset).rem_euclid(len as f32);
        let r0 = read_f.floor() as usize % len;
        let r1 = (r0 + self.channels as usize) % len;
        let frac = read_f - read_f.floor();
        let wet = self.buffer[r0] * (1.0 - frac) + self.buffer[r1] * frac;
        Some(s * (1.0 - mix) + wet * mix)
    }
}
impl Source for FlangerMod {
    fn current_frame_len(&self) -> Option<usize> { None }
    fn channels(&self) -> u16 { self.channels }
    fn sample_rate(&self) -> u32 { self.sample_rate }
    fn total_duration(&self) -> Option<Duration> { self.inner.total_duration() }
}

struct StereoWidenMod {
    inner: Box<dyn Source<Item = f32> + Send>,
    state: Arc<Mutex<ModifierState>>,
    chan_idx: u16,
    last_l: f32,
}
impl Iterator for StereoWidenMod {
    type Item = f32;
    fn next(&mut self) -> Option<f32> {
        let s = self.inner.next()?;
        let channels = self.inner.channels().max(1);
        let (width, enabled) = {
            let st = self.state.lock().unwrap();
            (st.value.clamp(0.0, 2.0), st.enabled)
        };
        if channels < 2 || !enabled {
            self.chan_idx = (self.chan_idx + 1) % channels;
            return Some(s);
        }
        let out = if self.chan_idx == 0 {
            self.last_l = s;
            s
        } else {
            let mid = (self.last_l + s) * 0.5;
            let side = (self.last_l - s) * 0.5;
            mid - side * width
        };
        self.chan_idx = (self.chan_idx + 1) % channels;
        Some(out)
    }
}
impl Source for StereoWidenMod {
    fn current_frame_len(&self) -> Option<usize> { self.inner.current_frame_len() }
    fn channels(&self) -> u16 { self.inner.channels() }
    fn sample_rate(&self) -> u32 { self.inner.sample_rate() }
    fn total_duration(&self) -> Option<Duration> { self.inner.total_duration() }
}

struct WobbleMod {
    inner: Box<dyn Source<Item = f32> + Send>,
    state: Arc<Mutex<ModifierState>>,
    sample_idx: u64,
}
impl Iterator for WobbleMod {
    type Item = f32;
    fn next(&mut self) -> Option<f32> {
        let s = self.inner.next()?;
        let channels = self.inner.channels().max(1) as u64;
        let sr = self.inner.sample_rate().max(1) as f32;
        let (rate, enabled) = {
            let st = self.state.lock().unwrap();
            (st.value.max(0.0), st.enabled)
        };
        let idx = self.sample_idx;
        self.sample_idx = self.sample_idx.wrapping_add(1);
        if !enabled { return Some(s); }
        let frame = idx / channels;
        let t = frame as f32 / sr;
        let lfo = (2.0 * std::f32::consts::PI * rate * t).sin();
        let depth = 0.85_f32;
        let gain = 1.0 - depth + depth * (lfo * 0.5 + 0.5);
        Some(s * gain)
    }
}
impl Source for WobbleMod {
    fn current_frame_len(&self) -> Option<usize> { self.inner.current_frame_len() }
    fn channels(&self) -> u16 { self.inner.channels() }
    fn sample_rate(&self) -> u32 { self.inner.sample_rate() }
    fn total_duration(&self) -> Option<Duration> { self.inner.total_duration() }
}

struct TelephoneMod {
    inner: Box<dyn Source<Item = f32> + Send>,
    state: Arc<Mutex<ModifierState>>,
}
impl Iterator for TelephoneMod {
    type Item = f32;
    fn next(&mut self) -> Option<f32> {
        let s = self.inner.next()?;
        let (drive, enabled) = {
            let st = self.state.lock().unwrap();
            (st.value.max(0.0), st.enabled)
        };
        if !enabled { return Some(s); }
        let k = 1.0 + drive * 2.0;
        Some((s * k).tanh() * 0.9)
    }
}
impl Source for TelephoneMod {
    fn current_frame_len(&self) -> Option<usize> { self.inner.current_frame_len() }
    fn channels(&self) -> u16 { self.inner.channels() }
    fn sample_rate(&self) -> u32 { self.inner.sample_rate() }
    fn total_duration(&self) -> Option<Duration> { self.inner.total_duration() }
}

struct UnderwaterMod {
    inner: Box<dyn Source<Item = f32> + Send>,
    state: Arc<Mutex<ModifierState>>,
    sample_idx: u64,
}
impl Iterator for UnderwaterMod {
    type Item = f32;
    fn next(&mut self) -> Option<f32> {
        let s = self.inner.next()?;
        let channels = self.inner.channels().max(1) as u64;
        let sr = self.inner.sample_rate().max(1) as f32;
        let (amt, enabled) = {
            let st = self.state.lock().unwrap();
            (st.value.clamp(0.0, 1.0), st.enabled)
        };
        let idx = self.sample_idx;
        self.sample_idx = self.sample_idx.wrapping_add(1);
        if !enabled { return Some(s); }
        let frame = idx / channels;
        let t = frame as f32 / sr;
        let lfo = (2.0 * std::f32::consts::PI * 0.3 * t).sin();
        Some(s * (1.0 - amt * 0.3 + amt * 0.3 * (lfo * 0.5 + 0.5)))
    }
}
impl Source for UnderwaterMod {
    fn current_frame_len(&self) -> Option<usize> { self.inner.current_frame_len() }
    fn channels(&self) -> u16 { self.inner.channels() }
    fn sample_rate(&self) -> u32 { self.inner.sample_rate() }
    fn total_duration(&self) -> Option<Duration> { self.inner.total_duration() }
}

struct MuteMod {
    inner: Box<dyn Source<Item = f32> + Send>,
    state: Arc<Mutex<ModifierState>>,
}
impl Iterator for MuteMod {
    type Item = f32;
    fn next(&mut self) -> Option<f32> {
        let s = self.inner.next()?;
        let enabled = self.state.lock().unwrap().enabled;
        if enabled { Some(0.0) } else { Some(s) }
    }
}
impl Source for MuteMod {
    fn current_frame_len(&self) -> Option<usize> { self.inner.current_frame_len() }
    fn channels(&self) -> u16 { self.inner.channels() }
    fn sample_rate(&self) -> u32 { self.inner.sample_rate() }
    fn total_duration(&self) -> Option<Duration> { self.inner.total_duration() }
}
