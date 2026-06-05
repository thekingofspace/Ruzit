use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use image::ImageReader;
use mlua::{Lua, MultiValue, RegistryKey, Table, UserData, UserDataMethods, Value};

use crate::libs::sfx::{self, SoundData};
use crate::libs::signal;
use crate::vfs::{self, Fs, split_owner};

static SHADER_ID: AtomicU64 = AtomicU64::new(1);

pub(crate) fn next_shader_id() -> u64 {
    SHADER_ID.fetch_add(1, Ordering::Relaxed)
}

const ASSET_CACHE_KEY: &str = "ruzit_asset_cache";
const ASSET_STRONG_KEY: &str = "ruzit_asset_strong";

static NEXT_BATCH_ID: AtomicU64 = AtomicU64::new(1);
static IO_INBOX: Mutex<Vec<(u64, Vec<PendingItem>)>> = Mutex::new(Vec::new());

thread_local! {
    static ACTIVE_BATCHES: RefCell<HashMap<u64, Arc<RegistryKey>>> = RefCell::new(HashMap::new());
}

struct PendingItem {
    user_key: String,
    cache_key: String,
    kind: String,
    bytes_result: Result<(Vec<u8>, String), String>,
}

fn resolve_pkg_alias(alias: &str, caller_pkg_id: &str, default_id: &str) -> String {
    match alias {
        "self" => caller_pkg_id.to_string(),
        "Game" | "game" => default_id.to_string(),
        other => other.to_string(),
    }
}

fn rebase_self_inner(alias: &str, caller_inner: &str, inner: &str) -> String {
    if alias != "self" {
        return inner.to_string();
    }
    let anchor = vfs::init_anchor(caller_inner);
    if anchor.is_empty() {
        inner.to_string()
    } else {
        format!("{anchor}/{inner}")
    }
}

fn global_cache_key(fs: &Fs, owner: &str, kind: &str, path: &str) -> String {
    match fs {
        Fs::Disk { .. } => format!("asset:disk:{kind}:{path}"),
        Fs::Bundle { default_id, .. } => {
            let (target_pkg, rest) = if let Some(rest) = path.strip_prefix('@') {
                if let Some((id, inner)) = rest.split_once('/') {
                    let (caller_pkg, caller_inner) = split_owner(owner, default_id);
                    (
                        resolve_pkg_alias(id, caller_pkg, default_id),
                        rebase_self_inner(id, caller_inner, inner),
                    )
                } else {
                    (default_id.clone(), path.to_string())
                }
            } else {
                let (caller_pkg, _) = split_owner(owner, default_id);
                (caller_pkg.to_string(), path.to_string())
            };
            format!("asset:pkg:{target_pkg}:{kind}:{rest}")
        }
    }
}

fn ensure_asset_cache(lua: &Lua) -> mlua::Result<Table> {
    if let Ok(t) = lua.named_registry_value::<Table>(ASSET_CACHE_KEY) {
        return Ok(t);
    }
    let t = lua.create_table()?;
    let meta = lua.create_table()?;
    meta.set("__mode", "v")?;
    t.set_metatable(Some(meta))?;
    lua.set_named_registry_value(ASSET_CACHE_KEY, t.clone())?;
    Ok(t)
}

fn ensure_strong_cache(lua: &Lua) -> mlua::Result<Table> {
    if let Ok(t) = lua.named_registry_value::<Table>(ASSET_STRONG_KEY) {
        return Ok(t);
    }
    let t = lua.create_table()?;
    lua.set_named_registry_value(ASSET_STRONG_KEY, t.clone())?;
    Ok(t)
}

fn parse_entry(entry: &str) -> mlua::Result<(String, String)> {
    let mut parts = entry.splitn(2, ':');
    let kind = parts.next().unwrap_or("").to_string();
    let path = parts
        .next()
        .ok_or_else(|| {
            mlua::Error::RuntimeError(format!(
                "Asset preload: '{entry}' must be 'Kind:path' (e.g. 'Image:ui.logo')"
            ))
        })?
        .to_string();
    if kind.is_empty() || path.is_empty() {
        return Err(mlua::Error::RuntimeError(format!(
            "Asset preload: '{entry}' has empty kind or path"
        )));
    }
    Ok((kind, path))
}

fn extract_asset_id(value: &Value, kind: &str) -> Option<u64> {
    let ud = match value {
        Value::UserData(u) => u,
        _ => return None,
    };
    match kind {
        "Image" => ud.borrow::<ImageAsset>().ok().map(|a| a.id),
        "Sound" => ud.borrow::<SoundData>().ok().map(|a| a.id),
        "Shader" => ud.borrow::<ShaderAsset>().ok().map(|a| a.id),
        "Fragment" => ud.borrow::<FragmentAsset>().ok().map(|a| a.id),
        "Model" => ud.borrow::<ModelAsset>().ok().map(|a| a.id),
        "Font" => ud.borrow::<FontAsset>().ok().map(|a| a.id),
        _ => None,
    }
}

pub fn free_asset_by_id(lua: &Lua, kind: &str, id: u64) {
    purge_asset_uses(lua, kind, id);
    if let Ok(cache) = ensure_asset_cache(lua) {
        let to_remove: Vec<String> = cache
            .pairs::<String, Value>()
            .filter_map(|p| p.ok())
            .filter(|(_, v)| extract_asset_id(v, kind) == Some(id))
            .map(|(k, _)| k)
            .collect();
        for k in to_remove {
            let _ = cache.set(k, Value::Nil);
        }
    }
    if let Ok(strong) = ensure_strong_cache(lua) {
        let to_remove: Vec<String> = strong
            .pairs::<String, Value>()
            .filter_map(|p| p.ok())
            .filter(|(_, v)| extract_asset_id(v, kind) == Some(id))
            .map(|(k, _)| k)
            .collect();
        for k in to_remove {
            let _ = strong.set(k, Value::Nil);
        }
    }
}

fn purge_asset_uses(lua: &Lua, kind: &str, asset_id: u64) {
    match kind {
        "Image" => {
            crate::libs::renderable::purge_texture(lua, asset_id);
            crate::libs::gui::purge_image(lua, asset_id);
        }
        "Sound" => sfx::purge_sound_data(asset_id),
        "Model" => crate::libs::renderable::purge_model(lua, asset_id),
        "Font" => crate::libs::gui::purge_font(lua, asset_id),
        "Shader" | "Fragment" => {
            crate::libs::renderable::purge_shader(lua, asset_id);
            crate::libs::gui::purge_shader(lua, asset_id);
        }
        _ => {}
    }
}

fn read_for_kind(
    fs: &Fs,
    owner: &str,
    kind: &str,
    path: &str,
) -> Result<(Vec<u8>, String), String> {
    let exts: &[&str] = match kind {
        "Image" => IMAGE_EXTS,
        "Sound" => sfx::SOUND_EXTS,
        "Shader" => SHADER_EXTS,
        "Fragment" => FRAGMENT_EXTS,
        "Model" => MODEL_EXTS,
        "AnimationSet" => MODEL_EXTS,
        "Font" => FONT_EXTS,
        "File" => {
            return read_file_bytes(fs, owner, path)
                .map(|b| (b, path.to_string()))
                .map_err(|e| e.to_string());
        }
        other => {
            return Err(format!(
                "unknown kind '{other}' (try 'Image', 'Sound', 'Shader', 'Fragment', 'Model', 'AnimationSet', 'Font', 'File')"
            ));
        }
    };
    read_bytes(fs, owner, path, exts, kind).map_err(|e| e.to_string())
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
                let cache_key = global_cache_key(&fs_clone, &owner_clone, &kind, &path);
                if let Some(hit) = cache_get(lua, &cache_key) {
                    return Ok(hit);
                }
                let value = get_asset(lua, &fs_clone, &owner_clone, &kind, &path)?;
                cache_set(lua, &cache_key, &value);
                Ok(value)
            },
        )?,
    )?;
    let fs_frag = fs.clone();
    let owner_frag = owner.clone();
    t.set(
        "LoadFragmented",
        lua.create_function(move |lua, path: String| -> mlua::Result<Table> {
            load_fragmented(lua, &fs_frag, &owner_frag, &path)
        })?,
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
    let fs_preload = fs.clone();
    let owner_preload = owner.clone();
    t.set(
        "PreloadAsync",
        lua.create_function(move |lua, entries: Vec<String>| -> mlua::Result<Table> {
            let parsed: Vec<(String, String, String)> = entries
                .iter()
                .map(|e| {
                    let (k, p) = parse_entry(e)?;
                    Ok((e.clone(), k, p))
                })
                .collect::<mlua::Result<Vec<_>>>()?;

            let signal = signal::new_instance(lua)?;
            if parsed.is_empty() {
                signal::fire(lua, &signal, MultiValue::new())?;
                return Ok(signal);
            }

            let signal_key = Arc::new(lua.create_registry_value(signal.clone())?);
            let batch_id = NEXT_BATCH_ID.fetch_add(1, Ordering::Relaxed);
            ACTIVE_BATCHES.with(|m| {
                m.borrow_mut().insert(batch_id, signal_key);
            });

            let fs_thread = fs_preload.clone();
            let owner_thread = owner_preload.clone();
            let resolved: Vec<(String, String, String, String)> = parsed
                .into_iter()
                .map(|(user_key, kind, path)| {
                    let cache_key = global_cache_key(&fs_thread, &owner_thread, &kind, &path);
                    (user_key, cache_key, kind, path)
                })
                .collect();
            std::thread::spawn(move || {
                let mut items = Vec::with_capacity(resolved.len());
                for (user_key, cache_key, kind, path) in resolved {
                    let bytes_result = read_for_kind(&fs_thread, &owner_thread, &kind, &path);
                    items.push(PendingItem {
                        user_key,
                        cache_key,
                        kind,
                        bytes_result,
                    });
                }
                IO_INBOX.lock().unwrap().push((batch_id, items));
            });
            Ok(signal)
        })?,
    )?;

    let fs_bulk = fs.clone();
    let owner_bulk = owner.clone();
    t.set(
        "BulkLoad",
        lua.create_function(move |lua, entries: Vec<String>| -> mlua::Result<Table> {
            let result = lua.create_table()?;
            let strong = ensure_strong_cache(lua)?;
            for entry in entries {
                let (kind, path) = parse_entry(&entry)?;
                let cache_key = global_cache_key(&fs_bulk, &owner_bulk, &kind, &path);
                let value = if let Some(hit) = cache_get(lua, &cache_key) {
                    hit
                } else {
                    let v = get_asset(lua, &fs_bulk, &owner_bulk, &kind, &path)?;
                    cache_set(lua, &cache_key, &v);
                    v
                };
                strong.set(cache_key, value.clone())?;
                result.set(entry, value)?;
            }
            Ok(result)
        })?,
    )?;

    let fs_drop = fs.clone();
    let owner_drop = owner.clone();
    t.set(
        "Drop",
        lua.create_function(move |lua, entry: String| -> mlua::Result<()> {
            let (kind, path) = parse_entry(&entry)?;
            let cache_key = global_cache_key(&fs_drop, &owner_drop, &kind, &path);

            let cached = cache_get(lua, &cache_key).or_else(|| {
                ensure_strong_cache(lua)
                    .ok()
                    .and_then(|t| t.get::<Value>(cache_key.clone()).ok())
                    .filter(|v| !matches!(v, Value::Nil))
            });
            if let Some(v) = cached {
                if let Some(id) = extract_asset_id(&v, &kind) {
                    purge_asset_uses(lua, &kind, id);
                }
            }

            if let Ok(cache) = ensure_asset_cache(lua) {
                cache.set(cache_key.clone(), Value::Nil)?;
            }
            if let Ok(strong) = ensure_strong_cache(lua) {
                strong.set(cache_key, Value::Nil)?;
            }
            Ok(())
        })?,
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
                    data: Mutex::new(Some(Arc::new(bytes.to_vec()))),
                    source: format!("<pixels:{width}x{height}>"),
                };
                Ok(Value::UserData(lua.create_userdata(asset)?))
            },
        )?,
    )?;

    t.set(
        "GetVideo",
        crate::libs::video::create_get_video_function(lua, fs.clone(), owner.clone())?,
    )?;
    Ok(t)
}

pub fn from_bytes(lua: &Lua, kind: &str, bytes: Vec<u8>, source: String) -> mlua::Result<Value> {
    match kind {
        "Image" => parse_image(lua, bytes, source),
        "Sound" => Ok(Value::UserData(lua.create_userdata(SoundData {
            id: next_shader_id(),
            bytes: Mutex::new(Some(Arc::new(bytes))),
            source,
        })?)),
        "Shader" => parse_text::<ShaderAsset>(lua, bytes, source),
        "Fragment" => parse_text::<FragmentAsset>(lua, bytes, source),
        "Model" => parse_model(lua, bytes, source),
        "AnimationSet" => parse_animation_set(lua, bytes, source),
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

pub fn read_video_bytes(fs: &Fs, owner: &str, path: &str) -> mlua::Result<(Vec<u8>, String)> {
    read_bytes(fs, owner, path, crate::libs::video::VIDEO_EXTS, "Video")
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
        data: Mutex::new(Some(Arc::new(rgba))),
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
        "AnimationSet" => load_animation_set(lua, fs, owner, path),
        "Font" => load_font(lua, fs, owner, path),
        "File" => load_file(lua, fs, owner, path),
        other => Err(mlua::Error::RuntimeError(format!(
            "Asset.GetAsset: unknown kind '{other}' (try 'Image', 'Sound', 'Shader', 'Fragment', 'Model', 'AnimationSet', 'Font', 'File')"
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
                    let (caller_pkg, caller_inner) = split_owner(owner, default_id);
                    (
                        resolve_pkg_alias(id, caller_pkg, default_id),
                        rebase_self_inner(id, caller_inner, inner),
                    )
                } else {
                    return Err(mlua::Error::RuntimeError(format!(
                        "Asset.GetAsset: bad package path '{path}'"
                    )));
                }
            } else {
                let (caller_pkg, _) = split_owner(owner, default_id);
                (caller_pkg.to_string(), path.to_string())
            };
            let pkg = packages.get_activated(&target_id).ok_or_else(|| {
                mlua::Error::RuntimeError(format!(
                    "Asset.GetAsset: package '{target_id}' is not loaded (call manifest.LoadManaged first)"
                ))
            })?;
            let key = rest_path.replace('\\', "/");
            pkg.read_asset(&key)
                .map_err(|e| mlua::Error::RuntimeError(format!("Asset.GetAsset: {e}")))
        }
    }
}

fn load_model(lua: &Lua, fs: &Fs, owner: &str, path: &str) -> mlua::Result<Value> {
    let (bytes, source) = read_bytes(fs, owner, path, MODEL_EXTS, "Model")?;
    parse_model(lua, bytes, source)
}

/// Load a model and split it into one ModelAsset per mesh. Returns a table
/// keyed by mesh name; meshes that share a name are disambiguated with a
/// ` (n)` suffix. Only FBX carries multiple named meshes; OBJ yields a single
/// entry keyed by the file stem.
fn load_fragmented(lua: &Lua, fs: &Fs, owner: &str, path: &str) -> mlua::Result<Table> {
    use crate::libs::renderable::mesh::{self, MeshFragment};

    let (bytes, source) = read_bytes(fs, owner, path, MODEL_EXTS, "Model")?;
    let is_fbx =
        bytes.starts_with(b"Kaydara FBX Binary") || source.to_ascii_lowercase().ends_with(".fbx");

    let fragments: Vec<MeshFragment> = if is_fbx {
        mesh::load_fbx_fragments(&bytes).map_err(|e| {
            mlua::Error::RuntimeError(format!("Asset.LoadFragmented '{source}': {e}"))
        })?
    } else {
        let text = String::from_utf8(bytes).map_err(|e| {
            mlua::Error::RuntimeError(format!("Asset.LoadFragmented '{source}' not UTF-8: {e}"))
        })?;
        let mesh = mesh::load_obj(&text).map_err(|e| {
            mlua::Error::RuntimeError(format!("Asset.LoadFragmented '{source}': {e}"))
        })?;
        let name = Path::new(&source)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("Mesh")
            .to_string();
        vec![MeshFragment {
            name,
            mesh,
            origin: [0.0, 0.0, 0.0],
            pivot: [0.0, 0.0, 0.0],
            rotation: [0.0, 0.0, 0.0],
        }]
    };

    let result = lua.create_table()?;
    let mut counts: HashMap<String, usize> = HashMap::new();
    for frag in fragments {
        let base = frag.name;
        let n = counts.entry(base.clone()).or_insert(0);
        let mut key = if *n == 0 {
            base.clone()
        } else {
            format!("{base} ({n})")
        };
        *n += 1;
        // Guard against colliding with an already-used key (e.g. a node literally
        // named "Wheel (1)").
        while result.contains_key(key.clone())? {
            key = format!("{base} ({n})");
            *n += 1;
        }

        let asset = ModelAsset {
            id: next_shader_id(),
            vertices: Arc::new(frag.mesh.vertices),
            indices: Arc::new(frag.mesh.indices),
            source: format!("{source}#{key}"),
            origin: frag.origin,
            pivot: frag.pivot,
            rotation: frag.rotation,
        };
        result.set(key, Value::UserData(lua.create_userdata(asset)?))?;
    }
    Ok(result)
}

fn parse_model(lua: &Lua, bytes: Vec<u8>, source: String) -> mlua::Result<Value> {
    let is_fbx =
        bytes.starts_with(b"Kaydara FBX Binary") || source.to_ascii_lowercase().ends_with(".fbx");

    let (mesh, origin, pivot, rotation) = if is_fbx {
        let loaded = crate::libs::renderable::mesh::load_fbx_full(&bytes)
            .map_err(|e| mlua::Error::RuntimeError(format!("Model parse '{source}': {e}")))?;
        (loaded.mesh, loaded.origin, loaded.pivot, loaded.rotation)
    } else {
        let text = String::from_utf8(bytes)
            .map_err(|e| mlua::Error::RuntimeError(format!("Model '{source}' not UTF-8: {e}")))?;
        let mesh = crate::libs::renderable::mesh::load_obj(&text)
            .map_err(|e| mlua::Error::RuntimeError(format!("Model parse '{source}': {e}")))?;
        (mesh, [0.0, 0.0, 0.0], [0.0, 0.0, 0.0], [0.0, 0.0, 0.0])
    };
    let asset = ModelAsset {
        id: next_shader_id(),
        vertices: Arc::new(mesh.vertices),
        indices: Arc::new(mesh.indices),
        source,
        origin,
        pivot,
        rotation,
    };
    Ok(Value::UserData(lua.create_userdata(asset)?))
}

fn load_animation_set(lua: &Lua, fs: &Fs, owner: &str, path: &str) -> mlua::Result<Value> {
    let (bytes, source) = read_bytes(fs, owner, path, MODEL_EXTS, "AnimationSet")?;
    parse_animation_set(lua, bytes, source)
}

fn parse_animation_set(lua: &Lua, bytes: Vec<u8>, source: String) -> mlua::Result<Value> {
    let is_fbx =
        bytes.starts_with(b"Kaydara FBX Binary") || source.to_ascii_lowercase().ends_with(".fbx");
    if !is_fbx {
        return Err(mlua::Error::RuntimeError(format!(
            "AnimationSet '{source}': only FBX files carry animation data (got non-FBX bytes)"
        )));
    }
    let loaded = crate::libs::renderable::mesh::load_fbx_full(&bytes)
        .map_err(|e| mlua::Error::RuntimeError(format!("AnimationSet parse '{source}': {e}")))?;
    let asset = AnimationSetAsset {
        id: next_shader_id(),
        animations: Arc::new(loaded.animations),
        source,
    };
    Ok(Value::UserData(lua.create_userdata(asset)?))
}

fn load_sound(lua: &Lua, fs: &Fs, owner: &str, path: &str) -> mlua::Result<Value> {
    let (bytes, source) = read_bytes(fs, owner, path, sfx::SOUND_EXTS, "Sound")?;
    let data = SoundData {
        id: next_shader_id(),
        bytes: Mutex::new(Some(Arc::new(bytes))),
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
        data: Mutex::new(Some(Arc::new(data))),
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
                    let (caller_pkg, caller_inner) = split_owner(owner, default_id);
                    (
                        resolve_pkg_alias(id, caller_pkg, default_id),
                        rebase_self_inner(id, caller_inner, inner),
                    )
                } else {
                    return Err(mlua::Error::RuntimeError(format!(
                        "Asset.GetAsset: bad package path '{path}'"
                    )));
                }
            } else {
                let (caller_pkg, _) = split_owner(owner, default_id);
                (caller_pkg.to_string(), path.to_string())
            };

            let pkg = packages.get_activated(&target_id).ok_or_else(|| {
                mlua::Error::RuntimeError(format!(
                    "Asset.GetAsset: package '{target_id}' is not loaded (call manifest.LoadManaged first)"
                ))
            })?;
            let key = resolve_bundle_from_pkg(&pkg, &rest_path, exts).ok_or_else(|| {
                mlua::Error::RuntimeError(format!(
                    "Asset.GetAsset: {kind} '{rest_path}' not found in package '{target_id}'"
                ))
            })?;
            let bytes = pkg
                .read_asset(&key)
                .map_err(|e| mlua::Error::RuntimeError(format!("Asset.GetAsset: {e}")))?;
            Ok((bytes, format!("@{target_id}/{key}")))
        }
    }
}

fn resolve_bundle_from_pkg(pkg: &crate::vfs::Package, path: &str, exts: &[&str]) -> Option<String> {
    if let Some(idx) = path.rfind('.') {
        let ext = &path[idx + 1..];
        if exts.iter().any(|e| e.eq_ignore_ascii_case(ext)) {
            let stem = path[..idx].replace('.', "/");
            let key = format!("{stem}.{ext}");
            if pkg.has_asset(&key) {
                return Some(key);
            }
        }
    }
    let base = path.replace('.', "/");
    for ext in exts {
        let key = format!("{base}.{ext}");
        if pkg.has_asset(&key) {
            return Some(key);
        }
    }
    None
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

fn dotted_to_path(dotted: &str) -> PathBuf {
    PathBuf::from(dotted.replace('.', std::path::MAIN_SEPARATOR_STR))
}

pub struct ImageAsset {
    pub id: u64,
    pub width: u32,
    pub height: u32,

    pub data: Mutex<Option<Arc<Vec<u8>>>>,
    pub source: String,
}

impl ImageAsset {
    pub fn data(&self) -> Option<Arc<Vec<u8>>> {
        self.data.lock().ok().and_then(|g| g.clone())
    }

    pub fn data_or_err(&self, ctx: &str) -> mlua::Result<Arc<Vec<u8>>> {
        self.data().ok_or_else(|| {
            mlua::Error::RuntimeError(format!("{ctx}: image '{}' has been freed", self.source))
        })
    }

    pub fn release(&self) {
        if let Ok(mut g) = self.data.lock() {
            *g = None;
        }
    }
}

impl UserData for ImageAsset {
    fn add_methods<M: UserDataMethods<Self>>(m: &mut M) {
        m.add_method("Width", |_, this, _: ()| Ok(this.width as i64));
        m.add_method("Height", |_, this, _: ()| Ok(this.height as i64));
        m.add_method("Source", |_, this, _: ()| Ok(this.source.clone()));
        m.add_method("Pixels", |lua, this, _: ()| -> mlua::Result<mlua::String> {
            let d = this.data_or_err("ImageAsset:Pixels")?;
            lua.create_string(d.as_slice())
        });
        m.add_method("Free", |lua, this, _: ()| {
            this.release();
            free_asset_by_id(lua, "Image", this.id);
            Ok(())
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

impl UserData for ShaderAsset {
    fn add_methods<M: UserDataMethods<Self>>(m: &mut M) {
        m.add_method("Source", |_, this, _: ()| Ok(this.source.clone()));
        m.add_method("Free", |lua, this, _: ()| {
            free_asset_by_id(lua, "Shader", this.id);
            Ok(())
        });
    }
}

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

impl UserData for FragmentAsset {
    fn add_methods<M: UserDataMethods<Self>>(m: &mut M) {
        m.add_method("Source", |_, this, _: ()| Ok(this.source.clone()));
        m.add_method("Free", |lua, this, _: ()| {
            free_asset_by_id(lua, "Fragment", this.id);
            Ok(())
        });
    }
}

pub struct ModelAsset {
    pub id: u64,
    pub vertices: Arc<Vec<crate::libs::renderable::mesh::Vertex3D>>,
    pub indices: Arc<Vec<u32>>,
    pub source: String,
    pub origin: [f32; 3],
    pub pivot: [f32; 3],
    pub rotation: [f32; 3],
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
        m.add_method("Origin", |_, this, _: ()| {
            Ok(crate::libs::primitives::Vector::new(
                this.origin[0],
                this.origin[1],
                this.origin[2],
            ))
        });
        m.add_method("Pivot", |_, this, _: ()| {
            Ok(crate::libs::primitives::Vector::new(
                this.pivot[0],
                this.pivot[1],
                this.pivot[2],
            ))
        });
        m.add_method("Rotation", |_, this, _: ()| {
            Ok(crate::libs::primitives::Vector::new(
                this.rotation[0],
                this.rotation[1],
                this.rotation[2],
            ))
        });
        // Model node's local transform as a CFrame:
        //   position = Lcl Translation (parent-relative origin)
        //   rotation = Lcl Rotation    (radians, XYZ Euler)
        // Pair this with the geometry-local vertices to place the part where
        // the source DCC put it, without re-baking the transform yourself.
        m.add_method("GetPivot", |_, this, _: ()| {
            let position = crate::libs::primitives::Vector::new(
                this.origin[0],
                this.origin[1],
                this.origin[2],
            );
            let rotation = crate::libs::primitives::Vector::new(
                this.rotation[0],
                this.rotation[1],
                this.rotation[2],
            );
            Ok(crate::libs::primitives::CFrame::new(position, rotation))
        });
        m.add_method("LocalBounds", |lua, this, _: ()| -> mlua::Result<Table> {
            if this.vertices.is_empty() {
                let t = lua.create_table()?;
                let zero = crate::libs::primitives::Vector::new(0.0, 0.0, 0.0);
                t.set("Min", zero)?;
                t.set("Max", zero)?;
                t.set("Size", zero)?;
                t.set("Center", zero)?;
                return Ok(t);
            }
            let mut min = [f32::INFINITY; 3];
            let mut max = [f32::NEG_INFINITY; 3];
            for v in this.vertices.iter() {
                for i in 0..3 {
                    if v.position[i] < min[i] {
                        min[i] = v.position[i];
                    }
                    if v.position[i] > max[i] {
                        max[i] = v.position[i];
                    }
                }
            }
            let to_vec = |a: [f32; 3]| crate::libs::primitives::Vector::new(a[0], a[1], a[2]);
            let size = [max[0] - min[0], max[1] - min[1], max[2] - min[2]];
            let center = [
                (min[0] + max[0]) * 0.5,
                (min[1] + max[1]) * 0.5,
                (min[2] + max[2]) * 0.5,
            ];
            let t = lua.create_table()?;
            t.set("Min", to_vec(min))?;
            t.set("Max", to_vec(max))?;
            t.set("Size", to_vec(size))?;
            t.set("Center", to_vec(center))?;
            Ok(t)
        });
        m.add_method("Free", |lua, this, _: ()| {
            free_asset_by_id(lua, "Model", this.id);
            Ok(())
        });
    }
}

pub struct AnimationSetAsset {
    pub id: u64,
    pub animations: Arc<Vec<crate::libs::renderable::mesh::FbxAnimClip>>,
    pub source: String,
}

impl UserData for AnimationSetAsset {
    fn add_methods<M: UserDataMethods<Self>>(m: &mut M) {
        m.add_method("Source", |_, this, _: ()| Ok(this.source.clone()));
        m.add_method("Count", |_, this, _: ()| Ok(this.animations.len() as i64));
        m.add_method("ListAnimations", |lua, this, _: ()| -> mlua::Result<Table> {
            let t = lua.create_table()?;
            for (i, clip) in this.animations.iter().enumerate() {
                t.set(i + 1, clip.name.clone())?;
            }
            Ok(t)
        });
        m.add_method(
            "GetAnimation",
            |lua, this, name: String| -> mlua::Result<Value> {
                let clip = this
                    .animations
                    .iter()
                    .find(|c| c.name == name)
                    .cloned()
                    .ok_or_else(|| {
                        mlua::Error::RuntimeError(format!(
                            "AnimationSet:GetAnimation: no animation named '{name}' in '{}'",
                            this.source
                        ))
                    })?;
                Ok(Value::UserData(lua.create_userdata(AnimationAsset {
                    clip: Arc::new(clip),
                    source: this.source.clone(),
                })?))
            },
        );
        m.add_method("Free", |lua, this, _: ()| {
            free_asset_by_id(lua, "AnimationSet", this.id);
            Ok(())
        });
    }
}

#[derive(Clone)]
pub struct AnimationAsset {
    pub clip: Arc<crate::libs::renderable::mesh::FbxAnimClip>,
    pub source: String,
}

impl UserData for AnimationAsset {
    fn add_methods<M: UserDataMethods<Self>>(m: &mut M) {
        m.add_method("Name", |_, this, _: ()| Ok(this.clip.name.clone()));
        m.add_method("Duration", |_, this, _: ()| Ok(this.clip.duration));
        m.add_method("Source", |_, this, _: ()| Ok(this.source.clone()));
        m.add_method("KeyframeCount", |_, this, _: ()| {
            Ok(this.clip.samples.len() as i64)
        });
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
        m.add_method("Free", |lua, this, _: ()| {
            free_asset_by_id(lua, "Font", this.id);
            Ok(())
        });
    }
}

pub fn pump(lua: &Lua) {
    let drained: Vec<(u64, Vec<PendingItem>)> = {
        let mut inbox = IO_INBOX.lock().unwrap();
        std::mem::take(&mut *inbox)
    };
    for (batch_id, items) in drained {
        let strong = match ensure_strong_cache(lua) {
            Ok(t) => t,
            Err(e) => {
                eprintln!("[Asset] preload strong cache: {e}");
                continue;
            }
        };
        for item in items {
            match item.bytes_result {
                Ok((bytes, source)) => match from_bytes(lua, &item.kind, bytes, source) {
                    Ok(value) => {
                        cache_set(lua, &item.cache_key, &value);
                        let _ = strong.set(item.cache_key, value);
                    }
                    Err(e) => {
                        eprintln!("[Asset] PreloadAsync parse '{}': {e}", item.user_key);
                    }
                },
                Err(msg) => {
                    eprintln!("[Asset] PreloadAsync read '{}': {msg}", item.user_key);
                }
            }
        }
        let signal_key = ACTIVE_BATCHES.with(|m| m.borrow_mut().remove(&batch_id));
        if let Some(key) = signal_key {
            match lua.registry_value::<Table>(&key) {
                Ok(signal) => {
                    if let Err(e) = signal::fire(lua, &signal, MultiValue::new()) {
                        eprintln!("[Asset] PreloadAsync signal fire: {e}");
                    }
                }
                Err(e) => eprintln!("[Asset] PreloadAsync signal lookup: {e}"),
            }
        }
    }
}
