use std::cell::RefCell;
use std::collections::HashMap;
use std::io::Cursor;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use mlua::{
    AnyUserData, Lua, MultiValue, RegistryKey, Table, UserData, UserDataFields, UserDataMethods,
    Value,
};
use rodio::{Decoder, OutputStream, OutputStreamHandle, Sink, Source};

use crate::libs::asset::{FragmentAsset, ShaderAsset};
use crate::libs::signal;

pub const SOUND_EXTS: &[&str] = &["wav", "mp3", "ogg", "flac"];

thread_local! {
    // We hold only the handle here. The `OutputStream` is intentionally leaked
    // (see `output_handle`) because dropping it during process teardown can
    // panic from inside cpal's audio thread on Windows.
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

    Ok(t)
}

/// Raw sound bytes returned by `Asset.GetAsset("Sound", ...)`. Decoding and
/// playback live in SFX; this struct just owns the source bytes so the same
/// asset can be handed to multiple `SFX.LoadSound` calls.
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
    LowPass(u32),
    Delay(f64),
    Repeat,
}

impl UserData for Shader {}

pub struct Sound {
    bytes: Arc<Vec<u8>>,
    source_path: String,
    started_key: Arc<RegistryKey>,
    stopped_key: Arc<RegistryKey>,
    started_table: Table,
    stopped_table: Table,
    shaders: Mutex<Vec<Shader>>,
    attached: Mutex<Vec<AttachedShader>>,
    update_links: Mutex<Vec<UpdateLink>>,
    current_id: Mutex<Option<u64>>,
}

/// One shader attached to a Sound. `params` is shared across the audio thread
/// (which reads per sample) and the Lua thread (which writes via `:SetData`),
/// so live parameter changes take effect mid-playback without re-Play().
#[derive(Clone)]
struct AttachedShader {
    id: u64,
    kind: String,
    params: Params,
}

type Params = Arc<Mutex<HashMap<String, f32>>>;

fn read_param(params: &Params, key: &str, default: f32) -> f32 {
    params.lock().unwrap().get(key).copied().unwrap_or(default)
}

fn parse_shader(code: &str, source: &str) -> mlua::Result<(String, HashMap<String, f32>)> {
    let mut iter = code.lines().filter_map(|raw| {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with("//") {
            None
        } else {
            Some(line)
        }
    });
    let kind = iter
        .next()
        .ok_or_else(|| {
            mlua::Error::RuntimeError(format!(
                "shader '{source}' is empty — first line must be the effect kind"
            ))
        })?
        .to_string();
    let mut params = HashMap::new();
    for line in iter {
        if let Some((k, v)) = line.split_once('=') {
            if let Ok(n) = v.trim().parse::<f32>() {
                params.insert(k.trim().to_string(), n);
            }
        }
    }
    Ok((kind, params))
}

fn shader_id(asset: &AnyUserData) -> mlua::Result<u64> {
    if let Ok(s) = asset.borrow::<ShaderAsset>() {
        return Ok(s.id);
    }
    if let Ok(f) = asset.borrow::<FragmentAsset>() {
        return Ok(f.id);
    }
    Err(mlua::Error::RuntimeError(
        "expected a Shader or Fragment asset".into(),
    ))
}

fn shader_attach_spec(asset: &AnyUserData) -> mlua::Result<AttachedShader> {
    if let Ok(s) = asset.borrow::<ShaderAsset>() {
        let (kind, params) = parse_shader(&s.code, &s.source)?;
        return Ok(AttachedShader {
            id: s.id,
            kind,
            params: Arc::new(Mutex::new(params)),
        });
    }
    if let Ok(f) = asset.borrow::<FragmentAsset>() {
        let (kind, params) = parse_shader(&f.code, &f.source)?;
        return Ok(AttachedShader {
            id: f.id,
            kind,
            params: Arc::new(Mutex::new(params)),
        });
    }
    Err(mlua::Error::RuntimeError(
        "expected a Shader or Fragment asset".into(),
    ))
}

struct UpdateLink {
    interval: f64,
    key: Arc<RegistryKey>,
}

struct ActivePlayback {
    id: u64,
    sink: Sink,
    start: Instant,
    started_fired: bool,
    started_key: Arc<RegistryKey>,
    stopped_key: Arc<RegistryKey>,
    updates: Vec<UpdateState>,
}

struct UpdateState {
    interval: f64,
    next_fire: f64,
    signal_key: Arc<RegistryKey>,
}

fn output_handle() -> mlua::Result<OutputStreamHandle> {
    OUTPUT.with(|cell| {
        let mut borrow = cell.borrow_mut();
        if borrow.is_none() {
            let (stream, handle) = OutputStream::try_default()
                .map_err(|e| mlua::Error::RuntimeError(format!("SFX audio init: {e}")))?;
            // Leak the stream: it must outlive any sink that references it, and
            // dropping it during process teardown panics from inside cpal's
            // audio thread on Windows ("threads should not terminate unexpectedly").
            std::mem::forget(stream);
            *borrow = Some(handle);
        }
        Ok(borrow.as_ref().unwrap().clone())
    })
}

fn load_from_data(lua: &Lua, data: &SoundData) -> mlua::Result<AnyUserData> {
    // Validate up-front so a bad file errors at LoadSound, not at Play.
    Decoder::new(Cursor::new((*data.bytes).clone())).map_err(|e| {
        mlua::Error::RuntimeError(format!("SFX.LoadSound: decode '{}': {e}", data.source))
    })?;

    let started = signal::new_instance(lua)?;
    let stopped = signal::new_instance(lua)?;
    let started_key = Arc::new(lua.create_registry_value(started.clone())?);
    let stopped_key = Arc::new(lua.create_registry_value(stopped.clone())?);

    lua.create_userdata(Sound {
        bytes: data.bytes.clone(),
        source_path: data.source.clone(),
        started_key,
        stopped_key,
        started_table: started,
        stopped_table: stopped,
        shaders: Mutex::new(Vec::new()),
        attached: Mutex::new(Vec::new()),
        update_links: Mutex::new(Vec::new()),
        current_id: Mutex::new(None),
    })
}

impl UserData for Sound {
    fn add_fields<F: UserDataFields<Self>>(f: &mut F) {
        f.add_field_method_get("Started", |_, this| Ok(this.started_table.clone()));
        f.add_field_method_get("Stopped", |_, this| Ok(this.stopped_table.clone()));
        f.add_field_method_get("Source", |_, this| Ok(this.source_path.clone()));
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
                // Writing through the Arc means any currently-playing source
                // sees the change on its next sample read.
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
    let mut source: Box<dyn Source<Item = f32> + Send> =
        Box::new(decoder.convert_samples::<f32>());
    for sh in shaders {
        source = match *sh {
            Shader::Volume(f) => Box::new(source.amplify(f)),
            Shader::Speed(f) => Box::new(source.speed(f)),
            Shader::FadeIn(s) => Box::new(source.fade_in(Duration::from_secs_f64(s))),
            Shader::LowPass(freq) => Box::new(source.low_pass(freq)),
            Shader::Delay(s) => Box::new(source.delay(Duration::from_secs_f64(s))),
            Shader::Repeat => Box::new(source.repeat_infinite()),
        };
    }
    for a in attached {
        source = apply_attached(source, a)?;
    }
    Ok(source)
}

/// Build a source wrapper for one attached shader. The wrapper owns a clone of
/// `attached.params` (an `Arc<Mutex<...>>`), so anything the wrapper reads
/// per-sample will reflect the latest value Lua wrote via `:SetData`.
fn apply_attached(
    source: Box<dyn Source<Item = f32> + Send>,
    attached: &AttachedShader,
) -> mlua::Result<Box<dyn Source<Item = f32> + Send>> {
    let params = attached.params.clone();
    match attached.kind.as_str() {
        // ---- amplitude / time effects ---------------------------------------
        "wobble" | "tremolo" => Ok(Box::new(Tremolo::new(source, params))),
        "volume" | "gain" => Ok(Box::new(LiveGain::new(source, params))),
        // Rodio's combinators snapshot their config; live SetData on these only
        // takes effect after the next Play().
        "speed" => Ok(Box::new(source.speed(read_param(&params, "factor", 1.0)))),
        "lowpass" => Ok(Box::new(
            source.low_pass(read_param(&params, "freq", 1000.0) as u32),
        )),

        // ---- placement / spatial --------------------------------------------
        // Stereo pan. amount=-1 is full left, 0 is center, +1 is full right.
        // Equal-power law so total energy stays roughly constant across the pan.
        "pan" => Ok(Box::new(Pan::new(source, params))),
        // Distance attenuation only, no pan. gain = 1 / (1 + falloff * distance).
        "distance" | "falloff" => Ok(Box::new(Distance::new(source, params))),
        // 3D placement: emitter at (x, y, z), listener at origin facing -z. Mixes
        // the input down to mono and emits stereo with equal-power pan derived
        // from x and inverse-square-ish attenuation from total distance.
        "spatial" | "position" | "3d" => Ok(Box::new(Spatial::new(source, params))),

        other => Err(mlua::Error::RuntimeError(format!(
            "AttachShader: unknown audio shader kind '{other}'"
        ))),
    }
}

// ---------------------------------------------------------------------------
// Effect: Tremolo (amplitude LFO)
// ---------------------------------------------------------------------------
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

// ---------------------------------------------------------------------------
// Effect: LiveGain (live-tunable scalar volume)
// ---------------------------------------------------------------------------
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

// ---------------------------------------------------------------------------
// Effect: Pan (stereo pan, equal-power)
// ---------------------------------------------------------------------------
/// Operates on whatever channel layout the source already has. For mono it
/// passes through unchanged (you can't pan a mono signal without producing
/// stereo — use `spatial` for that). For stereo it scales L and R; for surround
/// layouts the extra channels pass through unchanged.
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
        // Equal-power: gain_l = cos(θ), gain_r = sin(θ), θ = (amount + 1) * π/4
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

// ---------------------------------------------------------------------------
// Effect: Distance (attenuation by distance, no pan)
// ---------------------------------------------------------------------------
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

// ---------------------------------------------------------------------------
// Effect: Spatial (3D position → stereo)
// ---------------------------------------------------------------------------
/// Listener sits at the origin facing -z (right-handed: +x = right, +y = up,
/// +z = behind). Mixes the input to mono and emits stereo. Pan comes from x
/// vs. distance; level comes from `1 / (1 + falloff * distance)`.
///
/// Always outputs 2 channels regardless of input channel count.
struct Spatial<I> {
    inner: I,
    params: Params,
    /// Mono sample held between L (output_channel=0) and R (output_channel=1)
    /// emissions, computed once per *output* frame.
    held: f32,
    /// Stereo gains computed at the same time as `held`, reused for the R sample.
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

    /// Pull one input frame and collapse it to mono.
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
            // Start of a new stereo output frame: pull a mono input frame and
            // recompute spatial gains based on the current parameter snapshot.
            self.held = self.next_mono_frame()?;

            let p = self.params.lock().unwrap();
            let x = *p.get("x").unwrap_or(&0.0);
            let y = *p.get("y").unwrap_or(&0.0);
            let z = *p.get("z").unwrap_or(&0.0);
            let falloff = p.get("falloff").copied().unwrap_or(1.0).max(0.0);
            drop(p);

            let dist = (x * x + y * y + z * z).sqrt();
            let attenuation = 1.0 / (1.0 + falloff * dist);

            // Pan from the x component, normalized by the radial distance so
            // sources in front (z<0) and behind sit correctly between the ears.
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
        // Don't let cpal cache frame boundaries we can't honor — stereo output
        // doesn't line up with the inner source's frame layout.
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
    // Replace any prior playback for this Sound. We drop the previous active
    // entry directly so its Stopped event won't fire twice.
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
    let sink = Sink::try_new(&handle)
        .map_err(|e| mlua::Error::RuntimeError(format!("SFX sink: {e}")))?;
    let shaders = this.shaders.lock().unwrap().clone();
    let attached = this.attached.lock().unwrap().clone();
    let source = build_source(&this.bytes, &shaders, &attached)?;
    sink.append(source);

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

    ACTIVE.with(|c| {
        c.borrow_mut().push(ActivePlayback {
            id,
            sink,
            start: Instant::now(),
            started_fired: false,
            started_key: this.started_key.clone(),
            stopped_key: this.stopped_key.clone(),
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
            // Don't remove yet — let pump notice the empty sink and fire Stopped.
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
        // Anything pushed during firing (e.g. Stopped handler called Play) goes
        // after the survivors so newly-started playbacks aren't reaped this tick.
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
