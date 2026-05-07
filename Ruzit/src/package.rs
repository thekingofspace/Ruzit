//! Build / Package output: launcher exe + `.scripts.managed` + `.assets.managed`.
//!
//! See [`write_launcher_exe`], [`write_scripts_managed`], [`write_assets_managed`],
//! and [`load_managed_dir`] for the file-format details. Walks via [`collect_project`]
//! skip subfolders that contain their own `ManagedInfo.toml` so DLC sources don't
//! accidentally land in the standard build.

use std::collections::HashMap;
use std::env;
use std::fs;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use base64::{Engine, engine::general_purpose::STANDARD as B64};
use serde_json::{Value as JsonValue, json};

use crate::config::FileType;

pub const MAGIC: &[u8; 8] = b"RUZITPKG";

/// Slim metadata embedded in a launcher exe trailer.
///
/// Tells the launcher which package id under `./Managed/` is the entry point.
pub struct LauncherInfo {
    pub default_id: String,
    pub name: String,
    pub version: String,
    pub creator: String,
}

/// Loaded `.scripts.managed` / `.assets.managed` pair on disk, after envelope
/// decrypt. Asset values are kept as base64 strings so they're decoded **lazily**
/// when something actually asks for them via `Asset.GetAsset` — startup stays cheap
/// even with thousands of assets.
pub struct LoadedPackage {
    pub id: String,
    pub name: String,
    pub version: String,
    pub creator: String,
    pub entry: String,
    pub file_type: FileType,
    pub physical_root: Option<std::path::PathBuf>,
    pub files: HashMap<String, String>,
    pub assets: HashMap<String, String>, // base64 of raw bytes (lazy)
}

/// Walk a project root, separating Luau sources from assets.
/// Tooling files (build.toml, types.d.luau, .vscode, .git, target, *.exe) are skipped.
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
            // Skip subfolders that are themselves packages (have their own
            // ManagedInfo.toml) — those are built separately via `Ruzit Package`.
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

/// Remove a leading UTF-8 BOM (\u{FEFF}) if present.
///
/// Editors that save .luau as "UTF-8 with BOM" produce files Luau's parser
/// rejects with `Expected identifier when parsing expression, got Unicode
/// character U+feff`. We sanitize at packaging time so .managed files are
/// always BOM-free.
pub fn strip_bom(s: String) -> String {
    const BOM: &str = "\u{feff}";
    if s.starts_with(BOM) {
        s[BOM.len()..].to_string()
    } else {
        s
    }
}

/// Find every immediate-or-nested subfolder of `root` that looks like a DLC:
/// it has a `ManagedInfo.toml` and an `init.luau` (or whatever its
/// ManagedInfo declares as Entry).
///
/// Recursion stops once a DLC is found — we don't allow nested DLCs.
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
            continue; // don't recurse into a DLC
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
    if rel == "build.toml"
        || rel == "types.d.luau"
        || rel == "ManagedInfo.toml"
        || rel == ".DS_Store"
    {
        return true;
    }
    if rel.to_lowercase().ends_with(".exe") || rel.to_lowercase().ends_with(".managed") {
        return true;
    }
    false
}

// ============================================================================
// .managed file IO  (one file per kind: scripts or assets, paired by id)
// ============================================================================

pub fn write_scripts_managed(
    path: &Path,
    id: &str,
    name: &str,
    version: &str,
    creator: &str,
    entry: &str,
    file_type: FileType,
    files: &HashMap<String, String>,
) -> Result<(), String> {
    let mut json_files = serde_json::Map::new();
    for (k, v) in files {
        json_files.insert(k.clone(), JsonValue::String(v.clone()));
    }
    let body = json!({
        "kind": "scripts",
        "id": id,
        "name": name,
        "version": version,
        "creator": creator,
        "entry": entry,
        "file_type": file_type.as_str(),
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
) -> Result<(), String> {
    let mut json_assets = serde_json::Map::new();
    for (k, bytes) in assets {
        json_assets.insert(k.clone(), JsonValue::String(B64.encode(bytes)));
    }
    let body = json!({
        "kind": "assets",
        "id": id,
        "name": name,
        "assets": json_assets,
    });
    let plain = serde_json::to_vec(&body).map_err(|e| e.to_string())?;
    let encrypted = crate::managed::encrypt_payload(&plain)?;
    fs::write(path, encrypted).map_err(|e| format!("write {}: {e}", path.display()))
}

/// Load every `.managed` file under `dir`, pairing scripts+assets by id.
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
        });

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
                            pkg.files.insert(k.clone(), strip_bom(s.to_string()));
                        }
                    }
                }
            }
            "assets" => {
                if let Some(obj) = parsed.get("assets").and_then(|v| v.as_object()) {
                    for (k, v) in obj {
                        if let Some(b64) = v.as_str() {
                            // Lazy: keep the base64 string; Asset.GetAsset
                            // will decode it on demand.
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

// ============================================================================
// Launcher exe (Ruzit runtime + tiny trailer pointing at the default package)
// ============================================================================

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
    });
    let trailer_bytes = serde_json::to_vec(&trailer).map_err(|e| e.to_string())?;

    let self_exe = env::current_exe().map_err(|e| e.to_string())?;
    let mut exe_bytes = fs::read(&self_exe)
        .map_err(|e| format!("read {}: {e}", self_exe.display()))?;
    if windowed {
        // Switch the PE subsystem to GUI so Explorer launches don't flash a
        // console. Console output then requires the user passing --console.
        patch_subsystem_to_gui(&mut exe_bytes)?;
    } else {
        // Default: leave the launcher as console-subsystem (inherited from
        // Ruzit.exe). cmd launches always show output, double-clicking from
        // Explorer briefly flashes a console.
        zero_pe_checksum(&mut exe_bytes)?;
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
    })
}

pub fn default_generated_dir() -> Result<PathBuf, String> {
    let cwd = env::current_dir().map_err(|e| e.to_string())?;
    Ok(cwd.join("Generated"))
}

// ============================================================================
// PE patching
// ============================================================================

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
    let off = pe_offset(bytes)?;
    let subsys_offset = off + 0x5C;
    bytes[subsys_offset] = 2;
    bytes[subsys_offset + 1] = 0;
    zero_checksum_at(bytes, off);
    Ok(())
}

/// Zero `OptionalHeader.CheckSum` so editing tools (and our own appended
/// trailer) don't fail validation against a stale checksum.
fn zero_pe_checksum(bytes: &mut [u8]) -> Result<(), String> {
    let off = pe_offset(bytes)?;
    zero_checksum_at(bytes, off);
    Ok(())
}

fn zero_checksum_at(bytes: &mut [u8], pe_off: usize) {
    let cksum = pe_off + 0x58;
    for b in &mut bytes[cksum..cksum + 4] {
        *b = 0;
    }
}
