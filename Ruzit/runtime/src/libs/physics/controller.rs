use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use mlua::{
    AnyUserData, Lua, MultiValue, Table, UserData, UserDataFields, UserDataMethods, Value,
};
use rapier3d::na::{Point3, UnitQuaternion, Vector3 as NaVec3};
use rapier3d::prelude::*;

use crate::libs::primitives::{CFrame, Vector};
use crate::libs::renderable::{self, PartHandle, PartState};
use crate::libs::signal;

use super::PlaneState;

static NEXT_CONTROLLER_ID: AtomicU64 = AtomicU64::new(1);

pub struct ControllerState {
    pub id: u64,
    pub alive: bool,
    pub part: Arc<Mutex<PartState>>,
    pub override_cell: Arc<Mutex<CFrame>>,

    pub position: Vector,
    pub velocity: Vector,
    pub rotation: Vector,

    pub walk_speed: f32,
    pub turn_speed: f32,
    pub jump_power: f32,
    pub gravity: f32,
    pub use_plane_gravity: bool,
    pub max_slope: f32,
    pub max_step_height: f32,
    pub ground_y: f32,
    pub use_ground_y_fallback: bool,
    pub waypoint_threshold: f32,
    pub probe_distance: f32,
    pub capsule_radius: f32,
    pub capsule_half_height: f32,

    pub on_ground: bool,
    pub was_on_ground: bool,
    pub jump_request: bool,

    pub path: Vec<Vector>,
    pub path_idx: usize,

    pub controlled: bool,
    pub move_intent: [f32; 2],

    pub look_where_moving: bool,

    pub jumped_signal: Table,
    pub landed_signal: Table,
    pub moved_signal: Table,
    pub waypoint_reached_signal: Table,
    pub path_finished_signal: Table,
    pub changed_signal: Table,
}

#[derive(Default)]
pub struct ControllerOpts {
    pub walk_speed: Option<f32>,
    pub turn_speed: Option<f32>,
    pub jump_power: Option<f32>,
    pub gravity: Option<f32>,
    pub use_plane_gravity: Option<bool>,
    pub max_slope_deg: Option<f32>,
    pub max_step_height: Option<f32>,
    pub ground_y: Option<f32>,
    pub use_ground_y_fallback: Option<bool>,
    pub waypoint_threshold: Option<f32>,
    pub probe_distance: Option<f32>,
    pub capsule_radius: Option<f32>,
    pub capsule_half_height: Option<f32>,
    pub look_where_moving: Option<bool>,
}

impl ControllerOpts {
    pub fn from_table(t: Option<Table>) -> mlua::Result<Self> {
        let mut o = ControllerOpts::default();
        let Some(t) = t else { return Ok(o) };
        o.walk_speed = t.get::<f32>("WalkSpeed").ok();
        o.turn_speed = t.get::<f32>("TurnSpeed").ok();
        o.jump_power = t.get::<f32>("JumpPower").ok();
        o.gravity = t.get::<f32>("Gravity").ok();
        o.use_plane_gravity = t.get::<bool>("UsePlaneGravity").ok();
        o.max_slope_deg = t.get::<f32>("MaxSlope").ok();
        o.max_step_height = t.get::<f32>("MaxStepHeight").ok();
        o.ground_y = t.get::<f32>("GroundY").ok();
        o.use_ground_y_fallback = t.get::<bool>("UseGroundYFallback").ok();
        o.waypoint_threshold = t.get::<f32>("WaypointThreshold").ok();
        o.probe_distance = t.get::<f32>("ProbeDistance").ok();
        o.capsule_radius = t.get::<f32>("CapsuleRadius").ok();
        o.capsule_half_height = t.get::<f32>("CapsuleHalfHeight").ok();
        o.look_where_moving = t.get::<bool>("LookWhereMoving").ok();
        Ok(o)
    }
}

pub fn make_controller(
    lua: &Lua,
    part: Arc<Mutex<PartState>>,
    controlled: bool,
    opts: ControllerOpts,
) -> mlua::Result<ControllerState> {
    let (initial_pos, initial_rot, override_cell) = {
        let mut s = part.lock().unwrap();
        if !s.alive {
            return Err(mlua::Error::RuntimeError(
                "NewController: BasePart is destroyed".into(),
            ));
        }
        let cf = s.cframe;
        let cell = s
            .physics_override
            .get_or_insert_with(|| Arc::new(Mutex::new(cf)))
            .clone();
        if let Ok(mut g) = cell.lock() {
            *g = cf;
        }
        (cf.position, cf.rotation, cell)
    };

    let jumped = signal::new_instance(lua)?;
    let landed = signal::new_instance(lua)?;
    let moved = signal::new_instance(lua)?;
    let waypoint_reached = signal::new_instance(lua)?;
    let path_finished = signal::new_instance(lua)?;
    let changed = signal::new_instance(lua)?;

    let id = NEXT_CONTROLLER_ID.fetch_add(1, Ordering::Relaxed);
    Ok(ControllerState {
        id,
        alive: true,
        part,
        override_cell,
        position: initial_pos,
        velocity: Vector::new(0.0, 0.0, 0.0),
        rotation: initial_rot,
        walk_speed: opts.walk_speed.unwrap_or(16.0).max(0.0),
        turn_speed: opts.turn_speed.unwrap_or(12.0).max(0.0),
        jump_power: opts.jump_power.unwrap_or(28.0).max(0.0),
        gravity: opts.gravity.unwrap_or(50.0).max(0.0),
        use_plane_gravity: opts.use_plane_gravity.unwrap_or(true),
        max_slope: opts.max_slope_deg.unwrap_or(50.0).to_radians(),
        max_step_height: opts.max_step_height.unwrap_or(0.5).max(0.0),
        ground_y: opts.ground_y.unwrap_or(0.0),
        use_ground_y_fallback: opts.use_ground_y_fallback.unwrap_or(true),
        waypoint_threshold: opts.waypoint_threshold.unwrap_or(0.5).max(0.05),
        probe_distance: opts.probe_distance.unwrap_or(2.0).max(0.1),
        capsule_radius: opts.capsule_radius.unwrap_or(0.5).max(0.05),
        capsule_half_height: opts.capsule_half_height.unwrap_or(1.0).max(0.05),
        on_ground: false,
        was_on_ground: false,
        jump_request: false,
        path: Vec::new(),
        path_idx: 0,
        controlled,
        move_intent: [0.0, 0.0],
        look_where_moving: opts
            .look_where_moving
            .unwrap_or(!controlled),
        jumped_signal: jumped,
        landed_signal: landed,
        moved_signal: moved,
        waypoint_reached_signal: waypoint_reached,
        path_finished_signal: path_finished,
        changed_signal: changed,
    })
}

pub fn tick_controllers(lua: &Lua, plane_arc: &Arc<Mutex<PlaneState>>, dt: f32) {
    let (controller_ids, plane_gravity_y) = {
        let plane = plane_arc.lock().unwrap();
        if !plane.alive || !plane.enabled {
            return;
        }
        let ids: Vec<u64> = plane.controllers.keys().copied().collect();
        (ids, plane.gravity.y)
    };

    let mut fires: Vec<Fire> = Vec::new();

    for cid in controller_ids {
        let (mut state_snapshot, part_alive) = {
            let plane = plane_arc.lock().unwrap();
            let Some(c) = plane.controllers.get(&cid) else {
                continue;
            };
            if !c.alive {
                continue;
            }
            let part_alive = c.part.lock().map(|p| p.alive).unwrap_or(false);
            (clone_for_tick(c), part_alive)
        };
        if !part_alive {
            let mut plane = plane_arc.lock().unwrap();
            if let Some(c) = plane.controllers.get_mut(&cid) {
                c.alive = false;
            }
            continue;
        }

        let result = step_controller(&mut state_snapshot, plane_arc, plane_gravity_y, dt);

        {
            let mut plane = plane_arc.lock().unwrap();
            if let Some(c) = plane.controllers.get_mut(&cid) {
                c.position = state_snapshot.position;
                c.velocity = state_snapshot.velocity;
                c.rotation = state_snapshot.rotation;
                c.on_ground = state_snapshot.on_ground;
                c.was_on_ground = state_snapshot.was_on_ground;
                c.jump_request = state_snapshot.jump_request;
                c.path = state_snapshot.path.clone();
                c.path_idx = state_snapshot.path_idx;
            }
        }

        let part = state_snapshot.part.clone();
        let override_cell = state_snapshot.override_cell.clone();
        let new_cf = CFrame {
            position: state_snapshot.position,
            rotation: state_snapshot.rotation,
        };
        if let Ok(mut g) = override_cell.lock() {
            *g = new_cf;
        }
        if let Ok(mut p) = part.lock() {
            if p.alive {
                p.cframe = new_cf;
            }
        }

        fires.push(Fire {
            jumped: result.jumped,
            landed: result.landed,
            moved: result.moved_speed_sq > 1e-6,
            moved_dx: state_snapshot.velocity.x,
            moved_dy: state_snapshot.velocity.y,
            moved_dz: state_snapshot.velocity.z,
            waypoint_reached: result.waypoint_reached,
            path_finished: result.path_finished,
            jumped_signal: state_snapshot.jumped_signal.clone(),
            landed_signal: state_snapshot.landed_signal.clone(),
            moved_signal: state_snapshot.moved_signal.clone(),
            waypoint_reached_signal: state_snapshot.waypoint_reached_signal.clone(),
            path_finished_signal: state_snapshot.path_finished_signal.clone(),
        });
    }

    renderable::bump_parts_dirty();

    for f in fires {
        if f.jumped {
            let _ = signal::fire(lua, &f.jumped_signal, MultiValue::new());
        }
        if f.landed {
            let _ = signal::fire(lua, &f.landed_signal, MultiValue::new());
        }
        if f.moved {
            let mut args = MultiValue::new();
            args.push_back(Value::Number(f.moved_dx as f64));
            args.push_back(Value::Number(f.moved_dy as f64));
            args.push_back(Value::Number(f.moved_dz as f64));
            let _ = signal::fire(lua, &f.moved_signal, args);
        }
        if let Some(idx) = f.waypoint_reached {
            let mut args = MultiValue::new();
            args.push_back(Value::Integer(idx as i32));
            let _ = signal::fire(lua, &f.waypoint_reached_signal, args);
        }
        if f.path_finished {
            let _ = signal::fire(lua, &f.path_finished_signal, MultiValue::new());
        }
    }
}

struct Fire {
    jumped: bool,
    landed: bool,
    moved: bool,
    moved_dx: f32,
    moved_dy: f32,
    moved_dz: f32,
    waypoint_reached: Option<usize>,
    path_finished: bool,
    jumped_signal: Table,
    landed_signal: Table,
    moved_signal: Table,
    waypoint_reached_signal: Table,
    path_finished_signal: Table,
}

struct StepResult {
    jumped: bool,
    landed: bool,
    moved_speed_sq: f32,
    waypoint_reached: Option<usize>,
    path_finished: bool,
}

fn clone_for_tick(c: &ControllerState) -> ControllerState {
    ControllerState {
        id: c.id,
        alive: c.alive,
        part: c.part.clone(),
        override_cell: c.override_cell.clone(),
        position: c.position,
        velocity: c.velocity,
        rotation: c.rotation,
        walk_speed: c.walk_speed,
        turn_speed: c.turn_speed,
        jump_power: c.jump_power,
        gravity: c.gravity,
        use_plane_gravity: c.use_plane_gravity,
        max_slope: c.max_slope,
        max_step_height: c.max_step_height,
        ground_y: c.ground_y,
        use_ground_y_fallback: c.use_ground_y_fallback,
        waypoint_threshold: c.waypoint_threshold,
        probe_distance: c.probe_distance,
        capsule_radius: c.capsule_radius,
        capsule_half_height: c.capsule_half_height,
        on_ground: c.on_ground,
        was_on_ground: c.was_on_ground,
        jump_request: c.jump_request,
        path: c.path.clone(),
        path_idx: c.path_idx,
        controlled: c.controlled,
        move_intent: c.move_intent,
        look_where_moving: c.look_where_moving,
        jumped_signal: c.jumped_signal.clone(),
        landed_signal: c.landed_signal.clone(),
        moved_signal: c.moved_signal.clone(),
        waypoint_reached_signal: c.waypoint_reached_signal.clone(),
        path_finished_signal: c.path_finished_signal.clone(),
        changed_signal: c.changed_signal.clone(),
    }
}

fn step_controller(
    c: &mut ControllerState,
    plane_arc: &Arc<Mutex<PlaneState>>,
    plane_gravity_y: f32,
    dt: f32,
) -> StepResult {
    let dt = dt.max(0.0).min(1.0 / 30.0);

    let mut desired_h = [0.0_f32, 0.0_f32];
    let mut waypoint_reached: Option<usize> = None;
    let mut path_finished = false;

    let yaw = c.rotation.y;
    let forward = [yaw.sin(), yaw.cos()];
    let right = [yaw.cos(), -yaw.sin()];

    if !c.path.is_empty() && c.path_idx < c.path.len() {
        let wp = c.path[c.path_idx];
        let dx = wp.x - c.position.x;
        let dz = wp.z - c.position.z;
        let dist_h = (dx * dx + dz * dz).sqrt();
        if dist_h <= c.waypoint_threshold {
            waypoint_reached = Some(c.path_idx);
            c.path_idx += 1;
            if c.path_idx >= c.path.len() {
                c.path.clear();
                c.path_idx = 0;
                path_finished = true;
            }
        } else {
            let inv = 1.0 / dist_h.max(1e-6);
            desired_h[0] = dx * inv * c.walk_speed;
            desired_h[1] = dz * inv * c.walk_speed;
        }
    }

    if c.controlled {
        let f = c.move_intent[1];
        let r = c.move_intent[0];
        let mut vx = forward[0] * f + right[0] * r;
        let mut vz = forward[1] * f + right[1] * r;
        let len = (vx * vx + vz * vz).sqrt();
        if len > 1.0 {
            vx /= len;
            vz /= len;
        }
        desired_h[0] += vx * c.walk_speed;
        desired_h[1] += vz * c.walk_speed;
    }

    let len_h = (desired_h[0] * desired_h[0] + desired_h[1] * desired_h[1]).sqrt();
    if len_h > c.walk_speed {
        let inv = c.walk_speed / len_h;
        desired_h[0] *= inv;
        desired_h[1] *= inv;
    }

    let accel = 60.0_f32;
    let drag_h = if c.on_ground { 18.0 } else { 4.0 };
    let blend_in = (accel * dt).min(1.0);
    let blend_out = (drag_h * dt).min(1.0);
    if desired_h[0].abs() + desired_h[1].abs() > 1e-4 {
        c.velocity.x += (desired_h[0] - c.velocity.x) * blend_in;
        c.velocity.z += (desired_h[1] - c.velocity.z) * blend_in;
    } else {
        c.velocity.x += (0.0 - c.velocity.x) * blend_out;
        c.velocity.z += (0.0 - c.velocity.z) * blend_out;
    }

    let mut jumped = false;
    if c.jump_request {
        c.jump_request = false;
        if c.on_ground {
            c.velocity.y = c.jump_power;
            c.on_ground = false;
            jumped = true;
        }
    }

    let g = if c.use_plane_gravity {
        plane_gravity_y.abs().max(0.0)
    } else {
        c.gravity
    };
    if !c.on_ground {
        c.velocity.y -= g * dt;
    }

    c.position.x += c.velocity.x * dt;
    c.position.y += c.velocity.y * dt;
    c.position.z += c.velocity.z * dt;

    let ground_hit = probe_ground(plane_arc, c);
    let mut landed = false;
    let prev_on_ground = c.was_on_ground;
    c.on_ground = false;
    if let Some(ground_y) = ground_hit {
        let foot = c.position.y - c.capsule_half_height - c.capsule_radius;
        if foot <= ground_y + 0.05 && c.velocity.y <= 1e-3 {
            c.position.y = ground_y + c.capsule_half_height + c.capsule_radius;
            c.velocity.y = 0.0;
            c.on_ground = true;
        }
    } else if c.use_ground_y_fallback {
        let foot = c.position.y - c.capsule_half_height - c.capsule_radius;
        if foot <= c.ground_y && c.velocity.y <= 1e-3 {
            c.position.y = c.ground_y + c.capsule_half_height + c.capsule_radius;
            c.velocity.y = 0.0;
            c.on_ground = true;
        }
    }
    if c.on_ground && !prev_on_ground {
        landed = true;
    }
    c.was_on_ground = c.on_ground;

    if c.look_where_moving {
        let speed_h_sq = c.velocity.x * c.velocity.x + c.velocity.z * c.velocity.z;
        if speed_h_sq > 0.04 {
            let target_yaw = c.velocity.x.atan2(c.velocity.z);
            c.rotation.y = blend_angle(c.rotation.y, target_yaw, c.turn_speed * dt);
        }
    }

    let moved_speed_sq =
        c.velocity.x * c.velocity.x + c.velocity.y * c.velocity.y + c.velocity.z * c.velocity.z;

    StepResult {
        jumped,
        landed,
        moved_speed_sq,
        waypoint_reached,
        path_finished,
    }
}

fn blend_angle(current: f32, target: f32, factor: f32) -> f32 {
    let mut diff = target - current;
    while diff > std::f32::consts::PI {
        diff -= 2.0 * std::f32::consts::PI;
    }
    while diff < -std::f32::consts::PI {
        diff += 2.0 * std::f32::consts::PI;
    }
    let t = factor.clamp(0.0, 1.0);
    current + diff * t
}

fn probe_ground(plane_arc: &Arc<Mutex<PlaneState>>, c: &ControllerState) -> Option<f32> {
    let mut plane = plane_arc.lock().ok()?;
    let rapier = plane.rapier.as_deref_mut()?;
    let origin = Point3::new(
        c.position.x,
        c.position.y + c.capsule_half_height,
        c.position.z,
    );
    let dir = NaVec3::new(0.0, -1.0, 0.0);
    let max_toi =
        c.capsule_half_height + c.capsule_radius + c.probe_distance + c.max_step_height;
    let ray = rapier3d::geometry::Ray::new(origin, dir);
    let filter = QueryFilter::default();
    rapier
        .query_pipeline
        .cast_ray(&rapier.bodies, &rapier.colliders, &ray, max_toi, true, filter)
        .map(|(_handle, toi)| origin.y - toi)
}

pub fn part_of(c: &ControllerState) -> Arc<Mutex<PartState>> {
    c.part.clone()
}

pub fn fire_changed(lua: &Lua, sig: &Table, prop: &str) {
    let mut args = MultiValue::new();
    if let Ok(s) = lua.create_string(prop) {
        args.push_back(Value::String(s));
    }
    let _ = signal::fire(lua, sig, args);
}

#[derive(Clone)]
pub struct ControllerHandle {
    pub plane: Arc<Mutex<PlaneState>>,
    pub id: u64,
}

impl ControllerHandle {
    fn ensure_alive(&self, op: &str) -> mlua::Result<()> {
        let plane = self.plane.lock().unwrap();
        match plane.controllers.get(&self.id) {
            Some(c) if c.alive => Ok(()),
            _ => Err(mlua::Error::RuntimeError(format!(
                "Controller: {op} called on a destroyed controller"
            ))),
        }
    }
    fn with<R>(&self, op: &str, f: impl FnOnce(&ControllerState) -> R) -> mlua::Result<R> {
        let plane = self.plane.lock().unwrap();
        let c = plane.controllers.get(&self.id).ok_or_else(|| {
            mlua::Error::RuntimeError(format!(
                "Controller: {op} called on a destroyed controller"
            ))
        })?;
        Ok(f(c))
    }
    fn with_mut<R>(
        &self,
        op: &str,
        f: impl FnOnce(&mut ControllerState) -> R,
    ) -> mlua::Result<R> {
        let mut plane = self.plane.lock().unwrap();
        let c = plane.controllers.get_mut(&self.id).ok_or_else(|| {
            mlua::Error::RuntimeError(format!(
                "Controller: {op} called on a destroyed controller"
            ))
        })?;
        Ok(f(c))
    }
}

impl UserData for ControllerHandle {
    fn add_fields<F: UserDataFields<Self>>(f: &mut F) {
        f.add_field_method_get("Alive", |_, this| {
            Ok(this
                .with("get Alive", |c| c.alive)
                .unwrap_or(false))
        });
        f.add_field_method_get("Position", |_, this| {
            this.with("get Position", |c| c.position)
        });
        f.add_field_method_set("Position", |_, this, v: Vector| {
            this.with_mut("set Position", |c| {
                c.position = v;
                c.velocity = Vector::new(0.0, 0.0, 0.0);
            })
        });
        f.add_field_method_get("Velocity", |_, this| {
            this.with("get Velocity", |c| c.velocity)
        });
        f.add_field_method_set("Velocity", |_, this, v: Vector| {
            this.with_mut("set Velocity", |c| c.velocity = v)
        });
        f.add_field_method_get("Rotation", |_, this| {
            this.with("get Rotation", |c| c.rotation)
        });
        f.add_field_method_set("Rotation", |_, this, v: Vector| {
            this.with_mut("set Rotation", |c| c.rotation = v)
        });

        f.add_field_method_get("WalkSpeed", |_, this| {
            this.with("get WalkSpeed", |c| c.walk_speed)
        });
        f.add_field_method_set("WalkSpeed", |_, this, v: f32| {
            this.with_mut("set WalkSpeed", |c| c.walk_speed = v.max(0.0))
        });
        f.add_field_method_get("TurnSpeed", |_, this| {
            this.with("get TurnSpeed", |c| c.turn_speed)
        });
        f.add_field_method_set("TurnSpeed", |_, this, v: f32| {
            this.with_mut("set TurnSpeed", |c| c.turn_speed = v.max(0.0))
        });
        f.add_field_method_get("JumpPower", |_, this| {
            this.with("get JumpPower", |c| c.jump_power)
        });
        f.add_field_method_set("JumpPower", |_, this, v: f32| {
            this.with_mut("set JumpPower", |c| c.jump_power = v.max(0.0))
        });
        f.add_field_method_get("Gravity", |_, this| {
            this.with("get Gravity", |c| c.gravity)
        });
        f.add_field_method_set("Gravity", |_, this, v: f32| {
            this.with_mut("set Gravity", |c| {
                c.gravity = v.max(0.0);
                c.use_plane_gravity = false;
            })
        });
        f.add_field_method_get("UsePlaneGravity", |_, this| {
            this.with("get UsePlaneGravity", |c| c.use_plane_gravity)
        });
        f.add_field_method_set("UsePlaneGravity", |_, this, v: bool| {
            this.with_mut("set UsePlaneGravity", |c| c.use_plane_gravity = v)
        });
        f.add_field_method_get("OnGround", |_, this| {
            this.with("get OnGround", |c| c.on_ground)
        });
        f.add_field_method_get("LookWhereMoving", |_, this| {
            this.with("get LookWhereMoving", |c| c.look_where_moving)
        });
        f.add_field_method_set("LookWhereMoving", |_, this, v: bool| {
            this.with_mut("set LookWhereMoving", |c| c.look_where_moving = v)
        });
        f.add_field_method_get("Controlled", |_, this| {
            this.with("get Controlled", |c| c.controlled)
        });
        f.add_field_method_get("PathLength", |_, this| {
            this.with("get PathLength", |c| c.path.len() as i64)
        });
        f.add_field_method_get("PathIndex", |_, this| {
            this.with("get PathIndex", |c| c.path_idx as i64)
        });
        f.add_field_method_get("GroundY", |_, this| {
            this.with("get GroundY", |c| c.ground_y)
        });
        f.add_field_method_set("GroundY", |_, this, v: f32| {
            this.with_mut("set GroundY", |c| c.ground_y = v)
        });

        f.add_field_method_get("Jumped", |_, this| {
            this.with("get Jumped", |c| c.jumped_signal.clone())
        });
        f.add_field_method_get("Landed", |_, this| {
            this.with("get Landed", |c| c.landed_signal.clone())
        });
        f.add_field_method_get("Moved", |_, this| {
            this.with("get Moved", |c| c.moved_signal.clone())
        });
        f.add_field_method_get("WaypointReached", |_, this| {
            this.with("get WaypointReached", |c| c.waypoint_reached_signal.clone())
        });
        f.add_field_method_get("PathFinished", |_, this| {
            this.with("get PathFinished", |c| c.path_finished_signal.clone())
        });
        f.add_field_method_get("Changed", |_, this| {
            this.with("get Changed", |c| c.changed_signal.clone())
        });
    }

    fn add_methods<M: UserDataMethods<Self>>(m: &mut M) {
        m.add_method("BasePart", |_, this, _: ()| -> mlua::Result<PartHandle> {
            let part = this.with("BasePart", |c| c.part.clone())?;
            Ok(PartHandle::from_state(part))
        });

        m.add_method(
            "WalkTo",
            |_, this, target: Value| -> mlua::Result<()> {
                this.ensure_alive("WalkTo")?;
                let pos = vector_from_value(&target, "WalkTo")?;
                this.with_mut("WalkTo", |c| {
                    c.path.clear();
                    c.path.push(pos);
                    c.path_idx = 0;
                })?;
                Ok(())
            },
        );

        m.add_method(
            "ComputePath",
            |_, this, waypoints: Value| -> mlua::Result<i64> {
                this.ensure_alive("ComputePath")?;
                let points = match waypoints {
                    Value::Table(t) => {
                        let mut out: Vec<Vector> = Vec::new();
                        let len = t.raw_len() as i64;
                        for i in 1..=len {
                            let v: Value = t.get(i)?;
                            out.push(vector_from_value(&v, "ComputePath")?);
                        }
                        out
                    }
                    _ => {
                        return Err(mlua::Error::RuntimeError(
                            "ComputePath expects a table of waypoints".into(),
                        ));
                    }
                };
                let count = points.len() as i64;
                this.with_mut("ComputePath", |c| {
                    c.path = points;
                    c.path_idx = 0;
                })?;
                Ok(count)
            },
        );

        m.add_method("AppendWaypoint", |_, this, target: Value| -> mlua::Result<()> {
            this.ensure_alive("AppendWaypoint")?;
            let pos = vector_from_value(&target, "AppendWaypoint")?;
            this.with_mut("AppendWaypoint", |c| c.path.push(pos))?;
            Ok(())
        });

        m.add_method("StopWalking", |_, this, _: ()| -> mlua::Result<()> {
            this.with_mut("StopWalking", |c| {
                c.path.clear();
                c.path_idx = 0;
                c.move_intent = [0.0, 0.0];
            })?;
            Ok(())
        });

        m.add_method("Jump", |_, this, _: ()| -> mlua::Result<bool> {
            let mut fired = false;
            this.with_mut("Jump", |c| {
                if c.on_ground {
                    c.jump_request = true;
                    fired = true;
                }
            })?;
            Ok(fired)
        });

        m.add_method(
            "SetMoveVector",
            |_, this, (x, z): (f32, f32)| -> mlua::Result<()> {
                this.ensure_alive("SetMoveVector")?;
                this.with_mut("SetMoveVector", |c| {
                    c.move_intent = [x, z];
                })?;
                Ok(())
            },
        );
        m.add_method(
            "MoveForward",
            |_, this, intensity: Option<f32>| -> mlua::Result<()> {
                let amt = intensity.unwrap_or(1.0);
                this.with_mut("MoveForward", |c| c.move_intent[1] = amt)?;
                Ok(())
            },
        );
        m.add_method(
            "MoveBackward",
            |_, this, intensity: Option<f32>| -> mlua::Result<()> {
                let amt = intensity.unwrap_or(1.0);
                this.with_mut("MoveBackward", |c| c.move_intent[1] = -amt)?;
                Ok(())
            },
        );
        m.add_method(
            "MoveLeft",
            |_, this, intensity: Option<f32>| -> mlua::Result<()> {
                let amt = intensity.unwrap_or(1.0);
                this.with_mut("MoveLeft", |c| c.move_intent[0] = -amt)?;
                Ok(())
            },
        );
        m.add_method(
            "MoveRight",
            |_, this, intensity: Option<f32>| -> mlua::Result<()> {
                let amt = intensity.unwrap_or(1.0);
                this.with_mut("MoveRight", |c| c.move_intent[0] = amt)?;
                Ok(())
            },
        );
        m.add_method("ClearMoveIntent", |_, this, _: ()| -> mlua::Result<()> {
            this.with_mut("ClearMoveIntent", |c| c.move_intent = [0.0, 0.0])?;
            Ok(())
        });

        m.add_method("Teleport", |_, this, target: Value| -> mlua::Result<()> {
            this.ensure_alive("Teleport")?;
            let cf = cframe_from_value(&target, "Teleport")?;
            this.with_mut("Teleport", |c| {
                c.position = cf.position;
                c.rotation = cf.rotation;
                c.velocity = Vector::new(0.0, 0.0, 0.0);
                c.path.clear();
                c.path_idx = 0;
            })?;
            Ok(())
        });

        m.add_method("Destroy", |_, this, _: ()| -> mlua::Result<()> {
            let mut plane = this.plane.lock().unwrap();
            if let Some(c) = plane.controllers.get_mut(&this.id) {
                c.alive = false;
            }
            let removed = plane.controllers.remove(&this.id);
            drop(plane);
            if let Some(c) = removed {
                if let Ok(mut p) = c.part.lock() {
                    if p.alive {
                        if let Ok(g) = c.override_cell.lock() {
                            p.cframe = *g;
                        }
                        p.physics_override = None;
                    }
                }
            }
            Ok(())
        });
    }
}

fn vector_from_value(v: &Value, op: &str) -> mlua::Result<Vector> {
    match v {
        Value::UserData(ud) => {
            if let Ok(vec) = ud.borrow::<Vector>() {
                return Ok(*vec);
            }
            if let Ok(cf) = ud.borrow::<CFrame>() {
                return Ok(cf.position);
            }
            Err(mlua::Error::RuntimeError(format!(
                "{op}: expected a Vector or CFrame"
            )))
        }
        _ => Err(mlua::Error::RuntimeError(format!(
            "{op}: expected a Vector or CFrame"
        ))),
    }
}

fn cframe_from_value(v: &Value, op: &str) -> mlua::Result<CFrame> {
    match v {
        Value::UserData(ud) => {
            if let Ok(cf) = ud.borrow::<CFrame>() {
                return Ok(*cf);
            }
            if let Ok(vec) = ud.borrow::<Vector>() {
                return Ok(CFrame {
                    position: *vec,
                    rotation: Vector::new(0.0, 0.0, 0.0),
                });
            }
            Err(mlua::Error::RuntimeError(format!(
                "{op}: expected a CFrame or Vector"
            )))
        }
        _ => Err(mlua::Error::RuntimeError(format!(
            "{op}: expected a CFrame or Vector"
        ))),
    }
}

#[allow(dead_code)]
fn rapier_quat_from_yaw(yaw: f32) -> UnitQuaternion<f32> {
    UnitQuaternion::from_axis_angle(&NaVec3::y_axis(), yaw)
}

pub fn extract_part_ud(part_ud: AnyUserData) -> mlua::Result<Arc<Mutex<PartState>>> {
    let part = part_ud.borrow::<PartHandle>().map_err(|_| {
        mlua::Error::RuntimeError("NewController: expected a Renderable.BasePart".into())
    })?;
    Ok(part.state.clone())
}
