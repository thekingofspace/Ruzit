use std::cell::RefCell;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use mlua::{AnyUserData, UserData, UserDataFields, UserDataMethods, Value};

use crate::libs::asset::ImageAsset;
use crate::libs::primitives::{Color3, Dim};
use crate::libs::renderable::effect_volume::{
    color_sequence_from_value, color_sequence_to_table, number_sequence_from_value,
    number_sequence_to_table, range_from_value, range_to_table, ColorSequence, NumberSequence,
    Range,
};
use crate::libs::renderable::PartTextureRef;

use super::{build_attached, shader_asset_id, AttachedShader};

static NEXT_UI_EFFECT_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Copy, Debug)]
pub struct Vec2 {
    pub x: f32,
    pub y: f32,
}

impl Vec2 {
    pub fn new(x: f32, y: f32) -> Self {
        Self { x, y }
    }
    pub fn zero() -> Self {
        Self { x: 0.0, y: 0.0 }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct UIParticle {
    pub position: Vec2,
    pub velocity: Vec2,
    pub age: f32,
    pub lifetime: f32,
    pub rotation: f32,
    pub rot_speed: f32,
    pub seed_size: f32,
}

pub struct UIEffectVolumeState {
    pub id: u64,
    pub alive: bool,
    pub enabled: bool,

    pub position: Dim,
    pub size: Dim,
    pub texture: Option<PartTextureRef>,
    pub z_index: i32,

    pub rate: f32,
    pub lifetime: Range,
    pub speed: Range,
    pub acceleration: Vec2,
    pub drag: f32,
    pub spread: f32,
    pub emission_direction: Vec2,
    pub rotation_init: Range,
    pub rot_speed: Range,
    pub size_init: Range,

    pub color: ColorSequence,
    pub size_over_life: NumberSequence,
    pub transparency: NumberSequence,

    pub max_particles: usize,
    pub particles: Vec<UIParticle>,
    pub spawn_accumulator: f32,
    pub rng_state: u64,

    pub last_resolved_origin: Vec2,
    pub last_resolved_extent: Vec2,

    pub attached: Vec<AttachedShader>,
}

thread_local! {
    static UI_EFFECT_VOLUMES: RefCell<Vec<Arc<Mutex<UIEffectVolumeState>>>> =
        const { RefCell::new(Vec::new()) };
}

fn lcg(state: &mut u64) -> u32 {
    *state = state
        .wrapping_mul(6364136223846793005)
        .wrapping_add(1442695040888963407);
    (*state >> 33) as u32
}

fn rand_unit(state: &mut u64) -> f32 {
    (lcg(state) as f32) / (u32::MAX as f32)
}

fn rand_signed(state: &mut u64) -> f32 {
    rand_unit(state) * 2.0 - 1.0
}

fn rand_range(state: &mut u64, r: Range) -> f32 {
    r.min + (r.max - r.min) * rand_unit(state)
}

fn normalize_or(v: Vec2, fallback: Vec2) -> Vec2 {
    let len = (v.x * v.x + v.y * v.y).sqrt();
    if len > 1e-6 {
        Vec2::new(v.x / len, v.y / len)
    } else {
        fallback
    }
}

fn cone_sample_2d(state: &mut u64, axis: Vec2, half_angle_rad: f32) -> Vec2 {
    let base_angle = axis.y.atan2(axis.x);
    let delta = (rand_unit(state) * 2.0 - 1.0) * half_angle_rad;
    let a = base_angle + delta;
    Vec2::new(a.cos(), a.sin())
}

fn spawn_one(s: &mut UIEffectVolumeState) {
    if s.particles.len() >= s.max_particles {
        return;
    }

    let origin = s.last_resolved_origin;
    let extent = s.last_resolved_extent;
    let local = Vec2::new(
        rand_signed(&mut s.rng_state) * 0.5 * extent.x,
        rand_signed(&mut s.rng_state) * 0.5 * extent.y,
    );
    let position = Vec2::new(origin.x + local.x, origin.y + local.y);

    let dir = normalize_or(s.emission_direction, Vec2::new(0.0, -1.0));
    let spread_rad = s.spread.to_radians().clamp(0.0, std::f32::consts::PI);
    let dir = if spread_rad <= 1e-4 {
        dir
    } else {
        cone_sample_2d(&mut s.rng_state, dir, spread_rad * 0.5)
    };

    let speed = rand_range(&mut s.rng_state, s.speed);
    let velocity = Vec2::new(dir.x * speed, dir.y * speed);
    let lifetime = rand_range(&mut s.rng_state, s.lifetime).max(0.01);
    let rotation = rand_range(&mut s.rng_state, s.rotation_init);
    let rot_speed = rand_range(&mut s.rng_state, s.rot_speed);
    let size = rand_range(&mut s.rng_state, s.size_init).max(0.0);

    s.particles.push(UIParticle {
        position,
        velocity,
        age: 0.0,
        lifetime,
        rotation,
        rot_speed,
        seed_size: size,
    });
}

pub fn tick_ui_effect_volumes(dt: f32) {
    UI_EFFECT_VOLUMES.with(|c| {
        let mut reg = c.borrow_mut();
        reg.retain(|v| v.lock().unwrap().alive);
        for arc in reg.iter() {
            let mut s = arc.lock().unwrap();

            s.last_resolved_origin = Vec2::new(s.position.x, s.position.y);
            s.last_resolved_extent = Vec2::new(s.size.x, s.size.y);

            let drag = s.drag.max(0.0);
            let accel = s.acceleration;
            for p in s.particles.iter_mut() {
                p.age += dt;
                let attenuate = (1.0 - drag * dt).clamp(0.0, 1.0);
                p.velocity.x = p.velocity.x * attenuate + accel.x * dt;
                p.velocity.y = p.velocity.y * attenuate + accel.y * dt;
                p.position.x += p.velocity.x * dt;
                p.position.y += p.velocity.y * dt;
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
pub struct UIParticleRender {
    pub position: Vec2,
    pub color: Color3,
    pub alpha: f32,
    pub size: f32,
    pub rotation: f32,
    pub life_t: f32,
}

#[derive(Clone)]
pub struct UIEffectVolumeRender {
    pub id: u64,
    pub texture: Option<PartTextureRef>,
    pub active_shader: Option<AttachedShader>,
    pub z_index: i32,
    pub particles: Vec<UIParticleRender>,
}

pub fn ui_effect_volume_snapshot() -> Vec<UIEffectVolumeRender> {
    UI_EFFECT_VOLUMES.with(|c| {
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
                        UIParticleRender {
                            position: p.position,
                            color,
                            alpha,
                            size: p.seed_size * size_mul,
                            rotation: p.rotation,
                            life_t: t,
                        }
                    })
                    .collect();
                Some(UIEffectVolumeRender {
                    id: s.id,
                    texture: s.texture.clone(),
                    active_shader: s.attached.last().cloned(),
                    z_index: s.z_index,
                    particles,
                })
            })
            .collect()
    })
}

pub struct UIEffectVolumeHandle {
    pub inner: Arc<Mutex<UIEffectVolumeState>>,
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
                    "UIEffectVolume.Image expects an ImageAsset, DynImg, DrawableImg, or nil"
                        .into(),
                ))
            }
        }
        _ => Err(mlua::Error::RuntimeError(
            "UIEffectVolume.Image expects an ImageAsset, DynImg, DrawableImg, or nil".into(),
        )),
    }
}

fn vec2_from_value(v: Value, name: &str) -> mlua::Result<Vec2> {
    match v {
        Value::UserData(ud) => {
            if let Ok(d) = ud.borrow::<Dim>() {
                return Ok(Vec2::new(d.x, d.y));
            }
            Err(mlua::Error::RuntimeError(format!(
                "{name} expects a Dim or table {{ X, Y }}"
            )))
        }
        Value::Table(t) => {
            let x: f32 = t.get(1).or_else(|_| t.get("X")).or_else(|_| t.get("x"))?;
            let y: f32 = t.get(2).or_else(|_| t.get("Y")).or_else(|_| t.get("y"))?;
            Ok(Vec2::new(x, y))
        }
        _ => Err(mlua::Error::RuntimeError(format!(
            "{name} expects a Dim or table {{ X, Y }}"
        ))),
    }
}

fn vec2_to_table(lua: &mlua::Lua, v: Vec2) -> mlua::Result<mlua::Table> {
    let t = lua.create_table()?;
    t.set("X", v.x)?;
    t.set("Y", v.y)?;
    Ok(t)
}

impl UserData for UIEffectVolumeHandle {
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

        f.add_field_method_get("Position", |_, this| Ok(this.inner.lock().unwrap().position));
        f.add_field_method_set("Position", |_, this, ud: AnyUserData| {
            let d = *ud.borrow::<Dim>().map_err(|_| {
                mlua::Error::RuntimeError("UIEffectVolume.Position expects a Dim".into())
            })?;
            this.inner.lock().unwrap().position = d;
            Ok(())
        });

        f.add_field_method_get("Size", |_, this| Ok(this.inner.lock().unwrap().size));
        f.add_field_method_set("Size", |_, this, ud: AnyUserData| {
            let d = *ud.borrow::<Dim>().map_err(|_| {
                mlua::Error::RuntimeError("UIEffectVolume.Size expects a Dim".into())
            })?;
            this.inner.lock().unwrap().size = d;
            Ok(())
        });

        f.add_field_method_get("ZIndex", |_, this| {
            Ok(this.inner.lock().unwrap().z_index)
        });
        f.add_field_method_set("ZIndex", |_, this, v: i32| {
            this.inner.lock().unwrap().z_index = v;
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
            let r = range_from_value(v, "UIEffectVolume.Lifetime")?;
            this.inner.lock().unwrap().lifetime = r;
            Ok(())
        });

        f.add_field_method_get("Speed", |lua, this| {
            range_to_table(lua, this.inner.lock().unwrap().speed)
        });
        f.add_field_method_set("Speed", |_, this, v: Value| {
            let r = range_from_value(v, "UIEffectVolume.Speed")?;
            this.inner.lock().unwrap().speed = r;
            Ok(())
        });

        f.add_field_method_get("Acceleration", |lua, this| {
            vec2_to_table(lua, this.inner.lock().unwrap().acceleration)
        });
        f.add_field_method_set("Acceleration", |_, this, v: Value| {
            let v = vec2_from_value(v, "UIEffectVolume.Acceleration")?;
            this.inner.lock().unwrap().acceleration = v;
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

        f.add_field_method_get("EmissionDirection", |lua, this| {
            vec2_to_table(lua, this.inner.lock().unwrap().emission_direction)
        });
        f.add_field_method_set("EmissionDirection", |_, this, v: Value| {
            let v = vec2_from_value(v, "UIEffectVolume.EmissionDirection")?;
            this.inner.lock().unwrap().emission_direction = v;
            Ok(())
        });

        f.add_field_method_get("Rotation", |lua, this| {
            range_to_table(lua, this.inner.lock().unwrap().rotation_init)
        });
        f.add_field_method_set("Rotation", |_, this, v: Value| {
            let r = range_from_value(v, "UIEffectVolume.Rotation")?;
            this.inner.lock().unwrap().rotation_init = r;
            Ok(())
        });

        f.add_field_method_get("RotSpeed", |lua, this| {
            range_to_table(lua, this.inner.lock().unwrap().rot_speed)
        });
        f.add_field_method_set("RotSpeed", |_, this, v: Value| {
            let r = range_from_value(v, "UIEffectVolume.RotSpeed")?;
            this.inner.lock().unwrap().rot_speed = r;
            Ok(())
        });

        f.add_field_method_get("ParticleSize", |lua, this| {
            range_to_table(lua, this.inner.lock().unwrap().size_init)
        });
        f.add_field_method_set("ParticleSize", |_, this, v: Value| {
            let r = range_from_value(v, "UIEffectVolume.ParticleSize")?;
            this.inner.lock().unwrap().size_init = r;
            Ok(())
        });

        f.add_field_method_get("Color", |lua, this| {
            color_sequence_to_table(lua, &this.inner.lock().unwrap().color)
        });
        f.add_field_method_set("Color", |_, this, v: Value| {
            let seq = color_sequence_from_value(v, "UIEffectVolume.Color")?;
            this.inner.lock().unwrap().color = seq;
            Ok(())
        });

        f.add_field_method_get("SizeOverLife", |lua, this| {
            number_sequence_to_table(lua, &this.inner.lock().unwrap().size_over_life)
        });
        f.add_field_method_set("SizeOverLife", |_, this, v: Value| {
            let seq = number_sequence_from_value(v, "UIEffectVolume.SizeOverLife")?;
            this.inner.lock().unwrap().size_over_life = seq;
            Ok(())
        });

        f.add_field_method_get("Transparency", |lua, this| {
            number_sequence_to_table(lua, &this.inner.lock().unwrap().transparency)
        });
        f.add_field_method_set("Transparency", |_, this, v: Value| {
            let seq = number_sequence_from_value(v, "UIEffectVolume.Transparency")?;
            this.inner.lock().unwrap().transparency = seq;
            Ok(())
        });

        f.add_field_method_get("TimeScaleColor", |lua, this| {
            color_sequence_to_table(lua, &this.inner.lock().unwrap().color)
        });
        f.add_field_method_set("TimeScaleColor", |_, this, v: Value| {
            let seq = color_sequence_from_value(v, "UIEffectVolume.TimeScaleColor")?;
            this.inner.lock().unwrap().color = seq;
            Ok(())
        });

        f.add_field_method_get("TimeScaleTransparency", |lua, this| {
            number_sequence_to_table(lua, &this.inner.lock().unwrap().transparency)
        });
        f.add_field_method_set("TimeScaleTransparency", |_, this, v: Value| {
            let seq = number_sequence_from_value(v, "UIEffectVolume.TimeScaleTransparency")?;
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
            Ok(())
        });

        m.add_method(
            "AttachShader",
            |_, this, asset: AnyUserData| -> mlua::Result<()> {
                let attached = build_attached(&asset)?;
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
                let id = shader_asset_id(&asset)?;
                this.inner.lock().unwrap().attached.retain(|e| e.id != id);
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
                let id = shader_asset_id(&asset)?;
                let s = this.inner.lock().unwrap();
                let entry = s.attached.iter().find(|e| e.id == id).ok_or_else(|| {
                    mlua::Error::RuntimeError(
                        "SetData: shader is not attached to this UIEffectVolume".into(),
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
                let id = shader_asset_id(&asset)?;
                let s = this.inner.lock().unwrap();
                let entry = s.attached.iter().find(|e| e.id == id).ok_or_else(|| {
                    mlua::Error::RuntimeError(
                        "GetData: shader is not attached to this UIEffectVolume".into(),
                    )
                })?;
                let Some(slot) = entry.slot_of_name.get(&name) else {
                    return Ok(None);
                };
                Ok(Some(entry.params.lock().unwrap()[*slot as usize]))
            },
        );
    }
}

pub fn new_ui_effect_volume(image: Option<AnyUserData>) -> mlua::Result<UIEffectVolumeHandle> {
    let texture = match image {
        Some(ud) => texture_from_value(&Value::UserData(ud))?,
        None => None,
    };
    let id = NEXT_UI_EFFECT_ID.fetch_add(1, Ordering::Relaxed);
    let state = Arc::new(Mutex::new(UIEffectVolumeState {
        id,
        alive: true,
        enabled: true,
        position: Dim::new(0.5, 0.0),
        size: Dim::new(0.0, 100.0),
        texture,
        z_index: 0,
        rate: 20.0,
        lifetime: Range::new(1.0, 2.0),
        speed: Range::new(80.0, 160.0),
        acceleration: Vec2::new(0.0, 200.0),
        drag: 0.0,
        spread: 0.0,
        emission_direction: Vec2::new(0.0, -1.0),
        rotation_init: Range::new(0.0, 0.0),
        rot_speed: Range::new(0.0, 0.0),
        size_init: Range::new(16.0, 16.0),
        color: ColorSequence::constant(Color3::new(1.0, 1.0, 1.0)),
        size_over_life: NumberSequence::constant(1.0),
        transparency: NumberSequence::constant(0.0),
        max_particles: 1024,
        particles: Vec::new(),
        spawn_accumulator: 0.0,
        rng_state: 0x9E3779B97F4A7C15u64.wrapping_add(id * 2654435761),
        last_resolved_origin: Vec2::zero(),
        last_resolved_extent: Vec2::zero(),
        attached: Vec::new(),
    }));
    UI_EFFECT_VOLUMES.with(|c| c.borrow_mut().push(state.clone()));
    Ok(UIEffectVolumeHandle { inner: state })
}
