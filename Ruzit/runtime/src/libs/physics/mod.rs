
use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use mlua::{
    AnyUserData, Lua, MultiValue, Table, UserData, UserDataFields, UserDataMethods, Value,
};
use rayon::ThreadPool;

use rapier3d::na::{UnitQuaternion, Vector3 as NaVec3};
use rapier3d::prelude::*;

use crate::libs::primitives::{CFrame, Vector};
use crate::libs::renderable::{self, PartHandle, PartShape, PartState};
use crate::libs::signal;

static NEXT_PLANE_ID: AtomicU64 = AtomicU64::new(1);
static NEXT_OBJ_ID: AtomicU64 = AtomicU64::new(1);

thread_local! {
    static PLANES: RefCell<Vec<Arc<Mutex<PlaneState>>>> = const { RefCell::new(Vec::new()) };
}

pub struct PlaneState {
    pub id: u64,
    pub alive: bool,
    pub enabled: bool,
    pub gravity: Vector,
    pub objects: HashMap<u64, ObjectState>,
    pub threads: usize,
    pub pool: Option<Arc<ThreadPool>>,
    pub linear_damping: f32,
    pub angular_damping: f32,
    pub drag: f32,
    pub buoyancy: f32,
    pub rest_threshold: f32,
    pub solver_iterations: u32,
    pub loop_solver: bool,
    pub never_sleep: bool,
    pub defer_gpu: bool,
    pub gpu: Option<GpuPlaneResources>,
    pub rapier: Option<Box<RapierPlane>>,
}

pub struct RapierPlane {
    pub bodies: RigidBodySet,
    pub colliders: ColliderSet,
    pub impulse_joints: ImpulseJointSet,
    pub multibody_joints: MultibodyJointSet,
    pub island_manager: IslandManager,
    pub broad_phase: DefaultBroadPhase,
    pub narrow_phase: NarrowPhase,
    pub ccd_solver: CCDSolver,
    pub query_pipeline: QueryPipeline,
    pub pipeline: PhysicsPipeline,
    pub integration_parameters: IntegrationParameters,
}

impl RapierPlane {
    fn new() -> Self {
        Self {
            bodies: RigidBodySet::new(),
            colliders: ColliderSet::new(),
            impulse_joints: ImpulseJointSet::new(),
            multibody_joints: MultibodyJointSet::new(),
            island_manager: IslandManager::new(),
            broad_phase: DefaultBroadPhase::new(),
            narrow_phase: NarrowPhase::new(),
            ccd_solver: CCDSolver::new(),
            query_pipeline: QueryPipeline::new(),
            pipeline: PhysicsPipeline::new(),
            integration_parameters: IntegrationParameters::default(),
        }
    }
}

pub struct ObjectState {
    pub id: u64,
    pub alive: bool,
    pub part: Arc<Mutex<PartState>>,
    pub override_cell: Arc<Mutex<CFrame>>,
    pub position: Vector,
    pub velocity: Vector,
    pub rotation: Vector,
    pub angular_velocity: Vector,
    pub impulse_direction: Vector,
    pub mass: f32,
    pub bounciness: f32,
    pub friction: f32,
    pub anchored: bool,
    pub can_collide: bool,
    pub locked_axes: [bool; 3],
    pub locked_rotation: [bool; 3],
    pub rotate_target: Option<(Vector, f32)>,
    pub hitbox_quality: u32,
    pub cached_size: Vector,
    pub part_alive: bool,
    pub shape_id: u32,
    pub com_offset: Vector,
    pub density: f32,
    pub rapier_body: Option<RigidBodyHandle>,
}

pub fn create(lua: &Lua) -> mlua::Result<Table> {
    let t = lua.create_table()?;

    t.set(
        "New",
        lua.create_function(|_, opts: Option<Table>| -> mlua::Result<PlaneHandle> {
            let mut gravity = Vector::new(0.0, -50.0, 0.0);
            let mut threads: usize = 1;
            let mut linear_damping: f32 = 0.10;
            let mut angular_damping: f32 = 0.30;
            let mut drag: f32 = 0.0;
            let mut buoyancy: f32 = 0.0;
            let mut rest_threshold: f32 = 0.5;
            let mut solver_iterations: u32 = 4;
            let mut loop_solver: bool = false;
            let mut never_sleep: bool = false;
            let mut defer_gpu: bool = false;
            if let Some(opts) = opts {
                if let Ok(g) = opts.get::<Vector>("Gravity") {
                    gravity = g;
                }
                if let Ok(n) = opts.get::<i64>("Threads") {
                    threads = n.max(1) as usize;
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
                if let Ok(v) = opts.get::<f32>("Buoyancy") {
                    buoyancy = v;
                }
                if let Ok(v) = opts.get::<f32>("RestThreshold") {
                    rest_threshold = v.max(0.0);
                }
                if let Ok(v) = opts.get::<i64>("SolverIterations") {
                    solver_iterations = v.clamp(1, 64) as u32;
                }
                if let Ok(v) = opts.get::<bool>("LoopSolver") {
                    loop_solver = v;
                }
                if let Ok(v) = opts.get::<bool>("NeverSleep") {
                    never_sleep = v;
                }
                if let Ok(v) = opts.get::<bool>("DeferGpu") {
                    defer_gpu = v;
                }
            }
            let pool = if threads > 1 {
                rayon::ThreadPoolBuilder::new()
                    .num_threads(threads)
                    .thread_name(|i| format!("ruzit-physics-{i}"))
                    .build()
                    .ok()
                    .map(Arc::new)
            } else {
                None
            };
            let id = NEXT_PLANE_ID.fetch_add(1, Ordering::Relaxed);
            let state = Arc::new(Mutex::new(PlaneState {
                id,
                alive: true,
                enabled: true,
                gravity,
                objects: HashMap::new(),
                threads,
                pool,
                linear_damping,
                angular_damping,
                drag,
                buoyancy,
                rest_threshold,
                solver_iterations,
                loop_solver,
                never_sleep,
                defer_gpu,
                gpu: None,
                rapier: None,
            }));
            PLANES.with(|c| c.borrow_mut().push(state.clone()));
            Ok(PlaneHandle { state })
        })?,
    )?;

    Ok(t)
}

pub fn tick(lua: &Lua, dt: f64) {
    let dt = (dt as f32).min(1.0 / 30.0);
    let snapshot: Vec<Arc<Mutex<PlaneState>>> = PLANES.with(|c| {
        c.borrow_mut().retain(|p| p.lock().unwrap().alive);
        c.borrow().clone()
    });

    for plane_arc in &snapshot {
        let want_gpu = plane_arc.lock().unwrap().defer_gpu;
        let used_gpu = if want_gpu {
            step_plane_gpu(plane_arc, dt)
        } else {
            false
        };
        if !used_gpu {
            step_plane(plane_arc, dt);
        }
        fire_prop_signals(lua, plane_arc);
    }
}

fn fire_prop_signals(lua: &Lua, plane_arc: &Arc<Mutex<PlaneState>>) {
    let entries: Vec<(Arc<Mutex<crate::libs::renderable::PartState>>, Vector, Vector)> = {
        let plane = plane_arc.lock().unwrap();
        plane
            .objects
            .values()
            .filter(|o| o.alive && o.part_alive)
            .map(|o| (o.part.clone(), o.position, o.rotation))
            .collect()
    };
    for (part, pos, rot) in entries {
        let cf_sig = {
            let p = match part.lock() {
                Ok(p) => p,
                Err(_) => continue,
            };
            if !p.alive {
                continue;
            }
            p.prop_signals.get("CFrame").cloned()
        };
        if let Some(sig) = cf_sig {
            if renderable::signal_has_listeners(&sig) {
                let cf = CFrame::new(pos, rot);
                if let Ok(ud) = lua.create_userdata(cf) {
                    let mut args = MultiValue::new();
                    args.push_back(Value::UserData(ud));
                    let _ = signal::fire(lua, &sig, args);
                }
            }
        }
    }
}

fn step_plane(plane_arc: &Arc<Mutex<PlaneState>>, dt: f32) {
    let mut plane = plane_arc.lock().unwrap();
    if !plane.alive || !plane.enabled {
        return;
    }

    let dead: Vec<u64> = plane
        .objects
        .iter()
        .filter_map(|(k, o)| {
            if !o.alive {
                return Some(*k);
            }
            let alive = o.part.lock().map(|p| p.alive).unwrap_or(false);
            if !alive { Some(*k) } else { None }
        })
        .collect();
    for k in &dead {
        if let Some(obj) = plane.objects.remove(k) {
            if let (Some(handle), Some(rapier)) =
                (obj.rapier_body, plane.rapier.as_deref_mut())
            {
                rapier.bodies.remove(
                    handle,
                    &mut rapier.island_manager,
                    &mut rapier.colliders,
                    &mut rapier.impulse_joints,
                    &mut rapier.multibody_joints,
                    true,
                );
            }
        }
    }

    for obj in plane.objects.values_mut() {
        if let Ok(p) = obj.part.lock() {
            obj.part_alive = p.alive;
            obj.cached_size = p.size;
            obj.shape_id = match p.shape {
                PartShape::Sphere => 1,
                _ => 0,
            };
        } else {
            obj.part_alive = false;
        }
    }

    if plane.objects.is_empty() {
        return;
    }

    if plane.rapier.is_none() {
        plane.rapier = Some(Box::new(RapierPlane::new()));
    }

    let plane_gravity = plane.gravity;
    let lin_damp = plane.linear_damping;
    let ang_damp = plane.angular_damping;
    let drag_coef = plane.drag;
    let buoyancy = plane.buoyancy;
    let never_sleep = plane.never_sleep;
    let base_solver = if plane.loop_solver {
        64u32
    } else {
        plane.solver_iterations.max(1)
    };
    let max_q = plane
        .objects
        .values()
        .map(|o| o.hitbox_quality)
        .max()
        .unwrap_or(1)
        .max(1) as f32;
    let q_factor = ((max_q - 1.0) / 31.0).clamp(0.0, 1.0);
    let solver_iters = ((base_solver as f32) * (1.0 + q_factor * 1.5)).round() as u32;
    let friction_iters = (4.0 + q_factor * 12.0).round() as usize;
    let pgs_iters = (2.0 + q_factor * 6.0).round() as usize;
    let stab_iters = (2.0 + q_factor * 4.0).round() as usize;
    let allowed_lin_err = 0.001 / (1.0 + q_factor * 4.0);
    let prediction_dist = 0.002 / (1.0 + q_factor * 2.0);
    let damping_ratio = 5.0 + q_factor * 5.0;

    let ids: Vec<u64> = plane.objects.keys().copied().collect();

    {
        let plane: &mut PlaneState = &mut plane;
        let rapier = plane.rapier.as_deref_mut().unwrap();
        rapier.integration_parameters.dt = dt;
        rapier.integration_parameters.num_solver_iterations =
            std::num::NonZeroUsize::new(solver_iters as usize)
                .unwrap_or(std::num::NonZeroUsize::new(1).unwrap());
        rapier.integration_parameters.num_additional_friction_iterations = friction_iters;
        rapier.integration_parameters.num_internal_pgs_iterations = pgs_iters;
        rapier.integration_parameters.num_internal_stabilization_iterations = stab_iters;
        rapier.integration_parameters.normalized_allowed_linear_error = allowed_lin_err;
        rapier.integration_parameters.normalized_prediction_distance = prediction_dist;
        rapier.integration_parameters.contact_damping_ratio = damping_ratio;

        for id in &ids {
            let obj = match plane.objects.get_mut(id) {
                Some(o) => o,
                None => continue,
            };
            sync_obj_to_rapier(obj, rapier, lin_damp, ang_damp, drag_coef, buoyancy, never_sleep);
        }

        let gravity = NaVec3::new(plane_gravity.x, plane_gravity.y, plane_gravity.z);
        rapier.pipeline.step(
            &gravity,
            &rapier.integration_parameters,
            &mut rapier.island_manager,
            &mut rapier.broad_phase,
            &mut rapier.narrow_phase,
            &mut rapier.bodies,
            &mut rapier.colliders,
            &mut rapier.impulse_joints,
            &mut rapier.multibody_joints,
            &mut rapier.ccd_solver,
            Some(&mut rapier.query_pipeline),
            &(),
            &(),
        );

        for id in &ids {
            let obj = match plane.objects.get_mut(id) {
                Some(o) => o,
                None => continue,
            };
            sync_rapier_to_obj(obj, rapier);
            if obj.alive && obj.part_alive {
                if let Ok(mut g) = obj.override_cell.lock() {
                    *g = CFrame::new(obj.position, obj.rotation);
                }
            }
        }
    }

    renderable::bump_parts_dirty();
}

fn sync_obj_to_rapier(
    obj: &mut ObjectState,
    rapier: &mut RapierPlane,
    lin_damp: f32,
    ang_damp: f32,
    drag: f32,
    buoyancy: f32,
    never_sleep: bool,
) {
    let half = NaVec3::new(
        obj.cached_size.x.abs() * 0.5,
        obj.cached_size.y.abs() * 0.5,
        obj.cached_size.z.abs() * 0.5,
    );

    let make_collider = |obj: &ObjectState, half: NaVec3<f32>| -> Collider {
        let (mut builder, volume) = if obj.shape_id == 1 {
            let r = half.x.max(half.y).max(half.z).max(0.0001);
            let v = (4.0 / 3.0) * std::f32::consts::PI * r * r * r;
            (ColliderBuilder::ball(r), v)
        } else {
            let hx = half.x.max(0.0001);
            let hy = half.y.max(0.0001);
            let hz = half.z.max(0.0001);
            let v = (2.0 * hx) * (2.0 * hy) * (2.0 * hz);
            (ColliderBuilder::cuboid(hx, hy, hz), v)
        };
        let density = if obj.density > 0.0 {
            obj.density
        } else {
            obj.mass.max(0.0001) / volume.max(0.0001)
        };
        let q = obj.hitbox_quality.max(1) as f32;
        let skin = (0.02 / q).clamp(0.0002, 0.02);
        builder = builder
            .restitution(obj.bounciness.clamp(0.0, 1.0))
            .friction(obj.friction.clamp(0.0, 2.0))
            .density(density)
            .contact_skin(skin);
        if !obj.can_collide {
            builder = builder.collision_groups(InteractionGroups::none());
        }
        builder.build()
    };

    if obj.rapier_body.is_none() {
        let body_type = if obj.anchored {
            RigidBodyType::Fixed
        } else {
            RigidBodyType::Dynamic
        };
        let mut locked = LockedAxes::empty();
        if obj.locked_axes[0] {
            locked |= LockedAxes::TRANSLATION_LOCKED_X;
        }
        if obj.locked_axes[1] {
            locked |= LockedAxes::TRANSLATION_LOCKED_Y;
        }
        if obj.locked_axes[2] {
            locked |= LockedAxes::TRANSLATION_LOCKED_Z;
        }
        if obj.locked_rotation[0] {
            locked |= LockedAxes::ROTATION_LOCKED_X;
        }
        if obj.locked_rotation[1] {
            locked |= LockedAxes::ROTATION_LOCKED_Y;
        }
        if obj.locked_rotation[2] {
            locked |= LockedAxes::ROTATION_LOCKED_Z;
        }
        let com_present = obj.com_offset.x != 0.0
            || obj.com_offset.y != 0.0
            || obj.com_offset.z != 0.0;
        if obj.shape_id != 1 && obj.hitbox_quality < 2 && !com_present {
            locked |= LockedAxes::ROTATION_LOCKED_X
                | LockedAxes::ROTATION_LOCKED_Y
                | LockedAxes::ROTATION_LOCKED_Z;
        }

        let translation = NaVec3::new(obj.position.x, obj.position.y, obj.position.z);
        let rot = UnitQuaternion::from_euler_angles(
            obj.rotation.x,
            obj.rotation.y,
            obj.rotation.z,
        );
        let body = RigidBodyBuilder::new(body_type)
            .position(rapier3d::na::Isometry3::from_parts(
                translation.into(),
                rot,
            ))
            .linvel(NaVec3::new(obj.velocity.x, obj.velocity.y, obj.velocity.z))
            .angvel(NaVec3::new(
                obj.angular_velocity.x,
                obj.angular_velocity.y,
                obj.angular_velocity.z,
            ))
            .linear_damping(lin_damp)
            .angular_damping(ang_damp)
            .locked_axes(locked)
            .ccd_enabled(obj.hitbox_quality > 1)
            .can_sleep(!never_sleep)
            .build();
        let handle = rapier.bodies.insert(body);
        let collider = make_collider(obj, half);
        rapier
            .colliders
            .insert_with_parent(collider, handle, &mut rapier.bodies);
        obj.rapier_body = Some(handle);
        return;
    }

    let handle = obj.rapier_body.unwrap();

    let want_density = if obj.density > 0.0 {
        obj.density
    } else {
        let s = obj.cached_size;
        let volume = if obj.shape_id == 1 {
            let r = s.x.abs().max(s.y.abs()).max(s.z.abs()) * 0.5;
            (4.0 / 3.0) * std::f32::consts::PI * r * r * r
        } else {
            (s.x.abs() * s.y.abs() * s.z.abs()).max(0.0001)
        };
        obj.mass.max(0.0001) / volume.max(0.0001)
    };
    let col_handles: Vec<ColliderHandle> = match rapier.bodies.get(handle) {
        Some(b) => b.colliders().iter().copied().collect(),
        None => return,
    };
    let want_skin = (0.02 / (obj.hitbox_quality.max(1) as f32)).clamp(0.0002, 0.02);
    let mut needs_recompute = false;
    for ch in &col_handles {
        if let Some(c) = rapier.colliders.get_mut(*ch) {
            if (c.density() - want_density).abs() > 1e-4 {
                c.set_density(want_density);
                needs_recompute = true;
            }
            if (c.contact_skin() - want_skin).abs() > 1e-5 {
                c.set_contact_skin(want_skin);
            }
        }
    }

    let body = match rapier.bodies.get_mut(handle) {
        Some(b) => b,
        None => return,
    };
    if needs_recompute {
        body.recompute_mass_properties_from_colliders(&rapier.colliders);
    }
    body.set_linear_damping(lin_damp);
    body.set_angular_damping(ang_damp);

    {
        let activation = body.activation_mut();
        if never_sleep {
            activation.normalized_linear_threshold = -1.0;
            activation.angular_threshold = -1.0;
            activation.sleeping = false;
        } else {
            activation.normalized_linear_threshold =
                RigidBodyActivation::default_normalized_linear_threshold();
            activation.angular_threshold =
                RigidBodyActivation::default_angular_threshold();
        }
    }

    let want_translation = NaVec3::new(obj.position.x, obj.position.y, obj.position.z);
    let want_rotation = UnitQuaternion::from_euler_angles(
        obj.rotation.x,
        obj.rotation.y,
        obj.rotation.z,
    );
    let cur_pos = body.position();
    let pos_diff = (cur_pos.translation.vector - want_translation).norm();
    let rot_dot = cur_pos.rotation.coords.dot(&want_rotation.coords).abs();
    let rot_diff = 1.0 - rot_dot.min(1.0);
    if pos_diff > 1e-3 || rot_diff > 1e-4 {
        body.set_position(
            rapier3d::na::Isometry3::from_parts(want_translation.into(), want_rotation),
            true,
        );
    }

    let want_lv = NaVec3::new(obj.velocity.x, obj.velocity.y, obj.velocity.z);
    if (body.linvel() - want_lv).norm() > 1e-3 {
        body.set_linvel(want_lv, true);
    }
    let want_av = NaVec3::new(
        obj.angular_velocity.x,
        obj.angular_velocity.y,
        obj.angular_velocity.z,
    );
    if (body.angvel() - want_av).norm() > 1e-3 {
        body.set_angvel(want_av, true);
    }

    let mass_now = body.mass().max(0.0001);
    let mut fx = obj.impulse_direction.x * mass_now;
    let mut fy = obj.impulse_direction.y * mass_now;
    let mut fz = obj.impulse_direction.z * mass_now;

    if drag > 0.0 {
        let v = body.linvel();
        let speed = (v.x * v.x + v.y * v.y + v.z * v.z).sqrt();
        if speed > 1e-6 {
            let k = drag * speed;
            fx -= k * v.x;
            fy -= k * v.y;
            fz -= k * v.z;
        }
    }
    if buoyancy != 0.0 {
        fy += buoyancy * mass_now;
    }

    let force = NaVec3::new(fx, fy, fz);
    body.reset_forces(true);
    body.add_force(force, true);
}

fn sync_rapier_to_obj(obj: &mut ObjectState, rapier: &RapierPlane) {
    let handle = match obj.rapier_body {
        Some(h) => h,
        None => return,
    };
    let body = match rapier.bodies.get(handle) {
        Some(b) => b,
        None => return,
    };
    let pos = body.position();
    obj.position = Vector::new(
        pos.translation.vector.x,
        pos.translation.vector.y,
        pos.translation.vector.z,
    );
    let (rx, ry, rz) = pos.rotation.euler_angles();
    obj.rotation = Vector::new(rx, ry, rz);
    let lv = body.linvel();
    obj.velocity = Vector::new(lv.x, lv.y, lv.z);
    let av = body.angvel();
    obj.angular_velocity = Vector::new(av.x, av.y, av.z);
}

pub struct PlaneHandle {
    pub state: Arc<Mutex<PlaneState>>,
}

impl PlaneHandle {
    fn ensure_alive(&self, op: &str) -> mlua::Result<()> {
        if !self.state.lock().unwrap().alive {
            return Err(mlua::Error::RuntimeError(format!(
                "PhysicsPlane: {op} called on a destroyed plane"
            )));
        }
        Ok(())
    }
}

impl UserData for PlaneHandle {
    fn add_fields<F: UserDataFields<Self>>(f: &mut F) {
        f.add_field_method_get("Gravity", |_, this| {
            Ok(this.state.lock().unwrap().gravity)
        });
        f.add_field_method_set("Gravity", |_, this, v: Vector| {
            this.ensure_alive("set Gravity")?;
            this.state.lock().unwrap().gravity = v;
            Ok(())
        });
        f.add_field_method_get("Enabled", |_, this| {
            Ok(this.state.lock().unwrap().enabled)
        });
        f.add_field_method_set("Enabled", |_, this, v: bool| {
            this.ensure_alive("set Enabled")?;
            this.state.lock().unwrap().enabled = v;
            Ok(())
        });
        f.add_field_method_get("Threads", |_, this| {
            Ok(this.state.lock().unwrap().threads as i64)
        });
        f.add_field_method_get("LinearDamping", |_, this| {
            Ok(this.state.lock().unwrap().linear_damping)
        });
        f.add_field_method_set("LinearDamping", |_, this, v: f32| {
            this.ensure_alive("set LinearDamping")?;
            this.state.lock().unwrap().linear_damping = v.max(0.0);
            Ok(())
        });
        f.add_field_method_get("AngularDamping", |_, this| {
            Ok(this.state.lock().unwrap().angular_damping)
        });
        f.add_field_method_set("AngularDamping", |_, this, v: f32| {
            this.ensure_alive("set AngularDamping")?;
            this.state.lock().unwrap().angular_damping = v.max(0.0);
            Ok(())
        });
        f.add_field_method_get("Drag", |_, this| Ok(this.state.lock().unwrap().drag));
        f.add_field_method_set("Drag", |_, this, v: f32| {
            this.ensure_alive("set Drag")?;
            this.state.lock().unwrap().drag = v.max(0.0);
            Ok(())
        });
        f.add_field_method_get("Buoyancy", |_, this| Ok(this.state.lock().unwrap().buoyancy));
        f.add_field_method_set("Buoyancy", |_, this, v: f32| {
            this.ensure_alive("set Buoyancy")?;
            this.state.lock().unwrap().buoyancy = v;
            Ok(())
        });
        f.add_field_method_get("RestThreshold", |_, this| {
            Ok(this.state.lock().unwrap().rest_threshold)
        });
        f.add_field_method_set("RestThreshold", |_, this, v: f32| {
            this.ensure_alive("set RestThreshold")?;
            this.state.lock().unwrap().rest_threshold = v.max(0.0);
            Ok(())
        });
        f.add_field_method_get("SolverIterations", |_, this| {
            Ok(this.state.lock().unwrap().solver_iterations as i64)
        });
        f.add_field_method_set("SolverIterations", |_, this, v: i64| {
            this.ensure_alive("set SolverIterations")?;
            this.state.lock().unwrap().solver_iterations = v.clamp(1, 64) as u32;
            Ok(())
        });
        f.add_field_method_get("LoopSolver", |_, this| {
            Ok(this.state.lock().unwrap().loop_solver)
        });
        f.add_field_method_set("LoopSolver", |_, this, v: bool| {
            this.ensure_alive("set LoopSolver")?;
            this.state.lock().unwrap().loop_solver = v;
            Ok(())
        });
        f.add_field_method_get("NeverSleep", |_, this| {
            Ok(this.state.lock().unwrap().never_sleep)
        });
        f.add_field_method_set("NeverSleep", |_, this, v: bool| {
            this.ensure_alive("set NeverSleep")?;
            this.state.lock().unwrap().never_sleep = v;
            Ok(())
        });
        f.add_field_method_get("DeferGpu", |_, this| {
            Ok(this.state.lock().unwrap().defer_gpu)
        });
        f.add_field_method_set("DeferGpu", |_, this, v: bool| {
            this.ensure_alive("set DeferGpu")?;
            this.state.lock().unwrap().defer_gpu = v;
            Ok(())
        });
        f.add_field_method_get("Alive", |_, this| Ok(this.state.lock().unwrap().alive));
        f.add_field_method_get("ObjectCount", |_, this| {
            Ok(this.state.lock().unwrap().objects.len() as i64)
        });
    }

    fn add_methods<M: UserDataMethods<Self>>(m: &mut M) {
        m.add_method("Add", |_, this, part_ud: AnyUserData| -> mlua::Result<ObjectHandle> {
            this.ensure_alive("Add")?;
            let part = part_ud.borrow::<PartHandle>().map_err(|_| {
                mlua::Error::RuntimeError(
                    "PhysicsPlane:Add expects a Renderable.BasePart".into(),
                )
            })?;
            let part_state = part.state.clone();
            drop(part);

            let (initial_pos, initial_rot, initial_size, override_cell) = {
                let mut s = part_state.lock().unwrap();
                if !s.alive {
                    return Err(mlua::Error::RuntimeError(
                        "PhysicsPlane:Add: BasePart is destroyed".into(),
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
                (cf.position, cf.rotation, s.size, cell)
            };

            let id = NEXT_OBJ_ID.fetch_add(1, Ordering::Relaxed);
            let obj = ObjectState {
                id,
                alive: true,
                part: part_state,
                override_cell,
                position: initial_pos,
                velocity: Vector::new(0.0, 0.0, 0.0),
                rotation: initial_rot,
                angular_velocity: Vector::new(0.0, 0.0, 0.0),
                impulse_direction: Vector::new(0.0, 0.0, 0.0),
                mass: 1.0,
                bounciness: 0.0,
                friction: 0.5,
                anchored: false,
                can_collide: true,
                locked_axes: [false; 3],
                locked_rotation: [false; 3],
                rotate_target: None,
                hitbox_quality: 1,
                cached_size: initial_size,
                part_alive: true,
                shape_id: 0,
                com_offset: Vector::new(0.0, 0.0, 0.0),
                density: 0.0,
                rapier_body: None,
            };
            this.state.lock().unwrap().objects.insert(id, obj);
            Ok(ObjectHandle {
                plane: this.state.clone(),
                id,
            })
        });

        m.add_method("SetEnabled", |_, this, v: bool| {
            this.state.lock().unwrap().enabled = v;
            Ok(())
        });

        m.add_method("Step", |_, this, dt: f64| {
            if !this.state.lock().unwrap().alive {
                return Ok(());
            }
            step_plane(&this.state, (dt as f32).min(1.0 / 30.0));
            Ok(())
        });

        m.add_method("Destroy", |_, this, _: ()| {
            let mut s = this.state.lock().unwrap();
            for (_, obj) in s.objects.drain() {
                if let Ok(mut p) = obj.part.lock() {
                    if p.alive {
                        if let Ok(g) = obj.override_cell.lock() {
                            p.cframe = *g;
                        }
                        p.physics_override = None;
                    }
                }
            }
            s.rapier = None;
            s.alive = false;
            renderable::bump_parts_dirty();
            Ok(())
        });
    }
}

fn rapier_drop_body(plane: &mut PlaneState, handle: Option<RigidBodyHandle>) {
    let handle = match handle {
        Some(h) => h,
        None => return,
    };
    let rapier = match plane.rapier.as_deref_mut() {
        Some(r) => r,
        None => return,
    };
    rapier.bodies.remove(
        handle,
        &mut rapier.island_manager,
        &mut rapier.colliders,
        &mut rapier.impulse_joints,
        &mut rapier.multibody_joints,
        true,
    );
}

pub struct ObjectHandle {
    pub plane: Arc<Mutex<PlaneState>>,
    pub id: u64,
}

impl ObjectHandle {
    fn with_obj<R>(&self, op: &str, f: impl FnOnce(&ObjectState) -> R) -> mlua::Result<R> {
        let plane = self.plane.lock().unwrap();
        let obj = plane.objects.get(&self.id).ok_or_else(|| {
            mlua::Error::RuntimeError(format!(
                "PhysicsObject: {op} called on a destroyed/unlinked object"
            ))
        })?;
        Ok(f(obj))
    }

    fn with_obj_mut<R>(
        &self,
        op: &str,
        f: impl FnOnce(&mut ObjectState) -> R,
    ) -> mlua::Result<R> {
        let mut plane = self.plane.lock().unwrap();
        let obj = plane.objects.get_mut(&self.id).ok_or_else(|| {
            mlua::Error::RuntimeError(format!(
                "PhysicsObject: {op} called on a destroyed/unlinked object"
            ))
        })?;
        Ok(f(obj))
    }
}

impl UserData for ObjectHandle {
    fn add_fields<F: UserDataFields<Self>>(f: &mut F) {
        f.add_field_method_get("CFrame", |_, this| {
            this.with_obj("get CFrame", |o| CFrame::new(o.position, o.rotation))
        });
        f.add_field_method_set("CFrame", |_, this, value: AnyUserData| {
            let cf = *value.borrow::<CFrame>().map_err(|_| {
                mlua::Error::RuntimeError("PhysicsObject.CFrame expects a CFrame".into())
            })?;
            this.with_obj_mut("set CFrame", |o| {
                o.position = cf.position;
                o.rotation = cf.rotation;
            })
        });

        f.add_field_method_get("Velocity", |_, this| {
            this.with_obj("get Velocity", |o| o.velocity)
        });
        f.add_field_method_set("Velocity", |_, this, v: Vector| {
            this.with_obj_mut("set Velocity", |o| o.velocity = v)
        });

        f.add_field_method_get("AngularVelocity", |_, this| {
            this.with_obj("get AngularVelocity", |o| o.angular_velocity)
        });
        f.add_field_method_set("AngularVelocity", |_, this, v: Vector| {
            this.with_obj_mut("set AngularVelocity", |o| o.angular_velocity = v)
        });

        f.add_field_method_get("ImpulseDirection", |_, this| {
            this.with_obj("get ImpulseDirection", |o| o.impulse_direction)
        });
        f.add_field_method_set("ImpulseDirection", |_, this, v: Vector| {
            this.with_obj_mut("set ImpulseDirection", |o| o.impulse_direction = v)
        });

        f.add_field_method_get("Weight", |_, this| this.with_obj("get Weight", |o| o.mass));
        f.add_field_method_set("Weight", |_, this, v: f32| {
            this.with_obj_mut("set Weight", |o| {
                o.mass = v.max(0.0001);
                o.density = 0.0;
            })
        });

        f.add_field_method_get("Density", |_, this| {
            this.with_obj("get Density", |o| {
                if o.density > 0.0 {
                    o.density
                } else {
                    let s = o.cached_size;
                    let volume = if o.shape_id == 1 {
                        let r = s.x.abs().max(s.y.abs()).max(s.z.abs()) * 0.5;
                        (4.0 / 3.0) * std::f32::consts::PI * r * r * r
                    } else {
                        s.x.abs() * s.y.abs() * s.z.abs()
                    };
                    if volume > 1e-6 {
                        o.mass / volume
                    } else {
                        0.0
                    }
                }
            })
        });
        f.add_field_method_set("Density", |_, this, v: f32| {
            this.with_obj_mut("set Density", |o| {
                o.density = v.max(0.0);
            })
        });

        f.add_field_method_get("Bounciness", |_, this| {
            this.with_obj("get Bounciness", |o| o.bounciness)
        });
        f.add_field_method_set("Bounciness", |_, this, v: f32| {
            this.with_obj_mut("set Bounciness", |o| o.bounciness = v.clamp(0.0, 1.0))
        });

        f.add_field_method_get("Friction", |_, this| {
            this.with_obj("get Friction", |o| o.friction)
        });
        f.add_field_method_set("Friction", |_, this, v: f32| {
            this.with_obj_mut("set Friction", |o| o.friction = v.clamp(0.0, 1.0))
        });

        f.add_field_method_get("Anchored", |_, this| {
            this.with_obj("get Anchored", |o| o.anchored)
        });
        f.add_field_method_set("Anchored", |_, this, v: bool| {
            this.with_obj_mut("set Anchored", |o| {
                o.anchored = v;
                if v {
                    o.velocity = Vector::new(0.0, 0.0, 0.0);
                    o.angular_velocity = Vector::new(0.0, 0.0, 0.0);
                }
            })
        });

        f.add_field_method_get("CanCollide", |_, this| {
            this.with_obj("get CanCollide", |o| o.can_collide)
        });
        f.add_field_method_set("CanCollide", |_, this, v: bool| {
            this.with_obj_mut("set CanCollide", |o| o.can_collide = v)
        });

        f.add_field_method_get("HitBoxQuality", |_, this| {
            this.with_obj("get HitBoxQuality", |o| o.hitbox_quality as i64)
        });
        f.add_field_method_set("HitBoxQuality", |_, this, v: i64| {
            this.with_obj_mut("set HitBoxQuality", |o| {
                o.hitbox_quality = v.clamp(1, 32) as u32;
            })
        });

        f.add_field_method_get("CenterOfMass", |_, this| {
            this.with_obj("get CenterOfMass", |o| o.com_offset)
        });
        f.add_field_method_set("CenterOfMass", |_, this, v: Vector| {
            this.with_obj_mut("set CenterOfMass", |o| o.com_offset = v)
        });

        f.add_field_method_get("Size", |_, this| {
            this.with_obj("get Size", |o| {
                o.part.lock().map(|p| p.size).unwrap_or(Vector::new(1.0, 1.0, 1.0))
            })
        });
        f.add_field_method_set("Size", |lua, this, v: Vector| -> mlua::Result<()> {
            let part_arc = this.with_obj("set Size", |o| o.part.clone())?;
            let sig = {
                let mut p = part_arc.lock().unwrap();
                if !p.alive {
                    return Ok(());
                }
                p.size = v;
                p.changed_signal.clone()
            };
            renderable::bump_parts_dirty();
            let mut args = MultiValue::new();
            args.push_back(Value::String(lua.create_string("Size")?));
            let _ = signal::fire(lua, &sig, args);
            Ok(())
        });

        f.add_field_method_get("Alive", |_, this| {
            let plane = this.plane.lock().unwrap();
            Ok(plane.objects.contains_key(&this.id))
        });
    }

    fn add_methods<M: UserDataMethods<Self>>(m: &mut M) {
        m.add_method("ApplyImpulse", |_, this, v: Vector| {
            this.with_obj_mut("ApplyImpulse", |o| {
                if o.anchored {
                    return;
                }
                let m = o.mass.max(0.0001);
                if !o.locked_axes[0] {
                    o.velocity.x += v.x / m;
                }
                if !o.locked_axes[1] {
                    o.velocity.y += v.y / m;
                }
                if !o.locked_axes[2] {
                    o.velocity.z += v.z / m;
                }
            })?;
            Ok(())
        });

        m.add_method(
            "FieldRestraint",
            |_, this, opts: Table| -> mlua::Result<()> {
                this.with_obj_mut("FieldRestraint", |o| {
                    if let Ok(v) = opts.get::<bool>("LockX") {
                        o.locked_axes[0] = v;
                    }
                    if let Ok(v) = opts.get::<bool>("LockY") {
                        o.locked_axes[1] = v;
                    }
                    if let Ok(v) = opts.get::<bool>("LockZ") {
                        o.locked_axes[2] = v;
                    }
                    if let Ok(v) = opts.get::<bool>("LockRotX") {
                        o.locked_rotation[0] = v;
                    }
                    if let Ok(v) = opts.get::<bool>("LockRotY") {
                        o.locked_rotation[1] = v;
                    }
                    if let Ok(v) = opts.get::<bool>("LockRotZ") {
                        o.locked_rotation[2] = v;
                    }
                })?;
                Ok(())
            },
        );

        m.add_method(
            "RotateTo",
            |_, this, (target, strength): (Vector, Option<f32>)| -> mlua::Result<()> {
                this.with_obj_mut("RotateTo", |o| {
                    o.rotate_target = Some((target, strength.unwrap_or(8.0)));
                })?;
                Ok(())
            },
        );

        m.add_method("StopRotateTo", |_, this, _: ()| -> mlua::Result<()> {
            this.with_obj_mut("StopRotateTo", |o| o.rotate_target = None)?;
            Ok(())
        });

        m.add_method("RecalculateCenterOfMass", |_, this, _: ()| -> mlua::Result<()> {
            let part = this.with_obj("RecalculateCenterOfMass", |o| o.part.clone())?;
            let centroid = {
                let p = match part.lock() {
                    Ok(p) => p,
                    Err(_) => return Ok(()),
                };
                let model = p.deformed.as_ref().or(p.model.as_ref()).cloned();
                drop(p);
                let model = match model {
                    Some(m) => m,
                    None => return Ok(()),
                };
                if model.vertices.is_empty() {
                    return Ok(());
                }
                let mut sx = 0.0f64;
                let mut sy = 0.0f64;
                let mut sz = 0.0f64;
                for v in model.vertices.iter() {
                    sx += v.position[0] as f64;
                    sy += v.position[1] as f64;
                    sz += v.position[2] as f64;
                }
                let n = model.vertices.len() as f64;
                Vector::new((sx / n) as f32, (sy / n) as f32, (sz / n) as f32)
            };
            this.with_obj_mut("RecalculateCenterOfMass", |o| o.com_offset = centroid)?;
            Ok(())
        });

        m.add_method("BasePart", |_, this, _: ()| -> mlua::Result<PartHandle> {
            let part = this.with_obj("BasePart", |o| o.part.clone())?;
            Ok(PartHandle::from_state(part))
        });

        m.add_method("Unlink", |_, this, _: ()| -> mlua::Result<()> {
            let removed = {
                let mut plane = this.plane.lock().unwrap();
                let obj = plane.objects.remove(&this.id);
                if let Some(obj_ref) = obj.as_ref() {
                    rapier_drop_body(&mut plane, obj_ref.rapier_body);
                }
                obj
            };
            if let Some(obj) = removed {
                if let Ok(mut p) = obj.part.lock() {
                    if p.alive {
                        if let Ok(g) = obj.override_cell.lock() {
                            p.cframe = *g;
                        }
                        p.physics_override = None;
                    }
                }
            }
            Ok(())
        });

        m.add_method("Destroy", |lua, this, _: ()| -> mlua::Result<()> {
            let removed = {
                let mut plane = this.plane.lock().unwrap();
                let obj = plane.objects.remove(&this.id);
                if let Some(obj_ref) = obj.as_ref() {
                    rapier_drop_body(&mut plane, obj_ref.rapier_body);
                }
                obj
            };
            if let Some(obj) = removed {
                let sig = {
                    let mut p = obj.part.lock().unwrap();
                    if !p.alive {
                        return Ok(());
                    }
                    p.alive = false;
                    p.render = false;
                    p.attached.clear();
                    p.texture = None;
                    p.model = None;
                    p.deformed = None;
                    p.physics_override = None;
                    p.changed_signal.clone()
                };
                renderable::bump_parts_dirty();
                let mut args = MultiValue::new();
                args.push_back(Value::String(lua.create_string("Destroyed")?));
                let _ = signal::fire(lua, &sig, args);
            }
            Ok(())
        });
    }
}

use std::sync::OnceLock;

use bytemuck::{Pod, Zeroable};

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct GpuObject {
    pos: [f32; 3],
    flags: u32,
    vel: [f32; 3],
    mass: f32,
    rot: [f32; 3],
    bounciness: f32,
    omega: [f32; 3],
    target_strength: f32,
    impulse: [f32; 3],
    friction: f32,
    rot_target: [f32; 3],
    shape_id: u32,
    size: [f32; 3],
    hitbox_quality: u32,
    com_offset: [f32; 3],
    _p4: f32,
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct GpuParams {
    gravity: [f32; 3],
    dt: f32,
    lin_damp: f32,
    ang_damp: f32,
    rest_threshold: f32,
    count: u32,
}

const PHYSICS_WGSL: &str = r#"
struct GpuObject {
    pos: vec3<f32>,
    flags: u32,
    vel: vec3<f32>,
    mass: f32,
    rot: vec3<f32>,
    bounciness: f32,
    omega: vec3<f32>,
    target_strength: f32,
    impulse: vec3<f32>,
    friction: f32,
    rot_target: vec3<f32>,
    shape_id: u32,
    size: vec3<f32>,
    hitbox_quality: u32,
    com_offset: vec3<f32>,
    _p4: f32,
};

struct PhysicsParams {
    gravity: vec3<f32>,
    dt: f32,
    lin_damp: f32,
    ang_damp: f32,
    rest_threshold: f32,
    count: u32,
};

@group(0) @binding(0) var<uniform> params: PhysicsParams;
@group(0) @binding(1) var<storage, read> objects_in: array<GpuObject>;
@group(0) @binding(2) var<storage, read_write> objects_out: array<GpuObject>;

fn shape_inv_inertia(shape: u32, quality: u32, size: vec3<f32>, mass: f32, anchored: bool, com: vec3<f32>) -> vec3<f32> {
    if (anchored) { return vec3<f32>(0.0, 0.0, 0.0); }
    let m = max(mass, 0.0001);
    let s = abs(size);
    if (shape == 1u && quality > 1u) {
        let r = max(s.x, max(s.y, s.z)) * 0.5;
        let i = max(0.4 * m * r * r, 1e-6);
        return vec3<f32>(1.0 / i, 1.0 / i, 1.0 / i);
    }
    let com_present = com.x != 0.0 || com.y != 0.0 || com.z != 0.0;
    if (com_present) {
        let ixx = max(m * (s.y * s.y + s.z * s.z) / 12.0, 1e-6);
        let iyy = max(m * (s.x * s.x + s.z * s.z) / 12.0, 1e-6);
        let izz = max(m * (s.x * s.x + s.y * s.y) / 12.0, 1e-6);
        return vec3<f32>(1.0 / ixx, 1.0 / iyy, 1.0 / izz);
    }
    return vec3<f32>(0.0, 0.0, 0.0);
}

@compute @workgroup_size(64)
fn integrate(@builtin(global_invocation_id) gid: vec3<u32>) {
    let idx = gid.x;
    if (idx >= params.count) { return; }

    var o = objects_in[idx];

    let anchored = (o.flags & 1u) != 0u;
    if (anchored) {
        objects_out[idx] = o;
        return;
    }

    let lx  = (o.flags &   2u) != 0u;
    let ly  = (o.flags &   4u) != 0u;
    let lz  = (o.flags &   8u) != 0u;
    let lrx = (o.flags &  16u) != 0u;
    let lry = (o.flags &  32u) != 0u;
    let lrz = (o.flags &  64u) != 0u;
    let has_target = (o.flags & 128u) != 0u;

    let accel = params.gravity + o.impulse;
    if (!lx) { o.vel.x = o.vel.x + accel.x * params.dt; }
    if (!ly) { o.vel.y = o.vel.y + accel.y * params.dt; }
    if (!lz) { o.vel.z = o.vel.z + accel.z * params.dt; }

    let lin_factor = max(0.0, 1.0 - params.lin_damp * params.dt);
    o.vel = o.vel * lin_factor;

    if (has_target) {
        let two_pi = 6.28318530718;
        let dx = (o.rot_target.x - o.rot.x) - two_pi * round((o.rot_target.x - o.rot.x) / two_pi);
        let dy = (o.rot_target.y - o.rot.y) - two_pi * round((o.rot_target.y - o.rot.y) / two_pi);
        let dz = (o.rot_target.z - o.rot.z) - two_pi * round((o.rot_target.z - o.rot.z) / two_pi);
        o.omega = vec3<f32>(dx, dy, dz) * o.target_strength;
    } else {
        let ang_factor = max(0.0, 1.0 - params.ang_damp * params.dt);
        o.omega = o.omega * ang_factor;
    }

    if (!lx)  { o.pos.x = o.pos.x + o.vel.x * params.dt; }
    if (!ly)  { o.pos.y = o.pos.y + o.vel.y * params.dt; }
    if (!lz)  { o.pos.z = o.pos.z + o.vel.z * params.dt; }
    if (!lrx) { o.rot.x = o.rot.x + o.omega.x * params.dt; }
    if (!lry) { o.rot.y = o.rot.y + o.omega.y * params.dt; }
    if (!lrz) { o.rot.z = o.rot.z + o.omega.z * params.dt; }

    objects_out[idx] = o;
}

@compute @workgroup_size(64)
fn collide(@builtin(global_invocation_id) gid: vec3<u32>) {
    let idx = gid.x;
    if (idx >= params.count) { return; }

    var o = objects_in[idx];
    let anchored_a = (o.flags & 1u) != 0u;
    let can_collide_a = (o.flags & 256u) != 0u;
    if (!can_collide_a) {
        objects_out[idx] = o;
        return;
    }

    let half_a = abs(o.size) * 0.5;
    let inv_m_a = select(0.0, 1.0 / max(o.mass, 0.0001), !anchored_a);

    for (var j: u32 = 0u; j < params.count; j = j + 1u) {
        if (j == idx) { continue; }
        let other = objects_in[j];
        let anchored_b = (other.flags & 1u) != 0u;
        let can_collide_b = (other.flags & 256u) != 0u;
        if (!can_collide_b) { continue; }
        if (anchored_a && anchored_b) { continue; }

        let half_b = abs(other.size) * 0.5;
        let d = o.pos - other.pos;
        let overlap = (half_a + half_b) - abs(d);
        if (overlap.x <= 0.0 || overlap.y <= 0.0 || overlap.z <= 0.0) { continue; }

        var axis: u32 = 0u;
        var mtv = overlap.x;
        var sgn = sign(d.x);
        if (overlap.y < mtv) { axis = 1u; mtv = overlap.y; sgn = sign(d.y); }
        if (overlap.z < mtv) { axis = 2u; mtv = overlap.z; sgn = sign(d.z); }
        if (sgn == 0.0) { sgn = 1.0; }

        var push_a: f32 = 0.0;
        if (!anchored_a) {
            push_a = select(mtv, mtv * 0.5, !anchored_b);
        }
        if (axis == 0u)      { o.pos.x = o.pos.x + push_a * sgn; }
        else if (axis == 1u) { o.pos.y = o.pos.y + push_a * sgn; }
        else                 { o.pos.z = o.pos.z + push_a * sgn; }

        if (anchored_a) { continue; }

        var normal: vec3<f32>;
        if (axis == 0u)      { normal = vec3<f32>(sgn, 0.0, 0.0); }
        else if (axis == 1u) { normal = vec3<f32>(0.0, sgn, 0.0); }
        else                 { normal = vec3<f32>(0.0, 0.0, sgn); }

        let cmin = max(o.pos - half_a, other.pos - half_b);
        let cmax = min(o.pos + half_a, other.pos + half_b);
        var contact = (cmin + cmax) * 0.5;
        if (axis == 0u) {
            let plane_x = select(other.pos.x - half_b.x, other.pos.x + half_b.x, sgn > 0.0);
            contact.x = plane_x;
        } else if (axis == 1u) {
            let plane_y = select(other.pos.y - half_b.y, other.pos.y + half_b.y, sgn > 0.0);
            contact.y = plane_y;
        } else {
            let plane_z = select(other.pos.z - half_b.z, other.pos.z + half_b.z, sgn > 0.0);
            contact.z = plane_z;
        }

        let com_world_a = o.pos + o.com_offset;
        let com_world_b = other.pos + other.com_offset;
        let r_a = contact - com_world_a;
        let r_b = contact - com_world_b;
        let inv_m_b = select(0.0, 1.0 / max(other.mass, 0.0001), !anchored_b);
        let inv_i_a = shape_inv_inertia(o.shape_id, o.hitbox_quality, o.size, o.mass, anchored_a, o.com_offset);
        let inv_i_b = shape_inv_inertia(other.shape_id, other.hitbox_quality, other.size, other.mass, anchored_b, other.com_offset);

        let v_a_at = o.vel + cross(o.omega, r_a);
        let v_b_at = other.vel + cross(other.omega, r_b);
        let v_rel_n = dot(v_a_at - v_b_at, normal);
        if (v_rel_n >= 0.0) { continue; }

        let r_a_x_n = cross(r_a, normal);
        let r_b_x_n = cross(r_b, normal);
        let ang_a = dot(cross(r_a_x_n * inv_i_a, r_a), normal);
        let ang_b = dot(cross(r_b_x_n * inv_i_b, r_b), normal);
        let denom = inv_m_a + inv_m_b + ang_a + ang_b;
        if (denom <= 0.0) { continue; }

        let bounce_e = clamp(max(o.bounciness, other.bounciness), 0.0, 1.0);
        let restitution = select(bounce_e, 0.0, abs(v_rel_n) < params.rest_threshold);
        let j_n = -(1.0 + restitution) * v_rel_n / denom;
        let impulse = normal * j_n;

        o.vel = o.vel + impulse * inv_m_a;
        let dl = cross(r_a, impulse);
        o.omega = o.omega + dl * inv_i_a;

        let mu = sqrt(max(o.friction, 0.0) * max(other.friction, 0.0));
        if (mu > 0.0) {
            let v_rel = (o.vel + cross(o.omega, r_a)) - (other.vel + cross(other.omega, r_b));
            let v_rel_n_post = dot(v_rel, normal);
            let v_t = v_rel - normal * v_rel_n_post;
            let v_t_mag = length(v_t);
            if (v_t_mag > 1e-5) {
                let tangent = v_t / v_t_mag;
                let r_a_x_t = cross(r_a, tangent);
                let r_b_x_t = cross(r_b, tangent);
                let ang_t_a = dot(cross(r_a_x_t * inv_i_a, r_a), tangent);
                let ang_t_b = dot(cross(r_b_x_t * inv_i_b, r_b), tangent);
                let denom_t = inv_m_a + inv_m_b + ang_t_a + ang_t_b;
                if (denom_t > 0.0) {
                    let j_t_required = -v_t_mag / denom_t;
                    let max_t = mu * abs(j_n);
                    let j_t = clamp(j_t_required, -max_t, max_t);
                    let impulse_t = tangent * j_t;
                    o.vel = o.vel + impulse_t * inv_m_a;
                    let dl_t = cross(r_a, impulse_t);
                    o.omega = o.omega + dl_t * inv_i_a;
                }
            }
        }
    }

    objects_out[idx] = o;
}
"#;

struct PhysicsGpu {
    pipeline_integrate: wgpu::ComputePipeline,
    pipeline_collide: wgpu::ComputePipeline,
    bind_layout: wgpu::BindGroupLayout,
}

static PHYSICS_GPU: OnceLock<PhysicsGpu> = OnceLock::new();

fn ensure_physics_gpu() -> Option<&'static PhysicsGpu> {
    if let Some(g) = PHYSICS_GPU.get() {
        return Some(g);
    }
    let device = crate::libs::gui::render::GPU_DEVICE.get()?;

    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("ruzit physics shader"),
        source: wgpu::ShaderSource::Wgsl(PHYSICS_WGSL.into()),
    });
    let bind_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("ruzit physics bind layout"),
        entries: &[
            wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 1,
                visibility: wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Storage { read_only: true },
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 2,
                visibility: wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Storage { read_only: false },
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            },
        ],
    });
    let pl_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("ruzit physics pipeline layout"),
        bind_group_layouts: &[&bind_layout],
        push_constant_ranges: &[],
    });
    let pipeline_integrate = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: Some("ruzit physics integrate"),
        layout: Some(&pl_layout),
        module: &module,
        entry_point: "integrate",
        compilation_options: Default::default(),
        cache: None,
    });
    let pipeline_collide = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: Some("ruzit physics collide"),
        layout: Some(&pl_layout),
        module: &module,
        entry_point: "collide",
        compilation_options: Default::default(),
        cache: None,
    });

    let _ = PHYSICS_GPU.set(PhysicsGpu {
        pipeline_integrate,
        pipeline_collide,
        bind_layout,
    });
    PHYSICS_GPU.get()
}

pub struct GpuPlaneResources {
    storage_a: wgpu::Buffer,
    storage_b: wgpu::Buffer,
    params: wgpu::Buffer,
    readback: wgpu::Buffer,
    bind_ab: wgpu::BindGroup,
    bind_ba: wgpu::BindGroup,
    capacity: usize,
}

fn ensure_plane_gpu(plane: &mut PlaneState, gpu: &PhysicsGpu, needed: usize) {
    let cap = plane.gpu.as_ref().map(|g| g.capacity).unwrap_or(0);
    if cap >= needed && plane.gpu.is_some() {
        return;
    }
    let device = match crate::libs::gui::render::GPU_DEVICE.get() {
        Some(d) => d.clone(),
        None => return,
    };
    let new_cap = needed.max(cap.saturating_mul(2)).max(256);
    let storage_size = (new_cap * std::mem::size_of::<GpuObject>()) as u64;
    let storage_usage = wgpu::BufferUsages::STORAGE
        | wgpu::BufferUsages::COPY_SRC
        | wgpu::BufferUsages::COPY_DST;
    let storage_a = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("ruzit physics storage A"),
        size: storage_size,
        usage: storage_usage,
        mapped_at_creation: false,
    });
    let storage_b = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("ruzit physics storage B"),
        size: storage_size,
        usage: storage_usage,
        mapped_at_creation: false,
    });
    let params = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("ruzit physics params"),
        size: std::mem::size_of::<GpuParams>() as u64,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("ruzit physics readback"),
        size: storage_size,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let bind_ab = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("ruzit physics bind ab"),
        layout: &gpu.bind_layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: params.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: storage_a.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: storage_b.as_entire_binding(),
            },
        ],
    });
    let bind_ba = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("ruzit physics bind ba"),
        layout: &gpu.bind_layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: params.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: storage_b.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: storage_a.as_entire_binding(),
            },
        ],
    });
    plane.gpu = Some(GpuPlaneResources {
        storage_a,
        storage_b,
        params,
        readback,
        bind_ab,
        bind_ba,
        capacity: new_cap,
    });
}

fn pack_object(obj: &ObjectState) -> GpuObject {
    let mut flags: u32 = 0;
    if obj.anchored {
        flags |= 1;
    }
    if obj.locked_axes[0] {
        flags |= 2;
    }
    if obj.locked_axes[1] {
        flags |= 4;
    }
    if obj.locked_axes[2] {
        flags |= 8;
    }
    if obj.locked_rotation[0] {
        flags |= 16;
    }
    if obj.locked_rotation[1] {
        flags |= 32;
    }
    if obj.locked_rotation[2] {
        flags |= 64;
    }
    let (rot_target, target_strength) = match obj.rotate_target {
        Some((t, s)) => {
            flags |= 128;
            ([t.x, t.y, t.z], s)
        }
        None => ([0.0, 0.0, 0.0], 0.0),
    };
    if obj.can_collide {
        flags |= 256;
    }
    GpuObject {
        pos: [obj.position.x, obj.position.y, obj.position.z],
        flags,
        vel: [obj.velocity.x, obj.velocity.y, obj.velocity.z],
        mass: obj.mass.max(0.0001),
        rot: [obj.rotation.x, obj.rotation.y, obj.rotation.z],
        bounciness: obj.bounciness.clamp(0.0, 1.0),
        omega: [
            obj.angular_velocity.x,
            obj.angular_velocity.y,
            obj.angular_velocity.z,
        ],
        target_strength,
        impulse: [
            obj.impulse_direction.x,
            obj.impulse_direction.y,
            obj.impulse_direction.z,
        ],
        friction: obj.friction.clamp(0.0, 1.0),
        rot_target,
        shape_id: obj.shape_id,
        size: [obj.cached_size.x, obj.cached_size.y, obj.cached_size.z],
        hitbox_quality: obj.hitbox_quality,
        com_offset: [obj.com_offset.x, obj.com_offset.y, obj.com_offset.z],
        _p4: 0.0,
    }
}

fn step_plane_gpu(plane_arc: &Arc<Mutex<PlaneState>>, dt: f32) -> bool {
    let gpu = match ensure_physics_gpu() {
        Some(g) => g,
        None => return false,
    };
    let device = match crate::libs::gui::render::GPU_DEVICE.get() {
        Some(d) => d.clone(),
        None => return false,
    };
    let queue = match crate::libs::gui::render::GPU_QUEUE.get() {
        Some(q) => q.clone(),
        None => return false,
    };

    let mut plane = plane_arc.lock().unwrap();
    if !plane.alive || !plane.enabled {
        return true;
    }

    let dead: Vec<u64> = plane
        .objects
        .iter()
        .filter_map(|(k, o)| {
            if !o.alive {
                return Some(*k);
            }
            let alive = o.part.lock().map(|p| p.alive).unwrap_or(false);
            if !alive { Some(*k) } else { None }
        })
        .collect();
    for k in &dead {
        plane.objects.remove(k);
    }

    let mut max_quality: u32 = 1;
    for obj in plane.objects.values_mut() {
        if let Ok(p) = obj.part.lock() {
            obj.part_alive = p.alive;
            obj.cached_size = p.size;
            obj.shape_id = match p.shape {
                PartShape::Sphere => 1,
                _ => 0,
            };
        } else {
            obj.part_alive = false;
        }
        if obj.hitbox_quality > max_quality {
            max_quality = obj.hitbox_quality;
        }
    }

    let n = plane.objects.len();
    if n == 0 {
        return true;
    }

    ensure_plane_gpu(&mut plane, gpu, n);
    if plane.gpu.is_none() {
        return false;
    }

    let ids: Vec<u64> = plane.objects.keys().copied().collect();
    let sub_steps = max_quality.max(1);
    let sub_dt = dt / sub_steps as f32;
    let solver_iters = plane.solver_iterations.max(1);

    let gravity = plane.gravity;
    let lin_damp = plane.linear_damping;
    let ang_damp = plane.angular_damping;
    let rest_threshold = plane.rest_threshold;

    let mut packed: Vec<GpuObject> = Vec::with_capacity(n);
    for id in &ids {
        if let Some(obj) = plane.objects.get(id) {
            packed.push(pack_object(obj));
        }
    }
    let bytes = bytemuck::cast_slice::<GpuObject, u8>(&packed);
    {
        let g = plane.gpu.as_ref().unwrap();
        queue.write_buffer(&g.storage_a, 0, bytes);
        let params = GpuParams {
            gravity: [gravity.x, gravity.y, gravity.z],
            dt: sub_dt,
            lin_damp,
            ang_damp,
            rest_threshold,
            count: n as u32,
        };
        queue.write_buffer(&g.params, 0, bytemuck::bytes_of(&params));
    }

    let groups = (n as u32).div_ceil(64);
    let mut current_a = true;

    {
        let g = plane.gpu.as_ref().unwrap();
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("ruzit physics encoder"),
        });
        for _ in 0..sub_steps {
            {
                let bind = if current_a { &g.bind_ab } else { &g.bind_ba };
                let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                    label: Some("ruzit physics integrate pass"),
                    timestamp_writes: None,
                });
                pass.set_pipeline(&gpu.pipeline_integrate);
                pass.set_bind_group(0, bind, &[]);
                pass.dispatch_workgroups(groups, 1, 1);
            }
            current_a = !current_a;

            for _ in 0..solver_iters {
                let bind = if current_a { &g.bind_ab } else { &g.bind_ba };
                {
                    let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                        label: Some("ruzit physics collide pass"),
                        timestamp_writes: None,
                    });
                    pass.set_pipeline(&gpu.pipeline_collide);
                    pass.set_bind_group(0, bind, &[]);
                    pass.dispatch_workgroups(groups, 1, 1);
                }
                current_a = !current_a;
            }
        }
        let final_buf = if current_a {
            &g.storage_a
        } else {
            &g.storage_b
        };
        encoder.copy_buffer_to_buffer(final_buf, 0, &g.readback, 0, bytes.len() as u64);
        queue.submit(Some(encoder.finish()));
    }

    let result_bytes_len = bytes.len();
    let result_data: Vec<GpuObject> = {
        let g = plane.gpu.as_ref().unwrap();
        let slice = g.readback.slice(0..result_bytes_len as u64);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |r| {
            let _ = tx.send(r);
        });
        let _ = device.poll(wgpu::Maintain::Wait);
        let _ = rx.recv();
        let v = {
            let mapped = slice.get_mapped_range();
            let result: &[GpuObject] = bytemuck::cast_slice(&mapped);
            result.to_vec()
        };
        g.readback.unmap();
        v
    };
    for (i, id) in ids.iter().enumerate() {
        if let Some(obj) = plane.objects.get_mut(id) {
            if let Some(r) = result_data.get(i) {
                obj.position = Vector::new(r.pos[0], r.pos[1], r.pos[2]);
                obj.velocity = Vector::new(r.vel[0], r.vel[1], r.vel[2]);
                obj.rotation = Vector::new(r.rot[0], r.rot[1], r.rot[2]);
                obj.angular_velocity = Vector::new(r.omega[0], r.omega[1], r.omega[2]);
            }
        }
    }

    for obj in plane.objects.values() {
        if !obj.alive || !obj.part_alive {
            continue;
        }
        if let Ok(mut g) = obj.override_cell.lock() {
            *g = CFrame::new(obj.position, obj.rotation);
        }
    }
    renderable::bump_parts_dirty();
    true
}
