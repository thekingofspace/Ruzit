use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use mlua::{AnyUserData, Lua, MultiValue, Table, UserData, UserDataFields, UserDataMethods, Value};

use crate::libs::gui::{GuiPrimitive, PrimitiveState, Shape};
use crate::libs::primitives::Dim;
use crate::libs::signal;

static NEXT_GUI_PLANE_ID: AtomicU64 = AtomicU64::new(1);
static NEXT_GUI_OBJ_ID: AtomicU64 = AtomicU64::new(1);
static NEXT_GUI_CTRL_ID: AtomicU64 = AtomicU64::new(1);

thread_local! {
    pub(crate) static GUI_PLANES: RefCell<Vec<Arc<Mutex<GuiPlaneState>>>> =
        const { RefCell::new(Vec::new()) };
}

pub struct GuiPlaneState {
    pub id: u64,
    pub alive: bool,
    pub enabled: bool,
    pub gravity: Dim,
    pub linear_damping: f32,
    pub angular_damping: f32,
    pub drag: f32,
    pub constant_force: Dim,
    pub rest_threshold: f32,
    pub solver_iterations: u32,
    pub never_sleep: bool,
    pub bounds: Option<(Dim, Dim)>,
    pub objects: HashMap<u64, GuiObjectState>,
    pub controllers: HashMap<u64, GuiControllerState>,
}

pub struct GuiObjectState {
    pub id: u64,
    pub alive: bool,
    pub prim: Arc<Mutex<PrimitiveState>>,
    pub prim_alive: bool,
    pub position: Dim,
    pub velocity: Dim,
    pub rotation: f32,
    pub angular_velocity: f32,
    pub impulse: Dim,
    pub constant_force: Dim,
    pub mass: f32,
    pub bounciness: f32,
    pub friction: f32,
    pub anchored: bool,
    pub can_collide: bool,
    pub gravity_scale: f32,
    pub locked_x: bool,
    pub locked_y: bool,
    pub locked_rotation: bool,
    pub cached_size: Dim,
    pub cached_z_index: i32,
    pub cached_shape: Shape,
    pub sleeping: bool,
    pub sleep_timer: f32,
    pub controller_owned: bool,
}

pub struct GuiControllerState {
    pub id: u64,
    pub object_id: u64,
    pub move_dir: Dim,
    pub move_speed: f32,
    pub accel: f32,
    pub controlled: bool,
    pub jump_strength: f32,
    pub jump_request: bool,
    pub on_ground: bool,
    pub last_on_ground: bool,
    pub waypoints: Vec<Dim>,
    pub waypoint_index: usize,
    pub waypoint_radius: f32,
    pub waypoint_reached: Option<i64>,
    pub waypoint_reached_signal: Table,
    pub path_finished: bool,
    pub path_finished_signal: Table,
    pub moved_signal: Table,
    pub moved_dx: f32,
    pub moved_dy: f32,
    pub ground_signal: Table,
    pub ground_changed: bool,
    pub ground_value: bool,
}

pub struct GuiPlaneHandle {
    pub state: Arc<Mutex<GuiPlaneState>>,
}

pub struct GuiObjectHandle {
    pub plane: Arc<Mutex<GuiPlaneState>>,
    pub id: u64,
}

pub struct GuiControllerHandle {
    pub plane: Arc<Mutex<GuiPlaneState>>,
    pub id: u64,
}

pub fn create_new_gui(lua: &Lua) -> mlua::Result<mlua::Function> {
    lua.create_function(|_, opts: Option<Table>| -> mlua::Result<GuiPlaneHandle> {
        let mut gravity = Dim::new(0.0, 980.0);
        let mut linear_damping: f32 = 0.05;
        let mut angular_damping: f32 = 0.10;
        let mut drag: f32 = 0.0;
        let mut constant_force = Dim::new(0.0, 0.0);
        let mut rest_threshold: f32 = 0.25;
        let mut solver_iterations: u32 = 4;
        let mut never_sleep = false;
        let mut bounds: Option<(Dim, Dim)> = None;
        if let Some(opts) = opts {
            if let Ok(v) = opts.get::<Dim>("Gravity") {
                gravity = v;
            }
            if let Ok(v) = opts.get::<f32>("LinearDamping") {
                linear_damping = v.max(0.0);
            }
            if let Ok(v) = opts.get::<f32>("AngularDamping") {
                angular_damping = v.max(0.0);
            }
            if let Ok(v) = opts.get::<f32>("Drag") {
                drag = v.max(0.0);
            }
            if let Ok(v) = opts.get::<Dim>("ConstantForce") {
                constant_force = v;
            }
            if let Ok(v) = opts.get::<f32>("RestThreshold") {
                rest_threshold = v.max(0.0);
            }
            if let Ok(v) = opts.get::<i64>("SolverIterations") {
                solver_iterations = v.clamp(1, 32) as u32;
            }
            if let Ok(v) = opts.get::<bool>("NeverSleep") {
                never_sleep = v;
            }
            if let (Ok(min), Ok(max)) = (opts.get::<Dim>("BoundsMin"), opts.get::<Dim>("BoundsMax")) {
                bounds = Some((min, max));
            }
        }
        let id = NEXT_GUI_PLANE_ID.fetch_add(1, Ordering::Relaxed);
        let state = Arc::new(Mutex::new(GuiPlaneState {
            id,
            alive: true,
            enabled: true,
            gravity,
            linear_damping,
            angular_damping,
            drag,
            constant_force,
            rest_threshold,
            solver_iterations,
            never_sleep,
            bounds,
            objects: HashMap::new(),
            controllers: HashMap::new(),
        }));
        GUI_PLANES.with(|c| c.borrow_mut().push(state.clone()));
        Ok(GuiPlaneHandle { state })
    })
}

fn extract_prim(ud: AnyUserData) -> mlua::Result<Arc<Mutex<PrimitiveState>>> {
    let prim = ud.borrow::<GuiPrimitive>().map_err(|_| {
        mlua::Error::RuntimeError(
            "PhysicsGui: expected a GUI primitive (Square, Circle, Triangle, Image, or Text)".into(),
        )
    })?;
    Ok(prim.state_arc())
}

fn new_object_state(prim: Arc<Mutex<PrimitiveState>>) -> GuiObjectState {
    let id = NEXT_GUI_OBJ_ID.fetch_add(1, Ordering::Relaxed);
    let (position, size, rotation, z_index, shape) = {
        let p = prim.lock().unwrap();
        (p.position, p.size, p.rotation, p.z_index, p.shape)
    };
    GuiObjectState {
        id,
        alive: true,
        prim,
        prim_alive: true,
        position,
        velocity: Dim::new(0.0, 0.0),
        rotation,
        angular_velocity: 0.0,
        impulse: Dim::new(0.0, 0.0),
        constant_force: Dim::new(0.0, 0.0),
        mass: 1.0,
        bounciness: 0.0,
        friction: 0.5,
        anchored: false,
        can_collide: true,
        gravity_scale: 1.0,
        locked_x: false,
        locked_y: false,
        locked_rotation: false,
        cached_size: size,
        cached_z_index: z_index,
        cached_shape: shape,
        sleeping: false,
        sleep_timer: 0.0,
        controller_owned: false,
    }
}

impl UserData for GuiPlaneHandle {
    fn add_fields<F: UserDataFields<Self>>(f: &mut F) {
        f.add_field_method_get("Alive", |_, this| Ok(this.state.lock().unwrap().alive));
        f.add_field_method_get("ObjectCount", |_, this| {
            Ok(this.state.lock().unwrap().objects.len() as i64)
        });
        f.add_field_method_get("Gravity", |_, this| Ok(this.state.lock().unwrap().gravity));
        f.add_field_method_set("Gravity", |_, this, v: Dim| {
            this.state.lock().unwrap().gravity = v;
            Ok(())
        });
        f.add_field_method_get("Enabled", |_, this| {
            Ok(this.state.lock().unwrap().enabled)
        });
        f.add_field_method_set("Enabled", |_, this, v: bool| {
            this.state.lock().unwrap().enabled = v;
            Ok(())
        });
        f.add_field_method_get("LinearDamping", |_, this| {
            Ok(this.state.lock().unwrap().linear_damping)
        });
        f.add_field_method_set("LinearDamping", |_, this, v: f32| {
            this.state.lock().unwrap().linear_damping = v.max(0.0);
            Ok(())
        });
        f.add_field_method_get("AngularDamping", |_, this| {
            Ok(this.state.lock().unwrap().angular_damping)
        });
        f.add_field_method_set("AngularDamping", |_, this, v: f32| {
            this.state.lock().unwrap().angular_damping = v.max(0.0);
            Ok(())
        });
        f.add_field_method_get("Drag", |_, this| Ok(this.state.lock().unwrap().drag));
        f.add_field_method_set("Drag", |_, this, v: f32| {
            this.state.lock().unwrap().drag = v.max(0.0);
            Ok(())
        });
        f.add_field_method_get("ConstantForce", |_, this| {
            Ok(this.state.lock().unwrap().constant_force)
        });
        f.add_field_method_set("ConstantForce", |_, this, v: Dim| {
            this.state.lock().unwrap().constant_force = v;
            Ok(())
        });
        f.add_field_method_get("RestThreshold", |_, this| {
            Ok(this.state.lock().unwrap().rest_threshold)
        });
        f.add_field_method_set("RestThreshold", |_, this, v: f32| {
            this.state.lock().unwrap().rest_threshold = v.max(0.0);
            Ok(())
        });
        f.add_field_method_get("SolverIterations", |_, this| {
            Ok(this.state.lock().unwrap().solver_iterations as i64)
        });
        f.add_field_method_set("SolverIterations", |_, this, v: i64| {
            this.state.lock().unwrap().solver_iterations = v.clamp(1, 32) as u32;
            Ok(())
        });
        f.add_field_method_get("NeverSleep", |_, this| {
            Ok(this.state.lock().unwrap().never_sleep)
        });
        f.add_field_method_set("NeverSleep", |_, this, v: bool| {
            this.state.lock().unwrap().never_sleep = v;
            Ok(())
        });
    }

    fn add_methods<M: UserDataMethods<Self>>(m: &mut M) {
        m.add_method(
            "Add",
            |_, this, prim_ud: AnyUserData| -> mlua::Result<GuiObjectHandle> {
                let prim = extract_prim(prim_ud)?;
                let mut plane = this.state.lock().unwrap();
                if !plane.alive {
                    return Err(mlua::Error::RuntimeError(
                        "GuiPhysicsPlane:Add: plane has been destroyed".into(),
                    ));
                }
                let state = new_object_state(prim);
                let id = state.id;
                plane.objects.insert(id, state);
                Ok(GuiObjectHandle {
                    plane: this.state.clone(),
                    id,
                })
            },
        );

        m.add_method(
            "NewController",
            |lua, this, args: MultiValue| -> mlua::Result<GuiControllerHandle> {
                make_controller(lua, this.state.clone(), args, true)
            },
        );

        m.add_method(
            "NewUncontrolled",
            |lua, this, args: MultiValue| -> mlua::Result<GuiControllerHandle> {
                make_controller(lua, this.state.clone(), args, false)
            },
        );

        m.add_method("Step", |_, this, dt: f64| {
            step_plane(&this.state, (dt as f32).min(1.0 / 30.0));
            Ok(())
        });

        m.add_method("Destroy", |_, this, _: ()| {
            let mut plane = this.state.lock().unwrap();
            plane.alive = false;
            plane.objects.clear();
            plane.controllers.clear();
            Ok(())
        });

        m.add_method(
            "Raycast",
            |lua, this, (from, dir, max_dist): (Dim, Dim, Option<f64>)| -> mlua::Result<Value> {
                raycast(lua, &this.state, from, dir, max_dist.unwrap_or(1024.0) as f32)
            },
        );
    }
}

impl UserData for GuiObjectHandle {
    fn add_fields<F: UserDataFields<Self>>(f: &mut F) {
        f.add_field_method_get("Alive", |_, this| {
            Ok(this
                .plane
                .lock()
                .unwrap()
                .objects
                .get(&this.id)
                .map(|o| o.alive)
                .unwrap_or(false))
        });
        f.add_field_method_get("Position", |_, this| {
            with_object(this, |o| Ok(o.position))
        });
        f.add_field_method_set("Position", |_, this, v: Dim| {
            with_object_mut(this, |o| {
                o.position = v;
                o.sleeping = false;
                o.sleep_timer = 0.0;
                Ok(())
            })
        });
        f.add_field_method_get("Velocity", |_, this| {
            with_object(this, |o| Ok(o.velocity))
        });
        f.add_field_method_set("Velocity", |_, this, v: Dim| {
            with_object_mut(this, |o| {
                o.velocity = v;
                o.sleeping = false;
                o.sleep_timer = 0.0;
                Ok(())
            })
        });
        f.add_field_method_get("Rotation", |_, this| {
            with_object(this, |o| Ok(o.rotation))
        });
        f.add_field_method_set("Rotation", |_, this, v: f32| {
            with_object_mut(this, |o| {
                o.rotation = v;
                Ok(())
            })
        });
        f.add_field_method_get("AngularVelocity", |_, this| {
            with_object(this, |o| Ok(o.angular_velocity))
        });
        f.add_field_method_set("AngularVelocity", |_, this, v: f32| {
            with_object_mut(this, |o| {
                o.angular_velocity = v;
                Ok(())
            })
        });
        f.add_field_method_get("ImpulseDirection", |_, this| {
            with_object(this, |o| Ok(o.impulse))
        });
        f.add_field_method_set("ImpulseDirection", |_, this, v: Dim| {
            with_object_mut(this, |o| {
                o.impulse = v;
                o.sleeping = false;
                o.sleep_timer = 0.0;
                Ok(())
            })
        });
        f.add_field_method_get("ConstantForce", |_, this| {
            with_object(this, |o| Ok(o.constant_force))
        });
        f.add_field_method_set("ConstantForce", |_, this, v: Dim| {
            with_object_mut(this, |o| {
                o.constant_force = v;
                Ok(())
            })
        });
        f.add_field_method_get("Mass", |_, this| with_object(this, |o| Ok(o.mass)));
        f.add_field_method_set("Mass", |_, this, v: f32| {
            with_object_mut(this, |o| {
                o.mass = v.max(0.0001);
                Ok(())
            })
        });
        f.add_field_method_get("Bounciness", |_, this| {
            with_object(this, |o| Ok(o.bounciness))
        });
        f.add_field_method_set("Bounciness", |_, this, v: f32| {
            with_object_mut(this, |o| {
                o.bounciness = v.clamp(0.0, 1.0);
                Ok(())
            })
        });
        f.add_field_method_get("Friction", |_, this| {
            with_object(this, |o| Ok(o.friction))
        });
        f.add_field_method_set("Friction", |_, this, v: f32| {
            with_object_mut(this, |o| {
                o.friction = v.clamp(0.0, 4.0);
                Ok(())
            })
        });
        f.add_field_method_get("Anchored", |_, this| {
            with_object(this, |o| Ok(o.anchored))
        });
        f.add_field_method_set("Anchored", |_, this, v: bool| {
            with_object_mut(this, |o| {
                o.anchored = v;
                if v {
                    o.velocity = Dim::new(0.0, 0.0);
                    o.angular_velocity = 0.0;
                }
                Ok(())
            })
        });
        f.add_field_method_get("CanCollide", |_, this| {
            with_object(this, |o| Ok(o.can_collide))
        });
        f.add_field_method_set("CanCollide", |_, this, v: bool| {
            with_object_mut(this, |o| {
                o.can_collide = v;
                Ok(())
            })
        });
        f.add_field_method_get("GravityScale", |_, this| {
            with_object(this, |o| Ok(o.gravity_scale))
        });
        f.add_field_method_set("GravityScale", |_, this, v: f32| {
            with_object_mut(this, |o| {
                o.gravity_scale = v;
                Ok(())
            })
        });
        f.add_field_method_get("LockedX", |_, this| {
            with_object(this, |o| Ok(o.locked_x))
        });
        f.add_field_method_set("LockedX", |_, this, v: bool| {
            with_object_mut(this, |o| {
                o.locked_x = v;
                Ok(())
            })
        });
        f.add_field_method_get("LockedY", |_, this| {
            with_object(this, |o| Ok(o.locked_y))
        });
        f.add_field_method_set("LockedY", |_, this, v: bool| {
            with_object_mut(this, |o| {
                o.locked_y = v;
                Ok(())
            })
        });
        f.add_field_method_get("LockedRotation", |_, this| {
            with_object(this, |o| Ok(o.locked_rotation))
        });
        f.add_field_method_set("LockedRotation", |_, this, v: bool| {
            with_object_mut(this, |o| {
                o.locked_rotation = v;
                Ok(())
            })
        });
        f.add_field_method_get("ZIndex", |_, this| {
            with_object(this, |o| Ok(o.cached_z_index as i64))
        });
        f.add_field_method_get("Size", |_, this| {
            with_object(this, |o| Ok(o.cached_size))
        });
        f.add_field_method_get("Shape", |_, this| {
            with_object(this, |o| {
                Ok(match o.cached_shape {
                    Shape::Circle => "Circle",
                    Shape::Square => "Square",
                    Shape::Triangle => "Triangle",
                    Shape::Image => "Image",
                    Shape::Text => "Text",
                    Shape::Clippable => "Clippable",
                })
            })
        });
        f.add_field_method_get("Sleeping", |_, this| {
            with_object(this, |o| Ok(o.sleeping))
        });
    }

    fn add_methods<M: UserDataMethods<Self>>(m: &mut M) {
        m.add_method(
            "ApplyImpulse",
            |_, this, v: Dim| -> mlua::Result<()> {
                with_object_mut(this, |o| {
                    let inv = if o.anchored || o.mass <= 0.0001 {
                        0.0
                    } else {
                        1.0 / o.mass
                    };
                    o.velocity = Dim::new(o.velocity.x + v.x * inv, o.velocity.y + v.y * inv);
                    o.sleeping = false;
                    o.sleep_timer = 0.0;
                    Ok(())
                })
            },
        );
        m.add_method(
            "ApplyForce",
            |_, this, v: Dim| -> mlua::Result<()> {
                with_object_mut(this, |o| {
                    o.constant_force = v;
                    Ok(())
                })
            },
        );
        m.add_method("Wake", |_, this, _: ()| {
            with_object_mut(this, |o| {
                o.sleeping = false;
                o.sleep_timer = 0.0;
                Ok(())
            })
        });
        m.add_method("Destroy", |_, this, _: ()| -> mlua::Result<()> {
            let mut plane = this.plane.lock().unwrap();
            plane.objects.remove(&this.id);
            Ok(())
        });
    }
}

fn with_object<F, R>(handle: &GuiObjectHandle, f: F) -> mlua::Result<R>
where
    F: FnOnce(&GuiObjectState) -> mlua::Result<R>,
{
    let plane = handle.plane.lock().unwrap();
    let obj = plane.objects.get(&handle.id).ok_or_else(|| {
        mlua::Error::RuntimeError("PhysicsGuiObject: object no longer attached".into())
    })?;
    f(obj)
}

fn with_object_mut<F, R>(handle: &GuiObjectHandle, f: F) -> mlua::Result<R>
where
    F: FnOnce(&mut GuiObjectState) -> mlua::Result<R>,
{
    let mut plane = handle.plane.lock().unwrap();
    let obj = plane.objects.get_mut(&handle.id).ok_or_else(|| {
        mlua::Error::RuntimeError("PhysicsGuiObject: object no longer attached".into())
    })?;
    f(obj)
}

fn make_controller(
    lua: &Lua,
    plane: Arc<Mutex<GuiPlaneState>>,
    args: MultiValue,
    controlled: bool,
) -> mlua::Result<GuiControllerHandle> {
    let mut iter = args.into_iter();
    let prim_val = iter.next().ok_or_else(|| {
        mlua::Error::RuntimeError(
            "GuiPhysicsPlane:NewController: expected a GUI primitive as the first argument".into(),
        )
    })?;
    let prim_ud = match prim_val {
        Value::UserData(ud) => ud,
        _ => {
            return Err(mlua::Error::RuntimeError(
                "GuiPhysicsPlane:NewController: first argument must be a GUI primitive".into(),
            ));
        }
    };
    let prim = extract_prim(prim_ud)?;
    let opts = match iter.next() {
        Some(Value::Table(t)) => Some(t),
        _ => None,
    };
    let (move_speed, accel, jump_strength, waypoint_radius) = read_controller_opts(opts.as_ref());

    let mut object = new_object_state(prim);
    object.controller_owned = true;
    object.locked_rotation = true;
    let object_id = object.id;

    let controller_id = NEXT_GUI_CTRL_ID.fetch_add(1, Ordering::Relaxed);
    let state = GuiControllerState {
        id: controller_id,
        object_id,
        move_dir: Dim::new(0.0, 0.0),
        move_speed,
        accel,
        controlled,
        jump_strength,
        jump_request: false,
        on_ground: false,
        last_on_ground: false,
        waypoints: Vec::new(),
        waypoint_index: 0,
        waypoint_radius,
        waypoint_reached: None,
        waypoint_reached_signal: signal::new_instance(lua)?,
        path_finished: false,
        path_finished_signal: signal::new_instance(lua)?,
        moved_signal: signal::new_instance(lua)?,
        moved_dx: 0.0,
        moved_dy: 0.0,
        ground_signal: signal::new_instance(lua)?,
        ground_changed: false,
        ground_value: false,
    };

    let mut p = plane.lock().unwrap();
    p.objects.insert(object_id, object);
    p.controllers.insert(controller_id, state);
    drop(p);
    Ok(GuiControllerHandle {
        plane,
        id: controller_id,
    })
}

fn read_controller_opts(opts: Option<&Table>) -> (f32, f32, f32, f32) {
    let mut move_speed = 240.0;
    let mut accel = 1800.0;
    let mut jump_strength = 700.0;
    let mut waypoint_radius = 8.0;
    if let Some(t) = opts {
        if let Ok(v) = t.get::<f32>("MoveSpeed") {
            move_speed = v.max(0.0);
        }
        if let Ok(v) = t.get::<f32>("Acceleration") {
            accel = v.max(0.0);
        }
        if let Ok(v) = t.get::<f32>("JumpStrength") {
            jump_strength = v.max(0.0);
        }
        if let Ok(v) = t.get::<f32>("WaypointRadius") {
            waypoint_radius = v.max(0.1);
        }
    }
    (move_speed, accel, jump_strength, waypoint_radius)
}

impl UserData for GuiControllerHandle {
    fn add_fields<F: UserDataFields<Self>>(f: &mut F) {
        f.add_field_method_get("Alive", |_, this| {
            Ok(this
                .plane
                .lock()
                .unwrap()
                .controllers
                .contains_key(&this.id))
        });
        f.add_field_method_get("Object", |_, this| {
            let plane = this.plane.lock().unwrap();
            let obj_id = plane
                .controllers
                .get(&this.id)
                .map(|c| c.object_id)
                .ok_or_else(|| {
                    mlua::Error::RuntimeError("GUIController: controller no longer attached".into())
                })?;
            Ok(GuiObjectHandle {
                plane: this.plane.clone(),
                id: obj_id,
            })
        });
        f.add_field_method_get("MoveDirection", |_, this| {
            let plane = this.plane.lock().unwrap();
            let c = plane.controllers.get(&this.id).ok_or_else(|| {
                mlua::Error::RuntimeError("GUIController: controller no longer attached".into())
            })?;
            Ok(c.move_dir)
        });
        f.add_field_method_get("MoveSpeed", |_, this| {
            let plane = this.plane.lock().unwrap();
            Ok(plane
                .controllers
                .get(&this.id)
                .map(|c| c.move_speed)
                .unwrap_or(0.0))
        });
        f.add_field_method_set("MoveSpeed", |_, this, v: f32| {
            let mut plane = this.plane.lock().unwrap();
            if let Some(c) = plane.controllers.get_mut(&this.id) {
                c.move_speed = v.max(0.0);
            }
            Ok(())
        });
        f.add_field_method_get("Acceleration", |_, this| {
            Ok(this
                .plane
                .lock()
                .unwrap()
                .controllers
                .get(&this.id)
                .map(|c| c.accel)
                .unwrap_or(0.0))
        });
        f.add_field_method_set("Acceleration", |_, this, v: f32| {
            let mut plane = this.plane.lock().unwrap();
            if let Some(c) = plane.controllers.get_mut(&this.id) {
                c.accel = v.max(0.0);
            }
            Ok(())
        });
        f.add_field_method_get("JumpStrength", |_, this| {
            Ok(this
                .plane
                .lock()
                .unwrap()
                .controllers
                .get(&this.id)
                .map(|c| c.jump_strength)
                .unwrap_or(0.0))
        });
        f.add_field_method_set("JumpStrength", |_, this, v: f32| {
            let mut plane = this.plane.lock().unwrap();
            if let Some(c) = plane.controllers.get_mut(&this.id) {
                c.jump_strength = v.max(0.0);
            }
            Ok(())
        });
        f.add_field_method_get("OnGround", |_, this| {
            Ok(this
                .plane
                .lock()
                .unwrap()
                .controllers
                .get(&this.id)
                .map(|c| c.on_ground)
                .unwrap_or(false))
        });
        f.add_field_method_get("WaypointReached", |_, this| {
            Ok(this
                .plane
                .lock()
                .unwrap()
                .controllers
                .get(&this.id)
                .map(|c| c.waypoint_reached_signal.clone()))
        });
        f.add_field_method_get("PathFinished", |_, this| {
            Ok(this
                .plane
                .lock()
                .unwrap()
                .controllers
                .get(&this.id)
                .map(|c| c.path_finished_signal.clone()))
        });
        f.add_field_method_get("Moved", |_, this| {
            Ok(this
                .plane
                .lock()
                .unwrap()
                .controllers
                .get(&this.id)
                .map(|c| c.moved_signal.clone()))
        });
        f.add_field_method_get("GroundChanged", |_, this| {
            Ok(this
                .plane
                .lock()
                .unwrap()
                .controllers
                .get(&this.id)
                .map(|c| c.ground_signal.clone()))
        });
    }

    fn add_methods<M: UserDataMethods<Self>>(m: &mut M) {
        m.add_method("Move", |_, this, dir: Dim| -> mlua::Result<()> {
            let mut plane = this.plane.lock().unwrap();
            if let Some(c) = plane.controllers.get_mut(&this.id) {
                c.move_dir = dir;
            }
            Ok(())
        });
        m.add_method("Jump", |_, this, _: ()| -> mlua::Result<()> {
            let mut plane = this.plane.lock().unwrap();
            if let Some(c) = plane.controllers.get_mut(&this.id) {
                c.jump_request = true;
            }
            Ok(())
        });
        m.add_method(
            "SetWaypoints",
            |_, this, pts: Vec<Dim>| -> mlua::Result<()> {
                let mut plane = this.plane.lock().unwrap();
                if let Some(c) = plane.controllers.get_mut(&this.id) {
                    c.waypoints = pts;
                    c.waypoint_index = 0;
                    c.path_finished = false;
                }
                Ok(())
            },
        );
        m.add_method("ClearWaypoints", |_, this, _: ()| -> mlua::Result<()> {
            let mut plane = this.plane.lock().unwrap();
            if let Some(c) = plane.controllers.get_mut(&this.id) {
                c.waypoints.clear();
                c.waypoint_index = 0;
            }
            Ok(())
        });
        m.add_method(
            "MoveTo",
            |_, this, target: Dim| -> mlua::Result<()> {
                let mut plane = this.plane.lock().unwrap();
                if let Some(c) = plane.controllers.get_mut(&this.id) {
                    c.waypoints = vec![target];
                    c.waypoint_index = 0;
                    c.path_finished = false;
                }
                Ok(())
            },
        );
        m.add_method("Destroy", |_, this, _: ()| -> mlua::Result<()> {
            let mut plane = this.plane.lock().unwrap();
            let obj_id = plane.controllers.get(&this.id).map(|c| c.object_id);
            plane.controllers.remove(&this.id);
            if let Some(oid) = obj_id {
                plane.objects.remove(&oid);
            }
            Ok(())
        });
    }
}

pub fn tick(lua: &Lua, dt: f64) {
    let dt = (dt as f32).min(1.0 / 30.0);
    let snapshot: Vec<Arc<Mutex<GuiPlaneState>>> = GUI_PLANES.with(|c| {
        c.borrow_mut().retain(|p| p.lock().unwrap().alive);
        c.borrow().clone()
    });
    for plane in &snapshot {
        step_plane(plane, dt);
        tick_controllers(plane, dt);
        fire_signals(lua, plane);
        write_back(lua, plane);
    }
}

fn step_plane(plane_arc: &Arc<Mutex<GuiPlaneState>>, dt: f32) {
    let mut plane = plane_arc.lock().unwrap();
    if !plane.alive || !plane.enabled || plane.objects.is_empty() {
        return;
    }

    let dead: Vec<u64> = plane
        .objects
        .iter()
        .filter_map(|(k, o)| {
            let prim_alive = o.prim.lock().map(|p| p.alive).unwrap_or(false);
            if !o.alive || !prim_alive {
                Some(*k)
            } else {
                None
            }
        })
        .collect();
    for k in &dead {
        plane.objects.remove(k);
    }
    let dead_ctrls: Vec<u64> = plane
        .controllers
        .iter()
        .filter_map(|(k, c)| {
            if !plane.objects.contains_key(&c.object_id) {
                Some(*k)
            } else {
                None
            }
        })
        .collect();
    for k in &dead_ctrls {
        plane.controllers.remove(k);
    }

    for obj in plane.objects.values_mut() {
        if let Ok(p) = obj.prim.lock() {
            obj.prim_alive = p.alive;
            obj.cached_size = p.size;
            obj.cached_z_index = p.z_index;
            obj.cached_shape = p.shape;
        }
    }

    let gravity = plane.gravity;
    let lin_damp = plane.linear_damping;
    let ang_damp = plane.angular_damping;
    let drag = plane.drag;
    let plane_force = plane.constant_force;
    let rest_threshold = plane.rest_threshold;
    let never_sleep = plane.never_sleep;
    let solver_iters = plane.solver_iterations.max(1);
    let bounds = plane.bounds;

    for obj in plane.objects.values_mut() {
        if obj.anchored || obj.sleeping {
            obj.velocity = Dim::new(0.0, 0.0);
            obj.angular_velocity = 0.0;
            continue;
        }

        let inv_mass = if obj.mass > 0.0001 { 1.0 / obj.mass } else { 0.0 };

        let mut ax = (gravity.x * obj.gravity_scale) + plane_force.x + obj.constant_force.x;
        let mut ay = (gravity.y * obj.gravity_scale) + plane_force.y + obj.constant_force.y;
        ax += obj.impulse.x * inv_mass;
        ay += obj.impulse.y * inv_mass;
        obj.impulse = Dim::new(0.0, 0.0);

        if drag > 0.0 {
            let vx = obj.velocity.x;
            let vy = obj.velocity.y;
            ax -= vx.abs() * vx * drag * inv_mass;
            ay -= vy.abs() * vy * drag * inv_mass;
        }

        obj.velocity = Dim::new(
            (obj.velocity.x + ax * dt) * (1.0 - lin_damp * dt).max(0.0),
            (obj.velocity.y + ay * dt) * (1.0 - lin_damp * dt).max(0.0),
        );
        obj.angular_velocity *= (1.0 - ang_damp * dt).max(0.0);

        if obj.locked_x {
            obj.velocity = Dim::new(0.0, obj.velocity.y);
        }
        if obj.locked_y {
            obj.velocity = Dim::new(obj.velocity.x, 0.0);
        }
        if obj.locked_rotation {
            obj.angular_velocity = 0.0;
        }

        obj.position = Dim::new(
            obj.position.x + obj.velocity.x * dt,
            obj.position.y + obj.velocity.y * dt,
        );
        if !obj.locked_rotation {
            obj.rotation += obj.angular_velocity * dt;
        }

        if let Some((mn, mx)) = bounds {
            let half = Dim::new(obj.cached_size.x * 0.5, obj.cached_size.y * 0.5);
            if obj.position.x - half.x < mn.x {
                obj.position = Dim::new(mn.x + half.x, obj.position.y);
                if obj.velocity.x < 0.0 {
                    obj.velocity = Dim::new(-obj.velocity.x * obj.bounciness, obj.velocity.y);
                }
            } else if obj.position.x + half.x > mx.x {
                obj.position = Dim::new(mx.x - half.x, obj.position.y);
                if obj.velocity.x > 0.0 {
                    obj.velocity = Dim::new(-obj.velocity.x * obj.bounciness, obj.velocity.y);
                }
            }
            if obj.position.y - half.y < mn.y {
                obj.position = Dim::new(obj.position.x, mn.y + half.y);
                if obj.velocity.y < 0.0 {
                    obj.velocity = Dim::new(obj.velocity.x, -obj.velocity.y * obj.bounciness);
                }
            } else if obj.position.y + half.y > mx.y {
                obj.position = Dim::new(obj.position.x, mx.y - half.y);
                if obj.velocity.y > 0.0 {
                    obj.velocity = Dim::new(obj.velocity.x, -obj.velocity.y * obj.bounciness);
                }
            }
        }
    }

    // Z-index bucketed collision resolution.
    let ids: Vec<u64> = plane.objects.keys().copied().collect();
    for _ in 0..solver_iters {
        for i in 0..ids.len() {
            for j in (i + 1)..ids.len() {
                let a_id = ids[i];
                let b_id = ids[j];
                let (z_a, z_b, can_a, can_b) = {
                    let a = plane.objects.get(&a_id);
                    let b = plane.objects.get(&b_id);
                    match (a, b) {
                        (Some(a), Some(b)) => {
                            (a.cached_z_index, b.cached_z_index, a.can_collide, b.can_collide)
                        }
                        _ => continue,
                    }
                };
                if z_a != z_b || !can_a || !can_b {
                    continue;
                }
                resolve_pair(&mut plane.objects, a_id, b_id);
            }
        }
    }

    for obj in plane.objects.values_mut() {
        if obj.anchored {
            continue;
        }
        let speed2 = obj.velocity.x * obj.velocity.x + obj.velocity.y * obj.velocity.y;
        if !never_sleep && speed2 < rest_threshold * rest_threshold {
            obj.sleep_timer += dt;
            if obj.sleep_timer > 0.5 {
                obj.sleeping = true;
                obj.velocity = Dim::new(0.0, 0.0);
                obj.angular_velocity = 0.0;
            }
        } else {
            obj.sleep_timer = 0.0;
        }
    }
}

fn resolve_pair(objects: &mut HashMap<u64, GuiObjectState>, a_id: u64, b_id: u64) {
    let (a_pos, a_size, a_shape, a_mass, a_anchored, a_vel, a_bounce, a_friction) = {
        let a = match objects.get(&a_id) {
            Some(a) => a,
            None => return,
        };
        (
            a.position,
            a.cached_size,
            a.cached_shape,
            a.mass,
            a.anchored,
            a.velocity,
            a.bounciness,
            a.friction,
        )
    };
    let (b_pos, b_size, b_shape, b_mass, b_anchored, b_vel, b_bounce, b_friction) = {
        let b = match objects.get(&b_id) {
            Some(b) => b,
            None => return,
        };
        (
            b.position,
            b.cached_size,
            b.cached_shape,
            b.mass,
            b.anchored,
            b.velocity,
            b.bounciness,
            b.friction,
        )
    };
    if a_anchored && b_anchored {
        return;
    }
    let a_circle = matches!(a_shape, Shape::Circle);
    let b_circle = matches!(b_shape, Shape::Circle);

    let manifold = if a_circle && b_circle {
        collide_circle_circle(a_pos, a_size, b_pos, b_size)
    } else if a_circle && !b_circle {
        collide_circle_aabb(a_pos, a_size, b_pos, b_size)
    } else if !a_circle && b_circle {
        collide_circle_aabb(b_pos, b_size, a_pos, a_size).map(|(n, d)| (Dim::new(-n.x, -n.y), d))
    } else {
        collide_aabb_aabb(a_pos, a_size, b_pos, b_size)
    };
    let (normal, penetration) = match manifold {
        Some(v) => v,
        None => return,
    };

    let inv_a = if a_anchored || a_mass <= 0.0001 { 0.0 } else { 1.0 / a_mass };
    let inv_b = if b_anchored || b_mass <= 0.0001 { 0.0 } else { 1.0 / b_mass };
    let inv_sum = inv_a + inv_b;
    if inv_sum <= 0.0 {
        return;
    }
    let correction = penetration / inv_sum;
    let bias = 0.8;
    let a_shift = Dim::new(
        -normal.x * correction * inv_a * bias,
        -normal.y * correction * inv_a * bias,
    );
    let b_shift = Dim::new(
        normal.x * correction * inv_b * bias,
        normal.y * correction * inv_b * bias,
    );

    let rel_x = b_vel.x - a_vel.x;
    let rel_y = b_vel.y - a_vel.y;
    let vel_along_n = rel_x * normal.x + rel_y * normal.y;
    let restitution = a_bounce.min(b_bounce);
    let mut j = 0.0;
    if vel_along_n < 0.0 {
        j = -(1.0 + restitution) * vel_along_n / inv_sum;
    }
    let impulse_x = j * normal.x;
    let impulse_y = j * normal.y;

    let tangent_x = -normal.y;
    let tangent_y = normal.x;
    let vel_along_t = rel_x * tangent_x + rel_y * tangent_y;
    let friction_mu = (a_friction * b_friction).sqrt();
    let mut jt = -vel_along_t / inv_sum;
    let max_friction = j.abs() * friction_mu;
    if jt.abs() > max_friction {
        jt = jt.signum() * max_friction;
    }
    let fric_x = jt * tangent_x;
    let fric_y = jt * tangent_y;

    if let Some(a) = objects.get_mut(&a_id) {
        if !a.anchored {
            a.position = Dim::new(a.position.x + a_shift.x, a.position.y + a_shift.y);
            a.velocity = Dim::new(
                a.velocity.x - (impulse_x + fric_x) * inv_a,
                a.velocity.y - (impulse_y + fric_y) * inv_a,
            );
            a.sleeping = false;
            a.sleep_timer = 0.0;
        }
    }
    if let Some(b) = objects.get_mut(&b_id) {
        if !b.anchored {
            b.position = Dim::new(b.position.x + b_shift.x, b.position.y + b_shift.y);
            b.velocity = Dim::new(
                b.velocity.x + (impulse_x + fric_x) * inv_b,
                b.velocity.y + (impulse_y + fric_y) * inv_b,
            );
            b.sleeping = false;
            b.sleep_timer = 0.0;
        }
    }
}

fn collide_aabb_aabb(a_pos: Dim, a_size: Dim, b_pos: Dim, b_size: Dim) -> Option<(Dim, f32)> {
    let ax_hw = a_size.x * 0.5;
    let ay_hh = a_size.y * 0.5;
    let bx_hw = b_size.x * 0.5;
    let by_hh = b_size.y * 0.5;
    let dx = b_pos.x - a_pos.x;
    let dy = b_pos.y - a_pos.y;
    let overlap_x = ax_hw + bx_hw - dx.abs();
    if overlap_x <= 0.0 {
        return None;
    }
    let overlap_y = ay_hh + by_hh - dy.abs();
    if overlap_y <= 0.0 {
        return None;
    }
    if overlap_x < overlap_y {
        let sign = if dx > 0.0 { 1.0 } else { -1.0 };
        Some((Dim::new(sign, 0.0), overlap_x))
    } else {
        let sign = if dy > 0.0 { 1.0 } else { -1.0 };
        Some((Dim::new(0.0, sign), overlap_y))
    }
}

fn collide_circle_circle(a_pos: Dim, a_size: Dim, b_pos: Dim, b_size: Dim) -> Option<(Dim, f32)> {
    let ar = a_size.x.min(a_size.y) * 0.5;
    let br = b_size.x.min(b_size.y) * 0.5;
    let dx = b_pos.x - a_pos.x;
    let dy = b_pos.y - a_pos.y;
    let dist2 = dx * dx + dy * dy;
    let r = ar + br;
    if dist2 >= r * r {
        return None;
    }
    let dist = dist2.sqrt();
    if dist < 1e-5 {
        return Some((Dim::new(1.0, 0.0), r));
    }
    Some((Dim::new(dx / dist, dy / dist), r - dist))
}

fn collide_circle_aabb(c_pos: Dim, c_size: Dim, r_pos: Dim, r_size: Dim) -> Option<(Dim, f32)> {
    let cr = c_size.x.min(c_size.y) * 0.5;
    let half = Dim::new(r_size.x * 0.5, r_size.y * 0.5);
    let dx = c_pos.x - r_pos.x;
    let dy = c_pos.y - r_pos.y;
    let cx = dx.clamp(-half.x, half.x);
    let cy = dy.clamp(-half.y, half.y);
    let ox = dx - cx;
    let oy = dy - cy;
    let dist2 = ox * ox + oy * oy;
    if dist2 >= cr * cr {
        return None;
    }
    let dist = dist2.sqrt();
    if dist < 1e-5 {
        let pen_x = half.x - dx.abs();
        let pen_y = half.y - dy.abs();
        return if pen_x < pen_y {
            let sign = if dx >= 0.0 { 1.0 } else { -1.0 };
            Some((Dim::new(-sign, 0.0), pen_x + cr))
        } else {
            let sign = if dy >= 0.0 { 1.0 } else { -1.0 };
            Some((Dim::new(0.0, -sign), pen_y + cr))
        };
    }
    Some((Dim::new(-ox / dist, -oy / dist), cr - dist))
}

fn tick_controllers(plane_arc: &Arc<Mutex<GuiPlaneState>>, dt: f32) {
    let mut plane = plane_arc.lock().unwrap();
    if !plane.alive || !plane.enabled {
        return;
    }
    let ids: Vec<u64> = plane.controllers.keys().copied().collect();
    for id in ids {
        let (object_id, move_dir, move_speed, accel, controlled, jump_request, jump_strength) = {
            let c = match plane.controllers.get(&id) {
                Some(c) => c,
                None => continue,
            };
            (
                c.object_id,
                c.move_dir,
                c.move_speed,
                c.accel,
                c.controlled,
                c.jump_request,
                c.jump_strength,
            )
        };
        if controlled {
            if let Some(obj) = plane.objects.get_mut(&object_id) {
                let target_vx = move_dir.x.clamp(-1.0, 1.0) * move_speed;
                let dvx = target_vx - obj.velocity.x;
                let max_step = accel * dt;
                let step_x = dvx.clamp(-max_step, max_step);
                obj.velocity = Dim::new(obj.velocity.x + step_x, obj.velocity.y);
                if jump_request && obj.velocity.y > -0.1 && obj.velocity.y < 50.0 {
                    obj.velocity = Dim::new(obj.velocity.x, obj.velocity.y - jump_strength);
                    obj.sleeping = false;
                    obj.sleep_timer = 0.0;
                }
            }
        } else {
            // Waypoint mode.
            let (waypoints, idx, radius, finished) = {
                let c = plane.controllers.get(&id).unwrap();
                (c.waypoints.clone(), c.waypoint_index, c.waypoint_radius, c.path_finished)
            };
            if !waypoints.is_empty() && !finished && idx < waypoints.len() {
                let target = waypoints[idx];
                if let Some(obj) = plane.objects.get_mut(&object_id) {
                    let dx = target.x - obj.position.x;
                    let dy = target.y - obj.position.y;
                    let dist = (dx * dx + dy * dy).sqrt();
                    if dist <= radius {
                        if let Some(c) = plane.controllers.get_mut(&id) {
                            c.waypoint_reached = Some(idx as i64 + 1);
                            c.waypoint_index += 1;
                            if c.waypoint_index >= c.waypoints.len() {
                                c.path_finished = true;
                            }
                        }
                    } else if dist > 1e-3 {
                        let inv = 1.0 / dist;
                        let nx = dx * inv;
                        let ny = dy * inv;
                        let target_vx = nx * move_speed;
                        let target_vy = ny * move_speed;
                        let max_step = accel * dt;
                        let step_x = (target_vx - obj.velocity.x).clamp(-max_step, max_step);
                        let step_y = (target_vy - obj.velocity.y).clamp(-max_step, max_step);
                        obj.velocity = Dim::new(
                            obj.velocity.x + step_x,
                            obj.velocity.y + step_y,
                        );
                        obj.sleeping = false;
                        obj.sleep_timer = 0.0;
                    }
                }
            }
        }
        if let Some(c) = plane.controllers.get_mut(&id) {
            c.jump_request = false;
        }

        // Ground probe: small AABB below the body.
        if let Some(obj) = plane.objects.get(&object_id) {
            let (pos, size, z, vy) = (obj.position, obj.cached_size, obj.cached_z_index, obj.velocity.y);
            let pad = 2.0;
            let foot_y = pos.y + size.y * 0.5 + pad;
            let mut on_ground = false;
            for (oid, o) in plane.objects.iter() {
                if *oid == object_id || !o.can_collide || o.cached_z_index != z {
                    continue;
                }
                let half_x = o.cached_size.x * 0.5;
                let half_y = o.cached_size.y * 0.5;
                if (o.position.x - pos.x).abs() > (size.x * 0.5 + half_x) {
                    continue;
                }
                let top = o.position.y - half_y;
                let bot = o.position.y + half_y;
                if foot_y >= top - 0.1 && foot_y <= bot + 0.1 && vy >= -0.1 {
                    on_ground = true;
                    break;
                }
            }
            if let Some(c) = plane.controllers.get_mut(&id) {
                c.last_on_ground = c.on_ground;
                c.on_ground = on_ground;
                if c.on_ground != c.last_on_ground {
                    c.ground_changed = true;
                    c.ground_value = c.on_ground;
                }
            }
        }

        // Moved delta for signals.
        let vel = plane.objects.get(&object_id).map(|o| o.velocity);
        if let Some(v) = vel {
            if let Some(c) = plane.controllers.get_mut(&id) {
                c.moved_dx = v.x * dt;
                c.moved_dy = v.y * dt;
            }
        }
    }
}

fn fire_signals(lua: &Lua, plane_arc: &Arc<Mutex<GuiPlaneState>>) {
    let snapshots: Vec<(
        Option<i64>,
        Table,
        bool,
        Table,
        f32,
        f32,
        Table,
        bool,
        bool,
        Table,
    )> = {
        let mut plane = plane_arc.lock().unwrap();
        plane
            .controllers
            .values_mut()
            .map(|c| {
                let snap = (
                    c.waypoint_reached.take(),
                    c.waypoint_reached_signal.clone(),
                    c.path_finished,
                    c.path_finished_signal.clone(),
                    c.moved_dx,
                    c.moved_dy,
                    c.moved_signal.clone(),
                    c.ground_changed,
                    c.ground_value,
                    c.ground_signal.clone(),
                );
                c.ground_changed = false;
                c.moved_dx = 0.0;
                c.moved_dy = 0.0;
                if c.path_finished {
                    // Only fire once.
                    c.path_finished = false;
                }
                snap
            })
            .collect()
    };
    for (wp, wp_sig, path_done, path_sig, dx, dy, moved_sig, ground_changed, ground_value, ground_sig) in
        snapshots
    {
        if let Some(idx) = wp {
            let mut args = MultiValue::new();
            args.push_back(Value::Integer(idx));
            let _ = signal::fire(lua, &wp_sig, args);
        }
        if path_done {
            let _ = signal::fire(lua, &path_sig, MultiValue::new());
        }
        if dx.abs() > 1e-4 || dy.abs() > 1e-4 {
            let mut args = MultiValue::new();
            args.push_back(Value::Number(dx as f64));
            args.push_back(Value::Number(dy as f64));
            let _ = signal::fire(lua, &moved_sig, args);
        }
        if ground_changed {
            let mut args = MultiValue::new();
            args.push_back(Value::Boolean(ground_value));
            let _ = signal::fire(lua, &ground_sig, args);
        }
    }
}

fn write_back(lua: &Lua, plane_arc: &Arc<Mutex<GuiPlaneState>>) {
    let writes: Vec<(Arc<Mutex<PrimitiveState>>, Dim, f32)> = {
        let plane = plane_arc.lock().unwrap();
        plane
            .objects
            .values()
            .filter(|o| o.alive && o.prim_alive)
            .map(|o| (o.prim.clone(), o.position, o.rotation))
            .collect()
    };
    for (prim, pos, rot) in writes {
        let (changed_sig, pos_prop_sig, rot_prop_sig) = {
            let mut p = match prim.lock() {
                Ok(p) => p,
                Err(_) => continue,
            };
            if !p.alive {
                continue;
            }
            let pos_changed = p.position.x != pos.x || p.position.y != pos.y;
            let rot_changed = (p.rotation - rot).abs() > 1e-4;
            if pos_changed {
                p.position = pos;
            }
            if rot_changed {
                p.rotation = rot;
            }
            (
                if pos_changed || rot_changed {
                    Some(p.changed_signal.clone())
                } else {
                    None
                },
                if pos_changed {
                    p.prop_signals.get("Position").cloned()
                } else {
                    None
                },
                if rot_changed {
                    p.prop_signals.get("Rotation").cloned()
                } else {
                    None
                },
            )
        };
        if let Some(sig) = changed_sig {
            if let Ok(s) = lua.create_string("Position") {
                let mut args = MultiValue::new();
                args.push_back(Value::String(s));
                let _ = signal::fire(lua, &sig, args);
            }
        }
        if let Some(sig) = pos_prop_sig {
            if let Ok(ud) = lua.create_userdata(pos) {
                let mut args = MultiValue::new();
                args.push_back(Value::UserData(ud));
                let _ = signal::fire(lua, &sig, args);
            }
        }
        if let Some(sig) = rot_prop_sig {
            let mut args = MultiValue::new();
            args.push_back(Value::Number(rot as f64));
            let _ = signal::fire(lua, &sig, args);
        }
    }
}

fn raycast(
    lua: &Lua,
    plane_arc: &Arc<Mutex<GuiPlaneState>>,
    from: Dim,
    dir: Dim,
    max_dist: f32,
) -> mlua::Result<Value> {
    let mag = (dir.x * dir.x + dir.y * dir.y).sqrt();
    if mag < 1e-5 {
        return Ok(Value::Nil);
    }
    let nx = dir.x / mag;
    let ny = dir.y / mag;
    let plane = plane_arc.lock().unwrap();
    let mut best: Option<(u64, f32, Dim)> = None;
    for (id, obj) in plane.objects.iter() {
        if !obj.can_collide {
            continue;
        }
        let half = Dim::new(obj.cached_size.x * 0.5, obj.cached_size.y * 0.5);
        let mn_x = obj.position.x - half.x;
        let mx_x = obj.position.x + half.x;
        let mn_y = obj.position.y - half.y;
        let mx_y = obj.position.y + half.y;
        let (t_near, hit_normal) = ray_aabb(from, Dim::new(nx, ny), mn_x, mn_y, mx_x, mx_y);
        if let Some(t) = t_near {
            if t >= 0.0 && t <= max_dist {
                if best.as_ref().map(|(_, bt, _)| t < *bt).unwrap_or(true) {
                    best = Some((*id, t, hit_normal));
                }
            }
        }
    }
    drop(plane);
    if let Some((id, t, normal)) = best {
        let hit = lua.create_table()?;
        hit.set("Distance", t as f64)?;
        hit.set("Position", lua.create_userdata(Dim::new(from.x + nx * t, from.y + ny * t))?)?;
        hit.set("Normal", lua.create_userdata(normal)?)?;
        hit.set(
            "Object",
            GuiObjectHandle {
                plane: plane_arc.clone(),
                id,
            },
        )?;
        return Ok(Value::Table(hit));
    }
    Ok(Value::Nil)
}

fn ray_aabb(from: Dim, dir: Dim, mn_x: f32, mn_y: f32, mx_x: f32, mx_y: f32) -> (Option<f32>, Dim) {
    let inv_dx = if dir.x.abs() > 1e-7 { 1.0 / dir.x } else { f32::INFINITY };
    let inv_dy = if dir.y.abs() > 1e-7 { 1.0 / dir.y } else { f32::INFINITY };
    let tx1 = (mn_x - from.x) * inv_dx;
    let tx2 = (mx_x - from.x) * inv_dx;
    let ty1 = (mn_y - from.y) * inv_dy;
    let ty2 = (mx_y - from.y) * inv_dy;
    let t_min = tx1.min(tx2).max(ty1.min(ty2));
    let t_max = tx1.max(tx2).min(ty1.max(ty2));
    if t_max < 0.0 || t_min > t_max {
        return (None, Dim::new(0.0, 0.0));
    }
    let t = if t_min >= 0.0 { t_min } else { t_max };
    let mut nx = 0.0;
    let mut ny = 0.0;
    if t == tx1 {
        nx = -1.0;
    } else if t == tx2 {
        nx = 1.0;
    } else if t == ty1 {
        ny = -1.0;
    } else if t == ty2 {
        ny = 1.0;
    }
    (Some(t), Dim::new(nx, ny))
}
