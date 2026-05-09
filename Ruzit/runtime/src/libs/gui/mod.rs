use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use mlua::{AnyUserData, Lua, MultiValue, Table, UserData, UserDataFields, UserDataMethods, Value};

use crate::libs::asset::{self, FontAsset, FragmentAsset, ImageAsset, ShaderAsset};
use crate::libs::primitives::{Color3, Dim};
use crate::libs::signal;

pub mod render;

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

thread_local! {

    static REGISTRY: RefCell<Vec<Arc<Mutex<PrimitiveState>>>> = const { RefCell::new(Vec::new()) };

    static SKYBOX: RefCell<Option<Arc<SceneShaderState>>> = const { RefCell::new(None) };
    static POST_EFFECT: RefCell<Option<Arc<SceneShaderState>>> = const { RefCell::new(None) };
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shape {
    Square,
    Circle,
    Triangle,

    Image,

    Text,
}

impl Shape {
    pub fn shape_id(self) -> u32 {
        match self {
            Self::Square => 0,
            Self::Circle => 1,
            Self::Triangle => 2,
            Self::Image => 3,
            Self::Text => 4,
        }
    }
}

pub struct ImageRef {
    pub id: u64,
    pub width: u32,
    pub height: u32,
    pub data: Arc<Vec<u8>>,
}

#[derive(Clone)]
pub struct AttachedShader {
    pub id: u64,
    #[allow(dead_code)]
    pub source: String,

    pub wgsl: Arc<String>,

    pub slot_of_name: Arc<std::collections::HashMap<String, u8>>,

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

    pub attached: Vec<AttachedShader>,
    pub changed_signal: Table,

    pub image: Option<Arc<ImageRef>>,

    pub text: Option<TextState>,
}

pub struct TextState {
    #[allow(dead_code)]
    pub font_id: u64,
    pub font: Arc<fontdue::Font>,
    pub content: String,
    pub size_px: f32,
    pub color: Color3,

    pub baked: Option<Arc<ImageRef>>,
}

impl TextState {
    fn invalidate(&mut self) {
        self.baked = None;
    }
}

pub struct RenderItem {
    pub shape: Shape,
    pub size: Dim,
    pub position: Dim,
    pub color: Color3,
    pub transparency: f32,
    pub z_index: i32,

    pub active_shader: Option<AttachedShader>,
    pub image: Option<Arc<ImageRef>>,
}

pub fn purge_image(lua: &Lua, asset_id: u64) {
    purge_primitives_matching(lua, |s| {
        s.image.as_ref().map(|i| i.id == asset_id).unwrap_or(false)
    });
}

pub fn purge_font(lua: &Lua, asset_id: u64) {
    purge_primitives_matching(lua, |s| {
        s.text
            .as_ref()
            .map(|t| t.font_id == asset_id)
            .unwrap_or(false)
    });
}

pub fn purge_shader(lua: &Lua, asset_id: u64) {
    REGISTRY.with(|cell| {
        let reg = cell.borrow();
        for p in reg.iter() {
            let mut s = p.lock().unwrap();
            if !s.alive {
                continue;
            }
            s.attached.retain(|e| e.id != asset_id);
        }
    });
    SKYBOX.with(|c| {
        if c.borrow().as_ref().map(|s| s.id) == Some(asset_id) {
            *c.borrow_mut() = None;
        }
    });
    POST_EFFECT.with(|c| {
        if c.borrow().as_ref().map(|s| s.id) == Some(asset_id) {
            *c.borrow_mut() = None;
        }
    });
    let _ = lua;
}

fn purge_primitives_matching(lua: &Lua, mut pred: impl FnMut(&PrimitiveState) -> bool) {
    let states: Vec<Arc<Mutex<PrimitiveState>>> = REGISTRY.with(|cell| {
        let reg = cell.borrow();
        reg.iter().cloned().collect()
    });
    for state in states {
        let sig = {
            let mut s = state.lock().unwrap();
            if !s.alive || !pred(&s) {
                continue;
            }
            s.alive = false;
            s.visible = false;
            s.attached.clear();
            s.image = None;
            s.text = None;
            s.changed_signal.clone()
        };
        let _ = fire_changed(lua, sig, "Destroyed");
    }
}

pub fn snapshot() -> Vec<RenderItem> {
    REGISTRY.with(|cell| {
        let mut reg = cell.borrow_mut();
        reg.retain(|p| p.lock().unwrap().alive);
        let mut out: Vec<RenderItem> = reg
            .iter()
            .filter_map(|p| {
                let mut s = p.lock().unwrap();
                if !s.visible {
                    return None;
                }

                let (image, size) = if matches!(s.shape, Shape::Text) {
                    let baked = bake_text_if_dirty(&mut s);
                    let size = match &baked {
                        Some(img) => Dim::new(img.width as f32, img.height as f32),
                        None => Dim::new(0.0, 0.0),
                    };
                    (baked, size)
                } else {
                    (s.image.clone(), s.size)
                };
                Some(RenderItem {
                    shape: s.shape,
                    size,
                    position: s.position,
                    color: s.color,
                    transparency: s.transparency,
                    z_index: s.z_index,
                    active_shader: s.attached.last().cloned(),
                    image,
                })
            })
            .collect();
        out.sort_by_key(|r| r.z_index);
        out
    })
}

fn bake_text_if_dirty(s: &mut PrimitiveState) -> Option<Arc<ImageRef>> {
    let ts = s.text.as_mut()?;
    if let Some(img) = &ts.baked {
        return Some(img.clone());
    }
    let baked = bake_text(&ts.font, &ts.content, ts.size_px, ts.color);
    ts.baked = Some(baked.clone());
    Some(baked)
}

fn bake_text(font: &fontdue::Font, content: &str, size_px: f32, color: Color3) -> Arc<ImageRef> {
    use fontdue::layout::{CoordinateSystem, Layout, TextStyle};
    let id = asset::next_shader_id();
    if content.is_empty() || size_px < 1.0 {
        return Arc::new(ImageRef {
            id,
            width: 1,
            height: 1,
            data: Arc::new(vec![0, 0, 0, 0]),
        });
    }
    let mut layout: Layout<()> = Layout::new(CoordinateSystem::PositiveYDown);
    layout.append(&[font], &TextStyle::new(content, size_px, 0));
    let glyphs = layout.glyphs();
    if glyphs.is_empty() {
        return Arc::new(ImageRef {
            id,
            width: 1,
            height: 1,
            data: Arc::new(vec![0, 0, 0, 0]),
        });
    }

    let mut max_x: i32 = 0;
    let mut max_y: i32 = 0;
    for g in glyphs {
        max_x = max_x.max(g.x as i32 + g.width as i32);
        max_y = max_y.max(g.y as i32 + g.height as i32);
    }
    let width = max_x.max(1) as u32;
    let height = max_y.max(1) as u32;
    let mut buf = vec![0u8; (width * height * 4) as usize];
    let cr = (color.r * 255.0).round().clamp(0.0, 255.0) as u8;
    let cg = (color.g * 255.0).round().clamp(0.0, 255.0) as u8;
    let cb = (color.b * 255.0).round().clamp(0.0, 255.0) as u8;
    for g in glyphs {
        let (_metrics, bitmap) = font.rasterize_config(g.key);
        let gw = g.width as i32;
        let gh = g.height as i32;
        let gx = g.x as i32;
        let gy = g.y as i32;
        for j in 0..gh {
            for i in 0..gw {
                let alpha = bitmap[(j * gw + i) as usize];
                if alpha == 0 {
                    continue;
                }
                let px = gx + i;
                let py = gy + j;
                if px < 0 || py < 0 || px as u32 >= width || py as u32 >= height {
                    continue;
                }
                let off = ((py as u32 * width + px as u32) * 4) as usize;
                buf[off] = cr;
                buf[off + 1] = cg;
                buf[off + 2] = cb;
                buf[off + 3] = alpha;
            }
        }
    }
    Arc::new(ImageRef {
        id,
        width,
        height,
        data: Arc::new(buf),
    })
}

pub struct GuiPrimitive {
    state: Arc<Mutex<PrimitiveState>>,
}

impl GuiPrimitive {
    fn new(lua: &Lua, shape: Shape) -> mlua::Result<Self> {
        Self::with_state(lua, shape, None, None, Dim::new(100.0, 100.0))
    }

    fn new_image(lua: &Lua, asset: &ImageAsset) -> mlua::Result<Self> {
        let image = ImageRef {
            id: asset.id,
            width: asset.width,
            height: asset.height,
            data: asset.data.clone(),
        };

        let size = Dim::new(asset.width as f32, asset.height as f32);
        Self::with_state(lua, Shape::Image, Some(Arc::new(image)), None, size)
    }

    fn new_text(lua: &Lua, asset: &FontAsset) -> mlua::Result<Self> {
        let text_state = TextState {
            font_id: asset.id,
            font: asset.font.clone(),
            content: String::new(),
            size_px: 24.0,
            color: Color3::new(1.0, 1.0, 1.0),
            baked: None,
        };

        Self::with_state(lua, Shape::Text, None, Some(text_state), Dim::new(0.0, 0.0))
    }

    fn with_state(
        lua: &Lua,
        shape: Shape,
        image: Option<Arc<ImageRef>>,
        text: Option<TextState>,
        size: Dim,
    ) -> mlua::Result<Self> {
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        let changed_signal = signal::new_instance(lua)?;
        let state = Arc::new(Mutex::new(PrimitiveState {
            id,
            shape,
            size,
            position: Dim::new(0.0, 0.0),
            color: Color3::new(1.0, 1.0, 1.0),
            transparency: 0.0,
            z_index: 0,
            visible: false,
            alive: true,
            attached: Vec::new(),
            changed_signal,
            image,
            text,
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
                Shape::Text => "Text",
            })
        });

        f.add_field_method_get("Text", |_, this| -> mlua::Result<String> {
            let s = this.state.lock().unwrap();
            Ok(s.text
                .as_ref()
                .map(|t| t.content.clone())
                .unwrap_or_default())
        });
        f.add_field_method_set("Text", |lua, this, value: String| {
            this.ensure_alive("set Text")?;
            let signal_table = {
                let mut s = this.state.lock().unwrap();
                let ts = s.text.as_mut().ok_or_else(|| {
                    mlua::Error::RuntimeError("Text is only valid on Font primitives".into())
                })?;
                if ts.content != value {
                    ts.content = value;
                    ts.invalidate();
                }
                s.changed_signal.clone()
            };
            fire_changed(lua, signal_table, "Text")
        });
        f.add_field_method_get("TextSize", |_, this| -> mlua::Result<f32> {
            let s = this.state.lock().unwrap();
            Ok(s.text.as_ref().map(|t| t.size_px).unwrap_or(0.0))
        });
        f.add_field_method_set("TextSize", |lua, this, value: f32| {
            this.ensure_alive("set TextSize")?;
            let signal_table = {
                let mut s = this.state.lock().unwrap();
                let ts = s.text.as_mut().ok_or_else(|| {
                    mlua::Error::RuntimeError("TextSize is only valid on Font primitives".into())
                })?;
                let new_size = value.clamp(1.0, 1024.0);
                if ts.size_px != new_size {
                    ts.size_px = new_size;
                    ts.invalidate();
                }
                s.changed_signal.clone()
            };
            fire_changed(lua, signal_table, "TextSize")
        });
        f.add_field_method_get("TextColor", |_, this| -> mlua::Result<Color3> {
            let s = this.state.lock().unwrap();
            Ok(s.text
                .as_ref()
                .map(|t| t.color)
                .unwrap_or(Color3::new(1.0, 1.0, 1.0)))
        });
        f.add_field_method_set("TextColor", |lua, this, value: AnyUserData| {
            this.ensure_alive("set TextColor")?;
            let color = *value.borrow::<Color3>().map_err(|_| {
                mlua::Error::RuntimeError("TextColor expects a Primitives.Color3".into())
            })?;
            let signal_table = {
                let mut s = this.state.lock().unwrap();
                let ts = s.text.as_mut().ok_or_else(|| {
                    mlua::Error::RuntimeError("TextColor is only valid on Font primitives".into())
                })?;
                if ts.color.r != color.r || ts.color.g != color.g || ts.color.b != color.b {
                    ts.color = color;
                    ts.invalidate();
                }
                s.changed_signal.clone()
            };
            fire_changed(lua, signal_table, "TextColor")
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SceneSlot {
    Skybox,
    PostEffect,
}

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

    let prelude = render::FRAGMENT_PRELUDE;
    let wgsl = format!("{prelude}\n{code}");
    Ok(Arc::new(SceneShaderState {
        id,
        wgsl: Arc::new(wgsl),
        slot_of_name: Arc::new(slot_of_name),
        params: Arc::new(Mutex::new([0.0_f32; 16])),
    }))
}

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
    basic.set(
        "Font",
        lua.create_function(|lua, asset: AnyUserData| -> mlua::Result<GuiPrimitive> {
            let font = asset.borrow::<FontAsset>().map_err(|_| {
                mlua::Error::RuntimeError(
                    "GUI.Basic.Font expects a FontAsset (Asset.GetAsset(\"Font\", ...))".into(),
                )
            })?;
            GuiPrimitive::new_text(lua, &font)
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
