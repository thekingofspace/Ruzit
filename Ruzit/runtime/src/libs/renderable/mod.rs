use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use mlua::{
    AnyUserData, Lua, MultiValue, RegistryKey, Table, UserData, UserDataFields, UserDataMethods,
    Value,
};

use crate::libs::asset::{FragmentAsset, ImageAsset, ModelAsset, ShaderAsset};
use crate::libs::primitives::{value_to_vector_opt, CFrame, Color3, Vector};
use crate::libs::signal;

pub mod animation;
pub mod effect_volume;
pub mod mesh;

use animation::{
    AnimationTrack, Keyframe, KeyframeAction, TrackBaseline, TrackEvent, TrackRef, TrackState,
    UpdateLink,
};
pub use effect_volume::{
    effect_volume_snapshot, tick_effect_volumes, EffectVolumeHandle, EffectVolumeRender,
    ParticleRender,
};

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PartShape {
    Cube,
    Sphere,
    Model,
}

#[derive(Clone)]
pub struct AttachedShader3D {
    pub id: u64,
    pub wgsl: Arc<String>,
    pub slot_of_name: Arc<HashMap<String, u8>>,
    pub params: Arc<Mutex<[f32; 16]>>,
}

#[derive(Clone)]
pub struct ModelRef {
    pub id: u64,
    pub vertices: Arc<Vec<mesh::Vertex3D>>,
    pub indices: Arc<Vec<u32>>,
}

#[derive(Clone)]
pub struct PartTextureRef {
    pub id: u64,
    pub width: u32,
    pub height: u32,
    pub data: Arc<Vec<u8>>,
    pub version: u64,
    pub live: Option<Arc<Mutex<DynTextureBuffer>>>,
}

pub struct DynTextureBuffer {
    pub width: u32,
    pub height: u32,
    pub bytes: Vec<u8>,
    pub version: u64,
}

pub struct PartState {
    #[allow(dead_code)]
    pub id: u64,
    pub shape: PartShape,
    pub cframe: CFrame,
    pub size: Vector,
    pub color: Color3,
    pub render: bool,
    pub alive: bool,
    pub attached: Vec<AttachedShader3D>,
    pub changed_signal: Table,

    pub model: Option<ModelRef>,

    pub deformed: Option<ModelRef>,
    pub deform_version: u64,

    pub texture: Option<PartTextureRef>,

    pub cast_shadow: bool,
    pub receive_shadow: bool,
    pub ignore_raycast: bool,
    pub lit: bool,

    pub tracks: Vec<TrackRef>,

    pub source_animations: Option<Arc<Vec<mesh::FbxAnimClip>>>,

    pub physics_override: Option<Arc<Mutex<CFrame>>>,

    pub prop_signals: HashMap<String, Table>,
}

impl PartState {
    pub fn current_cframe(&self) -> CFrame {
        if let Some(o) = &self.physics_override {
            if let Ok(g) = o.lock() {
                return *g;
            }
        }
        self.cframe
    }
}

thread_local! {
    static PARTS: RefCell<Vec<Arc<Mutex<PartState>>>> = const { RefCell::new(Vec::new()) };
    static CAMERA: RefCell<CameraState> = RefCell::new(CameraState::default());
    static LIGHTING: RefCell<LightingState> = RefCell::new(LightingState::default());
    static FRAME_INDEX: RefCell<u32> = const { RefCell::new(0) };
    static DISTORTION_BOXES: RefCell<Vec<Arc<Mutex<DistortionBoxState>>>> = const { RefCell::new(Vec::new()) };
}

pub struct DistortionBoxState {
    pub id: u64,
    pub target: Arc<Mutex<PartState>>,
    pub alive: bool,
    pub initial_cf: CFrame,
    pub initial_size: Vector,
    pub current_cf: CFrame,
    pub current_size: Vector,
    pub last_applied_cf: Option<CFrame>,
    pub last_applied_size: Option<Vector>,
    pub captured: Vec<(usize, [f32; 3])>,
}

pub struct DistortionBoxHandle {
    pub inner: Arc<Mutex<DistortionBoxState>>,
}

impl UserData for DistortionBoxHandle {
    fn add_fields<F: UserDataFields<Self>>(f: &mut F) {
        f.add_field_method_get("CFrame", |_, this| {
            Ok(this.inner.lock().unwrap().current_cf)
        });
        f.add_field_method_set("CFrame", |_, this, value: AnyUserData| {
            let cf = *value.borrow::<CFrame>().map_err(|_| {
                mlua::Error::RuntimeError("DistortionBox.CFrame expects a CFrame".into())
            })?;
            let mut bs = this.inner.lock().unwrap();
            if bs.alive {
                bs.current_cf = cf;
            }
            Ok(())
        });
        f.add_field_method_get("Size", |_, this| {
            Ok(this.inner.lock().unwrap().current_size)
        });
        f.add_field_method_set("Size", |_, this, v: Vector| {
            let mut bs = this.inner.lock().unwrap();
            if bs.alive {
                bs.current_size = v;
            }
            Ok(())
        });
        f.add_field_method_get("InitialCFrame", |_, this| {
            Ok(this.inner.lock().unwrap().initial_cf)
        });
        f.add_field_method_get("InitialSize", |_, this| {
            Ok(this.inner.lock().unwrap().initial_size)
        });
        f.add_field_method_get("VertexCount", |_, this| {
            Ok(this.inner.lock().unwrap().captured.len() as i64)
        });
        f.add_field_method_get("IsAlive", |_, this| {
            Ok(this.inner.lock().unwrap().alive)
        });
    }

    fn add_methods<M: UserDataMethods<Self>>(m: &mut M) {
        m.add_method("Destroy", |_, this, _: ()| {
            this.inner.lock().unwrap().alive = false;
            Ok(())
        });
    }
}

fn capture_distortion_vertices(
    verts: &[mesh::Vertex3D],
    cf: CFrame,
    size: Vector,
) -> Vec<(usize, [f32; 3])> {
    let inv_rot = mat3_transpose(euler_to_matrix_local(cf.rotation));
    let half_x = size.x.abs() * 0.5;
    let half_y = size.y.abs() * 0.5;
    let half_z = size.z.abs() * 0.5;
    let mut out = Vec::new();
    for (i, v) in verts.iter().enumerate() {
        let delta = Vector::new(
            v.position[0] - cf.position.x,
            v.position[1] - cf.position.y,
            v.position[2] - cf.position.z,
        );
        let local = mat3_apply_local(inv_rot, delta);
        if local.x.abs() > half_x || local.y.abs() > half_y || local.z.abs() > half_z {
            continue;
        }
        let u = [
            if size.x.abs() > 1e-6 {
                local.x / size.x
            } else {
                0.0
            },
            if size.y.abs() > 1e-6 {
                local.y / size.y
            } else {
                0.0
            },
            if size.z.abs() > 1e-6 {
                local.z / size.z
            } else {
                0.0
            },
        ];
        out.push((i, u));
    }
    out
}

pub fn tick_distortion_boxes() {
    let snapshot: Vec<Arc<Mutex<DistortionBoxState>>> = DISTORTION_BOXES.with(|c| {
        let mut reg = c.borrow_mut();
        reg.retain(|b| {
            let bs = b.lock().unwrap();
            bs.target.lock().unwrap().alive
        });
        reg.iter().cloned().collect()
    });
    if snapshot.is_empty() {
        return;
    }

    let mut groups: HashMap<usize, Vec<Arc<Mutex<DistortionBoxState>>>> = HashMap::new();
    for b in snapshot {
        let key = Arc::as_ptr(&b.lock().unwrap().target) as usize;
        groups.entry(key).or_default().push(b);
    }

    for (_, boxes) in groups {
        let target = {
            let bs = boxes[0].lock().unwrap();
            bs.target.clone()
        };

        let any_dirty = boxes.iter().any(|b| {
            let bs = b.lock().unwrap();
            match (bs.last_applied_cf, bs.last_applied_size) {
                (Some(prev_cf), Some(prev_size)) => {
                    !cframe_close(prev_cf, bs.current_cf)
                        || !vector_close(prev_size, bs.current_size)
                }
                _ => true,
            }
        });
        if !any_dirty {
            continue;
        }

        let mut p = target.lock().unwrap();
        let Some(base_model) = p.model.clone() else {
            continue;
        };
        let mut working: Vec<mesh::Vertex3D> = match &p.deformed {
            Some(d) if d.id == base_model.id => (*d.vertices).clone(),
            _ => (*base_model.vertices).clone(),
        };
        let mut any_change = false;
        for box_arc in &boxes {
            let mut bs = box_arc.lock().unwrap();
            let cf = bs.current_cf;
            let size = bs.current_size;
            let rot = euler_to_matrix_local(cf.rotation);
            for (idx, u) in &bs.captured {
                if *idx >= working.len() {
                    continue;
                }
                let scaled = Vector::new(u[0] * size.x, u[1] * size.y, u[2] * size.z);
                let rotated = mat3_apply_local(rot, scaled);
                working[*idx].position = [
                    cf.position.x + rotated.x,
                    cf.position.y + rotated.y,
                    cf.position.z + rotated.z,
                ];
                any_change = true;
            }
            bs.last_applied_cf = Some(cf);
            bs.last_applied_size = Some(size);
        }
        if any_change {
            mesh::recompute_normals(&mut working, &base_model.indices);
            p.deformed = Some(ModelRef {
                id: base_model.id,
                vertices: Arc::new(working),
                indices: base_model.indices.clone(),
            });
            p.deform_version = p.deform_version.wrapping_add(1);
            bump_parts_dirty();
        }
    }
}

struct AudioLink {
    audio_id: usize,
    part: Arc<Mutex<PartState>>,
    update: Box<dyn Fn(f32, f32, f32) + Send>,
}

thread_local! {
    static AUDIO_LINKS: RefCell<Vec<AudioLink>> = const { RefCell::new(Vec::new()) };
}

pub fn tick_audio_links() {
    AUDIO_LINKS.with(|c| {
        let mut links = c.borrow_mut();
        links.retain(|link| link.part.lock().unwrap().alive);
        for link in links.iter() {
            let (x, y, z) = {
                let p = link.part.lock().unwrap();
                let pos = p.current_cframe().position;
                (pos.x, pos.y, pos.z)
            };
            (link.update)(x, y, z);
        }
    });
}

fn audio_id_of(audio: &AnyUserData) -> mlua::Result<usize> {
    if let Ok(snd) = audio.borrow::<crate::libs::sfx::Sound>() {
        return Ok(Arc::as_ptr(&snd.shaders) as usize);
    }
    if let Ok(vc) = audio.borrow::<crate::libs::voice::ChannelHandle>() {
        return Ok(Arc::as_ptr(&vc.shaders) as usize);
    }
    Err(mlua::Error::RuntimeError(
        "expected a Sound or VoiceChannel".into(),
    ))
}

fn link_audio_to_part(part: &Arc<Mutex<PartState>>, audio: AnyUserData) -> mlua::Result<()> {
    use crate::libs::sfx::{Shader as SfxShader, Sound, SpatialPos};
    use crate::libs::voice::{ChannelHandle, VoiceShader};

    let (audio_id, update): (usize, Box<dyn Fn(f32, f32, f32) + Send>) =
        if let Ok(snd) = audio.borrow::<Sound>() {
            let id = Arc::as_ptr(&snd.shaders) as usize;
            let pos_arc = snd.position.clone();
            let cb = Box::new(move |x: f32, y: f32, z: f32| {
                let mut guard = pos_arc.lock().unwrap();
                let (min_f, max_f) = guard
                    .as_ref()
                    .map(|p| (p.min_falloff, p.max_falloff))
                    .unwrap_or((0.0, 20.0));
                *guard = Some(SpatialPos {
                    x,
                    y,
                    z,
                    min_falloff: min_f,
                    max_falloff: max_f,
                });
            });
            let _ = std::marker::PhantomData::<SfxShader>;
            (id, cb)
        } else if let Ok(vc) = audio.borrow::<ChannelHandle>() {
            let id = Arc::as_ptr(&vc.shaders) as usize;
            let shaders_arc = vc.shaders.clone();
            let pos_arc = vc.position.clone();
            let cb = Box::new(move |x: f32, y: f32, z: f32| {
                let mut list = shaders_arc.lock().unwrap();
                let (mn, mx) = list
                    .iter()
                    .rev()
                    .find_map(|s| match s {
                        VoiceShader::Spatial {
                            min_falloff,
                            max_falloff,
                            ..
                        } => Some((*min_falloff, *max_falloff)),
                        _ => None,
                    })
                    .unwrap_or((0.0, 8.0));
                list.retain(|s| !matches!(s, VoiceShader::Spatial { .. }));
                list.push(VoiceShader::Spatial {
                    x,
                    y,
                    z,
                    min_falloff: mn,
                    max_falloff: mx,
                });
                drop(list);
                *pos_arc.lock().unwrap() = (x, y, z);
            });
            (id, cb)
        } else {
            return Err(mlua::Error::RuntimeError(
                "LinkInput: expected a Sound or VoiceChannel".into(),
            ));
        };

    let (x, y, z) = {
        let p = part.lock().unwrap();
        let pos = p.current_cframe().position;
        (pos.x, pos.y, pos.z)
    };
    update(x, y, z);

    AUDIO_LINKS.with(|c| {
        let mut links = c.borrow_mut();
        links.retain(|l| l.audio_id != audio_id);
        links.push(AudioLink {
            audio_id,
            part: part.clone(),
            update,
        });
    });
    Ok(())
}

fn unlink_audio_from_part(part: &Arc<Mutex<PartState>>, audio: AnyUserData) -> mlua::Result<()> {
    let audio_id = audio_id_of(&audio)?;
    let part_id = Arc::as_ptr(part) as usize;
    AUDIO_LINKS.with(|c| {
        c.borrow_mut().retain(|l| {
            !(l.audio_id == audio_id && Arc::as_ptr(&l.part) as usize == part_id)
        });
    });
    Ok(())
}

fn cframe_close(a: CFrame, b: CFrame) -> bool {
    let eps = 1e-5_f32;
    (a.position.x - b.position.x).abs() < eps
        && (a.position.y - b.position.y).abs() < eps
        && (a.position.z - b.position.z).abs() < eps
        && (a.rotation.x - b.rotation.x).abs() < eps
        && (a.rotation.y - b.rotation.y).abs() < eps
        && (a.rotation.z - b.rotation.z).abs() < eps
}

fn vector_close(a: Vector, b: Vector) -> bool {
    let eps = 1e-5_f32;
    (a.x - b.x).abs() < eps && (a.y - b.y).abs() < eps && (a.z - b.z).abs() < eps
}

#[derive(Clone, Copy)]
pub struct LightingState {
    pub sun_direction: Vector,
    pub sun_color: Color3,
    pub ambient: Color3,
    pub frame_index: u32,
}

impl Default for LightingState {
    fn default() -> Self {
        Self {
            sun_direction: Vector::new(-0.4, -1.0, -0.3),
            sun_color: Color3::new(1.0, 1.0, 1.0),
            ambient: Color3::new(0.25, 0.25, 0.25),
            frame_index: 0,
        }
    }
}

pub fn set_sun(direction: Vector, color: Option<Color3>) {
    LIGHTING.with(|cell| {
        let mut s = cell.borrow_mut();
        s.sun_direction = direction;
        if let Some(c) = color {
            s.sun_color = c;
        }
    });
    bump_lighting_dirty();
}

pub fn set_sun_color(color: Color3) {
    LIGHTING.with(|cell| cell.borrow_mut().sun_color = color);
    bump_lighting_dirty();
}

pub fn set_ambient(color: Color3) {
    LIGHTING.with(|cell| cell.borrow_mut().ambient = color);
    bump_lighting_dirty();
}

pub fn lighting_snapshot() -> LightingState {
    let mut snap = LIGHTING.with(|c| *c.borrow());
    snap.frame_index = FRAME_INDEX.with(|f| {
        let mut v = f.borrow_mut();
        *v = v.wrapping_add(1);
        *v
    });
    snap
}

pub struct CameraState {
    pub cframe: CFrame,
    pub fov_deg: f32,
    pub near: f32,
    pub far: f32,
}

impl Default for CameraState {
    fn default() -> Self {
        Self {
            cframe: CFrame::new(Vector::new(4.0, 3.0, 5.0), Vector::new(-0.4, 0.65, 0.0)),
            fov_deg: 60.0,
            near: 0.1,
            far: 1000.0,
        }
    }
}

pub fn list_part_states() -> Vec<Arc<Mutex<PartState>>> {
    PARTS.with(|cell| {
        let mut reg = cell.borrow_mut();
        reg.retain(|p| p.lock().unwrap().alive);
        reg.clone()
    })
}

pub fn purge_texture(lua: &Lua, asset_id: u64) {
    purge_parts_matching(lua, |s| {
        s.texture
            .as_ref()
            .map(|t| t.id == asset_id)
            .unwrap_or(false)
    });
}

pub fn purge_model(lua: &Lua, asset_id: u64) {
    purge_parts_matching(lua, |s| {
        s.model.as_ref().map(|m| m.id == asset_id).unwrap_or(false)
    });
}

pub fn purge_shader(lua: &Lua, asset_id: u64) {
    let states = list_part_states();
    for state in states {
        let mut s = state.lock().unwrap();
        if !s.alive {
            continue;
        }
        s.attached.retain(|e| e.id != asset_id);
        let _ = lua;
    }
}

fn purge_parts_matching(lua: &Lua, mut pred: impl FnMut(&PartState) -> bool) {
    let states = list_part_states();
    for state in states {
        let sig = {
            let mut s = state.lock().unwrap();
            if !s.alive || !pred(&s) {
                continue;
            }
            s.alive = false;
            s.render = false;
            s.attached.clear();
            s.texture = None;
            s.model = None;
            s.deformed = None;
            s.changed_signal.clone()
        };
        let _ = fire_changed(lua, sig, "Destroyed");
    }
}

pub fn camera_snapshot() -> CameraState {
    CAMERA.with(|c| {
        let s = c.borrow();
        CameraState {
            cframe: s.cframe,
            fov_deg: s.fov_deg,
            near: s.near,
            far: s.far,
        }
    })
}

pub fn set_camera_cframe(cf: CFrame) {
    CAMERA.with(|c| c.borrow_mut().cframe = cf);
    bump_camera_dirty();
}

pub struct PartRender {
    pub shape: PartShape,
    pub cframe: CFrame,
    pub size: Vector,
    pub color: Color3,
    pub active_shader: Option<AttachedShader3D>,
    pub model: Option<ModelRef>,
    pub texture: Option<PartTextureRef>,
    pub cast_shadow: bool,
    pub receive_shadow: bool,
    pub lit: bool,
}

thread_local! {
    static PARTS_SNAPSHOT_CACHE: RefCell<(u64, Arc<Vec<PartRender>>)> =
        RefCell::new((0, Arc::new(Vec::new())));
}

pub fn snapshot() -> Arc<Vec<PartRender>> {
    let version = parts_version();
    let cached_match = PARTS_SNAPSHOT_CACHE.with(|cell| {
        let cache = cell.borrow();
        if cache.0 == version {
            Some(cache.1.clone())
        } else {
            None
        }
    });
    if let Some(items) = cached_match {
        return items;
    }
    let items = build_parts_snapshot();
    let arc = Arc::new(items);
    PARTS_SNAPSHOT_CACHE.with(|cell| {
        *cell.borrow_mut() = (version, arc.clone());
    });
    arc
}

fn build_parts_snapshot() -> Vec<PartRender> {
    PARTS.with(|cell| {
        let mut reg = cell.borrow_mut();
        reg.retain(|p| p.lock().unwrap().alive);
        reg.iter()
            .filter_map(|p| {
                let s = p.lock().unwrap();
                if !s.render {
                    return None;
                }
                Some(PartRender {
                    shape: s.shape,
                    cframe: s.current_cframe(),
                    size: s.size,
                    color: s.color,
                    active_shader: s.attached.last().cloned(),

                    model: s.deformed.clone().or_else(|| s.model.clone()),
                    texture: s.texture.clone(),
                    cast_shadow: s.cast_shadow,
                    receive_shadow: s.receive_shadow,
                    lit: s.lit,
                })
            })
            .collect()
    })
}

pub struct PartHandle {
    pub(crate) state: Arc<Mutex<PartState>>,
}

impl PartHandle {
    pub fn from_state(state: Arc<Mutex<PartState>>) -> Self {
        Self { state }
    }

    pub fn new_shape(lua: &Lua, shape: PartShape, model: Option<ModelRef>) -> mlua::Result<Self> {
        Self::new_shape_with(lua, shape, model, None)
    }

    fn new_shape_with(
        lua: &Lua,
        shape: PartShape,
        model: Option<ModelRef>,
        source_animations: Option<Arc<Vec<mesh::FbxAnimClip>>>,
    ) -> mlua::Result<Self> {
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        let changed_signal = signal::new_instance(lua)?;
        let state = Arc::new(Mutex::new(PartState {
            id,
            shape,
            cframe: CFrame::new(Vector::new(0.0, 0.0, 0.0), Vector::new(0.0, 0.0, 0.0)),
            size: Vector::new(1.0, 1.0, 1.0),
            color: Color3::new(1.0, 1.0, 1.0),
            render: true,
            alive: true,
            attached: Vec::new(),
            changed_signal,
            model,
            deformed: None,
            deform_version: 0,
            texture: None,
            cast_shadow: true,
            receive_shadow: true,
            ignore_raycast: false,
            lit: false,
            tracks: Vec::new(),
            source_animations,
            physics_override: None,
            prop_signals: HashMap::new(),
        }));
        PARTS.with(|cell| cell.borrow_mut().push(state.clone()));
        bump_parts_dirty();
        Ok(Self { state })
    }

    fn ensure_alive(&self, op: &str) -> mlua::Result<()> {
        if !self.state.lock().unwrap().alive {
            return Err(mlua::Error::RuntimeError(format!(
                "Renderable: {op} called on a destroyed part"
            )));
        }
        Ok(())
    }
}

fn fire_changed(lua: &Lua, signal_table: Table, prop: &str) -> mlua::Result<()> {
    bump_parts_dirty();
    let mut args = MultiValue::new();
    args.push_back(Value::String(lua.create_string(prop)?));
    signal::fire(lua, &signal_table, args)
}

fn fire_prop_changed(lua: &Lua, prop_sig: Option<Table>, value: Value) {
    if let Some(sig) = prop_sig {
        let mut args = MultiValue::new();
        args.push_back(value);
        let _ = signal::fire(lua, &sig, args);
    }
}

pub fn signal_has_listeners(signal: &Table) -> bool {
    if let Ok(conns) = signal.get::<Table>("_conns") {
        for pair in conns.pairs::<Value, Value>() {
            if pair.is_ok() {
                return true;
            }
        }
    }
    if let Ok(waiting) = signal.get::<Table>("_waiting") {
        if waiting.len().unwrap_or(0) > 0 {
            return true;
        }
    }
    false
}

fn ensure_prop_signal(
    lua: &Lua,
    state: &mut PartState,
    prop: &str,
) -> mlua::Result<Table> {
    if let Some(sig) = state.prop_signals.get(prop) {
        return Ok(sig.clone());
    }
    let sig = signal::new_instance(lua)?;
    state.prop_signals.insert(prop.to_string(), sig.clone());
    Ok(sig)
}

static PARTS_VERSION: AtomicU64 = AtomicU64::new(1);
static CAMERA_VERSION: AtomicU64 = AtomicU64::new(1);
static LIGHTING_VERSION: AtomicU64 = AtomicU64::new(1);

pub fn bump_parts_dirty() {
    PARTS_VERSION.fetch_add(1, Ordering::Relaxed);
}

pub fn parts_version() -> u64 {
    PARTS_VERSION.load(Ordering::Relaxed)
}

pub fn bump_camera_dirty() {
    CAMERA_VERSION.fetch_add(1, Ordering::Relaxed);
}

pub fn camera_version() -> u64 {
    CAMERA_VERSION.load(Ordering::Relaxed)
}

pub fn bump_lighting_dirty() {
    LIGHTING_VERSION.fetch_add(1, Ordering::Relaxed);
}

pub fn lighting_version() -> u64 {
    LIGHTING_VERSION.load(Ordering::Relaxed)
}

type LocalMat3 = [[f32; 3]; 3];

fn euler_to_matrix_local(rot: Vector) -> LocalMat3 {
    let (sx, cx) = rot.x.sin_cos();
    let (sy, cy) = rot.y.sin_cos();
    let (sz, cz) = rot.z.sin_cos();
    [
        [cy * cz, -cy * sz, sy],
        [sx * sy * cz + cx * sz, -sx * sy * sz + cx * cz, -sx * cy],
        [-cx * sy * cz + sx * sz, cx * sy * sz + sx * cz, cx * cy],
    ]
}

fn mat3_apply_local(m: LocalMat3, v: Vector) -> Vector {
    Vector::new(
        m[0][0] * v.x + m[0][1] * v.y + m[0][2] * v.z,
        m[1][0] * v.x + m[1][1] * v.y + m[1][2] * v.z,
        m[2][0] * v.x + m[2][1] * v.y + m[2][2] * v.z,
    )
}

fn mat3_transpose(m: LocalMat3) -> LocalMat3 {
    [
        [m[0][0], m[1][0], m[2][0]],
        [m[0][1], m[1][1], m[2][1]],
        [m[0][2], m[1][2], m[2][2]],
    ]
}

fn fire_track_signal(lua: &Lua, key: &Arc<RegistryKey>, args: MultiValue) {
    match lua.registry_value::<Table>(key) {
        Ok(t) => {
            if let Err(e) = signal::fire(lua, &t, args) {
                eprintln!("[AnimationTrack] signal fire error: {e}");
            }
        }
        Err(e) => eprintln!("[AnimationTrack] signal lookup: {e}"),
    }
}

fn ensure_track_signal(lua: &Lua, slot: &mut Option<Arc<RegistryKey>>) -> mlua::Result<Table> {
    if let Some(k) = slot {
        return lua.registry_value(k);
    }
    let signal = signal::new_instance(lua)?;
    let key = Arc::new(lua.create_registry_value(signal.clone())?);
    *slot = Some(key);
    Ok(signal)
}

fn parse_param_decls(src: &str) -> HashMap<String, u8> {
    let mut map = HashMap::new();
    let mut next_slot: u8 = 0;
    for raw in src.lines() {
        let line = raw.trim();
        let rest = if let Some(r) = line.strip_prefix("//") {
            r.trim_start()
        } else if let Some(r) = line.strip_prefix("/*") {
            r.trim_start()
        } else {
            continue;
        };
        let Some(rest) = rest.strip_prefix("@ruzit") else {
            continue;
        };
        let rest = rest.trim_start();
        let Some(rest) = rest.strip_prefix("param") else {
            continue;
        };
        let name = rest.split_whitespace().next().unwrap_or("").to_string();
        if name.is_empty() {
            continue;
        }
        if next_slot >= 16 {
            eprintln!("[Renderable] shader has more than 16 @ruzit params; '{name}' ignored");
            continue;
        }
        map.entry(name).or_insert(next_slot);
        next_slot += 1;
    }
    map
}

pub(crate) fn build_attached_3d(asset: &AnyUserData) -> mlua::Result<AttachedShader3D> {
    let (id, code) = if let Ok(s) = asset.borrow::<ShaderAsset>() {
        (s.id, s.code.clone())
    } else if let Ok(f) = asset.borrow::<FragmentAsset>() {
        (f.id, f.code.clone())
    } else {
        return Err(mlua::Error::RuntimeError(
            "expected a Shader or Fragment asset".into(),
        ));
    };
    let slot_of_name = parse_param_decls(&code);

    let prelude = crate::libs::renderable::render::FRAGMENT_PRELUDE_3D;
    let has_user_vs = code.contains("@vertex");
    let has_user_fs = code.contains("@fragment");
    let mut wgsl = format!("{prelude}\n{code}");
    if !has_user_vs {
        wgsl.push('\n');
        wgsl.push_str(crate::libs::renderable::render::DEFAULT_VS_3D);
    }
    if !has_user_fs {
        wgsl.push('\n');
        wgsl.push_str(crate::libs::renderable::render::DEFAULT_FRAGMENT_WGSL_3D);
    }

    Ok(AttachedShader3D {
        id,
        wgsl: Arc::new(wgsl),
        slot_of_name: Arc::new(slot_of_name),
        params: Arc::new(Mutex::new([0.0_f32; 16])),
    })
}

pub(crate) fn shader_asset_id(asset: &AnyUserData) -> mlua::Result<u64> {
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

impl UserData for PartHandle {
    fn add_fields<F: UserDataFields<Self>>(f: &mut F) {
        f.add_field_method_get("Changed", |_, this| {
            Ok(this.state.lock().unwrap().changed_signal.clone())
        });
        f.add_field_method_get("Shape", |_, this| {
            Ok(match this.state.lock().unwrap().shape {
                PartShape::Cube => "Cube",
                PartShape::Sphere => "Sphere",
                PartShape::Model => "Model",
            })
        });

        f.add_field_method_get("CFrame", |_, this| {
            Ok(this.state.lock().unwrap().current_cframe())
        });
        f.add_field_method_set("CFrame", |lua, this, value: AnyUserData| {
            this.ensure_alive("set CFrame")?;
            let cf = *value
                .borrow::<CFrame>()
                .map_err(|_| mlua::Error::RuntimeError("CFrame expects a CFrame".into()))?;
            let (sig, prop_sig) = {
                let mut s = this.state.lock().unwrap();
                s.cframe = cf;
                if let Some(o) = &s.physics_override {
                    if let Ok(mut g) = o.lock() {
                        *g = cf;
                    }
                }
                (
                    s.changed_signal.clone(),
                    s.prop_signals.get("CFrame").cloned(),
                )
            };
            fire_changed(lua, sig, "CFrame")?;
            fire_prop_changed(lua, prop_sig, Value::UserData(lua.create_userdata(cf)?));
            Ok(())
        });

        f.add_field_method_get("Size", |_, this| Ok(this.state.lock().unwrap().size));
        f.add_field_method_set("Size", |lua, this, v: Vector| {
            this.ensure_alive("set Size")?;
            let (sig, prop_sig) = {
                let mut s = this.state.lock().unwrap();
                s.size = v;
                (
                    s.changed_signal.clone(),
                    s.prop_signals.get("Size").cloned(),
                )
            };
            fire_changed(lua, sig, "Size")?;
            fire_prop_changed(lua, prop_sig, Value::UserData(lua.create_userdata(v)?));
            Ok(())
        });

        f.add_field_method_get("Color", |_, this| Ok(this.state.lock().unwrap().color));
        f.add_field_method_set("Color", |lua, this, value: AnyUserData| {
            this.ensure_alive("set Color")?;
            let c = *value
                .borrow::<Color3>()
                .map_err(|_| mlua::Error::RuntimeError("Color expects a Color3".into()))?;
            let (sig, prop_sig) = {
                let mut s = this.state.lock().unwrap();
                s.color = c;
                (
                    s.changed_signal.clone(),
                    s.prop_signals.get("Color").cloned(),
                )
            };
            fire_changed(lua, sig, "Color")?;
            fire_prop_changed(lua, prop_sig, Value::UserData(lua.create_userdata(c)?));
            Ok(())
        });

        f.add_field_method_get("Render", |_, this| Ok(this.state.lock().unwrap().render));
        f.add_field_method_set("Render", |lua, this, value: bool| {
            this.ensure_alive("set Render")?;
            let (sig, prop_sig) = {
                let mut s = this.state.lock().unwrap();
                s.render = value;
                (
                    s.changed_signal.clone(),
                    s.prop_signals.get("Render").cloned(),
                )
            };
            fire_changed(lua, sig, "Render")?;
            fire_prop_changed(lua, prop_sig, Value::Boolean(value));
            Ok(())
        });

        f.add_field_method_get("CastShadow", |_, this| {
            Ok(this.state.lock().unwrap().cast_shadow)
        });
        f.add_field_method_set("CastShadow", |lua, this, value: bool| {
            this.ensure_alive("set CastShadow")?;
            let (sig, prop_sig) = {
                let mut s = this.state.lock().unwrap();
                s.cast_shadow = value;
                (
                    s.changed_signal.clone(),
                    s.prop_signals.get("CastShadow").cloned(),
                )
            };
            fire_changed(lua, sig, "CastShadow")?;
            fire_prop_changed(lua, prop_sig, Value::Boolean(value));
            Ok(())
        });

        f.add_field_method_get("ReceiveShadow", |_, this| {
            Ok(this.state.lock().unwrap().receive_shadow)
        });
        f.add_field_method_set("ReceiveShadow", |lua, this, value: bool| {
            this.ensure_alive("set ReceiveShadow")?;
            let (sig, prop_sig) = {
                let mut s = this.state.lock().unwrap();
                s.receive_shadow = value;
                (
                    s.changed_signal.clone(),
                    s.prop_signals.get("ReceiveShadow").cloned(),
                )
            };
            fire_changed(lua, sig, "ReceiveShadow")?;
            fire_prop_changed(lua, prop_sig, Value::Boolean(value));
            Ok(())
        });

        f.add_field_method_get("IgnoreInRaycast", |_, this| {
            Ok(this.state.lock().unwrap().ignore_raycast)
        });
        f.add_field_method_set("IgnoreInRaycast", |lua, this, value: bool| {
            this.ensure_alive("set IgnoreInRaycast")?;
            let (sig, prop_sig) = {
                let mut s = this.state.lock().unwrap();
                s.ignore_raycast = value;
                (
                    s.changed_signal.clone(),
                    s.prop_signals.get("IgnoreInRaycast").cloned(),
                )
            };
            fire_changed(lua, sig, "IgnoreInRaycast")?;
            fire_prop_changed(lua, prop_sig, Value::Boolean(value));
            Ok(())
        });

        f.add_field_method_get("Lit", |_, this| Ok(this.state.lock().unwrap().lit));
        f.add_field_method_set("Lit", |lua, this, value: bool| {
            this.ensure_alive("set Lit")?;
            let (sig, prop_sig) = {
                let mut s = this.state.lock().unwrap();
                s.lit = value;
                (
                    s.changed_signal.clone(),
                    s.prop_signals.get("Lit").cloned(),
                )
            };
            bump_parts_dirty();
            fire_changed(lua, sig, "Lit")?;
            fire_prop_changed(lua, prop_sig, Value::Boolean(value));
            Ok(())
        });

        f.add_field_method_get("Texture", |lua, this| -> mlua::Result<Value> {
            let s = this.state.lock().unwrap();
            match &s.texture {
                Some(_) => Ok(Value::Boolean(true)),
                None => Ok(Value::Nil),
            }
            .map(|v| v)
            .map(|v| {
                let _ = lua;
                v
            })
        });
        f.add_field_method_set("Texture", |lua, this, value: Value| {
            this.ensure_alive("set Texture")?;
            let new_tex = match value {
                Value::Nil => None,
                Value::UserData(ud) => {
                    if let Ok(img) = ud.borrow::<ImageAsset>() {
                        Some(PartTextureRef {
                            id: img.id,
                            width: img.width,
                            height: img.height,
                            data: img.data.clone(),
                            version: 0,
                            live: None,
                        })
                    } else if let Ok(dyn_img) = ud.borrow::<crate::libs::dynimg::DynImgHandle>() {
                        Some(crate::libs::dynimg::dynimg_to_part_texture(&dyn_img))
                    } else if let Ok(drawable) =
                        ud.borrow::<crate::libs::drawable::DrawableImgHandle>()
                    {
                        Some(crate::libs::drawable::drawable_to_part_texture(&drawable))
                    } else {
                        return Err(mlua::Error::RuntimeError(
                            "Texture expects an ImageAsset, DynImg, DrawableImg, or nil".into(),
                        ));
                    }
                }
                _ => {
                    return Err(mlua::Error::RuntimeError(
                        "Texture expects an ImageAsset, DynImg, DrawableImg, or nil".into(),
                    ));
                }
            };
            let sig = {
                let mut s = this.state.lock().unwrap();
                s.texture = new_tex;
                s.changed_signal.clone()
            };
            fire_changed(lua, sig, "Texture")
        });
    }

    fn add_methods<M: UserDataMethods<Self>>(m: &mut M) {
        m.add_method(
            "GetPropertyChanged",
            |lua, this, prop: String| -> mlua::Result<Table> {
                let mut s = this.state.lock().unwrap();
                ensure_prop_signal(lua, &mut s, &prop)
            },
        );

        m.add_method("Destroy", |lua, this, _: ()| -> mlua::Result<()> {
            let sig = {
                let mut s = this.state.lock().unwrap();
                if !s.alive {
                    return Ok(());
                }
                s.alive = false;
                s.render = false;
                s.attached.clear();
                s.changed_signal.clone()
            };
            fire_changed(lua, sig, "Destroyed")
        });

        m.add_method(
            "AttachShader",
            |_, this, asset: AnyUserData| -> mlua::Result<()> {
                this.ensure_alive("AttachShader")?;
                let attached = build_attached_3d(&asset)?;
                let mut s = this.state.lock().unwrap();
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
                this.ensure_alive("DetachShader")?;
                let id = shader_asset_id(&asset)?;
                let mut s = this.state.lock().unwrap();
                s.attached.retain(|e| e.id != id);
                Ok(())
            },
        );
        m.add_method("ClearShaders", |_, this, _: ()| -> mlua::Result<()> {
            this.ensure_alive("ClearShaders")?;
            this.state.lock().unwrap().attached.clear();
            Ok(())
        });
        m.add_method(
            "SetData",
            |_, this, (asset, name, value): (AnyUserData, String, f32)| -> mlua::Result<()> {
                this.ensure_alive("SetData")?;
                let id = shader_asset_id(&asset)?;
                let s = this.state.lock().unwrap();
                let entry = s.attached.iter().find(|e| e.id == id).ok_or_else(|| {
                    mlua::Error::RuntimeError("SetData: shader is not attached to this part".into())
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
                let s = this.state.lock().unwrap();
                let entry = s.attached.iter().find(|e| e.id == id).ok_or_else(|| {
                    mlua::Error::RuntimeError("GetData: shader is not attached to this part".into())
                })?;
                let Some(slot) = entry.slot_of_name.get(&name) else {
                    return Ok(None);
                };
                Ok(Some(entry.params.lock().unwrap()[*slot as usize]))
            },
        );

        m.add_method(
            "Deform",
            |_, this, (target, envelope, d): (AnyUserData, f32, Vector)| -> mlua::Result<u32> {
                this.ensure_alive("Deform")?;
                let cf = *target.borrow::<CFrame>().map_err(|_| {
                    mlua::Error::RuntimeError(
                        "Deform: first argument must be a CFrame (target point on the mesh)".into(),
                    )
                })?;
                let mut s = this.state.lock().unwrap();
                let base_model = s
                    .deformed
                    .clone()
                    .or_else(|| s.model.clone())
                    .ok_or_else(|| {
                        mlua::Error::RuntimeError(
                            "Deform: this Part has no mesh (Deform only applies to Renderable.BaseModel parts)"
                                .into(),
                        )
                    })?;
                let base_id = s.model.as_ref().map(|m| m.id).unwrap_or(base_model.id);
                let mut working: Vec<mesh::Vertex3D> = (*base_model.vertices).clone();
                let touched = mesh::deform_blob(
                    &mut working,
                    [cf.position.x, cf.position.y, cf.position.z],
                    envelope,
                    [d.x, d.y, d.z],
                );
                mesh::recompute_normals(&mut working, &base_model.indices);
                s.deformed = Some(ModelRef {
                    id: base_id,
                    vertices: Arc::new(working),
                    indices: base_model.indices.clone(),
                });
                s.deform_version = s.deform_version.wrapping_add(1);
                Ok(touched as u32)
            },
        );

        m.add_method("ResetDeformation", |_, this, _: ()| -> mlua::Result<()> {
            this.ensure_alive("ResetDeformation")?;
            let mut s = this.state.lock().unwrap();
            if s.deformed.is_some() {
                s.deformed = None;
                s.deform_version = s.deform_version.wrapping_add(1);
            }
            Ok(())
        });

        m.add_method(
            "LocalToWorld",
            |_, this, v: Vector| -> mlua::Result<Vector> {
                this.ensure_alive("LocalToWorld")?;
                let cf = this.state.lock().unwrap().cframe;
                let rot = euler_to_matrix_local(cf.rotation);
                let r = mat3_apply_local(rot, v);
                Ok(Vector::new(
                    cf.position.x + r.x,
                    cf.position.y + r.y,
                    cf.position.z + r.z,
                ))
            },
        );

        m.add_method(
            "WorldToLocal",
            |_, this, v: Vector| -> mlua::Result<Vector> {
                this.ensure_alive("WorldToLocal")?;
                let cf = this.state.lock().unwrap().cframe;
                let rel = Vector::new(
                    v.x - cf.position.x,
                    v.y - cf.position.y,
                    v.z - cf.position.z,
                );
                let rot = euler_to_matrix_local(cf.rotation);
                let inv = mat3_transpose(rot);
                Ok(mat3_apply_local(inv, rel))
            },
        );

        m.add_method(
            "GetTrack",
            |_, this, name: String| -> mlua::Result<AnimationTrackHandle> {
                this.ensure_alive("GetTrack")?;
                let mut s = this.state.lock().unwrap();
                if let Some(existing) = s
                    .tracks
                    .iter()
                    .find(|t| t.lock().unwrap().name == name)
                    .cloned()
                {
                    return Ok(AnimationTrackHandle {
                        track: existing,
                        part: this.state.clone(),
                    });
                }
                let mut track = AnimationTrack::new(name.clone());
                if let Some(anims) = &s.source_animations {
                    if let Some(clip) = anims.iter().find(|c| c.name == name) {
                        populate_track_from_fbx(&mut track, clip);
                    }
                }
                let new_track = Arc::new(Mutex::new(track));
                s.tracks.push(new_track.clone());
                Ok(AnimationTrackHandle {
                    track: new_track,
                    part: this.state.clone(),
                })
            },
        );

        m.add_method("GetTrackNames", |lua, this, _: ()| -> mlua::Result<Table> {
            let s = this.state.lock().unwrap();
            let out = lua.create_table()?;
            if let Some(anims) = &s.source_animations {
                for (i, clip) in anims.iter().enumerate() {
                    out.set(i + 1, clip.name.clone())?;
                }
            }
            Ok(out)
        });

        m.add_method("GetTracks", |lua, this, _: ()| -> mlua::Result<Table> {
            let s = this.state.lock().unwrap();
            let out = lua.create_table()?;
            for (i, t) in s.tracks.iter().enumerate() {
                out.set(
                    i + 1,
                    AnimationTrackHandle {
                        track: t.clone(),
                        part: this.state.clone(),
                    },
                )?;
            }
            Ok(out)
        });

        m.add_method(
            "LinkInput",
            |_, this, audio: AnyUserData| -> mlua::Result<()> {
                this.ensure_alive("LinkInput")?;
                link_audio_to_part(&this.state, audio)
            },
        );
        m.add_method(
            "UnlinkInput",
            |_, this, audio: AnyUserData| -> mlua::Result<()> {
                unlink_audio_from_part(&this.state, audio)
            },
        );
    }
}

#[derive(Clone)]
pub struct AnimationTrackHandle {
    pub track: TrackRef,
    pub part: Arc<Mutex<PartState>>,
}

impl UserData for AnimationTrackHandle {
    fn add_fields<F: UserDataFields<Self>>(f: &mut F) {
        f.add_field_method_get("Name", |_, this| {
            Ok(this.track.lock().unwrap().name.clone())
        });
        f.add_field_method_get("FadeTime", |_, this| {
            Ok(this.track.lock().unwrap().fade_time)
        });
        f.add_field_method_set("FadeTime", |_, this, v: f32| {
            this.track.lock().unwrap().fade_time = v.max(0.0);
            Ok(())
        });
        f.add_field_method_get("Looped", |_, this| Ok(this.track.lock().unwrap().looped));
        f.add_field_method_set("Looped", |_, this, v: bool| {
            this.track.lock().unwrap().looped = v;
            Ok(())
        });
        f.add_field_method_get("Speed", |_, this| Ok(this.track.lock().unwrap().speed));
        f.add_field_method_set("Speed", |_, this, v: f32| {
            this.track.lock().unwrap().speed = v;
            Ok(())
        });
        f.add_field_method_get("Length", |_, this| Ok(this.track.lock().unwrap().duration));
        f.add_field_method_get("TimePosition", |_, this| {
            Ok(this.track.lock().unwrap().time)
        });
        f.add_field_method_set("TimePosition", |_, this, v: f32| {
            this.track.lock().unwrap().time = v.max(0.0);
            Ok(())
        });
        f.add_field_method_get("IsPlaying", |_, this| {
            Ok(this.track.lock().unwrap().state == TrackState::Playing)
        });
        f.add_field_method_get("IsPaused", |_, this| {
            Ok(this.track.lock().unwrap().state == TrackState::Paused)
        });

        f.add_field_method_get("Played", |lua, this| -> mlua::Result<Table> {
            let mut t = this.track.lock().unwrap();
            ensure_track_signal(lua, &mut t.played_key)
        });
        f.add_field_method_get("Stopped", |lua, this| -> mlua::Result<Table> {
            let mut t = this.track.lock().unwrap();
            ensure_track_signal(lua, &mut t.stopped_key)
        });
        f.add_field_method_get("DidLoop", |lua, this| -> mlua::Result<Table> {
            let mut t = this.track.lock().unwrap();
            ensure_track_signal(lua, &mut t.did_loop_key)
        });
    }

    fn add_methods<M: UserDataMethods<Self>>(m: &mut M) {
        m.add_method("Play", |_, this, _: ()| -> mlua::Result<()> {
            let baseline = {
                let p = this.part.lock().unwrap();
                let model = p.deformed.clone().or_else(|| p.model.clone());
                TrackBaseline {
                    cframe: Some(p.cframe),
                    vertices: model.as_ref().map(|m| m.vertices.clone()),
                }
            };
            let mut t = this.track.lock().unwrap();
            t.baseline = baseline;
            t.time = 0.0;
            t.loop_count = 0;
            for link in t.update_links.iter_mut() {
                link.next_fire = link.interval;
            }
            t.state = TrackState::Playing;
            t.pending_events.push(TrackEvent::Played);
            Ok(())
        });

        m.add_method("Pause", |_, this, _: ()| -> mlua::Result<()> {
            let mut t = this.track.lock().unwrap();
            if t.state == TrackState::Playing {
                t.state = TrackState::Paused;
            }
            Ok(())
        });

        m.add_method("Resume", |_, this, _: ()| -> mlua::Result<()> {
            let mut t = this.track.lock().unwrap();
            if t.state == TrackState::Paused {
                t.state = TrackState::Playing;
            }
            Ok(())
        });

        m.add_method("Stop", |_, this, _: ()| -> mlua::Result<()> {
            let mut t = this.track.lock().unwrap();
            let was_active = t.state != TrackState::Stopped;
            t.state = TrackState::Stopped;
            t.time = 0.0;
            if was_active {
                t.pending_events.push(TrackEvent::Stopped);
            }
            Ok(())
        });

        m.add_method(
            "LinkToUpdate",
            |lua, this, interval: f64| -> mlua::Result<Table> {
                if !(interval > 0.0) {
                    return Err(mlua::Error::RuntimeError(
                        "AnimationTrack:LinkToUpdate: interval must be > 0".into(),
                    ));
                }
                let signal = signal::new_instance(lua)?;
                let key = Arc::new(lua.create_registry_value(signal.clone())?);
                let mut t = this.track.lock().unwrap();
                t.update_links.push(UpdateLink {
                    interval,
                    next_fire: interval,
                    key,
                });
                Ok(signal)
            },
        );

        m.add_method("Reset", |_, this, _: ()| -> mlua::Result<()> {
            let baseline = {
                let mut t = this.track.lock().unwrap();
                t.state = TrackState::Stopped;
                t.time = 0.0;
                t.baseline.clone()
            };
            let mut p = this.part.lock().unwrap();
            if let Some(cf) = baseline.cframe {
                p.cframe = cf;
            }

            if let Some(verts) = baseline.vertices {
                let base_model = p.model.clone();
                let same_as_base = base_model
                    .as_ref()
                    .map(|m| Arc::ptr_eq(&m.vertices, &verts))
                    .unwrap_or(false);
                if same_as_base {
                    p.deformed = None;
                } else if let Some(m) = base_model {
                    p.deformed = Some(ModelRef {
                        id: m.id,
                        vertices: verts,
                        indices: m.indices.clone(),
                    });
                }
                p.deform_version = p.deform_version.wrapping_add(1);
            }
            Ok(())
        });

        m.add_method(
            "AddKeyframe",
            |_, this, args: MultiValue| -> mlua::Result<()> {
                let mut iter = args.into_iter();
                let time = match iter.next() {
                    Some(Value::Number(n)) => n as f32,
                    Some(Value::Integer(n)) => n as f32,
                    _ => {
                        return Err(mlua::Error::RuntimeError(
                            "AddKeyframe: first arg must be a number (time in seconds)".into(),
                        ));
                    }
                };
                let kind = match iter.next() {
                    Some(Value::String(s)) => s.to_str()?.to_string(),
                    _ => {
                        return Err(mlua::Error::RuntimeError(
                            "AddKeyframe: second arg must be \"CFrame\" or \"Deform\"".into(),
                        ));
                    }
                };
                let action = match kind.as_str() {
                    "CFrame" => {
                        let cf_ud = match iter.next() {
                            Some(Value::UserData(u)) => u,
                            _ => {
                                return Err(mlua::Error::RuntimeError(
                                    "AddKeyframe(\"CFrame\"): need a CFrame as the third arg"
                                        .into(),
                                ));
                            }
                        };
                        let cf = *cf_ud.borrow::<CFrame>()?;
                        KeyframeAction::Cframe(cf)
                    }
                    "Deform" => {
                        let cf_ud = match iter.next() {
                            Some(Value::UserData(u)) => u,
                            _ => {
                                return Err(mlua::Error::RuntimeError(
                                    "AddKeyframe(\"Deform\"): need (cframe, envelope, displacement) after the kind".into(),
                                ));
                            }
                        };
                        let envelope = match iter.next() {
                            Some(Value::Number(n)) => n as f32,
                            Some(Value::Integer(n)) => n as f32,
                            _ => {
                                return Err(mlua::Error::RuntimeError(
                                    "AddKeyframe(\"Deform\"): envelope must be a number".into(),
                                ));
                            }
                        };
                        let disp_val = iter.next().ok_or_else(|| {
                            mlua::Error::RuntimeError(
                                "AddKeyframe(\"Deform\"): displacement must be a Vector".into(),
                            )
                        })?;
                        let d = value_to_vector_opt(&disp_val).ok_or_else(|| {
                            mlua::Error::RuntimeError(
                                "AddKeyframe(\"Deform\"): displacement must be a Vector".into(),
                            )
                        })?;
                        let cf = *cf_ud.borrow::<CFrame>()?;
                        KeyframeAction::Deform {
                            center: [cf.position.x, cf.position.y, cf.position.z],
                            envelope,
                            displacement: [d.x, d.y, d.z],
                        }
                    }
                    other => {
                        return Err(mlua::Error::RuntimeError(format!(
                            "AddKeyframe: unknown kind '{other}' (expected \"CFrame\" or \"Deform\")"
                        )));
                    }
                };
                this.track
                    .lock()
                    .unwrap()
                    .add_keyframe(Keyframe { time, action });
                Ok(())
            },
        );

        m.add_method("ClearKeyframes", |_, this, _: ()| -> mlua::Result<()> {
            let mut t = this.track.lock().unwrap();
            t.keyframes.clear();
            t.duration = 0.0;
            Ok(())
        });
    }
}

fn populate_track_from_fbx(track: &mut AnimationTrack, clip: &mesh::FbxAnimClip) {
    track.duration = clip.duration;
    for (t, trans, rot) in &clip.samples {
        let pos = trans
            .map(|p| Vector::new(p[0], p[1], p[2]))
            .unwrap_or_else(|| Vector::new(0.0, 0.0, 0.0));
        let r = rot
            .map(|r| Vector::new(r[0], r[1], r[2]))
            .unwrap_or_else(|| Vector::new(0.0, 0.0, 0.0));
        track.keyframes.push(Keyframe {
            time: *t,
            action: KeyframeAction::Cframe(CFrame::new(pos, r)),
        });
    }
    track.keyframes.sort_by(|a, b| {
        a.time
            .partial_cmp(&b.time)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
}

pub fn tick_animations(lua: &Lua, dt: f32) {
    if dt <= 0.0 {
        return;
    }
    let parts = list_part_states();
    let any_tracks = parts
        .iter()
        .any(|p| !p.lock().unwrap().tracks.is_empty());
    if !any_tracks {
        return;
    }
    bump_parts_dirty();
    for part_arc in parts {
        let snapshot = {
            let p = part_arc.lock().unwrap();
            if p.tracks.is_empty() {
                continue;
            }
            let base_model = p.model.clone();
            let base_cframe = p.cframe;
            (
                p.tracks.clone(),
                base_cframe,
                base_model.as_ref().map(|m| m.vertices.clone()),
                base_model.as_ref().map(|m| m.indices.clone()),
                base_model.as_ref().map(|m| m.id),
            )
        };
        let (tracks, base_cframe, base_verts, base_indices, base_id) = snapshot;
        for tref in tracks {
            let mut track = tref.lock().unwrap();
            let baseline_cf = track.baseline.cframe.unwrap_or(base_cframe);
            let baseline_verts = track
                .baseline
                .vertices
                .clone()
                .or_else(|| base_verts.clone());
            let baseline_indices = base_indices.clone();
            let (Some(bv), Some(bi)) = (baseline_verts, baseline_indices) else {
                let eval = animation::tick_track(
                    &mut track,
                    dt,
                    baseline_cf,
                    Arc::new(Vec::new()),
                    Arc::new(Vec::new()),
                );
                drop(track);
                if let Some(cf) = eval.cframe_override {
                    let mut p = part_arc.lock().unwrap();
                    p.cframe = cf;
                }
                continue;
            };
            let eval = animation::tick_track(&mut track, dt, baseline_cf, bv, bi);
            let drained_events = std::mem::take(&mut track.pending_events);
            let played_key = track.played_key.clone();
            let stopped_key = track.stopped_key.clone();
            let did_loop_key = track.did_loop_key.clone();
            let update_links = track.update_links.clone();
            drop(track);

            for ev in drained_events {
                match ev {
                    TrackEvent::Played => {
                        if let Some(k) = &played_key {
                            fire_track_signal(lua, k, MultiValue::new());
                        }
                    }
                    TrackEvent::Stopped => {
                        if let Some(k) = &stopped_key {
                            fire_track_signal(lua, k, MultiValue::new());
                        }
                    }
                    TrackEvent::Looped => {
                        if let Some(k) = &did_loop_key {
                            fire_track_signal(lua, k, MultiValue::new());
                        }
                    }
                    TrackEvent::Update {
                        link_idx,
                        fire_time,
                    } => {
                        if let Some(link) = update_links.get(link_idx) {
                            let mut args = MultiValue::new();
                            args.push_back(Value::Number(fire_time));
                            fire_track_signal(lua, &link.key, args);
                        }
                    }
                }
            }

            let mut p = part_arc.lock().unwrap();
            if let Some(cf) = eval.cframe_override {
                p.cframe = cf;
            }
            if let Some(verts) = eval.vertices_override {
                let id = base_id.unwrap_or(0);
                let indices = eval.indices_for_normals.unwrap_or_else(|| {
                    p.model
                        .as_ref()
                        .map(|m| m.indices.clone())
                        .unwrap_or_else(|| Arc::new(Vec::new()))
                });
                p.deformed = Some(ModelRef {
                    id,
                    vertices: verts,
                    indices,
                });
                p.deform_version = p.deform_version.wrapping_add(1);
            }
        }
    }
}

pub struct CameraHandle;

impl UserData for CameraHandle {
    fn add_fields<F: UserDataFields<Self>>(f: &mut F) {
        f.add_field_method_get("CFrame", |_, _| Ok(CAMERA.with(|c| c.borrow().cframe)));
        f.add_field_method_set("CFrame", |_, _, value: AnyUserData| {
            let cf = *value
                .borrow::<CFrame>()
                .map_err(|_| mlua::Error::RuntimeError("CFrame expects a CFrame".into()))?;
            CAMERA.with(|c| c.borrow_mut().cframe = cf);
            bump_camera_dirty();
            Ok(())
        });
        f.add_field_method_get("FOV", |_, _| Ok(CAMERA.with(|c| c.borrow().fov_deg)));
        f.add_field_method_set("FOV", |_, _, value: f32| {
            CAMERA.with(|c| c.borrow_mut().fov_deg = value.clamp(1.0, 179.0));
            bump_camera_dirty();
            Ok(())
        });
        f.add_field_method_get("Near", |_, _| Ok(CAMERA.with(|c| c.borrow().near)));
        f.add_field_method_set("Near", |_, _, value: f32| {
            CAMERA.with(|c| c.borrow_mut().near = value.max(0.001));
            bump_camera_dirty();
            Ok(())
        });
        f.add_field_method_get("Far", |_, _| Ok(CAMERA.with(|c| c.borrow().far)));
        f.add_field_method_set("Far", |_, _, value: f32| {
            CAMERA.with(|c| c.borrow_mut().far = value.max(0.01));
            bump_camera_dirty();
            Ok(())
        });
    }
}

pub fn create(lua: &Lua) -> mlua::Result<Table> {
    let t = lua.create_table()?;

    t.set(
        "BasePart",
        lua.create_function(
            |lua, shape_name: Option<String>| -> mlua::Result<PartHandle> {
                let shape = match shape_name.as_deref().unwrap_or("Cube") {
                    "Cube" | "cube" | "Box" | "box" => PartShape::Cube,
                    "Sphere" | "sphere" | "Ball" | "ball" => PartShape::Sphere,
                    other => {
                        return Err(mlua::Error::RuntimeError(format!(
                            "BasePart: unknown shape '{other}' (try 'Cube' or 'Sphere')"
                        )));
                    }
                };
                PartHandle::new_shape(lua, shape, None)
            },
        )?,
    )?;

    t.set(
        "BaseModel",
        lua.create_function(|lua, asset: AnyUserData| -> mlua::Result<PartHandle> {
            let ma = asset.borrow::<ModelAsset>().map_err(|_| {
                mlua::Error::RuntimeError(
                    "BaseModel expects a ModelAsset (Asset.GetAsset(\"Model\", ...))".into(),
                )
            })?;
            let model = ModelRef {
                id: ma.id,
                vertices: ma.vertices.clone(),
                indices: ma.indices.clone(),
            };
            let anims = if ma.animations.is_empty() {
                None
            } else {
                Some(ma.animations.clone())
            };
            PartHandle::new_shape_with(lua, PartShape::Model, Some(model), anims)
        })?,
    )?;

    t.set("Camera", lua.create_userdata(CameraHandle)?)?;

    t.set(
        "EffectVolume",
        lua.create_function(
            |_, image: Option<AnyUserData>| -> mlua::Result<EffectVolumeHandle> {
                effect_volume::new_effect_volume(image)
            },
        )?,
    )?;

    t.set(
        "DistortionBox",
        lua.create_function(
            |lua,
             (part, cf, size): (AnyUserData, AnyUserData, Vector)|
             -> mlua::Result<AnyUserData> {
                let h = part.borrow::<PartHandle>().map_err(|_| {
                    mlua::Error::RuntimeError(
                        "DistortionBox: first argument must be a BasePart".into(),
                    )
                })?;
                let cf = *cf.borrow::<CFrame>().map_err(|_| {
                    mlua::Error::RuntimeError(
                        "DistortionBox: second argument must be a CFrame".into(),
                    )
                })?;
                let captured = {
                    let p = h.state.lock().unwrap();
                    let model = p.model.as_ref().ok_or_else(|| {
                        mlua::Error::RuntimeError(
                            "DistortionBox: target part has no mesh (only BaseModel parts can be distorted)".into(),
                        )
                    })?;
                    capture_distortion_vertices(&model.vertices, cf, size)
                };
                let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
                let state = Arc::new(Mutex::new(DistortionBoxState {
                    id,
                    target: h.state.clone(),
                    alive: true,
                    initial_cf: cf,
                    initial_size: size,
                    current_cf: cf,
                    current_size: size,
                    last_applied_cf: None,
                    last_applied_size: None,
                    captured,
                }));
                DISTORTION_BOXES.with(|c| c.borrow_mut().push(state.clone()));
                lua.create_userdata(DistortionBoxHandle { inner: state })
            },
        )?,
    )?;

    t.set(
        "SimplifyMesh",
        lua.create_function(
            |lua, (asset, ratio): (AnyUserData, Option<f32>)| -> mlua::Result<AnyUserData> {
                let ma = asset.borrow::<ModelAsset>().map_err(|_| {
                    mlua::Error::RuntimeError(
                        "SimplifyMesh: first arg must be a ModelAsset (Asset.GetAsset(\"Model\", ...))".into(),
                    )
                })?;
                let r = ratio.unwrap_or(0.5).clamp(0.01, 1.0);
                let input = mesh::Mesh {
                    vertices: (*ma.vertices).clone(),
                    indices: (*ma.indices).clone(),
                };
                let simplified = mesh::simplify(&input, r);
                let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
                let new_asset = ModelAsset {
                    id,
                    vertices: Arc::new(simplified.vertices),
                    indices: Arc::new(simplified.indices),

animations: ma.animations.clone(),
                    source: format!("{} (LOD {:.0}%)", ma.source, r * 100.0),
                };
                lua.create_userdata(new_asset)
            },
        )?,
    )?;

    Ok(t)
}

pub mod render;
