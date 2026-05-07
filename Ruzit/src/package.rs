use std::collections::HashMap;
use std::env;
use std::fs;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use base64::{Engine, engine::general_purpose::STANDARD as B64};
use serde_json::{Value as JsonValue, json};

use crate::config::{BuildConfig, FileType};

const MAGIC: &[u8; 8] = b"RUZITPKG";

pub struct Bundle {
    pub files: HashMap<String, String>,
    pub assets: HashMap<String, Vec<u8>>,
    pub entry: String,
    pub config: BuildConfig,
}

/// Walk the project root and produce two maps:
///   - `sources`: every `.luau` / `.lua` text file (used by `require`)
///   - `assets`:  every file under `assets/` regardless of extension (used by `Asset.GetAsset`)
///
/// Tooling/editor files (build.toml, types.d.luau, .vscode, .git, target, *.exe) are skipped.
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
            walk(root, &path, sources, assets)?;
            continue;
        }
        if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
            if ext.eq_ignore_ascii_case("luau") || ext.eq_ignore_ascii_case("lua") {
                let text = fs::read_to_string(&path)
                    .map_err(|e| format!("read {}: {e}", path.display()))?;
                sources.insert(rel, text);
                continue;
            }
        }
        // Anything inside `assets/` becomes a binary asset.
        if rel.starts_with("assets/") {
            let bytes = fs::read(&path)
                .map_err(|e| format!("read {}: {e}", path.display()))?;
            assets.insert(rel, bytes);
        }
        // Files outside assets/ that aren't .luau/.lua are tooling — silently skipped.
    }
    Ok(())
}

fn should_skip(rel: &str) -> bool {
    if rel.starts_with(".vscode/") || rel.starts_with(".git/") || rel.starts_with("target/") {
        return true;
    }
    if rel == "build.toml" || rel == "types.d.luau" || rel == ".DS_Store" {
        return true;
    }
    if rel.to_lowercase().ends_with(".exe") {
        return true;
    }
    false
}

pub fn write_packaged_exe(
    files: &HashMap<String, String>,
    assets: &HashMap<String, Vec<u8>>,
    entry_rel: &str,
    config: &BuildConfig,
    icon_bytes: Option<&[u8]>,
    out_path: &Path,
) -> Result<usize, String> {
    let mut json_files = serde_json::Map::new();
    for (k, v) in files {
        json_files.insert(k.clone(), JsonValue::String(v.clone()));
    }
    let mut json_assets = serde_json::Map::new();
    for (k, bytes) in assets {
        json_assets.insert(k.clone(), JsonValue::String(B64.encode(bytes)));
    }
    let bundle = json!({
        "entry": entry_rel,
        "files": json_files,
        "assets": json_assets,
        "config": {
            "name": config.name,
            "version": config.version,
            "creator": config.creator,
            "file_type": config.file_type.as_str(),
        },
    });
    let bundle_bytes = serde_json::to_vec(&bundle).map_err(|e| e.to_string())?;

    // 1. Read our own exe into memory, patch PE bytes (subsystem + zero checksum),
    //    and write the result with explicit fsync. Doing it all in memory before
    //    write avoids a copy→read→write race that Windows would lock us out of.
    let self_exe = env::current_exe().map_err(|e| e.to_string())?;
    let mut exe_bytes = fs::read(&self_exe)
        .map_err(|e| format!("read {}: {e}", self_exe.display()))?;
    patch_subsystem_to_gui(&mut exe_bytes)?;
    {
        let mut f = fs::File::create(out_path)
            .map_err(|e| format!("create {}: {e}", out_path.display()))?;
        f.write_all(&exe_bytes)
            .map_err(|e| format!("write {}: {e}", out_path.display()))?;
        f.sync_all()
            .map_err(|e| format!("sync {}: {e}", out_path.display()))?;
    }
    drop(exe_bytes);

    // 2. If we have an icon, embed it now — must happen *before* we append the trailer,
    //    because Win32 BeginUpdateResource rewrites the PE and would clobber an overlay.
    //    Retry briefly if Windows AV / write-cache is still holding the file.
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

    // 3. Append bundle trailer.
    let mut f = fs::OpenOptions::new()
        .append(true)
        .open(out_path)
        .map_err(|e| format!("open {} for append: {e}", out_path.display()))?;
    f.write_all(&bundle_bytes).map_err(|e| e.to_string())?;
    f.write_all(&(bundle_bytes.len() as u64).to_le_bytes())
        .map_err(|e| e.to_string())?;
    f.write_all(MAGIC).map_err(|e| e.to_string())?;
    Ok(bundle_bytes.len())
}

pub fn try_self_bundle() -> Option<Bundle> {
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
    let entry = parsed.get("entry")?.as_str()?.to_string();
    let files_obj = parsed.get("files")?.as_object()?;
    let mut files = HashMap::new();
    for (k, v) in files_obj {
        files.insert(k.clone(), v.as_str()?.to_string());
    }
    let mut assets: HashMap<String, Vec<u8>> = HashMap::new();
    if let Some(assets_obj) = parsed.get("assets").and_then(|v| v.as_object()) {
        for (k, v) in assets_obj {
            if let Some(b64) = v.as_str() {
                if let Ok(bytes) = B64.decode(b64) {
                    assets.insert(k.clone(), bytes);
                }
            }
        }
    }

    let mut config = BuildConfig::default();
    if let Some(c) = parsed.get("config") {
        if let Some(s) = c.get("name").and_then(|x| x.as_str()) {
            config.name = s.to_string();
        }
        if let Some(s) = c.get("version").and_then(|x| x.as_str()) {
            config.version = s.to_string();
        }
        if let Some(s) = c.get("creator").and_then(|x| x.as_str()) {
            config.creator = s.to_string();
        }
        if let Some(s) = c.get("file_type").and_then(|x| x.as_str()) {
            if let Some(ft) = FileType::parse(s) {
                config.file_type = ft;
            }
        }
    }

    Some(Bundle {
        files,
        assets,
        entry,
        config,
    })
}

fn patch_subsystem_to_gui(bytes: &mut [u8]) -> Result<(), String> {
    if bytes.len() < 0x40 {
        return Err("PE too small for DOS header".into());
    }
    let pe_offset = u32::from_le_bytes([bytes[0x3C], bytes[0x3D], bytes[0x3E], bytes[0x3F]]) as usize;
    if pe_offset + 0x60 > bytes.len() {
        return Err("PE header out of bounds".into());
    }
    if &bytes[pe_offset..pe_offset + 4] != b"PE\0\0" {
        return Err("not a PE file (PE\\0\\0 sig missing)".into());
    }
    // Subsystem field lives at pe_offset + 0x5C (PE sig 4 + FileHeader 20 +
    // OptionalHeader offset 0x44 = 0x5C).
    let subsys_offset = pe_offset + 0x5C;
    bytes[subsys_offset] = 2; // IMAGE_SUBSYSTEM_WINDOWS_GUI
    bytes[subsys_offset + 1] = 0;

    // Zero the PE OptionalHeader.CheckSum field (offset 0x40 in the optional
    // header, absolute pe_offset + 0x58). Otherwise BeginUpdateResource on a
    // file we've hand-edited may treat the (now-stale) checksum as corruption.
    let cksum_offset = pe_offset + 0x58;
    for b in &mut bytes[cksum_offset..cksum_offset + 4] {
        *b = 0;
    }
    Ok(())
}

pub fn default_output_path(stem: &str) -> Result<PathBuf, String> {
    let cwd = env::current_dir().map_err(|e| e.to_string())?;
    Ok(cwd.join(format!("{stem}.exe")))
}
