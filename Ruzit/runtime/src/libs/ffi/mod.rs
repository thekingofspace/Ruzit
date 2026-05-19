use std::cell::RefCell;
use std::collections::HashMap;
use std::ffi::{CStr, CString};
use std::os::raw::c_char;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

thread_local! {
    static PROJECT_ROOT: RefCell<Option<PathBuf>> = const { RefCell::new(None) };
    static PENDING_ASYNC: RefCell<HashMap<u64, RegistryKey>> = RefCell::new(HashMap::new());
}

static NEXT_ASYNC_ID: AtomicU64 = AtomicU64::new(1);

struct AsyncResult {
    id: u64,
    payload: Result<String, String>,
}

fn completed_queue() -> &'static Mutex<Vec<AsyncResult>> {
    use std::sync::OnceLock;
    static Q: OnceLock<Mutex<Vec<AsyncResult>>> = OnceLock::new();
    Q.get_or_init(|| Mutex::new(Vec::new()))
}

pub fn set_project_root(root: PathBuf) {
    PROJECT_ROOT.with(|c| *c.borrow_mut() = Some(root));
}

fn project_bin_dir() -> Option<PathBuf> {
    PROJECT_ROOT.with(|c| {
        c.borrow().as_ref().map(|p| p.join("bin")).filter(|p| p.is_dir())
    })
}

use libloading::{Library, Symbol};
use mlua::{
    AnyUserData, Function, Lua, MultiValue, RegistryKey, Table, Thread, UserData, UserDataFields,
    UserDataMethods, Value,
};

use crate::libs::primitives::{CFrame, Color3, Vector};

type FfiCallFn = unsafe extern "C" fn(*const c_char, *const c_char) -> *mut c_char;
type FfiFreeFn = unsafe extern "C" fn(*mut c_char);

#[repr(C)]
#[derive(Clone, Copy)]
pub struct RuzitHost {
    pub call: extern "C" fn(name: *const c_char, args: *const c_char) -> *mut c_char,
    pub free: extern "C" fn(ptr: *mut c_char),
}

type RuzitFfiInitFn = unsafe extern "C" fn(RuzitHost);

pub enum FfiHandle {
    Primitive(Arc<Mutex<crate::libs::gui::PrimitiveState>>),
    Part(Arc<Mutex<crate::libs::renderable::PartState>>),
    DrawableImg(Arc<Mutex<crate::libs::renderable::DynTextureBuffer>>),
}

unsafe impl Send for FfiHandle {}
unsafe impl Sync for FfiHandle {}

pub enum MainTask {
    FireNamedSignal {
        name: String,
        args: serde_json::Value,
    },
    SetCamera {
        cframe: Option<CFrame>,
        fov: Option<f32>,
        near: Option<f32>,
        far: Option<f32>,
    },
    LuaPrint(String),
}

fn main_task_queue() -> &'static Mutex<Vec<MainTask>> {
    use std::sync::OnceLock;
    static Q: OnceLock<Mutex<Vec<MainTask>>> = OnceLock::new();
    Q.get_or_init(|| Mutex::new(Vec::new()))
}

fn enqueue_main_task(task: MainTask) {
    if let Ok(mut q) = main_task_queue().lock() {
        q.push(task);
    }
}

thread_local! {
    static NAMED_SIGNALS: RefCell<HashMap<String, mlua::RegistryKey>> =
        RefCell::new(HashMap::new());
}

fn world_store() -> &'static Mutex<HashMap<String, serde_json::Value>> {
    use std::sync::OnceLock;
    static W: OnceLock<Mutex<HashMap<String, serde_json::Value>>> = OnceLock::new();
    W.get_or_init(|| Mutex::new(HashMap::new()))
}

fn handles_registry() -> &'static Mutex<std::collections::HashMap<u64, FfiHandle>> {
    use std::sync::OnceLock;
    static REG: OnceLock<Mutex<std::collections::HashMap<u64, FfiHandle>>> = OnceLock::new();
    REG.get_or_init(|| Mutex::new(std::collections::HashMap::new()))
}

fn handles_dedup() -> &'static Mutex<std::collections::HashMap<usize, u64>> {
    use std::sync::OnceLock;
    static REG: OnceLock<Mutex<std::collections::HashMap<usize, u64>>> = OnceLock::new();
    REG.get_or_init(|| Mutex::new(std::collections::HashMap::new()))
}

static NEXT_HANDLE_ID: AtomicU64 = AtomicU64::new(1);

fn register_handle(handle: FfiHandle) -> u64 {
    let ptr_key = match &handle {
        FfiHandle::Primitive(arc) => Arc::as_ptr(arc) as usize,
        FfiHandle::Part(arc) => Arc::as_ptr(arc) as usize,
        FfiHandle::DrawableImg(arc) => Arc::as_ptr(arc) as usize,
    };
    {
        let dedup = handles_dedup().lock().unwrap();
        if let Some(id) = dedup.get(&ptr_key) {
            return *id;
        }
    }
    let id = NEXT_HANDLE_ID.fetch_add(1, Ordering::Relaxed);
    handles_registry().lock().unwrap().insert(id, handle);
    handles_dedup().lock().unwrap().insert(ptr_key, id);
    id
}

fn lookup_primitive(id: u64) -> Result<Arc<Mutex<crate::libs::gui::PrimitiveState>>, String> {
    let reg = handles_registry().lock().unwrap();
    match reg.get(&id) {
        Some(FfiHandle::Primitive(arc)) => Ok(arc.clone()),
        Some(_) => Err(format!("handle {id} is not a Primitive")),
        None => Err(format!("unknown handle {id}")),
    }
}

fn lookup_part(id: u64) -> Result<Arc<Mutex<crate::libs::renderable::PartState>>, String> {
    let reg = handles_registry().lock().unwrap();
    match reg.get(&id) {
        Some(FfiHandle::Part(arc)) => Ok(arc.clone()),
        Some(_) => Err(format!("handle {id} is not a BasePart")),
        None => Err(format!("unknown handle {id}")),
    }
}

fn lookup_drawable(
    id: u64,
) -> Result<Arc<Mutex<crate::libs::renderable::DynTextureBuffer>>, String> {
    let reg = handles_registry().lock().unwrap();
    match reg.get(&id) {
        Some(FfiHandle::DrawableImg(arc)) => Ok(arc.clone()),
        Some(_) => Err(format!("handle {id} is not a DrawableImg")),
        None => Err(format!("unknown handle {id}")),
    }
}

extern "C" fn host_call_impl(name: *const c_char, args: *const c_char) -> *mut c_char {
    let result = std::panic::catch_unwind(|| -> Result<serde_json::Value, String> {
        if name.is_null() {
            return Err("host_call: name is null".into());
        }
        let name = unsafe { CStr::from_ptr(name) }
            .to_string_lossy()
            .into_owned();
        let args_str = if args.is_null() {
            "null".to_string()
        } else {
            unsafe { CStr::from_ptr(args) }
                .to_string_lossy()
                .into_owned()
        };
        let parsed: serde_json::Value =
            serde_json::from_str(&args_str).unwrap_or(serde_json::Value::Null);
        host_dispatch(&name, &parsed)
    });
    let json = match result {
        Ok(Ok(v)) => v.to_string(),
        Ok(Err(e)) => serde_json::json!({ "error": e }).to_string(),
        Err(_) => serde_json::json!({ "error": "host_call panicked" }).to_string(),
    };
    match CString::new(json) {
        Ok(c) => c.into_raw(),
        Err(_) => std::ptr::null_mut(),
    }
}

extern "C" fn host_free_impl(ptr: *mut c_char) {
    if !ptr.is_null() {
        unsafe {
            let _ = CString::from_raw(ptr);
        }
    }
}

fn dim_xy(v: &serde_json::Value) -> Option<crate::libs::primitives::Dim> {
    let x = v.get("x").and_then(|x| x.as_f64())? as f32;
    let y = v.get("y").and_then(|y| y.as_f64())? as f32;
    Some(crate::libs::primitives::Dim::new(x, y))
}

fn host_dispatch(name: &str, args: &serde_json::Value) -> Result<serde_json::Value, String> {
    let handle = args.get("handle").and_then(|v| v.as_u64());
    match name {
        "Logging.Print" => {
            let msg = args
                .get("message")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let prefix = args
                .get("prefix")
                .and_then(|v| v.as_str())
                .unwrap_or("FFI");
            println!("[{prefix}] {msg}");
            Ok(serde_json::Value::Null)
        }

        "Primitive.GetPosition" => {
            let prim = lookup_primitive(handle.ok_or("missing handle")?)?;
            let s = prim.lock().map_err(|e| e.to_string())?;
            Ok(serde_json::json!({ "x": s.position.x, "y": s.position.y }))
        }
        "Primitive.SetPosition" => {
            let prim = lookup_primitive(handle.ok_or("missing handle")?)?;
            let dim = dim_xy(args).ok_or("missing x/y")?;
            let mut s = prim.lock().map_err(|e| e.to_string())?;
            s.position = dim;
            drop(s);
            crate::libs::gui::bump_dirty();
            Ok(serde_json::Value::Null)
        }
        "Primitive.GetSize" => {
            let prim = lookup_primitive(handle.ok_or("missing handle")?)?;
            let s = prim.lock().map_err(|e| e.to_string())?;
            Ok(serde_json::json!({ "x": s.size.x, "y": s.size.y }))
        }
        "Primitive.SetSize" => {
            let prim = lookup_primitive(handle.ok_or("missing handle")?)?;
            let dim = dim_xy(args).ok_or("missing x/y")?;
            let mut s = prim.lock().map_err(|e| e.to_string())?;
            s.size = dim;
            drop(s);
            crate::libs::gui::bump_dirty();
            Ok(serde_json::Value::Null)
        }
        "Primitive.GetColor" => {
            let prim = lookup_primitive(handle.ok_or("missing handle")?)?;
            let s = prim.lock().map_err(|e| e.to_string())?;
            Ok(serde_json::json!({ "r": s.color.r, "g": s.color.g, "b": s.color.b }))
        }
        "Primitive.SetColor" => {
            let prim = lookup_primitive(handle.ok_or("missing handle")?)?;
            let r = args.get("r").and_then(|v| v.as_f64()).unwrap_or(0.0) as f32;
            let g = args.get("g").and_then(|v| v.as_f64()).unwrap_or(0.0) as f32;
            let b = args.get("b").and_then(|v| v.as_f64()).unwrap_or(0.0) as f32;
            let mut s = prim.lock().map_err(|e| e.to_string())?;
            s.color = crate::libs::primitives::Color3::new(r, g, b);
            drop(s);
            crate::libs::gui::bump_dirty();
            Ok(serde_json::Value::Null)
        }
        "Primitive.SetVisible" => {
            let prim = lookup_primitive(handle.ok_or("missing handle")?)?;
            let v = args.get("visible").and_then(|v| v.as_bool()).unwrap_or(true);
            let mut s = prim.lock().map_err(|e| e.to_string())?;
            s.visible = v;
            drop(s);
            crate::libs::gui::bump_dirty();
            Ok(serde_json::Value::Null)
        }
        "Primitive.SetTransparency" => {
            let prim = lookup_primitive(handle.ok_or("missing handle")?)?;
            let v = args.get("value").and_then(|v| v.as_f64()).unwrap_or(0.0) as f32;
            let mut s = prim.lock().map_err(|e| e.to_string())?;
            s.transparency = v.clamp(0.0, 1.0);
            drop(s);
            crate::libs::gui::bump_dirty();
            Ok(serde_json::Value::Null)
        }
        "Primitive.SetRotation" => {
            let prim = lookup_primitive(handle.ok_or("missing handle")?)?;
            let deg = args.get("degrees").and_then(|v| v.as_f64()).unwrap_or(0.0) as f32;
            let mut s = prim.lock().map_err(|e| e.to_string())?;
            s.rotation = deg;
            drop(s);
            crate::libs::gui::bump_dirty();
            Ok(serde_json::Value::Null)
        }

        "BasePart.GetCFrame" => {
            let part = lookup_part(handle.ok_or("missing handle")?)?;
            let s = part.lock().map_err(|e| e.to_string())?;
            let cf = s.current_cframe();
            Ok(serde_json::json!({
                "_type": "CFrame",
                "position": { "x": cf.position.x, "y": cf.position.y, "z": cf.position.z },
                "rotation": { "x": cf.rotation.x, "y": cf.rotation.y, "z": cf.rotation.z },
            }))
        }
        "BasePart.SetCFrame" => {
            let part = lookup_part(handle.ok_or("missing handle")?)?;
            let pos = args
                .get("position")
                .and_then(|p| {
                    let x = p.get("x")?.as_f64()? as f32;
                    let y = p.get("y")?.as_f64()? as f32;
                    let z = p.get("z")?.as_f64()? as f32;
                    Some(Vector::new(x, y, z))
                })
                .unwrap_or(Vector::new(0.0, 0.0, 0.0));
            let rot = args
                .get("rotation")
                .and_then(|p| {
                    let x = p.get("x")?.as_f64()? as f32;
                    let y = p.get("y")?.as_f64()? as f32;
                    let z = p.get("z")?.as_f64()? as f32;
                    Some(Vector::new(x, y, z))
                })
                .unwrap_or(Vector::new(0.0, 0.0, 0.0));
            let cf = CFrame { position: pos, rotation: rot };
            let mut s = part.lock().map_err(|e| e.to_string())?;
            s.cframe = cf;
            if let Some(o) = &s.physics_override {
                if let Ok(mut g) = o.lock() {
                    *g = cf;
                }
            }
            drop(s);
            crate::libs::renderable::bump_parts_dirty();
            Ok(serde_json::Value::Null)
        }
        "BasePart.GetSize" => {
            let part = lookup_part(handle.ok_or("missing handle")?)?;
            let s = part.lock().map_err(|e| e.to_string())?;
            Ok(serde_json::json!({
                "_type": "Vector", "x": s.size.x, "y": s.size.y, "z": s.size.z,
            }))
        }
        "BasePart.SetSize" => {
            let part = lookup_part(handle.ok_or("missing handle")?)?;
            let x = args.get("x").and_then(|v| v.as_f64()).unwrap_or(0.0) as f32;
            let y = args.get("y").and_then(|v| v.as_f64()).unwrap_or(0.0) as f32;
            let z = args.get("z").and_then(|v| v.as_f64()).unwrap_or(0.0) as f32;
            let mut s = part.lock().map_err(|e| e.to_string())?;
            s.size = Vector::new(x, y, z);
            drop(s);
            crate::libs::renderable::bump_parts_dirty();
            Ok(serde_json::Value::Null)
        }
        "BasePart.GetColor" => {
            let part = lookup_part(handle.ok_or("missing handle")?)?;
            let s = part.lock().map_err(|e| e.to_string())?;
            Ok(serde_json::json!({
                "_type": "Color3", "r": s.color.r, "g": s.color.g, "b": s.color.b,
            }))
        }
        "BasePart.SetColor" => {
            let part = lookup_part(handle.ok_or("missing handle")?)?;
            let r = args.get("r").and_then(|v| v.as_f64()).unwrap_or(0.0) as f32;
            let g = args.get("g").and_then(|v| v.as_f64()).unwrap_or(0.0) as f32;
            let b = args.get("b").and_then(|v| v.as_f64()).unwrap_or(0.0) as f32;
            let mut s = part.lock().map_err(|e| e.to_string())?;
            s.color = Color3::new(r, g, b);
            drop(s);
            crate::libs::renderable::bump_parts_dirty();
            Ok(serde_json::Value::Null)
        }
        "BasePart.SetRender" => {
            let part = lookup_part(handle.ok_or("missing handle")?)?;
            let v = args.get("value").and_then(|v| v.as_bool()).unwrap_or(true);
            let mut s = part.lock().map_err(|e| e.to_string())?;
            s.render = v;
            drop(s);
            crate::libs::renderable::bump_parts_dirty();
            Ok(serde_json::Value::Null)
        }
        "BasePart.SetCastShadow" => {
            let part = lookup_part(handle.ok_or("missing handle")?)?;
            let v = args.get("value").and_then(|v| v.as_bool()).unwrap_or(true);
            let mut s = part.lock().map_err(|e| e.to_string())?;
            s.cast_shadow = v;
            drop(s);
            crate::libs::renderable::bump_parts_dirty();
            Ok(serde_json::Value::Null)
        }
        "BasePart.SetReceiveShadow" => {
            let part = lookup_part(handle.ok_or("missing handle")?)?;
            let v = args.get("value").and_then(|v| v.as_bool()).unwrap_or(true);
            let mut s = part.lock().map_err(|e| e.to_string())?;
            s.receive_shadow = v;
            drop(s);
            crate::libs::renderable::bump_parts_dirty();
            Ok(serde_json::Value::Null)
        }
        "BasePart.SetIgnoreRaycast" => {
            let part = lookup_part(handle.ok_or("missing handle")?)?;
            let v = args.get("value").and_then(|v| v.as_bool()).unwrap_or(false);
            let mut s = part.lock().map_err(|e| e.to_string())?;
            s.ignore_raycast = v;
            drop(s);
            Ok(serde_json::Value::Null)
        }
        "BasePart.SetLit" => {
            let part = lookup_part(handle.ok_or("missing handle")?)?;
            let v = args.get("value").and_then(|v| v.as_bool()).unwrap_or(true);
            let mut s = part.lock().map_err(|e| e.to_string())?;
            s.lit = v;
            drop(s);
            crate::libs::renderable::bump_parts_dirty();
            Ok(serde_json::Value::Null)
        }

        "DrawableImg.GetSize" => {
            let buf = lookup_drawable(handle.ok_or("missing handle")?)?;
            let b = buf.lock().map_err(|e| e.to_string())?;
            Ok(serde_json::json!({ "width": b.width, "height": b.height }))
        }
        "DrawableImg.WritePixel" => {
            let buf = lookup_drawable(handle.ok_or("missing handle")?)?;
            let x = args.get("x").and_then(|v| v.as_i64()).unwrap_or(0);
            let y = args.get("y").and_then(|v| v.as_i64()).unwrap_or(0);
            let (r, g, b_, a) = read_rgba(args);
            let mut buf = buf.lock().map_err(|e| e.to_string())?;
            put_pixel(&mut buf, x, y, r, g, b_, a);
            buf.version = buf.version.wrapping_add(1);
            Ok(serde_json::Value::Null)
        }
        "DrawableImg.Fill" => {
            let buf = lookup_drawable(handle.ok_or("missing handle")?)?;
            let (r, g, b_, a) = read_rgba(args);
            let mut buf = buf.lock().map_err(|e| e.to_string())?;
            for px in buf.bytes.chunks_exact_mut(4) {
                px[0] = r;
                px[1] = g;
                px[2] = b_;
                px[3] = a;
            }
            buf.version = buf.version.wrapping_add(1);
            Ok(serde_json::Value::Null)
        }
        "DrawableImg.Clear" => {
            let buf = lookup_drawable(handle.ok_or("missing handle")?)?;
            let mut buf = buf.lock().map_err(|e| e.to_string())?;
            for byte in buf.bytes.iter_mut() {
                *byte = 0;
            }
            buf.version = buf.version.wrapping_add(1);
            Ok(serde_json::Value::Null)
        }
        "DrawableImg.DrawRect" => {
            let buf = lookup_drawable(handle.ok_or("missing handle")?)?;
            let x = args.get("x").and_then(|v| v.as_i64()).unwrap_or(0);
            let y = args.get("y").and_then(|v| v.as_i64()).unwrap_or(0);
            let w = args.get("w").and_then(|v| v.as_i64()).unwrap_or(0);
            let h = args.get("h").and_then(|v| v.as_i64()).unwrap_or(0);
            let (r, g, b_, a) = read_rgba(args);
            let mut buf = buf.lock().map_err(|e| e.to_string())?;
            for j in y..y + h {
                for i in x..x + w {
                    put_pixel(&mut buf, i, j, r, g, b_, a);
                }
            }
            buf.version = buf.version.wrapping_add(1);
            Ok(serde_json::Value::Null)
        }
        "DrawableImg.DrawLine" => {
            let buf = lookup_drawable(handle.ok_or("missing handle")?)?;
            let x1 = args.get("x1").and_then(|v| v.as_i64()).unwrap_or(0) as i32;
            let y1 = args.get("y1").and_then(|v| v.as_i64()).unwrap_or(0) as i32;
            let x2 = args.get("x2").and_then(|v| v.as_i64()).unwrap_or(0) as i32;
            let y2 = args.get("y2").and_then(|v| v.as_i64()).unwrap_or(0) as i32;
            let (r, g, b_, a) = read_rgba(args);
            let mut buf = buf.lock().map_err(|e| e.to_string())?;
            bresenham(&mut buf, x1, y1, x2, y2, r, g, b_, a);
            buf.version = buf.version.wrapping_add(1);
            Ok(serde_json::Value::Null)
        }

        "Camera.Set" => {
            let cframe = args.get("cframe").and_then(|p| {
                let pos = p.get("position")?;
                let rot = p.get("rotation")?;
                Some(CFrame {
                    position: Vector::new(
                        pos.get("x")?.as_f64()? as f32,
                        pos.get("y")?.as_f64()? as f32,
                        pos.get("z")?.as_f64()? as f32,
                    ),
                    rotation: Vector::new(
                        rot.get("x")?.as_f64()? as f32,
                        rot.get("y")?.as_f64()? as f32,
                        rot.get("z")?.as_f64()? as f32,
                    ),
                })
            });
            let fov = args.get("fov").and_then(|v| v.as_f64()).map(|n| n as f32);
            let near = args.get("near").and_then(|v| v.as_f64()).map(|n| n as f32);
            let far = args.get("far").and_then(|v| v.as_f64()).map(|n| n as f32);
            enqueue_main_task(MainTask::SetCamera {
                cframe,
                fov,
                near,
                far,
            });
            Ok(serde_json::Value::Null)
        }
        "Camera.Get" => {
            let snap = crate::libs::renderable::camera_snapshot();
            Ok(serde_json::json!({
                "cframe": {
                    "position": { "x": snap.cframe.position.x, "y": snap.cframe.position.y, "z": snap.cframe.position.z },
                    "rotation": { "x": snap.cframe.rotation.x, "y": snap.cframe.rotation.y, "z": snap.cframe.rotation.z },
                },
                "fov":  snap.fov_deg,
                "near": snap.near,
                "far":  snap.far,
            }))
        }

        "Signal.Fire" => {
            let sig_name = args
                .get("name")
                .and_then(|v| v.as_str())
                .ok_or("Signal.Fire: missing `name`")?
                .to_string();
            let payload = args
                .get("args")
                .cloned()
                .unwrap_or(serde_json::Value::Null);
            enqueue_main_task(MainTask::FireNamedSignal {
                name: sig_name,
                args: payload,
            });
            Ok(serde_json::Value::Null)
        }

        "World.Set" => {
            let key = args
                .get("key")
                .and_then(|v| v.as_str())
                .ok_or("World.Set: missing `key`")?
                .to_string();
            let value = args.get("value").cloned().unwrap_or(serde_json::Value::Null);
            world_store().lock().unwrap().insert(key, value);
            Ok(serde_json::Value::Null)
        }
        "World.Get" => {
            let key = args
                .get("key")
                .and_then(|v| v.as_str())
                .ok_or("World.Get: missing `key`")?;
            Ok(world_store()
                .lock()
                .unwrap()
                .get(key)
                .cloned()
                .unwrap_or(serde_json::Value::Null))
        }
        "World.Has" => {
            let key = args
                .get("key")
                .and_then(|v| v.as_str())
                .ok_or("World.Has: missing `key`")?;
            Ok(serde_json::Value::Bool(
                world_store().lock().unwrap().contains_key(key),
            ))
        }
        "World.Delete" => {
            let key = args
                .get("key")
                .and_then(|v| v.as_str())
                .ok_or("World.Delete: missing `key`")?;
            let had = world_store().lock().unwrap().remove(key).is_some();
            Ok(serde_json::Value::Bool(had))
        }
        "World.Keys" => {
            let keys: Vec<String> = world_store().lock().unwrap().keys().cloned().collect();
            Ok(serde_json::json!(keys))
        }

        _ => Err(format!("unknown host RPC: {name}")),
    }
}

fn read_rgba(args: &serde_json::Value) -> (u8, u8, u8, u8) {
    let r = (args.get("r").and_then(|v| v.as_f64()).unwrap_or(0.0) * 255.0)
        .clamp(0.0, 255.0) as u8;
    let g = (args.get("g").and_then(|v| v.as_f64()).unwrap_or(0.0) * 255.0)
        .clamp(0.0, 255.0) as u8;
    let b = (args.get("b").and_then(|v| v.as_f64()).unwrap_or(0.0) * 255.0)
        .clamp(0.0, 255.0) as u8;
    let a = (args
        .get("a")
        .and_then(|v| v.as_f64())
        .unwrap_or(1.0)
        * 255.0)
        .clamp(0.0, 255.0) as u8;
    (r, g, b, a)
}

fn put_pixel(
    buf: &mut crate::libs::renderable::DynTextureBuffer,
    x: i64,
    y: i64,
    r: u8,
    g: u8,
    b: u8,
    a: u8,
) {
    if x < 0 || y < 0 {
        return;
    }
    let (w, h) = (buf.width as i64, buf.height as i64);
    if x >= w || y >= h {
        return;
    }
    let off = ((y * w + x) * 4) as usize;
    if off + 4 <= buf.bytes.len() {
        buf.bytes[off] = r;
        buf.bytes[off + 1] = g;
        buf.bytes[off + 2] = b;
        buf.bytes[off + 3] = a;
    }
}

fn bresenham(
    buf: &mut crate::libs::renderable::DynTextureBuffer,
    x1: i32,
    y1: i32,
    x2: i32,
    y2: i32,
    r: u8,
    g: u8,
    b: u8,
    a: u8,
) {
    let dx = (x2 - x1).abs();
    let dy = -((y2 - y1).abs());
    let sx = if x1 < x2 { 1 } else { -1 };
    let sy = if y1 < y2 { 1 } else { -1 };
    let mut err = dx + dy;
    let mut x = x1;
    let mut y = y1;
    loop {
        put_pixel(buf, x as i64, y as i64, r, g, b, a);
        if x == x2 && y == y2 {
            break;
        }
        let e2 = 2 * err;
        if e2 >= dy {
            err += dy;
            x += sx;
        }
        if e2 <= dx {
            err += dx;
            y += sy;
        }
    }
}

pub struct LoadedLib {
    lib: Library,
    pub source: String,
}

pub struct FFILibrary {
    pub state: Arc<Mutex<Option<LoadedLib>>>,
}

unsafe impl Send for FFILibrary {}
unsafe impl Sync for FFILibrary {}

pub fn create(lua: &Lua) -> mlua::Result<Table> {
    let t = lua.create_table()?;

    t.set(
        "Load",
        lua.create_function(|_, name: String| -> mlua::Result<FFILibrary> {
            let resolved = resolve_lib_path(&name)?;
            let lib = unsafe { Library::new(&resolved) }.map_err(|e| {
                mlua::Error::RuntimeError(format!(
                    "FFI.Load: failed to open '{}': {e}",
                    resolved.display()
                ))
            })?;
            unsafe {
                let _: Symbol<FfiCallFn> = lib.get(b"ruzit_ffi_call\0").map_err(|e| {
                    mlua::Error::RuntimeError(format!(
                        "FFI.Load: '{}' does not export `ruzit_ffi_call`: {e}",
                        resolved.display()
                    ))
                })?;
                let _: Symbol<FfiFreeFn> = lib.get(b"ruzit_ffi_free\0").map_err(|e| {
                    mlua::Error::RuntimeError(format!(
                        "FFI.Load: '{}' does not export `ruzit_ffi_free`: {e}",
                        resolved.display()
                    ))
                })?;
                if let Ok(init) = lib.get::<RuzitFfiInitFn>(b"ruzit_ffi_init\0") {
                    init(RuzitHost {
                        call: host_call_impl,
                        free: host_free_impl,
                    });
                }
            }
            Ok(FFILibrary {
                state: Arc::new(Mutex::new(Some(LoadedLib {
                    lib,
                    source: resolved.to_string_lossy().into_owned(),
                }))),
            })
        })?,
    )?;

    t.set(
        "BinDirectory",
        lua.create_function(|lua, _: ()| -> mlua::Result<Value> {
            match bin_directory() {
                Some(p) => Ok(Value::String(lua.create_string(p.to_string_lossy().as_ref())?)),
                None => Ok(Value::Nil),
            }
        })?,
    )?;

    t.set(
        "List",
        lua.create_function(|lua, _: ()| -> mlua::Result<Table> {
            let arr = lua.create_table()?;
            let Some(dir) = bin_directory() else {
                return Ok(arr);
            };
            let Ok(read) = std::fs::read_dir(&dir) else {
                return Ok(arr);
            };
            let mut idx: i64 = 0;
            for e in read.flatten() {
                let p = e.path();
                if !p.is_file() {
                    continue;
                }
                let Some(name) = p.file_name().and_then(|n| n.to_str()) else {
                    continue;
                };
                if !is_native_library(name) {
                    continue;
                }
                idx += 1;
                arr.set(idx, name.to_string())?;
            }
            Ok(arr)
        })?,
    )?;

    let begin_async = lua.create_function(
        |lua, (lib_ud, fn_name, args): (AnyUserData, String, Value)| -> mlua::Result<()> {
            let lib = lib_ud.borrow::<FFILibrary>()?;
            let state_arc = lib.state.clone();
            let payload = match &args {
                Value::Nil => "null".to_string(),
                _ => lua_value_to_json(&args).map_err(|e| {
                    mlua::Error::RuntimeError(format!(
                        "FFI.CallAsync: argument must be JSON-serialisable: {e}"
                    ))
                })?,
            };
            let thread = lua.current_thread();
            let key = lua.create_registry_value(thread)?;
            let id = NEXT_ASYNC_ID.fetch_add(1, Ordering::Relaxed);
            PENDING_ASYNC.with(|c| {
                c.borrow_mut().insert(id, key);
            });
            std::thread::spawn(move || {
                let outcome = run_ffi_call_blocking(&state_arc, &fn_name, &payload);
                let mut q = completed_queue().lock().unwrap();
                q.push(AsyncResult {
                    id,
                    payload: outcome,
                });
            });
            Ok(())
        },
    )?;
    t.set("_BeginAsyncCall", begin_async)?;

    t.set(
        "RegisterSignal",
        lua.create_function(
            |lua, (name, sig): (String, mlua::Table)| -> mlua::Result<()> {
                let key = lua.create_registry_value(sig)?;
                NAMED_SIGNALS.with(|c| {
                    let mut map = c.borrow_mut();
                    if let Some(prev) = map.insert(name, key) {
                        let _ = lua.remove_registry_value(prev);
                    }
                });
                Ok(())
            },
        )?,
    )?;

    t.set(
        "UnregisterSignal",
        lua.create_function(|lua, name: String| -> mlua::Result<bool> {
            let removed = NAMED_SIGNALS.with(|c| c.borrow_mut().remove(&name));
            Ok(match removed {
                Some(k) => {
                    let _ = lua.remove_registry_value(k);
                    true
                }
                None => false,
            })
        })?,
    )?;

    t.set(
        "SetData",
        lua.create_function(|_, (key, value): (String, Value)| -> mlua::Result<()> {
            let json = lua_value_to_json(&value).map_err(mlua::Error::RuntimeError)?;
            let parsed: serde_json::Value =
                serde_json::from_str(&json).map_err(|e| mlua::Error::RuntimeError(e.to_string()))?;
            world_store().lock().unwrap().insert(key, parsed);
            Ok(())
        })?,
    )?;

    t.set(
        "GetData",
        lua.create_function(|lua, key: String| -> mlua::Result<Value> {
            let val = world_store().lock().unwrap().get(&key).cloned();
            match val {
                Some(v) => json_to_lua_value(lua, &v),
                None => Ok(Value::Nil),
            }
        })?,
    )?;

    t.set(
        "HasData",
        lua.create_function(|_, key: String| -> mlua::Result<bool> {
            Ok(world_store().lock().unwrap().contains_key(&key))
        })?,
    )?;

    t.set(
        "DeleteData",
        lua.create_function(|_, key: String| -> mlua::Result<bool> {
            Ok(world_store().lock().unwrap().remove(&key).is_some())
        })?,
    )?;

    let wrapper = lua
        .load(
            r#"
local FFI = ...
FFI.CallAsync = function(handle, name, args)
    FFI._BeginAsyncCall(handle, name, args)
    return coroutine.yield()
end
"#,
        )
        .into_function()?;
    wrapper.call::<()>(t.clone())?;

    Ok(t)
}

fn run_ffi_call_blocking(
    state_arc: &Arc<Mutex<Option<LoadedLib>>>,
    fn_name: &str,
    payload: &str,
) -> Result<String, String> {
    let guard = state_arc.lock().map_err(|e| e.to_string())?;
    let loaded = guard.as_ref().ok_or_else(|| "library has been unloaded".to_string())?;
    let fn_cstr = CString::new(fn_name).map_err(|_| "export name has NUL bytes".to_string())?;
    let args_cstr =
        CString::new(payload).map_err(|_| "arguments JSON has NUL bytes".to_string())?;
    unsafe {
        let call: Symbol<FfiCallFn> =
            loaded.lib.get(b"ruzit_ffi_call\0").map_err(|e| e.to_string())?;
        let free: Symbol<FfiFreeFn> =
            loaded.lib.get(b"ruzit_ffi_free\0").map_err(|e| e.to_string())?;
        let ret = call(fn_cstr.as_ptr(), args_cstr.as_ptr());
        if ret.is_null() {
            return Ok(String::new());
        }
        let result = CStr::from_ptr(ret).to_string_lossy().into_owned();
        free(ret);
        Ok(result)
    }
}

pub fn pump(lua: &Lua) {
    let main_tasks: Vec<MainTask> = {
        let mut q = main_task_queue().lock().unwrap();
        std::mem::take(&mut *q)
    };
    for task in main_tasks {
        match task {
            MainTask::FireNamedSignal { name, args } => {
                let key = NAMED_SIGNALS.with(|c| c.borrow().get(&name).map(|k| {

                    let v: mlua::Result<mlua::Table> = lua.registry_value(k);
                    v
                }));
                if let Some(Ok(sig)) = key {
                    let ma = match json_value_to_multivalue(lua, &args) {
                        Ok(m) => m,
                        Err(e) => {
                            eprintln!("[FFI] Signal.Fire {name}: bad args: {e}");
                            continue;
                        }
                    };
                    if let Err(e) = crate::libs::signal::fire(lua, &sig, ma) {
                        eprintln!("[FFI] Signal.Fire {name}: {e}");
                    }
                } else {
                    eprintln!(
                        "[FFI] Signal.Fire: no signal registered under name '{name}'"
                    );
                }
            }
            MainTask::SetCamera {
                cframe,
                fov,
                near,
                far,
            } => {
                if let Some(cf) = cframe {
                    crate::libs::renderable::set_camera_cframe(cf);
                }
                if fov.is_some() || near.is_some() || far.is_some() {
                    let mut snap = crate::libs::renderable::camera_snapshot();
                    if let Some(v) = fov {
                        snap.fov_deg = v.clamp(1.0, 179.0);
                    }
                    if let Some(v) = near {
                        snap.near = v.max(0.001);
                    }
                    if let Some(v) = far {
                        snap.far = v.max(0.01);
                    }
                    crate::libs::renderable::set_camera_fov_near_far(
                        snap.fov_deg,
                        snap.near,
                        snap.far,
                    );
                }
            }
            MainTask::LuaPrint(msg) => {
                println!("{msg}");
            }
        }
    }

    let drained: Vec<AsyncResult> = {
        let mut q = completed_queue().lock().unwrap();
        std::mem::take(&mut *q)
    };
    if drained.is_empty() {
        return;
    }
    for done in drained {
        let key = PENDING_ASYNC.with(|c| c.borrow_mut().remove(&done.id));
        let Some(key) = key else { continue };
        let thread: Thread = match lua.registry_value(&key) {
            Ok(t) => t,
            Err(_) => {
                let _ = lua.remove_registry_value(key);
                continue;
            }
        };
        let _ = lua.remove_registry_value(key);

        match done.payload {
            Ok(json) => {
                let val = if json.is_empty() {
                    Value::Nil
                } else {
                    match serde_json::from_str::<serde_json::Value>(&json) {
                        Ok(parsed) => json_to_lua_value(lua, &parsed).unwrap_or(Value::Nil),
                        Err(e) => {
                            eprintln!("[FFI] async result malformed JSON: {e}");
                            Value::Nil
                        }
                    }
                };
                if let Err(e) = thread.resume::<MultiValue>(val) {
                    eprintln!("[FFI] async resume error: {e}");
                }
            }
            Err(e) => {
                let err_table = lua
                    .create_table()
                    .ok()
                    .and_then(|t| {
                        t.set("error", e.clone()).ok()?;
                        Some(Value::Table(t))
                    })
                    .unwrap_or(Value::Nil);
                if let Err(e2) = thread.resume::<MultiValue>(err_table) {
                    eprintln!("[FFI] async resume (err path) error: {e2}; original: {e}");
                }
            }
        }
    }
}

#[allow(dead_code)]
fn _unused_function_ref<'a>() -> Option<Function> {
    None
}

fn is_native_library(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    n.ends_with(".dll") || n.ends_with(".so") || n.ends_with(".dylib")
}

fn bin_directory() -> Option<PathBuf> {
    if let Some(p) = project_bin_dir() {
        return Some(p);
    }
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?.join("bin");
    if dir.is_dir() { Some(dir) } else { None }
}

fn resolve_lib_path(name: &str) -> mlua::Result<PathBuf> {
    let direct = Path::new(name);
    if direct.is_absolute() && direct.is_file() {
        return Ok(direct.to_path_buf());
    }

    let candidates = candidate_filenames(name);

    let mut search_dirs: Vec<PathBuf> = Vec::new();
    if let Some(p) = project_bin_dir() {
        search_dirs.push(p);
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(parent) = exe.parent() {
            let bin_next = parent.join("bin");
            if bin_next.is_dir() {
                search_dirs.push(bin_next);
            }
            search_dirs.push(parent.to_path_buf());
        }
    }

    for dir in &search_dirs {
        for filename in &candidates {
            let p = dir.join(filename);
            if p.is_file() {
                return Ok(p);
            }
        }
    }

    Err(mlua::Error::RuntimeError(format!(
        "FFI.Load: could not find native library for '{name}'. Tried {} in {} location(s) (bin/ next to the exe, the exe's directory, and the project's bin/ during test).",
        candidates.join(", "),
        search_dirs.len()
    )))
}

fn candidate_filenames(name: &str) -> Vec<String> {
    let lower = name.to_ascii_lowercase();
    let has_ext = lower.ends_with(".dll")
        || lower.ends_with(".so")
        || lower.ends_with(".dylib")
        || lower.contains('.');
    let mut out: Vec<String> = Vec::new();
    if has_ext {
        out.push(name.to_string());
    } else if cfg!(target_os = "windows") {
        out.push(format!("{name}.dll"));
    } else if cfg!(target_os = "macos") {
        out.push(format!("lib{name}.dylib"));
        out.push(format!("{name}.dylib"));
    } else {
        out.push(format!("lib{name}.so"));
        out.push(format!("{name}.so"));
    }
    out
}

impl UserData for FFILibrary {
    fn add_fields<F: UserDataFields<Self>>(f: &mut F) {
        f.add_field_method_get("Alive", |_, this| {
            Ok(this.state.lock().unwrap().is_some())
        });
        f.add_field_method_get("Source", |_, this| {
            Ok(this
                .state
                .lock()
                .unwrap()
                .as_ref()
                .map(|l| l.source.clone())
                .unwrap_or_default())
        });
    }

    fn add_methods<M: UserDataMethods<Self>>(m: &mut M) {
        m.add_method("Call", |lua, this, args: MultiValue| -> mlua::Result<Value> {
            let mut iter = args.into_iter();
            let fn_name = match iter.next() {
                Some(Value::String(s)) => s.to_str()?.to_string(),
                _ => {
                    return Err(mlua::Error::RuntimeError(
                        "FFILibrary:Call: first argument must be the export name (string)".into(),
                    ));
                }
            };
            let payload = match iter.next() {
                None | Some(Value::Nil) => "null".to_string(),
                Some(v) => lua_value_to_json(&v).map_err(|e| {
                    mlua::Error::RuntimeError(format!(
                        "FFILibrary:Call: argument must be JSON-serialisable: {e}"
                    ))
                })?,
            };

            let guard = this.state.lock().unwrap();
            let loaded = guard.as_ref().ok_or_else(|| {
                mlua::Error::RuntimeError(
                    "FFILibrary:Call: library has been unloaded".into(),
                )
            })?;
            let fn_cstr = CString::new(fn_name.as_bytes()).map_err(|_| {
                mlua::Error::RuntimeError(
                    "FFILibrary:Call: export name must not contain NUL bytes".into(),
                )
            })?;
            let args_cstr = CString::new(payload.as_bytes()).map_err(|_| {
                mlua::Error::RuntimeError(
                    "FFILibrary:Call: arguments JSON must not contain NUL bytes".into(),
                )
            })?;
            let call: Symbol<FfiCallFn> =
                unsafe { loaded.lib.get(b"ruzit_ffi_call\0") }.map_err(|e| {
                    mlua::Error::RuntimeError(format!(
                        "FFILibrary:Call: missing `ruzit_ffi_call`: {e}"
                    ))
                })?;
            let free: Symbol<FfiFreeFn> =
                unsafe { loaded.lib.get(b"ruzit_ffi_free\0") }.map_err(|e| {
                    mlua::Error::RuntimeError(format!(
                        "FFILibrary:Call: missing `ruzit_ffi_free`: {e}"
                    ))
                })?;

            let ret_ptr = unsafe { call(fn_cstr.as_ptr(), args_cstr.as_ptr()) };
            if ret_ptr.is_null() {
                return Ok(Value::Nil);
            }
            let json_str = unsafe { CStr::from_ptr(ret_ptr) }
                .to_string_lossy()
                .into_owned();
            unsafe { free(ret_ptr) };

            if json_str.is_empty() {
                return Ok(Value::Nil);
            }
            let parsed: serde_json::Value = serde_json::from_str(&json_str).map_err(|e| {
                mlua::Error::RuntimeError(format!(
                    "FFILibrary:Call: '{fn_name}' returned malformed JSON: {e}"
                ))
            })?;
            json_to_lua_value(lua, &parsed)
        });

        m.add_method("Unload", |_, this, _: ()| -> mlua::Result<()> {
            let mut slot = this.state.lock().unwrap();
            slot.take();
            Ok(())
        });
    }
}

fn lua_value_to_json(v: &Value) -> Result<String, String> {
    let json = lua_to_json_value(v)?;
    serde_json::to_string(&json).map_err(|e| e.to_string())
}

fn lua_to_json_value(v: &Value) -> Result<serde_json::Value, String> {
    use serde_json::Value as J;
    match v {
        Value::Nil => Ok(J::Null),
        Value::Boolean(b) => Ok(J::Bool(*b)),
        Value::Integer(i) => Ok(J::Number((*i as i64).into())),
        Value::Number(n) => Ok(serde_json::Number::from_f64(*n)
            .map(J::Number)
            .unwrap_or(J::Null)),
        Value::String(s) => Ok(J::String(s.to_str().map_err(|e| e.to_string())?.to_string())),
        Value::UserData(ud) => {
            if let Ok(vec) = ud.borrow::<Vector>() {
                return Ok(serde_json::json!({
                    "_type": "Vector",
                    "x": vec.x, "y": vec.y, "z": vec.z,
                }));
            }
            if let Ok(c) = ud.borrow::<Color3>() {
                return Ok(serde_json::json!({
                    "_type": "Color3",
                    "r": c.r, "g": c.g, "b": c.b,
                }));
            }
            if let Ok(cf) = ud.borrow::<CFrame>() {
                return Ok(serde_json::json!({
                    "_type": "CFrame",
                    "position": { "x": cf.position.x, "y": cf.position.y, "z": cf.position.z },
                    "rotation": { "x": cf.rotation.x, "y": cf.rotation.y, "z": cf.rotation.z },
                }));
            }
            if let Ok(prim) = ud.borrow::<crate::libs::gui::GuiPrimitive>() {
                let id = register_handle(FfiHandle::Primitive(prim.state_arc()));
                return Ok(serde_json::json!({
                    "_type":   "PrimitiveHandle",
                    "_handle": id,
                }));
            }
            if let Ok(part) = ud.borrow::<crate::libs::renderable::PartHandle>() {
                let id = register_handle(FfiHandle::Part(part.state.clone()));
                return Ok(serde_json::json!({
                    "_type":   "PartHandle",
                    "_handle": id,
                }));
            }
            if let Ok(drawable) = ud.borrow::<crate::libs::drawable::DrawableImgHandle>() {
                let buf = {
                    let inner = drawable.inner.lock().map_err(|e| e.to_string())?;
                    inner.buffer.clone()
                };
                let id = register_handle(FfiHandle::DrawableImg(buf));
                return Ok(serde_json::json!({
                    "_type":   "DrawableImgHandle",
                    "_handle": id,
                }));
            }
            Err("unsupported userdata: pass a Vector/Color3/CFrame, a GUI Primitive, a BasePart, or a DrawableImg".into())
        }
        Value::Table(t) => {
            let len = t.raw_len();
            if len > 0 {
                let mut arr: Vec<J> = Vec::with_capacity(len as usize);
                for i in 1..=len {
                    let v: Value = t.get(i).map_err(|e| e.to_string())?;
                    arr.push(lua_to_json_value(&v)?);
                }
                Ok(J::Array(arr))
            } else {
                let mut map = serde_json::Map::new();
                for pair in t.clone().pairs::<Value, Value>() {
                    let (k, val) = pair.map_err(|e| e.to_string())?;
                    let key = match k {
                        Value::String(s) => s.to_str().map_err(|e| e.to_string())?.to_string(),
                        Value::Integer(i) => i.to_string(),
                        Value::Number(n) => n.to_string(),
                        _ => return Err("only string/number keys are supported".into()),
                    };
                    map.insert(key, lua_to_json_value(&val)?);
                }
                Ok(J::Object(map))
            }
        }
        _ => Err(format!("unsupported argument type: {v:?}")),
    }
}

fn json_value_to_multivalue(lua: &Lua, v: &serde_json::Value) -> mlua::Result<MultiValue> {
    let mut out = MultiValue::new();
    match v {
        serde_json::Value::Null => {}
        serde_json::Value::Array(arr) => {
            for item in arr {
                out.push_back(json_to_lua_value(lua, item)?);
            }
        }
        other => {
            out.push_back(json_to_lua_value(lua, other)?);
        }
    }
    Ok(out)
}

fn json_to_lua_value(lua: &Lua, v: &serde_json::Value) -> mlua::Result<Value> {
    use serde_json::Value as J;
    Ok(match v {
        J::Null => Value::Nil,
        J::Bool(b) => Value::Boolean(*b),
        J::Number(n) => {
            if let Some(i) = n.as_i64() {
                if i >= i32::MIN as i64 && i <= i32::MAX as i64 {
                    Value::Integer(i as i32)
                } else {
                    Value::Number(i as f64)
                }
            } else if let Some(f) = n.as_f64() {
                Value::Number(f)
            } else {
                Value::Nil
            }
        }
        J::String(s) => Value::String(lua.create_string(s)?),
        J::Array(arr) => {
            let t = lua.create_table()?;
            for (i, item) in arr.iter().enumerate() {
                t.set(i as i64 + 1, json_to_lua_value(lua, item)?)?;
            }
            Value::Table(t)
        }
        J::Object(map) => {
            let t = lua.create_table()?;
            for (k, val) in map.iter() {
                t.set(k.as_str(), json_to_lua_value(lua, val)?)?;
            }
            Value::Table(t)
        }
    })
}

#[allow(dead_code)]
pub fn ud_borrow_lib(ud: &AnyUserData) -> Option<FFILibrary> {
    ud.borrow::<FFILibrary>().ok().map(|h| FFILibrary {
        state: h.state.clone(),
    })
}
