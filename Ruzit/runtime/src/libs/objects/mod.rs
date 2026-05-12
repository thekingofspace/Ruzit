use std::collections::HashMap;
use std::rc::Rc;
use std::sync::{Arc, Mutex};

use mlua::{
    AnyUserData, Function, Lua, MultiValue, RegistryKey, Table, UserData, UserDataFields,
    UserDataMethods, Value,
};

use crate::libs::gui::{
    AttachedShader, BillboardInner, GuiPrimitive, PrimitiveState, Shape as GuiShape, TextState,
};
use crate::libs::primitives::{CFrame, Dim, Vector, value_to_vector_opt};
use crate::libs::renderable::{
    AttachedShader3D, ModelRef, PartHandle, PartShape, PartState, PartTextureRef,
    bump_parts_dirty,
};

#[derive(Clone)]
enum ChildRef {
    Part(Arc<Mutex<PartState>>),
    Gui(Arc<Mutex<PrimitiveState>>),
    Movable(Arc<Mutex<MovableInner>>),
    Sizable(Arc<Mutex<SizableInner>>),
    Clonable(Arc<Mutex<ClonableInner>>),
}

impl ChildRef {
    fn alive(&self) -> bool {
        match self {
            ChildRef::Part(s) => s.lock().map(|g| g.alive).unwrap_or(false),
            ChildRef::Gui(s) => s.lock().map(|g| g.alive).unwrap_or(false),
            ChildRef::Movable(r) => r.lock().unwrap().alive,
            ChildRef::Sizable(r) => r.lock().unwrap().alive,
            ChildRef::Clonable(r) => r.lock().unwrap().alive,
        }
    }
}

fn child_ref_from_userdata(ud: &AnyUserData) -> mlua::Result<ChildRef> {
    if let Ok(p) = ud.borrow::<PartHandle>() {
        return Ok(ChildRef::Part(p.state.clone()));
    }
    if let Ok(g) = ud.borrow::<GuiPrimitive>() {
        return Ok(ChildRef::Gui(g.state_arc()));
    }
    if let Ok(m) = ud.borrow::<Movable>() {
        return Ok(ChildRef::Movable(m.inner.clone()));
    }
    if let Ok(s) = ud.borrow::<Sizable>() {
        return Ok(ChildRef::Sizable(s.inner.clone()));
    }
    if let Ok(c) = ud.borrow::<ClonableContainer>() {
        return Ok(ChildRef::Clonable(c.inner.clone()));
    }
    Err(mlua::Error::RuntimeError(
        "Objects: child must be a BasePart, GUI primitive, Movable, Sizable, or ClonableContainer".into(),
    ))
}

fn child_to_userdata<'a>(lua: &'a Lua, c: &ChildRef) -> mlua::Result<AnyUserData> {
    match c {
        ChildRef::Part(s) => lua.create_userdata(PartHandle::from_state(s.clone())),
        ChildRef::Gui(s) => lua.create_userdata(GuiPrimitive::from_state(s.clone())),
        ChildRef::Movable(r) => lua.create_userdata(Movable { inner: r.clone() }),
        ChildRef::Sizable(r) => lua.create_userdata(Sizable { inner: r.clone() }),
        ChildRef::Clonable(r) => lua.create_userdata(ClonableContainer { inner: r.clone() }),
    }
}

struct ChildEntry {
    inner: ChildRef,
    remove_cb: Option<Rc<RegistryKey>>,
}

fn propagate_pos_3d(child: &ChildRef, dp: Vector, dr: Vector) {
    match child {
        ChildRef::Part(s) => {
            let mut g = s.lock().unwrap();
            g.cframe = CFrame::new(
                Vector::new(
                    g.cframe.position.x + dp.x,
                    g.cframe.position.y + dp.y,
                    g.cframe.position.z + dp.z,
                ),
                Vector::new(
                    g.cframe.rotation.x + dr.x,
                    g.cframe.rotation.y + dr.y,
                    g.cframe.rotation.z + dr.z,
                ),
            );
        }
        ChildRef::Gui(_) => {}
        ChildRef::Movable(rc) => {
            let mut inner = rc.lock().unwrap();
            if inner.mode == MovableMode::ThreeD {
                inner.cframe = CFrame::new(
                    Vector::new(
                        inner.cframe.position.x + dp.x,
                        inner.cframe.position.y + dp.y,
                        inner.cframe.position.z + dp.z,
                    ),
                    Vector::new(
                        inner.cframe.rotation.x + dr.x,
                        inner.cframe.rotation.y + dr.y,
                        inner.cframe.rotation.z + dr.z,
                    ),
                );
            }
            let kids: Vec<ChildRef> =
                inner.children.iter().map(|c| c.inner.clone()).collect();
            drop(inner);
            for c in &kids {
                propagate_pos_3d(c, dp, dr);
            }
        }
        ChildRef::Sizable(rc) => {
            let kids: Vec<ChildRef> =
                rc.lock().unwrap().children.iter().map(|c| c.inner.clone()).collect();
            for c in &kids {
                propagate_pos_3d(c, dp, dr);
            }
        }
        ChildRef::Clonable(rc) => {
            let kids: Vec<ChildRef> =
                rc.lock().unwrap().children.iter().map(|c| c.inner.clone()).collect();
            for c in &kids {
                propagate_pos_3d(c, dp, dr);
            }
        }
    }
}

fn propagate_pos_2d(child: &ChildRef, dx: f32, dy: f32) {
    match child {
        ChildRef::Part(_) => {}
        ChildRef::Gui(s) => {
            let mut g = s.lock().unwrap();
            g.position = Dim::new(g.position.x + dx, g.position.y + dy);
        }
        ChildRef::Movable(rc) => {
            let mut inner = rc.lock().unwrap();
            if inner.mode == MovableMode::TwoD {
                inner.position = Dim::new(inner.position.x + dx, inner.position.y + dy);
            }
            let kids: Vec<ChildRef> =
                inner.children.iter().map(|c| c.inner.clone()).collect();
            drop(inner);
            for c in &kids {
                propagate_pos_2d(c, dx, dy);
            }
        }
        ChildRef::Sizable(rc) => {
            let kids: Vec<ChildRef> =
                rc.lock().unwrap().children.iter().map(|c| c.inner.clone()).collect();
            for c in &kids {
                propagate_pos_2d(c, dx, dy);
            }
        }
        ChildRef::Clonable(rc) => {
            let kids: Vec<ChildRef> =
                rc.lock().unwrap().children.iter().map(|c| c.inner.clone()).collect();
            for c in &kids {
                propagate_pos_2d(c, dx, dy);
            }
        }
    }
}

fn propagate_size_3d(child: &ChildRef, rx: f32, ry: f32, rz: f32) {
    match child {
        ChildRef::Part(s) => {
            let mut g = s.lock().unwrap();
            g.size = Vector::new(g.size.x * rx, g.size.y * ry, g.size.z * rz);
        }
        ChildRef::Gui(_) => {}
        ChildRef::Sizable(rc) => {
            let mut inner = rc.lock().unwrap();
            if inner.mode == SizableMode::ThreeD {
                inner.size_v = Vector::new(
                    inner.size_v.x * rx,
                    inner.size_v.y * ry,
                    inner.size_v.z * rz,
                );
            }
            let kids: Vec<ChildRef> =
                inner.children.iter().map(|c| c.inner.clone()).collect();
            drop(inner);
            for c in &kids {
                propagate_size_3d(c, rx, ry, rz);
            }
        }
        ChildRef::Movable(rc) => {
            let kids: Vec<ChildRef> =
                rc.lock().unwrap().children.iter().map(|c| c.inner.clone()).collect();
            for c in &kids {
                propagate_size_3d(c, rx, ry, rz);
            }
        }
        ChildRef::Clonable(rc) => {
            let kids: Vec<ChildRef> =
                rc.lock().unwrap().children.iter().map(|c| c.inner.clone()).collect();
            for c in &kids {
                propagate_size_3d(c, rx, ry, rz);
            }
        }
    }
}

fn propagate_size_2d(child: &ChildRef, rx: f32, ry: f32) {
    match child {
        ChildRef::Part(_) => {}
        ChildRef::Gui(s) => {
            let mut g = s.lock().unwrap();
            g.size = Dim::new(g.size.x * rx, g.size.y * ry);
        }
        ChildRef::Sizable(rc) => {
            let mut inner = rc.lock().unwrap();
            if inner.mode == SizableMode::TwoD {
                inner.size_d = Dim::new(inner.size_d.x * rx, inner.size_d.y * ry);
            }
            let kids: Vec<ChildRef> =
                inner.children.iter().map(|c| c.inner.clone()).collect();
            drop(inner);
            for c in &kids {
                propagate_size_2d(c, rx, ry);
            }
        }
        ChildRef::Movable(rc) => {
            let kids: Vec<ChildRef> =
                rc.lock().unwrap().children.iter().map(|c| c.inner.clone()).collect();
            for c in &kids {
                propagate_size_2d(c, rx, ry);
            }
        }
        ChildRef::Clonable(rc) => {
            let kids: Vec<ChildRef> =
                rc.lock().unwrap().children.iter().map(|c| c.inner.clone()).collect();
            for c in &kids {
                propagate_size_2d(c, rx, ry);
            }
        }
    }
}

fn run_remove_callback(lua: &Lua, key: &RegistryKey, child_ud: AnyUserData) {
    if let Ok(f) = lua.registry_value::<Function>(key) {
        let _ = f.call::<()>(child_ud);
    }
}

pub struct ClonableContainer {
    inner: Arc<Mutex<ClonableInner>>,
}

struct ClonableInner {
    children: Vec<ChildEntry>,
    alive: bool,
}

impl ClonableContainer {
    fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(ClonableInner {
                children: Vec::new(),
                alive: true,
            })),
        }
    }
}

impl UserData for ClonableContainer {
    fn add_fields<F: UserDataFields<Self>>(f: &mut F) {
        f.add_field_method_get("Alive", |_, this| Ok(this.inner.lock().unwrap().alive));
        f.add_field_method_get("ChildCount", |_, this| {
            Ok(this.inner.lock().unwrap().children.len() as i64)
        });
    }

    fn add_methods<M: UserDataMethods<Self>>(m: &mut M) {
        m.add_method(
            "AddChild",
            |lua, this, args: MultiValue| -> mlua::Result<()> {
                let mut iter = args.into_iter();
                let child_v = iter.next().ok_or_else(|| {
                    mlua::Error::RuntimeError(
                        "ClonableContainer:AddChild expects (child, removeFn?)".into(),
                    )
                })?;
                let remove_v = iter.next().unwrap_or(Value::Nil);

                let child_ud = match child_v {
                    Value::UserData(ud) => ud,
                    _ => {
                        return Err(mlua::Error::RuntimeError(
                            "ClonableContainer:AddChild expects a userdata child".into(),
                        ));
                    }
                };
                let cref = child_ref_from_userdata(&child_ud)?;
                if matches!(cref, ChildRef::Clonable(_)) {
                    return Err(mlua::Error::RuntimeError(
                        "ClonableContainer cannot contain another ClonableContainer".into(),
                    ));
                }
                let key = match remove_v {
                    Value::Nil => None,
                    Value::Function(f) => Some(Rc::new(lua.create_registry_value(f)?)),
                    _ => {
                        return Err(mlua::Error::RuntimeError(
                            "ClonableContainer:AddChild remove arg must be a function".into(),
                        ));
                    }
                };
                this.inner.lock().unwrap().children.push(empty_entry(cref, key));
                Ok(())
            },
        );

        m.add_method(
            "RemoveChild",
            |lua, this, target: AnyUserData| -> mlua::Result<bool> {
                let target_ref = child_ref_from_userdata(&target)?;
                let mut inner = this.inner.lock().unwrap();
                let pos = inner
                    .children
                    .iter()
                    .position(|c| same_ref(&c.inner, &target_ref));
                match pos {
                    Some(i) => {
                        let entry = inner.children.remove(i);
                        drop(inner);
                        if let Some(key) = entry.remove_cb {
                            if let Ok(ud) = child_to_userdata(lua, &entry.inner) {
                                run_remove_callback(lua, &key, ud);
                            }
                        }
                        Ok(true)
                    }
                    None => Ok(false),
                }
            },
        );

        m.add_method("GetChildren", |lua, this, _: ()| -> mlua::Result<Table> {
            let inner = this.inner.lock().unwrap();
            let out = lua.create_table()?;
            let mut i = 1;
            for c in &inner.children {
                if c.inner.alive() {
                    out.set(i, child_to_userdata(lua, &c.inner)?)?;
                    i += 1;
                }
            }
            Ok(out)
        });

        m.add_method("Clone", |lua, this, _: ()| -> mlua::Result<ClonableContainer> {
            let src = this.inner.lock().unwrap();
            let dst = ClonableContainer::new();
            let mut ctx = CloneContext::new();
            {
                let mut dst_inner = dst.inner.lock().unwrap();
                for c in &src.children {
                    if !c.inner.alive() {
                        continue;
                    }
                    let cloned = clone_child(lua, &c.inner, &mut ctx)?;
                    dst_inner.children.push(empty_entry(cloned, None));
                }
            }
            ctx.remap_clip_parents();
            Ok(dst)
        });

        m.add_method("Destroy", |lua, this, _: ()| -> mlua::Result<()> {
            destroy_inner(lua, &mut this.inner.lock().unwrap());
            Ok(())
        });
    }
}

fn empty_entry(inner: ChildRef, remove_cb: Option<Rc<RegistryKey>>) -> ChildEntry {
    ChildEntry { inner, remove_cb }
}

fn same_ref(a: &ChildRef, b: &ChildRef) -> bool {
    match (a, b) {
        (ChildRef::Part(x), ChildRef::Part(y)) => Arc::ptr_eq(x, y),
        (ChildRef::Gui(x), ChildRef::Gui(y)) => Arc::ptr_eq(x, y),
        (ChildRef::Movable(x), ChildRef::Movable(y)) => Arc::ptr_eq(x, y),
        (ChildRef::Sizable(x), ChildRef::Sizable(y)) => Arc::ptr_eq(x, y),
        (ChildRef::Clonable(x), ChildRef::Clonable(y)) => Arc::ptr_eq(x, y),
        _ => false,
    }
}

fn inherit_movable_cframe(c: &ChildRef) -> Option<CFrame> {
    match c {
        ChildRef::Part(s) => Some(s.lock().unwrap().cframe),
        ChildRef::Movable(rc) => {
            let inner = rc.lock().unwrap();
            if inner.mode == MovableMode::ThreeD {
                Some(inner.cframe)
            } else {
                None
            }
        }
        _ => None,
    }
}

fn inherit_movable_position(c: &ChildRef) -> Option<Dim> {
    match c {
        ChildRef::Gui(s) => Some(s.lock().unwrap().position),
        ChildRef::Movable(rc) => {
            let inner = rc.lock().unwrap();
            if inner.mode == MovableMode::TwoD {
                Some(inner.position)
            } else {
                None
            }
        }
        _ => None,
    }
}

fn inherit_sizable_vector(c: &ChildRef) -> Option<Vector> {
    match c {
        ChildRef::Part(s) => Some(s.lock().unwrap().size),
        ChildRef::Sizable(rc) => {
            let inner = rc.lock().unwrap();
            if inner.mode == SizableMode::ThreeD {
                Some(inner.size_v)
            } else {
                None
            }
        }
        _ => None,
    }
}

fn inherit_sizable_dim(c: &ChildRef) -> Option<Dim> {
    match c {
        ChildRef::Gui(s) => Some(s.lock().unwrap().size),
        ChildRef::Sizable(rc) => {
            let inner = rc.lock().unwrap();
            if inner.mode == SizableMode::TwoD {
                Some(inner.size_d)
            } else {
                None
            }
        }
        _ => None,
    }
}

fn mode_for_movable(c: &ChildRef) -> Option<MovableMode> {
    match c {
        ChildRef::Part(_) => Some(MovableMode::ThreeD),
        ChildRef::Gui(_) => Some(MovableMode::TwoD),
        ChildRef::Movable(rc) => match rc.lock().unwrap().mode {
            MovableMode::ThreeD => Some(MovableMode::ThreeD),
            MovableMode::TwoD => Some(MovableMode::TwoD),
            MovableMode::Unset => None,
        },
        ChildRef::Sizable(rc) => match rc.lock().unwrap().mode {
            SizableMode::ThreeD => Some(MovableMode::ThreeD),
            SizableMode::TwoD => Some(MovableMode::TwoD),
            SizableMode::Unset => None,
        },
        ChildRef::Clonable(_) => None,
    }
}

fn mode_for_sizable(c: &ChildRef) -> Option<SizableMode> {
    match c {
        ChildRef::Part(_) => Some(SizableMode::ThreeD),
        ChildRef::Gui(_) => Some(SizableMode::TwoD),
        ChildRef::Sizable(rc) => match rc.lock().unwrap().mode {
            SizableMode::ThreeD => Some(SizableMode::ThreeD),
            SizableMode::TwoD => Some(SizableMode::TwoD),
            SizableMode::Unset => None,
        },
        ChildRef::Movable(rc) => match rc.lock().unwrap().mode {
            MovableMode::ThreeD => Some(SizableMode::ThreeD),
            MovableMode::TwoD => Some(SizableMode::TwoD),
            MovableMode::Unset => None,
        },
        ChildRef::Clonable(_) => None,
    }
}

fn destroy_inner(lua: &Lua, inner: &mut ClonableInner) {
    if !inner.alive {
        return;
    }
    let drained: Vec<ChildEntry> = inner.children.drain(..).collect();
    inner.alive = false;
    for entry in drained {
        if let Some(key) = entry.remove_cb {
            if let Ok(ud) = child_to_userdata(lua, &entry.inner) {
                run_remove_callback(lua, &key, ud);
            }
        }
    }
}

struct CloneContext {
    gui_pairs: Vec<(Arc<Mutex<PrimitiveState>>, Arc<Mutex<PrimitiveState>>)>,
}

impl CloneContext {
    fn new() -> Self {
        Self {
            gui_pairs: Vec::new(),
        }
    }

    fn remap_clip_parents(&self) {
        let mut map: HashMap<usize, Arc<Mutex<PrimitiveState>>> = HashMap::new();
        for (orig, clone) in &self.gui_pairs {
            map.insert(Arc::as_ptr(orig) as usize, clone.clone());
        }
        for (orig, clone) in &self.gui_pairs {
            let original_clip = {
                let s = orig.lock().unwrap();
                s.clip_parent.clone()
            };
            if let Some(parent_arc) = original_clip {
                let key = Arc::as_ptr(&parent_arc) as usize;
                if let Some(cloned_parent) = map.get(&key) {
                    clone.lock().unwrap().clip_parent = Some(cloned_parent.clone());
                }
            }
        }
    }
}

fn clone_child(
    lua: &Lua,
    child: &ChildRef,
    ctx: &mut CloneContext,
) -> mlua::Result<ChildRef> {
    match child {
        ChildRef::Part(state) => {
            let snap = {
                let s = state.lock().unwrap();
                ClonedPartSpec {
                    shape: s.shape,
                    model: s.model.clone(),
                    cframe: s.cframe,
                    size: s.size,
                    color: s.color,
                    render: s.render,
                    cast_shadow: s.cast_shadow,
                    receive_shadow: s.receive_shadow,
                    ignore_raycast: s.ignore_raycast,
                    lit: s.lit,
                    deformed: s.deformed.clone(),
                    texture: s.texture.clone(),
                    attached: s.attached.iter().map(clone_attached_3d).collect(),
                }
            };
            let new_handle = PartHandle::new_shape(lua, snap.shape, snap.model)?;
            {
                let mut s = new_handle.state.lock().unwrap();
                s.cframe = snap.cframe;
                s.size = snap.size;
                s.color = snap.color;
                s.render = snap.render;
                s.cast_shadow = snap.cast_shadow;
                s.receive_shadow = snap.receive_shadow;
                s.ignore_raycast = snap.ignore_raycast;
                s.lit = snap.lit;
                s.deformed = snap.deformed;
                s.texture = snap.texture;
                s.attached = snap.attached;
            }
            bump_parts_dirty();
            Ok(ChildRef::Part(new_handle.state))
        }
        ChildRef::Gui(state) => {
            let snap = {
                let s = state.lock().unwrap();
                ClonedGuiSpec {
                    shape: s.shape,
                    size: s.size,
                    position: s.position,
                    color: s.color,
                    transparency: s.transparency,
                    z_index: s.z_index,
                    visible: s.visible,
                    image: s.image.clone(),
                    text: s.text.as_ref().map(clone_text_state),
                    attached: s.attached.iter().map(clone_attached_2d).collect(),
                }
            };
            let prim = GuiPrimitive::new(lua, snap.shape)?;
            {
                let arc = prim.state_arc();
                let mut s = arc.lock().unwrap();
                s.size = snap.size;
                s.position = snap.position;
                s.color = snap.color;
                s.transparency = snap.transparency;
                s.z_index = snap.z_index;
                s.visible = snap.visible;
                s.image = snap.image;
                s.text = snap.text;
                s.attached = snap.attached;
            }
            ctx.gui_pairs.push((state.clone(), prim.state_arc()));
            Ok(ChildRef::Gui(prim.state_arc()))
        }
        ChildRef::Movable(inner_rc) => {
            let src = inner_rc.lock().unwrap();
            let new_movable = Movable::new();
            {
                let mut dst = new_movable.inner.lock().unwrap();
                dst.mode = src.mode;
                dst.cframe = src.cframe;
                dst.position = src.position;
                for c in &src.children {
                    if !c.inner.alive() {
                        continue;
                    }
                    let cloned = clone_child(lua, &c.inner, ctx)?;
                    dst.children.push(empty_entry(cloned, None));
                }
            }
            Ok(ChildRef::Movable(new_movable.inner))
        }
        ChildRef::Sizable(inner_rc) => {
            let src = inner_rc.lock().unwrap();
            let new_sizable = Sizable::new();
            {
                let mut dst = new_sizable.inner.lock().unwrap();
                dst.mode = src.mode;
                dst.size_v = src.size_v;
                dst.size_d = src.size_d;
                for c in &src.children {
                    if !c.inner.alive() {
                        continue;
                    }
                    let cloned = clone_child(lua, &c.inner, ctx)?;
                    dst.children.push(empty_entry(cloned, None));
                }
            }
            Ok(ChildRef::Sizable(new_sizable.inner))
        }
        ChildRef::Clonable(inner_rc) => {
            let src = inner_rc.lock().unwrap();
            let new_clonable = ClonableContainer::new();
            {
                let mut dst = new_clonable.inner.lock().unwrap();
                for c in &src.children {
                    if !c.inner.alive() {
                        continue;
                    }
                    let cloned = clone_child(lua, &c.inner, ctx)?;
                    dst.children.push(empty_entry(cloned, None));
                }
            }
            Ok(ChildRef::Clonable(new_clonable.inner))
        }
    }
}

struct ClonedPartSpec {
    shape: PartShape,
    model: Option<ModelRef>,
    cframe: CFrame,
    size: Vector,
    color: crate::libs::primitives::Color3,
    render: bool,
    cast_shadow: bool,
    receive_shadow: bool,
    ignore_raycast: bool,
    lit: bool,
    deformed: Option<ModelRef>,
    texture: Option<PartTextureRef>,
    attached: Vec<AttachedShader3D>,
}

struct ClonedGuiSpec {
    shape: GuiShape,
    size: Dim,
    position: Dim,
    color: crate::libs::primitives::Color3,
    transparency: f32,
    z_index: i32,
    visible: bool,
    image: Option<Arc<crate::libs::gui::ImageRef>>,
    text: Option<TextState>,
    attached: Vec<AttachedShader>,
}

fn clone_attached_3d(s: &AttachedShader3D) -> AttachedShader3D {
    let params = *s.params.lock().unwrap();
    AttachedShader3D {
        id: s.id,
        wgsl: s.wgsl.clone(),
        slot_of_name: s.slot_of_name.clone(),
        params: Arc::new(Mutex::new(params)),
    }
}

fn clone_attached_2d(s: &AttachedShader) -> AttachedShader {
    let params = *s.params.lock().unwrap();
    AttachedShader {
        id: s.id,
        source: s.source.clone(),
        wgsl: s.wgsl.clone(),
        slot_of_name: s.slot_of_name.clone(),
        params: Arc::new(Mutex::new(params)),
    }
}

fn clone_text_state(t: &TextState) -> TextState {
    TextState {
        font_id: t.font_id,
        font: t.font.clone(),
        content: t.content.clone(),
        size_px: t.size_px,
        color: t.color,
        baked: None,
    }
}

#[derive(Clone, Copy, PartialEq)]
pub(crate) enum MovableMode {
    Unset,
    ThreeD,
    TwoD,
}

pub struct Movable {
    pub(crate) inner: Arc<Mutex<MovableInner>>,
}

pub(crate) struct MovableInner {
    children: Vec<ChildEntry>,
    alive: bool,
    pub(crate) mode: MovableMode,
    pub(crate) cframe: CFrame,
    pub(crate) position: Dim,
}

impl Movable {
    fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(MovableInner {
                children: Vec::new(),
                alive: true,
                mode: MovableMode::Unset,
                cframe: CFrame::new(
                    Vector::new(0.0, 0.0, 0.0),
                    Vector::new(0.0, 0.0, 0.0),
                ),
                position: Dim::new(0.0, 0.0),
            })),
        }
    }

    fn propagate_3d_delta(inner: &MovableInner, dp: Vector, dr: Vector) {
        let kids: Vec<ChildRef> =
            inner.children.iter().map(|c| c.inner.clone()).collect();
        for c in &kids {
            propagate_pos_3d(c, dp, dr);
        }
        bump_parts_dirty();
    }

    fn propagate_2d_delta(inner: &MovableInner, dx: f32, dy: f32) {
        let kids: Vec<ChildRef> =
            inner.children.iter().map(|c| c.inner.clone()).collect();
        for c in &kids {
            propagate_pos_2d(c, dx, dy);
        }
    }
}

impl UserData for Movable {
    fn add_fields<F: UserDataFields<Self>>(f: &mut F) {
        f.add_field_method_get("Alive", |_, this| Ok(this.inner.lock().unwrap().alive));
        f.add_field_method_get("ChildCount", |_, this| {
            Ok(this.inner.lock().unwrap().children.len() as i64)
        });
        f.add_field_method_get("Mode", |_, this| {
            Ok(match this.inner.lock().unwrap().mode {
                MovableMode::ThreeD => "3D",
                MovableMode::TwoD => "2D",
                MovableMode::Unset => "Unset",
            }
            .to_string())
        });
        f.add_field_method_get("CFrame", |_, this| Ok(this.inner.lock().unwrap().cframe));
        f.add_field_method_set("CFrame", |_, this, value: AnyUserData| {
            let mut inner = this.inner.lock().unwrap();
            if inner.mode == MovableMode::TwoD {
                return Err(mlua::Error::RuntimeError(
                    "Movable: this container is in 2D mode (GUI children). Use .Position instead.".into(),
                ));
            }
            let cf = *value.borrow::<CFrame>().map_err(|_| {
                mlua::Error::RuntimeError("Movable.CFrame expects a CFrame".into())
            })?;
            let dp = Vector::new(
                cf.position.x - inner.cframe.position.x,
                cf.position.y - inner.cframe.position.y,
                cf.position.z - inner.cframe.position.z,
            );
            let dr = Vector::new(
                cf.rotation.x - inner.cframe.rotation.x,
                cf.rotation.y - inner.cframe.rotation.y,
                cf.rotation.z - inner.cframe.rotation.z,
            );
            inner.cframe = cf;
            Movable::propagate_3d_delta(&inner, dp, dr);
            Ok(())
        });
        f.add_field_method_get("Position", |_, this| Ok(this.inner.lock().unwrap().position));
        f.add_field_method_set("Position", |_, this, value: AnyUserData| {
            let mut inner = this.inner.lock().unwrap();
            if inner.mode == MovableMode::ThreeD {
                return Err(mlua::Error::RuntimeError(
                    "Movable: this container is in 3D mode (BasePart children). Use .CFrame instead.".into(),
                ));
            }
            let d = *value.borrow::<Dim>().map_err(|_| {
                mlua::Error::RuntimeError("Movable.Position expects a Dim".into())
            })?;
            let dx = d.x - inner.position.x;
            let dy = d.y - inner.position.y;
            inner.position = d;
            Movable::propagate_2d_delta(&inner, dx, dy);
            Ok(())
        });
    }

    fn add_methods<M: UserDataMethods<Self>>(m: &mut M) {
        m.add_method(
            "AddChild",
            |lua, this, args: MultiValue| -> mlua::Result<()> {
                let mut iter = args.into_iter();
                let child_v = iter.next().ok_or_else(|| {
                    mlua::Error::RuntimeError(
                        "Movable:AddChild expects (child, removeFn?)".into(),
                    )
                })?;
                let remove_v = iter.next().unwrap_or(Value::Nil);

                let child_ud = match child_v {
                    Value::UserData(ud) => ud,
                    _ => {
                        return Err(mlua::Error::RuntimeError(
                            "Movable:AddChild expects a userdata child".into(),
                        ));
                    }
                };
                let cref = child_ref_from_userdata(&child_ud)?;
                let key = match remove_v {
                    Value::Nil => None,
                    Value::Function(f) => Some(Rc::new(lua.create_registry_value(f)?)),
                    _ => {
                        return Err(mlua::Error::RuntimeError(
                            "Movable:AddChild remove arg must be a function".into(),
                        ));
                    }
                };

                let mut inner = this.inner.lock().unwrap();
                let candidate_mode = mode_for_movable(&cref);
                if let Some(cm) = candidate_mode {
                    if inner.mode != MovableMode::Unset && inner.mode != cm {
                        return Err(mlua::Error::RuntimeError(
                            "Movable: cannot mix 2D and 3D children in the same Movable".into(),
                        ));
                    }
                    inner.mode = cm;
                }
                if inner.children.is_empty() {
                    match inner.mode {
                        MovableMode::ThreeD => {
                            if let Some(cf) = inherit_movable_cframe(&cref) {
                                inner.cframe = cf;
                            }
                        }
                        MovableMode::TwoD => {
                            if let Some(p) = inherit_movable_position(&cref) {
                                inner.position = p;
                            }
                        }
                        MovableMode::Unset => {}
                    }
                }
                let entry = empty_entry(cref.clone(), key);
                inner.children.push(entry);
                Ok(())
            },
        );

        m.add_method(
            "RemoveChild",
            |lua, this, target: AnyUserData| -> mlua::Result<bool> {
                let target_ref = child_ref_from_userdata(&target)?;
                let mut inner = this.inner.lock().unwrap();
                let pos = inner
                    .children
                    .iter()
                    .position(|c| same_ref(&c.inner, &target_ref));
                match pos {
                    Some(i) => {
                        let entry = inner.children.remove(i);
                        drop(inner);
                        if let Some(key) = entry.remove_cb {
                            if let Ok(ud) = child_to_userdata(lua, &entry.inner) {
                                run_remove_callback(lua, &key, ud);
                            }
                        }
                        Ok(true)
                    }
                    None => Ok(false),
                }
            },
        );

        m.add_method("GetChildren", |lua, this, _: ()| -> mlua::Result<Table> {
            let inner = this.inner.lock().unwrap();
            let out = lua.create_table()?;
            let mut i = 1;
            for c in &inner.children {
                if c.inner.alive() {
                    out.set(i, child_to_userdata(lua, &c.inner)?)?;
                    i += 1;
                }
            }
            Ok(out)
        });

        m.add_method("Destroy", |lua, this, _: ()| -> mlua::Result<()> {
            let mut inner = this.inner.lock().unwrap();
            if !inner.alive {
                return Ok(());
            }
            let drained: Vec<ChildEntry> = inner.children.drain(..).collect();
            inner.alive = false;
            drop(inner);
            for entry in drained {
                if let Some(key) = entry.remove_cb {
                    if let Ok(ud) = child_to_userdata(lua, &entry.inner) {
                        run_remove_callback(lua, &key, ud);
                    }
                }
            }
            Ok(())
        });
    }
}

#[derive(Clone, Copy, PartialEq)]
pub(crate) enum SizableMode {
    Unset,
    ThreeD,
    TwoD,
}

pub struct Sizable {
    pub(crate) inner: Arc<Mutex<SizableInner>>,
}

pub(crate) struct SizableInner {
    children: Vec<ChildEntry>,
    alive: bool,
    pub(crate) mode: SizableMode,
    pub(crate) size_v: Vector,
    pub(crate) size_d: Dim,
}

impl Sizable {
    fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(SizableInner {
                children: Vec::new(),
                alive: true,
                mode: SizableMode::Unset,
                size_v: Vector::new(1.0, 1.0, 1.0),
                size_d: Dim::new(1.0, 1.0),
            })),
        }
    }

    fn propagate_3d_ratio(inner: &SizableInner, rx: f32, ry: f32, rz: f32) {
        let kids: Vec<ChildRef> =
            inner.children.iter().map(|c| c.inner.clone()).collect();
        for c in &kids {
            propagate_size_3d(c, rx, ry, rz);
        }
        bump_parts_dirty();
    }

    fn propagate_2d_ratio(inner: &SizableInner, rx: f32, ry: f32) {
        let kids: Vec<ChildRef> =
            inner.children.iter().map(|c| c.inner.clone()).collect();
        for c in &kids {
            propagate_size_2d(c, rx, ry);
        }
    }
}

fn safe_div(a: f32, b: f32) -> f32 {
    if b.abs() < 1e-6 { 1.0 } else { a / b }
}

impl UserData for Sizable {
    fn add_fields<F: UserDataFields<Self>>(f: &mut F) {
        f.add_field_method_get("Alive", |_, this| Ok(this.inner.lock().unwrap().alive));
        f.add_field_method_get("ChildCount", |_, this| {
            Ok(this.inner.lock().unwrap().children.len() as i64)
        });
        f.add_field_method_get("Mode", |_, this| {
            Ok(match this.inner.lock().unwrap().mode {
                SizableMode::ThreeD => "3D",
                SizableMode::TwoD => "2D",
                SizableMode::Unset => "Unset",
            }
            .to_string())
        });
        f.add_field_method_get("Size", |lua, this| -> mlua::Result<Value> {
            let inner = this.inner.lock().unwrap();
            match inner.mode {
                SizableMode::TwoD => Ok(Value::UserData(lua.create_userdata(inner.size_d)?)),
                _ => Ok(Value::UserData(lua.create_userdata(inner.size_v)?)),
            }
        });
        f.add_field_method_set("Size", |_, this, value: Value| {
            let mut inner = this.inner.lock().unwrap();
            match &value {
                Value::UserData(ud) => {
                    if let Ok(d) = ud.borrow::<Dim>() {
                        if inner.mode == SizableMode::ThreeD {
                            return Err(mlua::Error::RuntimeError(
                                "Sizable: container is in 3D mode; pass a Vector for Size".into(),
                            ));
                        }
                        let rx = safe_div(d.x, inner.size_d.x);
                        let ry = safe_div(d.y, inner.size_d.y);
                        inner.size_d = *d;
                        Sizable::propagate_2d_ratio(&inner, rx, ry);
                        return Ok(());
                    }
                }
                _ => {}
            }
            if let Some(v) = value_to_vector_opt(&value) {
                if inner.mode == SizableMode::TwoD {
                    return Err(mlua::Error::RuntimeError(
                        "Sizable: container is in 2D mode; pass a Dim for Size".into(),
                    ));
                }
                let rx = safe_div(v.x, inner.size_v.x);
                let ry = safe_div(v.y, inner.size_v.y);
                let rz = safe_div(v.z, inner.size_v.z);
                inner.size_v = v;
                Sizable::propagate_3d_ratio(&inner, rx, ry, rz);
                return Ok(());
            }
            Err(mlua::Error::RuntimeError(
                "Sizable.Size expects a Vector (3D mode) or a Dim (2D mode)".into(),
            ))
        });
    }

    fn add_methods<M: UserDataMethods<Self>>(m: &mut M) {
        m.add_method(
            "AddChild",
            |lua, this, args: MultiValue| -> mlua::Result<()> {
                let mut iter = args.into_iter();
                let child_v = iter.next().ok_or_else(|| {
                    mlua::Error::RuntimeError(
                        "Sizable:AddChild expects (child, removeFn?)".into(),
                    )
                })?;
                let remove_v = iter.next().unwrap_or(Value::Nil);

                let child_ud = match child_v {
                    Value::UserData(ud) => ud,
                    _ => {
                        return Err(mlua::Error::RuntimeError(
                            "Sizable:AddChild expects a userdata child".into(),
                        ));
                    }
                };
                let cref = child_ref_from_userdata(&child_ud)?;
                let key = match remove_v {
                    Value::Nil => None,
                    Value::Function(f) => Some(Rc::new(lua.create_registry_value(f)?)),
                    _ => {
                        return Err(mlua::Error::RuntimeError(
                            "Sizable:AddChild remove arg must be a function".into(),
                        ));
                    }
                };

                let mut inner = this.inner.lock().unwrap();
                let candidate_mode = mode_for_sizable(&cref);
                if let Some(cm) = candidate_mode {
                    if inner.mode != SizableMode::Unset && inner.mode != cm {
                        return Err(mlua::Error::RuntimeError(
                            "Sizable: cannot mix 2D and 3D children in the same Sizable".into(),
                        ));
                    }
                    inner.mode = cm;
                }
                if inner.children.is_empty() {
                    match inner.mode {
                        SizableMode::ThreeD => {
                            if let Some(v) = inherit_sizable_vector(&cref) {
                                inner.size_v = v;
                            }
                        }
                        SizableMode::TwoD => {
                            if let Some(d) = inherit_sizable_dim(&cref) {
                                inner.size_d = d;
                            }
                        }
                        SizableMode::Unset => {}
                    }
                }
                let entry = empty_entry(cref.clone(), key);
                inner.children.push(entry);
                Ok(())
            },
        );

        m.add_method(
            "RemoveChild",
            |lua, this, target: AnyUserData| -> mlua::Result<bool> {
                let target_ref = child_ref_from_userdata(&target)?;
                let mut inner = this.inner.lock().unwrap();
                let pos = inner
                    .children
                    .iter()
                    .position(|c| same_ref(&c.inner, &target_ref));
                match pos {
                    Some(i) => {
                        let entry = inner.children.remove(i);
                        drop(inner);
                        if let Some(key) = entry.remove_cb {
                            if let Ok(ud) = child_to_userdata(lua, &entry.inner) {
                                run_remove_callback(lua, &key, ud);
                            }
                        }
                        Ok(true)
                    }
                    None => Ok(false),
                }
            },
        );

        m.add_method("GetChildren", |lua, this, _: ()| -> mlua::Result<Table> {
            let inner = this.inner.lock().unwrap();
            let out = lua.create_table()?;
            let mut i = 1;
            for c in &inner.children {
                if c.inner.alive() {
                    out.set(i, child_to_userdata(lua, &c.inner)?)?;
                    i += 1;
                }
            }
            Ok(out)
        });

        m.add_method("Destroy", |lua, this, _: ()| -> mlua::Result<()> {
            let mut inner = this.inner.lock().unwrap();
            if !inner.alive {
                return Ok(());
            }
            let drained: Vec<ChildEntry> = inner.children.drain(..).collect();
            inner.alive = false;
            drop(inner);
            for entry in drained {
                if let Some(key) = entry.remove_cb {
                    if let Ok(ud) = child_to_userdata(lua, &entry.inner) {
                        run_remove_callback(lua, &key, ud);
                    }
                }
            }
            Ok(())
        });
    }
}

pub(crate) fn movable_tween_set_cframe(inner_arc: &Arc<Mutex<MovableInner>>, new_cf: CFrame) {
    let mut inner = inner_arc.lock().unwrap();
    if !inner.alive || inner.mode == MovableMode::TwoD {
        return;
    }
    let dp = Vector::new(
        new_cf.position.x - inner.cframe.position.x,
        new_cf.position.y - inner.cframe.position.y,
        new_cf.position.z - inner.cframe.position.z,
    );
    let dr = Vector::new(
        new_cf.rotation.x - inner.cframe.rotation.x,
        new_cf.rotation.y - inner.cframe.rotation.y,
        new_cf.rotation.z - inner.cframe.rotation.z,
    );
    inner.cframe = new_cf;
    Movable::propagate_3d_delta(&inner, dp, dr);
}

pub(crate) fn movable_tween_set_position(inner_arc: &Arc<Mutex<MovableInner>>, new_pos: Dim) {
    let mut inner = inner_arc.lock().unwrap();
    if !inner.alive || inner.mode == MovableMode::ThreeD {
        return;
    }
    let dx = new_pos.x - inner.position.x;
    let dy = new_pos.y - inner.position.y;
    inner.position = new_pos;
    Movable::propagate_2d_delta(&inner, dx, dy);
}

pub(crate) fn sizable_tween_set_size_3d(inner_arc: &Arc<Mutex<SizableInner>>, new_size: Vector) {
    let mut inner = inner_arc.lock().unwrap();
    if !inner.alive || inner.mode == SizableMode::TwoD {
        return;
    }
    let rx = safe_div(new_size.x, inner.size_v.x);
    let ry = safe_div(new_size.y, inner.size_v.y);
    let rz = safe_div(new_size.z, inner.size_v.z);
    inner.size_v = new_size;
    Sizable::propagate_3d_ratio(&inner, rx, ry, rz);
}

pub(crate) fn sizable_tween_set_size_2d(inner_arc: &Arc<Mutex<SizableInner>>, new_size: Dim) {
    let mut inner = inner_arc.lock().unwrap();
    if !inner.alive || inner.mode == SizableMode::ThreeD {
        return;
    }
    let rx = safe_div(new_size.x, inner.size_d.x);
    let ry = safe_div(new_size.y, inner.size_d.y);
    inner.size_d = new_size;
    Sizable::propagate_2d_ratio(&inner, rx, ry);
}

pub struct Billboard {
    pub(crate) inner: Arc<Mutex<BillboardInner>>,
}

impl Billboard {
    fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(BillboardInner {
                position: Vector::new(0.0, 0.0, 0.0),
                size: Dim::new(300.0, 300.0),
                scale_with_camera: false,
                alive: true,
                children: Vec::new(),
            })),
        }
    }
}

impl UserData for Billboard {
    fn add_fields<F: UserDataFields<Self>>(f: &mut F) {
        f.add_field_method_get("Alive", |_, this| Ok(this.inner.lock().unwrap().alive));
        f.add_field_method_get("ChildCount", |_, this| {
            let mut inner = this.inner.lock().unwrap();
            inner.children.retain(|w| w.upgrade().is_some());
            Ok(inner.children.len() as i64)
        });

        f.add_field_method_get("Position", |_, this| Ok(this.inner.lock().unwrap().position));
        f.add_field_method_set("Position", |_, this, v: Vector| {
            this.inner.lock().unwrap().position = v;
            Ok(())
        });

        f.add_field_method_get("Size", |_, this| Ok(this.inner.lock().unwrap().size));
        f.add_field_method_set("Size", |_, this, ud: AnyUserData| {
            let d = *ud.borrow::<Dim>().map_err(|_| {
                mlua::Error::RuntimeError("Billboard.Size expects a Dim".into())
            })?;
            this.inner.lock().unwrap().size = d;
            Ok(())
        });

        f.add_field_method_get("ScaleWithCamera", |_, this| {
            Ok(this.inner.lock().unwrap().scale_with_camera)
        });
        f.add_field_method_set("ScaleWithCamera", |_, this, v: bool| {
            this.inner.lock().unwrap().scale_with_camera = v;
            Ok(())
        });
    }

    fn add_methods<M: UserDataMethods<Self>>(m: &mut M) {
        m.add_method("AddChild", |_, this, child: AnyUserData| -> mlua::Result<()> {
            let gp = child.borrow::<GuiPrimitive>().map_err(|_| {
                mlua::Error::RuntimeError(
                    "Billboard:AddChild expects a GUI primitive".into(),
                )
            })?;
            let state_arc = gp.state_arc();
            {
                let mut s = state_arc.lock().unwrap();
                if let Some(existing) = &s.billboard_parent {
                    if !Arc::ptr_eq(existing, &this.inner) {
                        return Err(mlua::Error::RuntimeError(
                            "Billboard:AddChild: primitive is already linked to a different Billboard"
                                .into(),
                        ));
                    }
                    return Ok(());
                }
                s.billboard_parent = Some(this.inner.clone());
            }
            let mut inner = this.inner.lock().unwrap();
            inner.children.retain(|w| {
                w.upgrade()
                    .map(|a| !Arc::ptr_eq(&a, &state_arc))
                    .unwrap_or(false)
            });
            inner.children.push(Arc::downgrade(&state_arc));
            Ok(())
        });

        m.add_method(
            "RemoveChild",
            |_, this, child: AnyUserData| -> mlua::Result<()> {
                let gp = child.borrow::<GuiPrimitive>().map_err(|_| {
                    mlua::Error::RuntimeError(
                        "Billboard:RemoveChild expects a GUI primitive".into(),
                    )
                })?;
                let state_arc = gp.state_arc();
                {
                    let mut s = state_arc.lock().unwrap();
                    if let Some(existing) = &s.billboard_parent {
                        if Arc::ptr_eq(existing, &this.inner) {
                            s.billboard_parent = None;
                        }
                    }
                }
                let mut inner = this.inner.lock().unwrap();
                inner.children.retain(|w| {
                    w.upgrade()
                        .map(|a| !Arc::ptr_eq(&a, &state_arc))
                        .unwrap_or(false)
                });
                Ok(())
            },
        );

        m.add_method("GetChildren", |lua, this, _: ()| -> mlua::Result<Table> {
            let inner = this.inner.lock().unwrap();
            let out = lua.create_table()?;
            let mut i = 1;
            for w in inner.children.iter() {
                if let Some(state_arc) = w.upgrade() {
                    let gp = GuiPrimitive::from_state(state_arc);
                    out.set(i, gp)?;
                    i += 1;
                }
            }
            Ok(out)
        });

        m.add_method("Destroy", |_, this, _: ()| -> mlua::Result<()> {
            let mut inner = this.inner.lock().unwrap();
            inner.alive = false;
            for w in inner.children.drain(..) {
                if let Some(state_arc) = w.upgrade() {
                    state_arc.lock().unwrap().billboard_parent = None;
                }
            }
            Ok(())
        });
    }
}

pub fn create(lua: &Lua) -> mlua::Result<Table> {
    let t = lua.create_table()?;
    t.set(
        "ClonableContainer",
        lua.create_function(|_, _: ()| Ok(ClonableContainer::new()))?,
    )?;
    t.set(
        "Movable",
        lua.create_function(|_, _: ()| Ok(Movable::new()))?,
    )?;
    t.set(
        "Sizable",
        lua.create_function(|_, _: ()| Ok(Sizable::new()))?,
    )?;
    t.set(
        "Billboard",
        lua.create_function(|_, _: ()| Ok(Billboard::new()))?,
    )?;
    Ok(t)
}
