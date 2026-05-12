use std::cell::RefCell;
use std::f32::consts::PI;
use std::sync::{Arc, Mutex};

use mlua::{Lua, MultiValue, Table, UserData, UserDataFields, UserDataMethods, Value};

use crate::libs::gui::{GuiPrimitive, PrimitiveState};
use crate::libs::primitives::{CFrame, Color3, Dim, Vector};
use crate::libs::renderable::{self, PartHandle, PartState};
use crate::libs::signal;

#[derive(Clone, Copy, PartialEq)]
enum Family {
    Linear,
    Sine,
    Quad,
    Cubic,
    Quart,
    Quint,
    Expo,
    Circ,
    Back,
    Elastic,
    Bounce,
}

#[derive(Clone, Copy, PartialEq)]
enum Direction {
    In,
    Out,
    InOut,
}

fn parse_family(s: &str) -> mlua::Result<Family> {
    Ok(match s.trim().to_ascii_lowercase().as_str() {
        "" | "linear" => Family::Linear,
        "sine" => Family::Sine,
        "quad" => Family::Quad,
        "cubic" => Family::Cubic,
        "quart" => Family::Quart,
        "quint" => Family::Quint,
        "expo" => Family::Expo,
        "circ" => Family::Circ,
        "back" => Family::Back,
        "elastic" => Family::Elastic,
        "bounce" => Family::Bounce,
        other => {
            return Err(mlua::Error::RuntimeError(format!(
                "TweenService: unknown easing style '{other}' (expected one of: Linear, Sine, Quad, Cubic, Quart, Quint, Expo, Circ, Back, Elastic, Bounce)"
            )));
        }
    })
}

fn parse_direction(s: &str) -> mlua::Result<Direction> {
    Ok(match s.trim().to_ascii_lowercase().as_str() {
        "" | "out" => Direction::Out,
        "in" => Direction::In,
        "inout" | "in_out" | "in-out" => Direction::InOut,
        other => {
            return Err(mlua::Error::RuntimeError(format!(
                "TweenService: unknown direction '{other}' (expected one of: In, Out, InOut)"
            )));
        }
    })
}

fn parse_style_value(v: Value) -> mlua::Result<Family> {
    match v {
        Value::Nil => Ok(Family::Linear),
        Value::String(s) => parse_family(s.to_str()?.as_ref()),
        _ => Err(mlua::Error::RuntimeError(
            "TweenService: style must be a string (e.g. \"Linear\", \"Bounce\", \"Cubic\")".into(),
        )),
    }
}

fn parse_direction_value(v: Value) -> mlua::Result<Direction> {
    match v {
        Value::Nil => Ok(Direction::Out),
        Value::String(s) => parse_direction(s.to_str()?.as_ref()),
        _ => Err(mlua::Error::RuntimeError(
            "TweenService: direction must be a string (\"In\", \"Out\", or \"InOut\")".into(),
        )),
    }
}

fn ease_in(family: Family, t: f32) -> f32 {
    match family {
        Family::Linear => t,
        Family::Sine => 1.0 - (t * PI * 0.5).cos(),
        Family::Quad => t * t,
        Family::Cubic => t * t * t,
        Family::Quart => t * t * t * t,
        Family::Quint => t * t * t * t * t,
        Family::Expo => {
            if t <= 0.0 {
                0.0
            } else {
                2.0_f32.powf(10.0 * t - 10.0)
            }
        }
        Family::Circ => 1.0 - (1.0 - t * t).max(0.0).sqrt(),
        Family::Back => {
            const C1: f32 = 1.70158;
            const C3: f32 = C1 + 1.0;
            C3 * t * t * t - C1 * t * t
        }
        Family::Elastic => {
            const C4: f32 = (2.0 * PI) / 3.0;
            if t <= 0.0 {
                0.0
            } else if t >= 1.0 {
                1.0
            } else {
                -(2.0_f32.powf(10.0 * t - 10.0)) * ((t * 10.0 - 10.75) * C4).sin()
            }
        }
        Family::Bounce => 1.0 - ease_out_bounce(1.0 - t),
    }
}

fn ease_out_bounce(t: f32) -> f32 {
    const N1: f32 = 7.5625;
    const D1: f32 = 2.75;
    if t < 1.0 / D1 {
        N1 * t * t
    } else if t < 2.0 / D1 {
        let t = t - 1.5 / D1;
        N1 * t * t + 0.75
    } else if t < 2.5 / D1 {
        let t = t - 2.25 / D1;
        N1 * t * t + 0.9375
    } else {
        let t = t - 2.625 / D1;
        N1 * t * t + 0.984375
    }
}

fn ease(family: Family, direction: Direction, t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    match direction {
        Direction::In => ease_in(family, t),
        Direction::Out => 1.0 - ease_in(family, 1.0 - t),
        Direction::InOut => {
            if t < 0.5 {
                ease_in(family, t * 2.0) * 0.5
            } else {
                1.0 - ease_in(family, (1.0 - t) * 2.0) * 0.5
            }
        }
    }
}

fn lerp_f(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

fn lerp_vec(a: Vector, b: Vector, t: f32) -> Vector {
    Vector::new(lerp_f(a.x, b.x, t), lerp_f(a.y, b.y, t), lerp_f(a.z, b.z, t))
}

fn lerp_color(a: Color3, b: Color3, t: f32) -> Color3 {
    Color3::new(lerp_f(a.r, b.r, t), lerp_f(a.g, b.g, t), lerp_f(a.b, b.b, t))
}

fn lerp_cframe(a: CFrame, b: CFrame, t: f32) -> CFrame {
    CFrame {
        position: lerp_vec(a.position, b.position, t),
        rotation: lerp_vec(a.rotation, b.rotation, t),
    }
}

fn lerp_dim(a: Dim, b: Dim, t: f32) -> Dim {
    Dim::new(lerp_f(a.x, b.x, t), lerp_f(a.y, b.y, t))
}

enum TweenProp {
    PartCFrame { start: CFrame, end_: CFrame },
    PartColor { start: Color3, end_: Color3 },
    PartSize { start: Vector, end_: Vector },
    GuiPosition { start: Dim, end_: Dim },
    GuiSize { start: Dim, end_: Dim },
    GuiColor { start: Color3, end_: Color3 },
    GuiTransparency { start: f32, end_: f32 },
    GuiZIndex { start: f32, end_: f32 },
    SoundVolume { start: f32, end_: f32 },
    SoundPitch { start: f32, end_: f32 },
    SoundPan { start: f32, end_: f32 },
    SoundDistortion { start: f32, end_: f32 },
    SoundPosition { start: Vector, end_: Vector },
    SoundMinFalloff { start: f32, end_: f32 },
    SoundMaxFalloff { start: f32, end_: f32 },
    VoiceVolume { start: f32, end_: f32 },
    VoicePitch { start: f32, end_: f32 },
    VoicePosition { start: Vector, end_: Vector },
    VoiceMinFalloff { start: f32, end_: f32 },
    VoiceMaxFalloff { start: f32, end_: f32 },
}

#[derive(Clone, Copy, PartialEq)]
enum TweenState {
    Idle,
    Playing,
    Paused,
    Completed,
    Cancelled,
}

enum TweenTarget {
    Part(Arc<Mutex<PartState>>),
    Gui(Arc<Mutex<PrimitiveState>>),
    Sound {
        shaders: Arc<Mutex<Vec<crate::libs::sfx::Shader>>>,
        position: Arc<Mutex<Option<crate::libs::sfx::SpatialPos>>>,
    },
    Voice {
        shaders: Arc<Mutex<Vec<crate::libs::voice::VoiceShader>>>,
        position: Arc<Mutex<(f32, f32, f32)>>,
    },
}

struct GoalSpec {
    property: String,
    value: GoalValue,
}

enum GoalValue {
    CFrame(CFrame),
    Color(Color3),
    Vector(Vector),
    Dim(Dim),
    Number(f32),
}

struct TweenInner {
    duration: f32,
    family: Family,
    direction: Direction,
    state: TweenState,
    elapsed: f32,
    target: TweenTarget,
    goals: Vec<GoalSpec>,
    properties: Vec<TweenProp>,
    completed_signal: Table,
}

thread_local! {
    static ACTIVE: RefCell<Vec<Arc<Mutex<TweenInner>>>> = const { RefCell::new(Vec::new()) };
}

fn snapshot_start_values(inner: &mut TweenInner) -> mlua::Result<()> {
    inner.properties.clear();
    match &inner.target {
        TweenTarget::Part(state_arc) => {
            let s = state_arc.lock().unwrap();
            for g in &inner.goals {
                let prop = match (g.property.as_str(), &g.value) {
                    ("CFrame", GoalValue::CFrame(end_)) => TweenProp::PartCFrame {
                        start: s.cframe,
                        end_: *end_,
                    },
                    ("Color", GoalValue::Color(end_)) => TweenProp::PartColor {
                        start: s.color,
                        end_: *end_,
                    },
                    ("Size", GoalValue::Vector(end_)) => TweenProp::PartSize {
                        start: s.size,
                        end_: *end_,
                    },
                    (name, _) => {
                        return Err(mlua::Error::RuntimeError(format!(
                            "TweenService: BasePart has no tweenable property '{name}' (supported: CFrame, Color, Size — and the value type must match)"
                        )));
                    }
                };
                inner.properties.push(prop);
            }
        }
        TweenTarget::Gui(state_arc) => {
            let s = state_arc.lock().unwrap();
            for g in &inner.goals {
                let prop = match (g.property.as_str(), &g.value) {
                    ("Position", GoalValue::Dim(end_)) => TweenProp::GuiPosition {
                        start: s.position,
                        end_: *end_,
                    },
                    ("Size", GoalValue::Dim(end_)) => TweenProp::GuiSize {
                        start: s.size,
                        end_: *end_,
                    },
                    ("Color", GoalValue::Color(end_)) => TweenProp::GuiColor {
                        start: s.color,
                        end_: *end_,
                    },
                    ("Transparency", GoalValue::Number(end_)) => TweenProp::GuiTransparency {
                        start: s.transparency,
                        end_: *end_,
                    },
                    ("ZIndex", GoalValue::Number(end_)) => TweenProp::GuiZIndex {
                        start: s.z_index as f32,
                        end_: *end_,
                    },
                    (name, _) => {
                        return Err(mlua::Error::RuntimeError(format!(
                            "TweenService: GUI primitive has no tweenable property '{name}' (supported: Position, Size, Color, Transparency, ZIndex — and the value type must match)"
                        )));
                    }
                };
                inner.properties.push(prop);
            }
        }
        TweenTarget::Sound { shaders, position } => {
            use crate::libs::sfx::Shader;
            let find_scalar = |pick: fn(&Shader) -> Option<f32>| -> Option<f32> {
                shaders.lock().unwrap().iter().rev().find_map(pick)
            };
            let pos_snap = *position.lock().unwrap();
            for g in &inner.goals {
                let prop = match (g.property.as_str(), &g.value) {
                    ("Volume", GoalValue::Number(end_)) => TweenProp::SoundVolume {
                        start: find_scalar(|s| if let Shader::Volume(v) = s { Some(*v) } else { None }).unwrap_or(1.0),
                        end_: *end_,
                    },
                    ("Pitch", GoalValue::Number(end_)) | ("Speed", GoalValue::Number(end_)) => TweenProp::SoundPitch {
                        start: find_scalar(|s| if let Shader::Speed(v) = s { Some(*v) } else { None }).unwrap_or(1.0),
                        end_: *end_,
                    },
                    ("Pan", GoalValue::Number(end_)) => TweenProp::SoundPan {
                        start: find_scalar(|s| if let Shader::Pan(v) = s { Some(*v) } else { None }).unwrap_or(0.0),
                        end_: *end_,
                    },
                    ("Distortion", GoalValue::Number(end_)) => TweenProp::SoundDistortion {
                        start: find_scalar(|s| if let Shader::Distortion(v) = s { Some(*v) } else { None }).unwrap_or(0.0),
                        end_: *end_,
                    },
                    ("Position", GoalValue::Vector(end_)) => TweenProp::SoundPosition {
                        start: pos_snap.map(|p| Vector::new(p.x, p.y, p.z)).unwrap_or(Vector::new(0.0, 0.0, 0.0)),
                        end_: *end_,
                    },
                    ("MinFalloff", GoalValue::Number(end_)) => TweenProp::SoundMinFalloff {
                        start: pos_snap.map(|p| p.min_falloff).unwrap_or(0.0),
                        end_: *end_,
                    },
                    ("MaxFalloff", GoalValue::Number(end_)) => TweenProp::SoundMaxFalloff {
                        start: pos_snap.map(|p| p.max_falloff).unwrap_or(20.0),
                        end_: *end_,
                    },
                    (name, _) => {
                        return Err(mlua::Error::RuntimeError(format!(
                            "TweenService: Sound has no tweenable property '{name}' (supported: Volume, Pitch/Speed, Pan, Distortion, Position, MinFalloff, MaxFalloff — and the value type must match)"
                        )));
                    }
                };
                inner.properties.push(prop);
            }
        }
        TweenTarget::Voice { shaders, position } => {
            use crate::libs::voice::VoiceShader;
            let find_scalar = |pick: fn(&VoiceShader) -> Option<f32>| -> Option<f32> {
                shaders.lock().unwrap().iter().rev().find_map(pick)
            };
            let find_spatial = || -> Option<(f32, f32, f32, f32, f32)> {
                shaders.lock().unwrap().iter().rev().find_map(|s| match s {
                    VoiceShader::Spatial { x, y, z, min_falloff, max_falloff } => {
                        Some((*x, *y, *z, *min_falloff, *max_falloff))
                    }
                    _ => None,
                })
            };
            for g in &inner.goals {
                let prop = match (g.property.as_str(), &g.value) {
                    ("Volume", GoalValue::Number(end_)) => TweenProp::VoiceVolume {
                        start: find_scalar(|s| if let VoiceShader::Volume(v) = s { Some(*v) } else { None }).unwrap_or(1.0),
                        end_: *end_,
                    },
                    ("Pitch", GoalValue::Number(end_)) | ("Speed", GoalValue::Number(end_)) => TweenProp::VoicePitch {
                        start: find_scalar(|s| if let VoiceShader::Speed(v) = s { Some(*v) } else { None }).unwrap_or(1.0),
                        end_: *end_,
                    },
                    ("Position", GoalValue::Vector(end_)) => {
                        let start = find_spatial()
                            .map(|(x, y, z, _, _)| Vector::new(x, y, z))
                            .unwrap_or_else(|| {
                                let p = *position.lock().unwrap();
                                Vector::new(p.0, p.1, p.2)
                            });
                        TweenProp::VoicePosition { start, end_: *end_ }
                    }
                    ("MinFalloff", GoalValue::Number(end_)) => TweenProp::VoiceMinFalloff {
                        start: find_spatial().map(|(_, _, _, mn, _)| mn).unwrap_or(0.0),
                        end_: *end_,
                    },
                    ("MaxFalloff", GoalValue::Number(end_)) => TweenProp::VoiceMaxFalloff {
                        start: find_spatial().map(|(_, _, _, _, mx)| mx).unwrap_or(8.0),
                        end_: *end_,
                    },
                    (name, _) => {
                        return Err(mlua::Error::RuntimeError(format!(
                            "TweenService: VoiceChannel has no tweenable property '{name}' (supported: Volume, Pitch/Speed, Position, MinFalloff, MaxFalloff — and the value type must match)"
                        )));
                    }
                };
                inner.properties.push(prop);
            }
        }
    }
    Ok(())
}

fn apply_progress(inner: &TweenInner, t: f32) {
    let eased = ease(inner.family, inner.direction, t);
    match &inner.target {
        TweenTarget::Part(state_arc) => {
            let mut s = state_arc.lock().unwrap();
            for prop in &inner.properties {
                match prop {
                    TweenProp::PartCFrame { start, end_ } => {
                        s.cframe = lerp_cframe(*start, *end_, eased);
                    }
                    TweenProp::PartColor { start, end_ } => {
                        s.color = lerp_color(*start, *end_, eased);
                    }
                    TweenProp::PartSize { start, end_ } => {
                        s.size = lerp_vec(*start, *end_, eased);
                    }
                    _ => {}
                }
            }
            drop(s);
            renderable::bump_parts_dirty();
        }
        TweenTarget::Gui(state_arc) => {
            let mut s = state_arc.lock().unwrap();
            for prop in &inner.properties {
                match prop {
                    TweenProp::GuiPosition { start, end_ } => {
                        s.position = lerp_dim(*start, *end_, eased);
                    }
                    TweenProp::GuiSize { start, end_ } => {
                        s.size = lerp_dim(*start, *end_, eased);
                    }
                    TweenProp::GuiColor { start, end_ } => {
                        s.color = lerp_color(*start, *end_, eased);
                    }
                    TweenProp::GuiTransparency { start, end_ } => {
                        s.transparency = lerp_f(*start, *end_, eased).clamp(0.0, 1.0);
                    }
                    TweenProp::GuiZIndex { start, end_ } => {
                        s.z_index = lerp_f(*start, *end_, eased).round() as i32;
                    }
                    _ => {}
                }
            }
        }
        TweenTarget::Sound { shaders, position } => {
            use crate::libs::sfx::{Shader, SpatialPos};
            fn replace<F: Fn(&Shader) -> bool>(list: &mut Vec<Shader>, kind: F, new: Shader) {
                list.retain(|s| !kind(s));
                list.push(new);
            }
            for prop in &inner.properties {
                match prop {
                    TweenProp::SoundVolume { start, end_ } => {
                        let v = lerp_f(*start, *end_, eased).max(0.0);
                        replace(&mut shaders.lock().unwrap(), |s| matches!(s, Shader::Volume(_)), Shader::Volume(v));
                    }
                    TweenProp::SoundPitch { start, end_ } => {
                        let v = lerp_f(*start, *end_, eased).max(0.0);
                        replace(&mut shaders.lock().unwrap(), |s| matches!(s, Shader::Speed(_)), Shader::Speed(v));
                    }
                    TweenProp::SoundPan { start, end_ } => {
                        let v = lerp_f(*start, *end_, eased).clamp(-1.0, 1.0);
                        replace(&mut shaders.lock().unwrap(), |s| matches!(s, Shader::Pan(_)), Shader::Pan(v));
                    }
                    TweenProp::SoundDistortion { start, end_ } => {
                        let v = lerp_f(*start, *end_, eased).max(0.0);
                        replace(&mut shaders.lock().unwrap(), |s| matches!(s, Shader::Distortion(_)), Shader::Distortion(v));
                    }
                    TweenProp::SoundPosition { start, end_ } => {
                        let v = lerp_vec(*start, *end_, eased);
                        let mut guard = position.lock().unwrap();
                        let (min_f, max_f) = guard.as_ref().map(|p| (p.min_falloff, p.max_falloff)).unwrap_or((0.0, 20.0));
                        *guard = Some(SpatialPos { x: v.x, y: v.y, z: v.z, min_falloff: min_f, max_falloff: max_f });
                    }
                    TweenProp::SoundMinFalloff { start, end_ } => {
                        let v = lerp_f(*start, *end_, eased).max(0.0);
                        let mut guard = position.lock().unwrap();
                        match guard.as_mut() {
                            Some(p) => {
                                p.min_falloff = v;
                                if p.max_falloff < p.min_falloff { p.max_falloff = p.min_falloff; }
                            }
                            None => {
                                *guard = Some(SpatialPos { x: 0.0, y: 0.0, z: 0.0, min_falloff: v, max_falloff: v.max(20.0) });
                            }
                        }
                    }
                    TweenProp::SoundMaxFalloff { start, end_ } => {
                        let v = lerp_f(*start, *end_, eased).max(0.0);
                        let mut guard = position.lock().unwrap();
                        match guard.as_mut() {
                            Some(p) => {
                                p.max_falloff = v;
                                if p.min_falloff > p.max_falloff { p.min_falloff = p.max_falloff; }
                            }
                            None => {
                                *guard = Some(SpatialPos { x: 0.0, y: 0.0, z: 0.0, min_falloff: 0.0, max_falloff: v });
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
        TweenTarget::Voice { shaders, position } => {
            use crate::libs::voice::VoiceShader;
            fn replace<F: Fn(&VoiceShader) -> bool>(list: &mut Vec<VoiceShader>, kind: F, new: VoiceShader) {
                list.retain(|s| !kind(s));
                list.push(new);
            }
            let current_spatial = || -> (f32, f32, f32, f32, f32) {
                shaders.lock().unwrap().iter().rev().find_map(|s| match s {
                    VoiceShader::Spatial { x, y, z, min_falloff, max_falloff } => {
                        Some((*x, *y, *z, *min_falloff, *max_falloff))
                    }
                    _ => None,
                }).unwrap_or((0.0, 0.0, 0.0, 0.0, 8.0))
            };
            for prop in &inner.properties {
                match prop {
                    TweenProp::VoiceVolume { start, end_ } => {
                        let v = lerp_f(*start, *end_, eased).max(0.0);
                        replace(&mut shaders.lock().unwrap(), |s| matches!(s, VoiceShader::Volume(_)), VoiceShader::Volume(v));
                    }
                    TweenProp::VoicePitch { start, end_ } => {
                        let v = lerp_f(*start, *end_, eased).max(0.0);
                        replace(&mut shaders.lock().unwrap(), |s| matches!(s, VoiceShader::Speed(_)), VoiceShader::Speed(v));
                    }
                    TweenProp::VoicePosition { start, end_ } => {
                        let v = lerp_vec(*start, *end_, eased);
                        let (_, _, _, mn, mx) = current_spatial();
                        replace(
                            &mut shaders.lock().unwrap(),
                            |s| matches!(s, VoiceShader::Spatial { .. }),
                            VoiceShader::Spatial { x: v.x, y: v.y, z: v.z, min_falloff: mn, max_falloff: mx },
                        );
                        *position.lock().unwrap() = (v.x, v.y, v.z);
                    }
                    TweenProp::VoiceMinFalloff { start, end_ } => {
                        let v = lerp_f(*start, *end_, eased).max(0.0);
                        let (x, y, z, _, mx) = current_spatial();
                        let new_max = mx.max(v);
                        replace(
                            &mut shaders.lock().unwrap(),
                            |s| matches!(s, VoiceShader::Spatial { .. }),
                            VoiceShader::Spatial { x, y, z, min_falloff: v, max_falloff: new_max },
                        );
                        *position.lock().unwrap() = (x, y, z);
                    }
                    TweenProp::VoiceMaxFalloff { start, end_ } => {
                        let v = lerp_f(*start, *end_, eased).max(0.0);
                        let (x, y, z, mn, _) = current_spatial();
                        let new_min = mn.min(v);
                        replace(
                            &mut shaders.lock().unwrap(),
                            |s| matches!(s, VoiceShader::Spatial { .. }),
                            VoiceShader::Spatial { x, y, z, min_falloff: new_min, max_falloff: v },
                        );
                        *position.lock().unwrap() = (x, y, z);
                    }
                    _ => {}
                }
            }
        }
    }
}

pub fn tick(lua: &Lua, dt: f32) {
    let snapshot: Vec<Arc<Mutex<TweenInner>>> = ACTIVE.with(|c| c.borrow().clone());
    let mut completed: Vec<Arc<Mutex<TweenInner>>> = Vec::new();

    for tw in &snapshot {
        let mut inner = tw.lock().unwrap();
        if inner.state != TweenState::Playing {
            continue;
        }
        inner.elapsed += dt;
        let dur = inner.duration.max(1e-6);
        let raw_t = (inner.elapsed / dur).clamp(0.0, 1.0);
        apply_progress(&inner, raw_t);
        if inner.elapsed >= dur {
            inner.state = TweenState::Completed;
            completed.push(tw.clone());
        }
    }

    ACTIVE.with(|c| {
        c.borrow_mut().retain(|t| {
            let s = t.lock().unwrap().state;
            s == TweenState::Playing || s == TweenState::Paused
        });
    });

    for tw in completed {
        let signal = tw.lock().unwrap().completed_signal.clone();
        let _ = signal::fire(lua, &signal, MultiValue::new());
    }
}

pub struct TweenHandle {
    inner: Arc<Mutex<TweenInner>>,
    completed: Table,
}

impl UserData for TweenHandle {
    fn add_fields<F: UserDataFields<Self>>(f: &mut F) {
        f.add_field_method_get("Completed", |_, this| Ok(this.completed.clone()));
        f.add_field_method_get("State", |_, this| {
            let s = this.inner.lock().unwrap().state;
            let label = match s {
                TweenState::Idle => "Idle",
                TweenState::Playing => "Playing",
                TweenState::Paused => "Paused",
                TweenState::Completed => "Completed",
                TweenState::Cancelled => "Cancelled",
            };
            Ok(label.to_string())
        });
        f.add_field_method_get("Elapsed", |_, this| Ok(this.inner.lock().unwrap().elapsed));
        f.add_field_method_get("Duration", |_, this| {
            Ok(this.inner.lock().unwrap().duration)
        });
    }

    fn add_methods<M: UserDataMethods<Self>>(m: &mut M) {
        m.add_method("Play", |_, this, _: ()| -> mlua::Result<()> {
            let mut inner = this.inner.lock().unwrap();
            if inner.state == TweenState::Playing {
                return Ok(());
            }
            if inner.state == TweenState::Completed || inner.state == TweenState::Cancelled || inner.state == TweenState::Idle {
                inner.elapsed = 0.0;
                snapshot_start_values(&mut inner)?;
            }
            inner.state = TweenState::Playing;
            drop(inner);
            let already = ACTIVE.with(|c| {
                c.borrow().iter().any(|a| Arc::ptr_eq(a, &this.inner))
            });
            if !already {
                ACTIVE.with(|c| c.borrow_mut().push(this.inner.clone()));
            }
            Ok(())
        });

        m.add_method("Pause", |_, this, _: ()| -> mlua::Result<()> {
            let mut inner = this.inner.lock().unwrap();
            if inner.state == TweenState::Playing {
                inner.state = TweenState::Paused;
            }
            Ok(())
        });

        m.add_method("Resume", |_, this, _: ()| -> mlua::Result<()> {
            let mut inner = this.inner.lock().unwrap();
            if inner.state == TweenState::Paused {
                inner.state = TweenState::Playing;
            }
            Ok(())
        });

        m.add_method("Cancel", |_, this, _: ()| -> mlua::Result<()> {
            let mut inner = this.inner.lock().unwrap();
            inner.state = TweenState::Cancelled;
            Ok(())
        });

        m.add_method("Wait", |lua, this, _: ()| -> mlua::Result<()> {
            let state = this.inner.lock().unwrap().state;
            if state == TweenState::Completed || state == TweenState::Cancelled {
                return Ok(());
            }
            let signal = this.completed.clone();
            let wait: mlua::Function = signal.get("Wait")?;
            wait.call::<MultiValue>(Value::Table(signal))?;
            let _ = lua;
            Ok(())
        });
    }
}

fn parse_goal(property: &str, value: Value) -> mlua::Result<GoalValue> {
    match property {
        "CFrame" => match value {
            Value::UserData(ud) => Ok(GoalValue::CFrame(*ud.borrow::<CFrame>().map_err(|_| {
                mlua::Error::RuntimeError("TweenService: CFrame goal must be a CFrame".into())
            })?)),
            _ => Err(mlua::Error::RuntimeError(
                "TweenService: CFrame goal must be a CFrame".into(),
            )),
        },
        "Color" => match value {
            Value::UserData(ud) => Ok(GoalValue::Color(*ud.borrow::<Color3>().map_err(|_| {
                mlua::Error::RuntimeError("TweenService: Color goal must be a Color3".into())
            })?)),
            _ => Err(mlua::Error::RuntimeError(
                "TweenService: Color goal must be a Color3".into(),
            )),
        },
        "Size" | "Position" => match value {
            Value::UserData(ud) => {
                if let Ok(d) = ud.borrow::<Dim>() {
                    return Ok(GoalValue::Dim(*d));
                }
                if let Ok(v) = ud.borrow::<Vector>() {
                    return Ok(GoalValue::Vector(*v));
                }
                Err(mlua::Error::RuntimeError(format!(
                    "TweenService: {property} goal must be a Vector (BasePart) or a Dim (GUI)"
                )))
            }
            _ => Err(mlua::Error::RuntimeError(format!(
                "TweenService: {property} goal must be a Vector (BasePart) or a Dim (GUI)"
            ))),
        },
        "Transparency" | "ZIndex" | "Volume" | "Pitch" | "Speed" | "Pan" | "Distortion"
        | "MinFalloff" | "MaxFalloff" => match value {
            Value::Integer(n) => Ok(GoalValue::Number(n as f32)),
            Value::Number(n) => Ok(GoalValue::Number(n as f32)),
            _ => Err(mlua::Error::RuntimeError(format!(
                "TweenService: {property} goal must be a number"
            ))),
        },
        other => Err(mlua::Error::RuntimeError(format!(
            "TweenService: unsupported property '{other}' (BasePart: CFrame, Color, Size — GUI: Position, Size, Color, Transparency, ZIndex — Sound: Volume, Pitch, Pan, Distortion, Position, MinFalloff, MaxFalloff — VoiceChannel: Volume, Pitch, Position, MinFalloff, MaxFalloff)"
        ))),
    }
}

fn create_tween(
    lua: &Lua,
    args: MultiValue,
) -> mlua::Result<TweenHandle> {
    let mut iter = args.into_iter();
    let target_v = iter.next().ok_or_else(|| {
        mlua::Error::RuntimeError("TweenService.new: missing target".into())
    })?;
    let time_v = iter.next().ok_or_else(|| {
        mlua::Error::RuntimeError("TweenService.new: missing duration (number)".into())
    })?;
    let style_v = iter.next().unwrap_or(Value::Nil);
    let next_v = iter.next().unwrap_or(Value::Nil);
    let last_v = iter.next().unwrap_or(Value::Nil);

    let (direction_v, props_v) = match (next_v, last_v) {
        (Value::Table(t), Value::Nil) => (Value::Nil, Value::Table(t)),
        (direction, props) => (direction, props),
    };

    let duration: f32 = match time_v {
        Value::Integer(n) => n as f32,
        Value::Number(n) => n as f32,
        _ => {
            return Err(mlua::Error::RuntimeError(
                "TweenService.new: duration must be a number".into(),
            ));
        }
    };
    if duration <= 0.0 {
        return Err(mlua::Error::RuntimeError(
            "TweenService.new: duration must be > 0".into(),
        ));
    }
    let family = parse_style_value(style_v)?;
    let direction = parse_direction_value(direction_v)?;

    let props_table = match props_v {
        Value::Table(t) => t,
        _ => {
            return Err(mlua::Error::RuntimeError(
                "TweenService.new: properties must be a table".into(),
            ));
        }
    };

    let target = match &target_v {
        Value::UserData(ud) => {
            if let Ok(part) = ud.borrow::<PartHandle>() {
                TweenTarget::Part(part.state.clone())
            } else if let Ok(gui) = ud.borrow::<GuiPrimitive>() {
                TweenTarget::Gui(gui.state_arc())
            } else if let Ok(snd) = ud.borrow::<crate::libs::sfx::Sound>() {
                TweenTarget::Sound {
                    shaders: snd.shaders.clone(),
                    position: snd.position.clone(),
                }
            } else if let Ok(vc) = ud.borrow::<crate::libs::voice::ChannelHandle>() {
                TweenTarget::Voice {
                    shaders: vc.shaders.clone(),
                    position: vc.position.clone(),
                }
            } else {
                return Err(mlua::Error::RuntimeError(
                    "TweenService.new: target must be a BasePart, GUI primitive, Sound, or VoiceChannel".into(),
                ));
            }
        }
        _ => {
            return Err(mlua::Error::RuntimeError(
                "TweenService.new: target must be a userdata (BasePart, GUI primitive, Sound, or VoiceChannel)".into(),
            ));
        }
    };

    let mut goals: Vec<GoalSpec> = Vec::new();
    for pair in props_table.pairs::<String, Value>() {
        let (k, v) = pair?;
        let g = parse_goal(&k, v)?;
        goals.push(GoalSpec {
            property: k,
            value: g,
        });
    }
    if goals.is_empty() {
        return Err(mlua::Error::RuntimeError(
            "TweenService.new: properties table is empty".into(),
        ));
    }

    let signal = signal::new_instance(lua)?;
    let inner = Arc::new(Mutex::new(TweenInner {
        duration,
        family,
        direction,
        state: TweenState::Idle,
        elapsed: 0.0,
        target,
        goals,
        properties: Vec::new(),
        completed_signal: signal.clone(),
    }));
    Ok(TweenHandle {
        inner,
        completed: signal,
    })
}

fn lerp_value(lua: &Lua, args: MultiValue) -> mlua::Result<Value> {
    let mut iter = args.into_iter();
    let a = iter
        .next()
        .ok_or_else(|| mlua::Error::RuntimeError("TweenService.Lerp: missing first argument (start value)".into()))?;
    let b = iter
        .next()
        .ok_or_else(|| mlua::Error::RuntimeError("TweenService.Lerp: missing second argument (goal value)".into()))?;
    let alpha_v = iter
        .next()
        .ok_or_else(|| mlua::Error::RuntimeError("TweenService.Lerp: missing third argument (alpha)".into()))?;
    let style = iter.next().unwrap_or(Value::Nil);

    let direction_v = iter.next().unwrap_or(Value::Nil);

    let alpha = match alpha_v {
        Value::Integer(n) => n as f32,
        Value::Number(n) => n as f32,
        _ => {
            return Err(mlua::Error::RuntimeError(
                "TweenService.Lerp: alpha must be a number".into(),
            ));
        }
    };
    let family = parse_style_value(style)?;
    let direction = parse_direction_value(direction_v)?;
    let eased = ease(family, direction, alpha);

    match (&a, &b) {
        (Value::Integer(_) | Value::Number(_), Value::Integer(_) | Value::Number(_)) => {
            let af = match a {
                Value::Integer(n) => n as f32,
                Value::Number(n) => n as f32,
                _ => unreachable!(),
            };
            let bf = match b {
                Value::Integer(n) => n as f32,
                Value::Number(n) => n as f32,
                _ => unreachable!(),
            };
            Ok(Value::Number(lerp_f(af, bf, eased) as f64))
        }
        (Value::UserData(ua), Value::UserData(ub)) => {
            if let (Ok(va), Ok(vb)) = (ua.borrow::<Vector>(), ub.borrow::<Vector>()) {
                let r = lerp_vec(*va, *vb, eased);
                return Ok(Value::UserData(lua.create_userdata(r)?));
            }
            if let (Ok(ca), Ok(cb)) = (ua.borrow::<Color3>(), ub.borrow::<Color3>()) {
                let r = lerp_color(*ca, *cb, eased);
                return Ok(Value::UserData(lua.create_userdata(r)?));
            }
            if let (Ok(ca), Ok(cb)) = (ua.borrow::<CFrame>(), ub.borrow::<CFrame>()) {
                let r = lerp_cframe(*ca, *cb, eased);
                return Ok(Value::UserData(lua.create_userdata(r)?));
            }
            if let (Ok(da), Ok(db)) = (ua.borrow::<Dim>(), ub.borrow::<Dim>()) {
                let r = lerp_dim(*da, *db, eased);
                return Ok(Value::UserData(lua.create_userdata(r)?));
            }
            Err(mlua::Error::RuntimeError(
                "TweenService.Lerp: unsupported userdata pair (supported: Vector, Color3, CFrame, Dim)".into(),
            ))
        }
        _ => Err(mlua::Error::RuntimeError(
            "TweenService.Lerp: start and goal must be the same type (number, Vector, Color3, CFrame, or Dim)".into(),
        )),
    }
}

pub fn create(lua: &Lua) -> mlua::Result<Table> {
    let t = lua.create_table()?;
    t.set("new", lua.create_function(create_tween)?)?;
    t.set("Create", lua.create_function(create_tween)?)?;
    t.set("Lerp", lua.create_function(lerp_value)?)?;
    Ok(t)
}
