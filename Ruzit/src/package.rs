

use std::collections::HashMap;
use std::env;
use std::fs;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use base64::{Engine, engine::general_purpose::STANDARD as B64};
use serde_json::{Value as JsonValue, json};

use crate::config::FileType;

pub const MAGIC: &[u8; 8] = b"RUZITPKG";


pub struct LauncherInfo {
    pub default_id: String,
    pub name: String,
    pub version: String,
    pub creator: String,
    pub steam_app_id: Option<u32>,
}


pub struct LoadedPackage {
    pub id: String,
    pub name: String,
    pub version: String,
    pub creator: String,
    pub entry: String,
    pub file_type: FileType,
    pub physical_root: Option<std::path::PathBuf>,
    pub files: HashMap<String, String>,
    pub assets: HashMap<String, String>,
    /// When true, every value in `files` and `assets` is base64(zstd(raw)).
    /// Decompressed lazily on access — vfs::read_module for scripts,
    /// Asset.GetAsset for assets.
    pub compressed: bool,
}


pub fn collect_project(
    root: &Path,
) -> Result<(HashMap<String, String>, HashMap<String, Vec<u8>>), String> {
    let mut sources: HashMap<String, String> = HashMap::new();
    let mut assets: HashMap<String, Vec<u8>> = HashMap::new();
    walk(root, root, &mut sources, &mut assets)?;
    Ok((sources, assets))
}

fn walk(
    root: &Path,
    dir: &Path,
    sources: &mut HashMap<String, String>,
    assets: &mut HashMap<String, Vec<u8>>,
) -> Result<(), String> {
    for entry in fs::read_dir(dir).map_err(|e| format!("read_dir {}: {e}", dir.display()))? {
        let entry = entry.map_err(|e| e.to_string())?;
        let path = entry.path();
        let rel = path
            .strip_prefix(root)
            .map_err(|e| e.to_string())?
            .to_string_lossy()
            .replace('\\', "/");
        if should_skip(&rel) {
            continue;
        }
        if path.is_dir() {
            
            
            if path.join("ManagedInfo.toml").is_file() {
                continue;
            }
            walk(root, &path, sources, assets)?;
            continue;
        }
        if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
            if ext.eq_ignore_ascii_case("luau") || ext.eq_ignore_ascii_case("lua") {
                let text = fs::read_to_string(&path)
                    .map_err(|e| format!("read {}: {e}", path.display()))?;
                sources.insert(rel, strip_bom(text));
                continue;
            }
        }
        if rel.starts_with("assets/") {
            let bytes = fs::read(&path).map_err(|e| format!("read {}: {e}", path.display()))?;
            assets.insert(rel, bytes);
        }
    }
    Ok(())
}


pub fn strip_bom(s: String) -> String {
    const BOM: &str = "\u{feff}";
    if s.starts_with(BOM) {
        s[BOM.len()..].to_string()
    } else {
        s
    }
}


pub fn find_dlc_folders(root: &Path) -> Result<Vec<PathBuf>, String> {
    let mut out = Vec::new();
    walk_for_dlcs(root, root, &mut out)?;
    Ok(out)
}

fn walk_for_dlcs(root: &Path, dir: &Path, out: &mut Vec<PathBuf>) -> Result<(), String> {
    for entry in fs::read_dir(dir).map_err(|e| format!("read_dir {}: {e}", dir.display()))? {
        let entry = entry.map_err(|e| e.to_string())?;
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let rel = path
            .strip_prefix(root)
            .map_err(|e| e.to_string())?
            .to_string_lossy()
            .replace('\\', "/");
        if rel.starts_with('.')
            || rel.starts_with(".vscode/")
            || rel.starts_with(".git/")
            || rel.starts_with("Generated/")
            || rel.starts_with("target/")
        {
            continue;
        }
        if path.join("ManagedInfo.toml").is_file() {
            out.push(path);
            continue; 
        }
        walk_for_dlcs(root, &path, out)?;
    }
    Ok(())
}

fn should_skip(rel: &str) -> bool {
    if rel.starts_with(".vscode/")
        || rel.starts_with(".git/")
        || rel.starts_with("target/")
        || rel.starts_with("Generated/")
    {
        return true;
    }
    if rel == "build.toml" || rel == "ManagedInfo.toml" || rel == ".DS_Store" {
        return true;
    }
    let lower = rel.to_lowercase();
    // Declaration files exist purely for the LSP — they aren't real modules
    // and shouldn't be packed into scripts.managed. Catches `types.d.luau`
    // at any depth plus any other `*.d.luau` / `*.d.lua` the user adds.
    if lower.ends_with(".d.luau") || lower.ends_with(".d.lua") {
        return true;
    }
    if lower.ends_with(".exe") || lower.ends_with(".managed") {
        return true;
    }
    false
}


pub fn write_scripts_managed(
    path: &Path,
    id: &str,
    name: &str,
    version: &str,
    creator: &str,
    entry: &str,
    file_type: FileType,
    files: &HashMap<String, String>,
    compress: bool,
) -> Result<(), String> {
    let mut json_files = serde_json::Map::new();
    for (k, v) in files {
        let value = if compress {
            let compressed = zstd::stream::encode_all(v.as_bytes(), 3)
                .map_err(|e| format!("compress {k}: {e}"))?;
            B64.encode(&compressed)
        } else {
            v.clone()
        };
        json_files.insert(k.clone(), JsonValue::String(value));
    }
    let body = json!({
        "kind": "scripts",
        "id": id,
        "name": name,
        "version": version,
        "creator": creator,
        "entry": entry,
        "file_type": file_type.as_str(),
        "compressed": compress,
        "files": json_files,
    });
    let plain = serde_json::to_vec(&body).map_err(|e| e.to_string())?;
    let encrypted = crate::managed::encrypt_payload(&plain)?;
    fs::write(path, encrypted).map_err(|e| format!("write {}: {e}", path.display()))
}

/// Single-file write of every asset. Use `write_assets_sharded` to split
/// across multiple files for diff-friendly updates.
pub fn write_assets_managed(
    path: &Path,
    id: &str,
    name: &str,
    assets: &HashMap<String, Vec<u8>>,
    compress: bool,
) -> Result<(), String> {
    let mut json_assets = serde_json::Map::new();
    for (k, bytes) in assets {
        let payload: Vec<u8> = if compress {
            zstd::stream::encode_all(bytes.as_slice(), 3)
                .map_err(|e| format!("compress {k}: {e}"))?
        } else {
            bytes.clone()
        };
        json_assets.insert(k.clone(), JsonValue::String(B64.encode(&payload)));
    }
    let body = json!({
        "kind": "assets",
        "id": id,
        "name": name,
        "compressed": compress,
        "assets": json_assets,
    });
    let plain = serde_json::to_vec(&body).map_err(|e| e.to_string())?;
    let encrypted = crate::managed::encrypt_payload(&plain)?;
    fs::write(path, encrypted).map_err(|e| format!("write {}: {e}", path.display()))
}

/// Auto-tune shard size so updates over Steam Pipe / a CDN don't have to
/// rewrite one giant file. Aims for `~ceil(sqrt(asset_count))` shards
/// (so a typical patch touches ~1/sqrt(N) of the bundle on average), with
/// per-shard size clamped to [4 MB, 256 MB]. The wide ceiling means
/// AAA-scale projects (~6 GB) settle around 24-32 shards of ~200 MB each
/// instead of 200+ tiny shards — fewer files for Steam Pipe / OS to track,
/// while still small enough that a single touched shard's re-download
/// finishes in seconds on a normal connection.
fn dynamic_shard_target(asset_count: usize, total_bytes: u64) -> u64 {
    const MIN_SHARD: u64 = 4 * 1024 * 1024;
    const MAX_SHARD: u64 = 256 * 1024 * 1024;
    if asset_count == 0 || total_bytes <= MIN_SHARD {
        return MIN_SHARD;
    }
    // Cap shard count at 256 so absurdly fragmented projects don't drown
    // the OS in tiny files; floor at 2 so we always at least try to split.
    let target_shards = ((asset_count as f64).sqrt().ceil() as u64).clamp(2, 256);
    let per_shard = total_bytes / target_shards;
    per_shard.clamp(MIN_SHARD, MAX_SHARD)
}

/// Sharded asset write: partitions assets into N `<id>.assets.shardNNNN.managed`
/// files (each one is a normal assets.managed) plus a small manifest at
/// `<id>.assets.manifest.managed` that lists which assets live in which shard.
/// Walks assets in alphabetical order so a given asset always lands in the
/// same shard run-to-run — keeps shard hashes stable across builds so a
/// CDN / Steam Pipe only re-downloads touched shards on patch.
///
/// Shard size is computed automatically based on total payload size + asset
/// count, optimizing for "many smallish files that update fast" rather than
/// "one huge file Steam has to fully rewrite". Targets ~ceil(sqrt(N)) shards
/// (so 100 assets → 10 shards, 400 → 20, 1000 → 32) clamped to a 2 MB floor
/// and a 32 MB ceiling per shard.
pub fn write_assets_sharded(
    managed_dir: &Path,
    id: &str,
    name: &str,
    assets: &HashMap<String, Vec<u8>>,
    compress: bool,
) -> Result<usize, String> {
    if assets.is_empty() {
        return Ok(0);
    }
    let total_bytes: u64 = assets.values().map(|v| v.len() as u64).sum();
    let target_bytes = dynamic_shard_target(assets.len(), total_bytes);

    let mut keys: Vec<&String> = assets.keys().collect();
    keys.sort();

    let mut shards: Vec<Vec<&String>> = Vec::new();
    let mut current: Vec<&String> = Vec::new();
    let mut current_bytes: u64 = 0;
    for k in &keys {
        let len = assets[*k].len() as u64;
        if !current.is_empty() && current_bytes + len > target_bytes {
            shards.push(std::mem::take(&mut current));
            current_bytes = 0;
        }
        current.push(*k);
        current_bytes += len;
        if len > target_bytes && !current.is_empty() {
            shards.push(std::mem::take(&mut current));
            current_bytes = 0;
        }
    }
    if !current.is_empty() {
        shards.push(current);
    }

    let mut shard_descriptors = Vec::with_capacity(shards.len());
    for (idx, shard) in shards.iter().enumerate() {
        let shard_name = format!("{id}.assets.shard{:04}.managed", idx + 1);
        let shard_path = managed_dir.join(&shard_name);
        let mut shard_assets: HashMap<String, Vec<u8>> = HashMap::new();
        for k in shard {
            shard_assets.insert((*k).clone(), assets[*k].clone());
        }
        write_assets_managed(&shard_path, id, name, &shard_assets, compress)?;
        let size = fs::metadata(&shard_path).map(|m| m.len()).unwrap_or(0);
        shard_descriptors.push(json!({
            "name": shard_name,
            "asset_count": shard.len(),
            "size_bytes": size,
            "assets": shard,
        }));
    }

    let manifest_path = managed_dir.join(format!("{id}.assets.manifest.managed"));
    let body = json!({
        "kind": "assets_manifest",
        "id": id,
        "name": name,
        "compressed": compress,
        "shards": shard_descriptors,
    });
    let plain = serde_json::to_vec(&body).map_err(|e| e.to_string())?;
    let encrypted = crate::managed::encrypt_payload(&plain)?;
    fs::write(&manifest_path, encrypted)
        .map_err(|e| format!("write {}: {e}", manifest_path.display()))?;

    Ok(shards.len())
}


pub fn load_managed_dir(dir: &Path) -> Result<HashMap<String, LoadedPackage>, String> {
    let mut packages: HashMap<String, LoadedPackage> = HashMap::new();
    if !dir.is_dir() {
        return Ok(packages);
    }
    for entry in fs::read_dir(dir).map_err(|e| format!("read_dir {}: {e}", dir.display()))? {
        let entry = entry.map_err(|e| e.to_string())?;
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("managed") {
            continue;
        }
        let encrypted = fs::read(&path).map_err(|e| format!("read {}: {e}", path.display()))?;
        let plain = crate::managed::decrypt_payload(&encrypted)
            .map_err(|e| format!("decrypt {}: {e}", path.display()))?;
        let parsed: JsonValue =
            serde_json::from_slice(&plain).map_err(|e| format!("parse {}: {e}", path.display()))?;
        let kind = parsed.get("kind").and_then(|v| v.as_str()).unwrap_or("");
        let id = parsed
            .get("id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| format!("{} missing 'id'", path.display()))?
            .to_string();

        let pkg = packages.entry(id.clone()).or_insert_with(|| LoadedPackage {
            id: id.clone(),
            name: id.clone(),
            version: String::new(),
            creator: String::new(),
            entry: "Main.luau".to_string(),
            file_type: FileType::Relative,
            physical_root: None,
            files: HashMap::new(),
            assets: HashMap::new(),
            compressed: false,
        });
        if parsed.get("compressed").and_then(|v| v.as_bool()).unwrap_or(false) {
            pkg.compressed = true;
        }

        if let Some(s) = parsed.get("name").and_then(|v| v.as_str()) {
            pkg.name = s.to_string();
        }
        if let Some(s) = parsed.get("version").and_then(|v| v.as_str()) {
            pkg.version = s.to_string();
        }
        if let Some(s) = parsed.get("creator").and_then(|v| v.as_str()) {
            pkg.creator = s.to_string();
        }

        match kind {
            "scripts" => {
                if let Some(s) = parsed.get("entry").and_then(|v| v.as_str()) {
                    pkg.entry = s.to_string();
                }
                if let Some(s) = parsed.get("file_type").and_then(|v| v.as_str()) {
                    if let Some(ft) = FileType::parse(s) {
                        pkg.file_type = ft;
                    }
                }
                if let Some(obj) = parsed.get("files").and_then(|v| v.as_object()) {
                    for (k, v) in obj {
                        if let Some(s) = v.as_str() {
                            // Preserve the encoded form when compressed —
                            // decompression + BOM stripping happens in
                            // vfs::read_module on access.
                            let stored = if pkg.compressed { s.to_string() } else { strip_bom(s.to_string()) };
                            pkg.files.insert(k.clone(), stored);
                        }
                    }
                }
            }
            "assets" => {
                if let Some(obj) = parsed.get("assets").and_then(|v| v.as_object()) {
                    for (k, v) in obj {
                        if let Some(b64) = v.as_str() {
                            
                            
                            pkg.assets.insert(k.clone(), b64.to_string());
                        }
                    }
                }
            }
            _ => {}
        }
    }
    Ok(packages)
}


pub fn write_launcher_exe(
    out_path: &Path,
    info: &LauncherInfo,
    icon_bytes: Option<&[u8]>,
    windowed: bool,
) -> Result<(), String> {
    let trailer = json!({
        "kind": "launcher",
        "default_id": info.default_id,
        "name": info.name,
        "version": info.version,
        "creator": info.creator,
        "steam_app_id": info.steam_app_id,
    });
    let trailer_bytes = serde_json::to_vec(&trailer).map_err(|e| e.to_string())?;

    let self_exe = env::current_exe().map_err(|e| e.to_string())?;
    let mut exe_bytes = fs::read(&self_exe)
        .map_err(|e| format!("read {}: {e}", self_exe.display()))?;
    
    
    if windowed {
        patch_subsystem_to_gui(&mut exe_bytes)?;
    } else {
        patch_subsystem_to_console(&mut exe_bytes)?;
    }
    {
        let mut f = fs::File::create(out_path)
            .map_err(|e| format!("create {}: {e}", out_path.display()))?;
        f.write_all(&exe_bytes)
            .map_err(|e| format!("write {}: {e}", out_path.display()))?;
        f.sync_all()
            .map_err(|e| format!("sync {}: {e}", out_path.display()))?;
    }
    drop(exe_bytes);

    if let Some(ico) = icon_bytes {
        let mut last_err = String::new();
        let mut ok = false;
        for delay_ms in [0u64, 100, 300, 700] {
            if delay_ms > 0 {
                std::thread::sleep(std::time::Duration::from_millis(delay_ms));
            }
            match crate::icon::embed_icon(out_path, ico) {
                Ok(()) => {
                    ok = true;
                    break;
                }
                Err(e) => last_err = e,
            }
        }
        if !ok {
            return Err(format!("icon embed (after retries): {last_err}"));
        }
    }

    let mut f = fs::OpenOptions::new()
        .append(true)
        .open(out_path)
        .map_err(|e| format!("open {} for append: {e}", out_path.display()))?;
    f.write_all(&trailer_bytes).map_err(|e| e.to_string())?;
    f.write_all(&(trailer_bytes.len() as u64).to_le_bytes())
        .map_err(|e| e.to_string())?;
    f.write_all(MAGIC).map_err(|e| e.to_string())?;
    Ok(())
}

pub fn try_self_launcher() -> Option<LauncherInfo> {
    let exe = env::current_exe().ok()?;
    let mut f = fs::File::open(&exe).ok()?;
    let len = f.metadata().ok()?.len();
    if len < 16 {
        return None;
    }
    f.seek(SeekFrom::End(-16)).ok()?;
    let mut tail = [0u8; 16];
    f.read_exact(&mut tail).ok()?;
    if &tail[8..16] != MAGIC {
        return None;
    }
    let json_len = u64::from_le_bytes(tail[0..8].try_into().ok()?);
    if json_len == 0 || json_len + 16 > len {
        return None;
    }
    f.seek(SeekFrom::End(-(16 + json_len as i64))).ok()?;
    let mut buf = vec![0u8; json_len as usize];
    f.read_exact(&mut buf).ok()?;

    let parsed: JsonValue = serde_json::from_slice(&buf).ok()?;
    if parsed.get("kind").and_then(|v| v.as_str()) != Some("launcher") {
        return None;
    }
    Some(LauncherInfo {
        default_id: parsed.get("default_id")?.as_str()?.to_string(),
        name: parsed
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string(),
        version: parsed
            .get("version")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string(),
        creator: parsed
            .get("creator")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string(),
        steam_app_id: parsed
            .get("steam_app_id")
            .and_then(|v| v.as_u64())
            .and_then(|v| if v <= u32::MAX as u64 { Some(v as u32) } else { None }),
    })
}

pub fn default_generated_dir() -> Result<PathBuf, String> {
    let cwd = env::current_dir().map_err(|e| e.to_string())?;
    Ok(cwd.join("Generated"))
}


fn pe_offset(bytes: &[u8]) -> Result<usize, String> {
    if bytes.len() < 0x40 {
        return Err("PE too small for DOS header".into());
    }
    let off = u32::from_le_bytes([bytes[0x3C], bytes[0x3D], bytes[0x3E], bytes[0x3F]]) as usize;
    if off + 0x60 > bytes.len() {
        return Err("PE header out of bounds".into());
    }
    if &bytes[off..off + 4] != b"PE\0\0" {
        return Err("not a PE file".into());
    }
    Ok(off)
}

fn patch_subsystem_to_gui(bytes: &mut [u8]) -> Result<(), String> {
    write_subsystem(bytes, 2)
}

fn patch_subsystem_to_console(bytes: &mut [u8]) -> Result<(), String> {
    write_subsystem(bytes, 3)
}

fn write_subsystem(bytes: &mut [u8], value: u8) -> Result<(), String> {
    let off = pe_offset(bytes)?;
    let subsys_offset = off + 0x5C;
    bytes[subsys_offset] = value;
    bytes[subsys_offset + 1] = 0;
    zero_checksum_at(bytes, off);
    Ok(())
}

fn zero_checksum_at(bytes: &mut [u8], pe_off: usize) {
    let cksum = pe_off + 0x58;
    for b in &mut bytes[cksum..cksum + 4] {
        *b = 0;
    }
}
