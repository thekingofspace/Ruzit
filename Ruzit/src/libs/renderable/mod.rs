

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use mlua::{
    AnyUserData, Lua, MultiValue, Table, UserData, UserDataFields, UserDataMethods, Value,
};

use crate::libs::asset::{FragmentAsset, ImageAsset, ModelAsset, ShaderAsset};
use crate::libs::primitives::{CFrame, Color3, Vector};
use crate::libs::signal;

pub mod mesh;

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
    
    
    pub texture: Option<PartTextureRef>,
}

thread_local! {
    static PARTS: RefCell<Vec<Arc<Mutex<PartState>>>> = const { RefCell::new(Vec::new()) };
    static CAMERA: RefCell<CameraState> = RefCell::new(CameraState::default());
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
            cframe: CFrame::new(
                Vector::new(4.0, 3.0, 5.0),
                Vector::new(-0.4, 0.65, 0.0),
            ),
            fov_deg: 60.0,
            near: 0.1,
            far: 1000.0,
        }
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

pub struct PartRender {
    pub shape: PartShape,
    pub cframe: CFrame,
    pub size: Vector,
    pub color: Color3,
    pub active_shader: Option<AttachedShader3D>,
    pub model: Option<ModelRef>,
    pub texture: Option<PartTextureRef>,
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
                    model: s.model.clone(),
                    texture: s.texture.clone(),
                })
            })
            .collect()
    })
}


pub struct PartHandle {
    state: Arc<Mutex<PartState>>,
}

impl PartHandle {
    fn new_shape(lua: &Lua, shape: PartShape, model: Option<ModelRef>) -> mlua::Result<Self> {
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        let changed_signal = signal::new_instance(lua)?;
        let state = Arc::new(Mutex::new(PartState {
            id,
            shape,
            cframe: CFrame::new(Vector::new(0.0, 0.0, 0.0), Vector::new(0.0, 0.0, 0.0)),
            size: Vector::new(1.0, 1.0, 1.0),
            color: Color3::new(255, 255, 255),
            render: true,
            alive: true,
            attached: Vec::new(),
            changed_signal,
            model,
            texture: None,
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
                    mlua::Error::RuntimeError(
                        "SetData: shader is not attached to this part".into(),
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
                let s = this.state.lock().unwrap();
                let entry = s.attached.iter().find(|e| e.id == id).ok_or_else(|| {
                    mlua::Error::RuntimeError(
                        "GetData: shader is not attached to this part".into(),
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


pub struct CameraHandle;

impl UserData for CameraHandle {
    fn add_fields<F: UserDataFields<Self>>(f: &mut F) {
        f.add_field_method_get("CFrame", |_, _| {
            Ok(CAMERA.with(|c| c.borrow().cframe))
        });
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
        lua.create_function(|lua, shape_name: Option<String>| -> mlua::Result<PartHandle> {
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
        })?,
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
            PartHandle::new_shape(lua, PartShape::Model, Some(model))
        })?,
    )?;

    t.set("Camera", lua.create_userdata(CameraHandle)?)?;

    Ok(t)
}

pub mod render;
