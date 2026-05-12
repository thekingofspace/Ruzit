use std::cell::RefCell;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use mlua::{AnyUserData, UserData, UserDataFields, UserDataMethods, Value};

use crate::libs::asset::ImageAsset;
use crate::libs::primitives::{CFrame, Color3, Vector};
use crate::libs::shader::{AttachedShader as AudioAttachedShader, shader_attach_spec, shader_id};

use super::PartTextureRef;

pub use crate::libs::gui::AttachedShader as ParticleAttachedShader;

static NEXT_EFFECT_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Copy, Debug)]
pub struct Range {
    pub min: f32,
    pub max: f32,
}

impl Range {
    pub fn new(min: f32, max: f32) -> Self {
        Self { min, max }
    }
    pub fn constant(v: f32) -> Self {
        Self { min: v, max: v }
    }
    fn sample(&self, t: f32) -> f32 {
        self.min + (self.max - self.min) * t
    }
}

#[derive(Clone, Debug)]
pub struct NumberSequence {
    pub stops: Vec<(f32, f32)>,
}

impl NumberSequence {
    pub fn constant(v: f32) -> Self {
        Self {
            stops: vec![(0.0, v), (1.0, v)],
        }
    }
    pub fn sample(&self, t: f32) -> f32 {
        let t = t.clamp(0.0, 1.0);
        if self.stops.is_empty() {
            return 0.0;
        }
        if self.stops.len() == 1 {
            return self.stops[0].1;
        }
        if t <= self.stops[0].0 {
            return self.stops[0].1;
        }
        let last = self.stops.last().unwrap();
        if t >= last.0 {
            return last.1;
        }
        for w in self.stops.windows(2) {
            let (a, b) = (w[0], w[1]);
            if t >= a.0 && t <= b.0 {
                let span = (b.0 - a.0).max(1e-6);
                let f = (t - a.0) / span;
                return a.1 + (b.1 - a.1) * f;
            }
        }
        last.1
    }
}

#[derive(Clone, Debug)]
pub struct ColorSequence {
    pub stops: Vec<(f32, Color3)>,
}

impl ColorSequence {
    pub fn constant(c: Color3) -> Self {
        Self {
            stops: vec![(0.0, c), (1.0, c)],
        }
    }
    pub fn sample(&self, t: f32) -> Color3 {
        let t = t.clamp(0.0, 1.0);
        if self.stops.is_empty() {
            return Color3::new(1.0, 1.0, 1.0);
        }
        if self.stops.len() == 1 {
            return self.stops[0].1;
        }
        if t <= self.stops[0].0 {
            return self.stops[0].1;
        }
        let last = self.stops.last().unwrap();
        if t >= last.0 {
            return last.1;
        }
        for w in self.stops.windows(2) {
            let (a, b) = (w[0], w[1]);
            if t >= a.0 && t <= b.0 {
                let span = (b.0 - a.0).max(1e-6);
                let f = (t - a.0) / span;
                return Color3::new(
                    a.1.r + (b.1.r - a.1.r) * f,
                    a.1.g + (b.1.g - a.1.g) * f,
                    a.1.b + (b.1.b - a.1.b) * f,
                );
            }
        }
        last.1
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Particle {
    pub position: Vector,
    pub velocity: Vector,
    pub age: f32,
    pub lifetime: f32,
    pub rotation: f32,
    pub rot_speed: f32,
    pub seed_size: f32,
    pub random_force: Vector,
}

pub struct EffectVolumeState {
    pub id: u64,
    pub alive: bool,
    pub enabled: bool,

    pub cframe: CFrame,
    pub size: Vector,
    pub texture: Option<PartTextureRef>,

    pub rate: f32,
    pub lifetime: Range,
    pub speed: Range,
    pub acceleration: Vector,
    pub randomize_force_x: Range,
    pub randomize_force_y: Range,
    pub randomize_force_z: Range,
    pub drag: f32,
    pub spread: f32,
    pub emission_direction: Vector,
    pub rotation_init: Range,
    pub rot_speed: Range,
    pub size_init: Range,

    pub color: ColorSequence,
    pub size_over_life: NumberSequence,
    pub transparency: NumberSequence,

    pub max_particles: usize,
    pub particles: Vec<Particle>,
    pub spawn_accumulator: f32,
    pub rng_state: u64,

    pub face_camera: bool,

    pub attached: Vec<ParticleAttachedShader>,
    pub audio_attached: Vec<AudioAttachedShader>,
}

thread_local! {
    static EFFECT_VOLUMES: RefCell<Vec<Arc<Mutex<EffectVolumeState>>>> =
        const { RefCell::new(Vec::new()) };
}

fn lcg(state: &mut u64) -> u32 {
    *state = state.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
    (*state >> 33) as u32
}

fn rand_unit(state: &mut u64) -> f32 {
    (lcg(state) as f32) / (u32::MAX as f32)
}

fn rand_signed(state: &mut u64) -> f32 {
    rand_unit(state) * 2.0 - 1.0
}

fn rand_range(state: &mut u64, r: Range) -> f32 {
    r.sample(rand_unit(state))
}

fn euler_to_axes(rot: Vector) -> (Vector, Vector, Vector) {
    let sx = rot.x.sin();
    let cx = rot.x.cos();
    let sy = rot.y.sin();
    let cy = rot.y.cos();
    let sz = rot.z.sin();
    let cz = rot.z.cos();
    let x_axis = Vector::new(cy * cz, sx * sy * cz + cx * sz, -cx * sy * cz + sx * sz);
    let y_axis = Vector::new(-cy * sz, -sx * sy * sz + cx * cz, cx * sy * sz + sx * cz);
    let z_axis = Vector::new(sy, -sx * cy, cx * cy);
    (x_axis, y_axis, z_axis)
}

fn rotate_around(axis_x: Vector, axis_y: Vector, axis_z: Vector, local: Vector) -> Vector {
    Vector::new(
        axis_x.x * local.x + axis_y.x * local.y + axis_z.x * local.z,
        axis_x.y * local.x + axis_y.y * local.y + axis_z.y * local.z,
        axis_x.z * local.x + axis_y.z * local.y + axis_z.z * local.z,
    )
}

fn normalize_or(v: Vector, fallback: Vector) -> Vector {
    let len = (v.x * v.x + v.y * v.y + v.z * v.z).sqrt();
    if len > 1e-6 {
        Vector::new(v.x / len, v.y / len, v.z / len)
    } else {
        fallback
    }
}

fn cone_sample(state: &mut u64, axis: Vector, half_angle_rad: f32) -> Vector {
    let cos_min = half_angle_rad.cos();
    let cos_t = cos_min + (1.0 - cos_min) * rand_unit(state);
    let sin_t = (1.0 - cos_t * cos_t).max(0.0).sqrt();
    let phi = rand_unit(state) * std::f32::consts::TAU;

    let helper = if axis.y.abs() < 0.999 {
        Vector::new(0.0, 1.0, 0.0)
    } else {
        Vector::new(1.0, 0.0, 0.0)
    };
    let right = normalize_or(
        Vector::new(
            axis.y * helper.z - axis.z * helper.y,
            axis.z * helper.x - axis.x * helper.z,
            axis.x * helper.y - axis.y * helper.x,
        ),
        Vector::new(1.0, 0.0, 0.0),
    );
    let up = Vector::new(
        axis.y * right.z - axis.z * right.y,
        axis.z * right.x - axis.x * right.z,
        axis.x * right.y - axis.y * right.x,
    );
    let cos_phi = phi.cos();
    let sin_phi = phi.sin();
    Vector::new(
        axis.x * cos_t + (right.x * cos_phi + up.x * sin_phi) * sin_t,
        axis.y * cos_t + (right.y * cos_phi + up.y * sin_phi) * sin_t,
        axis.z * cos_t + (right.z * cos_phi + up.z * sin_phi) * sin_t,
    )
}

fn spawn_one(s: &mut EffectVolumeState) {
    if s.particles.len() >= s.max_particles {
        return;
    }

    let (ax, ay, az) = euler_to_axes(s.cframe.rotation);
    let local = Vector::new(
        rand_signed(&mut s.rng_state) * 0.5 * s.size.x,
        rand_signed(&mut s.rng_state) * 0.5 * s.size.y,
        rand_signed(&mut s.rng_state) * 0.5 * s.size.z,
    );
    let world_offset = rotate_around(ax, ay, az, local);
    let position = Vector::new(
        s.cframe.position.x + world_offset.x,
        s.cframe.position.y + world_offset.y,
        s.cframe.position.z + world_offset.z,
    );

    let dir_local = normalize_or(s.emission_direction, Vector::new(0.0, 1.0, 0.0));
    let dir_world = normalize_or(
        rotate_around(ax, ay, az, dir_local),
        Vector::new(0.0, 1.0, 0.0),
    );
    let spread_rad = s.spread.to_radians().clamp(0.0, std::f32::consts::PI);
    let dir = if spread_rad <= 1e-4 {
        dir_world
    } else {
        cone_sample(&mut s.rng_state, dir_world, spread_rad * 0.5)
    };

    let speed = rand_range(&mut s.rng_state, s.speed);
    let velocity = Vector::new(dir.x * speed, dir.y * speed, dir.z * speed);
    let lifetime = rand_range(&mut s.rng_state, s.lifetime).max(0.01);
    let rotation = rand_range(&mut s.rng_state, s.rotation_init);
    let rot_speed = rand_range(&mut s.rng_state, s.rot_speed);
    let size = rand_range(&mut s.rng_state, s.size_init).max(0.0);
    let random_force = Vector::new(
        rand_range(&mut s.rng_state, s.randomize_force_x),
        rand_range(&mut s.rng_state, s.randomize_force_y),
        rand_range(&mut s.rng_state, s.randomize_force_z),
    );

    s.particles.push(Particle {
        position,
        velocity,
        age: 0.0,
        lifetime,
        rotation,
        rot_speed,
        seed_size: size,
        random_force,
    });
}

pub fn tick_effect_volumes(dt: f32) {
    EFFECT_VOLUMES.with(|c| {
        let mut reg = c.borrow_mut();
        reg.retain(|v| v.lock().unwrap().alive);
        for arc in reg.iter() {
            let mut s = arc.lock().unwrap();
            let drag = s.drag.max(0.0);
            let accel = s.acceleration;
            for p in s.particles.iter_mut() {
                p.age += dt;
                let attenuate = (1.0 - drag * dt).clamp(0.0, 1.0);
                p.velocity.x = p.velocity.x * attenuate + (accel.x + p.random_force.x) * dt;
                p.velocity.y = p.velocity.y * attenuate + (accel.y + p.random_force.y) * dt;
                p.velocity.z = p.velocity.z * attenuate + (accel.z + p.random_force.z) * dt;
                p.position.x += p.velocity.x * dt;
                p.position.y += p.velocity.y * dt;
                p.position.z += p.velocity.z * dt;
                p.rotation += p.rot_speed * dt;
            }
            s.particles.retain(|p| p.age < p.lifetime);

            if s.enabled && s.rate > 0.0 {
                s.spawn_accumulator += s.rate * dt;
                while s.spawn_accumulator >= 1.0 {
                    s.spawn_accumulator -= 1.0;
                    spawn_one(&mut s);
                }
            } else {
                s.spawn_accumulator = 0.0;
            }
        }
    });
}

#[derive(Clone)]
pub struct ParticleRender {
    pub position: Vector,
    pub color: Color3,
    pub alpha: f32,
    pub size: f32,
    pub rotation: f32,
    pub life_t: f32,
}

#[derive(Clone)]
pub struct EffectVolumeRender {
    pub id: u64,
    pub texture: Option<PartTextureRef>,
    pub active_shader: Option<ParticleAttachedShader>,
    pub face_camera: bool,
    pub particles: Vec<ParticleRender>,
}

pub fn effect_volume_snapshot() -> Vec<EffectVolumeRender> {
    EFFECT_VOLUMES.with(|c| {
        let reg = c.borrow();
        reg.iter()
            .filter_map(|arc| {
                let s = arc.lock().unwrap();
                if !s.alive || s.particles.is_empty() {
                    return None;
                }
                let particles = s
                    .particles
                    .iter()
                    .map(|p| {
                        let t = (p.age / p.lifetime).clamp(0.0, 1.0);
                        let color = s.color.sample(t);
                        let alpha = (1.0 - s.transparency.sample(t)).clamp(0.0, 1.0);
                        let size_mul = s.size_over_life.sample(t).max(0.0);
                        ParticleRender {
                            position: p.position,
                            color,
                            alpha,
                            size: p.seed_size * size_mul,
                            rotation: p.rotation,
                            life_t: t,
                        }
                    })
                    .collect();
                Some(EffectVolumeRender {
                    id: s.id,
                    texture: s.texture.clone(),
                    active_shader: s.attached.last().cloned(),
                    face_camera: s.face_camera,
                    particles,
                })
            })
            .collect()
    })
}

pub fn is_active() -> bool {
    EFFECT_VOLUMES.with(|c| {
        c.borrow()
            .iter()
            .any(|v| !v.lock().unwrap().particles.is_empty())
    })
}

pub struct EffectVolumeHandle {
    pub inner: Arc<Mutex<EffectVolumeState>>,
}

fn texture_from_value(v: &Value) -> mlua::Result<Option<PartTextureRef>> {
    match v {
        Value::Nil => Ok(None),
        Value::UserData(ud) => {
            if let Ok(img) = ud.borrow::<ImageAsset>() {
                Ok(Some(PartTextureRef {
                    id: img.id,
                    width: img.width,
                    height: img.height,
                    data: img.data.clone(),
                    version: 0,
                    live: None,
                }))
            } else if let Ok(dyn_img) = ud.borrow::<crate::libs::dynimg::DynImgHandle>() {
                Ok(Some(crate::libs::dynimg::dynimg_to_part_texture(&dyn_img)))
            } else if let Ok(drawable) =
                ud.borrow::<crate::libs::drawable::DrawableImgHandle>()
            {
                Ok(Some(crate::libs::drawable::drawable_to_part_texture(
                    &drawable,
                )))
            } else {
                Err(mlua::Error::RuntimeError(
                    "EffectVolume.Image expects an ImageAsset, DynImg, DrawableImg, or nil".into(),
                ))
            }
        }
        _ => Err(mlua::Error::RuntimeError(
            "EffectVolume.Image expects an ImageAsset, DynImg, DrawableImg, or nil".into(),
        )),
    }
}

pub(crate) fn range_from_value(v: Value, name: &str) -> mlua::Result<Range> {
    match v {
        Value::Integer(n) => Ok(Range::constant(n as f32)),
        Value::Number(n) => Ok(Range::constant(n as f32)),
        Value::Table(t) => {
            let mut a: Option<f32> = None;
            let mut b: Option<f32> = None;
            if let Ok(n) = t.get::<f32>(1) {
                a = Some(n);
            }
            if let Ok(n) = t.get::<f32>(2) {
                b = Some(n);
            }
            if let Ok(n) = t.get::<f32>("Min") {
                a = Some(n);
            }
            if let Ok(n) = t.get::<f32>("min") {
                a = Some(n);
            }
            if let Ok(n) = t.get::<f32>("Max") {
                b = Some(n);
            }
            if let Ok(n) = t.get::<f32>("max") {
                b = Some(n);
            }
            match (a, b) {
                (Some(min), Some(max)) => Ok(Range::new(min.min(max), min.max(max))),
                (Some(only), None) => Ok(Range::constant(only)),
                _ => Err(mlua::Error::RuntimeError(format!(
                    "EffectVolume.{name} expects a number or a table {{ Min, Max }}"
                ))),
            }
        }
        _ => Err(mlua::Error::RuntimeError(format!(
            "EffectVolume.{name} expects a number or a table {{ Min, Max }}"
        ))),
    }
}

pub(crate) fn range_to_table(lua: &mlua::Lua, r: Range) -> mlua::Result<mlua::Table> {
    let t = lua.create_table()?;
    t.set("Min", r.min)?;
    t.set("Max", r.max)?;
    Ok(t)
}

pub(crate) fn number_sequence_from_value(
    v: Value,
    name: &str,
) -> mlua::Result<NumberSequence> {
    match v {
        Value::Integer(n) => Ok(NumberSequence::constant(n as f32)),
        Value::Number(n) => Ok(NumberSequence::constant(n as f32)),
        Value::Table(t) => {
            let mut stops: Vec<(f32, f32)> = Vec::new();
            let len = t.len().unwrap_or(0) as usize;
            if len > 0 {
                if let Ok(first) = t.get::<mlua::Value>(1) {
                    if matches!(first, mlua::Value::Table(_)) {
                        for i in 1..=len {
                            let pair: mlua::Table = t.get(i)?;
                            let t_at: f32 = pair.get(1)?;
                            let value: f32 = pair.get(2)?;
                            stops.push((t_at.clamp(0.0, 1.0), value));
                        }
                    }
                }
            }
            if stops.is_empty() {
                for pair in t.pairs::<mlua::Value, f32>() {
                    let (k, val) = pair?;
                    let t_at = match k {
                        mlua::Value::Integer(n) => n as f32,
                        mlua::Value::Number(n) => n as f32,
                        _ => continue,
                    };
                    stops.push((t_at.clamp(0.0, 1.0), val));
                }
            }
            if stops.is_empty() {
                return Err(mlua::Error::RuntimeError(format!(
                    "{name}: sequence table must contain at least one stop \
                     (formats: {{ [t]=value, ... }} or {{ {{ t, value }}, ... }})"
                )));
            }
            stops.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
            Ok(NumberSequence { stops })
        }
        _ => Err(mlua::Error::RuntimeError(format!(
            "{name} expects a number or a table of {{ [t]=value, ... }} or {{ {{ t, value }}, ... }} stops"
        ))),
    }
}

pub(crate) fn number_sequence_to_table(
    lua: &mlua::Lua,
    seq: &NumberSequence,
) -> mlua::Result<mlua::Table> {
    let out = lua.create_table()?;
    for (i, (t, v)) in seq.stops.iter().enumerate() {
        let pair = lua.create_table()?;
        pair.set(1, *t)?;
        pair.set(2, *v)?;
        out.set(i + 1, pair)?;
    }
    Ok(out)
}

pub(crate) fn color_sequence_from_value(
    v: Value,
    name: &str,
) -> mlua::Result<ColorSequence> {
    match v {
        Value::UserData(ud) => {
            if let Ok(c) = ud.borrow::<Color3>() {
                return Ok(ColorSequence::constant(*c));
            }
            Err(mlua::Error::RuntimeError(format!(
                "{name} expects a Color3 or a table of stops"
            )))
        }
        Value::Table(t) => {
            let mut stops: Vec<(f32, Color3)> = Vec::new();
            let len = t.len().unwrap_or(0) as usize;
            if len > 0 {
                if let Ok(first) = t.get::<mlua::Value>(1) {
                    if matches!(first, mlua::Value::Table(_)) {
                        for i in 1..=len {
                            let pair: mlua::Table = t.get(i)?;
                            let t_at: f32 = pair.get(1)?;
                            let color_v: AnyUserData = pair.get(2)?;
                            let c = *color_v.borrow::<Color3>().map_err(|_| {
                                mlua::Error::RuntimeError(format!(
                                    "{name}: second element of each stop must be a Color3"
                                ))
                            })?;
                            stops.push((t_at.clamp(0.0, 1.0), c));
                        }
                    }
                }
            }
            if stops.is_empty() {
                for pair in t.pairs::<mlua::Value, AnyUserData>() {
                    let (k, ud) = pair?;
                    let t_at = match k {
                        mlua::Value::Integer(n) => n as f32,
                        mlua::Value::Number(n) => n as f32,
                        _ => continue,
                    };
                    let c = *ud.borrow::<Color3>().map_err(|_| {
                        mlua::Error::RuntimeError(format!(
                            "{name}: value at key {t_at} must be a Color3"
                        ))
                    })?;
                    stops.push((t_at.clamp(0.0, 1.0), c));
                }
            }
            if stops.is_empty() {
                return Err(mlua::Error::RuntimeError(format!(
                    "{name}: sequence table must contain at least one stop \
                     (formats: {{ [t]=Color3, ... }} or {{ {{ t, Color3 }}, ... }})"
                )));
            }
            stops.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
            Ok(ColorSequence { stops })
        }
        _ => Err(mlua::Error::RuntimeError(format!(
            "{name} expects a Color3 or a table of stops"
        ))),
    }
}

pub(crate) fn color_sequence_to_table(
    lua: &mlua::Lua,
    seq: &ColorSequence,
) -> mlua::Result<mlua::Table> {
    let out = lua.create_table()?;
    for (i, (t, c)) in seq.stops.iter().enumerate() {
        let pair = lua.create_table()?;
        pair.set(1, *t)?;
        pair.set(2, lua.create_userdata(*c)?)?;
        out.set(i + 1, pair)?;
    }
    Ok(out)
}

impl UserData for EffectVolumeHandle {
    fn add_fields<F: UserDataFields<Self>>(f: &mut F) {
        f.add_field_method_get("IsAlive", |_, this| Ok(this.inner.lock().unwrap().alive));
        f.add_field_method_get("ParticleCount", |_, this| {
            Ok(this.inner.lock().unwrap().particles.len() as i64)
        });

        f.add_field_method_get("Enabled", |_, this| {
            Ok(this.inner.lock().unwrap().enabled)
        });
        f.add_field_method_set("Enabled", |_, this, v: bool| {
            this.inner.lock().unwrap().enabled = v;
            Ok(())
        });

        f.add_field_method_get("FaceCamera", |_, this| {
            Ok(this.inner.lock().unwrap().face_camera)
        });
        f.add_field_method_set("FaceCamera", |_, this, v: bool| {
            this.inner.lock().unwrap().face_camera = v;
            Ok(())
        });

        f.add_field_method_get("CFrame", |_, this| Ok(this.inner.lock().unwrap().cframe));
        f.add_field_method_set("CFrame", |_, this, ud: AnyUserData| {
            let cf = *ud.borrow::<CFrame>().map_err(|_| {
                mlua::Error::RuntimeError("EffectVolume.CFrame expects a CFrame".into())
            })?;
            this.inner.lock().unwrap().cframe = cf;
            Ok(())
        });

        f.add_field_method_get("Size", |_, this| Ok(this.inner.lock().unwrap().size));
        f.add_field_method_set("Size", |_, this, v: Vector| {
            this.inner.lock().unwrap().size = v;
            Ok(())
        });

        f.add_field_method_set("Image", |_, this, v: Value| {
            let tex = texture_from_value(&v)?;
            this.inner.lock().unwrap().texture = tex;
            Ok(())
        });
        f.add_field_method_get("Image", |_, this| {
            Ok(this.inner.lock().unwrap().texture.is_some())
        });

        f.add_field_method_get("Rate", |_, this| Ok(this.inner.lock().unwrap().rate));
        f.add_field_method_set("Rate", |_, this, v: f32| {
            this.inner.lock().unwrap().rate = v.max(0.0);
            Ok(())
        });

        f.add_field_method_get("MaxParticles", |_, this| {
            Ok(this.inner.lock().unwrap().max_particles as i64)
        });
        f.add_field_method_set("MaxParticles", |_, this, v: i64| {
            let v = v.max(1).min(100_000) as usize;
            let mut s = this.inner.lock().unwrap();
            s.max_particles = v;
            if s.particles.len() > v {
                s.particles.truncate(v);
            }
            Ok(())
        });

        f.add_field_method_get("Lifetime", |lua, this| {
            range_to_table(lua, this.inner.lock().unwrap().lifetime)
        });
        f.add_field_method_set("Lifetime", |_, this, v: Value| {
            let r = range_from_value(v, "Lifetime")?;
            this.inner.lock().unwrap().lifetime = r;
            Ok(())
        });

        f.add_field_method_get("Speed", |lua, this| {
            range_to_table(lua, this.inner.lock().unwrap().speed)
        });
        f.add_field_method_set("Speed", |_, this, v: Value| {
            let r = range_from_value(v, "Speed")?;
            this.inner.lock().unwrap().speed = r;
            Ok(())
        });

        f.add_field_method_get("Acceleration", |_, this| {
            Ok(this.inner.lock().unwrap().acceleration)
        });
        f.add_field_method_set("Acceleration", |_, this, v: Vector| {
            this.inner.lock().unwrap().acceleration = v;
            Ok(())
        });

        f.add_field_method_get("RandomizeForce", |lua, this| {
            let s = this.inner.lock().unwrap();
            let t = lua.create_table()?;
            t.set("X", range_to_table(lua, s.randomize_force_x)?)?;
            t.set("Y", range_to_table(lua, s.randomize_force_y)?)?;
            t.set("Z", range_to_table(lua, s.randomize_force_z)?)?;
            Ok(t)
        });
        f.add_field_method_set("RandomizeForce", |_, this, v: Value| {
            let t = match v {
                Value::Table(t) => t,
                Value::Nil => {
                    let mut s = this.inner.lock().unwrap();
                    s.randomize_force_x = Range::new(0.0, 0.0);
                    s.randomize_force_y = Range::new(0.0, 0.0);
                    s.randomize_force_z = Range::new(0.0, 0.0);
                    return Ok(());
                }
                _ => {
                    return Err(mlua::Error::RuntimeError(
                        "EffectVolume.RandomizeForce expects a table { X = {min,max}, Y = ..., Z = ... } or nil".into(),
                    ));
                }
            };
            let x_v: Value = t.get("X").unwrap_or(Value::Nil);
            let y_v: Value = t.get("Y").unwrap_or(Value::Nil);
            let z_v: Value = t.get("Z").unwrap_or(Value::Nil);
            let x = match x_v {
                Value::Nil => Range::new(0.0, 0.0),
                other => range_from_value(other, "EffectVolume.RandomizeForce.X")?,
            };
            let y = match y_v {
                Value::Nil => Range::new(0.0, 0.0),
                other => range_from_value(other, "EffectVolume.RandomizeForce.Y")?,
            };
            let z = match z_v {
                Value::Nil => Range::new(0.0, 0.0),
                other => range_from_value(other, "EffectVolume.RandomizeForce.Z")?,
            };
            let mut s = this.inner.lock().unwrap();
            s.randomize_force_x = x;
            s.randomize_force_y = y;
            s.randomize_force_z = z;
            Ok(())
        });

        f.add_field_method_get("Drag", |_, this| Ok(this.inner.lock().unwrap().drag));
        f.add_field_method_set("Drag", |_, this, v: f32| {
            this.inner.lock().unwrap().drag = v.max(0.0);
            Ok(())
        });

        f.add_field_method_get("Spread", |_, this| Ok(this.inner.lock().unwrap().spread));
        f.add_field_method_set("Spread", |_, this, v: f32| {
            this.inner.lock().unwrap().spread = v.clamp(0.0, 180.0);
            Ok(())
        });

        f.add_field_method_get("EmissionDirection", |_, this| {
            Ok(this.inner.lock().unwrap().emission_direction)
        });
        f.add_field_method_set("EmissionDirection", |_, this, v: Vector| {
            this.inner.lock().unwrap().emission_direction = v;
            Ok(())
        });

        f.add_field_method_get("Rotation", |lua, this| {
            range_to_table(lua, this.inner.lock().unwrap().rotation_init)
        });
        f.add_field_method_set("Rotation", |_, this, v: Value| {
            let r = range_from_value(v, "Rotation")?;
            this.inner.lock().unwrap().rotation_init = r;
            Ok(())
        });

        f.add_field_method_get("RotSpeed", |lua, this| {
            range_to_table(lua, this.inner.lock().unwrap().rot_speed)
        });
        f.add_field_method_set("RotSpeed", |_, this, v: Value| {
            let r = range_from_value(v, "RotSpeed")?;
            this.inner.lock().unwrap().rot_speed = r;
            Ok(())
        });

        f.add_field_method_get("ParticleSize", |lua, this| {
            range_to_table(lua, this.inner.lock().unwrap().size_init)
        });
        f.add_field_method_set("ParticleSize", |_, this, v: Value| {
            let r = range_from_value(v, "ParticleSize")?;
            this.inner.lock().unwrap().size_init = r;
            Ok(())
        });

        f.add_field_method_get("Color", |lua, this| {
            color_sequence_to_table(lua, &this.inner.lock().unwrap().color)
        });
        f.add_field_method_set("Color", |_, this, v: Value| {
            let seq = color_sequence_from_value(v, "Color")?;
            this.inner.lock().unwrap().color = seq;
            Ok(())
        });

        f.add_field_method_get("SizeOverLife", |lua, this| {
            number_sequence_to_table(lua, &this.inner.lock().unwrap().size_over_life)
        });
        f.add_field_method_set("SizeOverLife", |_, this, v: Value| {
            let seq = number_sequence_from_value(v, "SizeOverLife")?;
            this.inner.lock().unwrap().size_over_life = seq;
            Ok(())
        });

        f.add_field_method_get("Transparency", |lua, this| {
            number_sequence_to_table(lua, &this.inner.lock().unwrap().transparency)
        });
        f.add_field_method_set("Transparency", |_, this, v: Value| {
            let seq = number_sequence_from_value(v, "EffectVolume.Transparency")?;
            this.inner.lock().unwrap().transparency = seq;
            Ok(())
        });

        f.add_field_method_get("TimeScaleColor", |lua, this| {
            color_sequence_to_table(lua, &this.inner.lock().unwrap().color)
        });
        f.add_field_method_set("TimeScaleColor", |_, this, v: Value| {
            let seq = color_sequence_from_value(v, "EffectVolume.TimeScaleColor")?;
            this.inner.lock().unwrap().color = seq;
            Ok(())
        });

        f.add_field_method_get("TimeScaleTransparency", |lua, this| {
            number_sequence_to_table(lua, &this.inner.lock().unwrap().transparency)
        });
        f.add_field_method_set("TimeScaleTransparency", |_, this, v: Value| {
            let seq = number_sequence_from_value(v, "EffectVolume.TimeScaleTransparency")?;
            this.inner.lock().unwrap().transparency = seq;
            Ok(())
        });
    }

    fn add_methods<M: UserDataMethods<Self>>(m: &mut M) {
        m.add_method("Emit", |_, this, n: Option<i64>| {
            let n = n.unwrap_or(1).max(0) as usize;
            let mut s = this.inner.lock().unwrap();
            for _ in 0..n {
                spawn_one(&mut s);
            }
            Ok(())
        });

        m.add_method("Clear", |_, this, _: ()| {
            this.inner.lock().unwrap().particles.clear();
            Ok(())
        });

        m.add_method("Destroy", |_, this, _: ()| {
            let mut s = this.inner.lock().unwrap();
            s.alive = false;
            s.enabled = false;
            s.particles.clear();
            s.attached.clear();
            s.audio_attached.clear();
            Ok(())
        });

        m.add_method(
            "AttachShader",
            |_, this, asset: AnyUserData| -> mlua::Result<()> {
                let attached = crate::libs::gui::build_attached(&asset)?;
                let mut s = this.inner.lock().unwrap();
                if s.attached.iter().any(|e| e.id == attached.id) {
                    return Err(mlua::Error::RuntimeError(
                        "AttachShader: shader is already attached".into(),
                    ));
                }
                s.attached.push(attached);
                Ok(())
            },
        );
        m.add_method(
            "DetachShader",
            |_, this, asset: AnyUserData| -> mlua::Result<()> {
                let id = crate::libs::gui::shader_asset_id(&asset)?;
                this.inner
                    .lock()
                    .unwrap()
                    .attached
                    .retain(|e| e.id != id);
                Ok(())
            },
        );
        m.add_method("ClearShaders", |_, this, _: ()| {
            this.inner.lock().unwrap().attached.clear();
            Ok(())
        });
        m.add_method(
            "SetData",
            |_, this, (asset, name, value): (AnyUserData, String, f32)| -> mlua::Result<()> {
                let id = crate::libs::gui::shader_asset_id(&asset)?;
                let s = this.inner.lock().unwrap();
                let entry = s.attached.iter().find(|e| e.id == id).ok_or_else(|| {
                    mlua::Error::RuntimeError(
                        "SetData: shader is not attached to this EffectVolume".into(),
                    )
                })?;
                let slot = *entry.slot_of_name.get(&name).ok_or_else(|| {
                    mlua::Error::RuntimeError(format!(
                        "SetData: '{name}' is not a `// @ruzit param` declared in the shader"
                    ))
                })?;
                entry.params.lock().unwrap()[slot as usize] = value;
                Ok(())
            },
        );
        m.add_method(
            "GetData",
            |_, this, (asset, name): (AnyUserData, String)| -> mlua::Result<Option<f32>> {
                let id = crate::libs::gui::shader_asset_id(&asset)?;
                let s = this.inner.lock().unwrap();
                let entry = s.attached.iter().find(|e| e.id == id).ok_or_else(|| {
                    mlua::Error::RuntimeError(
                        "GetData: shader is not attached to this EffectVolume".into(),
                    )
                })?;
                let Some(slot) = entry.slot_of_name.get(&name) else {
                    return Ok(None);
                };
                Ok(Some(entry.params.lock().unwrap()[*slot as usize]))
            },
        );

        m.add_method(
            "AttachAudioShader",
            |_, this, asset: AnyUserData| -> mlua::Result<()> {
                let attached = shader_attach_spec(&asset)?;
                let mut s = this.inner.lock().unwrap();
                if s.audio_attached.iter().any(|e| e.id == attached.id) {
                    return Err(mlua::Error::RuntimeError(
                        "AttachAudioShader: shader is already attached".into(),
                    ));
                }
                s.audio_attached.push(attached);
                Ok(())
            },
        );
        m.add_method(
            "DetachAudioShader",
            |_, this, asset: AnyUserData| -> mlua::Result<()> {
                let id = shader_id(&asset)?;
                this.inner
                    .lock()
                    .unwrap()
                    .audio_attached
                    .retain(|e| e.id != id);
                Ok(())
            },
        );
    }
}

pub fn new_effect_volume(image: Option<AnyUserData>) -> mlua::Result<EffectVolumeHandle> {
    let texture = match image {
        Some(ud) => texture_from_value(&Value::UserData(ud))?,
        None => None,
    };
    let id = NEXT_EFFECT_ID.fetch_add(1, Ordering::Relaxed);
    let state = Arc::new(Mutex::new(EffectVolumeState {
        id,
        alive: true,
        enabled: true,
        cframe: CFrame::new(Vector::new(0.0, 0.0, 0.0), Vector::new(0.0, 0.0, 0.0)),
        size: Vector::new(1.0, 1.0, 1.0),
        texture,
        rate: 20.0,
        lifetime: Range::new(1.0, 2.0),
        speed: Range::new(2.0, 4.0),
        acceleration: Vector::new(0.0, 0.0, 0.0),
        randomize_force_x: Range::new(0.0, 0.0),
        randomize_force_y: Range::new(0.0, 0.0),
        randomize_force_z: Range::new(0.0, 0.0),
        drag: 0.0,
        spread: 0.0,
        emission_direction: Vector::new(0.0, 1.0, 0.0),
        rotation_init: Range::new(0.0, 0.0),
        rot_speed: Range::new(0.0, 0.0),
        size_init: Range::new(0.5, 0.5),
        color: ColorSequence::constant(Color3::new(1.0, 1.0, 1.0)),
        size_over_life: NumberSequence::constant(1.0),
        transparency: NumberSequence::constant(0.0),
        max_particles: 2048,
        particles: Vec::new(),
        spawn_accumulator: 0.0,
        rng_state: 0x9E3779B97F4A7C15u64.wrapping_add(id as u64 * 2654435761),
        face_camera: true,
        attached: Vec::new(),
        audio_attached: Vec::new(),
    }));
    EFFECT_VOLUMES.with(|c| c.borrow_mut().push(state.clone()));
    Ok(EffectVolumeHandle { inner: state })
}
