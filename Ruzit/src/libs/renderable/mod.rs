use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use mlua::{AnyUserData, Lua, MultiValue, Table, UserData, UserDataFields, UserDataMethods, Value};

use crate::libs::asset::{FragmentAsset, ImageAsset, ModelAsset, ShaderAsset};
use crate::libs::primitives::{CFrame, Color3, Vector};
use crate::libs::signal;

pub mod animation;
pub mod mesh;

use animation::{AnimationTrack, Keyframe, KeyframeAction, TrackBaseline, TrackRef, TrackState};

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
    /// Per-part deformed vertex data; takes precedence over `model.vertices`
    /// at render time. Lazily initialised on the first `Deform` call so parts
    /// that never deform pay nothing extra. Bumping `deform_version` is what
    /// tells the renderer's bind-group cache to re-upload.
    pub deformed: Option<ModelRef>,
    pub deform_version: u64,

    pub texture: Option<PartTextureRef>,

    pub cast_shadow: bool,
    pub receive_shadow: bool,
    pub ignore_raycast: bool,

    /// Tracks owned by this part. Indexed by name from the Lua side so the
    /// same track is returned for repeated `:GetTrack("walk")` calls — the
    /// AnimationTrack userdata carries the Arc<Mutex<...>>.
    pub tracks: Vec<TrackRef>,

    /// FBX-parsed animation clips inherited from the source ModelAsset.
    /// `:GetTrack(name)` consults this when materialising a new track so
    /// the keyframes are pre-populated from the FBX file. None for
    /// non-FBX models or for synthesised meshes (Cube/Sphere).
    pub source_animations: Option<Arc<Vec<mesh::FbxAnimClip>>>,
}

thread_local! {
    static PARTS: RefCell<Vec<Arc<Mutex<PartState>>>> = const { RefCell::new(Vec::new()) };
    static CAMERA: RefCell<CameraState> = RefCell::new(CameraState::default());
    static LIGHTING: RefCell<LightingState> = RefCell::new(LightingState::default());
    static FRAME_INDEX: RefCell<u32> = const { RefCell::new(0) };
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
}

pub fn snapshot() -> Vec<PartRender> {
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
                    cframe: s.cframe,
                    size: s.size,
                    color: s.color,
                    active_shader: s.attached.last().cloned(),

                    model: s.deformed.clone().or_else(|| s.model.clone()),
                    texture: s.texture.clone(),
                    cast_shadow: s.cast_shadow,
                    receive_shadow: s.receive_shadow,
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

    fn new_shape(lua: &Lua, shape: PartShape, model: Option<ModelRef>) -> mlua::Result<Self> {
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
            tracks: Vec::new(),
            source_animations,
        }));
        PARTS.with(|cell| cell.borrow_mut().push(state.clone()));
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
    let mut args = MultiValue::new();
    args.push_back(Value::String(lua.create_string(prop)?));
    signal::fire(lua, &signal_table, args)
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

fn build_attached_3d(asset: &AnyUserData) -> mlua::Result<AttachedShader3D> {
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

fn shader_asset_id(asset: &AnyUserData) -> mlua::Result<u64> {
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

        f.add_field_method_get("CFrame", |_, this| Ok(this.state.lock().unwrap().cframe));
        f.add_field_method_set("CFrame", |lua, this, value: AnyUserData| {
            this.ensure_alive("set CFrame")?;
            let cf = *value
                .borrow::<CFrame>()
                .map_err(|_| mlua::Error::RuntimeError("CFrame expects a CFrame".into()))?;
            let sig = {
                let mut s = this.state.lock().unwrap();
                s.cframe = cf;
                s.changed_signal.clone()
            };
            fire_changed(lua, sig, "CFrame")
        });

        f.add_field_method_get("Size", |_, this| Ok(this.state.lock().unwrap().size));
        f.add_field_method_set("Size", |lua, this, value: AnyUserData| {
            this.ensure_alive("set Size")?;
            let v = *value
                .borrow::<Vector>()
                .map_err(|_| mlua::Error::RuntimeError("Size expects a Vector".into()))?;
            let sig = {
                let mut s = this.state.lock().unwrap();
                s.size = v;
                s.changed_signal.clone()
            };
            fire_changed(lua, sig, "Size")
        });

        f.add_field_method_get("Color", |_, this| Ok(this.state.lock().unwrap().color));
        f.add_field_method_set("Color", |lua, this, value: AnyUserData| {
            this.ensure_alive("set Color")?;
            let c = *value
                .borrow::<Color3>()
                .map_err(|_| mlua::Error::RuntimeError("Color expects a Color3".into()))?;
            let sig = {
                let mut s = this.state.lock().unwrap();
                s.color = c;
                s.changed_signal.clone()
            };
            fire_changed(lua, sig, "Color")
        });

        f.add_field_method_get("Render", |_, this| Ok(this.state.lock().unwrap().render));
        f.add_field_method_set("Render", |lua, this, value: bool| {
            this.ensure_alive("set Render")?;
            let sig = {
                let mut s = this.state.lock().unwrap();
                s.render = value;
                s.changed_signal.clone()
            };
            fire_changed(lua, sig, "Render")
        });

        f.add_field_method_get("CastShadow", |_, this| {
            Ok(this.state.lock().unwrap().cast_shadow)
        });
        f.add_field_method_set("CastShadow", |lua, this, value: bool| {
            this.ensure_alive("set CastShadow")?;
            let sig = {
                let mut s = this.state.lock().unwrap();
                s.cast_shadow = value;
                s.changed_signal.clone()
            };
            fire_changed(lua, sig, "CastShadow")
        });

        f.add_field_method_get("ReceiveShadow", |_, this| {
            Ok(this.state.lock().unwrap().receive_shadow)
        });
        f.add_field_method_set("ReceiveShadow", |lua, this, value: bool| {
            this.ensure_alive("set ReceiveShadow")?;
            let sig = {
                let mut s = this.state.lock().unwrap();
                s.receive_shadow = value;
                s.changed_signal.clone()
            };
            fire_changed(lua, sig, "ReceiveShadow")
        });

        f.add_field_method_get("IgnoreInRaycast", |_, this| {
            Ok(this.state.lock().unwrap().ignore_raycast)
        });
        f.add_field_method_set("IgnoreInRaycast", |lua, this, value: bool| {
            this.ensure_alive("set IgnoreInRaycast")?;
            let sig = {
                let mut s = this.state.lock().unwrap();
                s.ignore_raycast = value;
                s.changed_signal.clone()
            };
            fire_changed(lua, sig, "IgnoreInRaycast")
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
                    let img = ud.borrow::<ImageAsset>().map_err(|_| {
                        mlua::Error::RuntimeError(
                            "Texture expects an ImageAsset (Asset.GetAsset(\"Image\", ...)) or nil"
                                .into(),
                        )
                    })?;
                    Some(PartTextureRef {
                        id: img.id,
                        width: img.width,
                        height: img.height,
                        data: img.data.clone(),
                    })
                }
                _ => {
                    return Err(mlua::Error::RuntimeError(
                        "Texture expects an ImageAsset or nil".into(),
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
            |_, this, (target, envelope, disp): (AnyUserData, f32, AnyUserData)| -> mlua::Result<u32> {
                this.ensure_alive("Deform")?;
                let cf = *target.borrow::<CFrame>().map_err(|_| {
                    mlua::Error::RuntimeError(
                        "Deform: first argument must be a CFrame (target point on the mesh)".into(),
                    )
                })?;
                let d = *disp.borrow::<Vector>().map_err(|_| {
                    mlua::Error::RuntimeError(
                        "Deform: third argument must be a Vector (displacement)".into(),
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
    }
}

/// Userdata wrapper around `Arc<Mutex<AnimationTrack>>` plus the part it's
/// attached to (needed so Play()/Reset() can read the part's CFrame and
/// model as a baseline).
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
            t.state = TrackState::Playing;
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
            t.state = TrackState::Stopped;
            t.time = 0.0;
            Ok(())
        });

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
                        let disp_ud = match iter.next() {
                            Some(Value::UserData(u)) => u,
                            _ => {
                                return Err(mlua::Error::RuntimeError(
                                    "AddKeyframe(\"Deform\"): displacement must be a Vector"
                                        .into(),
                                ));
                            }
                        };
                        let cf = *cf_ud.borrow::<CFrame>()?;
                        let d = *disp_ud.borrow::<Vector>()?;
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

/// Translate an FBX-extracted animation clip into AnimationTrack keyframes.
/// Each FBX sample becomes one CFrame keyframe whose position is the FBX
/// translation channel and whose rotation is the FBX Euler-degree channel
/// (we store rotation in the same XYZ-degree convention `CFrame.new` already
/// uses, so no extra conversion is needed). Looped is left to the user —
/// the FBX file format has no "should this clip loop" flag.
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

/// Tick every track attached to every alive part by `dt` seconds. Called
/// once per frame from the heart loop. Multi-track parts are evaluated in
/// insertion order and later tracks overwrite earlier ones — the simplest
/// behavior that's still useful, with proper blending left for later.
pub fn tick_animations(dt: f32) {
    if dt <= 0.0 {
        return;
    }
    let parts = list_part_states();
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
            drop(track);

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
            Ok(())
        });
        f.add_field_method_get("FOV", |_, _| Ok(CAMERA.with(|c| c.borrow().fov_deg)));
        f.add_field_method_set("FOV", |_, _, value: f32| {
            CAMERA.with(|c| c.borrow_mut().fov_deg = value.clamp(1.0, 179.0));
            Ok(())
        });
        f.add_field_method_get("Near", |_, _| Ok(CAMERA.with(|c| c.borrow().near)));
        f.add_field_method_set("Near", |_, _, value: f32| {
            CAMERA.with(|c| c.borrow_mut().near = value.max(0.001));
            Ok(())
        });
        f.add_field_method_get("Far", |_, _| Ok(CAMERA.with(|c| c.borrow().far)));
        f.add_field_method_set("Far", |_, _, value: f32| {
            CAMERA.with(|c| c.borrow_mut().far = value.max(0.01));
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

    t.set(
        "SetSun",
        lua.create_function(
            |_, (dir, color): (AnyUserData, Option<AnyUserData>)| -> mlua::Result<()> {
                let d = *dir.borrow::<Vector>()?;
                let c = match color {
                    Some(ud) => Some(*ud.borrow::<Color3>()?),
                    None => None,
                };
                LIGHTING.with(|cell| {
                    let mut s = cell.borrow_mut();
                    s.sun_direction = d;
                    if let Some(c) = c {
                        s.sun_color = c;
                    }
                });
                Ok(())
            },
        )?,
    )?;
    t.set(
        "SetSunColor",
        lua.create_function(|_, color: AnyUserData| -> mlua::Result<()> {
            let c = *color.borrow::<Color3>()?;
            LIGHTING.with(|cell| cell.borrow_mut().sun_color = c);
            Ok(())
        })?,
    )?;
    t.set(
        "SetAmbient",
        lua.create_function(|_, color: AnyUserData| -> mlua::Result<()> {
            let c = *color.borrow::<Color3>()?;
            LIGHTING.with(|cell| cell.borrow_mut().ambient = c);
            Ok(())
        })?,
    )?;
    t.set(
        "GetLighting",
        lua.create_function(|lua, _: ()| -> mlua::Result<Table> {
            let snap = LIGHTING.with(|c| *c.borrow());
            let out = lua.create_table()?;
            out.set("SunDirection", snap.sun_direction)?;
            out.set("SunColor", snap.sun_color)?;
            out.set("Ambient", snap.ambient)?;
            Ok(out)
        })?,
    )?;

    Ok(t)
}

pub mod render;
