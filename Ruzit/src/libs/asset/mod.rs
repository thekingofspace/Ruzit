use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use base64::Engine;
use image::ImageReader;
use mlua::{Lua, Table, UserData, UserDataMethods, Value};

use crate::libs::sfx::{self, SoundData};
use crate::vfs::{self, Fs, split_owner};

/// Monotonic id used by shader-style assets so attached effects can be
/// identified across `:SetData`/`:DetachShader` calls without relying on
/// userdata pointer equality.
static SHADER_ID: AtomicU64 = AtomicU64::new(1);

pub(crate) fn next_shader_id() -> u64 {
    SHADER_ID.fetch_add(1, Ordering::Relaxed)
}

pub fn create(lua: &Lua, fs: Fs, owner: String) -> mlua::Result<Table> {
    let t = lua.create_table()?;
    let fs_clone = fs.clone();
    let owner_clone = owner.clone();
    t.set(
        "GetAsset",
        lua.create_function(
            move |lua, (kind, path): (String, String)| -> mlua::Result<Value> {
                get_asset(lua, &fs_clone, &owner_clone, &kind, &path)
            },
        )?,
    )?;
    Ok(t)
}

const IMAGE_EXTS: &[&str] = &["png", "jpg", "jpeg", "bmp", "gif", "webp"];
const SHADER_EXTS: &[&str] = &["shader", "glsl", "wgsl", "hlsl", "vert", "metal"];
const FRAGMENT_EXTS: &[&str] = &["frag", "fragment", "fs", "glslf"];

fn get_asset(
    lua: &Lua,
    fs: &Fs,
    owner: &str,
    kind: &str,
    path: &str,
) -> mlua::Result<Value> {
    match kind {
        "Image" => load_image(lua, fs, owner, path),
        "Sound" => load_sound(lua, fs, owner, path),
        "Shader" => load_text::<ShaderAsset>(lua, fs, owner, path, SHADER_EXTS, "Shader"),
        "Fragment" => load_text::<FragmentAsset>(lua, fs, owner, path, FRAGMENT_EXTS, "Fragment"),
        other => Err(mlua::Error::RuntimeError(format!(
            "Asset.GetAsset: unknown kind '{other}' (try 'Image', 'Sound', 'Shader', 'Fragment')"
        ))),
    }
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
    let code = String::from_utf8(bytes).map_err(|e| {
        mlua::Error::RuntimeError(format!("Asset.GetAsset: '{source}' not valid UTF-8: {e}"))
    })?;
    Ok(Value::UserData(lua.create_userdata(T::make(code, source))?))
}

trait TextAsset {
    fn make(code: String, source: String) -> Self;
}

fn load_image(lua: &Lua, fs: &Fs, owner: &str, path: &str) -> mlua::Result<Value> {
    let (bytes, source) = read_bytes(fs, owner, path, IMAGE_EXTS, "Image")?;

    let img = ImageReader::new(std::io::Cursor::new(&bytes))
        .with_guessed_format()
        .map_err(|e| mlua::Error::RuntimeError(format!("guess {source}: {e}")))?
        .decode()
        .map_err(|e| mlua::Error::RuntimeError(format!("decode {source}: {e}")))?;

    let rgba = img.to_rgba8();
    let (width, height) = rgba.dimensions();
    let data = rgba.into_raw();

    let asset = ImageAsset {
        width,
        height,
        data,
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
            // Either an explicit "@<id>/..." prefix overrides, or we use the calling
            // file's package as context.
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
            // Lazy decode: assets are kept as base64 strings until access.
            let b64 = pkg.assets.get(&key).ok_or_else(|| {
                mlua::Error::RuntimeError(format!("Asset.GetAsset: '{key}' missing"))
            })?;
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(b64)
                .map_err(|e| {
                    mlua::Error::RuntimeError(format!("Asset.GetAsset: '{key}' base64 decode: {e}"))
                })?;
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

fn resolve_bundle(
    assets: &HashMap<String, String>,
    path: &str,
    exts: &[&str],
) -> Option<String> {
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
    pub width: u32,
    pub height: u32,
    pub data: Vec<u8>,
    pub source: String,
}

impl UserData for ImageAsset {
    fn add_methods<M: UserDataMethods<Self>>(m: &mut M) {
        m.add_method("Width", |_, this, _: ()| Ok(this.width as i64));
        m.add_method("Height", |_, this, _: ()| Ok(this.height as i64));
        m.add_method("Source", |_, this, _: ()| Ok(this.source.clone()));
        m.add_method("Pixels", |lua, this, _: ()| lua.create_string(&this.data));
    }
}

/// Opaque shader handle. The source text is intentionally not exposed to Lua —
/// these are meant to be attached to host objects (sounds, meshes, UI) and
/// parameterized via `:SetData(shader, name, value)`.
pub struct ShaderAsset {
    pub id: u64,
    pub code: String,
    pub source: String,
}

impl TextAsset for ShaderAsset {
    fn make(code: String, source: String) -> Self {
        Self { id: next_shader_id(), code, source }
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
        Self { id: next_shader_id(), code, source }
    }
}

impl UserData for FragmentAsset {}
