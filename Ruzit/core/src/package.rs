use std::collections::HashMap;
use std::env;
use std::fs;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use base64::{Engine, engine::general_purpose::STANDARD as B64};
use serde_json::{Value as JsonValue, json};

use crate::config::FileType;

pub const MAGIC: &[u8; 8] = b"RUZITPKG";

pub const PACKAGES_DIR_NAME: &str = "Packages";

pub fn encode_assets_b64(raw: HashMap<String, Vec<u8>>) -> HashMap<String, String> {
    raw.into_iter().map(|(k, v)| (k, B64.encode(v))).collect()
}

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
    pub scripts_compressed: bool,
    pub assets_compressed: bool,
    pub scripts_bytecode: bool,
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

            let key = rel
                .strip_prefix("assets/")
                .map(|s| s.to_string())
                .unwrap_or(rel);
            assets.insert(key, bytes);
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
            || rel == PACKAGES_DIR_NAME
            || rel.starts_with(&format!("{PACKAGES_DIR_NAME}/"))
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
    files: &HashMap<String, Vec<u8>>,
    compress: bool,
    bytecode: bool,
) -> Result<(), String> {
    let mut json_files = serde_json::Map::new();
    for (k, v) in files {
        let value = if compress {
            let compressed = zstd::stream::encode_all(v.as_slice(), 3)
                .map_err(|e| format!("compress {k}: {e}"))?;
            B64.encode(&compressed)
        } else if bytecode {
            B64.encode(v.as_slice())
        } else {
            String::from_utf8(v.clone())
                .map_err(|_| format!("script {k} contains non-UTF-8 bytes; enable compress or bytecode"))?
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
        "bytecode": bytecode,
        "files": json_files,
    });
    let plain = serde_json::to_vec(&body).map_err(|e| e.to_string())?;
    let encrypted = crate::managed::encrypt_payload(&plain)?;
    fs::write(path, encrypted).map_err(|e| format!("write {}: {e}", path.display()))
}

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

fn dynamic_shard_target(asset_count: usize, total_bytes: u64) -> u64 {
    const MIN_SHARD: u64 = 4 * 1024 * 1024;
    const MAX_SHARD: u64 = 256 * 1024 * 1024;
    if asset_count == 0 || total_bytes <= MIN_SHARD {
        return MIN_SHARD;
    }
    let target_shards = ((asset_count as f64).sqrt().ceil() as u64).clamp(2, 256);
    let per_shard = total_bytes / target_shards;
    per_shard.clamp(MIN_SHARD, MAX_SHARD)
}

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

pub fn discover_managed_files(dir: &Path) -> Result<HashMap<String, Vec<PathBuf>>, String> {
    let mut groups: HashMap<String, Vec<PathBuf>> = HashMap::new();
    if !dir.is_dir() {
        return Ok(groups);
    }
    for entry in fs::read_dir(dir).map_err(|e| format!("read_dir {}: {e}", dir.display()))? {
        let entry = entry.map_err(|e| e.to_string())?;
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("managed") {
            continue;
        }
        let fname = match path.file_name().and_then(|s| s.to_str()) {
            Some(s) => s,
            None => continue,
        };
        let id = match fname.find('.') {
            Some(dot) if dot > 0 => &fname[..dot],
            _ => continue,
        };
        groups.entry(id.to_string()).or_default().push(path);
    }
    Ok(groups)
}

pub enum ManagedSource {
    Disk(PathBuf),
    Memory { label: String, bytes: Vec<u8> },
}

impl ManagedSource {
    /// Return `(label_for_errors, encrypted_bytes)`.
    pub fn read(&self) -> Result<(String, Vec<u8>), String> {
        match self {
            ManagedSource::Disk(p) => {
                let bytes = fs::read(p).map_err(|e| format!("read {}: {e}", p.display()))?;
                Ok((p.display().to_string(), bytes))
            }
            ManagedSource::Memory { label, bytes } => Ok((label.clone(), bytes.clone())),
        }
    }
}

impl From<PathBuf> for ManagedSource {
    fn from(p: PathBuf) -> Self {
        ManagedSource::Disk(p)
    }
}

pub fn load_single_managed_package(
    id: &str,
    sources: &[ManagedSource],
) -> Result<LoadedPackage, String> {
    let mut pkg = LoadedPackage {
        id: id.to_string(),
        name: id.to_string(),
        version: String::new(),
        creator: String::new(),
        entry: "Main.luau".to_string(),
        file_type: FileType::Relative,
        physical_root: None,
        files: HashMap::new(),
        assets: HashMap::new(),
        scripts_compressed: false,
        assets_compressed: false,
        scripts_bytecode: false,
    };
    for source in sources {
        let (label, encrypted) = source.read()?;
        merge_managed_bytes_into(&mut pkg, &label, &encrypted)?;
    }
    Ok(pkg)
}

pub fn merge_managed_bytes_into(
    pkg: &mut LoadedPackage,
    label: &str,
    encrypted: &[u8],
) -> Result<(), String> {
    let plain = crate::managed::decrypt_payload(encrypted)
        .map_err(|e| format!("decrypt {label}: {e}"))?;
    let parsed: JsonValue =
        serde_json::from_slice(&plain).map_err(|e| format!("parse {label}: {e}"))?;
    let kind = parsed.get("kind").and_then(|v| v.as_str()).unwrap_or("");
    let header_compressed = parsed
        .get("compressed")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let header_bytecode = parsed
        .get("bytecode")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

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
            pkg.scripts_compressed = header_compressed;
            pkg.scripts_bytecode = header_bytecode;
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
                        let stored = if header_compressed || header_bytecode {
                            s.to_string()
                        } else {
                            strip_bom(s.to_string())
                        };
                        pkg.files.insert(k.clone(), stored);
                    }
                }
            }
        }
        "assets" | "assets_manifest" => {
            pkg.assets_compressed = header_compressed;
            if let Some(obj) = parsed.get("assets").and_then(|v| v.as_object()) {
                for (k, v) in obj {
                    if let Some(b64) = v.as_str() {
                        let normalized =
                            k.strip_prefix("assets/").unwrap_or(k.as_str()).to_string();
                        pkg.assets.insert(normalized, b64.to_string());
                    }
                }
            }
        }
        _ => {}
    }
    Ok(())
}

pub fn locate_runtime_template() -> Result<PathBuf, String> {
    let self_exe = env::current_exe().map_err(|e| e.to_string())?;
    let dir = self_exe
        .parent()
        .ok_or_else(|| format!("could not derive parent of {}", self_exe.display()))?;
    let candidate_names: &[&str] = if cfg!(target_os = "windows") {
        &["ruzitrun.exe"]
    } else {
        &["ruzitrun"]
    };
    for name in candidate_names {
        let p = dir.join(name);
        if p.is_file() && p != self_exe {
            return Ok(p);
        }
    }
    Err(format!(
        "build: could not find ruzitrun{} alongside {}. \
         The runtime binary must be installed next to the CLI for `build` to copy it as a launcher template.",
        if cfg!(target_os = "windows") {
            ".exe"
        } else {
            ""
        },
        self_exe.display()
    ))
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

    let self_exe = locate_runtime_template()?;
    let mut exe_bytes =
        fs::read(&self_exe).map_err(|e| format!("read {}: {e}", self_exe.display()))?;

    #[cfg(windows)]
    {
        if windowed {
            patch_subsystem_to_gui(&mut exe_bytes)?;
        } else {
            patch_subsystem_to_console(&mut exe_bytes)?;
        }
    }
    #[cfg(not(windows))]
    {
        let _ = windowed;
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

    #[cfg(windows)]
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
    #[cfg(not(windows))]
    {
        let _ = icon_bytes;
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
            .and_then(|v| {
                if v <= u32::MAX as u64 {
                    Some(v as u32)
                } else {
                    None
                }
            }),
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
