use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use mlua::{AnyUserData, Lua, MultiValue, Table, UserData, UserDataFields, UserDataMethods, Value};

use crate::libs::asset::{self, FontAsset, FragmentAsset, ImageAsset, ShaderAsset};
use crate::libs::primitives::{Color3, Dim};
use crate::libs::signal;

pub mod effect_volume;
pub mod particle_pipeline;
pub mod render;
pub mod spatial;
pub mod spline;

pub use effect_volume::{
    tick_ui_effect_volumes, ui_effect_volume_snapshot, UIEffectVolumeHandle, UIEffectVolumeRender,
    UIParticleRender,
};
pub use spline::{Spline, SplineRender, SplineVertex};

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

thread_local! {

    static REGISTRY: RefCell<Vec<Arc<Mutex<PrimitiveState>>>> = const { RefCell::new(Vec::new()) };

    static SKYBOX: RefCell<Option<Arc<SceneShaderState>>> = const { RefCell::new(None) };
    static POST_EFFECT: RefCell<Option<Arc<SceneShaderState>>> = const { RefCell::new(None) };
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shape {
    Square,
    Circle,
    Triangle,

    Image,

    Text,

    Clippable,
}

impl Shape {
    pub fn shape_id(self) -> u32 {
        match self {
            Self::Square => 0,
            Self::Circle => 1,
            Self::Triangle => 2,
            Self::Image => 3,
            Self::Text => 4,
            Self::Clippable => 5,
        }
    }
}

pub struct ImageRef {
    pub id: u64,
    pub width: u32,
    pub height: u32,
    pub data: Arc<Vec<u8>>,
}

#[derive(Clone)]
pub struct AttachedShader {
    pub id: u64,
    #[allow(dead_code)]
    pub source: String,

    pub wgsl: Arc<String>,

    pub slot_of_name: Arc<std::collections::HashMap<String, u8>>,

    pub params: Arc<Mutex<[f32; 16]>>,
    /// Draw-order weight. Lower = drawn first (becomes the base); higher =
    /// drawn later (appears on top). Defaults to 0; ties preserve the order
    /// AttachShader was called in.
    pub priority: i32,
}

pub struct PrimitiveState {
    #[allow(dead_code)]
    pub id: u64,
    pub shape: Shape,
    pub size: Dim,
    pub position: Dim,
    // Raw anchor point (0..1), forwarded from Declar. Declar pre-applies the anchor
    // against the node's Size when computing `position`; for TEXT we re-apply it
    // against the true BAKED width at snapshot time so centred text lands exactly
    // on centre regardless of how good the Lua width estimate was.
    pub anchor: Dim,
    pub rotation: f32,
    pub color: Color3,
    pub transparency: f32,
    pub z_index: i32,
    pub visible: bool,
    pub alive: bool,

    pub attached: Vec<AttachedShader>,
    pub changed_signal: Table,

    pub image: Option<Arc<ImageRef>>,

    pub text: Option<TextState>,

    pub dyn_img_owner: Option<u64>,

    pub prop_signals: HashMap<String, Table>,

    pub clip_parent: Option<Arc<Mutex<PrimitiveState>>>,
    pub clip_shape: Shape,

    pub billboard_parent: Option<Arc<Mutex<BillboardInner>>>,
}

#[derive(Clone, Copy, Debug)]
pub struct BillboardAnchor {
    pub world_pos: crate::libs::primitives::Vector,
    pub scale_with_camera: bool,
    pub canvas_size: Dim,
}

pub struct BillboardInner {
    pub position: crate::libs::primitives::Vector,
    pub size: Dim,
    pub scale_with_camera: bool,
    pub alive: bool,
    pub children: Vec<std::sync::Weak<Mutex<PrimitiveState>>>,
}

pub struct TextState {
    #[allow(dead_code)]
    pub font_id: u64,
    pub font: Arc<fontdue::Font>,
    pub content: String,
    pub size_px: f32,
    pub style: FontStyle,
    pub underline: bool,
    pub strikethrough: bool,

    pub baked: Option<Arc<ImageRef>>,
    pub baked_color: Option<Color3>,
}

impl TextState {
    fn invalidate(&mut self) {
        self.baked = None;
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FontStyle {
    Regular,
    Italic,
    Light,
    LightItalic,
    Medium,
    MediumItalic,
    SemiBold,
    SemiBoldItalic,
    Bold,
    BoldItalic,
    ExtraBold,
    ExtraBoldItalic,
    Black,
    BlackItalic,
}

impl FontStyle {
    pub const ALL: &'static [FontStyle] = &[
        FontStyle::Regular,
        FontStyle::Italic,
        FontStyle::Light,
        FontStyle::LightItalic,
        FontStyle::Medium,
        FontStyle::MediumItalic,
        FontStyle::SemiBold,
        FontStyle::SemiBoldItalic,
        FontStyle::Bold,
        FontStyle::BoldItalic,
        FontStyle::ExtraBold,
        FontStyle::ExtraBoldItalic,
        FontStyle::Black,
        FontStyle::BlackItalic,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            FontStyle::Regular => "Regular",
            FontStyle::Italic => "Italic",
            FontStyle::Light => "Light",
            FontStyle::LightItalic => "LightItalic",
            FontStyle::Medium => "Medium",
            FontStyle::MediumItalic => "MediumItalic",
            FontStyle::SemiBold => "SemiBold",
            FontStyle::SemiBoldItalic => "SemiBoldItalic",
            FontStyle::Bold => "Bold",
            FontStyle::BoldItalic => "BoldItalic",
            FontStyle::ExtraBold => "ExtraBold",
            FontStyle::ExtraBoldItalic => "ExtraBoldItalic",
            FontStyle::Black => "Black",
            FontStyle::BlackItalic => "BlackItalic",
        }
    }

    pub fn parse(s: &str) -> Option<FontStyle> {
        Self::ALL.iter().copied().find(|st| st.as_str().eq_ignore_ascii_case(s))
    }

    fn is_italic(self) -> bool {
        matches!(
            self,
            FontStyle::Italic
                | FontStyle::LightItalic
                | FontStyle::MediumItalic
                | FontStyle::SemiBoldItalic
                | FontStyle::BoldItalic
                | FontStyle::ExtraBoldItalic
                | FontStyle::BlackItalic
        )
    }

    fn weight_dilation_px(self, size_px: f32) -> f32 {
        let scale = (size_px / 24.0).max(0.5);
        let base = match self {
            FontStyle::Light | FontStyle::LightItalic => -0.4,
            FontStyle::Regular | FontStyle::Italic => 0.0,
            FontStyle::Medium | FontStyle::MediumItalic => 0.35,
            FontStyle::SemiBold | FontStyle::SemiBoldItalic => 0.8,
            FontStyle::Bold | FontStyle::BoldItalic => 1.4,
            FontStyle::ExtraBold | FontStyle::ExtraBoldItalic => 2.2,
            FontStyle::Black | FontStyle::BlackItalic => 3.0,
        };
        base * scale
    }
}

pub struct RenderItem {
    pub shape: Shape,
    pub size: Dim,
    pub position: Dim,
    pub rotation: f32,
    pub color: Color3,
    pub transparency: f32,
    pub z_index: i32,

    pub active_shaders: Vec<AttachedShader>,
    pub image: Option<Arc<ImageRef>>,

    pub clip: Option<ClipInfo>,

    pub billboard_anchor: Option<BillboardAnchor>,
}

#[derive(Clone, Copy, Debug)]
pub struct ClipInfo {
    pub pos: Dim,
    pub size: Dim,
    pub rotation: f32,
    pub shape: Shape,
}

pub fn list_primitive_states() -> Vec<Arc<Mutex<PrimitiveState>>> {
    REGISTRY.with(|cell| cell.borrow().iter().cloned().collect())
}

pub fn purge_image(lua: &Lua, asset_id: u64) {
    purge_primitives_matching(lua, |s| {
        s.image.as_ref().map(|i| i.id == asset_id).unwrap_or(false)
    });
}

pub fn purge_font(lua: &Lua, asset_id: u64) {
    purge_primitives_matching(lua, |s| {
        s.text
            .as_ref()
            .map(|t| t.font_id == asset_id)
            .unwrap_or(false)
    });
}

pub fn purge_shader(lua: &Lua, asset_id: u64) {
    REGISTRY.with(|cell| {
        let reg = cell.borrow();
        for p in reg.iter() {
            let mut s = p.lock().unwrap();
            if !s.alive {
                continue;
            }
            s.attached.retain(|e| e.id != asset_id);
        }
    });
    SKYBOX.with(|c| {
        if c.borrow().as_ref().map(|s| s.id) == Some(asset_id) {
            *c.borrow_mut() = None;
        }
    });
    POST_EFFECT.with(|c| {
        if c.borrow().as_ref().map(|s| s.id) == Some(asset_id) {
            *c.borrow_mut() = None;
        }
    });
    let _ = lua;
}

fn purge_primitives_matching(lua: &Lua, mut pred: impl FnMut(&PrimitiveState) -> bool) {
    let states: Vec<Arc<Mutex<PrimitiveState>>> = REGISTRY.with(|cell| {
        let reg = cell.borrow();
        reg.iter().cloned().collect()
    });
    for state in states {
        let sig = {
            let mut s = state.lock().unwrap();
            if !s.alive || !pred(&s) {
                continue;
            }
            s.alive = false;
            s.visible = false;
            s.attached.clear();
            s.image = None;
            s.text = None;
            s.changed_signal.clone()
        };
        let _ = fire_changed(lua, sig, "Destroyed");
    }
}

thread_local! {
    static SNAPSHOT_CACHE: RefCell<(u64, Arc<Vec<RenderItem>>)> =
        RefCell::new((0, Arc::new(Vec::new())));
}

pub fn snapshot() -> Arc<Vec<RenderItem>> {
    let version = current_version();
    let cached_match = SNAPSHOT_CACHE.with(|cell| {
        let cache = cell.borrow();
        if cache.0 == version {
            Some(cache.1.clone())
        } else {
            None
        }
    });
    if let Some(items) = cached_match {
        return items;
    }
    let items = build_snapshot();
    let arc = Arc::new(items);
    SNAPSHOT_CACHE.with(|cell| {
        *cell.borrow_mut() = (version, arc.clone());
    });
    arc
}

fn build_snapshot() -> Vec<RenderItem> {
    REGISTRY.with(|cell| {
        let mut reg = cell.borrow_mut();
        reg.retain(|p| p.lock().unwrap().alive);
        let mut out: Vec<RenderItem> = reg
            .iter()
            .filter_map(|p| {
                let mut s = p.lock().unwrap();
                if !s.visible || s.dyn_img_owner.is_some() {
                    return None;
                }
                if matches!(s.shape, Shape::Clippable) {
                    return None;
                }

                let (image, size, position) = if matches!(s.shape, Shape::Text) {
                    let baked = bake_text_if_dirty(&mut s);
                    let size = match &baked {
                        Some(img) => Dim::new(img.width as f32, img.height as f32),
                        None => Dim::new(0.0, 0.0),
                    };
                    // Declar applied the anchor against the node's estimated Size; the
                    // baked image is a different width, so re-apply it on X against the
                    // TRUE baked width -> centred/anchored text lands exactly on target
                    // no matter how rough the Lua width estimate was. Y is left as-is so
                    // the caller's optical vertical lift (for the font's ascent band)
                    // still applies.
                    let pos = Dim::new(
                        s.position.x + s.anchor.x * (s.size.x - size.x),
                        s.position.y,
                    );
                    (baked, size, pos)
                } else {
                    (s.image.clone(), s.size, s.position)
                };

                let clip = s.clip_parent.as_ref().and_then(|parent_arc| {
                    let parent = parent_arc.lock().ok()?;
                    if !parent.alive {
                        return None;
                    }
                    Some(ClipInfo {
                        pos: parent.position,
                        size: parent.size,
                        rotation: parent.rotation,
                        shape: parent.clip_shape,
                    })
                });

                let billboard_anchor = s.billboard_parent.as_ref().and_then(|b_arc| {
                    let b = b_arc.lock().ok()?;
                    if !b.alive {
                        return None;
                    }
                    Some(BillboardAnchor {
                        world_pos: b.position,
                        scale_with_camera: b.scale_with_camera,
                        canvas_size: b.size,
                    })
                });
                Some(RenderItem {
                    shape: s.shape,
                    size,
                    position,
                    rotation: s.rotation,
                    color: s.color,
                    transparency: s.transparency,
                    z_index: s.z_index,
                    active_shaders: {
                        let mut v = s.attached.clone();
                        v.sort_by_key(|sh| sh.priority);
                        v
                    },
                    image,
                    clip,
                    billboard_anchor,
                })
            })
            .collect();
        out.sort_by_key(|r| r.z_index);
        out
    })
}

fn bake_text_if_dirty(s: &mut PrimitiveState) -> Option<Arc<ImageRef>> {
    let base_color = s.color;
    let ts = s.text.as_mut()?;
    let cached_ok = match (&ts.baked, ts.baked_color) {
        (Some(img), Some(c)) if color_eq(c, base_color) => Some(img.clone()),
        _ => None,
    };
    if let Some(img) = cached_ok {
        return Some(img);
    }
    let baked = bake_text(
        &ts.font,
        &ts.content,
        ts.size_px,
        base_color,
        ts.style,
        ts.underline,
        ts.strikethrough,
    );
    ts.baked = Some(baked.clone());
    ts.baked_color = Some(base_color);
    Some(baked)
}

fn color_eq(a: Color3, b: Color3) -> bool {
    a.r == b.r && a.g == b.g && a.b == b.b
}

fn parse_color_runs(content: &str, base: Color3) -> (Vec<Color3>, Vec<(String, u16)>) {
    let mut colors: Vec<Color3> = vec![base];
    let mut runs: Vec<(String, u16)> = Vec::new();
    let mut buf = String::new();
    let mut current_idx: u16 = 0;
    let bytes = content.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'{' {
            if i + 1 < bytes.len() && bytes[i + 1] == b'{' {
                buf.push('{');
                i += 2;
                continue;
            }
            if let Some(close_rel) = content[i + 1..].find('}') {
                let tag = &content[i + 1..i + 1 + close_rel];
                if let Some(new_color) = parse_color_tag(tag, base) {
                    if !buf.is_empty() {
                        runs.push((std::mem::take(&mut buf), current_idx));
                    }
                    let idx = colors
                        .iter()
                        .position(|c| color_eq(*c, new_color))
                        .unwrap_or_else(|| {
                            let n = colors.len();
                            colors.push(new_color);
                            n
                        });
                    current_idx = idx as u16;
                    i = i + 1 + close_rel + 1;
                    continue;
                }
            }
        }
        let ch_len = utf8_char_len(bytes[i]);
        buf.push_str(&content[i..i + ch_len]);
        i += ch_len;
    }
    if !buf.is_empty() {
        runs.push((buf, current_idx));
    }
    (colors, runs)
}

fn parse_color_tag(tag: &str, base: Color3) -> Option<Color3> {
    let tag = tag.trim();
    if tag.is_empty() {
        return Some(base);
    }
    let hex = tag.strip_prefix('#')?;
    match hex.len() {
        3 => {
            let r = u8::from_str_radix(&hex[0..1], 16).ok()?;
            let g = u8::from_str_radix(&hex[1..2], 16).ok()?;
            let b = u8::from_str_radix(&hex[2..3], 16).ok()?;
            Some(Color3::new(
                (r * 17) as f32 / 255.0,
                (g * 17) as f32 / 255.0,
                (b * 17) as f32 / 255.0,
            ))
        }
        6 => {
            let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
            let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
            let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
            Some(Color3::new(
                r as f32 / 255.0,
                g as f32 / 255.0,
                b as f32 / 255.0,
            ))
        }
        _ => None,
    }
}

fn utf8_char_len(b: u8) -> usize {
    if b < 0x80 {
        1
    } else if b < 0xC0 {
        1
    } else if b < 0xE0 {
        2
    } else if b < 0xF0 {
        3
    } else {
        4
    }
}

fn bake_text(
    font: &fontdue::Font,
    content: &str,
    size_px: f32,
    base_color: Color3,
    style: FontStyle,
    underline: bool,
    strikethrough: bool,
) -> Arc<ImageRef> {
    use fontdue::layout::{CoordinateSystem, Layout, TextStyle};
    let id = asset::next_shader_id();
    if content.is_empty() || size_px < 1.0 {
        return Arc::new(ImageRef {
            id,
            width: 1,
            height: 1,
            data: Arc::new(vec![0, 0, 0, 0]),
        });
    }
    let (colors, runs) = parse_color_runs(content, base_color);
    if runs.is_empty() {
        return Arc::new(ImageRef {
            id,
            width: 1,
            height: 1,
            data: Arc::new(vec![0, 0, 0, 0]),
        });
    }
    let mut layout: Layout<u16> = Layout::new(CoordinateSystem::PositiveYDown);
    for (segment, idx) in &runs {
        layout.append(
            &[font],
            &TextStyle::with_user_data(segment.as_str(), size_px, 0, *idx),
        );
    }
    let glyphs = layout.glyphs();
    if glyphs.is_empty() {
        return Arc::new(ImageRef {
            id,
            width: 1,
            height: 1,
            data: Arc::new(vec![0, 0, 0, 0]),
        });
    }

    let mut max_x: i32 = 0;
    let mut max_y: i32 = 0;
    for g in glyphs {
        max_x = max_x.max(g.x as i32 + g.width as i32);
        max_y = max_y.max(g.y as i32 + g.height as i32);
    }

    let italic = style.is_italic();
    let italic_slope: f32 = if italic { 0.22 } else { 0.0 };
    let dilation = style.weight_dilation_px(size_px);
    let extra_pad = dilation.abs().ceil().max(0.0) as i32 + 1;
    let italic_extra = (max_y as f32 * italic_slope).ceil() as i32;

    let pad_left = extra_pad.max(italic_extra);
    let pad_right = extra_pad + italic_extra;
    let pad_top = extra_pad;
    let pad_bottom = extra_pad + if underline { (size_px / 16.0).ceil() as i32 + 1 } else { 0 };

    let width = (max_x + pad_left + pad_right).max(1) as u32;
    let height = (max_y + pad_top + pad_bottom).max(1) as u32;
    let mut alpha = vec![0u8; (width * height) as usize];
    let mut color_idx_map = vec![0u16; (width * height) as usize];

    for g in glyphs {
        let (_metrics, bitmap) = font.rasterize_config(g.key);
        let gw = g.width as i32;
        let gh = g.height as i32;
        let gx = g.x as i32 + pad_left;
        let gy = g.y as i32 + pad_top;
        let g_color_idx = g.user_data;
        for j in 0..gh {
            let shear_off = if italic {
                ((max_y - (g.y as i32 + j)) as f32 * italic_slope).round() as i32
            } else {
                0
            };
            for i in 0..gw {
                let a = bitmap[(j * gw + i) as usize];
                if a == 0 {
                    continue;
                }
                let px = gx + i + shear_off;
                let py = gy + j;
                if px < 0 || py < 0 || px as u32 >= width || py as u32 >= height {
                    continue;
                }
                let off = (py as u32 * width + px as u32) as usize;
                if alpha[off] < a {
                    alpha[off] = a;
                    color_idx_map[off] = g_color_idx;
                }
            }
        }
    }

    if dilation > 0.05 {
        let (new_alpha, new_idx) = dilate_alpha_colored(
            &alpha,
            &color_idx_map,
            width as i32,
            height as i32,
            dilation,
        );
        alpha = new_alpha;
        color_idx_map = new_idx;
    } else if dilation < -0.05 {
        alpha = erode_alpha(&alpha, width as i32, height as i32, -dilation);
    }

    if underline {
        let thickness = ((size_px / 16.0).round() as i32).max(1);
        let y_start = (max_y + pad_top + (size_px / 8.0).round() as i32).clamp(0, height as i32 - 1);
        let y_end = (y_start + thickness).clamp(0, height as i32);
        let x_start = pad_left.max(0).min(width as i32);
        let x_end = (max_x + pad_left + italic_extra).clamp(0, width as i32);
        for y in y_start..y_end {
            for x in x_start..x_end {
                let off = (y as u32 * width + x as u32) as usize;
                alpha[off] = 255;
                color_idx_map[off] = 0;
            }
        }
    }
    if strikethrough {
        let thickness = ((size_px / 16.0).round() as i32).max(1);
        let y_center = pad_top + (max_y as f32 * 0.62).round() as i32;
        let y_start = (y_center - thickness / 2).clamp(0, height as i32 - 1);
        let y_end = (y_start + thickness).clamp(0, height as i32);
        let x_start = pad_left.max(0).min(width as i32);
        let x_end = (max_x + pad_left + italic_extra).clamp(0, width as i32);
        for y in y_start..y_end {
            for x in x_start..x_end {
                let off = (y as u32 * width + x as u32) as usize;
                alpha[off] = 255;
                color_idx_map[off] = 0;
            }
        }
    }

    let palette: Vec<[u8; 3]> = colors
        .iter()
        .map(|c| {
            [
                (c.r * 255.0).round().clamp(0.0, 255.0) as u8,
                (c.g * 255.0).round().clamp(0.0, 255.0) as u8,
                (c.b * 255.0).round().clamp(0.0, 255.0) as u8,
            ]
        })
        .collect();

    let mut buf = vec![0u8; (width * height * 4) as usize];
    for i in 0..(width * height) as usize {
        let a = alpha[i];
        if a == 0 {
            continue;
        }
        let rgb = palette
            .get(color_idx_map[i] as usize)
            .copied()
            .unwrap_or(palette[0]);
        let off = i * 4;
        buf[off] = rgb[0];
        buf[off + 1] = rgb[1];
        buf[off + 2] = rgb[2];
        buf[off + 3] = a;
    }
    Arc::new(ImageRef {
        id,
        width,
        height,
        data: Arc::new(buf),
    })
}

fn dilate_alpha_colored(
    src: &[u8],
    src_idx: &[u16],
    width: i32,
    height: i32,
    amount: f32,
) -> (Vec<u8>, Vec<u16>) {
    let radius = amount.round() as i32;
    let frac = (amount - radius as f32).clamp(0.0, 1.0);
    if radius <= 0 && frac < 0.05 {
        return (src.to_vec(), src_idx.to_vec());
    }
    let mut dst = vec![0u8; src.len()];
    let mut dst_idx = vec![0u16; src.len()];
    let r = radius.max(1);
    for y in 0..height {
        for x in 0..width {
            let mut best: u8 = 0;
            let mut best_idx: u16 = 0;
            for dy in -r..=r {
                let yy = y + dy;
                if yy < 0 || yy >= height {
                    continue;
                }
                for dx in -r..=r {
                    let xx = x + dx;
                    if xx < 0 || xx >= width {
                        continue;
                    }
                    let off = (yy * width + xx) as usize;
                    let v = src[off];
                    if v > best {
                        best = v;
                        best_idx = src_idx[off];
                    }
                }
            }
            let here = (y * width + x) as usize;
            let blended = if frac > 0.05 && best > 0 {
                let src_v = src[here];
                let mix = src_v as f32 * (1.0 - frac) + best as f32 * frac;
                mix.round().clamp(0.0, 255.0) as u8
            } else {
                best
            };
            dst[here] = blended;
            dst_idx[here] = if src[here] >= best { src_idx[here] } else { best_idx };
        }
    }
    (dst, dst_idx)
}

fn erode_alpha(src: &[u8], width: i32, height: i32, amount: f32) -> Vec<u8> {
    let radius = amount.round().max(1.0) as i32;
    let mut dst = vec![0u8; src.len()];
    for y in 0..height {
        for x in 0..width {
            let mut worst: u8 = 255;
            for dy in -radius..=radius {
                let yy = y + dy;
                if yy < 0 || yy >= height {
                    worst = 0;
                    break;
                }
                for dx in -radius..=radius {
                    let xx = x + dx;
                    if xx < 0 || xx >= width {
                        worst = 0;
                        break;
                    }
                    let v = src[(yy * width + xx) as usize];
                    if v < worst {
                        worst = v;
                    }
                }
                if worst == 0 {
                    break;
                }
            }
            dst[(y * width + x) as usize] = worst;
        }
    }
    dst
}

pub struct GuiPrimitive {
    pub(crate) state: Arc<Mutex<PrimitiveState>>,
}

impl GuiPrimitive {
    pub fn state_arc(&self) -> Arc<Mutex<PrimitiveState>> {
        self.state.clone()
    }

    pub fn from_state(state: Arc<Mutex<PrimitiveState>>) -> Self {
        Self { state }
    }

    pub fn new(lua: &Lua, shape: Shape) -> mlua::Result<Self> {
        Self::with_state(lua, shape, None, None, Dim::new(100.0, 100.0))
    }

    fn new_image(lua: &Lua, asset: &ImageAsset) -> mlua::Result<Self> {
        let image = ImageRef {
            id: asset.id,
            width: asset.width,
            height: asset.height,
            data: asset.data.clone(),
        };

        let size = Dim::new(asset.width as f32, asset.height as f32);
        Self::with_state(lua, Shape::Image, Some(Arc::new(image)), None, size)
    }

    fn new_text(lua: &Lua, asset: &FontAsset) -> mlua::Result<Self> {
        let text_state = TextState {
            font_id: asset.id,
            font: asset.font.clone(),
            content: String::new(),
            size_px: 24.0,
            style: FontStyle::Regular,
            underline: false,
            strikethrough: false,
            baked: None,
            baked_color: None,
        };

        Self::with_state(lua, Shape::Text, None, Some(text_state), Dim::new(0.0, 0.0))
    }

    fn with_state(
        lua: &Lua,
        shape: Shape,
        image: Option<Arc<ImageRef>>,
        text: Option<TextState>,
        size: Dim,
    ) -> mlua::Result<Self> {
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        let changed_signal = signal::new_instance(lua)?;
        let state = Arc::new(Mutex::new(PrimitiveState {
            id,
            shape,
            size,
            position: Dim::new(0.0, 0.0),
            anchor: Dim::new(0.0, 0.0),
            rotation: 0.0,
            color: Color3::new(1.0, 1.0, 1.0),
            transparency: 0.0,
            z_index: 0,
            visible: false,
            alive: true,
            attached: Vec::new(),
            changed_signal,
            image,
            text,
            dyn_img_owner: None,
            prop_signals: HashMap::new(),
            clip_parent: None,
            clip_shape: Shape::Square,
            billboard_parent: None,
        }));
        REGISTRY.with(|cell| cell.borrow_mut().push(state.clone()));
        bump_dirty();
        Ok(Self { state })
    }

    fn ensure_alive(&self, op: &str) -> mlua::Result<()> {
        let s = self.state.lock().unwrap();
        if !s.alive {
            return Err(mlua::Error::RuntimeError(format!(
                "GUI: {op} called on a destroyed primitive"
            )));
        }
        Ok(())
    }
}

fn fire_changed(lua: &Lua, signal_table: Table, prop: &str) -> mlua::Result<()> {
    bump_dirty();
    let mut args = MultiValue::new();
    args.push_back(Value::String(lua.create_string(prop)?));
    signal::fire(lua, &signal_table, args)
}

fn fire_prop_changed(lua: &Lua, prop_sig: Option<Table>, value: Value) {
    if let Some(sig) = prop_sig {
        let mut args = MultiValue::new();
        args.push_back(value);
        let _ = signal::fire(lua, &sig, args);
    }
}

fn ensure_prop_signal(
    lua: &Lua,
    state: &mut PrimitiveState,
    prop: &str,
) -> mlua::Result<Table> {
    if let Some(sig) = state.prop_signals.get(prop) {
        return Ok(sig.clone());
    }
    let sig = signal::new_instance(lua)?;
    state.prop_signals.insert(prop.to_string(), sig.clone());
    Ok(sig)
}

static GUI_VERSION: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

pub fn bump_dirty() {
    GUI_VERSION.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
}

pub fn current_version() -> u64 {
    GUI_VERSION.load(std::sync::atomic::Ordering::Relaxed)
}

pub(crate) fn build_attached(
    asset: &AnyUserData,
    priority: i32,
) -> mlua::Result<AttachedShader> {
    let (id, source, code) = if let Ok(s) = asset.borrow::<ShaderAsset>() {
        (s.id, s.source.clone(), s.code.clone())
    } else if let Ok(f) = asset.borrow::<FragmentAsset>() {
        (f.id, f.source.clone(), f.code.clone())
    } else {
        return Err(mlua::Error::RuntimeError(
            "expected a Shader or Fragment asset".into(),
        ));
    };

    let slot_of_name = parse_param_decls(&code);
    let prelude = render::FRAGMENT_PRELUDE;
    let wgsl = format!("{prelude}\n{code}");

    Ok(AttachedShader {
        id,
        source,
        wgsl: Arc::new(wgsl),
        slot_of_name: Arc::new(slot_of_name),
        params: Arc::new(Mutex::new([0.0_f32; 16])),
        priority,
    })
}

fn parse_param_decls(src: &str) -> std::collections::HashMap<String, u8> {
    let mut map = std::collections::HashMap::new();
    let mut next_slot: u8 = 0;
    for raw in src.lines() {
        let line = raw.trim();
        let rest = if let Some(r) = line.strip_prefix("//") {
            r.trim_start()
        } else if let Some(r) = line.strip_prefix("/*") {
            r.trim_start()
        } else {
            continue;
        };
        let Some(rest) = rest.strip_prefix("@ruzit") else {
            continue;
        };
        let rest = rest.trim_start();
        let Some(rest) = rest.strip_prefix("param") else {
            continue;
        };

        let name = rest.split_whitespace().next().unwrap_or("").to_string();
        if name.is_empty() {
            continue;
        }
        if next_slot >= 16 {
            eprintln!("[GUI] shader has more than 16 @ruzit params; '{name}' ignored");
            continue;
        }
        map.entry(name).or_insert(next_slot);
        next_slot += 1;
    }
    map
}

impl UserData for GuiPrimitive {
    fn add_fields<F: UserDataFields<Self>>(f: &mut F) {
        f.add_field_method_get("Changed", |_, this| {
            Ok(this.state.lock().unwrap().changed_signal.clone())
        });
        f.add_field_method_get("Size", |_, this| Ok(this.state.lock().unwrap().size));
        f.add_field_method_set("Size", |lua, this, value: AnyUserData| {
            this.ensure_alive("set Size")?;
            let dim = *value
                .borrow::<Dim>()
                .map_err(|_| mlua::Error::RuntimeError("Size expects a Primitives.Dim".into()))?;
            let (signal_table, prop_sig) = {
                let mut s = this.state.lock().unwrap();
                s.size = dim;
                (
                    s.changed_signal.clone(),
                    s.prop_signals.get("Size").cloned(),
                )
            };
            fire_changed(lua, signal_table, "Size")?;
            fire_prop_changed(lua, prop_sig, Value::UserData(lua.create_userdata(dim)?));
            Ok(())
        });
        f.add_field_method_get("Position", |_, this| {
            Ok(this.state.lock().unwrap().position)
        });
        f.add_field_method_set("Position", |lua, this, value: AnyUserData| {
            this.ensure_alive("set Position")?;
            let dim = *value.borrow::<Dim>().map_err(|_| {
                mlua::Error::RuntimeError("Position expects a Primitives.Dim".into())
            })?;
            let (signal_table, prop_sig) = {
                let mut s = this.state.lock().unwrap();
                s.position = dim;
                (
                    s.changed_signal.clone(),
                    s.prop_signals.get("Position").cloned(),
                )
            };
            fire_changed(lua, signal_table, "Position")?;
            fire_prop_changed(lua, prop_sig, Value::UserData(lua.create_userdata(dim)?));
            Ok(())
        });
        // Raw anchor (0..1) forwarded by Declar so the engine can re-centre baked
        // text. No signals -- it's a layout input, not an observable visual prop.
        f.add_field_method_get("AnchorPoint", |_, this| {
            Ok(this.state.lock().unwrap().anchor)
        });
        f.add_field_method_set("AnchorPoint", |_, this, value: AnyUserData| {
            let dim = *value.borrow::<Dim>().map_err(|_| {
                mlua::Error::RuntimeError("AnchorPoint expects a Primitives.Dim".into())
            })?;
            this.state.lock().unwrap().anchor = dim;
            Ok(())
        });
        f.add_field_method_get("Rotation", |_, this| {
            Ok(this.state.lock().unwrap().rotation)
        });
        f.add_field_method_set("Rotation", |lua, this, deg: f32| {
            this.ensure_alive("set Rotation")?;
            let (signal_table, prop_sig) = {
                let mut s = this.state.lock().unwrap();
                s.rotation = deg;
                (
                    s.changed_signal.clone(),
                    s.prop_signals.get("Rotation").cloned(),
                )
            };
            fire_changed(lua, signal_table, "Rotation")?;
            fire_prop_changed(lua, prop_sig, Value::Number(deg as f64));
            Ok(())
        });

        f.add_field_method_get("Color", |_, this| Ok(this.state.lock().unwrap().color));
        f.add_field_method_set("Color", |lua, this, value: AnyUserData| {
            this.ensure_alive("set Color")?;
            let color = *value.borrow::<Color3>().map_err(|_| {
                mlua::Error::RuntimeError("Color expects a Primitives.Color3".into())
            })?;
            let (signal_table, prop_sig) = {
                let mut s = this.state.lock().unwrap();
                s.color = color;
                (
                    s.changed_signal.clone(),
                    s.prop_signals.get("Color").cloned(),
                )
            };
            fire_changed(lua, signal_table, "Color")?;
            fire_prop_changed(lua, prop_sig, Value::UserData(lua.create_userdata(color)?));
            Ok(())
        });
        f.add_field_method_get("Transparency", |_, this| {
            Ok(this.state.lock().unwrap().transparency)
        });
        f.add_field_method_set("Transparency", |lua, this, value: f32| {
            this.ensure_alive("set Transparency")?;
            let clamped = value.clamp(0.0, 1.0);
            let (signal_table, prop_sig) = {
                let mut s = this.state.lock().unwrap();
                s.transparency = clamped;
                (
                    s.changed_signal.clone(),
                    s.prop_signals.get("Transparency").cloned(),
                )
            };
            fire_changed(lua, signal_table, "Transparency")?;
            fire_prop_changed(lua, prop_sig, Value::Number(clamped as f64));
            Ok(())
        });
        f.add_field_method_get("ZIndex", |_, this| {
            Ok(this.state.lock().unwrap().z_index as i64)
        });
        f.add_field_method_set("ZIndex", |lua, this, value: i64| {
            this.ensure_alive("set ZIndex")?;
            let clamped = value.clamp(i32::MIN as i64, i32::MAX as i64) as i32;
            let (signal_table, prop_sig) = {
                let mut s = this.state.lock().unwrap();
                s.z_index = clamped;
                (
                    s.changed_signal.clone(),
                    s.prop_signals.get("ZIndex").cloned(),
                )
            };
            fire_changed(lua, signal_table, "ZIndex")?;
            fire_prop_changed(lua, prop_sig, Value::Integer(clamped as i64));
            Ok(())
        });
        f.add_field_method_get("Visible", |_, this| Ok(this.state.lock().unwrap().visible));
        f.add_field_method_set("Visible", |lua, this, value: bool| {
            this.ensure_alive("set Visible")?;
            let (signal_table, prop_sig) = {
                let mut s = this.state.lock().unwrap();
                s.visible = value;
                (
                    s.changed_signal.clone(),
                    s.prop_signals.get("Visible").cloned(),
                )
            };
            fire_changed(lua, signal_table, "Visible")?;
            fire_prop_changed(lua, prop_sig, Value::Boolean(value));
            Ok(())
        });
        f.add_field_method_get("Shape", |_, this| {
            Ok(match this.state.lock().unwrap().shape {
                Shape::Circle => "Circle",
                Shape::Square => "Square",
                Shape::Triangle => "Triangle",
                Shape::Image => "Image",
                Shape::Text => "Text",
                Shape::Clippable => "Clippable",
            })
        });

        f.add_field_method_get("Text", |_, this| -> mlua::Result<String> {
            let s = this.state.lock().unwrap();
            Ok(s.text
                .as_ref()
                .map(|t| t.content.clone())
                .unwrap_or_default())
        });
        f.add_field_method_set("Text", |lua, this, value: String| {
            this.ensure_alive("set Text")?;
            let new_value = value.clone();
            let (signal_table, prop_sig, changed) = {
                let mut s = this.state.lock().unwrap();
                let ts = s.text.as_mut().ok_or_else(|| {
                    mlua::Error::RuntimeError("Text is only valid on Font primitives".into())
                })?;
                let changed = ts.content != value;
                if changed {
                    ts.content = value;
                    ts.invalidate();
                }
                (
                    s.changed_signal.clone(),
                    s.prop_signals.get("Text").cloned(),
                    changed,
                )
            };
            if changed {
                bump_dirty();
            }
            fire_changed(lua, signal_table, "Text")?;
            fire_prop_changed(lua, prop_sig, Value::String(lua.create_string(&new_value)?));
            Ok(())
        });
        f.add_field_method_get("TextSize", |_, this| -> mlua::Result<f32> {
            let s = this.state.lock().unwrap();
            Ok(s.text.as_ref().map(|t| t.size_px).unwrap_or(0.0))
        });
        f.add_field_method_set("TextSize", |lua, this, value: f32| {
            this.ensure_alive("set TextSize")?;
            let new_size = value.clamp(1.0, 1024.0);
            let (signal_table, prop_sig, changed) = {
                let mut s = this.state.lock().unwrap();
                let ts = s.text.as_mut().ok_or_else(|| {
                    mlua::Error::RuntimeError("TextSize is only valid on Font primitives".into())
                })?;
                let changed = ts.size_px != new_size;
                if changed {
                    ts.size_px = new_size;
                    ts.invalidate();
                }
                (
                    s.changed_signal.clone(),
                    s.prop_signals.get("TextSize").cloned(),
                    changed,
                )
            };
            if changed {
                // Snapshot is cached by current_version(); without bumping it
                // the renderer keeps serving the old baked text even though
                // ts.size_px and ts.baked have been updated.
                bump_dirty();
            }
            fire_changed(lua, signal_table, "TextSize")?;
            fire_prop_changed(lua, prop_sig, Value::Number(new_size as f64));
            Ok(())
        });
        f.add_field_method_get("FontStyle", |_, this| -> mlua::Result<String> {
            let s = this.state.lock().unwrap();
            Ok(s.text
                .as_ref()
                .map(|t| t.style.as_str().to_string())
                .unwrap_or_else(|| "Regular".to_string()))
        });
        f.add_field_method_set("FontStyle", |lua, this, value: String| {
            this.ensure_alive("set FontStyle")?;
            let parsed = FontStyle::parse(&value).ok_or_else(|| {
                mlua::Error::RuntimeError(format!(
                    "FontStyle: unknown style '{value}'. Use GUI.ListFontStyles() to enumerate."
                ))
            })?;
            let style_str = parsed.as_str().to_string();
            let (signal_table, prop_sig, changed) = {
                let mut s = this.state.lock().unwrap();
                let ts = s.text.as_mut().ok_or_else(|| {
                    mlua::Error::RuntimeError(
                        "FontStyle is only valid on Font primitives".into(),
                    )
                })?;
                let changed = ts.style != parsed;
                if changed {
                    ts.style = parsed;
                    ts.invalidate();
                }
                (
                    s.changed_signal.clone(),
                    s.prop_signals.get("FontStyle").cloned(),
                    changed,
                )
            };
            if changed {
                bump_dirty();
            }
            fire_changed(lua, signal_table, "FontStyle")?;
            fire_prop_changed(
                lua,
                prop_sig,
                Value::String(lua.create_string(&style_str)?),
            );
            Ok(())
        });

        f.add_field_method_get("Underline", |_, this| -> mlua::Result<bool> {
            let s = this.state.lock().unwrap();
            Ok(s.text.as_ref().map(|t| t.underline).unwrap_or(false))
        });
        f.add_field_method_set("Underline", |lua, this, value: bool| {
            this.ensure_alive("set Underline")?;
            let (signal_table, prop_sig, changed) = {
                let mut s = this.state.lock().unwrap();
                let ts = s.text.as_mut().ok_or_else(|| {
                    mlua::Error::RuntimeError(
                        "Underline is only valid on Font primitives".into(),
                    )
                })?;
                let changed = ts.underline != value;
                if changed {
                    ts.underline = value;
                    ts.invalidate();
                }
                (
                    s.changed_signal.clone(),
                    s.prop_signals.get("Underline").cloned(),
                    changed,
                )
            };
            if changed {
                bump_dirty();
            }
            fire_changed(lua, signal_table, "Underline")?;
            fire_prop_changed(lua, prop_sig, Value::Boolean(value));
            Ok(())
        });

        f.add_field_method_get("Strikethrough", |_, this| -> mlua::Result<bool> {
            let s = this.state.lock().unwrap();
            Ok(s.text.as_ref().map(|t| t.strikethrough).unwrap_or(false))
        });
        f.add_field_method_set("Strikethrough", |lua, this, value: bool| {
            this.ensure_alive("set Strikethrough")?;
            let (signal_table, prop_sig, changed) = {
                let mut s = this.state.lock().unwrap();
                let ts = s.text.as_mut().ok_or_else(|| {
                    mlua::Error::RuntimeError(
                        "Strikethrough is only valid on Font primitives".into(),
                    )
                })?;
                let changed = ts.strikethrough != value;
                if changed {
                    ts.strikethrough = value;
                    ts.invalidate();
                }
                (
                    s.changed_signal.clone(),
                    s.prop_signals.get("Strikethrough").cloned(),
                    changed,
                )
            };
            if changed {
                bump_dirty();
            }
            fire_changed(lua, signal_table, "Strikethrough")?;
            fire_prop_changed(lua, prop_sig, Value::Boolean(value));
            Ok(())
        });
    }

    fn add_methods<M: UserDataMethods<Self>>(m: &mut M) {
        m.add_method(
            "GetPropertyChanged",
            |lua, this, prop: String| -> mlua::Result<Table> {
                let mut s = this.state.lock().unwrap();
                ensure_prop_signal(lua, &mut s, &prop)
            },
        );

        m.add_method("ListFontStyles", |lua, this, _: ()| -> mlua::Result<Table> {
            {
                let s = this.state.lock().unwrap();
                if s.text.is_none() {
                    return Err(mlua::Error::RuntimeError(
                        "ListFontStyles: only valid on Font primitives".into(),
                    ));
                }
            }
            let t = lua.create_table()?;
            for (i, st) in FontStyle::ALL.iter().enumerate() {
                t.set(i as i64 + 1, st.as_str())?;
            }
            Ok(t)
        });

        m.add_method(
            "SetShape",
            |_, this, name: String| -> mlua::Result<()> {
                this.ensure_alive("SetShape")?;
                {
                    let s = this.state.lock().unwrap();
                    if !matches!(s.shape, Shape::Clippable) {
                        return Err(mlua::Error::RuntimeError(
                            "SetShape: only valid on a GUI.Basic.Clippable container".into(),
                        ));
                    }
                }
                let new_shape = match name.as_str() {
                    "Square" | "square" | "rect" | "Rectangle" => Shape::Square,
                    "Circle" | "circle" | "Ellipse" | "ellipse" => Shape::Circle,
                    "Triangle" | "triangle" => Shape::Triangle,
                    other => {
                        return Err(mlua::Error::RuntimeError(format!(
                            "SetShape: '{other}' is not a valid clip shape (use 'Square', 'Circle', or 'Triangle')"
                        )));
                    }
                };
                this.state.lock().unwrap().clip_shape = new_shape;
                bump_dirty();
                Ok(())
            },
        );

        m.add_method(
            "AddClippable",
            |_, this, child_ud: AnyUserData| -> mlua::Result<()> {
                this.ensure_alive("AddClippable")?;
                {
                    let s = this.state.lock().unwrap();
                    if !matches!(s.shape, Shape::Clippable) {
                        return Err(mlua::Error::RuntimeError(
                            "AddClippable: only valid on a GUI.Basic.Clippable container"
                                .into(),
                        ));
                    }
                }
                let child = child_ud.borrow::<GuiPrimitive>().map_err(|_| {
                    mlua::Error::RuntimeError(
                        "AddClippable expects a GUI primitive as the child".into(),
                    )
                })?;
                if Arc::ptr_eq(&this.state, &child.state) {
                    return Err(mlua::Error::RuntimeError(
                        "AddClippable: a Clippable cannot clip itself".into(),
                    ));
                }
                child.state.lock().unwrap().clip_parent = Some(this.state.clone());
                bump_dirty();
                Ok(())
            },
        );

        m.add_method(
            "RemoveClippable",
            |_, this, child_ud: AnyUserData| -> mlua::Result<()> {
                let child = child_ud.borrow::<GuiPrimitive>().map_err(|_| {
                    mlua::Error::RuntimeError(
                        "RemoveClippable expects a GUI primitive as the child".into(),
                    )
                })?;
                let mut s = child.state.lock().unwrap();
                if let Some(parent) = &s.clip_parent {
                    if Arc::ptr_eq(parent, &this.state) {
                        s.clip_parent = None;
                        bump_dirty();
                    }
                }
                Ok(())
            },
        );

        m.add_method("Destroy", |lua, this, _: ()| -> mlua::Result<()> {
            let signal_table = {
                let mut s = this.state.lock().unwrap();
                if !s.alive {
                    return Ok(());
                }
                s.alive = false;
                s.visible = false;
                s.attached.clear();
                s.changed_signal.clone()
            };
            fire_changed(lua, signal_table, "Destroyed")
        });

        m.add_method(
            "AttachShader",
            |_, this, (asset, priority): (AnyUserData, Option<i32>)| -> mlua::Result<()> {
                this.ensure_alive("AttachShader")?;
                let attached = build_attached(&asset, priority.unwrap_or(0))?;
                let mut s = this.state.lock().unwrap();
                if s.attached.iter().any(|e| e.id == attached.id) {
                    return Err(mlua::Error::RuntimeError(
                        "AttachShader: shader is already attached".into(),
                    ));
                }
                s.attached.push(attached);
                Ok(())
            },
        );
        m.add_method(
            "DetachShader",
            |_, this, asset: AnyUserData| -> mlua::Result<()> {
                this.ensure_alive("DetachShader")?;
                let id = shader_asset_id(&asset)?;
                let mut s = this.state.lock().unwrap();
                s.attached.retain(|e| e.id != id);
                Ok(())
            },
        );
        m.add_method(
            "SetData",
            |_, this, (asset, name, value): (AnyUserData, String, f32)| -> mlua::Result<()> {
                this.ensure_alive("SetData")?;
                let id = shader_asset_id(&asset)?;
                let s = this.state.lock().unwrap();
                let entry = s.attached.iter().find(|e| e.id == id).ok_or_else(|| {
                    mlua::Error::RuntimeError(
                        "SetData: shader is not attached to this primitive".into(),
                    )
                })?;
                let slot = *entry.slot_of_name.get(&name).ok_or_else(|| {
                    mlua::Error::RuntimeError(format!(
                        "SetData: '{name}' is not a `// @ruzit param` declared in the shader"
                    ))
                })?;
                entry.params.lock().unwrap()[slot as usize] = value;
                Ok(())
            },
        );
        m.add_method(
            "GetData",
            |_, this, (asset, name): (AnyUserData, String)| -> mlua::Result<Option<f32>> {
                let id = shader_asset_id(&asset)?;
                let s = this.state.lock().unwrap();
                let entry = s.attached.iter().find(|e| e.id == id).ok_or_else(|| {
                    mlua::Error::RuntimeError(
                        "GetData: shader is not attached to this primitive".into(),
                    )
                })?;
                let Some(slot) = entry.slot_of_name.get(&name) else {
                    return Ok(None);
                };
                Ok(Some(entry.params.lock().unwrap()[*slot as usize]))
            },
        );
        m.add_method("ClearShaders", |_, this, _: ()| -> mlua::Result<()> {
            this.ensure_alive("ClearShaders")?;
            this.state.lock().unwrap().attached.clear();
            Ok(())
        });
    }
}

pub(crate) fn shader_asset_id(asset: &AnyUserData) -> mlua::Result<u64> {
    if let Ok(s) = asset.borrow::<ShaderAsset>() {
        return Ok(s.id);
    }
    if let Ok(f) = asset.borrow::<FragmentAsset>() {
        return Ok(f.id);
    }
    Err(mlua::Error::RuntimeError(
        "expected a Shader or Fragment asset".into(),
    ))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SceneSlot {
    Skybox,
    PostEffect,
}

pub struct SceneShaderState {
    pub id: u64,
    pub wgsl: Arc<String>,
    pub slot_of_name: Arc<HashMap<String, u8>>,
    pub params: Arc<Mutex<[f32; 16]>>,
}

pub fn skybox_snapshot() -> Option<Arc<SceneShaderState>> {
    SKYBOX.with(|c| c.borrow().as_ref().cloned())
}

pub fn post_effect_snapshot() -> Option<Arc<SceneShaderState>> {
    POST_EFFECT.with(|c| c.borrow().as_ref().cloned())
}

pub fn build_scene_shader(asset: &AnyUserData) -> mlua::Result<Arc<SceneShaderState>> {
    let (id, code) = if let Ok(s) = asset.borrow::<ShaderAsset>() {
        (s.id, s.code.clone())
    } else if let Ok(f) = asset.borrow::<FragmentAsset>() {
        (f.id, f.code.clone())
    } else {
        return Err(mlua::Error::RuntimeError(
            "expected a Shader or Fragment asset".into(),
        ));
    };
    let slot_of_name = parse_param_decls(&code);

    let prelude = render::FRAGMENT_PRELUDE;
    let wgsl = format!("{prelude}\n{code}");
    Ok(Arc::new(SceneShaderState {
        id,
        wgsl: Arc::new(wgsl),
        slot_of_name: Arc::new(slot_of_name),
        params: Arc::new(Mutex::new([0.0_f32; 16])),
    }))
}

pub struct SceneShader {
    pub slot: SceneSlot,
    pub state: Arc<SceneShaderState>,
}

impl SceneShader {
    fn current_in_slot(&self) -> bool {
        let cur = match self.slot {
            SceneSlot::Skybox => SKYBOX.with(|c| c.borrow().as_ref().map(|s| s.id)),
            SceneSlot::PostEffect => POST_EFFECT.with(|c| c.borrow().as_ref().map(|s| s.id)),
        };
        cur == Some(self.state.id)
    }
}

impl UserData for SceneShader {
    fn add_methods<M: UserDataMethods<Self>>(m: &mut M) {
        m.add_method(
            "SetData",
            |_, this, (name, value): (String, f32)| -> mlua::Result<()> {
                let slot = *this.state.slot_of_name.get(&name).ok_or_else(|| {
                    mlua::Error::RuntimeError(format!(
                        "SetData: '{name}' is not a `// @ruzit param` declared in the shader"
                    ))
                })?;
                this.state.params.lock().unwrap()[slot as usize] = value;
                Ok(())
            },
        );
        m.add_method(
            "GetData",
            |_, this, name: String| -> mlua::Result<Option<f32>> {
                let Some(slot) = this.state.slot_of_name.get(&name) else {
                    return Ok(None);
                };
                Ok(Some(this.state.params.lock().unwrap()[*slot as usize]))
            },
        );

        m.add_method("Destroy", |_, this, _: ()| -> mlua::Result<()> {
            if this.current_in_slot() {
                match this.slot {
                    SceneSlot::Skybox => SKYBOX.with(|c| *c.borrow_mut() = None),
                    SceneSlot::PostEffect => POST_EFFECT.with(|c| *c.borrow_mut() = None),
                }
            }
            Ok(())
        });
    }
}

pub fn install_scene_shader(slot: SceneSlot, state: Arc<SceneShaderState>) {
    match slot {
        SceneSlot::Skybox => SKYBOX.with(|c| *c.borrow_mut() = Some(state)),
        SceneSlot::PostEffect => POST_EFFECT.with(|c| *c.borrow_mut() = Some(state)),
    }
}

pub fn clear_scene_shader(slot: SceneSlot) {
    match slot {
        SceneSlot::Skybox => SKYBOX.with(|c| *c.borrow_mut() = None),
        SceneSlot::PostEffect => POST_EFFECT.with(|c| *c.borrow_mut() = None),
    }
}

pub fn create(lua: &Lua) -> mlua::Result<Table> {
    let t = lua.create_table()?;

    let basic = lua.create_table()?;
    basic.set(
        "Circle",
        lua.create_function(|lua, _: ()| GuiPrimitive::new(lua, Shape::Circle))?,
    )?;
    basic.set(
        "Square",
        lua.create_function(|lua, _: ()| GuiPrimitive::new(lua, Shape::Square))?,
    )?;
    basic.set(
        "Triangle",
        lua.create_function(|lua, _: ()| GuiPrimitive::new(lua, Shape::Triangle))?,
    )?;
    basic.set(
        "Clippable",
        lua.create_function(|lua, _: ()| GuiPrimitive::new(lua, Shape::Clippable))?,
    )?;
    basic.set(
        "Spline",
        lua.create_function(|lua, _: ()| spline::Spline::new(lua))?,
    )?;
    basic.set(
        "Image",
        lua.create_function(|lua, asset: AnyUserData| -> mlua::Result<GuiPrimitive> {
            let img = asset.borrow::<ImageAsset>().map_err(|_| {
                mlua::Error::RuntimeError(
                    "GUI.Basic.Image expects an ImageAsset (Asset.GetAsset(\"Image\", ...))".into(),
                )
            })?;
            GuiPrimitive::new_image(lua, &img)
        })?,
    )?;
    basic.set(
        "Font",
        lua.create_function(|lua, asset: AnyUserData| -> mlua::Result<GuiPrimitive> {
            let font = asset.borrow::<FontAsset>().map_err(|_| {
                mlua::Error::RuntimeError(
                    "GUI.Basic.Font expects a FontAsset (Asset.GetAsset(\"Font\", ...))".into(),
                )
            })?;
            GuiPrimitive::new_text(lua, &font)
        })?,
    )?;
    t.set("Basic", basic)?;

    t.set(
        "DrawableImg",
        lua.create_function(
            |lua, (width, height): (u32, u32)| -> mlua::Result<AnyUserData> {
                if width == 0 || height == 0 {
                    return Err(mlua::Error::RuntimeError(
                        "GUI.DrawableImg: width and height must be > 0".into(),
                    ));
                }
                let state = crate::libs::drawable::new_drawable(width, height);
                lua.create_userdata(crate::libs::drawable::DrawableImgHandle { inner: state })
            },
        )?,
    )?;
    t.set(
        "CanvasBuffer",
        lua.create_function(
            |lua, (width, height): (u32, u32)| -> mlua::Result<AnyUserData> {
                if width == 0 || height == 0 {
                    return Err(mlua::Error::RuntimeError(
                        "GUI.CanvasBuffer: width and height must be > 0".into(),
                    ));
                }
                let state = crate::libs::drawable::new_canvas_buffer(width, height);
                lua.create_userdata(crate::libs::drawable::CanvasBufferHandle { inner: state })
            },
        )?,
    )?;
    t.set(
        "AnimatedImage",
        lua.create_function(|lua, args: MultiValue| {
            crate::libs::anim_image::make_animated_image(lua, args)
        })?,
    )?;

    t.set("Raycast", lua.create_function(raycast_2d)?)?;
    t.set("CheckArea", lua.create_function(check_area_2d)?)?;
    t.set("OverlapSphere", lua.create_function(overlap_sphere_2d)?)?;
    t.set("OverlapBox", lua.create_function(overlap_box_2d)?)?;
    t.set("OverlapFrustum", lua.create_function(overlap_frustum_2d)?)?;
    t.set("GetItemsInZone", lua.create_function(get_items_in_zone_2d)?)?;

    t.set(
        "UIEffectVolume",
        lua.create_function(
            |_, image: Option<AnyUserData>| -> mlua::Result<UIEffectVolumeHandle> {
                effect_volume::new_ui_effect_volume(image)
            },
        )?,
    )?;

    Ok(t)
}

fn dim_or_table_to_xy(v: &Value, name: &str) -> mlua::Result<[f32; 2]> {
    match v {
        Value::UserData(ud) => {
            if let Ok(d) = ud.borrow::<Dim>() {
                return Ok([d.x, d.y]);
            }
            Err(mlua::Error::RuntimeError(format!(
                "{name}: expected a Dim or {{ X, Y }} table"
            )))
        }
        Value::Table(t) => {
            let x: f32 = t.get(1).or_else(|_| t.get("X")).or_else(|_| t.get("x"))?;
            let y: f32 = t.get(2).or_else(|_| t.get("Y")).or_else(|_| t.get("y"))?;
            Ok([x, y])
        }
        _ => Err(mlua::Error::RuntimeError(format!(
            "{name}: expected a Dim or {{ X, Y }} table"
        ))),
    }
}

fn apply_filter_and_collect(
    lua: &Lua,
    hits: Vec<Arc<Mutex<PrimitiveState>>>,
    filter: Option<mlua::Function>,
) -> mlua::Result<Table> {
    let out = lua.create_table()?;
    let mut idx = 1;
    for state_arc in hits {
        let keep = match &filter {
            Some(f) => {
                let gp = GuiPrimitive::from_state(state_arc.clone());
                let v: Value = f.call(gp)?;
                matches!(v, Value::Boolean(true))
            }
            None => true,
        };
        if keep {
            out.set(idx, GuiPrimitive::from_state(state_arc))?;
            idx += 1;
        }
    }
    Ok(out)
}

fn raycast_2d(lua: &Lua, args: MultiValue) -> mlua::Result<Value> {
    let mut iter = args.into_iter();
    let origin_v = iter.next().ok_or_else(|| {
        mlua::Error::RuntimeError("GUI.Raycast: missing origin (Dim or {x, y})".into())
    })?;
    let dir_v = iter.next().ok_or_else(|| {
        mlua::Error::RuntimeError("GUI.Raycast: missing direction (Dim or {x, y})".into())
    })?;
    let filter_v = iter.next().unwrap_or(Value::Nil);
    let max_dist_v = iter.next().unwrap_or(Value::Nil);

    let origin = dim_or_table_to_xy(&origin_v, "GUI.Raycast: origin")?;
    let direction = dim_or_table_to_xy(&dir_v, "GUI.Raycast: direction")?;
    let dir_len = (direction[0] * direction[0] + direction[1] * direction[1]).sqrt();
    if dir_len < 1e-6 {
        return Err(mlua::Error::RuntimeError(
            "GUI.Raycast: direction has zero length".into(),
        ));
    }
    let filter: Option<mlua::Function> = match filter_v {
        Value::Nil => None,
        Value::Function(f) => Some(f),
        other => {
            return Err(mlua::Error::RuntimeError(format!(
                "GUI.Raycast: filter must be a function or nil (got {})",
                other.type_name()
            )))
        }
    };
    let max_dist = match max_dist_v {
        Value::Nil => 1.0e6,
        Value::Integer(n) => n as f32,
        Value::Number(n) => n as f32,
        _ => {
            return Err(mlua::Error::RuntimeError(
                "GUI.Raycast: maxDistance must be a number or nil".into(),
            ))
        }
    };

    let hits = match spatial::gpu_raycast_2d(origin, direction, max_dist) {
        Some(h) => h,
        None => return Ok(Value::Nil),
    };

    for h in hits {
        let keep = match &filter {
            Some(f) => {
                let gp = GuiPrimitive::from_state(h.state.clone());
                let v: Value = f.call(gp)?;
                matches!(v, Value::Boolean(true))
            }
            None => true,
        };
        if !keep {
            continue;
        }
        let out = lua.create_table()?;
        out.set("Primitive", GuiPrimitive::from_state(h.state.clone()))?;
        out.set("Distance", h.distance)?;
        out.set("Position", Dim::new(h.position[0], h.position[1]))?;
        return Ok(Value::Table(out));
    }
    Ok(Value::Nil)
}

fn parse_filter(v: Value, label: &str) -> mlua::Result<Option<mlua::Function>> {
    match v {
        Value::Nil => Ok(None),
        Value::Function(f) => Ok(Some(f)),
        other => Err(mlua::Error::RuntimeError(format!(
            "{label}: filter must be a function or nil (got {})",
            other.type_name()
        ))),
    }
}

fn check_area_2d(lua: &Lua, args: MultiValue) -> mlua::Result<Table> {
    let mut iter = args.into_iter();
    let center_v = iter
        .next()
        .ok_or_else(|| mlua::Error::RuntimeError("GUI.CheckArea: missing center".into()))?;
    let size_v = iter
        .next()
        .ok_or_else(|| mlua::Error::RuntimeError("GUI.CheckArea: missing size".into()))?;
    let _quality = iter.next().unwrap_or(Value::Nil);
    let filter = parse_filter(iter.next().unwrap_or(Value::Nil), "GUI.CheckArea")?;

    let c = dim_or_table_to_xy(&center_v, "GUI.CheckArea: center")?;
    let s = dim_or_table_to_xy(&size_v, "GUI.CheckArea: size")?;
    let lo = [c[0] - s[0] * 0.5, c[1] - s[1] * 0.5];
    let hi = [c[0] + s[0] * 0.5, c[1] + s[1] * 0.5];

    let hits = spatial::gpu_overlap_2d(spatial::OverlapShape2D::Aabb { lo, hi })
        .unwrap_or_default();
    apply_filter_and_collect(lua, hits, filter)
}

fn overlap_sphere_2d(lua: &Lua, args: MultiValue) -> mlua::Result<Table> {
    let mut iter = args.into_iter();
    let center_v = iter.next().ok_or_else(|| {
        mlua::Error::RuntimeError("GUI.OverlapSphere: missing center".into())
    })?;
    let radius_v = iter.next().ok_or_else(|| {
        mlua::Error::RuntimeError("GUI.OverlapSphere: missing radius".into())
    })?;
    let filter = parse_filter(iter.next().unwrap_or(Value::Nil), "GUI.OverlapSphere")?;

    let center = dim_or_table_to_xy(&center_v, "GUI.OverlapSphere: center")?;
    let radius: f32 = match radius_v {
        Value::Integer(n) => n as f32,
        Value::Number(n) => n as f32,
        _ => {
            return Err(mlua::Error::RuntimeError(
                "GUI.OverlapSphere: radius must be a number".into(),
            ))
        }
    };
    let hits = spatial::gpu_overlap_2d(spatial::OverlapShape2D::Circle { center, radius })
        .unwrap_or_default();
    apply_filter_and_collect(lua, hits, filter)
}

fn overlap_box_2d(lua: &Lua, args: MultiValue) -> mlua::Result<Table> {
    let mut iter = args.into_iter();
    let center_v = iter
        .next()
        .ok_or_else(|| mlua::Error::RuntimeError("GUI.OverlapBox: missing center".into()))?;
    let size_v = iter
        .next()
        .ok_or_else(|| mlua::Error::RuntimeError("GUI.OverlapBox: missing size".into()))?;
    let rotation_v = iter.next().unwrap_or(Value::Nil);
    let filter = parse_filter(iter.next().unwrap_or(Value::Nil), "GUI.OverlapBox")?;

    let center = dim_or_table_to_xy(&center_v, "GUI.OverlapBox: center")?;
    let size = dim_or_table_to_xy(&size_v, "GUI.OverlapBox: size")?;
    let rotation = match rotation_v {
        Value::Nil => 0.0_f32,
        Value::Integer(n) => n as f32,
        Value::Number(n) => n as f32,
        _ => {
            return Err(mlua::Error::RuntimeError(
                "GUI.OverlapBox: rotation must be a number (radians) or nil".into(),
            ))
        }
    };
    let hits = spatial::gpu_overlap_2d(spatial::OverlapShape2D::Box {
        center,
        size,
        rotation,
    })
    .unwrap_or_default();
    apply_filter_and_collect(lua, hits, filter)
}

fn overlap_frustum_2d(lua: &Lua, args: MultiValue) -> mlua::Result<Table> {
    let filter = parse_filter(args.into_iter().next().unwrap_or(Value::Nil), "GUI.OverlapFrustum")?;
    let vp = spatial::viewport_bounds()
        .unwrap_or([0.0, 0.0, 1920.0, 1080.0]);
    let hits = spatial::gpu_overlap_2d(spatial::OverlapShape2D::Aabb {
        lo: [vp[0], vp[1]],
        hi: [vp[2], vp[3]],
    })
    .unwrap_or_default();
    apply_filter_and_collect(lua, hits, filter)
}

fn get_items_in_zone_2d(lua: &Lua, args: MultiValue) -> mlua::Result<Table> {
    let mut iter = args.into_iter();
    let cframe_v = iter.next().ok_or_else(|| {
        mlua::Error::RuntimeError("GUI.GetItemsInZone: missing cframe / center".into())
    })?;
    let size_v = iter
        .next()
        .ok_or_else(|| mlua::Error::RuntimeError("GUI.GetItemsInZone: missing size".into()))?;
    let filter = parse_filter(iter.next().unwrap_or(Value::Nil), "GUI.GetItemsInZone")?;

    let (center, rotation) = match &cframe_v {
        Value::UserData(ud) => {
            if let Ok(cf) = ud.borrow::<crate::libs::primitives::CFrame>() {
                ([cf.position.x, cf.position.y], cf.rotation.z)
            } else if let Ok(d) = ud.borrow::<Dim>() {
                ([d.x, d.y], 0.0_f32)
            } else {
                return Err(mlua::Error::RuntimeError(
                    "GUI.GetItemsInZone: cframe must be a CFrame or Dim".into(),
                ));
            }
        }
        Value::Table(_) => (dim_or_table_to_xy(&cframe_v, "GUI.GetItemsInZone: center")?, 0.0_f32),
        _ => {
            return Err(mlua::Error::RuntimeError(
                "GUI.GetItemsInZone: cframe must be a CFrame, Dim, or {X, Y} table".into(),
            ))
        }
    };
    let size = dim_or_table_to_xy(&size_v, "GUI.GetItemsInZone: size")?;

    let hits = spatial::gpu_overlap_2d(spatial::OverlapShape2D::Box {
        center,
        size,
        rotation,
    })
    .unwrap_or_default();
    apply_filter_and_collect(lua, hits, filter)
}
