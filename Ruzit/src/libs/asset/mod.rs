use std::collections::HashMap;
use std::path::{Path, PathBuf};

use image::ImageReader;
use mlua::{Lua, Table, UserData, UserDataMethods, Value};

use crate::vfs::{self, Fs};

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

fn get_asset(
    lua: &Lua,
    fs: &Fs,
    _owner: &str,
    kind: &str,
    path: &str,
) -> mlua::Result<Value> {
    match kind {
        "Image" => load_image(lua, fs, path),
        other => Err(mlua::Error::RuntimeError(format!(
            "Asset.GetAsset: unknown kind '{other}' (try 'Image')"
        ))),
    }
}

fn load_image(lua: &Lua, fs: &Fs, path: &str) -> mlua::Result<Value> {
    let (bytes, source) = read_image_bytes(fs, path)?;

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

fn read_image_bytes(fs: &Fs, path: &str) -> mlua::Result<(Vec<u8>, String)> {
    match fs {
        Fs::Disk { .. } => {
            let root = vfs::fs_root(fs);
            let resolved = resolve_disk(&root, path).ok_or_else(|| {
                mlua::Error::RuntimeError(format!(
                    "Asset.GetAsset: image '{path}' not found under {}",
                    root.display()
                ))
            })?;
            let bytes = std::fs::read(&resolved).map_err(|e| {
                mlua::Error::RuntimeError(format!("read {}: {e}", resolved.display()))
            })?;
            Ok((bytes, resolved.to_string_lossy().into_owned()))
        }
        Fs::Bundle { assets, .. } => {
            let key = resolve_bundle(assets, path).ok_or_else(|| {
                mlua::Error::RuntimeError(format!(
                    "Asset.GetAsset: image '{path}' not found in bundle"
                ))
            })?;
            let bytes = assets.get(&key).cloned().ok_or_else(|| {
                mlua::Error::RuntimeError(format!("Asset.GetAsset: '{key}' missing"))
            })?;
            Ok((bytes, format!("<bundle>/{key}")))
        }
    }
}

fn resolve_disk(root: &Path, path: &str) -> Option<PathBuf> {
    if let Some(idx) = path.rfind('.') {
        let ext = &path[idx + 1..];
        if IMAGE_EXTS.iter().any(|e| e.eq_ignore_ascii_case(ext)) {
            let stem = &path[..idx];
            let direct = root.join(dotted_to_path(stem)).with_extension(ext);
            if direct.is_file() {
                return Some(direct);
            }
        }
    }
    let base = root.join(dotted_to_path(path));
    for ext in IMAGE_EXTS {
        let candidate = base.with_extension(ext);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

fn resolve_bundle(assets: &HashMap<String, Vec<u8>>, path: &str) -> Option<String> {
    if let Some(idx) = path.rfind('.') {
        let ext = &path[idx + 1..];
        if IMAGE_EXTS.iter().any(|e| e.eq_ignore_ascii_case(ext)) {
            let stem = &path[..idx].replace('.', "/");
            let key = format!("{stem}.{ext}");
            if assets.contains_key(&key) {
                return Some(key);
            }
        }
    }
    let base = path.replace('.', "/");
    for ext in IMAGE_EXTS {
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
    pub data: Vec<u8>, // RGBA8
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
