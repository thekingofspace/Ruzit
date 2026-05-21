use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use mlua::{
    AnyUserData, Lua, Table, UserData, UserDataFields, UserDataMethods, Value,
};

use crate::libs::primitives::{Color3, Dim};
use crate::libs::signal;

use super::{AttachedShader, bump_dirty, fire_changed};

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

thread_local! {
    pub(crate) static SPLINE_REGISTRY: RefCell<Vec<Arc<Mutex<SplineState>>>> =
        const { RefCell::new(Vec::new()) };
}

#[derive(Clone, Copy, Debug)]
pub struct SplineNode {
    pub position: Dim,
    pub thickness: f32,
    pub angle: f32,
}

impl Default for SplineNode {
    fn default() -> Self {
        Self {
            position: Dim::new(0.0, 0.0),
            thickness: 4.0,
            angle: 0.0,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SplineStyle {
    Solid,
    Dashed,
    Dotted,
}

impl SplineStyle {
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "Solid" | "solid" => Some(Self::Solid),
            "Dashed" | "dashed" | "dash" => Some(Self::Dashed),
            "Dotted" | "dotted" | "dot" => Some(Self::Dotted),
            _ => None,
        }
    }
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Solid => "Solid",
            Self::Dashed => "Dashed",
            Self::Dotted => "Dotted",
        }
    }
    pub fn id(&self) -> u32 {
        match self {
            Self::Solid => 0,
            Self::Dashed => 1,
            Self::Dotted => 2,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SplineCap {
    Rounded,
    Miter,
    Square,
}

impl SplineCap {
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "Rounded" | "rounded" | "round" => Some(Self::Rounded),
            "Miter" | "miter" => Some(Self::Miter),
            "Square" | "square" | "butt" | "flat" => Some(Self::Square),
            _ => None,
        }
    }
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Rounded => "Rounded",
            Self::Miter => "Miter",
            Self::Square => "Square",
        }
    }
}

pub struct SplineState {
    pub id: u64,
    pub nodes: Vec<SplineNode>,
    pub color: Color3,
    pub transparency: f32,
    pub z_index: i32,
    pub style: SplineStyle,
    pub cap: SplineCap,
    pub visible: bool,
    pub alive: bool,
    pub attached: Vec<AttachedShader>,
    pub changed_signal: Table,
    pub prop_signals: HashMap<String, Table>,
}

pub struct Spline {
    pub state: Arc<Mutex<SplineState>>,
}

impl Spline {
    pub fn new(lua: &Lua) -> mlua::Result<Self> {
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        let changed_signal = signal::new_instance(lua)?;
        let state = Arc::new(Mutex::new(SplineState {
            id,
            nodes: Vec::new(),
            color: Color3::new(1.0, 1.0, 1.0),
            transparency: 0.0,
            z_index: 0,
            style: SplineStyle::Solid,
            cap: SplineCap::Rounded,
            visible: true,
            alive: true,
            attached: Vec::new(),
            changed_signal,
            prop_signals: HashMap::new(),
        }));
        SPLINE_REGISTRY.with(|c| c.borrow_mut().push(state.clone()));
        bump_dirty();
        Ok(Self { state })
    }

    pub fn state_arc(&self) -> Arc<Mutex<SplineState>> {
        self.state.clone()
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct SplineVertex {
    pub position: [f32; 2],
    pub uv: [f32; 2],
}

pub struct SplineRender {
    pub id: u64,
    pub vertices: Vec<SplineVertex>,
    pub color: Color3,
    pub transparency: f32,
    pub z_index: i32,
    pub active_shader: Option<AttachedShader>,
    pub aabb: (f32, f32, f32, f32),
    pub style: SplineStyle,
}

pub fn snapshot() -> Vec<SplineRender> {
    SPLINE_REGISTRY.with(|cell| {
        let mut reg = cell.borrow_mut();
        reg.retain(|p| p.lock().unwrap().alive);
        reg.iter()
            .filter_map(|p| {
                let s = p.lock().unwrap();
                if !s.visible || s.nodes.len() < 2 {
                    return None;
                }
                let vertices = generate_geometry(&s.nodes, s.cap);
                if vertices.is_empty() {
                    return None;
                }
                let aabb = compute_aabb(&s.nodes);
                Some(SplineRender {
                    id: s.id,
                    vertices,
                    color: s.color,
                    transparency: s.transparency,
                    z_index: s.z_index,
                    active_shader: s.attached.last().cloned(),
                    aabb,
                    style: s.style,
                })
            })
            .collect()
    })
}

fn compute_aabb(nodes: &[SplineNode]) -> (f32, f32, f32, f32) {
    let mut min_x = f32::INFINITY;
    let mut min_y = f32::INFINITY;
    let mut max_x = f32::NEG_INFINITY;
    let mut max_y = f32::NEG_INFINITY;
    let mut max_t = 0.0_f32;
    for n in nodes {
        if n.position.x < min_x {
            min_x = n.position.x;
        }
        if n.position.y < min_y {
            min_y = n.position.y;
        }
        if n.position.x > max_x {
            max_x = n.position.x;
        }
        if n.position.y > max_y {
            max_y = n.position.y;
        }
        if n.thickness > max_t {
            max_t = n.thickness;
        }
    }
    let pad = max_t * 0.5 + 1.0;
    (
        min_x - pad,
        min_y - pad,
        max_x - min_x + pad * 2.0,
        max_y - min_y + pad * 2.0,
    )
}

fn perp(v: (f32, f32)) -> (f32, f32) {
    (-v.1, v.0)
}

fn norm(v: (f32, f32)) -> (f32, f32) {
    let l = (v.0 * v.0 + v.1 * v.1).sqrt();
    if l < 1e-6 {
        (0.0, 0.0)
    } else {
        (v.0 / l, v.1 / l)
    }
}

fn rotate(v: (f32, f32), rad: f32) -> (f32, f32) {
    let c = rad.cos();
    let s = rad.sin();
    (v.0 * c - v.1 * s, v.0 * s + v.1 * c)
}

fn generate_geometry(nodes: &[SplineNode], cap: SplineCap) -> Vec<SplineVertex> {
    let n = nodes.len();
    if n < 2 {
        return Vec::new();
    }
    let mut left_edges: Vec<(f32, f32)> = Vec::with_capacity(n);
    let mut right_edges: Vec<(f32, f32)> = Vec::with_capacity(n);

    for i in 0..n {
        let pos = (nodes[i].position.x, nodes[i].position.y);
        let half = nodes[i].thickness * 0.5;
        let incoming = if i > 0 {
            let p = (nodes[i - 1].position.x, nodes[i - 1].position.y);
            norm((pos.0 - p.0, pos.1 - p.1))
        } else {
            let np = (nodes[i + 1].position.x, nodes[i + 1].position.y);
            norm((np.0 - pos.0, np.1 - pos.1))
        };
        let outgoing = if i < n - 1 {
            let np = (nodes[i + 1].position.x, nodes[i + 1].position.y);
            norm((np.0 - pos.0, np.1 - pos.1))
        } else {
            incoming
        };
        let avg = norm((
            (incoming.0 + outgoing.0) * 0.5,
            (incoming.1 + outgoing.1) * 0.5,
        ));
        let mut p = perp(avg);
        let node_rot = nodes[i].angle.to_radians();
        if node_rot.abs() > 1e-5 {
            p = rotate(p, node_rot);
        }
        let dot = p.0 * perp(outgoing).0 + p.1 * perp(outgoing).1;
        let miter_scale = if dot.abs() < 0.05 {
            1.0
        } else {
            (1.0 / dot).clamp(0.5, 4.0)
        };
        let ext = half * miter_scale;
        left_edges.push((pos.0 - p.0 * ext, pos.1 - p.1 * ext));
        right_edges.push((pos.0 + p.0 * ext, pos.1 + p.1 * ext));
    }

    let mut out: Vec<SplineVertex> = Vec::with_capacity((n - 1) * 6 + 32);
    let total = (n - 1) as f32;
    for i in 0..n - 1 {
        let u0 = i as f32 / total;
        let u1 = (i + 1) as f32 / total;
        let l0 = left_edges[i];
        let r0 = right_edges[i];
        let l1 = left_edges[i + 1];
        let r1 = right_edges[i + 1];
        out.push(SplineVertex {
            position: [l0.0, l0.1],
            uv: [u0, 0.0],
        });
        out.push(SplineVertex {
            position: [r0.0, r0.1],
            uv: [u0, 1.0],
        });
        out.push(SplineVertex {
            position: [r1.0, r1.1],
            uv: [u1, 1.0],
        });
        out.push(SplineVertex {
            position: [l0.0, l0.1],
            uv: [u0, 0.0],
        });
        out.push(SplineVertex {
            position: [r1.0, r1.1],
            uv: [u1, 1.0],
        });
        out.push(SplineVertex {
            position: [l1.0, l1.1],
            uv: [u1, 0.0],
        });
    }

    match cap {
        SplineCap::Rounded => {
            push_round_cap(&mut out, &nodes[0], &nodes[1], true);
            push_round_cap(&mut out, &nodes[n - 1], &nodes[n - 2], false);
        }
        SplineCap::Square => {
            push_square_cap(&mut out, &nodes[0], &nodes[1], true);
            push_square_cap(&mut out, &nodes[n - 1], &nodes[n - 2], false);
        }
        SplineCap::Miter => {}
    }

    out
}

fn push_round_cap(
    out: &mut Vec<SplineVertex>,
    here: &SplineNode,
    neighbor: &SplineNode,
    is_start: bool,
) {
    let pos = (here.position.x, here.position.y);
    let nbr = (neighbor.position.x, neighbor.position.y);
    let dir = if is_start {
        norm((pos.0 - nbr.0, pos.1 - nbr.1))
    } else {
        norm((pos.0 - nbr.0, pos.1 - nbr.1))
    };
    let half = here.thickness * 0.5;
    let segments = 8;
    let pi = std::f32::consts::PI;
    let perp = perp(dir);
    let p_left = (pos.0 + perp.0 * half, pos.1 + perp.1 * half);
    let p_right = (pos.0 - perp.0 * half, pos.1 - perp.1 * half);
    let u = if is_start { 0.0 } else { 1.0 };

    let mut prev = p_left;
    let mut prev_uv = if is_start { (u, 0.0) } else { (u, 0.0) };
    for k in 1..=segments {
        let t = (k as f32) / (segments as f32);
        let a = pi * t;
        let mut rotated = (perp.0, perp.1);
        let c = a.cos();
        let s = a.sin();
        let rx = rotated.0 * c - rotated.1 * s;
        let ry = rotated.0 * s + rotated.1 * c;
        rotated = (rx, ry);
        let p = (pos.0 + rotated.0 * half, pos.1 + rotated.1 * half);
        out.push(SplineVertex {
            position: [pos.0, pos.1],
            uv: [u, 0.5],
        });
        out.push(SplineVertex {
            position: [prev.0, prev.1],
            uv: [prev_uv.0, prev_uv.1],
        });
        out.push(SplineVertex {
            position: [p.0, p.1],
            uv: [u, t],
        });
        prev = p;
        prev_uv = (u, t);
    }
    let _ = p_right;
}

fn push_square_cap(
    out: &mut Vec<SplineVertex>,
    here: &SplineNode,
    neighbor: &SplineNode,
    is_start: bool,
) {
    let pos = (here.position.x, here.position.y);
    let nbr = (neighbor.position.x, neighbor.position.y);
    let dir = norm((pos.0 - nbr.0, pos.1 - nbr.1));
    let perp = perp(dir);
    let half = here.thickness * 0.5;
    let extended = (pos.0 + dir.0 * half, pos.1 + dir.1 * half);
    let p_left = (pos.0 + perp.0 * half, pos.1 + perp.1 * half);
    let p_right = (pos.0 - perp.0 * half, pos.1 - perp.1 * half);
    let p_left_ext = (extended.0 + perp.0 * half, extended.1 + perp.1 * half);
    let p_right_ext = (extended.0 - perp.0 * half, extended.1 - perp.1 * half);
    let u = if is_start { 0.0 } else { 1.0 };
    out.push(SplineVertex {
        position: [p_left.0, p_left.1],
        uv: [u, 0.0],
    });
    out.push(SplineVertex {
        position: [p_left_ext.0, p_left_ext.1],
        uv: [u, 0.0],
    });
    out.push(SplineVertex {
        position: [p_right_ext.0, p_right_ext.1],
        uv: [u, 1.0],
    });
    out.push(SplineVertex {
        position: [p_left.0, p_left.1],
        uv: [u, 0.0],
    });
    out.push(SplineVertex {
        position: [p_right_ext.0, p_right_ext.1],
        uv: [u, 1.0],
    });
    out.push(SplineVertex {
        position: [p_right.0, p_right.1],
        uv: [u, 1.0],
    });
}

impl UserData for Spline {
    fn add_fields<F: UserDataFields<Self>>(f: &mut F) {
        f.add_field_method_get("Alive", |_, this| Ok(this.state.lock().unwrap().alive));
        f.add_field_method_get("Changed", |_, this| {
            Ok(this.state.lock().unwrap().changed_signal.clone())
        });
        f.add_field_method_get("Color", |_, this| Ok(this.state.lock().unwrap().color));
        f.add_field_method_set("Color", |lua, this, v: AnyUserData| {
            let c = *v.borrow::<Color3>().map_err(|_| {
                mlua::Error::RuntimeError("Color expects a Primitives.Color3".into())
            })?;
            let sig = {
                let mut s = this.state.lock().unwrap();
                s.color = c;
                s.changed_signal.clone()
            };
            fire_changed(lua, sig, "Color")?;
            Ok(())
        });
        f.add_field_method_get("Transparency", |_, this| {
            Ok(this.state.lock().unwrap().transparency)
        });
        f.add_field_method_set("Transparency", |lua, this, v: f32| {
            let sig = {
                let mut s = this.state.lock().unwrap();
                s.transparency = v.clamp(0.0, 1.0);
                s.changed_signal.clone()
            };
            fire_changed(lua, sig, "Transparency")?;
            Ok(())
        });
        f.add_field_method_get("ZIndex", |_, this| {
            Ok(this.state.lock().unwrap().z_index as i64)
        });
        f.add_field_method_set("ZIndex", |lua, this, v: i64| {
            let clamped = v.clamp(i32::MIN as i64, i32::MAX as i64) as i32;
            let sig = {
                let mut s = this.state.lock().unwrap();
                s.z_index = clamped;
                s.changed_signal.clone()
            };
            fire_changed(lua, sig, "ZIndex")?;
            Ok(())
        });
        f.add_field_method_get("Visible", |_, this| Ok(this.state.lock().unwrap().visible));
        f.add_field_method_set("Visible", |lua, this, v: bool| {
            let sig = {
                let mut s = this.state.lock().unwrap();
                s.visible = v;
                s.changed_signal.clone()
            };
            fire_changed(lua, sig, "Visible")?;
            Ok(())
        });
        f.add_field_method_get("Style", |_, this| {
            Ok(this.state.lock().unwrap().style.as_str())
        });
        f.add_field_method_set("Style", |lua, this, v: String| {
            let style = SplineStyle::from_str(&v).ok_or_else(|| {
                mlua::Error::RuntimeError(format!(
                    "Spline.Style: '{v}' is not a valid style (use 'Solid', 'Dashed', or 'Dotted')"
                ))
            })?;
            let sig = {
                let mut s = this.state.lock().unwrap();
                s.style = style;
                s.changed_signal.clone()
            };
            fire_changed(lua, sig, "Style")?;
            Ok(())
        });
        f.add_field_method_get("Cap", |_, this| Ok(this.state.lock().unwrap().cap.as_str()));
        f.add_field_method_set("Cap", |lua, this, v: String| {
            let cap = SplineCap::from_str(&v).ok_or_else(|| {
                mlua::Error::RuntimeError(format!(
                    "Spline.Cap: '{v}' is not a valid cap (use 'Rounded', 'Miter', or 'Square')"
                ))
            })?;
            let sig = {
                let mut s = this.state.lock().unwrap();
                s.cap = cap;
                s.changed_signal.clone()
            };
            fire_changed(lua, sig, "Cap")?;
            Ok(())
        });
        f.add_field_method_get("NodeCount", |_, this| {
            Ok(this.state.lock().unwrap().nodes.len() as i64)
        });
    }

    fn add_methods<M: UserDataMethods<Self>>(m: &mut M) {
        m.add_method("SetNodes", |lua, this, t: Table| -> mlua::Result<()> {
            let nodes = parse_nodes(&t)?;
            let sig = {
                let mut s = this.state.lock().unwrap();
                s.nodes = nodes;
                s.changed_signal.clone()
            };
            fire_changed(lua, sig, "Nodes")?;
            Ok(())
        });
        m.add_method(
            "GetNodes",
            |lua, this, _: ()| -> mlua::Result<Table> {
                let nodes = this.state.lock().unwrap().nodes.clone();
                let out = lua.create_table()?;
                for (i, n) in nodes.iter().enumerate() {
                    let row = lua.create_table()?;
                    row.set("Position", lua.create_userdata(n.position)?)?;
                    row.set("Thickness", n.thickness)?;
                    row.set("Angle", n.angle)?;
                    out.set(i as i64 + 1, row)?;
                }
                Ok(out)
            },
        );
        m.add_method(
            "InsertNode",
            |lua, this, (index, node_tbl): (i64, Table)| -> mlua::Result<()> {
                let node = parse_node(&node_tbl)?;
                let sig = {
                    let mut s = this.state.lock().unwrap();
                    let n = s.nodes.len() as i64;
                    let idx = if index <= 0 { 0 } else if index > n { n as usize } else { (index - 1) as usize };
                    s.nodes.insert(idx, node);
                    s.changed_signal.clone()
                };
                fire_changed(lua, sig, "Nodes")?;
                Ok(())
            },
        );
        m.add_method(
            "RemoveNode",
            |lua, this, index: i64| -> mlua::Result<()> {
                let sig = {
                    let mut s = this.state.lock().unwrap();
                    let n = s.nodes.len() as i64;
                    if index < 1 || index > n {
                        return Err(mlua::Error::RuntimeError(format!(
                            "Spline:RemoveNode: index {index} out of range (1..={n})"
                        )));
                    }
                    s.nodes.remove((index - 1) as usize);
                    s.changed_signal.clone()
                };
                fire_changed(lua, sig, "Nodes")?;
                Ok(())
            },
        );
        m.add_method(
            "SetNode",
            |lua, this, (index, node_tbl): (i64, Table)| -> mlua::Result<()> {
                let node = parse_node(&node_tbl)?;
                let sig = {
                    let mut s = this.state.lock().unwrap();
                    let n = s.nodes.len() as i64;
                    if index < 1 || index > n {
                        return Err(mlua::Error::RuntimeError(format!(
                            "Spline:SetNode: index {index} out of range (1..={n})"
                        )));
                    }
                    s.nodes[(index - 1) as usize] = node;
                    s.changed_signal.clone()
                };
                fire_changed(lua, sig, "Nodes")?;
                Ok(())
            },
        );
        m.add_method("Destroy", |_, this, _: ()| -> mlua::Result<()> {
            let mut s = this.state.lock().unwrap();
            s.alive = false;
            s.visible = false;
            s.attached.clear();
            bump_dirty();
            Ok(())
        });
        m.add_method(
            "AttachShader",
            |_, this, ud: AnyUserData| -> mlua::Result<()> {
                let frag = ud.borrow::<crate::libs::asset::FragmentAsset>().ok();
                let id = frag
                    .as_ref()
                    .map(|f| f.id)
                    .or_else(|| {
                        ud.borrow::<crate::libs::asset::ShaderAsset>()
                            .ok()
                            .map(|s| s.id)
                    })
                    .ok_or_else(|| {
                        mlua::Error::RuntimeError(
                            "Spline:AttachShader expects a FragmentAsset or ShaderAsset".into(),
                        )
                    })?;
                let (wgsl, source) = if let Some(f) = frag {
                    (f.code.clone(), f.source.clone())
                } else {
                    let s = ud.borrow::<crate::libs::asset::ShaderAsset>()?;
                    (s.code.clone(), s.source.clone())
                };
                let attached = AttachedShader {
                    id,
                    source,
                    wgsl: Arc::new(wgsl),
                    slot_of_name: Arc::new(std::collections::HashMap::new()),
                    params: Arc::new(Mutex::new([0.0_f32; 16])),
                };
                this.state.lock().unwrap().attached.push(attached);
                bump_dirty();
                Ok(())
            },
        );
        m.add_method("ClearShaders", |_, this, _: ()| -> mlua::Result<()> {
            this.state.lock().unwrap().attached.clear();
            bump_dirty();
            Ok(())
        });
        m.add_method(
            "GetPropertyChangedSignal",
            |lua, this, name: String| -> mlua::Result<Table> {
                let mut s = this.state.lock().unwrap();
                if let Some(t) = s.prop_signals.get(&name) {
                    return Ok(t.clone());
                }
                let sig = signal::new_instance(lua)?;
                s.prop_signals.insert(name, sig.clone());
                Ok(sig)
            },
        );
        m.add_meta_method("__tostring", |_, this, _: ()| {
            Ok(format!(
                "Spline(id={}, nodes={})",
                this.state.lock().unwrap().id,
                this.state.lock().unwrap().nodes.len()
            ))
        });
    }
}

fn parse_nodes(t: &Table) -> mlua::Result<Vec<SplineNode>> {
    let len = t.raw_len() as usize;
    let mut out = Vec::with_capacity(len);
    for i in 1..=len {
        let v: Value = t.get(i as i64)?;
        let row = match v {
            Value::Table(r) => r,
            _ => {
                return Err(mlua::Error::RuntimeError(format!(
                    "Spline nodes: entry #{i} must be a table"
                )))
            }
        };
        out.push(parse_node(&row)?);
    }
    Ok(out)
}

fn parse_node(t: &Table) -> mlua::Result<SplineNode> {
    let position = if let Ok(ud) = t.get::<AnyUserData>("Position") {
        *ud.borrow::<Dim>().map_err(|_| {
            mlua::Error::RuntimeError("Spline node Position must be a Primitives.Dim".into())
        })?
    } else {
        Dim::new(0.0, 0.0)
    };
    let thickness = t.get::<f32>("Thickness").unwrap_or(4.0).max(0.0);
    let angle = t.get::<f32>("Angle").unwrap_or(0.0);
    Ok(SplineNode {
        position,
        thickness,
        angle,
    })
}

