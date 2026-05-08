use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use base64::Engine;
use image::ImageReader;
use mlua::{Lua, Table, UserData, UserDataMethods, Value};

use crate::libs::sfx::{self, SoundData};
use crate::vfs::{self, Fs, split_owner};

static SHADER_ID: AtomicU64 = AtomicU64::new(1);

pub(crate) fn next_shader_id() -> u64 {
    SHADER_ID.fetch_add(1, Ordering::Relaxed)
}

const ASSET_CACHE_KEY: &str = "ruzit_asset_cache";

fn ensure_asset_cache(lua: &Lua) -> mlua::Result<Table> {
    if let Ok(t) = lua.named_registry_value::<Table>(ASSET_CACHE_KEY) {
        return Ok(t);
    }
    let t = lua.create_table()?;
    let meta = lua.create_table()?;
    meta.set("__mode", "v")?;
    t.set_metatable(Some(meta));
    lua.set_named_registry_value(ASSET_CACHE_KEY, t.clone())?;
    Ok(t)
}

fn cache_get(lua: &Lua, key: &str) -> Option<Value> {
    let cache = ensure_asset_cache(lua).ok()?;
    match cache.get::<Value>(key).ok()? {
        Value::Nil => None,
        v => Some(v),
    }
}

fn cache_set(lua: &Lua, key: &str, value: &Value) {
    if let Ok(cache) = ensure_asset_cache(lua) {
        let _ = cache.set(key, value.clone());
    }
}

pub fn create(lua: &Lua, fs: Fs, owner: String) -> mlua::Result<Table> {
    let t = lua.create_table()?;
    let fs_clone = fs.clone();
    let owner_clone = owner.clone();
    t.set(
        "GetAsset",
        lua.create_function(
            move |lua, (kind, path): (String, String)| -> mlua::Result<Value> {
                let cache_key = format!("vfs:{owner_clone}:{kind}:{path}");
                if let Some(hit) = cache_get(lua, &cache_key) {
                    return Ok(hit);
                }
                let value = get_asset(lua, &fs_clone, &owner_clone, &kind, &path)?;
                cache_set(lua, &cache_key, &value);
                Ok(value)
            },
        )?,
    )?;
    t.set(
        "FromString",
        lua.create_function(
            |lua,
             (kind, data, label): (String, mlua::String, Option<String>)|
             -> mlua::Result<Value> {
                let bytes = data.as_bytes().to_vec();
                let source = label.unwrap_or_else(|| format!("<string:{}>", kind));
                from_bytes(lua, &kind, bytes, source)
            },
        )?,
    )?;
    t.set(
        "ImportAsset",
        lua.create_function(
            |lua, (kind, path): (String, String)| -> mlua::Result<Value> {
                let p = std::path::Path::new(&path);
                let resolved_kind_for_key = if kind.is_empty() || kind == "Auto" {
                    detect_kind_from_extension(p).unwrap_or_else(|| kind.clone())
                } else {
                    kind.clone()
                };
                let cache_key = format!("import:{resolved_kind_for_key}:{path}");
                if let Some(hit) = cache_get(lua, &cache_key) {
                    return Ok(hit);
                }
                let bytes = std::fs::read(p).map_err(|e| {
                    mlua::Error::RuntimeError(format!("ImportAsset: read '{path}': {e}"))
                })?;
                let resolved_kind = if kind.is_empty() || kind == "Auto" {
                    detect_kind_from_extension(p).ok_or_else(|| {
                        mlua::Error::RuntimeError(format!(
                            "ImportAsset: cannot guess kind from '{path}'; pass an explicit kind"
                        ))
                    })?
                } else {
                    kind
                };
                let value = from_bytes(lua, &resolved_kind, bytes, path)?;
                cache_set(lua, &cache_key, &value);
                Ok(value)
            },
        )?,
    )?;
    t.set(
        "FromPixels",
        lua.create_function(
            |lua, (width, height, data): (u32, u32, mlua::String)| -> mlua::Result<Value> {
                let bytes = data.as_bytes();
                let expected = (width as usize) * (height as usize) * 4;
                if bytes.len() != expected {
                    return Err(mlua::Error::RuntimeError(format!(
                        "Asset.FromPixels: expected {expected} bytes ({width}x{height} RGBA), got {}",
                        bytes.len()
                    )));
                }
                let asset = ImageAsset {
                    id: next_shader_id(),
                    width,
                    height,
                    data: Arc::new(bytes.to_vec()),
                    source: format!("<pixels:{width}x{height}>"),
                };
                Ok(Value::UserData(lua.create_userdata(asset)?))
            },
        )?,
    )?;
    Ok(t)
}

pub fn from_bytes(lua: &Lua, kind: &str, bytes: Vec<u8>, source: String) -> mlua::Result<Value> {
    match kind {
        "Image" => parse_image(lua, bytes, source),
        "Sound" => Ok(Value::UserData(lua.create_userdata(SoundData {
            bytes: Arc::new(bytes),
            source,
        })?)),
        "Shader" => parse_text::<ShaderAsset>(lua, bytes, source),
        "Fragment" => parse_text::<FragmentAsset>(lua, bytes, source),
        "Model" => parse_model(lua, bytes, source),
        "Font" => parse_font(lua, bytes, source),
        "File" => {
            let s = String::from_utf8(bytes).map_err(|e| {
                mlua::Error::RuntimeError(format!("Asset.FromString: '{source}' not UTF-8: {e}"))
            })?;
            Ok(Value::String(lua.create_string(&s)?))
        }
        other => Err(mlua::Error::RuntimeError(format!(
            "Asset.FromString: unknown kind '{other}'"
        ))),
    }
}

fn detect_kind_from_extension(path: &Path) -> Option<String> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    if IMAGE_EXTS.iter().any(|e| *e == ext) {
        return Some("Image".into());
    }
    if SHADER_EXTS.iter().any(|e| *e == ext) {
        return Some("Shader".into());
    }
    if FRAGMENT_EXTS.iter().any(|e| *e == ext) {
        return Some("Fragment".into());
    }
    if MODEL_EXTS.iter().any(|e| *e == ext) {
        return Some("Model".into());
    }
    if FONT_EXTS.iter().any(|e| *e == ext) {
        return Some("Font".into());
    }
    if sfx::SOUND_EXTS.iter().any(|e| *e == ext) {
        return Some("Sound".into());
    }
    None
}

pub fn make_image_from_rgba(
    lua: &Lua,
    width: u32,
    height: u32,
    rgba: Vec<u8>,
    source: String,
) -> mlua::Result<Value> {
    let asset = ImageAsset {
        id: next_shader_id(),
        width,
        height,
        data: Arc::new(rgba),
        source,
    };
    Ok(Value::UserData(lua.create_userdata(asset)?))
}

const IMAGE_EXTS: &[&str] = &["png", "jpg", "jpeg", "bmp", "gif", "webp"];
const SHADER_EXTS: &[&str] = &["shader", "glsl", "wgsl", "hlsl", "vert", "metal"];
const FRAGMENT_EXTS: &[&str] = &["frag", "fragment", "fs", "glslf"];
const MODEL_EXTS: &[&str] = &["obj", "fbx"];
const FONT_EXTS: &[&str] = &["ttf", "otf"];

fn get_asset(lua: &Lua, fs: &Fs, owner: &str, kind: &str, path: &str) -> mlua::Result<Value> {
    match kind {
        "Image" => load_image(lua, fs, owner, path),
        "Sound" => load_sound(lua, fs, owner, path),
        "Shader" => load_text::<ShaderAsset>(lua, fs, owner, path, SHADER_EXTS, "Shader"),
        "Fragment" => load_text::<FragmentAsset>(lua, fs, owner, path, FRAGMENT_EXTS, "Fragment"),
        "Model" => load_model(lua, fs, owner, path),
        "Font" => load_font(lua, fs, owner, path),
        "File" => load_file(lua, fs, owner, path),
        other => Err(mlua::Error::RuntimeError(format!(
            "Asset.GetAsset: unknown kind '{other}' (try 'Image', 'Sound', 'Shader', 'Fragment', 'Model', 'Font', 'File')"
        ))),
    }
}

fn load_font(lua: &Lua, fs: &Fs, owner: &str, path: &str) -> mlua::Result<Value> {
    let (bytes, source) = read_bytes(fs, owner, path, FONT_EXTS, "Font")?;
    parse_font(lua, bytes, source)
}

fn parse_font(lua: &Lua, bytes: Vec<u8>, source: String) -> mlua::Result<Value> {
    let font = fontdue::Font::from_bytes(bytes.as_slice(), fontdue::FontSettings::default())
        .map_err(|e| mlua::Error::RuntimeError(format!("Font parse '{source}': {e}")))?;
    let asset = FontAsset {
        id: next_shader_id(),
        font: Arc::new(font),
        source,
    };
    Ok(Value::UserData(lua.create_userdata(asset)?))
}

fn load_file(lua: &Lua, fs: &Fs, owner: &str, path: &str) -> mlua::Result<Value> {
    let bytes = read_file_bytes(fs, owner, path)?;
    let s = String::from_utf8(bytes).map_err(|e| {
        mlua::Error::RuntimeError(format!(
            "Asset.GetAsset: File '{path}' not valid UTF-8: {e}"
        ))
    })?;
    Ok(Value::String(lua.create_string(&s)?))
}

fn read_file_bytes(fs: &Fs, owner: &str, path: &str) -> mlua::Result<Vec<u8>> {
    match fs {
        Fs::Disk { .. } => {
            let root = vfs::fs_root(fs);
            let full = root.join(path.replace('\\', "/"));
            if !full.is_file() {
                return Err(mlua::Error::RuntimeError(format!(
                    "Asset.GetAsset: File '{path}' not found under {}",
                    root.display()
                )));
            }
            std::fs::read(&full)
                .map_err(|e| mlua::Error::RuntimeError(format!("read {}: {e}", full.display())))
        }
        Fs::Bundle {
            packages,
            default_id,
            ..
        } => {
            let (target_id, rest_path) = if let Some(rest) = path.strip_prefix('@') {
                if let Some((id, inner)) = rest.split_once('/') {
                    (id.to_string(), inner.to_string())
                } else {
                    return Err(mlua::Error::RuntimeError(format!(
                        "Asset.GetAsset: bad package path '{path}'"
                    )));
                }
            } else {
                let (caller_pkg, _) = split_owner(owner, default_id);
                (caller_pkg.to_string(), path.to_string())
            };
            let pkg = packages.get(&target_id).ok_or_else(|| {
                mlua::Error::RuntimeError(format!(
                    "Asset.GetAsset: package '{target_id}' is not loaded"
                ))
            })?;
            let key = rest_path.replace('\\', "/");
            let b64 = pkg.assets.get(&key).ok_or_else(|| {
                mlua::Error::RuntimeError(format!(
                    "Asset.GetAsset: File '{key}' not found in package '{target_id}'"
                ))
            })?;
            let raw = base64::engine::general_purpose::STANDARD
                .decode(b64)
                .map_err(|e| {
                    mlua::Error::RuntimeError(format!("Asset.GetAsset: '{key}' base64 decode: {e}"))
                })?;
            if pkg.assets_compressed {
                zstd::stream::decode_all(raw.as_slice()).map_err(|e| {
                    mlua::Error::RuntimeError(format!("Asset.GetAsset: '{key}' zstd: {e}"))
                })
            } else {
                Ok(raw)
            }
        }
    }
}

fn load_model(lua: &Lua, fs: &Fs, owner: &str, path: &str) -> mlua::Result<Value> {
    let (bytes, source) = read_bytes(fs, owner, path, MODEL_EXTS, "Model")?;
    parse_model(lua, bytes, source)
}

fn parse_model(lua: &Lua, bytes: Vec<u8>, source: String) -> mlua::Result<Value> {
    let is_fbx =
        bytes.starts_with(b"Kaydara FBX Binary") || source.to_ascii_lowercase().ends_with(".fbx");

    let (mesh, animations) = if is_fbx {
        let loaded = crate::libs::renderable::mesh::load_fbx_full(&bytes)
            .map_err(|e| mlua::Error::RuntimeError(format!("Model parse '{source}': {e}")))?;
        (loaded.mesh, loaded.animations)
    } else {
        let text = String::from_utf8(bytes)
            .map_err(|e| mlua::Error::RuntimeError(format!("Model '{source}' not UTF-8: {e}")))?;
        let mesh = crate::libs::renderable::mesh::load_obj(&text)
            .map_err(|e| mlua::Error::RuntimeError(format!("Model parse '{source}': {e}")))?;
        (mesh, Vec::new())
    };
    let asset = ModelAsset {
        id: next_shader_id(),
        vertices: Arc::new(mesh.vertices),
        indices: Arc::new(mesh.indices),
        animations: Arc::new(animations),
        source,
    };
    Ok(Value::UserData(lua.create_userdata(asset)?))
}

fn load_sound(lua: &Lua, fs: &Fs, owner: &str, path: &str) -> mlua::Result<Value> {
    let (bytes, source) = read_bytes(fs, owner, path, sfx::SOUND_EXTS, "Sound")?;
    let data = SoundData {
        bytes: Arc::new(bytes),
        source,
    };
    Ok(Value::UserData(lua.create_userdata(data)?))
}

fn load_text<T: TextAsset + UserData + 'static>(
    lua: &Lua,
    fs: &Fs,
    owner: &str,
    path: &str,
    exts: &[&str],
    kind: &str,
) -> mlua::Result<Value> {
    let (bytes, source) = read_bytes(fs, owner, path, exts, kind)?;
    parse_text::<T>(lua, bytes, source)
}

fn parse_text<T: TextAsset + UserData + 'static>(
    lua: &Lua,
    bytes: Vec<u8>,
    source: String,
) -> mlua::Result<Value> {
    let code = String::from_utf8(bytes)
        .map_err(|e| mlua::Error::RuntimeError(format!("'{source}' not UTF-8: {e}")))?;
    Ok(Value::UserData(lua.create_userdata(T::make(code, source))?))
}

trait TextAsset {
    fn make(code: String, source: String) -> Self;
}

fn load_image(lua: &Lua, fs: &Fs, owner: &str, path: &str) -> mlua::Result<Value> {
    let (bytes, source) = read_bytes(fs, owner, path, IMAGE_EXTS, "Image")?;
    parse_image(lua, bytes, source)
}

fn parse_image(lua: &Lua, bytes: Vec<u8>, source: String) -> mlua::Result<Value> {
    let img = ImageReader::new(std::io::Cursor::new(&bytes))
        .with_guessed_format()
        .map_err(|e| mlua::Error::RuntimeError(format!("guess {source}: {e}")))?
        .decode()
        .map_err(|e| mlua::Error::RuntimeError(format!("decode {source}: {e}")))?;

    let rgba = img.to_rgba8();
    let (width, height) = rgba.dimensions();
    let data = rgba.into_raw();

    let asset = ImageAsset {
        id: next_shader_id(),
        width,
        height,
        data: Arc::new(data),
        source,
    };
    Ok(Value::UserData(lua.create_userdata(asset)?))
}

fn read_bytes(
    fs: &Fs,
    owner: &str,
    path: &str,
    exts: &[&str],
    kind: &str,
) -> mlua::Result<(Vec<u8>, String)> {
    match fs {
        Fs::Disk { .. } => {
            let root = vfs::fs_root(fs);
            let resolved = resolve_disk(&root, path, exts).ok_or_else(|| {
                mlua::Error::RuntimeError(format!(
                    "Asset.GetAsset: {kind} '{path}' not found under {}",
                    root.display()
                ))
            })?;
            let bytes = std::fs::read(&resolved).map_err(|e| {
                mlua::Error::RuntimeError(format!("read {}: {e}", resolved.display()))
            })?;
            Ok((bytes, resolved.to_string_lossy().into_owned()))
        }
        Fs::Bundle {
            packages,
            default_id,
            ..
        } => {
            let (target_id, rest_path) = if let Some(rest) = path.strip_prefix('@') {
                if let Some((id, inner)) = rest.split_once('/') {
                    (id.to_string(), inner.to_string())
                } else {
                    return Err(mlua::Error::RuntimeError(format!(
                        "Asset.GetAsset: bad package path '{path}'"
                    )));
                }
            } else {
                let (caller_pkg, _) = split_owner(owner, default_id);
                (caller_pkg.to_string(), path.to_string())
            };

            let pkg = packages.get(&target_id).ok_or_else(|| {
                mlua::Error::RuntimeError(format!(
                    "Asset.GetAsset: package '{target_id}' is not loaded"
                ))
            })?;
            let key = resolve_bundle(&pkg.assets, &rest_path, exts).ok_or_else(|| {
                mlua::Error::RuntimeError(format!(
                    "Asset.GetAsset: {kind} '{rest_path}' not found in package '{target_id}'"
                ))
            })?;

            let b64 = pkg.assets.get(&key).ok_or_else(|| {
                mlua::Error::RuntimeError(format!("Asset.GetAsset: '{key}' missing"))
            })?;
            let raw = base64::engine::general_purpose::STANDARD
                .decode(b64)
                .map_err(|e| {
                    mlua::Error::RuntimeError(format!("Asset.GetAsset: '{key}' base64 decode: {e}"))
                })?;
            let bytes = if pkg.assets_compressed {
                zstd::stream::decode_all(raw.as_slice()).map_err(|e| {
                    mlua::Error::RuntimeError(format!("Asset.GetAsset: '{key}' zstd: {e}"))
                })?
            } else {
                raw
            };
            Ok((bytes, format!("@{target_id}/{key}")))
        }
    }
}

fn resolve_disk(root: &Path, path: &str, exts: &[&str]) -> Option<PathBuf> {
    if let Some(idx) = path.rfind('.') {
        let ext = &path[idx + 1..];
        if exts.iter().any(|e| e.eq_ignore_ascii_case(ext)) {
            let stem = &path[..idx];
            let direct = root.join(dotted_to_path(stem)).with_extension(ext);
            if direct.is_file() {
                return Some(direct);
            }
        }
    }
    let base = root.join(dotted_to_path(path));
    for ext in exts {
        let candidate = base.with_extension(ext);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

fn resolve_bundle(assets: &HashMap<String, String>, path: &str, exts: &[&str]) -> Option<String> {
    if let Some(idx) = path.rfind('.') {
        let ext = &path[idx + 1..];
        if exts.iter().any(|e| e.eq_ignore_ascii_case(ext)) {
            let stem = path[..idx].replace('.', "/");
            let key = format!("{stem}.{ext}");
            if assets.contains_key(&key) {
                return Some(key);
            }
        }
    }
    let base = path.replace('.', "/");
    for ext in exts {
        let key = format!("{base}.{ext}");
        if assets.contains_key(&key) {
            return Some(key);
        }
    }
    None
}

fn dotted_to_path(dotted: &str) -> PathBuf {
    PathBuf::from(dotted.replace('.', std::path::MAIN_SEPARATOR_STR))
}

pub struct ImageAsset {
    pub id: u64,
    pub width: u32,
    pub height: u32,

    pub data: Arc<Vec<u8>>,
    pub source: String,
}

impl UserData for ImageAsset {
    fn add_methods<M: UserDataMethods<Self>>(m: &mut M) {
        m.add_method("Width", |_, this, _: ()| Ok(this.width as i64));
        m.add_method("Height", |_, this, _: ()| Ok(this.height as i64));
        m.add_method("Source", |_, this, _: ()| Ok(this.source.clone()));
        m.add_method("Pixels", |lua, this, _: ()| {
            lua.create_string(this.data.as_slice())
        });
    }
}

pub struct ShaderAsset {
    pub id: u64,
    pub code: String,
    pub source: String,
}

impl TextAsset for ShaderAsset {
    fn make(code: String, source: String) -> Self {
        Self {
            id: next_shader_id(),
            code,
            source,
        }
    }
}

impl UserData for ShaderAsset {}

pub struct FragmentAsset {
    pub id: u64,
    pub code: String,
    pub source: String,
}

impl TextAsset for FragmentAsset {
    fn make(code: String, source: String) -> Self {
        Self {
            id: next_shader_id(),
            code,
            source,
        }
    }
}

impl UserData for FragmentAsset {}

pub struct ModelAsset {
    pub id: u64,
    pub vertices: Arc<Vec<crate::libs::renderable::mesh::Vertex3D>>,
    pub indices: Arc<Vec<u32>>,

    pub animations: Arc<Vec<crate::libs::renderable::mesh::FbxAnimClip>>,
    pub source: String,
}

impl UserData for ModelAsset {
    fn add_methods<M: UserDataMethods<Self>>(m: &mut M) {
        m.add_method("VertexCount", |_, this, _: ()| {
            Ok(this.vertices.len() as i64)
        });
        m.add_method("TriangleCount", |_, this, _: ()| {
            Ok((this.indices.len() / 3) as i64)
        });
        m.add_method("Source", |_, this, _: ()| Ok(this.source.clone()));
    }
}

pub struct FontAsset {
    pub id: u64,
    pub font: Arc<fontdue::Font>,
    pub source: String,
}

impl UserData for FontAsset {
    fn add_methods<M: UserDataMethods<Self>>(m: &mut M) {
        m.add_method("Source", |_, this, _: ()| Ok(this.source.clone()));
    }
}
