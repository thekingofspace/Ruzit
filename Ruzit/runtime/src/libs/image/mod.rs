use std::collections::HashMap;
use std::sync::Mutex;

use mlua::{AnyUserData, Lua, Table, UserData, UserDataMethods, Value};

use crate::libs::asset::ImageAsset;
use crate::libs::primitives::Color3;

pub fn create(lua: &Lua) -> mlua::Result<Table> {
    let t = lua.create_table()?;

    t.set(
        "GetColorAt",
        lua.create_function(
            |_, (ud, x, y): (AnyUserData, i64, i64)| -> mlua::Result<Color3> {
                let img = ud.borrow::<ImageAsset>()?;
                let (r, g, b, _) = sample_pixel(&img, x, y)?;
                Ok(Color3::new(r, g, b))
            },
        )?,
    )?;

    t.set(
        "GetAlphaAt",
        lua.create_function(
            |_, (ud, x, y): (AnyUserData, i64, i64)| -> mlua::Result<f64> {
                let img = ud.borrow::<ImageAsset>()?;
                let (_, _, _, a) = sample_pixel(&img, x, y)?;
                Ok(a as f64)
            },
        )?,
    )?;

    t.set(
        "GetPredominantColor",
        lua.create_function(|_, ud: AnyUserData| -> mlua::Result<Color3> {
            let img = ud.borrow::<ImageAsset>()?;
            let d = img.data_or_err("Image.GetPredominantColor")?;
            Ok(predominant_color(&d))
        })?,
    )?;

    t.set(
        "AverageColor",
        lua.create_function(|_, ud: AnyUserData| -> mlua::Result<Color3> {
            let img = ud.borrow::<ImageAsset>()?;
            let d = img.data_or_err("Image.AverageColor")?;
            Ok(average_color(&d))
        })?,
    )?;

    t.set(
        "ToSeed",
        lua.create_function(|_, ud: AnyUserData| -> mlua::Result<i64> {
            let img = ud.borrow::<ImageAsset>()?;
            let d = img.data_or_err("Image.ToSeed")?;
            let h = fnv1a64(&d) ^ ((img.width as u64) << 32) ^ (img.height as u64);
            Ok(h as i64)
        })?,
    )?;

    t.set(
        "Noise",
        lua.create_function(
            |_, (width, height, fill): (i64, i64, Option<f64>)| -> mlua::Result<NoiseField> {
                if width <= 0 || height <= 0 || width > 8192 || height > 8192 {
                    return Err(mlua::Error::RuntimeError(format!(
                        "Image.Noise: invalid size {width}x{height} (must be 1..=8192)"
                    )));
                }
                let w = width as u32;
                let h = height as u32;
                let v = fill.unwrap_or(0.0) as f32;
                Ok(NoiseField {
                    width: w,
                    height: h,
                    data: Mutex::new(vec![v; (w * h) as usize]),
                })
            },
        )?,
    )?;

    Ok(t)
}

fn sample_pixel(img: &ImageAsset, x: i64, y: i64) -> mlua::Result<(f32, f32, f32, f32)> {
    if x < 0 || y < 0 || x >= img.width as i64 || y >= img.height as i64 {
        return Err(mlua::Error::RuntimeError(format!(
            "Image: ({x}, {y}) out of bounds for {}x{}",
            img.width, img.height
        )));
    }
    let d = img.data_or_err("Image.sample_pixel")?;
    let i = ((y as u32 * img.width + x as u32) * 4) as usize;
    Ok((
        d[i] as f32 / 255.0,
        d[i + 1] as f32 / 255.0,
        d[i + 2] as f32 / 255.0,
        d[i + 3] as f32 / 255.0,
    ))
}

fn predominant_color(data: &[u8]) -> Color3 {
    let mut buckets: HashMap<u16, u64> = HashMap::new();
    for chunk in data.chunks_exact(4) {
        if chunk[3] < 8 {
            continue;
        }
        let key = (((chunk[0] >> 4) as u16) << 8)
            | (((chunk[1] >> 4) as u16) << 4)
            | ((chunk[2] >> 4) as u16);
        *buckets.entry(key).or_insert(0) += 1;
    }
    if buckets.is_empty() {
        return Color3::new(0.0, 0.0, 0.0);
    }
    let (best, _) = buckets.into_iter().max_by_key(|(_, c)| *c).unwrap();
    let r = (((best >> 8) & 0xF) as f32 * 16.0 + 8.0) / 255.0;
    let g = (((best >> 4) & 0xF) as f32 * 16.0 + 8.0) / 255.0;
    let b = ((best & 0xF) as f32 * 16.0 + 8.0) / 255.0;
    Color3::new(r, g, b)
}

fn average_color(data: &[u8]) -> Color3 {
    let mut sr = 0.0f64;
    let mut sg = 0.0f64;
    let mut sb = 0.0f64;
    let mut wsum = 0.0f64;
    for chunk in data.chunks_exact(4) {
        let a = chunk[3] as f64 / 255.0;
        if a <= 0.0 {
            continue;
        }
        sr += chunk[0] as f64 * a;
        sg += chunk[1] as f64 * a;
        sb += chunk[2] as f64 * a;
        wsum += a;
    }
    if wsum <= 0.0 {
        return Color3::new(0.0, 0.0, 0.0);
    }
    Color3::new(
        (sr / wsum / 255.0) as f32,
        (sg / wsum / 255.0) as f32,
        (sb / wsum / 255.0) as f32,
    )
}

fn fnv1a64(data: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for &b in data {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

pub struct NoiseField {
    pub width: u32,
    pub height: u32,
    pub data: Mutex<Vec<f32>>,
}

impl UserData for NoiseField {
    fn add_methods<M: UserDataMethods<Self>>(m: &mut M) {
        m.add_method("Width", |_, this, _: ()| Ok(this.width as i64));
        m.add_method("Height", |_, this, _: ()| Ok(this.height as i64));

        m.add_method("Clear", |_, this, fill: Option<f64>| -> mlua::Result<()> {
            let v = fill.unwrap_or(0.0) as f32;
            let mut d = this.data.lock().unwrap();
            for x in d.iter_mut() {
                *x = v;
            }
            Ok(())
        });

        m.add_method("Fill", |_, this, fill: f64| -> mlua::Result<()> {
            let v = fill as f32;
            let mut d = this.data.lock().unwrap();
            for x in d.iter_mut() {
                *x = v;
            }
            Ok(())
        });

        m.add_method(
            "GetValueAt",
            |_, this, (x, y): (i64, i64)| -> mlua::Result<f64> {
                if x < 0 || y < 0 || x >= this.width as i64 || y >= this.height as i64 {
                    return Err(mlua::Error::RuntimeError(format!(
                        "NoiseField: ({x}, {y}) out of bounds for {}x{}",
                        this.width, this.height
                    )));
                }
                let i = (y as u32 * this.width + x as u32) as usize;
                Ok(this.data.lock().unwrap()[i] as f64)
            },
        );

        m.add_method(
            "SetValueAt",
            |_, this, (x, y, v): (i64, i64, f64)| -> mlua::Result<()> {
                if x < 0 || y < 0 || x >= this.width as i64 || y >= this.height as i64 {
                    return Err(mlua::Error::RuntimeError(format!(
                        "NoiseField: ({x}, {y}) out of bounds for {}x{}",
                        this.width, this.height
                    )));
                }
                let i = (y as u32 * this.width + x as u32) as usize;
                this.data.lock().unwrap()[i] = v as f32;
                Ok(())
            },
        );

        m.add_method(
            "GetColorAt",
            |_, this, (x, y): (i64, i64)| -> mlua::Result<Color3> {
                if x < 0 || y < 0 || x >= this.width as i64 || y >= this.height as i64 {
                    return Err(mlua::Error::RuntimeError(format!(
                        "NoiseField: ({x}, {y}) out of bounds for {}x{}",
                        this.width, this.height
                    )));
                }
                let i = (y as u32 * this.width + x as u32) as usize;
                let v = this.data.lock().unwrap()[i].clamp(0.0, 1.0);
                Ok(Color3::new(v, v, v))
            },
        );

        m.add_method("Normalize", |_, this, _: ()| -> mlua::Result<()> {
            let mut d = this.data.lock().unwrap();
            if d.is_empty() {
                return Ok(());
            }
            let (mut lo, mut hi) = (f32::INFINITY, f32::NEG_INFINITY);
            for &v in d.iter() {
                if v < lo {
                    lo = v;
                }
                if v > hi {
                    hi = v;
                }
            }
            let span = hi - lo;
            if span <= 1e-9 {
                for x in d.iter_mut() {
                    *x = 0.0;
                }
            } else {
                let inv = 1.0 / span;
                for x in d.iter_mut() {
                    *x = (*x - lo) * inv;
                }
            }
            Ok(())
        });

        m.add_method("ToBytes", |lua, this, _: ()| {
            let d = this.data.lock().unwrap();
            let mut out = Vec::with_capacity(d.len() * 4);
            for &v in d.iter() {
                let g = (v.clamp(0.0, 1.0) * 255.0) as u8;
                out.push(g);
                out.push(g);
                out.push(g);
                out.push(255u8);
            }
            lua.create_string(&out)
        });

        m.add_method(
            "GenerateLayer",
            |_, this, params: Table| -> mlua::Result<()> {
                let kind: String = read_str(&params, "Type", "type").unwrap_or_else(|| "fbm".to_string());
                let scale = read_num(&params, "Scale", "scale").unwrap_or(16.0).max(0.0001);
                let amplitude = read_num(&params, "Amplitude", "amplitude").unwrap_or(1.0);
                let octaves = read_num(&params, "Octaves", "octaves")
                    .map(|v| v as i64)
                    .unwrap_or(4)
                    .clamp(1, 12) as u32;
                let lacunarity = read_num(&params, "Lacunarity", "lacunarity").unwrap_or(2.0);
                let persistence = read_num(&params, "Persistence", "persistence").unwrap_or(0.5);
                let seed = read_num(&params, "Seed", "seed").map(|v| v as i64).unwrap_or(0) as u64;
                let offset_x = read_num(&params, "OffsetX", "offsetX").unwrap_or(0.0);
                let offset_y = read_num(&params, "OffsetY", "offsetY").unwrap_or(0.0);

                let mode = read_str(&params, "Mode", "mode").unwrap_or_else(|| "add".to_string());

                let mut buf = vec![0.0f32; (this.width * this.height) as usize];
                match kind.as_str() {
                    "value" => fill_value_noise(&mut buf, this.width, this.height, scale, seed, offset_x, offset_y),
                    "perlin" | "gradient" => fill_perlin_noise(&mut buf, this.width, this.height, scale, seed, offset_x, offset_y),
                    "ridged" => fill_ridged_fbm(&mut buf, this.width, this.height, scale, octaves, lacunarity, persistence, seed, offset_x, offset_y),
                    "white" => fill_white_noise(&mut buf, this.width, this.height, seed),
                    _ => fill_fbm(&mut buf, this.width, this.height, scale, octaves, lacunarity, persistence, seed, offset_x, offset_y),
                }
                let amp = amplitude as f32;
                let mut d = this.data.lock().unwrap();
                match mode.as_str() {
                    "set" | "replace" => {
                        for (dst, src) in d.iter_mut().zip(buf.iter()) {
                            *dst = src * amp;
                        }
                    }
                    "mul" | "multiply" => {
                        for (dst, src) in d.iter_mut().zip(buf.iter()) {
                            *dst *= src * amp;
                        }
                    }
                    "max" => {
                        for (dst, src) in d.iter_mut().zip(buf.iter()) {
                            let s = src * amp;
                            if s > *dst {
                                *dst = s;
                            }
                        }
                    }
                    "min" => {
                        for (dst, src) in d.iter_mut().zip(buf.iter()) {
                            let s = src * amp;
                            if s < *dst {
                                *dst = s;
                            }
                        }
                    }
                    _ => {
                        for (dst, src) in d.iter_mut().zip(buf.iter()) {
                            *dst += src * amp;
                        }
                    }
                }
                Ok(())
            },
        );
    }
}

fn read_num(t: &Table, upper: &str, lower: &str) -> Option<f64> {
    if let Ok(v) = t.get::<Value>(upper) {
        if let Some(n) = num_of(&v) {
            return Some(n);
        }
    }
    if let Ok(v) = t.get::<Value>(lower) {
        return num_of(&v);
    }
    None
}

fn read_str(t: &Table, upper: &str, lower: &str) -> Option<String> {
    if let Ok(s) = t.get::<String>(upper) {
        return Some(s);
    }
    if let Ok(s) = t.get::<String>(lower) {
        return Some(s);
    }
    None
}

fn num_of(v: &Value) -> Option<f64> {
    match v {
        Value::Number(n) => Some(*n),
        Value::Integer(n) => Some(*n as f64),
        _ => None,
    }
}

fn hash2(x: i32, y: i32, seed: u64) -> u32 {
    let mut h = seed ^ (x as u32 as u64).wrapping_mul(0x9E3779B97F4A7C15);
    h ^= (y as u32 as u64).wrapping_mul(0xBF58476D1CE4E5B9);
    h ^= h >> 27;
    h = h.wrapping_mul(0x94D049BB133111EB);
    h ^= h >> 31;
    h as u32
}

fn hash_to_unit(x: i32, y: i32, seed: u64) -> f32 {
    (hash2(x, y, seed) as f32) / (u32::MAX as f32)
}

fn smoothstep(t: f32) -> f32 {
    t * t * (3.0 - 2.0 * t)
}

fn fill_white_noise(buf: &mut [f32], w: u32, h: u32, seed: u64) {
    for y in 0..h {
        for x in 0..w {
            buf[(y * w + x) as usize] = hash_to_unit(x as i32, y as i32, seed);
        }
    }
}

fn value_noise2(fx: f32, fy: f32, seed: u64) -> f32 {
    let xi = fx.floor() as i32;
    let yi = fy.floor() as i32;
    let tx = smoothstep(fx - xi as f32);
    let ty = smoothstep(fy - yi as f32);
    let v00 = hash_to_unit(xi, yi, seed);
    let v10 = hash_to_unit(xi + 1, yi, seed);
    let v01 = hash_to_unit(xi, yi + 1, seed);
    let v11 = hash_to_unit(xi + 1, yi + 1, seed);
    let a = v00 + (v10 - v00) * tx;
    let b = v01 + (v11 - v01) * tx;
    a + (b - a) * ty
}

fn fill_value_noise(buf: &mut [f32], w: u32, h: u32, scale: f64, seed: u64, ox: f64, oy: f64) {
    let inv = 1.0 / scale as f32;
    let ox = ox as f32;
    let oy = oy as f32;
    for y in 0..h {
        for x in 0..w {
            let fx = (x as f32 + ox) * inv;
            let fy = (y as f32 + oy) * inv;
            buf[(y * w + x) as usize] = value_noise2(fx, fy, seed);
        }
    }
}

fn grad_dot(ix: i32, iy: i32, dx: f32, dy: f32, seed: u64) -> f32 {
    let h = hash2(ix, iy, seed);
    let angle = (h as f32 / u32::MAX as f32) * std::f32::consts::TAU;
    let gx = angle.cos();
    let gy = angle.sin();
    gx * dx + gy * dy
}

fn perlin2(fx: f32, fy: f32, seed: u64) -> f32 {
    let xi = fx.floor() as i32;
    let yi = fy.floor() as i32;
    let tx = fx - xi as f32;
    let ty = fy - yi as f32;
    let u = smoothstep(tx);
    let v = smoothstep(ty);
    let n00 = grad_dot(xi, yi, tx, ty, seed);
    let n10 = grad_dot(xi + 1, yi, tx - 1.0, ty, seed);
    let n01 = grad_dot(xi, yi + 1, tx, ty - 1.0, seed);
    let n11 = grad_dot(xi + 1, yi + 1, tx - 1.0, ty - 1.0, seed);
    let a = n00 + (n10 - n00) * u;
    let b = n01 + (n11 - n01) * u;
    let raw = a + (b - a) * v;
    raw * 0.5 + 0.5
}

fn fill_perlin_noise(buf: &mut [f32], w: u32, h: u32, scale: f64, seed: u64, ox: f64, oy: f64) {
    let inv = 1.0 / scale as f32;
    let ox = ox as f32;
    let oy = oy as f32;
    for y in 0..h {
        for x in 0..w {
            let fx = (x as f32 + ox) * inv;
            let fy = (y as f32 + oy) * inv;
            buf[(y * w + x) as usize] = perlin2(fx, fy, seed);
        }
    }
}

fn fill_fbm(
    buf: &mut [f32],
    w: u32,
    h: u32,
    scale: f64,
    octaves: u32,
    lacunarity: f64,
    persistence: f64,
    seed: u64,
    ox: f64,
    oy: f64,
) {
    let inv = 1.0 / scale as f32;
    let lac = lacunarity as f32;
    let per = persistence as f32;
    let ox = ox as f32;
    let oy = oy as f32;
    for y in 0..h {
        for x in 0..w {
            let mut amp = 1.0f32;
            let mut freq = 1.0f32;
            let mut sum = 0.0f32;
            let mut norm = 0.0f32;
            for o in 0..octaves {
                let fx = (x as f32 + ox) * inv * freq;
                let fy = (y as f32 + oy) * inv * freq;
                sum += perlin2(fx, fy, seed ^ (o as u64).wrapping_mul(0xA24BAED4963EE407)) * amp;
                norm += amp;
                amp *= per;
                freq *= lac;
            }
            buf[(y * w + x) as usize] = if norm > 0.0 { sum / norm } else { 0.0 };
        }
    }
}

fn fill_ridged_fbm(
    buf: &mut [f32],
    w: u32,
    h: u32,
    scale: f64,
    octaves: u32,
    lacunarity: f64,
    persistence: f64,
    seed: u64,
    ox: f64,
    oy: f64,
) {
    let inv = 1.0 / scale as f32;
    let lac = lacunarity as f32;
    let per = persistence as f32;
    let ox = ox as f32;
    let oy = oy as f32;
    for y in 0..h {
        for x in 0..w {
            let mut amp = 1.0f32;
            let mut freq = 1.0f32;
            let mut sum = 0.0f32;
            let mut norm = 0.0f32;
            for o in 0..octaves {
                let fx = (x as f32 + ox) * inv * freq;
                let fy = (y as f32 + oy) * inv * freq;
                let n = perlin2(fx, fy, seed ^ (o as u64).wrapping_mul(0xA24BAED4963EE407));
                let ridged = 1.0 - (n * 2.0 - 1.0).abs();
                sum += ridged * amp;
                norm += amp;
                amp *= per;
                freq *= lac;
            }
            buf[(y * w + x) as usize] = if norm > 0.0 { sum / norm } else { 0.0 };
        }
    }
}
