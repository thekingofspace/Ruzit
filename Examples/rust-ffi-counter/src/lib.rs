use std::collections::HashMap;
use std::ffi::{CStr, CString};
use std::os::raw::c_char;
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

use serde_json::{json, Value};

#[repr(C)]
#[derive(Clone, Copy)]
pub struct RuzitHost {
    pub call: extern "C" fn(name: *const c_char, args: *const c_char) -> *mut c_char,
    pub free: extern "C" fn(ptr: *mut c_char),
}

static HOST: OnceLock<RuzitHost> = OnceLock::new();

#[unsafe(no_mangle)]
pub extern "C" fn ruzit_ffi_init(host: RuzitHost) {
    let _ = HOST.set(host);
}

fn host_call(name: &str, args: &Value) -> Value {
    let Some(host) = HOST.get() else {
        return json!({ "error": "host vtable not bound (ruzit_ffi_init wasn't called)" });
    };
    let name_c = match CString::new(name) {
        Ok(c) => c,
        Err(_) => return json!({ "error": "name has NUL bytes" }),
    };
    let args_c = match CString::new(args.to_string()) {
        Ok(c) => c,
        Err(_) => return json!({ "error": "args has NUL bytes" }),
    };
    let ret = (host.call)(name_c.as_ptr(), args_c.as_ptr());
    if ret.is_null() {
        return Value::Null;
    }
    let s = unsafe { CStr::from_ptr(ret) }
        .to_string_lossy()
        .into_owned();
    (host.free)(ret);
    serde_json::from_str(&s).unwrap_or(Value::Null)
}

static COUNTER: Mutex<i64> = Mutex::new(0);

#[derive(Clone, Copy, Debug)]
struct Particle {
    name_id: u64,
    pos: [f32; 3],
    vel: [f32; 3],
}

struct World {
    next_id: u64,
    by_id: HashMap<u64, Particle>,
    by_name: HashMap<String, u64>,
    gravity: f32,
}

impl Default for World {
    fn default() -> Self {
        Self {
            next_id: 1,
            by_id: HashMap::new(),
            by_name: HashMap::new(),
            gravity: -9.81,
        }
    }
}

fn world() -> &'static Mutex<World> {
    use std::sync::OnceLock;
    static WORLD: OnceLock<Mutex<World>> = OnceLock::new();
    WORLD.get_or_init(|| Mutex::new(World::default()))
}

fn pick_vec(v: &Value, key: &str) -> Option<[f32; 3]> {
    let node = v.get(key)?;
    let x = node.get("x")?.as_f64()? as f32;
    let y = node.get("y")?.as_f64()? as f32;
    let z = node.get("z")?.as_f64()? as f32;
    Some([x, y, z])
}

fn vec_json(v: [f32; 3]) -> Value {
    json!({ "_type": "Vector", "x": v[0], "y": v[1], "z": v[2] })
}

fn error_json(export: &str, message: &str) -> Value {
    json!({ "error": format!("{export}: {message}") })
}

#[unsafe(no_mangle)]
pub extern "C" fn ruzit_ffi_call(name: *const c_char, args: *const c_char) -> *mut c_char {
    if name.is_null() {
        return std::ptr::null_mut();
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
    let parsed: Value = serde_json::from_str(&args_str).unwrap_or(Value::Null);

    let result = match name.as_str() {
        "increment" => {
            let step = parsed.get("step").and_then(|v| v.as_i64()).unwrap_or(1);
            let mut c = COUNTER.lock().unwrap();
            *c += step;
            json!(*c)
        }
        "decrement" => {
            let step = parsed.get("step").and_then(|v| v.as_i64()).unwrap_or(1);
            let mut c = COUNTER.lock().unwrap();
            *c -= step;
            json!(*c)
        }
        "get" => json!(*COUNTER.lock().unwrap()),
        "set" => {
            let value = parsed.get("value").and_then(|v| v.as_i64()).unwrap_or(0);
            let mut c = COUNTER.lock().unwrap();
            *c = value;
            json!(*c)
        }
        "reset" => {
            *COUNTER.lock().unwrap() = 0;
            Value::Null
        }
        "echo" => parsed,
        "version" => json!({
            "name":    "ruzit-ffi-counter",
            "version": env!("CARGO_PKG_VERSION"),
        }),

        "slow_compute" => {
            let ms = parsed.get("ms").and_then(|v| v.as_u64()).unwrap_or(250);
            let n = parsed.get("n").and_then(|v| v.as_u64()).unwrap_or(1_000_000);
            let started = Instant::now();
            std::thread::sleep(std::time::Duration::from_millis(ms));
            let mut acc: f64 = 0.0;
            for i in 0..n {
                let x = i as f64;
                acc += x * x;
            }
            json!({
                "result_squared_sum": acc,
                "elapsed_ms":         started.elapsed().as_secs_f64() * 1000.0,
                "thread":             format!("{:?}", std::thread::current().id()),
            })
        }

        "spawn_particle" => {
            let name = parsed
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or("anon")
                .to_string();
            let pos = pick_vec(&parsed, "position").unwrap_or([0.0, 0.0, 0.0]);
            let vel = pick_vec(&parsed, "velocity").unwrap_or([0.0, 0.0, 0.0]);
            let mut w = world().lock().unwrap();
            let id = w.next_id;
            w.next_id += 1;
            w.by_id.insert(id, Particle { name_id: id, pos, vel });
            w.by_name.insert(name.clone(), id);
            json!({ "id": id, "name": name, "position": vec_json(pos) })
        }
        "set_gravity" => {
            let g = parsed.get("g").and_then(|v| v.as_f64()).unwrap_or(-9.81) as f32;
            world().lock().unwrap().gravity = g;
            json!({ "gravity": g })
        }
        "tick_world" => {
            let dt = parsed.get("dt").and_then(|v| v.as_f64()).unwrap_or(1.0 / 60.0) as f32;
            let mut w = world().lock().unwrap();
            let g = w.gravity;
            for p in w.by_id.values_mut() {
                p.vel[1] += g * dt;
                p.pos[0] += p.vel[0] * dt;
                p.pos[1] += p.vel[1] * dt;
                p.pos[2] += p.vel[2] * dt;
            }
            json!({ "stepped": w.by_id.len(), "gravity": g, "dt": dt })
        }
        "get_particle" => {
            let id_opt = parsed.get("id").and_then(|v| v.as_u64());
            let name_opt = parsed.get("name").and_then(|v| v.as_str());
            let w = world().lock().unwrap();
            let id = match (id_opt, name_opt) {
                (Some(id), _) => Some(id),
                (None, Some(n)) => w.by_name.get(n).copied(),
                _ => None,
            };
            match id.and_then(|i| w.by_id.get(&i)) {
                Some(p) => json!({
                    "id":       p.name_id,
                    "position": vec_json(p.pos),
                    "velocity": vec_json(p.vel),
                }),
                None => Value::Null,
            }
        }
        "all_particles" => {
            let w = world().lock().unwrap();
            let arr: Vec<Value> = w
                .by_id
                .values()
                .map(|p| {
                    json!({
                        "id":       p.name_id,
                        "position": vec_json(p.pos),
                        "velocity": vec_json(p.vel),
                    })
                })
                .collect();
            json!(arr)
        }
        "clear_world" => {
            let mut w = world().lock().unwrap();
            let removed = w.by_id.len();
            w.by_id.clear();
            w.by_name.clear();
            json!({ "removed": removed })
        }

        "orbit_part" => 'orbit: {
            let handle_opt = parsed
                .get("part")
                .and_then(|p| p.get("_handle"))
                .and_then(|v| v.as_u64());
            let Some(handle) = handle_opt else {
                break 'orbit error_json(&name, "missing `part` arg (pass a BasePart)");
            };
            let cx = parsed.get("cx").and_then(|v| v.as_f64()).unwrap_or(0.0);
            let cy = parsed.get("cy").and_then(|v| v.as_f64()).unwrap_or(0.0);
            let cz = parsed.get("cz").and_then(|v| v.as_f64()).unwrap_or(0.0);
            let radius = parsed.get("radius").and_then(|v| v.as_f64()).unwrap_or(5.0);
            let revolutions = parsed
                .get("revolutions")
                .and_then(|v| v.as_f64())
                .unwrap_or(1.0);
            let steps = parsed.get("steps").and_then(|v| v.as_u64()).unwrap_or(120).max(8);
            let duration_ms = parsed
                .get("duration_ms")
                .and_then(|v| v.as_u64())
                .unwrap_or(2000);
            std::thread::spawn(move || {
                let dt_ms = duration_ms / steps;
                for i in 0..=steps {
                    let t = i as f64 / steps as f64;
                    let theta = t * revolutions * std::f64::consts::TAU;
                    let x = cx + radius * theta.cos();
                    let z = cz + radius * theta.sin();
                    let _ = host_call(
                        "BasePart.SetCFrame",
                        &json!({
                            "handle": handle,
                            "position": { "x": x, "y": cy, "z": z },
                            "rotation": { "x": 0.0, "y": -theta, "z": 0.0 },
                        }),
                    );
                    if i != steps {
                        std::thread::sleep(std::time::Duration::from_millis(dt_ms));
                    }
                }
                let _ = host_call(
                    "Signal.Fire",
                    &json!({
                        "name": "ffi.orbit_done",
                        "args": [{ "handle": handle, "elapsed_ms": duration_ms }],
                    }),
                );
            });
            json!({ "started": true, "handle": handle, "duration_ms": duration_ms })
        }

        "paint_drawable" => 'paint: {
            let handle_opt = parsed
                .get("img")
                .and_then(|p| p.get("_handle"))
                .and_then(|v| v.as_u64());
            let Some(handle) = handle_opt else {
                break 'paint error_json(&name, "missing `img` arg (pass a DrawableImg)");
            };
            let style = parsed
                .get("style")
                .and_then(|v| v.as_str())
                .unwrap_or("waves")
                .to_string();
            let style_for_reply = style.clone();
            std::thread::spawn(move || {
                let size = host_call("DrawableImg.GetSize", &json!({ "handle": handle }));
                let w = size.get("width").and_then(|v| v.as_u64()).unwrap_or(0);
                let h = size.get("height").and_then(|v| v.as_u64()).unwrap_or(0);
                let _ = host_call(
                    "DrawableImg.Clear",
                    &json!({ "handle": handle }),
                );
                if w == 0 || h == 0 {
                    return;
                }
                match style.as_str() {
                    "waves" => {
                        for y in 0..h {
                            for x in 0..w {
                                let fx = x as f64 / w as f64;
                                let fy = y as f64 / h as f64;
                                let r = 0.5 + 0.5 * ((fx * 24.0).sin() * (fy * 12.0).cos());
                                let g = 0.5 + 0.5 * ((fx * 18.0 + fy * 4.0).sin());
                                let b = 0.5 + 0.5 * ((fy * 16.0).cos());
                                let _ = host_call(
                                    "DrawableImg.WritePixel",
                                    &json!({
                                        "handle": handle,
                                        "x": x, "y": y,
                                        "r": r, "g": g, "b": b, "a": 1.0,
                                    }),
                                );
                            }
                        }
                    }
                    "checker" => {
                        let cell = (w / 8).max(1) as i64;
                        for y in 0..h as i64 {
                            for x in 0..w as i64 {
                                let on = ((x / cell) + (y / cell)) % 2 == 0;
                                let v = if on { 0.9 } else { 0.1 };
                                let _ = host_call(
                                    "DrawableImg.WritePixel",
                                    &json!({
                                        "handle": handle,
                                        "x": x, "y": y,
                                        "r": v, "g": v, "b": v, "a": 1.0,
                                    }),
                                );
                            }
                        }
                    }
                    _ => {
                        let _ = host_call(
                            "DrawableImg.Fill",
                            &json!({
                                "handle": handle,
                                "r": 0.2, "g": 0.6, "b": 0.9, "a": 1.0,
                            }),
                        );
                    }
                }
                let _ = host_call(
                    "Logging.Print",
                    &json!({
                        "prefix":  "ffi-counter",
                        "message": format!("paint_drawable[{style}] done on handle {handle}"),
                    }),
                );
                let _ = host_call(
                    "Signal.Fire",
                    &json!({
                        "name": "ffi.paint_done",
                        "args": [{ "handle": handle, "style": style }],
                    }),
                );
            });
            json!({ "started": true, "handle": handle, "style": style_for_reply })
        }

        "world_set" => {
            let key = parsed.get("key").and_then(|v| v.as_str()).unwrap_or("");
            let value = parsed.get("value").cloned().unwrap_or(Value::Null);
            let _ = host_call(
                "World.Set",
                &json!({ "key": key, "value": value }),
            );
            json!({ "key": key })
        }
        "world_get" => {
            let key = parsed.get("key").and_then(|v| v.as_str()).unwrap_or("");
            host_call("World.Get", &json!({ "key": key }))
        }

        "tween_color" => 'tween: {
            let handle_opt = parsed
                .get("primitive")
                .and_then(|p| p.get("_handle"))
                .and_then(|v| v.as_u64());
            let Some(handle) = handle_opt else {
                break 'tween error_json(&name, "missing `primitive` arg (pass a GUI Primitive)");
            };
            let from = parsed
                .get("from")
                .cloned()
                .unwrap_or_else(|| json!({ "r": 1.0, "g": 0.0, "b": 0.0 }));
            let to = parsed
                .get("to")
                .cloned()
                .unwrap_or_else(|| json!({ "r": 0.0, "g": 0.0, "b": 1.0 }));
            let duration_ms = parsed
                .get("duration_ms")
                .and_then(|v| v.as_u64())
                .unwrap_or(1000);
            let steps = parsed.get("steps").and_then(|v| v.as_u64()).unwrap_or(60).max(2);
            std::thread::spawn(move || {
                let fr = from.get("r").and_then(|v| v.as_f64()).unwrap_or(1.0);
                let fg = from.get("g").and_then(|v| v.as_f64()).unwrap_or(0.0);
                let fb = from.get("b").and_then(|v| v.as_f64()).unwrap_or(0.0);
                let tr = to.get("r").and_then(|v| v.as_f64()).unwrap_or(0.0);
                let tg = to.get("g").and_then(|v| v.as_f64()).unwrap_or(0.0);
                let tb = to.get("b").and_then(|v| v.as_f64()).unwrap_or(1.0);
                let dt_ms = duration_ms / steps;
                for i in 0..=steps {
                    let t = i as f64 / steps as f64;
                    let r = fr + (tr - fr) * t;
                    let g = fg + (tg - fg) * t;
                    let b = fb + (tb - fb) * t;
                    let _ = host_call(
                        "Primitive.SetColor",
                        &json!({ "handle": handle, "r": r, "g": g, "b": b }),
                    );
                    if i != steps {
                        std::thread::sleep(std::time::Duration::from_millis(dt_ms));
                    }
                }
                let _ = host_call(
                    "Logging.Print",
                    &json!({
                        "prefix":  "ffi-counter",
                        "message": format!("tween_color done on handle {handle}"),
                    }),
                );
            });
            json!({ "started": true, "handle": handle, "duration_ms": duration_ms })
        }

        _ => json!({ "error": format!("unknown export: {name}") }),
    };

    let s = result.to_string();
    match CString::new(s) {
        Ok(c) => c.into_raw(),
        Err(_) => std::ptr::null_mut(),
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn ruzit_ffi_free(ptr: *mut c_char) {
    if !ptr.is_null() {
        unsafe {
            let _ = CString::from_raw(ptr);
        }
    }
}

// ─── Native mlua section ──────────────────────────────────────────────
//
// Optional ABI. When the host loads the DLL it calls
// `ruzit_ffi_init_native(lua_state, table_name)` if the symbol exists.
// We construct a Lua wrapper from the raw state and populate the table
// the host pre-created for us. Anything we put on that table becomes
// reachable in Luau as `lib:GetNative().<name>`.
//
// Use it when:
//   * you want to receive real userdata (Vector, Color3, CFrame, etc.) and
//     mutate them without JSON marshalling, OR
//   * you want to define your own UserData types in Rust and hand them
//     back to Luau as first-class objects.
//
// SAFETY:
//   * host + DLL must link the same mlua version (this Cargo.toml pins
//     mlua = "=0.11.6"). A version drift is undefined behaviour.
//   * never store an `mlua::RegistryKey` across sides — each binary has
//     its own ID counter / drop tracking.
//   * if you unload the DLL (`lib:Unload()` or process exit), every
//     callback you registered becomes dangling. Either don't unload, or
//     clear `GetNative()` entries first from Luau.

#[derive(Clone, Copy, Debug)]
pub struct PointHandle {
    pub x: f32,
    pub y: f32,
}

impl mlua::UserData for PointHandle {
    fn add_fields<F: mlua::UserDataFields<Self>>(f: &mut F) {
        f.add_field_method_get("X", |_, this| Ok(this.x as f64));
        f.add_field_method_set("X", |_, this, v: f64| {
            this.x = v as f32;
            Ok(())
        });
        f.add_field_method_get("Y", |_, this| Ok(this.y as f64));
        f.add_field_method_set("Y", |_, this, v: f64| {
            this.y = v as f32;
            Ok(())
        });
    }

    fn add_methods<M: mlua::UserDataMethods<Self>>(m: &mut M) {
        m.add_method("Length", |_, this, _: ()| {
            Ok(((this.x * this.x + this.y * this.y) as f64).sqrt())
        });
        m.add_method_mut("Translate", |_, this, (dx, dy): (f64, f64)| {
            this.x += dx as f32;
            this.y += dy as f32;
            Ok(())
        });
        m.add_meta_method("__tostring", |_, this, _: ()| {
            Ok(format!("Point({}, {})", this.x, this.y))
        });
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ruzit_ffi_init_native(
    state: *mut mlua::lua_State,
    table_name: *const c_char,
) -> bool {
    if state.is_null() || table_name.is_null() {
        return false;
    }
    let name = match unsafe { CStr::from_ptr(table_name) }.to_str() {
        Ok(s) => s.to_string(),
        Err(_) => return false,
    };
    let lua: &mlua::Lua = unsafe { mlua::Lua::get_or_init_from_ptr(state) };

    let table: mlua::Table = match lua.named_registry_value(&name) {
        Ok(t) => t,
        Err(_) => return false,
    };

    // Constructor for our DLL-defined userdata.
    let new_point = match lua.create_function(|_, (x, y): (f64, f64)| {
        Ok(PointHandle {
            x: x as f32,
            y: y as f32,
        })
    }) {
        Ok(f) => f,
        Err(_) => return false,
    };
    if table.set("Point", new_point).is_err() {
        return false;
    }

    // Function that receives a userdata, edits it in place.
    let translate = match lua.create_function(
        |_, (point, dx, dy): (mlua::AnyUserData, f64, f64)| {
            let mut p = point.borrow_mut::<PointHandle>()?;
            p.x += dx as f32;
            p.y += dy as f32;
            Ok(())
        },
    ) {
        Ok(f) => f,
        Err(_) => return false,
    };
    if table.set("Translate", translate).is_err() {
        return false;
    }

    // Function that takes one userdata and returns another (different type).
    let to_polar = match lua.create_function(|_, point: mlua::AnyUserData| {
        let p = point.borrow::<PointHandle>()?;
        let r = ((p.x * p.x + p.y * p.y) as f64).sqrt();
        let theta = (p.y as f64).atan2(p.x as f64);
        Ok((r, theta))
    }) {
        Ok(f) => f,
        Err(_) => return false,
    };
    if table.set("ToPolar", to_polar).is_err() {
        return false;
    }

    true
}
