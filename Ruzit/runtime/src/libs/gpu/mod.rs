use std::cell::RefCell;
use std::sync::atomic::{AtomicU64, Ordering as AtomicOrdering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

mod spatial;

use mlua::{AnyUserData, Lua, MultiValue, Table, UserData, UserDataMethods, Value};

use crate::libs::gui::render::{GPU_DEVICE, GPU_META, GPU_QUEUE, GpuMeta};
use crate::libs::primitives::{CFrame, Vector};
use crate::libs::renderable::{self, PartHandle, PartShape, PartState};

const DEFAULT_MAX_DISTANCE: f32 = 1.0e6;

thread_local! {
    static FRAME_STATS: RefCell<FrameTracker> = RefCell::new(FrameTracker::new());
}

static ACTIVE_STORAGE: Mutex<Option<Arc<wgpu::Buffer>>> = Mutex::new(None);
static STORAGE_VERSION: AtomicU64 = AtomicU64::new(0);

pub fn current_storage() -> Option<Arc<wgpu::Buffer>> {
    ACTIVE_STORAGE.lock().unwrap().clone()
}
pub fn current_storage_version() -> u64 {
    STORAGE_VERSION.load(AtomicOrdering::SeqCst)
}

pub struct ShadowConfig {
    pub enabled: bool,
    pub quality: u32,
    pub distance: f32,
    pub bias: f32,
    pub pcf: u32,
}

static SHADOW_CONFIG: Mutex<ShadowConfig> = Mutex::new(ShadowConfig {
    enabled: false,
    quality: 1024,
    distance: 80.0,
    bias: 0.0015,
    pcf: 1,
});
static SHADOW_VERSION: AtomicU64 = AtomicU64::new(0);

pub fn shadow_config() -> ShadowConfig {
    let c = SHADOW_CONFIG.lock().unwrap();
    ShadowConfig {
        enabled: c.enabled,
        quality: c.quality,
        distance: c.distance,
        bias: c.bias,
        pcf: c.pcf,
    }
}
pub fn shadow_config_version() -> u64 {
    SHADOW_VERSION.load(AtomicOrdering::SeqCst)
}

fn bump_shadow_version() {
    SHADOW_VERSION.fetch_add(1, AtomicOrdering::SeqCst);
}

struct FrameTracker {
    last_tick: Option<Instant>,
    smoothed_dt: f32,
    frame_count: u64,
    started_at: Instant,
}

impl FrameTracker {
    fn new() -> Self {
        Self {
            last_tick: None,
            smoothed_dt: 1.0 / 60.0,
            frame_count: 0,
            started_at: Instant::now(),
        }
    }
}

pub fn record_frame() {
    FRAME_STATS.with(|cell| {
        let mut t = cell.borrow_mut();
        let now = Instant::now();
        if let Some(prev) = t.last_tick {
            let dt = (now - prev).as_secs_f32().max(0.0001);
            t.smoothed_dt = t.smoothed_dt * 0.9 + dt * 0.1;
        }
        t.last_tick = Some(now);
        t.frame_count = t.frame_count.saturating_add(1);
    });
}

pub fn create(lua: &Lua) -> mlua::Result<Table> {
    let t = lua.create_table()?;
    t.set("Info", lua.create_function(info)?)?;
    t.set("Limits", lua.create_function(limits)?)?;
    t.set("FrameStats", lua.create_function(frame_stats)?)?;
    t.set("Raycast", lua.create_function(raycast)?)?;
    t.set("ScreenToRay", lua.create_function(screen_to_ray)?)?;
    t.set("WorldToScreen", lua.create_function(world_to_screen)?)?;
    t.set("NewBuffer", lua.create_function(new_buffer)?)?;
    t.set("SetBuffer", lua.create_function(set_buffer)?)?;
    t.set("ClearBuffer", lua.create_function(clear_buffer)?)?;
    t.set("GetBounds", lua.create_function(get_bounds)?)?;
    t.set("CheckArea", lua.create_function(check_area)?)?;
    t.set("OverlapSphere", lua.create_function(overlap_sphere)?)?;
    t.set("OverlapBox", lua.create_function(overlap_box)?)?;
    t.set("OverlapFrustum", lua.create_function(overlap_frustum)?)?;
    t.set("GetItemsInZone", lua.create_function(get_items_in_zone)?)?;
    t.set(
        "SetShadowsEnabled",
        lua.create_function(|_, on: bool| -> mlua::Result<()> {
            SHADOW_CONFIG.lock().unwrap().enabled = on;
            bump_shadow_version();
            Ok(())
        })?,
    )?;
    t.set(
        "GetShadowsEnabled",
        lua.create_function(|_, _: ()| -> mlua::Result<bool> {
            Ok(SHADOW_CONFIG.lock().unwrap().enabled)
        })?,
    )?;
    t.set(
        "SetShadowMapQuality",
        lua.create_function(|_, size: i64| -> mlua::Result<()> {
            let clamped = size.clamp(64, 8192) as u32;
            let pow2 = clamped.next_power_of_two();
            SHADOW_CONFIG.lock().unwrap().quality = pow2;
            bump_shadow_version();
            Ok(())
        })?,
    )?;
    t.set(
        "GetShadowMapQuality",
        lua.create_function(|_, _: ()| -> mlua::Result<i64> {
            Ok(SHADOW_CONFIG.lock().unwrap().quality as i64)
        })?,
    )?;
    t.set(
        "SetShadowDistance",
        lua.create_function(|_, d: f32| -> mlua::Result<()> {
            SHADOW_CONFIG.lock().unwrap().distance = d.max(0.0);
            bump_shadow_version();
            Ok(())
        })?,
    )?;
    t.set(
        "GetShadowDistance",
        lua.create_function(|_, _: ()| -> mlua::Result<f32> {
            Ok(SHADOW_CONFIG.lock().unwrap().distance)
        })?,
    )?;
    t.set(
        "SetShadowBias",
        lua.create_function(|_, b: f32| -> mlua::Result<()> {
            SHADOW_CONFIG.lock().unwrap().bias = b.max(0.0);
            bump_shadow_version();
            Ok(())
        })?,
    )?;
    t.set(
        "GetShadowBias",
        lua.create_function(|_, _: ()| -> mlua::Result<f32> {
            Ok(SHADOW_CONFIG.lock().unwrap().bias)
        })?,
    )?;
    t.set(
        "SetShadowPCF",
        lua.create_function(|_, taps: i64| -> mlua::Result<()> {
            let v = taps.clamp(1, 9) as u32;
            SHADOW_CONFIG.lock().unwrap().pcf = v;
            bump_shadow_version();
            Ok(())
        })?,
    )?;
    t.set(
        "GetShadowPCF",
        lua.create_function(|_, _: ()| -> mlua::Result<i64> {
            Ok(SHADOW_CONFIG.lock().unwrap().pcf as i64)
        })?,
    )?;
    t.set(
        "SetMaxFrameRate",
        lua.create_function(|_, fps: Option<f64>| -> mlua::Result<()> {
            let cap = match fps {
                None => None,
                Some(f) if f <= 0.0 => None,
                Some(f) => Some(f.round().max(1.0) as u32),
            };
            crate::heart::set_max_fps(cap);
            Ok(())
        })?,
    )?;
    t.set(
        "GetMaxFrameRate",
        lua.create_function(|_, _: ()| -> mlua::Result<Value> {
            match crate::heart::get_max_fps() {
                Some(fps) => Ok(Value::Number(fps as f64)),
                None => Ok(Value::Nil),
            }
        })?,
    )?;
    t.set(
        "SetVSync",
        lua.create_function(|_, on: bool| -> mlua::Result<()> {
            crate::libs::gui::render::set_vsync(on);
            Ok(())
        })?,
    )?;
    t.set(
        "GetVSync",
        lua.create_function(|_, _: ()| -> mlua::Result<bool> {
            Ok(crate::libs::gui::render::get_vsync())
        })?,
    )?;
    t.set(
        "SetPowerMode",
        lua.create_function(|_, mode: String| -> mlua::Result<()> {
            match mode.as_str() {
                "Quality" | "quality" | "Q" | "max" | "Max" => {
                    crate::libs::gui::render::set_power_mode_quality();
                }
                "Performance" | "performance" | "P" | "battery" | "Battery" => {
                    crate::libs::gui::render::set_power_mode_performance();
                }
                "Auto" | "auto" | "default" | "Default" => {
                    crate::libs::gui::render::set_power_mode_auto();
                }
                other => {
                    return Err(mlua::Error::RuntimeError(format!(
                        "GPU.SetPowerMode: unknown mode '{other}' (try 'Quality', 'Performance', 'Auto')"
                    )));
                }
            }
            Ok(())
        })?,
    )?;
    t.set(
        "GetPowerMode",
        lua.create_function(|_, _: ()| -> mlua::Result<String> {
            Ok(crate::libs::gui::render::power_mode_label().to_string())
        })?,
    )?;
    Ok(t)
}

fn get_bounds(lua: &Lua, args: MultiValue) -> mlua::Result<Table> {
    let mut iter = args.into_iter();
    let part_v = iter.next().ok_or_else(|| {
        mlua::Error::RuntimeError("GPU.GetBounds: missing first argument (BasePart)".into())
    })?;
    let quality = match iter.next() {
        Some(Value::Integer(n)) => n.max(1) as u32,
        Some(Value::Number(n)) => (n as i64).max(1) as u32,
        _ => 1,
    };

    let state = part_state_from_value(&part_v)?;
    let s = state.lock().unwrap();
    if !s.alive {
        return Err(mlua::Error::RuntimeError(
            "GPU.GetBounds: BasePart has been destroyed".into(),
        ));
    }
    let aabbs = part_world_aabbs(&s, quality);
    drop(s);

    let out = lua.create_table()?;
    for (i, (min, max)) in aabbs.iter().enumerate() {
        let entry = lua.create_table()?;
        let center = Vector::new(
            (min.x + max.x) * 0.5,
            (min.y + max.y) * 0.5,
            (min.z + max.z) * 0.5,
        );
        let extents = Vector::new(max.x - min.x, max.y - min.y, max.z - min.z);
        entry.set("Position", center)?;
        entry.set("Size", extents)?;
        entry.set("Min", *min)?;
        entry.set("Max", *max)?;
        out.set(i + 1, entry)?;
    }
    Ok(out)
}

fn check_area(lua: &Lua, args: MultiValue) -> mlua::Result<Table> {
    let mut iter = args.into_iter();
    let center_v = iter.next().ok_or_else(|| {
        mlua::Error::RuntimeError("GPU.CheckArea: missing first argument (center Vector)".into())
    })?;
    let size_v = iter.next().ok_or_else(|| {
        mlua::Error::RuntimeError("GPU.CheckArea: missing second argument (size Vector)".into())
    })?;
    let quality = match iter.next() {
        Some(Value::Integer(n)) => n.max(1) as u32,
        Some(Value::Number(n)) => (n as i64).max(1) as u32,
        _ => 1,
    };

    let center = vector_from_value(&center_v, "GPU.CheckArea: center")?;
    let size = vector_from_value(&size_v, "GPU.CheckArea: size")?;
    let half = Vector::new(size.x * 0.5, size.y * 0.5, size.z * 0.5);
    let q_min = Vector::new(center.x - half.x, center.y - half.y, center.z - half.z);
    let q_max = Vector::new(center.x + half.x, center.y + half.y, center.z + half.z);

    let out = lua.create_table()?;
    let mut idx = 1;
    for state_arc in renderable::list_part_states() {
        let alive = {
            let s = state_arc.lock().unwrap();
            s.alive && s.render
        };
        if !alive {
            continue;
        }
        let aabbs = {
            let s = state_arc.lock().unwrap();
            part_world_aabbs(&s, quality)
        };
        let intersects = aabbs.iter().any(|(min, max)| {
            !(max.x < q_min.x
                || min.x > q_max.x
                || max.y < q_min.y
                || min.y > q_max.y
                || max.z < q_min.z
                || min.z > q_max.z)
        });
        if intersects {
            let part = lua.create_userdata(PartHandle::from_state(state_arc))?;
            out.set(idx, part)?;
            idx += 1;
        }
    }
    Ok(out)
}

fn collect_overlap_results(
    lua: &Lua,
    parts: Vec<Arc<Mutex<PartState>>>,
    filter: Option<mlua::Function>,
) -> mlua::Result<Table> {
    let out = lua.create_table()?;
    let mut idx = 1;
    for state_arc in parts {
        let part = lua.create_userdata(PartHandle::from_state(state_arc))?;
        let accept = match &filter {
            None => true,
            Some(f) => match f.call::<Value>(part.clone())? {
                Value::Boolean(b) => b,
                Value::Nil => false,
                _ => true,
            },
        };
        if accept {
            out.set(idx, part)?;
            idx += 1;
        }
    }
    Ok(out)
}

fn overlap_sphere(lua: &Lua, args: MultiValue) -> mlua::Result<Table> {
    let mut iter = args.into_iter();
    let center_v = iter.next().ok_or_else(|| {
        mlua::Error::RuntimeError("GPU.OverlapSphere: missing center (Vector)".into())
    })?;
    let radius_v = iter.next().ok_or_else(|| {
        mlua::Error::RuntimeError("GPU.OverlapSphere: missing radius (number)".into())
    })?;
    let filter = match iter.next() {
        None | Some(Value::Nil) => None,
        Some(Value::Function(f)) => Some(f),
        Some(other) => {
            return Err(mlua::Error::RuntimeError(format!(
                "GPU.OverlapSphere: filter must be a function or nil (got {})",
                other.type_name()
            )));
        }
    };

    let center = vector_from_value(&center_v, "GPU.OverlapSphere: center")?;
    let radius = match radius_v {
        Value::Integer(n) => n as f32,
        Value::Number(n) => n as f32,
        _ => {
            return Err(mlua::Error::RuntimeError(
                "GPU.OverlapSphere: radius must be a number".into(),
            ));
        }
    };
    if radius <= 0.0 {
        return Err(mlua::Error::RuntimeError(
            "GPU.OverlapSphere: radius must be > 0".into(),
        ));
    }

    let parts = spatial::gpu_overlap(spatial::OverlapShape::Sphere { center, radius })
        .ok_or_else(|| {
            mlua::Error::RuntimeError(
                "GPU.OverlapSphere: GPU not initialized (open a window first)".into(),
            )
        })?;
    collect_overlap_results(lua, parts, filter)
}

fn overlap_box(lua: &Lua, args: MultiValue) -> mlua::Result<Table> {
    let mut iter = args.into_iter();
    let center_v = iter.next().ok_or_else(|| {
        mlua::Error::RuntimeError("GPU.OverlapBox: missing center (Vector)".into())
    })?;
    let size_v = iter.next().ok_or_else(|| {
        mlua::Error::RuntimeError("GPU.OverlapBox: missing size (Vector)".into())
    })?;
    let rot_v = iter.next().unwrap_or(Value::Nil);
    let filter = match iter.next() {
        None | Some(Value::Nil) => None,
        Some(Value::Function(f)) => Some(f),
        Some(other) => {
            return Err(mlua::Error::RuntimeError(format!(
                "GPU.OverlapBox: filter must be a function or nil (got {})",
                other.type_name()
            )));
        }
    };

    let center = vector_from_value(&center_v, "GPU.OverlapBox: center")?;
    let size = vector_from_value(&size_v, "GPU.OverlapBox: size")?;
    let rotation = match rot_v {
        Value::Nil => Vector::new(0.0, 0.0, 0.0),
        v => vector_from_value(&v, "GPU.OverlapBox: rotation")?,
    };

    let parts = spatial::gpu_overlap(spatial::OverlapShape::Box {
        center,
        size,
        rotation,
    })
    .ok_or_else(|| {
        mlua::Error::RuntimeError(
            "GPU.OverlapBox: GPU not initialized (open a window first)".into(),
        )
    })?;
    collect_overlap_results(lua, parts, filter)
}

fn overlap_frustum(lua: &Lua, args: MultiValue) -> mlua::Result<Table> {
    let mut iter = args.into_iter();
    let filter = match iter.next() {
        None | Some(Value::Nil) => None,
        Some(Value::Function(f)) => Some(f),
        Some(other) => {
            return Err(mlua::Error::RuntimeError(format!(
                "GPU.OverlapFrustum: filter must be a function or nil (got {})",
                other.type_name()
            )));
        }
    };

    let cam = renderable::camera_snapshot();
    let (w, h) = window_size();
    let aspect = if h > 0 { w as f32 / h as f32 } else { 1.0 };
    let planes = spatial::frustum_planes_from_camera(
        cam.cframe.position,
        cam.cframe.rotation,
        cam.fov_deg,
        aspect,
        cam.near,
        cam.far,
    );

    let parts = spatial::gpu_overlap(spatial::OverlapShape::Frustum { planes })
        .ok_or_else(|| {
            mlua::Error::RuntimeError(
                "GPU.OverlapFrustum: GPU not initialized (open a window first)".into(),
            )
        })?;
    collect_overlap_results(lua, parts, filter)
}

fn get_items_in_zone(lua: &Lua, args: MultiValue) -> mlua::Result<Table> {
    let mut iter = args.into_iter();
    let cf_v = iter.next().ok_or_else(|| {
        mlua::Error::RuntimeError("GPU.GetItemsInZone: missing first argument (CFrame)".into())
    })?;
    let size_v = iter.next().ok_or_else(|| {
        mlua::Error::RuntimeError("GPU.GetItemsInZone: missing second argument (Vector size)".into())
    })?;
    let filter = match iter.next() {
        None | Some(Value::Nil) => None,
        Some(Value::Function(f)) => Some(f),
        Some(other) => {
            return Err(mlua::Error::RuntimeError(format!(
                "GPU.GetItemsInZone: filter must be a function or nil (got {})",
                other.type_name()
            )));
        }
    };

    let zone_cf = match &cf_v {
        Value::UserData(ud) => *ud.borrow::<CFrame>().map_err(|_| {
            mlua::Error::RuntimeError("GPU.GetItemsInZone: first argument must be a CFrame".into())
        })?,
        _ => {
            return Err(mlua::Error::RuntimeError(
                "GPU.GetItemsInZone: first argument must be a CFrame".into(),
            ));
        }
    };
    let zone_size = vector_from_value(&size_v, "GPU.GetItemsInZone: size")?;
    if zone_size.x <= 0.0 || zone_size.y <= 0.0 || zone_size.z <= 0.0 {
        return Err(mlua::Error::RuntimeError(
            "GPU.GetItemsInZone: size components must be > 0".into(),
        ));
    }

    let parts = spatial::gpu_zone_query(zone_cf, zone_size).ok_or_else(|| {
        mlua::Error::RuntimeError(
            "GPU.GetItemsInZone: GPU not initialized (open a window first)".into(),
        )
    })?;
    collect_overlap_results(lua, parts, filter)
}

fn part_state_from_value(v: &Value) -> mlua::Result<Arc<Mutex<PartState>>> {
    if let Value::UserData(ud) = v {
        if let Ok(handle) = ud.borrow::<PartHandle>() {
            return Ok(handle.state.clone());
        }
    }
    Err(mlua::Error::RuntimeError(
        "expected a BasePart (Renderable.BasePart / Renderable.BaseModel)".into(),
    ))
}

fn part_world_aabbs(state: &PartState, quality: u32) -> Vec<(Vector, Vector)> {
    let cf = state.cframe;
    let size = state.size;

    let model = state.deformed.as_ref().or(state.model.as_ref());
    let use_mesh = matches!(state.shape, PartShape::Model)
        && model.map(|m| !m.vertices.is_empty()).unwrap_or(false);

    if !use_mesh {
        return vec![rotated_box_aabb(cf, size)];
    }

    let m = model.unwrap();
    let rot = euler_to_matrix(cf.rotation);
    let mut world_verts: Vec<Vector> = Vec::with_capacity(m.vertices.len());
    for v in m.vertices.iter() {
        let local = Vector::new(
            v.position[0] * size.x,
            v.position[1] * size.y,
            v.position[2] * size.z,
        );
        let r = mat3_apply(rot, local);
        world_verts.push(Vector::new(
            cf.position.x + r.x,
            cf.position.y + r.y,
            cf.position.z + r.z,
        ));
    }

    if quality <= 1 {
        return vec![aabb_of(&world_verts)];
    }

    let n = (quality as usize).min(world_verts.len()).max(1);
    let initial = aabb_of(&world_verts);
    let ex = (
        initial.1.x - initial.0.x,
        initial.1.y - initial.0.y,
        initial.1.z - initial.0.z,
    );
    let axis = if ex.0 >= ex.1 && ex.0 >= ex.2 {
        0
    } else if ex.1 >= ex.2 {
        1
    } else {
        2
    };

    world_verts.sort_by(|a, b| {
        let av = if axis == 0 {
            a.x
        } else if axis == 1 {
            a.y
        } else {
            a.z
        };
        let bv = if axis == 0 {
            b.x
        } else if axis == 1 {
            b.y
        } else {
            b.z
        };
        av.partial_cmp(&bv).unwrap_or(std::cmp::Ordering::Equal)
    });

    let chunk_size = world_verts.len().div_ceil(n);
    let mut out = Vec::with_capacity(n);
    for chunk in world_verts.chunks(chunk_size) {
        if !chunk.is_empty() {
            out.push(aabb_of(chunk));
        }
    }
    out
}

fn rotated_box_aabb(cf: CFrame, size: Vector) -> (Vector, Vector) {
    let rot = euler_to_matrix(cf.rotation);
    let half = Vector::new(size.x * 0.5, size.y * 0.5, size.z * 0.5);
    let signs = [-1.0_f32, 1.0_f32];
    let mut min = Vector::new(f32::INFINITY, f32::INFINITY, f32::INFINITY);
    let mut max = Vector::new(f32::NEG_INFINITY, f32::NEG_INFINITY, f32::NEG_INFINITY);
    for sx in signs {
        for sy in signs {
            for sz in signs {
                let local = Vector::new(half.x * sx, half.y * sy, half.z * sz);
                let r = mat3_apply(rot, local);
                let world = Vector::new(
                    cf.position.x + r.x,
                    cf.position.y + r.y,
                    cf.position.z + r.z,
                );
                min.x = min.x.min(world.x);
                min.y = min.y.min(world.y);
                min.z = min.z.min(world.z);
                max.x = max.x.max(world.x);
                max.y = max.y.max(world.y);
                max.z = max.z.max(world.z);
            }
        }
    }
    (min, max)
}

fn aabb_of(verts: &[Vector]) -> (Vector, Vector) {
    let mut min = Vector::new(f32::INFINITY, f32::INFINITY, f32::INFINITY);
    let mut max = Vector::new(f32::NEG_INFINITY, f32::NEG_INFINITY, f32::NEG_INFINITY);
    for v in verts {
        min.x = min.x.min(v.x);
        min.y = min.y.min(v.y);
        min.z = min.z.min(v.z);
        max.x = max.x.max(v.x);
        max.y = max.y.max(v.y);
        max.z = max.z.max(v.z);
    }
    (min, max)
}

pub struct GPUBuffer {
    buffer: Arc<wgpu::Buffer>,
    floats: usize,
}

impl GPUBuffer {
    pub fn floats(&self) -> usize {
        self.floats
    }
    pub fn raw(&self) -> &wgpu::Buffer {
        &self.buffer
    }
}

impl UserData for GPUBuffer {
    fn add_methods<M: UserDataMethods<Self>>(m: &mut M) {
        m.add_method("Size", |_, this, _: ()| Ok(this.floats as i64));
        m.add_method(
            "Write",
            |_, this, (offset, values): (i64, Vec<f32>)| -> mlua::Result<()> {
                if offset < 0 {
                    return Err(mlua::Error::RuntimeError(
                        "GPUBuffer:Write: offset must be >= 0".into(),
                    ));
                }
                let off = offset as usize;
                if off + values.len() > this.floats {
                    return Err(mlua::Error::RuntimeError(format!(
                        "GPUBuffer:Write: writing {} floats at offset {} exceeds buffer size {}",
                        values.len(),
                        off,
                        this.floats
                    )));
                }
                let queue = GPU_QUEUE.get().ok_or_else(|| {
                    mlua::Error::RuntimeError(
                        "GPUBuffer:Write: GPU not initialized (open a window first)".into(),
                    )
                })?;
                let bytes: &[u8] = bytemuck::cast_slice(&values);
                queue.write_buffer(&this.buffer, (off as u64) * 4, bytes);
                Ok(())
            },
        );
        m.add_method("Fill", |_, this, value: f32| -> mlua::Result<()> {
            let queue = GPU_QUEUE.get().ok_or_else(|| {
                mlua::Error::RuntimeError(
                    "GPUBuffer:Fill: GPU not initialized (open a window first)".into(),
                )
            })?;
            let v = vec![value; this.floats];
            queue.write_buffer(&this.buffer, 0, bytemuck::cast_slice(&v));
            Ok(())
        });
    }
}

fn new_buffer(_lua: &Lua, size: i64) -> mlua::Result<GPUBuffer> {
    if size <= 0 {
        return Err(mlua::Error::RuntimeError(
            "GPU.NewBuffer: size must be > 0".into(),
        ));
    }
    let device = GPU_DEVICE.get().ok_or_else(|| {
        mlua::Error::RuntimeError("GPU.NewBuffer: GPU not initialized (open a window first)".into())
    })?;
    let bytes = (size as u64) * 4;
    let buffer = Arc::new(device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("Ruzit user storage buffer"),
        size: bytes,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    }));
    Ok(GPUBuffer {
        buffer,
        floats: size as usize,
    })
}

fn set_buffer(_lua: &Lua, buf: AnyUserData) -> mlua::Result<()> {
    let b = buf.borrow::<GPUBuffer>()?;
    *ACTIVE_STORAGE.lock().unwrap() = Some(Arc::clone(&b.buffer));
    STORAGE_VERSION.fetch_add(1, AtomicOrdering::SeqCst);
    Ok(())
}

fn clear_buffer(_lua: &Lua, _: ()) -> mlua::Result<()> {
    *ACTIVE_STORAGE.lock().unwrap() = None;
    STORAGE_VERSION.fetch_add(1, AtomicOrdering::SeqCst);
    Ok(())
}

fn meta_or_default() -> GpuMeta {
    GPU_META.get().cloned().unwrap_or_else(|| GpuMeta {
        name: "(no GPU initialized)".into(),
        vendor: 0,
        device: 0,
        backend: "Unknown".into(),
        driver: String::new(),
        driver_info: String::new(),
        device_type: "Unknown".into(),
        max_texture_size: 0,
        max_buffer_size: 0,
        max_bind_groups: 0,
        max_vertex_buffers: 0,
        max_compute_workgroup_size: [0, 0, 0],
    })
}

fn info(lua: &Lua, _: ()) -> mlua::Result<Table> {
    let m = meta_or_default();
    let t = lua.create_table()?;
    t.set("Name", m.name)?;
    t.set("Vendor", vendor_string(m.vendor))?;
    t.set("VendorID", m.vendor as i64)?;
    t.set("DeviceID", m.device as i64)?;
    t.set("Backend", m.backend)?;
    t.set("Driver", m.driver)?;
    t.set("DriverInfo", m.driver_info)?;
    t.set("DeviceType", m.device_type)?;
    Ok(t)
}

fn limits(lua: &Lua, _: ()) -> mlua::Result<Table> {
    let m = meta_or_default();
    let t = lua.create_table()?;
    t.set("MaxTextureSize", m.max_texture_size as i64)?;
    t.set("MaxBufferSize", m.max_buffer_size as f64)?;
    t.set("MaxBindGroups", m.max_bind_groups as i64)?;
    t.set("MaxVertexBuffers", m.max_vertex_buffers as i64)?;
    let wg = lua.create_table()?;
    wg.set("X", m.max_compute_workgroup_size[0] as i64)?;
    wg.set("Y", m.max_compute_workgroup_size[1] as i64)?;
    wg.set("Z", m.max_compute_workgroup_size[2] as i64)?;
    t.set("MaxComputeWorkgroup", wg)?;
    Ok(t)
}

fn frame_stats(lua: &Lua, _: ()) -> mlua::Result<Table> {
    let (smoothed_dt, frames, uptime) = FRAME_STATS.with(|cell| {
        let s = cell.borrow();
        (
            s.smoothed_dt,
            s.frame_count,
            s.started_at.elapsed().as_secs_f32(),
        )
    });
    let part_count = renderable::list_part_states().len();
    let fps = if smoothed_dt > 0.0 {
        1.0 / smoothed_dt
    } else {
        0.0
    };
    let t = lua.create_table()?;
    t.set("FPS", fps as f64)?;
    t.set("FrameTime", smoothed_dt as f64)?;
    t.set("FrameCount", frames as i64)?;
    t.set("Uptime", uptime as f64)?;
    t.set("PartCount", part_count as i64)?;
    Ok(t)
}

fn vendor_string(id: u32) -> &'static str {
    match id {
        0x10DE => "NVIDIA",
        0x1002 | 0x1022 => "AMD",
        0x8086 => "Intel",
        0x13B5 => "ARM",
        0x5143 => "Qualcomm",
        0x1010 => "Imagination",
        0x106B => "Apple",
        0x1414 => "Microsoft",
        0 => "Unknown",
        _ => "Other",
    }
}

fn raycast(lua: &Lua, args: MultiValue) -> mlua::Result<Value> {
    let mut iter = args.into_iter();
    let origin_v: Value = iter
        .next()
        .ok_or_else(|| mlua::Error::RuntimeError("GPU.Raycast: missing origin (Vector)".into()))?;
    let dir_v: Value = iter.next().ok_or_else(|| {
        mlua::Error::RuntimeError("GPU.Raycast: missing direction (Vector)".into())
    })?;
    let filter_v: Value = iter.next().unwrap_or(Value::Nil);
    let max_dist_v: Value = iter.next().unwrap_or(Value::Nil);

    let origin = vector_from_value(&origin_v, "GPU.Raycast: origin")?;
    let dir_raw = vector_from_value(&dir_v, "GPU.Raycast: direction")?;
    let dir_len = dir_raw.magnitude();
    if dir_len < 1e-6 {
        return Err(mlua::Error::RuntimeError(
            "GPU.Raycast: direction has zero length".into(),
        ));
    }
    let direction = Vector::new(
        dir_raw.x / dir_len,
        dir_raw.y / dir_len,
        dir_raw.z / dir_len,
    );

    let filter = match filter_v {
        Value::Nil => None,
        Value::Function(f) => Some(f),
        other => {
            return Err(mlua::Error::RuntimeError(format!(
                "GPU.Raycast: filter must be a function or nil (got {})",
                other.type_name()
            )));
        }
    };

    let max_dist = match max_dist_v {
        Value::Nil => DEFAULT_MAX_DISTANCE,
        Value::Integer(n) => n as f32,
        Value::Number(n) => n as f32,
        other => {
            return Err(mlua::Error::RuntimeError(format!(
                "GPU.Raycast: maxDistance must be a number or nil (got {})",
                other.type_name()
            )));
        }
    };

    let mut hits: Vec<Hit> = Vec::new();
    if let Some(gpu_hits) = spatial::gpu_raycast(origin, direction, max_dist) {
        for h in gpu_hits {
            let ignore = h.state.lock().map(|s| s.ignore_raycast).unwrap_or(true);
            if ignore {
                continue;
            }
            hits.push(Hit {
                state: h.state,
                distance: h.distance,
                position: h.position,
                normal: h.normal,
            });
        }
    } else {
        for state_arc in renderable::list_part_states() {
            let snap = {
                let s = state_arc.lock().unwrap();
                if !s.alive || !s.render || s.ignore_raycast {
                    continue;
                }
                (s.shape, s.cframe, s.size)
            };
            if let Some((dist, world_pos, world_normal)) =
                intersect_part(snap.0, snap.1, snap.2, origin, direction)
            {
                if dist > max_dist {
                    continue;
                }
                hits.push(Hit {
                    state: state_arc,
                    distance: dist,
                    position: world_pos,
                    normal: world_normal,
                });
            }
        }

        hits.sort_by(|a, b| {
            a.distance
                .partial_cmp(&b.distance)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
    }

    for hit in hits {
        let part = lua.create_userdata(PartHandle::from_state(hit.state))?;
        let accept = match &filter {
            None => true,
            Some(f) => match f.call::<Value>(part.clone())? {
                Value::Boolean(b) => b,
                Value::Nil => false,
                _ => true,
            },
        };
        if accept {
            let t = lua.create_table()?;
            t.set("Part", part)?;
            t.set("Position", hit.position)?;
            t.set("Distance", hit.distance as f64)?;
            t.set("Normal", hit.normal)?;
            return Ok(Value::Table(t));
        }
    }

    Ok(Value::Nil)
}

struct Hit {
    state: std::sync::Arc<std::sync::Mutex<PartState>>,
    distance: f32,
    position: Vector,
    normal: Vector,
}

fn vector_from_value(v: &Value, label: &str) -> mlua::Result<Vector> {
    crate::libs::primitives::value_to_vector_opt(v).ok_or_else(|| {
        mlua::Error::RuntimeError(format!("{label} must be a Vector"))
    })
}

fn screen_to_ray(lua: &Lua, point: AnyUserData) -> mlua::Result<MultiValue> {
    let p = point.borrow::<crate::libs::primitives::Dim>()?;
    let cam = renderable::camera_snapshot();
    let (w, h) = window_size();
    let aspect = if h > 0 { w as f32 / h as f32 } else { 1.0 };
    let fov_rad = cam.fov_deg.to_radians();
    let tan_half = (fov_rad * 0.5).tan();

    let nx = if w > 0 {
        (p.x / w as f32) * 2.0 - 1.0
    } else {
        0.0
    };
    let ny = if h > 0 {
        1.0 - (p.y / h as f32) * 2.0
    } else {
        0.0
    };

    let local = Vector::new(nx * aspect * tan_half, ny * tan_half, -1.0);
    let len = local.magnitude().max(1e-6);
    let local = Vector::new(local.x / len, local.y / len, local.z / len);

    let rot = euler_to_matrix(cam.cframe.rotation);
    let world = mat3_apply(rot, local);

    let mut out = MultiValue::new();
    out.push_back(Value::UserData(lua.create_userdata(cam.cframe.position)?));
    out.push_back(Value::UserData(lua.create_userdata(world)?));
    Ok(out)
}

fn world_to_screen(lua: &Lua, world: Vector) -> mlua::Result<MultiValue> {
    let world_pos = world;
    let cam = renderable::camera_snapshot();
    let (w, h) = window_size();
    let aspect = if h > 0 { w as f32 / h as f32 } else { 1.0 };

    let rot = euler_to_matrix(cam.cframe.rotation);
    let rel = Vector::new(
        world_pos.x - cam.cframe.position.x,
        world_pos.y - cam.cframe.position.y,
        world_pos.z - cam.cframe.position.z,
    );
    let view = mat3_apply(transpose(rot), rel);

    let mut out = MultiValue::new();
    if view.z >= -cam.near {
        out.push_back(Value::Nil);
        out.push_back(Value::Boolean(true));
        return Ok(out);
    }
    let fov_rad = cam.fov_deg.to_radians();
    let tan_half = (fov_rad * 0.5).tan();
    let nx = view.x / (-view.z * tan_half * aspect);
    let ny = view.y / (-view.z * tan_half);
    let sx = (nx + 1.0) * 0.5 * w as f32;
    let sy = (1.0 - ny) * 0.5 * h as f32;
    let dim = crate::libs::primitives::Dim::new(sx, sy);
    out.push_back(Value::UserData(lua.create_userdata(dim)?));
    out.push_back(Value::Boolean(false));
    Ok(out)
}

fn window_size() -> (u32, u32) {
    let mut size = (0u32, 0u32);
    crate::libs::window::with_window_static(|w| {
        let s = w.inner_size();
        size = (s.width, s.height);
    });
    size
}

fn intersect_part(
    shape: PartShape,
    cframe: CFrame,
    size: Vector,
    origin: Vector,
    direction: Vector,
) -> Option<(f32, Vector, Vector)> {
    let rot = euler_to_matrix(cframe.rotation);
    let inv = transpose(rot);
    let local_origin = mat3_apply(
        inv,
        Vector::new(
            origin.x - cframe.position.x,
            origin.y - cframe.position.y,
            origin.z - cframe.position.z,
        ),
    );
    let local_dir = mat3_apply(inv, direction);

    let half = Vector::new(size.x * 0.5, size.y * 0.5, size.z * 0.5);

    let (t, local_pos, local_normal) = match shape {
        PartShape::Cube | PartShape::Model => ray_obb(local_origin, local_dir, half)?,
        PartShape::Sphere => ray_ellipsoid(local_origin, local_dir, half)?,
    };

    let world_pos = Vector::new(
        cframe.position.x + mat3_apply(rot, local_pos).x,
        cframe.position.y + mat3_apply(rot, local_pos).y,
        cframe.position.z + mat3_apply(rot, local_pos).z,
    );
    let world_normal = mat3_apply(rot, local_normal);
    let nl = world_normal.magnitude().max(1e-6);
    let world_normal = Vector::new(
        world_normal.x / nl,
        world_normal.y / nl,
        world_normal.z / nl,
    );

    Some((t, world_pos, world_normal))
}

fn ray_obb(origin: Vector, dir: Vector, half: Vector) -> Option<(f32, Vector, Vector)> {
    let bounds_min = [-half.x, -half.y, -half.z];
    let bounds_max = [half.x, half.y, half.z];
    let o = [origin.x, origin.y, origin.z];
    let d = [dir.x, dir.y, dir.z];

    let mut tmin = f32::NEG_INFINITY;
    let mut tmax = f32::INFINITY;
    let mut hit_axis = 0usize;
    let mut hit_sign = 1.0f32;

    for axis in 0..3 {
        if d[axis].abs() < 1e-8 {
            if o[axis] < bounds_min[axis] || o[axis] > bounds_max[axis] {
                return None;
            }
        } else {
            let inv = 1.0 / d[axis];
            let mut t1 = (bounds_min[axis] - o[axis]) * inv;
            let mut t2 = (bounds_max[axis] - o[axis]) * inv;
            let mut sign = -1.0;
            if t1 > t2 {
                std::mem::swap(&mut t1, &mut t2);
                sign = 1.0;
            }
            if t1 > tmin {
                tmin = t1;
                hit_axis = axis;
                hit_sign = sign;
            }
            if t2 < tmax {
                tmax = t2;
            }
            if tmin > tmax {
                return None;
            }
        }
    }

    let t = if tmin > 0.0 { tmin } else { tmax };
    if t < 0.0 {
        return None;
    }

    let pos = Vector::new(o[0] + d[0] * t, o[1] + d[1] * t, o[2] + d[2] * t);
    let mut n = [0.0f32; 3];
    n[hit_axis] = hit_sign;
    Some((t, pos, Vector::new(n[0], n[1], n[2])))
}

fn ray_ellipsoid(origin: Vector, dir: Vector, half: Vector) -> Option<(f32, Vector, Vector)> {
    let rx = half.x.max(1e-6);
    let ry = half.y.max(1e-6);
    let rz = half.z.max(1e-6);
    let o = Vector::new(origin.x / rx, origin.y / ry, origin.z / rz);
    let d = Vector::new(dir.x / rx, dir.y / ry, dir.z / rz);

    let a = d.x * d.x + d.y * d.y + d.z * d.z;
    let b = 2.0 * (o.x * d.x + o.y * d.y + o.z * d.z);
    let c = o.x * o.x + o.y * o.y + o.z * o.z - 1.0;
    let disc = b * b - 4.0 * a * c;
    if disc < 0.0 || a.abs() < 1e-8 {
        return None;
    }
    let sq = disc.sqrt();
    let t1 = (-b - sq) / (2.0 * a);
    let t2 = (-b + sq) / (2.0 * a);
    let t = if t1 > 0.0 {
        t1
    } else if t2 > 0.0 {
        t2
    } else {
        return None;
    };

    let local = Vector::new(
        origin.x + dir.x * t,
        origin.y + dir.y * t,
        origin.z + dir.z * t,
    );
    let n = Vector::new(
        local.x / (rx * rx),
        local.y / (ry * ry),
        local.z / (rz * rz),
    );
    let nl = n.magnitude().max(1e-6);
    let normal = Vector::new(n.x / nl, n.y / nl, n.z / nl);
    Some((t, local, normal))
}

type Mat3 = [[f32; 3]; 3];

fn euler_to_matrix(rot: Vector) -> Mat3 {
    let (sx, cx) = rot.x.sin_cos();
    let (sy, cy) = rot.y.sin_cos();
    let (sz, cz) = rot.z.sin_cos();
    [
        [cy * cz, -cy * sz, sy],
        [sx * sy * cz + cx * sz, -sx * sy * sz + cx * cz, -sx * cy],
        [-cx * sy * cz + sx * sz, cx * sy * sz + sx * cz, cx * cy],
    ]
}

fn mat3_apply(m: Mat3, v: Vector) -> Vector {
    Vector::new(
        m[0][0] * v.x + m[0][1] * v.y + m[0][2] * v.z,
        m[1][0] * v.x + m[1][1] * v.y + m[1][2] * v.z,
        m[2][0] * v.x + m[2][1] * v.y + m[2][2] * v.z,
    )
}

fn transpose(m: Mat3) -> Mat3 {
    [
        [m[0][0], m[1][0], m[2][0]],
        [m[0][1], m[1][1], m[2][1]],
        [m[0][2], m[1][2], m[2][2]],
    ]
}
