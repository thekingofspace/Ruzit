use std::cell::RefCell;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use mlua::{AnyUserData, Lua, Table, UserData, UserDataFields, UserDataMethods};

use crate::libs::asset;
use crate::libs::gui::{GuiPrimitive, PrimitiveState, Shape};
use crate::libs::renderable::{DynTextureBuffer, PartTextureRef};

static NEXT_DYNIMG_ID: AtomicU64 = AtomicU64::new(1);

thread_local! {
    static DYNIMG_REGISTRY: RefCell<Vec<Arc<Mutex<DynImgState>>>> = const { RefCell::new(Vec::new()) };
}

pub struct DynImgState {
    pub id: u64,
    pub texture_id: u64,
    pub width: u32,
    pub height: u32,
    pub primitives: Vec<Arc<Mutex<PrimitiveState>>>,
    pub buffer: Arc<Mutex<DynTextureBuffer>>,
    pub alive: bool,
}

pub struct DynImgHandle {
    pub inner: Arc<Mutex<DynImgState>>,
}

impl UserData for DynImgHandle {
    fn add_fields<F: UserDataFields<Self>>(f: &mut F) {
        f.add_field_method_get("Width", |_, this| {
            Ok(this.inner.lock().unwrap().width as i64)
        });
        f.add_field_method_get("Height", |_, this| {
            Ok(this.inner.lock().unwrap().height as i64)
        });
        f.add_field_method_get("IsAlive", |_, this| Ok(this.inner.lock().unwrap().alive));
    }

    fn add_methods<M: UserDataMethods<Self>>(m: &mut M) {
        m.add_method("Add", |_, this, prim: AnyUserData| -> mlua::Result<()> {
            let p = prim.borrow::<GuiPrimitive>().map_err(|_| {
                mlua::Error::RuntimeError("DynImg:Add expects a GUI Primitive".into())
            })?;
            let prim_state = p.state_arc();
            let mut s = this.inner.lock().unwrap();
            if !s.alive {
                return Ok(());
            }
            {
                let mut ps = prim_state.lock().unwrap();
                ps.dyn_img_owner = Some(s.id);
                ps.visible = false;
            }
            if !s
                .primitives
                .iter()
                .any(|p| Arc::as_ptr(p) == Arc::as_ptr(&prim_state))
            {
                s.primitives.push(prim_state);
            }
            Ok(())
        });
        m.add_method(
            "Remove",
            |_, this, prim: AnyUserData| -> mlua::Result<bool> {
                let p = prim.borrow::<GuiPrimitive>().map_err(|_| {
                    mlua::Error::RuntimeError("DynImg:Remove expects a GUI Primitive".into())
                })?;
                let prim_state = p.state_arc();
                let mut s = this.inner.lock().unwrap();
                let before = s.primitives.len();
                s.primitives
                    .retain(|q| Arc::as_ptr(q) != Arc::as_ptr(&prim_state));
                let removed = s.primitives.len() != before;
                if removed {
                    let mut ps = prim_state.lock().unwrap();
                    if ps.dyn_img_owner == Some(s.id) {
                        ps.dyn_img_owner = None;
                    }
                }
                Ok(removed)
            },
        );
        m.add_method("Destroy", |_, this, _: ()| -> mlua::Result<()> {
            let mut s = this.inner.lock().unwrap();
            s.alive = false;
            for prim in &s.primitives {
                let mut ps = prim.lock().unwrap();
                if ps.dyn_img_owner == Some(s.id) {
                    ps.dyn_img_owner = None;
                }
            }
            s.primitives.clear();
            Ok(())
        });
    }
}

pub fn dynimg_to_part_texture(dyn_img: &DynImgHandle) -> PartTextureRef {
    let s = dyn_img.inner.lock().unwrap();
    PartTextureRef {
        id: s.texture_id,
        width: s.width,
        height: s.height,
        data: Arc::new(Vec::new()),
        version: 0,
        live: Some(s.buffer.clone()),
    }
}

pub fn create(lua: &Lua) -> mlua::Result<Table> {
    let t = lua.create_table()?;
    t.set(
        "New",
        lua.create_function(
            |lua, (width, height, prims): (u32, u32, Option<Table>)| -> mlua::Result<AnyUserData> {
                if width == 0 || height == 0 {
                    return Err(mlua::Error::RuntimeError(
                        "DynImg.New: width and height must be > 0".into(),
                    ));
                }
                let id = NEXT_DYNIMG_ID.fetch_add(1, Ordering::Relaxed);
                let texture_id = asset::next_shader_id();
                let bytes = vec![0u8; (width as usize) * (height as usize) * 4];
                let buffer = Arc::new(Mutex::new(DynTextureBuffer {
                    width,
                    height,
                    bytes,
                    version: 1,
                }));
                let state = Arc::new(Mutex::new(DynImgState {
                    id,
                    texture_id,
                    width,
                    height,
                    primitives: Vec::new(),
                    buffer,
                    alive: true,
                }));
                if let Some(list) = prims {
                    for pair in list.pairs::<i64, AnyUserData>() {
                        let (_, ud) = pair?;
                        let p = ud.borrow::<GuiPrimitive>().map_err(|_| {
                            mlua::Error::RuntimeError(
                                "DynImg.New: every entry in the primitives list must be a GUI Primitive".into(),
                            )
                        })?;
                        let prim_state = p.state_arc();
                        {
                            let mut ps = prim_state.lock().unwrap();
                            ps.dyn_img_owner = Some(id);
                            ps.visible = false;
                        }
                        state.lock().unwrap().primitives.push(prim_state);
                    }
                }
                DYNIMG_REGISTRY.with(|c| c.borrow_mut().push(state.clone()));
                lua.create_userdata(DynImgHandle { inner: state })
            },
        )?,
    )?;
    Ok(t)
}

pub fn tick() {
    let snapshot: Vec<Arc<Mutex<DynImgState>>> = DYNIMG_REGISTRY.with(|c| {
        let mut reg = c.borrow_mut();
        reg.retain(|s| s.lock().unwrap().alive);
        reg.iter().cloned().collect()
    });

    for state_arc in snapshot {
        let s = state_arc.lock().unwrap();
        if !s.alive {
            continue;
        }
        let width = s.width;
        let height = s.height;

        struct Layer {
            shape: Shape,
            x0: i32,
            y0: i32,
            w: i32,
            h: i32,
            color: [f32; 3],
            transparency: f32,
            image: Option<Arc<crate::libs::gui::ImageRef>>,
            z: i32,
        }

        let mut layers: Vec<Layer> = Vec::with_capacity(s.primitives.len());
        for prim in &s.primitives {
            let p = prim.lock().unwrap();
            if !p.alive {
                continue;
            }
            let (image, size) = if matches!(p.shape, Shape::Text) {
                let baked = p.text.as_ref().and_then(|t| t.baked.clone());
                let size = match &baked {
                    Some(img) => crate::libs::primitives::Dim::new(
                        img.width as f32,
                        img.height as f32,
                    ),
                    None => crate::libs::primitives::Dim::new(0.0, 0.0),
                };
                (baked, size)
            } else {
                (p.image.clone(), p.size)
            };
            layers.push(Layer {
                shape: p.shape,
                x0: p.position.x as i32,
                y0: p.position.y as i32,
                w: size.x as i32,
                h: size.y as i32,
                color: [p.color.r, p.color.g, p.color.b],
                transparency: p.transparency.clamp(0.0, 1.0),
                image,
                z: p.z_index,
            });
        }
        layers.sort_by_key(|l| l.z);

        drop(s);

        let buffer_arc = state_arc.lock().unwrap().buffer.clone();
        let mut buffer = buffer_arc.lock().unwrap_or_else(|e| e.into_inner());
        let mut bytes = vec![0u8; (width as usize) * (height as usize) * 4];

        for layer in &layers {
            if layer.w <= 0 || layer.h <= 0 {
                continue;
            }
            let x0 = layer.x0.max(0);
            let y0 = layer.y0.max(0);
            let x1 = (layer.x0 + layer.w).min(width as i32);
            let y1 = (layer.y0 + layer.h).min(height as i32);
            if x0 >= x1 || y0 >= y1 {
                continue;
            }
            let layer_alpha = (1.0 - layer.transparency).clamp(0.0, 1.0);
            for py in y0..y1 {
                for px in x0..x1 {
                    let local_x = px - layer.x0;
                    let local_y = py - layer.y0;
                    let (sr, sg, sb, sa) = match (layer.shape, &layer.image) {
                        (Shape::Image, Some(img)) | (_, Some(img)) => sample_rgba(
                            img,
                            local_x,
                            local_y,
                            layer.w as u32,
                            layer.h as u32,
                        ),
                        (_, None) => (
                            (layer.color[0] * 255.0) as u8,
                            (layer.color[1] * 255.0) as u8,
                            (layer.color[2] * 255.0) as u8,
                            255u8,
                        ),
                    };
                    let src_a_norm = (sa as f32 / 255.0) * layer_alpha;
                    if src_a_norm <= 0.0 {
                        continue;
                    }
                    let tint_r = layer.color[0] * (sr as f32 / 255.0);
                    let tint_g = layer.color[1] * (sg as f32 / 255.0);
                    let tint_b = layer.color[2] * (sb as f32 / 255.0);
                    let off = ((py as u32 * width + px as u32) * 4) as usize;
                    let dr = bytes[off] as f32 / 255.0;
                    let dg = bytes[off + 1] as f32 / 255.0;
                    let db = bytes[off + 2] as f32 / 255.0;
                    let da = bytes[off + 3] as f32 / 255.0;
                    let out_a = src_a_norm + da * (1.0 - src_a_norm);
                    let out_r =
                        (tint_r * src_a_norm + dr * da * (1.0 - src_a_norm)) / out_a.max(1e-6);
                    let out_g =
                        (tint_g * src_a_norm + dg * da * (1.0 - src_a_norm)) / out_a.max(1e-6);
                    let out_b =
                        (tint_b * src_a_norm + db * da * (1.0 - src_a_norm)) / out_a.max(1e-6);
                    bytes[off] = (out_r.clamp(0.0, 1.0) * 255.0) as u8;
                    bytes[off + 1] = (out_g.clamp(0.0, 1.0) * 255.0) as u8;
                    bytes[off + 2] = (out_b.clamp(0.0, 1.0) * 255.0) as u8;
                    bytes[off + 3] = (out_a.clamp(0.0, 1.0) * 255.0) as u8;
                }
            }
        }

        buffer.bytes = bytes;
        buffer.version = buffer.version.wrapping_add(1);
    }
}

fn sample_rgba(
    img: &crate::libs::gui::ImageRef,
    local_x: i32,
    local_y: i32,
    rect_w: u32,
    rect_h: u32,
) -> (u8, u8, u8, u8) {
    if img.width == 0 || img.height == 0 || rect_w == 0 || rect_h == 0 {
        return (255, 255, 255, 255);
    }
    let u = (local_x as f32) / (rect_w as f32);
    let v = (local_y as f32) / (rect_h as f32);
    let sx = (u * img.width as f32).clamp(0.0, (img.width - 1) as f32) as u32;
    let sy = (v * img.height as f32).clamp(0.0, (img.height - 1) as f32) as u32;
    let off = ((sy * img.width + sx) * 4) as usize;
    if off + 3 >= img.data.len() {
        return (255, 255, 255, 255);
    }
    (
        img.data[off],
        img.data[off + 1],
        img.data[off + 2],
        img.data[off + 3],
    )
}
