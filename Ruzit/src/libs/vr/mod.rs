use std::cell::RefCell;
use std::sync::atomic::{AtomicBool, Ordering};

use mlua::{AnyUserData, Lua, MultiValue, Table, UserData, UserDataFields, UserDataMethods, Value};

use crate::libs::primitives::{CFrame, Vector};
use crate::libs::renderable::set_camera_cframe;
use crate::libs::signal;

thread_local! {
    static STATE: RefCell<VrState> = RefCell::new(VrState::default());
}

static VR_LINKED: AtomicBool = AtomicBool::new(false);

#[allow(dead_code)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum InputState {
    Begin,
    End,
}

impl InputState {
    pub fn as_str(self) -> &'static str {
        match self {
            InputState::Begin => "Begin",
            InputState::End => "End",
        }
    }
}

struct VrState {
    body: CFrame,
    head: CFrame,
    left: ControllerPose,
    right: ControllerPose,
    head_moved: Option<Table>,
    left_moved: Option<Table>,
    right_moved: Option<Table>,
    left_input: Option<Table>,
    right_input: Option<Table>,

    connected_signal: Option<Table>,

    runtime_name: String,
    refresh_rate: f32,
    is_headset_worn: bool,

    ipd: f32,

    play_area_size: [f32; 2],
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
        let id = CFrame::new(Vector::new(0.0, 0.0, 0.0), Vector::new(0.0, 0.0, 0.0));
        Self {
            body: id,
            head: id,
            left: ControllerPose::default(),
            right: ControllerPose::default(),
            head_moved: None,
            left_moved: None,
            right_moved: None,
            left_input: None,
            right_input: None,
            connected_signal: None,
            runtime_name: String::new(),
            refresh_rate: 90.0,
            is_headset_worn: false,
            ipd: 0.063,
            play_area_size: [0.0, 0.0],
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

pub fn is_vr_linked() -> bool {
    VR_LINKED.load(Ordering::Relaxed)
}

fn detected_runtime_name() -> &'static str {
    if std::env::var_os("OPENVR_RUNTIME").is_some() || std::env::var_os("VR_OVERRIDE").is_some() {
        return "SteamVR";
    }
    if std::env::var_os("OCULUS_RUNTIME").is_some() {
        return "Oculus";
    }
    if std::env::var_os("XR_RUNTIME_JSON").is_some() {
        return "OpenXR";
    }
    #[cfg(windows)]
    {
        if windows_registry_has_openxr_runtime() {
            return "OpenXR";
        }
    }
    "Unknown"
}

pub fn pump(_lua: &Lua) {
    if !is_vr_linked() {
        return;
    }
    let composed = with_state(|st| compose_camera(st.body, st.head));
    set_camera_cframe(composed);
}

fn compose_camera(body: CFrame, head: CFrame) -> CFrame {
    CFrame::new(
        Vector::new(
            body.position.x + head.position.x,
            body.position.y + head.position.y,
            body.position.z + head.position.z,
        ),
        Vector::new(
            body.rotation.x + head.rotation.x,
            body.rotation.y + head.rotation.y,
            body.rotation.z + head.rotation.z,
        ),
    )
}

#[derive(Clone)]
pub struct VRCameraHandle;

impl UserData for VRCameraHandle {
    fn add_fields<F: UserDataFields<Self>>(f: &mut F) {
        f.add_field_method_get("BodyCframe", |_, _| Ok(with_state(|s| s.body)));
        f.add_field_method_set("BodyCframe", |_, _, value: AnyUserData| {
            let cf = *value
                .borrow::<CFrame>()
                .map_err(|_| mlua::Error::RuntimeError("BodyCframe expects a CFrame".into()))?;
            with_state_mut(|s| s.body = cf);
            Ok(())
        });

        f.add_field_method_get("HeadCframe", |_, _| Ok(with_state(|s| s.head)));
        f.add_field_method_get("HeadMoved", |lua, _| {
            if let Some(sig) = with_state(|s| s.head_moved.clone()) {
                return Ok(sig);
            }
            let sig = signal::new_instance(lua)?;
            with_state_mut(|s| s.head_moved = Some(sig.clone()));
            Ok(sig)
        });
        f.add_field_method_get("IsLinked", |_, _| Ok(is_vr_linked()));

        f.add_field_method_get("RuntimeName", |_, _| {
            Ok(with_state(|s| s.runtime_name.clone()))
        });
        f.add_field_method_get("RefreshRate", |_, _| Ok(with_state(|s| s.refresh_rate)));
        f.add_field_method_get("IsHeadsetWorn", |_, _| {
            Ok(with_state(|s| s.is_headset_worn))
        });
        f.add_field_method_get("IPD", |_, _| Ok(with_state(|s| s.ipd)));

        f.add_field_method_get("Connected", |lua, _| {
            if let Some(sig) = with_state(|s| s.connected_signal.clone()) {
                return Ok(sig);
            }
            let sig = signal::new_instance(lua)?;
            with_state_mut(|s| s.connected_signal = Some(sig.clone()));
            Ok(sig)
        });
    }

    fn add_methods<M: UserDataMethods<Self>>(m: &mut M) {
        m.add_method("Unlink", |_, _, _: ()| -> mlua::Result<()> {
            VR_LINKED.store(false, Ordering::Relaxed);
            Ok(())
        });

        m.add_method("GetControllers", |lua, _, _: ()| -> mlua::Result<Table> {
            let t = lua.create_table()?;
            t.set(
                "Left",
                lua.create_userdata(ControllerHandle { side: Side::Left })?,
            )?;
            t.set(
                "Right",
                lua.create_userdata(ControllerHandle { side: Side::Right })?,
            )?;
            Ok(t)
        });

        m.add_method("Recenter", |_, _, _: ()| -> mlua::Result<()> {
            let zero = Vector::new(0.0, 0.0, 0.0);
            with_state_mut(|s| {
                s.head = CFrame::new(zero, zero);
                s.body = CFrame::new(zero, zero);
            });
            Ok(())
        });

        m.add_method("GetEyePose", |_, _, side: String| -> mlua::Result<CFrame> {
            let (head, ipd) = with_state(|s| (s.head, s.ipd));
            let half = ipd * 0.5;
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
        });

        m.add_method("GetPlayArea", |lua, _, _: ()| -> mlua::Result<Table> {
            let size = with_state(|s| s.play_area_size);
            let t = lua.create_table()?;
            t.set("Width", size[0])?;
            t.set("Depth", size[1])?;
            Ok(t)
        });

        m.add_method(
            "Fade",
            |_, _, (_color, _duration): (AnyUserData, f32)| -> mlua::Result<()> { Ok(()) },
        );
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Side {
    Left,
    Right,
}

#[derive(Clone)]
pub struct ControllerHandle {
    side: Side,
}

impl UserData for ControllerHandle {
    fn add_fields<F: UserDataFields<Self>>(f: &mut F) {
        f.add_field_method_get("Side", |_, this| {
            Ok(match this.side {
                Side::Left => "Left",
                Side::Right => "Right",
            })
        });

        f.add_field_method_get("CFrame", |_, this| {
            Ok(with_state(|s| match this.side {
                Side::Left => s.left.cframe,
                Side::Right => s.right.cframe,
            }))
        });
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

        f.add_field_method_get("Trigger", |_, this| {
            Ok(with_state(|s| match this.side {
                Side::Left => s.left.trigger,
                Side::Right => s.right.trigger,
            }))
        });
        f.add_field_method_get("Grip", |_, this| {
            Ok(with_state(|s| match this.side {
                Side::Left => s.left.grip,
                Side::Right => s.right.grip,
            }))
        });
        f.add_field_method_get("Thumbstick", |_, this| {
            Ok(with_state(|s| match this.side {
                Side::Left => s.left.thumbstick,
                Side::Right => s.right.thumbstick,
            }))
        });

        f.add_field_method_get("Velocity", |_, this| {
            Ok(with_state(|s| match this.side {
                Side::Left => s.left.velocity,
                Side::Right => s.right.velocity,
            }))
        });
        f.add_field_method_get("AngularVelocity", |_, this| {
            Ok(with_state(|s| match this.side {
                Side::Left => s.left.angular_velocity,
                Side::Right => s.right.angular_velocity,
            }))
        });
        f.add_field_method_get("IsConnected", |_, this| {
            Ok(with_state(|s| match this.side {
                Side::Left => s.left.is_connected,
                Side::Right => s.right.is_connected,
            }))
        });

        f.add_field_method_get("BatteryLevel", |_, this| {
            Ok(with_state(|s| match this.side {
                Side::Left => s.left.battery,
                Side::Right => s.right.battery,
            }))
        });
    }

    fn add_methods<M: UserDataMethods<Self>>(m: &mut M) {
        m.add_method(
            "Vibrate",
            |_,
             _this,
             (_duration, _frequency, _amplitude): (f32, Option<f32>, Option<f32>)|
             -> mlua::Result<bool> { Ok(false) },
        );
        m.add_method("StopVibration", |_, _this, _: ()| -> mlua::Result<()> {
            Ok(())
        });
    }
}

#[allow(dead_code)]
pub fn push_controller_pose(side_left: bool, cframe: CFrame, lua: &Lua) {
    let signal_table = with_state_mut(|st| {
        if side_left {
            st.left.cframe = cframe;
            st.left_moved.clone()
        } else {
            st.right.cframe = cframe;
            st.right_moved.clone()
        }
    });
    if let Some(sig) = signal_table {
        let mut args = MultiValue::new();
        args.push_back(Value::UserData(match lua.create_userdata(cframe) {
            Ok(u) => u,
            Err(_) => return,
        }));
        let _ = signal::fire(lua, &sig, args);
    }
}

#[allow(dead_code)]
pub fn push_controller_input(
    side_left: bool,
    name: &str,
    value: f32,
    state_kind: InputState,
    lua: &Lua,
) {
    let signal_table = with_state(|st| {
        if side_left {
            st.left_input.clone()
        } else {
            st.right_input.clone()
        }
    });
    if let Some(sig) = signal_table {
        let mut args = MultiValue::new();
        let s = match lua.create_string(name) {
            Ok(s) => s,
            Err(_) => return,
        };
        args.push_back(Value::String(s));
        args.push_back(Value::Number(value as f64));
        let st_str = match lua.create_string(state_kind.as_str()) {
            Ok(s) => s,
            Err(_) => return,
        };
        args.push_back(Value::String(st_str));
        let _ = signal::fire(lua, &sig, args);
    }
}

pub fn create(lua: &Lua) -> mlua::Result<Table> {
    let api = lua.create_table()?;
    api.set(
        "IsVrPresent",
        lua.create_function(|_, _: ()| -> mlua::Result<bool> { Ok(is_vr_present()) })?,
    )?;
    api.set(
        "LinkVRView",
        lua.create_function(|_, _: ()| -> mlua::Result<VRCameraHandle> {
            let name = detected_runtime_name();
            with_state_mut(|s| {
                s.runtime_name = name.to_string();

                s.left.is_connected = true;
                s.right.is_connected = true;
            });
            VR_LINKED.store(true, Ordering::Relaxed);
            Ok(VRCameraHandle)
        })?,
    )?;
    Ok(api)
}
