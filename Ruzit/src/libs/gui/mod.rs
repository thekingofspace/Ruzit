use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use mlua::{
    AnyUserData, Lua, MultiValue, Table, UserData, UserDataFields, UserDataMethods, Value,
};

use crate::libs::asset::{FragmentAsset, ImageAsset, ShaderAsset};
use crate::libs::primitives::{Color3, Dim};
use crate::libs::signal;

pub mod render;

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

thread_local! {
    /// Live primitives, in insertion order. Render order is determined by
    /// `z_index`, computed at draw time.
    static REGISTRY: RefCell<Vec<Arc<Mutex<PrimitiveState>>>> = const { RefCell::new(Vec::new()) };

    /// Optional scene-wide shaders. Skybox draws before primitives, post-effect
    /// after (sampling the rendered scene as a texture). Each slot holds at
    /// most one active shader.
    static SKYBOX: RefCell<Option<Arc<SceneShaderState>>> = const { RefCell::new(None) };
    static POST_EFFECT: RefCell<Option<Arc<SceneShaderState>>> = const { RefCell::new(None) };
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shape {
    Square,
    Circle,
    Triangle,
    /// Full-quad rectangle that samples a texture in the fragment shader.
    /// `ruzit_inside_shape(uv, shape=3)` always returns true so there's no
    /// geometric mask — clipping is the texture's job (alpha = 0 anywhere
    /// you don't want a pixel).
    Image,
}

impl Shape {
    pub fn shape_id(self) -> u32 {
        match self {
            Self::Square => 0,
            Self::Circle => 1,
            Self::Triangle => 2,
            Self::Image => 3,
        }
    }
}

/// Reference to a texture-bound image. The GPU caches by `id`, so multiple
/// primitives sharing the same `ImageAsset` upload only once. `data` is held
/// even after the upload so a future GpuState can re-upload (the primitive
/// outlives the GPU surface across resizes etc.).
pub struct ImageRef {
    pub id: u64,
    pub width: u32,
    pub height: u32,
    pub data: Arc<Vec<u8>>,
}

/// One WGSL fragment shader the user attached. Holds the parsed param-name →
/// slot map and the live values (16 floats packed into 4 vec4s on the GPU).
#[derive(Clone)]
pub struct AttachedShader {
    pub id: u64,
    #[allow(dead_code)]
    pub source: String,
    /// Full WGSL fragment-stage source the engine should compile (prelude +
    /// user code).
    pub wgsl: Arc<String>,
    /// Lookup: param name → linear slot in [0, 16).
    pub slot_of_name: Arc<std::collections::HashMap<String, u8>>,
    /// 16 floats packed into 4 vec4s. Mutated by `:SetData` from the Lua
    /// thread; read by the renderer on each draw.
    pub params: Arc<Mutex<[f32; 16]>>,
}

pub struct PrimitiveState {
    #[allow(dead_code)]
    pub id: u64,
    pub shape: Shape,
    pub size: Dim,
    pub position: Dim,
    pub color: Color3,
    pub transparency: f32,
    pub z_index: i32,
    pub visible: bool,
    pub alive: bool,
    /// All currently-attached shaders, in the order :AttachShader was called.
    /// Only the last one is active at render time — composing multiple
    /// fragment shaders into a single pipeline is out of scope for now.
    pub attached: Vec<AttachedShader>,
    pub changed_signal: Table,
    /// Set for `Shape::Image` primitives only; renderer uploads + binds it.
    pub image: Option<Arc<ImageRef>>,
}

/// Render snapshot of one primitive. Cloned on the Lua thread, consumed on
/// the same thread by the renderer so Lua handlers can't mutate state
/// mid-render.
pub struct RenderItem {
    pub shape: Shape,
    pub size: Dim,
    pub position: Dim,
    pub color: Color3,
    pub transparency: f32,
    pub z_index: i32,
    /// The active shader (last attached), if any.
    pub active_shader: Option<AttachedShader>,
    pub image: Option<Arc<ImageRef>>,
}

pub fn snapshot() -> Vec<RenderItem> {
    REGISTRY.with(|cell| {
        let mut reg = cell.borrow_mut();
        reg.retain(|p| p.lock().unwrap().alive);
        let mut out: Vec<RenderItem> = reg
            .iter()
            .filter_map(|p| {
                let s = p.lock().unwrap();
                if !s.visible {
                    return None;
                }
                Some(RenderItem {
                    shape: s.shape,
                    size: s.size,
                    position: s.position,
                    color: s.color,
                    transparency: s.transparency,
                    z_index: s.z_index,
                    active_shader: s.attached.last().cloned(),
                    image: s.image.clone(),
                })
            })
            .collect();
        out.sort_by_key(|r| r.z_index);
        out
    })
}

pub struct GuiPrimitive {
    state: Arc<Mutex<PrimitiveState>>,
}

impl GuiPrimitive {
    fn new(lua: &Lua, shape: Shape) -> mlua::Result<Self> {
        Self::with_state(lua, shape, None, Dim::new(100.0, 100.0))
    }

    fn new_image(lua: &Lua, asset: &ImageAsset) -> mlua::Result<Self> {
        let image = ImageRef {
            id: asset.id,
            width: asset.width,
            height: asset.height,
            data: asset.data.clone(),
        };
        // Default the size to the image's pixel dimensions — feels natural
        // for `Asset.GetAsset("Image", ...) → GUI.Basic.Image(asset)` to come
        // up at the asset's native size; user can resize via .Size.
        let size = Dim::new(asset.width as f32, asset.height as f32);
        Self::with_state(lua, Shape::Image, Some(Arc::new(image)), size)
    }

    fn with_state(
        lua: &Lua,
        shape: Shape,
        image: Option<Arc<ImageRef>>,
        size: Dim,
    ) -> mlua::Result<Self> {
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        let changed_signal = signal::new_instance(lua)?;
        let state = Arc::new(Mutex::new(PrimitiveState {
            id,
            shape,
            size,
            position: Dim::new(0.0, 0.0),
            color: Color3::new(255, 255, 255),
            transparency: 0.0,
            z_index: 0,
            visible: true,
            alive: true,
            attached: Vec::new(),
            changed_signal,
            image,
        }));
        REGISTRY.with(|cell| cell.borrow_mut().push(state.clone()));
        Ok(Self { state })
    }

    fn ensure_alive(&self, op: &str) -> mlua::Result<()> {
        let s = self.state.lock().unwrap();
        if !s.alive {
            return Err(mlua::Error::RuntimeError(format!(
                "GUI: {op} called on a destroyed primitive"
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

/// Compile a `Shader` or `Fragment` asset into an `AttachedShader`. The asset's
/// text is prefixed with the engine prelude (uniforms, varyings, helpers) so
/// user code can refer to `U`, `VsOut`, `p(...)`, and `ruzit_inside_shape`.
fn build_attached(asset: &AnyUserData) -> mlua::Result<AttachedShader> {
    let (id, source, code) = if let Ok(s) = asset.borrow::<ShaderAsset>() {
        (s.id, s.source.clone(), s.code.clone())
    } else if let Ok(f) = asset.borrow::<FragmentAsset>() {
        (f.id, f.source.clone(), f.code.clone())
    } else {
        return Err(mlua::Error::RuntimeError(
            "expected a Shader or Fragment asset".into(),
        ));
    };

    let slot_of_name = parse_param_decls(&code);
    let prelude = render::FRAGMENT_PRELUDE;
    let wgsl = format!("{prelude}\n{code}");

    Ok(AttachedShader {
        id,
        source,
        wgsl: Arc::new(wgsl),
        slot_of_name: Arc::new(slot_of_name),
        params: Arc::new(Mutex::new([0.0_f32; 16])),
    })
}

/// Parse `// @ruzit param NAME` lines (in declaration order) and assign each
/// a linear slot in [0, 16). Slots beyond 15 are silently dropped.
fn parse_param_decls(src: &str) -> std::collections::HashMap<String, u8> {
    let mut map = std::collections::HashMap::new();
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
        // Take the first whitespace-delimited token after `param`. Anything
        // after that (including a stray `*/` from a block comment) is fine to
        // ignore.
        let name = rest.split_whitespace().next().unwrap_or("").to_string();
        if name.is_empty() {
            continue;
        }
        if next_slot >= 16 {
            eprintln!("[GUI] shader has more than 16 @ruzit params; '{name}' ignored");
            continue;
        }
        map.entry(name).or_insert(next_slot);
        next_slot += 1;
    }
    map
}

impl UserData for GuiPrimitive {
    fn add_fields<F: UserDataFields<Self>>(f: &mut F) {
        f.add_field_method_get("Changed", |_, this| {
            Ok(this.state.lock().unwrap().changed_signal.clone())
        });
        f.add_field_method_get("Size", |_, this| Ok(this.state.lock().unwrap().size));
        f.add_field_method_set("Size", |lua, this, value: AnyUserData| {
            this.ensure_alive("set Size")?;
            let dim = *value
                .borrow::<Dim>()
                .map_err(|_| mlua::Error::RuntimeError("Size expects a Primitives.Dim".into()))?;
            let signal_table = {
                let mut s = this.state.lock().unwrap();
                s.size = dim;
                s.changed_signal.clone()
            };
            fire_changed(lua, signal_table, "Size")
        });
        f.add_field_method_get("Position", |_, this| {
            Ok(this.state.lock().unwrap().position)
        });
        f.add_field_method_set("Position", |lua, this, value: AnyUserData| {
            this.ensure_alive("set Position")?;
            let dim = *value.borrow::<Dim>().map_err(|_| {
                mlua::Error::RuntimeError("Position expects a Primitives.Dim".into())
            })?;
            let signal_table = {
                let mut s = this.state.lock().unwrap();
                s.position = dim;
                s.changed_signal.clone()
            };
            fire_changed(lua, signal_table, "Position")
        });
        f.add_field_method_get("Color", |_, this| Ok(this.state.lock().unwrap().color));
        f.add_field_method_set("Color", |lua, this, value: AnyUserData| {
            this.ensure_alive("set Color")?;
            let color = *value.borrow::<Color3>().map_err(|_| {
                mlua::Error::RuntimeError("Color expects a Primitives.Color3".into())
            })?;
            let signal_table = {
                let mut s = this.state.lock().unwrap();
                s.color = color;
                s.changed_signal.clone()
            };
            fire_changed(lua, signal_table, "Color")
        });
        f.add_field_method_get("Transparency", |_, this| {
            Ok(this.state.lock().unwrap().transparency)
        });
        f.add_field_method_set("Transparency", |lua, this, value: f32| {
            this.ensure_alive("set Transparency")?;
            let signal_table = {
                let mut s = this.state.lock().unwrap();
                s.transparency = value.clamp(0.0, 1.0);
                s.changed_signal.clone()
            };
            fire_changed(lua, signal_table, "Transparency")
        });
        f.add_field_method_get("ZIndex", |_, this| {
            Ok(this.state.lock().unwrap().z_index as i64)
        });
        f.add_field_method_set("ZIndex", |lua, this, value: i64| {
            this.ensure_alive("set ZIndex")?;
            let signal_table = {
                let mut s = this.state.lock().unwrap();
                s.z_index = value.clamp(i32::MIN as i64, i32::MAX as i64) as i32;
                s.changed_signal.clone()
            };
            fire_changed(lua, signal_table, "ZIndex")
        });
        f.add_field_method_get("Visible", |_, this| Ok(this.state.lock().unwrap().visible));
        f.add_field_method_set("Visible", |lua, this, value: bool| {
            this.ensure_alive("set Visible")?;
            let signal_table = {
                let mut s = this.state.lock().unwrap();
                s.visible = value;
                s.changed_signal.clone()
            };
            fire_changed(lua, signal_table, "Visible")
        });
        f.add_field_method_get("Shape", |_, this| {
            Ok(match this.state.lock().unwrap().shape {
                Shape::Circle => "Circle",
                Shape::Square => "Square",
                Shape::Triangle => "Triangle",
                Shape::Image => "Image",
            })
        });
    }

    fn add_methods<M: UserDataMethods<Self>>(m: &mut M) {
        m.add_method("Destroy", |lua, this, _: ()| -> mlua::Result<()> {
            let signal_table = {
                let mut s = this.state.lock().unwrap();
                if !s.alive {
                    return Ok(());
                }
                s.alive = false;
                s.visible = false;
                s.attached.clear();
                s.changed_signal.clone()
            };
            fire_changed(lua, signal_table, "Destroyed")
        });

        m.add_method(
            "AttachShader",
            |_, this, asset: AnyUserData| -> mlua::Result<()> {
                this.ensure_alive("AttachShader")?;
                let attached = build_attached(&asset)?;
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
        m.add_method(
            "SetData",
            |_, this, (asset, name, value): (AnyUserData, String, f32)| -> mlua::Result<()> {
                this.ensure_alive("SetData")?;
                let id = shader_asset_id(&asset)?;
                let s = this.state.lock().unwrap();
                let entry = s.attached.iter().find(|e| e.id == id).ok_or_else(|| {
                    mlua::Error::RuntimeError(
                        "SetData: shader is not attached to this primitive".into(),
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
                        "GetData: shader is not attached to this primitive".into(),
                    )
                })?;
                let Some(slot) = entry.slot_of_name.get(&name) else {
                    return Ok(None);
                };
                Ok(Some(entry.params.lock().unwrap()[*slot as usize]))
            },
        );
        m.add_method("ClearShaders", |_, this, _: ()| -> mlua::Result<()> {
            this.ensure_alive("ClearShaders")?;
            this.state.lock().unwrap().attached.clear();
            Ok(())
        });
    }
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

// ---------------------------------------------------------------------------
// Scene-wide shaders: Skybox + PostEffect
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SceneSlot {
    Skybox,
    PostEffect,
}

/// Scene-level WGSL shader. Skybox runs before primitives at fullscreen
/// (uv ∈ [0,1] across the window, IMG bound to a 1×1 white texture).
/// PostEffect runs after primitives and samples the rendered scene as IMG.
pub struct SceneShaderState {
    pub id: u64,
    pub wgsl: Arc<String>,
    pub slot_of_name: Arc<HashMap<String, u8>>,
    pub params: Arc<Mutex<[f32; 16]>>,
}

pub fn skybox_snapshot() -> Option<Arc<SceneShaderState>> {
    SKYBOX.with(|c| c.borrow().as_ref().cloned())
}

pub fn post_effect_snapshot() -> Option<Arc<SceneShaderState>> {
    POST_EFFECT.with(|c| c.borrow().as_ref().cloned())
}

fn build_scene_shader(asset: &AnyUserData) -> mlua::Result<Arc<SceneShaderState>> {
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
    // Same prelude as primitives — IMG/IMG_SAMP and the params helper carry
    // the same meaning. Skybox just gets a white IMG and a fullscreen quad
    // vertex; PostEffect gets the scene render as IMG.
    let prelude = render::FRAGMENT_PRELUDE;
    let wgsl = format!("{prelude}\n{code}");
    Ok(Arc::new(SceneShaderState {
        id,
        wgsl: Arc::new(wgsl),
        slot_of_name: Arc::new(slot_of_name),
        params: Arc::new(Mutex::new([0.0_f32; 16])),
    }))
}

/// Lua-facing handle. Holds an Arc so :SetData mutations on the userdata
/// reach the renderer through the shared params Mutex.
pub struct SceneShader {
    slot: SceneSlot,
    state: Arc<SceneShaderState>,
}

impl SceneShader {
    fn current_in_slot(&self) -> bool {
        let cur = match self.slot {
            SceneSlot::Skybox => SKYBOX.with(|c| c.borrow().as_ref().map(|s| s.id)),
            SceneSlot::PostEffect => POST_EFFECT.with(|c| c.borrow().as_ref().map(|s| s.id)),
        };
        cur == Some(self.state.id)
    }
}

impl UserData for SceneShader {
    fn add_methods<M: UserDataMethods<Self>>(m: &mut M) {
        m.add_method(
            "SetData",
            |_, this, (name, value): (String, f32)| -> mlua::Result<()> {
                let slot = *this.state.slot_of_name.get(&name).ok_or_else(|| {
                    mlua::Error::RuntimeError(format!(
                        "SetData: '{name}' is not a `// @ruzit param` declared in the shader"
                    ))
                })?;
                this.state.params.lock().unwrap()[slot as usize] = value;
                Ok(())
            },
        );
        m.add_method(
            "GetData",
            |_, this, name: String| -> mlua::Result<Option<f32>> {
                let Some(slot) = this.state.slot_of_name.get(&name) else {
                    return Ok(None);
                };
                Ok(Some(this.state.params.lock().unwrap()[*slot as usize]))
            },
        );
        // Destroy clears whichever scene slot we own — no-op if a different
        // shader has been assigned in the meantime.
        m.add_method("Destroy", |_, this, _: ()| -> mlua::Result<()> {
            if this.current_in_slot() {
                match this.slot {
                    SceneSlot::Skybox => SKYBOX.with(|c| *c.borrow_mut() = None),
                    SceneSlot::PostEffect => POST_EFFECT.with(|c| *c.borrow_mut() = None),
                }
            }
            Ok(())
        });
    }
}

fn install_scene_shader(slot: SceneSlot, state: Arc<SceneShaderState>) {
    match slot {
        SceneSlot::Skybox => SKYBOX.with(|c| *c.borrow_mut() = Some(state)),
        SceneSlot::PostEffect => POST_EFFECT.with(|c| *c.borrow_mut() = Some(state)),
    }
}

pub fn create(lua: &Lua) -> mlua::Result<Table> {
    let t = lua.create_table()?;

    let basic = lua.create_table()?;
    basic.set(
        "Circle",
        lua.create_function(|lua, _: ()| GuiPrimitive::new(lua, Shape::Circle))?,
    )?;
    basic.set(
        "Square",
        lua.create_function(|lua, _: ()| GuiPrimitive::new(lua, Shape::Square))?,
    )?;
    basic.set(
        "Triangle",
        lua.create_function(|lua, _: ()| GuiPrimitive::new(lua, Shape::Triangle))?,
    )?;
    basic.set(
        "Image",
        lua.create_function(|lua, asset: AnyUserData| -> mlua::Result<GuiPrimitive> {
            let img = asset.borrow::<ImageAsset>().map_err(|_| {
                mlua::Error::RuntimeError(
                    "GUI.Basic.Image expects an ImageAsset (Asset.GetAsset(\"Image\", ...))".into(),
                )
            })?;
            GuiPrimitive::new_image(lua, &img)
        })?,
    )?;
    t.set("Basic", basic)?;

    t.set(
        "SetSkybox",
        lua.create_function(|_, asset: AnyUserData| -> mlua::Result<SceneShader> {
            let state = build_scene_shader(&asset)?;
            install_scene_shader(SceneSlot::Skybox, state.clone());
            Ok(SceneShader {
                slot: SceneSlot::Skybox,
                state,
            })
        })?,
    )?;
    t.set(
        "ClearSkybox",
        lua.create_function(|_, _: ()| -> mlua::Result<()> {
            SKYBOX.with(|c| *c.borrow_mut() = None);
            Ok(())
        })?,
    )?;
    t.set(
        "SetPostEffect",
        lua.create_function(|_, asset: AnyUserData| -> mlua::Result<SceneShader> {
            let state = build_scene_shader(&asset)?;
            install_scene_shader(SceneSlot::PostEffect, state.clone());
            Ok(SceneShader {
                slot: SceneSlot::PostEffect,
                state,
            })
        })?,
    )?;
    t.set(
        "ClearPostEffect",
        lua.create_function(|_, _: ()| -> mlua::Result<()> {
            POST_EFFECT.with(|c| *c.borrow_mut() = None);
            Ok(())
        })?,
    )?;

    Ok(t)
}
