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
    pub length: f32,
    pub padding: f32,
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
            length: 12.0,
            padding: 8.0,
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
    pub length: f32,
    pub padding: f32,
    pub total_pixel_length: f32,
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
                let total_pixel_length = polyline_length(&s.nodes);
                Some(SplineRender {
                    id: s.id,
                    vertices,
                    color: s.color,
                    transparency: s.transparency,
                    z_index: s.z_index,
                    active_shader: s.attached.last().cloned(),
                    aabb,
                    style: s.style,
                    length: s.length,
                    padding: s.padding,
                    total_pixel_length,
                })
            })
            .collect()
    })
}

fn polyline_length(nodes: &[SplineNode]) -> f32 {
    let mut total = 0.0_f32;
    for i in 0..nodes.len().saturating_sub(1) {
        let a = nodes[i].position;
        let b = nodes[i + 1].position;
        let dx = b.x - a.x;
        let dy = b.y - a.y;
        total += (dx * dx + dy * dy).sqrt();
    }
    total.max(1.0)
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
    let samples = sample_curve(nodes);
    if samples.len() < 2 {
        return Vec::new();
    }

    let mut out: Vec<SplineVertex> = Vec::with_capacity(samples.len() * 6 + 32);
    for i in 0..samples.len() - 1 {
        let a = &samples[i];
        let b = &samples[i + 1];
        let pa = perp(a.tangent);
        let pb = perp(b.tangent);
        let half_a = a.thickness * 0.5;
        let half_b = b.thickness * 0.5;
        let l0 = (a.pos.0 - pa.0 * half_a, a.pos.1 - pa.1 * half_a);
        let r0 = (a.pos.0 + pa.0 * half_a, a.pos.1 + pa.1 * half_a);
        let l1 = (b.pos.0 - pb.0 * half_b, b.pos.1 - pb.1 * half_b);
        let r1 = (b.pos.0 + pb.0 * half_b, b.pos.1 + pb.1 * half_b);
        out.push(SplineVertex { position: [l0.0, l0.1], uv: [a.u, 0.0] });
        out.push(SplineVertex { position: [r0.0, r0.1], uv: [a.u, 1.0] });
        out.push(SplineVertex { position: [r1.0, r1.1], uv: [b.u, 1.0] });
        out.push(SplineVertex { position: [l0.0, l0.1], uv: [a.u, 0.0] });
        out.push(SplineVertex { position: [r1.0, r1.1], uv: [b.u, 1.0] });
        out.push(SplineVertex { position: [l1.0, l1.1], uv: [b.u, 0.0] });
    }

    match cap {
        SplineCap::Rounded => {
            let first = &samples[0];
            let last = samples.last().unwrap();
            push_round_cap_sample(&mut out, first.pos, first.tangent, first.thickness, true);
            push_round_cap_sample(&mut out, last.pos, last.tangent, last.thickness, false);
        }
        SplineCap::Square => {
            let first = &samples[0];
            let last = samples.last().unwrap();
            push_square_cap_sample(&mut out, first.pos, first.tangent, first.thickness, true);
            push_square_cap_sample(&mut out, last.pos, last.tangent, last.thickness, false);
        }
        SplineCap::Miter => {}
    }

    out
}

struct CurveSample {
    pos: (f32, f32),
    tangent: (f32, f32),
    thickness: f32,
    u: f32,
}

fn sample_curve(nodes: &[SplineNode]) -> Vec<CurveSample> {
    let n = nodes.len();
    let mut out: Vec<CurveSample> = Vec::with_capacity(n * 12);
    if n < 2 {
        return out;
    }
    let tangents = node_tangents(nodes);
    for i in 0..n - 1 {
        let p0 = (nodes[i].position.x, nodes[i].position.y);
        let p3 = (nodes[i + 1].position.x, nodes[i + 1].position.y);
        let chord = ((p3.0 - p0.0).powi(2) + (p3.1 - p0.1).powi(2)).sqrt();
        let handle = chord / 3.0;
        let t0 = tangents[i];
        let t1 = tangents[i + 1];
        let p1 = (p0.0 + t0.0 * handle, p0.1 + t0.1 * handle);
        let p2 = (p3.0 - t1.0 * handle, p3.1 - t1.1 * handle);
        let steps = (chord / 8.0).round().clamp(8.0, 32.0) as i32;
        let th0 = nodes[i].thickness;
        let th1 = nodes[i + 1].thickness;
        let u_start = i as f32 / (n - 1) as f32;
        let u_end = (i + 1) as f32 / (n - 1) as f32;
        let skip_first = i > 0;
        for s in 0..=steps {
            if s == 0 && skip_first {
                continue;
            }
            let t = s as f32 / steps as f32;
            let pos = cubic_bezier(p0, p1, p2, p3, t);
            let tangent = norm(cubic_bezier_tangent(p0, p1, p2, p3, t));
            let thickness = th0 + (th1 - th0) * t;
            let u = u_start + (u_end - u_start) * t;
            out.push(CurveSample {
                pos,
                tangent,
                thickness,
                u,
            });
        }
    }
    out
}

fn node_tangents(nodes: &[SplineNode]) -> Vec<(f32, f32)> {
    let n = nodes.len();
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let pos = (nodes[i].position.x, nodes[i].position.y);
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
        let rot = nodes[i].angle.to_radians();
        out.push(if rot.abs() > 1e-5 {
            rotate(avg, rot)
        } else {
            avg
        });
    }
    out
}

fn cubic_bezier(
    p0: (f32, f32),
    p1: (f32, f32),
    p2: (f32, f32),
    p3: (f32, f32),
    t: f32,
) -> (f32, f32) {
    let u = 1.0 - t;
    let b0 = u * u * u;
    let b1 = 3.0 * u * u * t;
    let b2 = 3.0 * u * t * t;
    let b3 = t * t * t;
    (
        b0 * p0.0 + b1 * p1.0 + b2 * p2.0 + b3 * p3.0,
        b0 * p0.1 + b1 * p1.1 + b2 * p2.1 + b3 * p3.1,
    )
}

fn cubic_bezier_tangent(
    p0: (f32, f32),
    p1: (f32, f32),
    p2: (f32, f32),
    p3: (f32, f32),
    t: f32,
) -> (f32, f32) {
    let u = 1.0 - t;
    let c0 = 3.0 * u * u;
    let c1 = 6.0 * u * t;
    let c2 = 3.0 * t * t;
    (
        c0 * (p1.0 - p0.0) + c1 * (p2.0 - p1.0) + c2 * (p3.0 - p2.0),
        c0 * (p1.1 - p0.1) + c1 * (p2.1 - p1.1) + c2 * (p3.1 - p2.1),
    )
}

fn push_round_cap_sample(
    out: &mut Vec<SplineVertex>,
    pos: (f32, f32),
    tangent: (f32, f32),
    thickness: f32,
    is_start: bool,
) {
    let dir = if is_start {
        (-tangent.0, -tangent.1)
    } else {
        tangent
    };
    let half = thickness * 0.5;
    let segments = 8;
    let pi = std::f32::consts::PI;
    let perp = perp(dir);
    let u = if is_start { 0.0 } else { 1.0 };
    let p_left = (pos.0 + perp.0 * half, pos.1 + perp.1 * half);

    let mut prev = p_left;
    let mut prev_uv = (u, 0.0);
    for k in 1..=segments {
        let t = (k as f32) / (segments as f32);
        let a = pi * t;
        let c = a.cos();
        let s = a.sin();
        let rx = perp.0 * c - perp.1 * s;
        let ry = perp.0 * s + perp.1 * c;
        let p = (pos.0 + rx * half, pos.1 + ry * half);
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
}

fn push_square_cap_sample(
    out: &mut Vec<SplineVertex>,
    pos: (f32, f32),
    tangent: (f32, f32),
    thickness: f32,
    is_start: bool,
) {
    let dir = if is_start {
        (-tangent.0, -tangent.1)
    } else {
        tangent
    };
    let perp = perp(dir);
    let half = thickness * 0.5;
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
        f.add_field_method_get("Length", |_, this| Ok(this.state.lock().unwrap().length));
        f.add_field_method_set("Length", |lua, this, v: f32| {
            let sig = {
                let mut s = this.state.lock().unwrap();
                s.length = v.max(0.0);
                s.changed_signal.clone()
            };
            fire_changed(lua, sig, "Length")?;
            Ok(())
        });
        f.add_field_method_get("Padding", |_, this| Ok(this.state.lock().unwrap().padding));
        f.add_field_method_set("Padding", |lua, this, v: f32| {
            let sig = {
                let mut s = this.state.lock().unwrap();
                s.padding = v.max(0.0);
                s.changed_signal.clone()
            };
            fire_changed(lua, sig, "Padding")?;
            Ok(())
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

