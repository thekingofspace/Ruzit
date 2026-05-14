// High-level `import("VirtualReality")` API.
//
// Always compiled. When the `vr` Cargo feature is on, the optional
// `indite` crate is linked in and real OpenXR-backed pose updates can
// drive this state on each heart tick (the actual indite↔wgpu bridge
// is staged for a follow-up — indite needs wgpu 28 and we're on wgpu
// 22, so this module currently runs in a no-op backend mode but
// exposes the full Lua API so games can be written against it today).
//
// `HasVrFlag` reports `cfg!(feature = "vr")` so Luau code can branch
// gracefully on the build target.

use std::cell::RefCell;

use mlua::{
    Lua, MultiValue, Table, UserData, UserDataFields, UserDataMethods, Value,
};

use crate::libs::primitives::{CFrame, Vector};
use crate::libs::renderable::set_camera_cframe;
use crate::libs::signal;

thread_local! {
    static STATE: RefCell<VrState> = RefCell::new(VrState::default());
}

struct VrState {
    linked: bool,
    body: CFrame,
    head: CFrame,
    left: ControllerPose,
    right: ControllerPose,

    head_moved: Option<Table>,
    left_moved: Option<Table>,
    right_moved: Option<Table>,
    left_input: Option<Table>,
    right_input: Option<Table>,
}

#[derive(Clone, Copy)]
struct ControllerPose {
    cframe: CFrame,
    velocity: Vector,
    angular_velocity: Vector,
    trigger: f32,
    grip: f32,
    thumbstick: Vector,
    is_connected: bool,
    battery: Option<f32>,
}

impl Default for ControllerPose {
    fn default() -> Self {
        let zero = Vector::new(0.0, 0.0, 0.0);
        Self {
            cframe: CFrame::new(zero, zero),
            velocity: zero,
            angular_velocity: zero,
            trigger: 0.0,
            grip: 0.0,
            thumbstick: zero,
            is_connected: false,
            battery: None,
        }
    }
}

impl Default for VrState {
    fn default() -> Self {
        let zero = Vector::new(0.0, 0.0, 0.0);
        let id = CFrame::new(zero, zero);
        Self {
            linked: false,
            body: id,
            head: id,
            left: ControllerPose::default(),
            right: ControllerPose::default(),
            head_moved: None,
            left_moved: None,
            right_moved: None,
            left_input: None,
            right_input: None,
        }
    }
}

fn with_state<R>(f: impl FnOnce(&VrState) -> R) -> R {
    STATE.with(|s| f(&s.borrow()))
}

fn with_state_mut<R>(f: impl FnOnce(&mut VrState) -> R) -> R {
    STATE.with(|s| f(&mut s.borrow_mut()))
}

pub fn is_vr_present() -> bool {
    if !cfg!(feature = "vr") {
        return false;
    }
    if std::env::var_os("XR_RUNTIME_JSON").is_some() {
        return true;
    }
    if std::env::var_os("OPENVR_RUNTIME").is_some() || std::env::var_os("VR_OVERRIDE").is_some() {
        return true;
    }
    #[cfg(windows)]
    {
        if windows_registry_has_openxr_runtime() {
            return true;
        }
    }
    false
}

#[cfg(windows)]
fn windows_registry_has_openxr_runtime() -> bool {
    use std::process::Command;
    for view in ["", "/reg:64", "/reg:32"] {
        let mut args = vec![
            "QUERY",
            r"HKLM\SOFTWARE\Khronos\OpenXR\1",
            "/v",
            "ActiveRuntime",
        ];
        if !view.is_empty() {
            args.push(view);
        }
        let out = Command::new("reg").args(&args).output();
        if let Ok(out) = out {
            if out.status.success() && !out.stdout.is_empty() {
                return true;
            }
        }
    }
    false
}

fn compose_world(body: CFrame, local: CFrame) -> CFrame {
    CFrame::new(
        Vector::new(
            body.position.x + local.position.x,
            body.position.y + local.position.y,
            body.position.z + local.position.z,
        ),
        Vector::new(
            body.rotation.x + local.rotation.x,
            body.rotation.y + local.rotation.y,
            body.rotation.z + local.rotation.z,
        ),
    )
}

pub fn pump(_lua: &Lua) {
    let (linked, body, head) = with_state(|s| (s.linked, s.body, s.head));
    if !linked {
        return;
    }
    set_camera_cframe(compose_world(body, head));
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Side {
    Left,
    Right,
}

#[derive(Clone)]
pub struct VRController {
    side: Side,
}

impl VRController {
    fn pose(&self) -> ControllerPose {
        with_state(|s| match self.side {
            Side::Left => s.left,
            Side::Right => s.right,
        })
    }
}

impl UserData for VRController {
    fn add_fields<F: UserDataFields<Self>>(f: &mut F) {
        f.add_field_method_get("Side", |_, this| {
            Ok(match this.side {
                Side::Left => "Left",
                Side::Right => "Right",
            })
        });
        f.add_field_method_get("CFrame", |_, this| Ok(this.pose().cframe));
        f.add_field_method_get("Trigger", |_, this| Ok(this.pose().trigger));
        f.add_field_method_get("Grip", |_, this| Ok(this.pose().grip));
        f.add_field_method_get("Thumbstick", |_, this| Ok(this.pose().thumbstick));
        f.add_field_method_get("Velocity", |_, this| Ok(this.pose().velocity));
        f.add_field_method_get("AngularVelocity", |_, this| Ok(this.pose().angular_velocity));
        f.add_field_method_get("IsConnected", |_, this| Ok(this.pose().is_connected));
        f.add_field_method_get("BatteryLevel", |_, this| Ok(this.pose().battery));

        f.add_field_method_get("Moved", |lua, this| {
            let existing = with_state(|s| match this.side {
                Side::Left => s.left_moved.clone(),
                Side::Right => s.right_moved.clone(),
            });
            if let Some(sig) = existing {
                return Ok(sig);
            }
            let sig = signal::new_instance(lua)?;
            with_state_mut(|s| match this.side {
                Side::Left => s.left_moved = Some(sig.clone()),
                Side::Right => s.right_moved = Some(sig.clone()),
            });
            Ok(sig)
        });

        // Fires `(name: string, value: number, "Begin"|"End")` for trigger /
        // grip / thumbstick clicks / button events. Names match the OpenXR
        // input bindings ("Trigger", "Grip", "Thumbstick", "A", "B", "X", "Y",
        // "Menu", "ThumbstickClick").
        f.add_field_method_get("OnInput", |lua, this| {
            let existing = with_state(|s| match this.side {
                Side::Left => s.left_input.clone(),
                Side::Right => s.right_input.clone(),
            });
            if let Some(sig) = existing {
                return Ok(sig);
            }
            let sig = signal::new_instance(lua)?;
            with_state_mut(|s| match this.side {
                Side::Left => s.left_input = Some(sig.clone()),
                Side::Right => s.right_input = Some(sig.clone()),
            });
            Ok(sig)
        });
    }

    fn add_methods<M: UserDataMethods<Self>>(m: &mut M) {
        m.add_method("ToWorldSpace", |_, this, _: ()| -> mlua::Result<CFrame> {
            let body = with_state(|s| s.body);
            Ok(compose_world(body, this.pose().cframe))
        });

        m.add_method(
            "Vibrate",
            |_, _this, (_duration, _frequency, _amplitude): (f32, Option<f32>, Option<f32>)|
             -> mlua::Result<bool> { Ok(false) },
        );

        m.add_method("StopVibration", |_, _this, _: ()| -> mlua::Result<()> {
            Ok(())
        });
    }
}

pub fn create(lua: &Lua) -> mlua::Result<Table> {
    let api = lua.create_table()?;

    api.set("HasVrFlag", cfg!(feature = "vr"))?;
    api.set("IsVrPresent", is_vr_present())?;

    let m = lua.create_table()?;

    m.set("__index", lua.create_function(|lua, (_t, key): (Table, String)| -> mlua::Result<Value> {
        match key.as_str() {
            "BodyCFrame" => {
                let cf = with_state(|s| s.body);
                Ok(Value::UserData(lua.create_userdata(cf)?))
            }
            "HeadCFrame" => {
                let cf = with_state(|s| s.head);
                Ok(Value::UserData(lua.create_userdata(cf)?))
            }
            "IsLinked" => Ok(Value::Boolean(with_state(|s| s.linked))),
            "HeadMoved" => {
                if let Some(sig) = with_state(|s| s.head_moved.clone()) {
                    return Ok(Value::Table(sig));
                }
                let sig = signal::new_instance(lua)?;
                with_state_mut(|s| s.head_moved = Some(sig.clone()));
                Ok(Value::Table(sig))
            }
            _ => Ok(Value::Nil),
        }
    })?)?;

    m.set("__newindex", lua.create_function(|_, (_t, key, value): (Table, String, Value)| -> mlua::Result<()> {
        match key.as_str() {
            "BodyCFrame" => {
                let cf = match value {
                    Value::UserData(ud) => *ud
                        .borrow::<CFrame>()
                        .map_err(|_| mlua::Error::RuntimeError("BodyCFrame expects a CFrame".into()))?,
                    _ => return Err(mlua::Error::RuntimeError("BodyCFrame expects a CFrame".into())),
                };
                with_state_mut(|s| s.body = cf);
                Ok(())
            }
            other => Err(mlua::Error::RuntimeError(format!(
                "VirtualReality: '{other}' is read-only or not settable"
            ))),
        }
    })?)?;

    api.set_metatable(Some(m));

    api.set(
        "LinkCamera",
        lua.create_function(|_, _: MultiValue| -> mlua::Result<bool> {
            if !cfg!(feature = "vr") {
                return Ok(false);
            }
            with_state_mut(|s| {
                s.linked = true;
                s.left.is_connected = true;
                s.right.is_connected = true;
            });
            Ok(true)
        })?,
    )?;

    api.set(
        "UnlinkCamera",
        lua.create_function(|_, _: MultiValue| -> mlua::Result<()> {
            with_state_mut(|s| s.linked = false);
            Ok(())
        })?,
    )?;

    api.set(
        "HeadToWorldSpace",
        lua.create_function(|_, _: MultiValue| -> mlua::Result<CFrame> {
            let (body, head) = with_state(|s| (s.body, s.head));
            Ok(compose_world(body, head))
        })?,
    )?;

    api.set(
        "Recenter",
        lua.create_function(|_, _: MultiValue| -> mlua::Result<()> {
            let zero = Vector::new(0.0, 0.0, 0.0);
            with_state_mut(|s| {
                s.head = CFrame::new(zero, zero);
                s.body = CFrame::new(zero, zero);
            });
            Ok(())
        })?,
    )?;

    api.set(
        "GetControllers",
        lua.create_function(|lua, _: MultiValue| -> mlua::Result<Table> {
            let t = lua.create_table()?;
            // Array form per the new API contract: conns[1] = Left, conns[2] = Right.
            t.set(1, lua.create_userdata(VRController { side: Side::Left })?)?;
            t.set(2, lua.create_userdata(VRController { side: Side::Right })?)?;
            // Also expose named accessors so `controllers.Left` works.
            t.set("Left", lua.create_userdata(VRController { side: Side::Left })?)?;
            t.set("Right", lua.create_userdata(VRController { side: Side::Right })?)?;
            Ok(t)
        })?,
    )?;

    api.set(
        "GetEyePose",
        lua.create_function(|_, side: String| -> mlua::Result<CFrame> {
            const IPD: f32 = 0.063;
            let head = with_state(|s| s.head);
            let half = IPD * 0.5;
            let offset_x = match side.as_str() {
                "Left" => -half,
                "Right" => half,
                other => {
                    return Err(mlua::Error::RuntimeError(format!(
                        "GetEyePose: side must be \"Left\" or \"Right\" (got '{other}')"
                    )));
                }
            };
            Ok(CFrame::new(
                Vector::new(head.position.x + offset_x, head.position.y, head.position.z),
                head.rotation,
            ))
        })?,
    )?;

    Ok(api)
}

/// Backend hook (called by the indite-backed pump when the `vr` feature is on).
/// No-op when the feature is off so the public API surface always exists.
#[allow(dead_code)]
pub fn push_head_pose(lua: &Lua, head: CFrame) {
    let signal_table = with_state_mut(|s| {
        s.head = head;
        s.head_moved.clone()
    });
    if let Some(sig) = signal_table {
        if let Ok(ud) = lua.create_userdata(head) {
            let mut args = MultiValue::new();
            args.push_back(Value::UserData(ud));
            let _ = signal::fire(lua, &sig, args);
        }
    }
}

#[allow(dead_code)]
pub fn push_controller_pose(lua: &Lua, side_left: bool, cframe: CFrame) {
    let signal_table = with_state_mut(|s| {
        if side_left {
            s.left.cframe = cframe;
            s.left_moved.clone()
        } else {
            s.right.cframe = cframe;
            s.right_moved.clone()
        }
    });
    if let Some(sig) = signal_table {
        if let Ok(ud) = lua.create_userdata(cframe) {
            let mut args = MultiValue::new();
            args.push_back(Value::UserData(ud));
            let _ = signal::fire(lua, &sig, args);
        }
    }
}

#[allow(dead_code)]
pub fn push_controller_input(
    lua: &Lua,
    side_left: bool,
    name: &str,
    value: f32,
    began: bool,
) {
    let signal_table = with_state(|s| {
        if side_left {
            s.left_input.clone()
        } else {
            s.right_input.clone()
        }
    });
    if let Some(sig) = signal_table {
        let mut args = MultiValue::new();
        if let Ok(s) = lua.create_string(name) {
            args.push_back(Value::String(s));
        }
        args.push_back(Value::Number(value as f64));
        if let Ok(s) = lua.create_string(if began { "Begin" } else { "End" }) {
            args.push_back(Value::String(s));
        }
        let _ = signal::fire(lua, &sig, args);
    }
}

#[allow(dead_code)]
pub fn set_controller_axes(
    side_left: bool,
    trigger: f32,
    grip: f32,
    thumbstick: Vector,
) {
    with_state_mut(|s| {
        let pose = if side_left { &mut s.left } else { &mut s.right };
        pose.trigger = trigger;
        pose.grip = grip;
        pose.thumbstick = thumbstick;
    });
}
