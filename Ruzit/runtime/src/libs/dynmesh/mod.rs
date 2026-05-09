use std::cell::RefCell;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use mlua::{AnyUserData, Lua, Table, UserData, UserDataMethods, Value};

use crate::libs::primitives::{CFrame, Vector};
use crate::libs::renderable::{PartHandle, PartState};

static NEXT_DYNMESH_ID: AtomicU64 = AtomicU64::new(1);
static NEXT_WELD_ID: AtomicU64 = AtomicU64::new(1);

thread_local! {
    static DYNMESH_REGISTRY: RefCell<Vec<Arc<Mutex<DynMeshState>>>> = const {
        RefCell::new(Vec::new())
    };
}

pub fn create(lua: &Lua) -> mlua::Result<Table> {
    let t = lua.create_table()?;
    t.set(
        "new",
        lua.create_function(|_, base: AnyUserData| -> mlua::Result<DynMeshHandle> {
            let handle = base.borrow::<PartHandle>().map_err(|_| {
                mlua::Error::RuntimeError(
                    "DynMesh.new: expected a BasePart (Renderable.BasePart / Renderable.BaseModel)"
                        .into(),
                )
            })?;
            DynMeshHandle::create(handle.state.clone())
        })?,
    )?;
    Ok(t)
}

pub struct DynMeshState {
    #[allow(dead_code)]
    pub id: u64,
    pub base: Arc<Mutex<PartState>>,
    pub welds: Vec<Weld>,
    pub alive: bool,
}

pub struct Weld {
    pub id: u64,
    pub name: Option<String>,
    pub target: Arc<Mutex<PartState>>,
    pub kind: WeldKind,
    pub original_size_x: f32,
    pub joint_state: Mutex<JointState>,
}

#[derive(Default, Clone, Copy)]
pub struct JointState {
    pub last_parent_cframe: Option<CFrame>,
}

pub enum WeldKind {
    Static {
        offset_pos: Vector,
        offset_rot: Vector,
    },

    Joint {
        offset_pos: Vector,
        offset_rot: Vector,
    },

    Vertex {
        vertex_index: usize,
    },

    Stretch {
        anchors: Vec<Anchor>,
    },
}

#[derive(Clone, Copy)]
pub enum Anchor {
    Local(Vector),
    Vertex(usize),
}

pub struct DynMeshHandle {
    pub inner: Arc<Mutex<DynMeshState>>,
}

impl DynMeshHandle {
    fn create(base: Arc<Mutex<PartState>>) -> mlua::Result<Self> {
        let id = NEXT_DYNMESH_ID.fetch_add(1, Ordering::Relaxed);
        let state = Arc::new(Mutex::new(DynMeshState {
            id,
            base,
            welds: Vec::new(),
            alive: true,
        }));
        DYNMESH_REGISTRY.with(|c| {
            let mut reg = c.borrow_mut();
            reg.retain(|s| s.lock().unwrap().alive);
            reg.push(state.clone());
        });
        Ok(Self { inner: state })
    }
}

impl UserData for DynMeshHandle {
    fn add_methods<M: UserDataMethods<Self>>(m: &mut M) {
        m.add_method(
            "Weld",
            |_, this, (target, opts): (AnyUserData, Option<Table>)| -> mlua::Result<i64> {
                let target_handle = target.borrow::<PartHandle>().map_err(|_| {
                    mlua::Error::RuntimeError(
                        "DynMesh:Weld: first argument must be a BasePart".into(),
                    )
                })?;
                let target_state = target_handle.state.clone();
                let original_size_x = target_state.lock().unwrap().size.x;

                let (kind, name) = parse_weld_options(opts.as_ref())?;

                let weld_id = NEXT_WELD_ID.fetch_add(1, Ordering::Relaxed);
                let mut s = this.inner.lock().unwrap();
                if !s.alive {
                    return Err(mlua::Error::RuntimeError(
                        "DynMesh:Weld: this DynMesh has been destroyed".into(),
                    ));
                }
                s.welds.push(Weld {
                    id: weld_id,
                    name,
                    target: target_state,
                    kind,
                    original_size_x,
                    joint_state: Mutex::new(JointState::default()),
                });
                Ok(weld_id as i64)
            },
        );

        m.add_method(
            "Stretch",
            |_,
             this,
             (target, anchors_table): (AnyUserData, Table)|
             -> mlua::Result<i64> {
                let target_handle = target.borrow::<PartHandle>().map_err(|_| {
                    mlua::Error::RuntimeError(
                        "DynMesh:Stretch: first argument must be a BasePart".into(),
                    )
                })?;
                let target_state = target_handle.state.clone();
                let original_size_x = target_state.lock().unwrap().size.x;

                let mut anchors: Vec<Anchor> = Vec::new();
                for pair in anchors_table.pairs::<i64, Table>() {
                    let (_, t) = pair?;
                    if let Some(idx) = t.get::<Option<i64>>("vertex")? {
                        anchors.push(Anchor::Vertex(idx.max(0) as usize));
                    } else if let Some(v) = t.get::<Option<AnyUserData>>("point")? {
                        let pos = *v.borrow::<Vector>().map_err(|_| {
                            mlua::Error::RuntimeError(
                                "DynMesh:Stretch: 'point' must be a Vector".into(),
                            )
                        })?;
                        anchors.push(Anchor::Local(pos));
                    } else {
                        return Err(mlua::Error::RuntimeError(
                            "DynMesh:Stretch: each anchor needs 'point' (Vector) or 'vertex' (number)"
                                .into(),
                        ));
                    }
                }
                if anchors.len() < 2 {
                    return Err(mlua::Error::RuntimeError(
                        "DynMesh:Stretch: need at least 2 anchors".into(),
                    ));
                }

                let weld_id = NEXT_WELD_ID.fetch_add(1, Ordering::Relaxed);
                let mut s = this.inner.lock().unwrap();
                if !s.alive {
                    return Err(mlua::Error::RuntimeError(
                        "DynMesh:Stretch: this DynMesh has been destroyed".into(),
                    ));
                }
                s.welds.push(Weld {
                    id: weld_id,
                    name: None,
                    target: target_state,
                    kind: WeldKind::Stretch { anchors },
                    original_size_x,
                    joint_state: Mutex::new(JointState::default()),
                });
                Ok(weld_id as i64)
            },
        );

        m.add_method("Detach", |_, this, weld_id: i64| -> mlua::Result<bool> {
            let mut s = this.inner.lock().unwrap();
            detach_welds(&mut s.welds, |w| w.id as i64 == weld_id)
        });

        m.add_method(
            "GetPart",
            |lua, this, weld_id: i64| -> mlua::Result<Value> {
                let s = this.inner.lock().unwrap();
                if let Some(w) = s.welds.iter().find(|w| w.id as i64 == weld_id) {
                    let h = PartHandle::from_state(w.target.clone());
                    Ok(Value::UserData(lua.create_userdata(h)?))
                } else {
                    Ok(Value::Nil)
                }
            },
        );

        m.add_method("GetParts", |lua, this, _: ()| -> mlua::Result<Table> {
            let s = this.inner.lock().unwrap();
            let out = lua.create_table()?;
            for (i, w) in s.welds.iter().enumerate() {
                let h = PartHandle::from_state(w.target.clone());
                let entry = lua.create_table()?;
                entry.set("Id", w.id as i64)?;
                if let Some(n) = &w.name {
                    entry.set("Name", n.clone())?;
                }
                entry.set("Part", lua.create_userdata(h)?)?;
                out.set(i + 1, entry)?;
            }
            Ok(out)
        });

        m.add_method(
            "GetPartByName",
            |lua, this, name: String| -> mlua::Result<Value> {
                let s = this.inner.lock().unwrap();
                if let Some(w) = s
                    .welds
                    .iter()
                    .find(|w| w.name.as_deref() == Some(name.as_str()))
                {
                    let h = PartHandle::from_state(w.target.clone());
                    Ok(Value::UserData(lua.create_userdata(h)?))
                } else {
                    Ok(Value::Nil)
                }
            },
        );

        m.add_method(
            "DetachByName",
            |_, this, name: String| -> mlua::Result<bool> {
                let mut s = this.inner.lock().unwrap();
                detach_welds(&mut s.welds, |w| w.name.as_deref() == Some(name.as_str()))
            },
        );

        m.add_method(
            "DetachPart",
            |_, this, target: AnyUserData| -> mlua::Result<bool> {
                let handle = target.borrow::<PartHandle>().map_err(|_| {
                    mlua::Error::RuntimeError(
                        "DynMesh:DetachPart: argument must be a BasePart".into(),
                    )
                })?;
                let target_ptr = Arc::as_ptr(&handle.state) as usize;
                let mut s = this.inner.lock().unwrap();
                detach_welds(&mut s.welds, |w| {
                    Arc::as_ptr(&w.target) as usize == target_ptr
                })
            },
        );

        m.add_method("Clear", |_, this, _: ()| -> mlua::Result<i64> {
            let mut s = this.inner.lock().unwrap();
            let mut welds = std::mem::take(&mut s.welds);
            let count = welds.len() as i64;

            for w in welds.iter_mut() {
                restore_weld_state(w);
            }
            Ok(count)
        });

        m.add_method("GetBase", |lua, this, _: ()| -> mlua::Result<AnyUserData> {
            let s = this.inner.lock().unwrap();
            let h = PartHandle::from_state(s.base.clone());
            lua.create_userdata(h)
        });

        m.add_method("IsAlive", |_, this, _: ()| -> mlua::Result<bool> {
            let s = this.inner.lock().unwrap();
            Ok(s.alive && s.base.lock().unwrap().alive)
        });

        m.add_method("Destroy", |_, this, _: ()| -> mlua::Result<()> {
            let mut s = this.inner.lock().unwrap();
            s.alive = false;
            s.welds.clear();
            Ok(())
        });
    }
}

fn detach_welds(welds: &mut Vec<Weld>, mut pred: impl FnMut(&Weld) -> bool) -> mlua::Result<bool> {
    let mut removed_any = false;
    let mut i = 0;
    while i < welds.len() {
        if pred(&welds[i]) {
            let mut w = welds.remove(i);
            restore_weld_state(&mut w);
            removed_any = true;
        } else {
            i += 1;
        }
    }
    Ok(removed_any)
}

fn restore_weld_state(weld: &mut Weld) {
    match &weld.kind {
        WeldKind::Stretch { .. } => {
            let mut t = weld.target.lock().unwrap();
            if t.alive && weld.original_size_x > 0.0 {
                t.size = Vector::new(weld.original_size_x, t.size.y, t.size.z);
            }
        }
        WeldKind::Joint { .. } => {
            *weld.joint_state.lock().unwrap() = JointState::default();
        }
        _ => {}
    }
}

fn parse_weld_options(opts: Option<&Table>) -> mlua::Result<(WeldKind, Option<String>)> {
    let Some(t) = opts else {
        return Ok((
            WeldKind::Static {
                offset_pos: Vector::new(0.0, 0.0, 0.0),
                offset_rot: Vector::new(0.0, 0.0, 0.0),
            },
            None,
        ));
    };
    let name = t.get::<Option<String>>("name")?;
    if let Some(idx) = t.get::<Option<i64>>("vertex")? {
        return Ok((
            WeldKind::Vertex {
                vertex_index: idx.max(0) as usize,
            },
            name,
        ));
    }
    let pos = match t.get::<Option<AnyUserData>>("point")? {
        Some(v) => *v.borrow::<Vector>().map_err(|_| {
            mlua::Error::RuntimeError("DynMesh:Weld: 'point' must be a Vector".into())
        })?,
        None => Vector::new(0.0, 0.0, 0.0),
    };
    let rot = match t.get::<Option<AnyUserData>>("rotation")? {
        Some(v) => *v.borrow::<Vector>().map_err(|_| {
            mlua::Error::RuntimeError("DynMesh:Weld: 'rotation' must be a Vector".into())
        })?,
        None => Vector::new(0.0, 0.0, 0.0),
    };
    let kind_str = t
        .get::<Option<String>>("kind")?
        .unwrap_or_else(|| "Static".to_string());
    let kind = match kind_str.as_str() {
        "Static" | "static" | "Attach" | "attach" | "Rigid" | "rigid" => WeldKind::Static {
            offset_pos: pos,
            offset_rot: rot,
        },
        "Joint" | "joint" | "Hierarchy" | "hierarchy" => WeldKind::Joint {
            offset_pos: pos,
            offset_rot: rot,
        },
        other => {
            return Err(mlua::Error::RuntimeError(format!(
                "DynMesh:Weld: unknown kind '{other}' (try 'Static' / 'Attach' / 'Joint')"
            )));
        }
    };
    Ok((kind, name))
}

pub fn tick() {
    let snapshot: Vec<Arc<Mutex<DynMeshState>>> = DYNMESH_REGISTRY.with(|c| {
        let mut reg = c.borrow_mut();
        reg.retain(|s| {
            let st = s.lock().unwrap();
            if !st.alive {
                return false;
            }

            if !st.base.lock().unwrap().alive {
                drop(st);
                let mut st = s.lock().unwrap();
                st.alive = false;
                st.welds.clear();
                return false;
            }
            true
        });
        reg.iter().cloned().collect()
    });

    for state_arc in snapshot {
        {
            let mut s = state_arc.lock().unwrap();
            s.welds.retain(|w| w.target.lock().unwrap().alive);
        }
        let s = state_arc.lock().unwrap();
        if !s.alive {
            continue;
        }
        let (base_cf, base_size, base_alive, base_world_verts) = {
            let bs = s.base.lock().unwrap();
            if !bs.alive {
                continue;
            }
            (
                bs.cframe,
                bs.size,
                true,
                cached_world_vertices_if_needed(&bs, has_vertex_anchored_weld(&s)),
            )
        };
        if !base_alive {
            continue;
        }
        for weld in &s.welds {
            match &weld.kind {
                WeldKind::Static {
                    offset_pos,
                    offset_rot,
                } => apply_static_weld(&weld.target, base_cf, *offset_pos, *offset_rot),
                WeldKind::Joint {
                    offset_pos,
                    offset_rot,
                } => apply_joint_weld(weld, base_cf, *offset_pos, *offset_rot),
                WeldKind::Vertex { vertex_index } => {
                    if let Some(world_verts) = &base_world_verts {
                        if let Some(world_pos) = world_verts.get(*vertex_index) {
                            apply_static_weld(
                                &weld.target,
                                CFrame::new(*world_pos, base_cf.rotation),
                                Vector::new(0.0, 0.0, 0.0),
                                Vector::new(0.0, 0.0, 0.0),
                            );
                        }
                    } else {
                        apply_static_weld(
                            &weld.target,
                            base_cf,
                            Vector::new(0.0, 0.0, 0.0),
                            Vector::new(0.0, 0.0, 0.0),
                        );
                    }
                }
                WeldKind::Stretch { anchors } => {
                    let world_pts: Vec<Vector> = anchors
                        .iter()
                        .map(|a| anchor_to_world(*a, base_cf, base_size, &base_world_verts))
                        .collect();
                    apply_stretch_weld(&weld.target, &world_pts, weld.original_size_x);
                }
            }
        }
    }
}

fn has_vertex_anchored_weld(s: &DynMeshState) -> bool {
    s.welds.iter().any(|w| match &w.kind {
        WeldKind::Vertex { .. } => true,
        WeldKind::Stretch { anchors } => anchors.iter().any(|a| matches!(a, Anchor::Vertex(_))),
        _ => false,
    })
}

fn cached_world_vertices_if_needed(bs: &PartState, needed: bool) -> Option<Vec<Vector>> {
    if !needed {
        return None;
    }
    let model = bs.deformed.as_ref().or(bs.model.as_ref())?;
    if model.vertices.is_empty() {
        return None;
    }
    let rot = euler_to_matrix(bs.cframe.rotation);
    let mut out = Vec::with_capacity(model.vertices.len());
    for v in model.vertices.iter() {
        let local = Vector::new(
            v.position[0] * bs.size.x,
            v.position[1] * bs.size.y,
            v.position[2] * bs.size.z,
        );
        let r = mat3_apply(rot, local);
        out.push(Vector::new(
            bs.cframe.position.x + r.x,
            bs.cframe.position.y + r.y,
            bs.cframe.position.z + r.z,
        ));
    }
    Some(out)
}

fn anchor_to_world(
    a: Anchor,
    base_cf: CFrame,
    base_size: Vector,
    world_verts: &Option<Vec<Vector>>,
) -> Vector {
    match a {
        Anchor::Local(p) => {
            let rot = euler_to_matrix(base_cf.rotation);
            let scaled = Vector::new(p.x * base_size.x, p.y * base_size.y, p.z * base_size.z);
            let r = mat3_apply(rot, scaled);
            Vector::new(
                base_cf.position.x + r.x,
                base_cf.position.y + r.y,
                base_cf.position.z + r.z,
            )
        }
        Anchor::Vertex(idx) => match world_verts.as_ref().and_then(|v| v.get(idx)) {
            Some(p) => *p,
            None => base_cf.position,
        },
    }
}

fn apply_static_weld(
    target: &Arc<Mutex<PartState>>,
    base_cf: CFrame,
    offset_pos: Vector,
    offset_rot: Vector,
) {
    let mut t = target.lock().unwrap();
    if !t.alive {
        return;
    }
    let rot = euler_to_matrix(base_cf.rotation);
    let r = mat3_apply(rot, offset_pos);
    t.cframe = CFrame::new(
        Vector::new(
            base_cf.position.x + r.x,
            base_cf.position.y + r.y,
            base_cf.position.z + r.z,
        ),
        Vector::new(
            base_cf.rotation.x + offset_rot.x,
            base_cf.rotation.y + offset_rot.y,
            base_cf.rotation.z + offset_rot.z,
        ),
    );
}

fn apply_joint_weld(weld: &Weld, base_cf: CFrame, offset_pos: Vector, offset_rot: Vector) {
    let mut joint = weld.joint_state.lock().unwrap();
    let last_parent = joint.last_parent_cframe;
    joint.last_parent_cframe = Some(base_cf);
    drop(joint);

    let mut t = weld.target.lock().unwrap();
    if !t.alive {
        return;
    }

    match last_parent {
        None => {
            let rot = euler_to_matrix(base_cf.rotation);
            let r = mat3_apply(rot, offset_pos);
            t.cframe = CFrame::new(
                Vector::new(
                    base_cf.position.x + r.x,
                    base_cf.position.y + r.y,
                    base_cf.position.z + r.z,
                ),
                Vector::new(
                    base_cf.rotation.x + offset_rot.x,
                    base_cf.rotation.y + offset_rot.y,
                    base_cf.rotation.z + offset_rot.z,
                ),
            );
        }
        Some(prev) => {
            let dx = base_cf.position.x - prev.position.x;
            let dy = base_cf.position.y - prev.position.y;
            let dz = base_cf.position.z - prev.position.z;
            let drx = base_cf.rotation.x - prev.rotation.x;
            let dry = base_cf.rotation.y - prev.rotation.y;
            let drz = base_cf.rotation.z - prev.rotation.z;
            t.cframe = CFrame::new(
                Vector::new(
                    t.cframe.position.x + dx,
                    t.cframe.position.y + dy,
                    t.cframe.position.z + dz,
                ),
                Vector::new(
                    t.cframe.rotation.x + drx,
                    t.cframe.rotation.y + dry,
                    t.cframe.rotation.z + drz,
                ),
            );
        }
    }
    let _ = offset_rot;
}

type Mat3 = [[f32; 3]; 3];

fn euler_to_matrix(rot: Vector) -> Mat3 {
    let (sx, cx) = rot.x.sin_cos();
    let (sy, cy) = rot.y.sin_cos();
    let (sz, cz) = rot.z.sin_cos();
    [
        [cy * cz, -cy * sz, sy],
        [sx * sy * cz + cx * sz, -sx * sy * sz + cx * cz, -sx * cy],
        [-cx * sy * cz + sx * sz, cx * sy * sz + sx * cz, cx * cy],
    ]
}

fn mat3_apply(m: Mat3, v: Vector) -> Vector {
    Vector::new(
        m[0][0] * v.x + m[0][1] * v.y + m[0][2] * v.z,
        m[1][0] * v.x + m[1][1] * v.y + m[1][2] * v.z,
        m[2][0] * v.x + m[2][1] * v.y + m[2][2] * v.z,
    )
}

fn apply_stretch_weld(
    target: &Arc<Mutex<PartState>>,
    world_anchors: &[Vector],
    original_size_x: f32,
) {
    if world_anchors.len() < 2 {
        return;
    }
    let a = world_anchors[0];
    let b = world_anchors[world_anchors.len() - 1];
    let mid = Vector::new((a.x + b.x) * 0.5, (a.y + b.y) * 0.5, (a.z + b.z) * 0.5);
    let dx = b.x - a.x;
    let dy = b.y - a.y;
    let dz = b.z - a.z;
    let len = (dx * dx + dy * dy + dz * dz).sqrt().max(1e-6);
    let yaw = dz.atan2(dx);
    let pitch = (-dy / len).asin();

    let mut t = target.lock().unwrap();
    if !t.alive {
        return;
    }
    t.cframe = CFrame::new(mid, Vector::new(0.0, yaw, pitch));
    let new_x = if original_size_x > 1e-4 {
        len
    } else {
        len.max(0.01)
    };
    t.size = Vector::new(new_x, t.size.y, t.size.z);
}
