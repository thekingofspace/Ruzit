use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use mlua::{AnyUserData, Lua, UserData, UserDataMethods, Value};

use crate::libs::asset;
use crate::libs::primitives::Color3;
use crate::libs::renderable::{DynTextureBuffer, PartTextureRef};

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

pub struct DrawableImgState {
    pub id: u64,
    pub texture_id: u64,
    pub width: u32,
    pub height: u32,
    pub buffer: Arc<Mutex<DynTextureBuffer>>,
    pub source: String,
    pub alive: bool,
}

pub struct DrawableImgHandle {
    pub inner: Arc<Mutex<DrawableImgState>>,
}

pub struct CanvasBufferState {
    pub width: u32,
    pub height: u32,
    pub bytes: Vec<u8>,
}

pub struct CanvasBufferHandle {
    pub inner: Arc<Mutex<CanvasBufferState>>,
}

pub fn new_drawable(width: u32, height: u32) -> Arc<Mutex<DrawableImgState>> {
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    let texture_id = asset::next_shader_id();
    let bytes = vec![0u8; (width as usize) * (height as usize) * 4];
    let buffer = Arc::new(Mutex::new(DynTextureBuffer {
        width,
        height,
        bytes,
        version: 1,
    }));
    Arc::new(Mutex::new(DrawableImgState {
        id,
        texture_id,
        width,
        height,
        buffer,
        source: format!("<drawable:{width}x{height}>"),
        alive: true,
    }))
}

pub fn new_canvas_buffer(width: u32, height: u32) -> Arc<Mutex<CanvasBufferState>> {
    let bytes = vec![0u8; (width as usize) * (height as usize) * 4];
    Arc::new(Mutex::new(CanvasBufferState {
        width,
        height,
        bytes,
    }))
}

pub fn drawable_to_part_texture(d: &DrawableImgHandle) -> PartTextureRef {
    let s = d.inner.lock().unwrap();
    PartTextureRef {
        id: s.texture_id,
        width: s.width,
        height: s.height,
        data: Arc::new(Vec::new()),
        version: 0,
        live: Some(s.buffer.clone()),
    }
}

fn color_from_value(v: &Value, ctx: &str) -> mlua::Result<Color3> {
    match v {
        Value::UserData(ud) => Ok(*ud.borrow::<Color3>().map_err(|_| {
            mlua::Error::RuntimeError(format!("{ctx}: expected a Color3"))
        })?),
        _ => Err(mlua::Error::RuntimeError(format!(
            "{ctx}: expected a Color3"
        ))),
    }
}

fn put_pixel_blend(bytes: &mut [u8], w: u32, h: u32, x: i32, y: i32, c: Color3, alpha: f32) {
    if alpha <= 0.0 || x < 0 || y < 0 || x >= w as i32 || y >= h as i32 {
        return;
    }
    let off = ((y as u32 * w + x as u32) * 4) as usize;
    if off + 3 >= bytes.len() {
        return;
    }
    let dr = bytes[off] as f32 / 255.0;
    let dg = bytes[off + 1] as f32 / 255.0;
    let db = bytes[off + 2] as f32 / 255.0;
    let da = bytes[off + 3] as f32 / 255.0;
    let out_a = alpha + da * (1.0 - alpha);
    let inv = if out_a > 1e-6 { 1.0 / out_a } else { 0.0 };
    let out_r = (c.r * alpha + dr * da * (1.0 - alpha)) * inv;
    let out_g = (c.g * alpha + dg * da * (1.0 - alpha)) * inv;
    let out_b = (c.b * alpha + db * da * (1.0 - alpha)) * inv;
    bytes[off] = (out_r.clamp(0.0, 1.0) * 255.0) as u8;
    bytes[off + 1] = (out_g.clamp(0.0, 1.0) * 255.0) as u8;
    bytes[off + 2] = (out_b.clamp(0.0, 1.0) * 255.0) as u8;
    bytes[off + 3] = (out_a.clamp(0.0, 1.0) * 255.0) as u8;
}

fn put_pixel_set(bytes: &mut [u8], w: u32, h: u32, x: i32, y: i32, c: Color3, alpha: f32) {
    if x < 0 || y < 0 || x >= w as i32 || y >= h as i32 {
        return;
    }
    let off = ((y as u32 * w + x as u32) * 4) as usize;
    if off + 3 >= bytes.len() {
        return;
    }
    bytes[off] = (c.r.clamp(0.0, 1.0) * 255.0) as u8;
    bytes[off + 1] = (c.g.clamp(0.0, 1.0) * 255.0) as u8;
    bytes[off + 2] = (c.b.clamp(0.0, 1.0) * 255.0) as u8;
    bytes[off + 3] = (alpha.clamp(0.0, 1.0) * 255.0) as u8;
}

fn fill_rect(
    bytes: &mut [u8],
    w: u32,
    h: u32,
    x: i32,
    y: i32,
    rw: i32,
    rh: i32,
    c: Color3,
    alpha: f32,
) {
    if rw <= 0 || rh <= 0 || alpha <= 0.0 {
        return;
    }
    let x0 = x.max(0);
    let y0 = y.max(0);
    let x1 = (x + rw).min(w as i32);
    let y1 = (y + rh).min(h as i32);
    for py in y0..y1 {
        for px in x0..x1 {
            put_pixel_blend(bytes, w, h, px, py, c, alpha);
        }
    }
}

fn draw_line(
    bytes: &mut [u8],
    w: u32,
    h: u32,
    x0: i32,
    y0: i32,
    x1: i32,
    y1: i32,
    c: Color3,
    alpha: f32,
) {
    let mut x = x0;
    let mut y = y0;
    let dx = (x1 - x0).abs();
    let dy = -(y1 - y0).abs();
    let sx = if x0 < x1 { 1 } else { -1 };
    let sy = if y0 < y1 { 1 } else { -1 };
    let mut err = dx + dy;
    loop {
        put_pixel_blend(bytes, w, h, x, y, c, alpha);
        if x == x1 && y == y1 {
            break;
        }
        let e2 = 2 * err;
        if e2 >= dy {
            if x == x1 {
                break;
            }
            err += dy;
            x += sx;
        }
        if e2 <= dx {
            if y == y1 {
                break;
            }
            err += dx;
            y += sy;
        }
    }
}

fn draw_circle(
    bytes: &mut [u8],
    w: u32,
    h: u32,
    cx: i32,
    cy: i32,
    radius: i32,
    c: Color3,
    alpha: f32,
    filled: bool,
) {
    if radius <= 0 {
        if radius == 0 {
            put_pixel_blend(bytes, w, h, cx, cy, c, alpha);
        }
        return;
    }
    if filled {
        let r2 = radius * radius;
        for py in -radius..=radius {
            for px in -radius..=radius {
                if px * px + py * py <= r2 {
                    put_pixel_blend(bytes, w, h, cx + px, cy + py, c, alpha);
                }
            }
        }
        return;
    }
    let mut x = radius;
    let mut y = 0;
    let mut err = 1 - x;
    while x >= y {
        for &(px, py) in &[
            (cx + x, cy + y),
            (cx + y, cy + x),
            (cx - y, cy + x),
            (cx - x, cy + y),
            (cx - x, cy - y),
            (cx - y, cy - x),
            (cx + y, cy - x),
            (cx + x, cy - y),
        ] {
            put_pixel_blend(bytes, w, h, px, py, c, alpha);
        }
        y += 1;
        if err < 0 {
            err += 2 * y + 1;
        } else {
            x -= 1;
            err += 2 * (y - x + 1);
        }
    }
}

fn clear_to(bytes: &mut [u8], c: Color3, alpha: f32) {
    let r = (c.r.clamp(0.0, 1.0) * 255.0) as u8;
    let g = (c.g.clamp(0.0, 1.0) * 255.0) as u8;
    let b = (c.b.clamp(0.0, 1.0) * 255.0) as u8;
    let a = (alpha.clamp(0.0, 1.0) * 255.0) as u8;
    let mut i = 0;
    while i + 3 < bytes.len() {
        bytes[i] = r;
        bytes[i + 1] = g;
        bytes[i + 2] = b;
        bytes[i + 3] = a;
        i += 4;
    }
}

fn blit_buffer(
    dst: &mut [u8],
    dst_w: u32,
    dst_h: u32,
    src: &[u8],
    src_w: u32,
    src_h: u32,
    at_x: i32,
    at_y: i32,
) {
    for sy in 0..src_h as i32 {
        for sx in 0..src_w as i32 {
            let off = ((sy * src_w as i32 + sx) * 4) as usize;
            if off + 3 >= src.len() {
                continue;
            }
            let sr = src[off] as f32 / 255.0;
            let sg = src[off + 1] as f32 / 255.0;
            let sb = src[off + 2] as f32 / 255.0;
            let sa = src[off + 3] as f32 / 255.0;
            put_pixel_blend(
                dst,
                dst_w,
                dst_h,
                at_x + sx,
                at_y + sy,
                Color3::new(sr, sg, sb),
                sa,
            );
        }
    }
}

fn alpha_from_transparency(t: Option<f32>) -> f32 {
    (1.0 - t.unwrap_or(0.0)).clamp(0.0, 1.0)
}

impl UserData for DrawableImgHandle {
    fn add_methods<M: UserDataMethods<Self>>(m: &mut M) {
        m.add_method("Width", |_, this, _: ()| {
            Ok(this.inner.lock().unwrap().width as i64)
        });
        m.add_method("Height", |_, this, _: ()| {
            Ok(this.inner.lock().unwrap().height as i64)
        });
        m.add_method("Source", |_, this, _: ()| {
            Ok(this.inner.lock().unwrap().source.clone())
        });
        m.add_method("Pixels", |lua, this, _: ()| {
            let s = this.inner.lock().unwrap();
            let buf = s.buffer.lock().unwrap();
            lua.create_string(&buf.bytes)
        });
        m.add_method("IsAlive", |_, this, _: ()| {
            Ok(this.inner.lock().unwrap().alive)
        });

        m.add_method(
            "WritePixel",
            |_, this, (x, y, color, transparency): (i32, i32, Value, Option<f32>)| {
                let c = color_from_value(&color, "DrawableImg:WritePixel")?;
                let a = alpha_from_transparency(transparency);
                let s = this.inner.lock().unwrap();
                let mut buf = s.buffer.lock().unwrap();
                let (w, h) = (buf.width, buf.height);
                put_pixel_set(&mut buf.bytes, w, h, x, y, c, a);
                buf.version = buf.version.wrapping_add(1);
                Ok(())
            },
        );

        let draw_rect = |_: &Lua,
                         this: &DrawableImgHandle,
                         (x, y, w, h, color, transparency): (
            i32,
            i32,
            i32,
            i32,
            Value,
            Option<f32>,
        )|
         -> mlua::Result<()> {
            let c = color_from_value(&color, "DrawableImg:DrawRect")?;
            let a = alpha_from_transparency(transparency);
            let s = this.inner.lock().unwrap();
            let mut buf = s.buffer.lock().unwrap();
            let (bw, bh) = (buf.width, buf.height);
            fill_rect(&mut buf.bytes, bw, bh, x, y, w, h, c, a);
            buf.version = buf.version.wrapping_add(1);
            Ok(())
        };
        m.add_method("DrawRect", draw_rect);
        m.add_method("DrawCube", draw_rect);

        m.add_method(
            "DrawLine",
            |_,
             this,
             (x0, y0, x1, y1, color, transparency): (
                i32,
                i32,
                i32,
                i32,
                Value,
                Option<f32>,
            )|
             -> mlua::Result<()> {
                let c = color_from_value(&color, "DrawableImg:DrawLine")?;
                let a = alpha_from_transparency(transparency);
                let s = this.inner.lock().unwrap();
                let mut buf = s.buffer.lock().unwrap();
                let (bw, bh) = (buf.width, buf.height);
                draw_line(&mut buf.bytes, bw, bh, x0, y0, x1, y1, c, a);
                buf.version = buf.version.wrapping_add(1);
                Ok(())
            },
        );

        m.add_method(
            "DrawCircle",
            |_,
             this,
             (cx, cy, radius, color, transparency, filled): (
                i32,
                i32,
                i32,
                Value,
                Option<f32>,
                Option<bool>,
            )|
             -> mlua::Result<()> {
                let c = color_from_value(&color, "DrawableImg:DrawCircle")?;
                let a = alpha_from_transparency(transparency);
                let s = this.inner.lock().unwrap();
                let mut buf = s.buffer.lock().unwrap();
                let (bw, bh) = (buf.width, buf.height);
                draw_circle(
                    &mut buf.bytes,
                    bw,
                    bh,
                    cx,
                    cy,
                    radius,
                    c,
                    a,
                    filled.unwrap_or(false),
                );
                buf.version = buf.version.wrapping_add(1);
                Ok(())
            },
        );

        m.add_method(
            "Fill",
            |_, this, (color, transparency): (Value, Option<f32>)| -> mlua::Result<()> {
                let c = color_from_value(&color, "DrawableImg:Fill")?;
                let a = alpha_from_transparency(transparency);
                let s = this.inner.lock().unwrap();
                let mut buf = s.buffer.lock().unwrap();
                clear_to(&mut buf.bytes, c, a);
                buf.version = buf.version.wrapping_add(1);
                Ok(())
            },
        );

        m.add_method(
            "Clear",
            |_, this, _: ()| -> mlua::Result<()> {
                let s = this.inner.lock().unwrap();
                let mut buf = s.buffer.lock().unwrap();
                for byte in buf.bytes.iter_mut() {
                    *byte = 0;
                }
                buf.version = buf.version.wrapping_add(1);
                Ok(())
            },
        );

        m.add_method(
            "Apply",
            |_,
             this,
             (canvas_ud, at_x, at_y): (AnyUserData, Option<i32>, Option<i32>)|
             -> mlua::Result<()> {
                let cb = canvas_ud.borrow::<CanvasBufferHandle>().map_err(|_| {
                    mlua::Error::RuntimeError(
                        "DrawableImg:Apply expects a CanvasBuffer".into(),
                    )
                })?;
                let cb_state = cb.inner.lock().unwrap();
                let s = this.inner.lock().unwrap();
                let mut buf = s.buffer.lock().unwrap();
                let (bw, bh) = (buf.width, buf.height);
                blit_buffer(
                    &mut buf.bytes,
                    bw,
                    bh,
                    &cb_state.bytes,
                    cb_state.width,
                    cb_state.height,
                    at_x.unwrap_or(0),
                    at_y.unwrap_or(0),
                );
                buf.version = buf.version.wrapping_add(1);
                Ok(())
            },
        );

        m.add_method("Destroy", |_, this, _: ()| -> mlua::Result<()> {
            let mut s = this.inner.lock().unwrap();
            s.alive = false;
            let mut buf = s.buffer.lock().unwrap();
            for byte in buf.bytes.iter_mut() {
                *byte = 0;
            }
            buf.version = buf.version.wrapping_add(1);
            Ok(())
        });
    }
}

impl UserData for CanvasBufferHandle {
    fn add_methods<M: UserDataMethods<Self>>(m: &mut M) {
        m.add_method("Width", |_, this, _: ()| {
            Ok(this.inner.lock().unwrap().width as i64)
        });
        m.add_method("Height", |_, this, _: ()| {
            Ok(this.inner.lock().unwrap().height as i64)
        });
        m.add_method("Pixels", |lua, this, _: ()| {
            let s = this.inner.lock().unwrap();
            lua.create_string(&s.bytes)
        });

        m.add_method(
            "WritePixel",
            |_, this, (x, y, color, transparency): (i32, i32, Value, Option<f32>)| {
                let c = color_from_value(&color, "CanvasBuffer:WritePixel")?;
                let a = alpha_from_transparency(transparency);
                let mut s = this.inner.lock().unwrap();
                let (w, h) = (s.width, s.height);
                put_pixel_set(&mut s.bytes, w, h, x, y, c, a);
                Ok(())
            },
        );

        let draw_rect = |_: &Lua,
                         this: &CanvasBufferHandle,
                         (x, y, w, h, color, transparency): (
            i32,
            i32,
            i32,
            i32,
            Value,
            Option<f32>,
        )|
         -> mlua::Result<()> {
            let c = color_from_value(&color, "CanvasBuffer:DrawRect")?;
            let a = alpha_from_transparency(transparency);
            let mut s = this.inner.lock().unwrap();
            let (bw, bh) = (s.width, s.height);
            fill_rect(&mut s.bytes, bw, bh, x, y, w, h, c, a);
            Ok(())
        };
        m.add_method("DrawRect", draw_rect);
        m.add_method("DrawCube", draw_rect);

        m.add_method(
            "DrawLine",
            |_,
             this,
             (x0, y0, x1, y1, color, transparency): (
                i32,
                i32,
                i32,
                i32,
                Value,
                Option<f32>,
            )|
             -> mlua::Result<()> {
                let c = color_from_value(&color, "CanvasBuffer:DrawLine")?;
                let a = alpha_from_transparency(transparency);
                let mut s = this.inner.lock().unwrap();
                let (bw, bh) = (s.width, s.height);
                draw_line(&mut s.bytes, bw, bh, x0, y0, x1, y1, c, a);
                Ok(())
            },
        );

        m.add_method(
            "DrawCircle",
            |_,
             this,
             (cx, cy, radius, color, transparency, filled): (
                i32,
                i32,
                i32,
                Value,
                Option<f32>,
                Option<bool>,
            )|
             -> mlua::Result<()> {
                let c = color_from_value(&color, "CanvasBuffer:DrawCircle")?;
                let a = alpha_from_transparency(transparency);
                let mut s = this.inner.lock().unwrap();
                let (bw, bh) = (s.width, s.height);
                draw_circle(
                    &mut s.bytes,
                    bw,
                    bh,
                    cx,
                    cy,
                    radius,
                    c,
                    a,
                    filled.unwrap_or(false),
                );
                Ok(())
            },
        );

        m.add_method(
            "Fill",
            |_, this, (color, transparency): (Value, Option<f32>)| -> mlua::Result<()> {
                let c = color_from_value(&color, "CanvasBuffer:Fill")?;
                let a = alpha_from_transparency(transparency);
                let mut s = this.inner.lock().unwrap();
                clear_to(&mut s.bytes, c, a);
                Ok(())
            },
        );

        m.add_method("Clear", |_, this, _: ()| -> mlua::Result<()> {
            let mut s = this.inner.lock().unwrap();
            for byte in s.bytes.iter_mut() {
                *byte = 0;
            }
            Ok(())
        });

        m.add_method(
            "Apply",
            |_,
             this,
             (target_ud, at_x, at_y): (AnyUserData, Option<i32>, Option<i32>)|
             -> mlua::Result<()> {
                let target = target_ud.borrow::<DrawableImgHandle>().map_err(|_| {
                    mlua::Error::RuntimeError(
                        "CanvasBuffer:Apply expects a DrawableImg as the target".into(),
                    )
                })?;
                let src = this.inner.lock().unwrap();
                let dst_state = target.inner.lock().unwrap();
                let mut buf = dst_state.buffer.lock().unwrap();
                let (bw, bh) = (buf.width, buf.height);
                blit_buffer(
                    &mut buf.bytes,
                    bw,
                    bh,
                    &src.bytes,
                    src.width,
                    src.height,
                    at_x.unwrap_or(0),
                    at_y.unwrap_or(0),
                );
                buf.version = buf.version.wrapping_add(1);
                Ok(())
            },
        );
    }
}
