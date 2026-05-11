use std::cell::RefCell;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use mlua::{AnyUserData, Lua, Table, UserData, UserDataFields, UserDataMethods, Value};

use crate::libs::primitives::{CFrame, Color3, Vector};

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum LightKind {
    Point,
    Spot,
}

pub struct LightSourceState {
    pub id: u64,
    pub alive: bool,
    pub kind: LightKind,
    pub cframe: CFrame,
    pub color: Color3,
    pub brightness: f32,
    pub range: f32,
    pub cast_shadow: bool,
    pub cone_inner: f32,
    pub cone_outer: f32,
    pub falloff: f32,
    pub shader_id: Option<u64>,
    pub shader_params: [f32; 16],
}

impl Default for LightSourceState {
    fn default() -> Self {
        Self {
            id: 0,
            alive: true,
            kind: LightKind::Point,
            cframe: CFrame::new(Vector::new(0.0, 0.0, 0.0), Vector::new(0.0, 0.0, 0.0)),
            color: Color3::new(1.0, 1.0, 1.0),
            brightness: 1.0,
            range: 16.0,
            cast_shadow: false,
            cone_inner: std::f32::consts::FRAC_PI_6,
            cone_outer: std::f32::consts::FRAC_PI_4,
            falloff: 2.0,
            shader_id: None,
            shader_params: [0.0; 16],
        }
    }
}

thread_local! {
    static LIGHTS: RefCell<Vec<Arc<Mutex<LightSourceState>>>> = const { RefCell::new(Vec::new()) };
}

static NEXT_LIGHT_ID: AtomicU64 = AtomicU64::new(1);

pub fn list_lights() -> Vec<Arc<Mutex<LightSourceState>>> {
    LIGHTS.with(|c| {
        c.borrow_mut().retain(|l| l.lock().unwrap().alive);
        c.borrow().clone()
    })
}

pub struct LightHandle {
    pub state: Arc<Mutex<LightSourceState>>,
}

impl LightHandle {
    fn ensure_alive(&self, action: &str) -> mlua::Result<()> {
        if !self.state.lock().unwrap().alive {
            return Err(mlua::Error::RuntimeError(format!(
                "{action}: LightSource has been destroyed"
            )));
        }
        Ok(())
    }
}

impl UserData for LightHandle {
    fn add_fields<F: UserDataFields<Self>>(f: &mut F) {
        f.add_field_method_get("Kind", |_, this| {
            let s = this.state.lock().unwrap();
            Ok(match s.kind {
                LightKind::Point => "PointLight",
                LightKind::Spot => "Spotlight",
            }
            .to_string())
        });
        f.add_field_method_get("CFrame", |_, this| Ok(this.state.lock().unwrap().cframe));
        f.add_field_method_set("CFrame", |_, this, v: AnyUserData| {
            this.ensure_alive("set CFrame")?;
            let cf = *v.borrow::<CFrame>().map_err(|_| {
                mlua::Error::RuntimeError("LightSource.CFrame expects a CFrame".into())
            })?;
            this.state.lock().unwrap().cframe = cf;
            Ok(())
        });
        f.add_field_method_get("Color", |_, this| Ok(this.state.lock().unwrap().color));
        f.add_field_method_set("Color", |_, this, v: AnyUserData| {
            this.ensure_alive("set Color")?;
            let c = *v.borrow::<Color3>().map_err(|_| {
                mlua::Error::RuntimeError("LightSource.Color expects a Color3".into())
            })?;
            this.state.lock().unwrap().color = c;
            Ok(())
        });
        f.add_field_method_get("Brightness", |_, this| {
            Ok(this.state.lock().unwrap().brightness)
        });
        f.add_field_method_set("Brightness", |_, this, v: f32| {
            this.ensure_alive("set Brightness")?;
            this.state.lock().unwrap().brightness = v.max(0.0);
            Ok(())
        });
        f.add_field_method_get("Range", |_, this| Ok(this.state.lock().unwrap().range));
        f.add_field_method_set("Range", |_, this, v: f32| {
            this.ensure_alive("set Range")?;
            this.state.lock().unwrap().range = v.max(0.0);
            Ok(())
        });
        f.add_field_method_get("CastShadow", |_, this| {
            Ok(this.state.lock().unwrap().cast_shadow)
        });
        f.add_field_method_set("CastShadow", |_, this, v: bool| {
            this.ensure_alive("set CastShadow")?;
            this.state.lock().unwrap().cast_shadow = v;
            Ok(())
        });
        f.add_field_method_get("Falloff", |_, this| Ok(this.state.lock().unwrap().falloff));
        f.add_field_method_set("Falloff", |_, this, v: f32| {
            this.ensure_alive("set Falloff")?;
            this.state.lock().unwrap().falloff = v.max(0.0);
            Ok(())
        });
        f.add_field_method_get("ConeInner", |_, this| {
            Ok(this.state.lock().unwrap().cone_inner)
        });
        f.add_field_method_set("ConeInner", |_, this, v: f32| {
            this.ensure_alive("set ConeInner")?;
            let mut s = this.state.lock().unwrap();
            if s.kind != LightKind::Spot {
                return Err(mlua::Error::RuntimeError(
                    "ConeInner only applies to a Spotlight".into(),
                ));
            }
            s.cone_inner = v.clamp(0.0, std::f32::consts::PI);
            Ok(())
        });
        f.add_field_method_get("ConeOuter", |_, this| {
            Ok(this.state.lock().unwrap().cone_outer)
        });
        f.add_field_method_set("ConeOuter", |_, this, v: f32| {
            this.ensure_alive("set ConeOuter")?;
            let mut s = this.state.lock().unwrap();
            if s.kind != LightKind::Spot {
                return Err(mlua::Error::RuntimeError(
                    "ConeOuter only applies to a Spotlight".into(),
                ));
            }
            s.cone_outer = v.clamp(0.0, std::f32::consts::PI);
            Ok(())
        });
        f.add_field_method_get("Alive", |_, this| Ok(this.state.lock().unwrap().alive));
    }

    fn add_methods<M: UserDataMethods<Self>>(m: &mut M) {
        m.add_method("Destroy", |_, this, _: ()| {
            let mut s = this.state.lock().unwrap();
            s.alive = false;
            Ok(())
        });
        m.add_method(
            "AttachShader",
            |_, this, asset: AnyUserData| -> mlua::Result<()> {
                this.ensure_alive("AttachShader")?;
                let shader = crate::libs::shader::shader_id(&asset)?;
                let mut s = this.state.lock().unwrap();
                s.shader_id = Some(shader);
                Ok(())
            },
        );
        m.add_method("DetachShader", |_, this, _: ()| {
            this.ensure_alive("DetachShader")?;
            this.state.lock().unwrap().shader_id = None;
            Ok(())
        });
        m.add_method(
            "SetShaderParam",
            |_, this, (slot, value): (i64, f32)| -> mlua::Result<()> {
                this.ensure_alive("SetShaderParam")?;
                let slot = slot.clamp(0, 15) as usize;
                this.state.lock().unwrap().shader_params[slot] = value;
                Ok(())
            },
        );
        m.add_method(
            "Direction",
            |_, this, _: ()| -> mlua::Result<Vector> {
                let s = this.state.lock().unwrap();
                let r = s.cframe.rotation;
                let (sx, cx) = r.x.sin_cos();
                let (sy, cy) = r.y.sin_cos();
                let dir = Vector::new(-cx * sy, sx, -cx * cy);
                Ok(dir)
            },
        );
    }
}

pub fn generate_light_source(lua: &Lua, args: mlua::MultiValue) -> mlua::Result<LightHandle> {
    let mut iter = args.into_iter();
    let kind_v = iter.next().ok_or_else(|| {
        mlua::Error::RuntimeError(
            "LightingService.GenerateLightSource: missing first argument (\"PointLight\" or \"Spotlight\")".into(),
        )
    })?;
    let opts_v = iter.next().unwrap_or(Value::Nil);

    let kind = match kind_v {
        Value::String(s) => match s.to_str()?.as_ref() {
            "PointLight" | "pointlight" | "Point" | "point" => LightKind::Point,
            "Spotlight" | "spotlight" | "Spot" | "spot" => LightKind::Spot,
            other => {
                return Err(mlua::Error::RuntimeError(format!(
                    "LightingService.GenerateLightSource: unknown kind '{other}' (expected \"PointLight\" or \"Spotlight\")"
                )));
            }
        },
        _ => {
            return Err(mlua::Error::RuntimeError(
                "LightingService.GenerateLightSource: first argument must be a string".into(),
            ));
        }
    };

    let mut state = LightSourceState {
        id: NEXT_LIGHT_ID.fetch_add(1, Ordering::Relaxed),
        kind,
        ..Default::default()
    };

    if let Value::Table(opts) = opts_v {
        if let Ok(v) = opts.get::<AnyUserData>("CFrame") {
            if let Ok(cf) = v.borrow::<CFrame>() {
                state.cframe = *cf;
            }
        }
        if let Ok(v) = opts.get::<AnyUserData>("Color") {
            if let Ok(c) = v.borrow::<Color3>() {
                state.color = *c;
            }
        }
        if let Ok(v) = opts.get::<f32>("Brightness") {
            state.brightness = v.max(0.0);
        }
        if let Ok(v) = opts.get::<f32>("Range") {
            state.range = v.max(0.0);
        }
        if let Ok(v) = opts.get::<bool>("CastShadow") {
            state.cast_shadow = v;
        }
        if let Ok(v) = opts.get::<f32>("Falloff") {
            state.falloff = v.max(0.0);
        }
        if let Ok(v) = opts.get::<f32>("ConeInner") {
            state.cone_inner = v.clamp(0.0, std::f32::consts::PI);
        }
        if let Ok(v) = opts.get::<f32>("ConeOuter") {
            state.cone_outer = v.clamp(0.0, std::f32::consts::PI);
        }
    }

    let arc = Arc::new(Mutex::new(state));
    LIGHTS.with(|c| c.borrow_mut().push(arc.clone()));
    let _ = lua;
    Ok(LightHandle { state: arc })
}

pub fn active_lights_table(lua: &Lua) -> mlua::Result<Table> {
    let lights = list_lights();
    let t = lua.create_table()?;
    for (i, l) in lights.iter().enumerate() {
        t.set(i + 1, lua.create_userdata(LightHandle { state: l.clone() })?)?;
    }
    Ok(t)
}

pub fn pack_lights() -> (u32, Vec<f32>) {
    let lights = list_lights();
    let mut out: Vec<f32> = Vec::with_capacity(lights.len() * 16);
    for l in &lights {
        let s = l.lock().unwrap();
        let kind_id: f32 = match s.kind {
            LightKind::Point => 0.0,
            LightKind::Spot => 1.0,
        };
        out.push(s.cframe.position.x);
        out.push(s.cframe.position.y);
        out.push(s.cframe.position.z);
        out.push(kind_id);

        let r = s.cframe.rotation;
        let (sx, cx) = r.x.sin_cos();
        let (sy, cy) = r.y.sin_cos();
        let dir_x = -cx * sy;
        let dir_y = sx;
        let dir_z = -cx * cy;
        out.push(dir_x);
        out.push(dir_y);
        out.push(dir_z);
        out.push(s.brightness);

        out.push(s.color.r);
        out.push(s.color.g);
        out.push(s.color.b);
        out.push(s.range);

        out.push(s.cone_inner.cos());
        out.push(s.cone_outer.cos());
        out.push(s.falloff);
        out.push(if s.cast_shadow { 1.0 } else { 0.0 });
    }
    (lights.len() as u32, out)
}
