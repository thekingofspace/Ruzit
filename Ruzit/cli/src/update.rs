use std::env;
use std::fs;
use std::io::{Cursor, Read};
use std::path::{Path, PathBuf};

use ureq::Agent;
use zip::ZipArchive;

const STEAM_LIB_NAME_WIN: &str = "steam_api64.dll";
const STEAM_LIB_NAME_LINUX: &str = "libsteam_api.so";
const STEAM_LIB_NAME_MAC: &str = "libsteam_api.dylib";

fn steam_lib_name() -> &'static str {
    if cfg!(target_os = "windows") {
        STEAM_LIB_NAME_WIN
    } else if cfg!(target_os = "linux") {
        STEAM_LIB_NAME_LINUX
    } else if cfg!(target_os = "macos") {
        STEAM_LIB_NAME_MAC
    } else {
        ""
    }
}

fn agent() -> Agent {
    ureq::AgentBuilder::new()
        .timeout(std::time::Duration::from_secs(60))
        .user_agent("ruzit/1.2.5")
        .build()
}

fn download_bytes(url: &str) -> Result<Vec<u8>, String> {
    println!("[ruzit] GET {url}");
    let resp = agent()
        .get(url)
        .call()
        .map_err(|e| format!("HTTP error: {e}"))?;
    let status = resp.status();
    if status != 200 {
        return Err(format!("HTTP {status} for {url}"));
    }
    let mut reader = resp.into_reader();
    let mut buf: Vec<u8> = Vec::new();
    reader
        .read_to_end(&mut buf)
        .map_err(|e| format!("read body: {e}"))?;
    Ok(buf)
}

fn download_to(url: &str, dest: &Path) -> Result<u64, String> {
    let buf = download_bytes(url)?;
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("mkdir {}: {e}", parent.display()))?;
    }
    fs::write(dest, &buf).map_err(|e| format!("write {}: {e}", dest.display()))?;
    Ok(buf.len() as u64)
}

fn extract_one_from_zip(
    archive_bytes: &[u8],
    inner_name: &str,
    dest: &Path,
) -> Result<u64, String> {
    let mut zip =
        ZipArchive::new(Cursor::new(archive_bytes)).map_err(|e| format!("open zip: {e}"))?;
    let mut entry = zip
        .by_name(inner_name)
        .map_err(|e| format!("zip missing '{inner_name}': {e}"))?;
    let mut buf: Vec<u8> = Vec::with_capacity(entry.size() as usize);
    entry
        .read_to_end(&mut buf)
        .map_err(|e| format!("read zip entry '{inner_name}': {e}"))?;
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("mkdir {}: {e}", parent.display()))?;
    }
    fs::write(dest, &buf).map_err(|e| format!("write {}: {e}", dest.display()))?;
    Ok(buf.len() as u64)
}

const DEFAULT_TYPES_URL: &str =
    "https://raw.githubusercontent.com/thekingofspace/Ruzit/master/types.d.luau";

pub fn types_dluau_url() -> String {
    env::var("RUZIT_TYPES_URL").unwrap_or_else(|_| DEFAULT_TYPES_URL.to_string())
}

pub fn fetch_types_dluau() -> Result<String, String> {
    let url = types_dluau_url();
    let bytes = download_bytes(&url)?;
    String::from_utf8(bytes).map_err(|e| format!("types.d.luau from {url} is not valid UTF-8: {e}"))
}

pub fn cmd_refresh_types(arg: Option<&String>) -> Result<(), String> {
    let dest_dir = match arg {
        Some(p) => PathBuf::from(p),
        None => env::current_dir().map_err(|e| e.to_string())?,
    };
    let dest = dest_dir.join("types.d.luau");
    let body = fetch_types_dluau()?;
    fs::create_dir_all(&dest_dir).map_err(|e| format!("mkdir {}: {e}", dest_dir.display()))?;
    fs::write(&dest, &body).map_err(|e| format!("write {}: {e}", dest.display()))?;
    println!(
        "[ruzit] refreshed {} ({} bytes)",
        dest.display(),
        body.len()
    );
    Ok(())
}

pub fn cmd_fetch_deps(arg: Option<&String>) -> Result<(), String> {
    let dest_dir = match arg {
        Some(p) => PathBuf::from(p),
        None => env::current_dir().map_err(|e| e.to_string())?,
    };
    let lib = steam_lib_name();
    if lib.is_empty() {
        return Err("fetch-deps: no Steam redistributable for this platform".into());
    }
    let url = match env::var("RUZIT_STEAMSDK_URL") {
        Ok(v) => v,
        Err(_) => {
            return Err(format!(
                "fetch-deps: set RUZIT_STEAMSDK_URL to a direct {lib} URL.\n\
                 Get the Steamworks SDK from https://partner.steamgames.com/, host \n\
                 the {lib} from sdk/redistributable_bin/ on your own server, then set\n\
                 RUZIT_STEAMSDK_URL=https://your.host/path/{lib}"
            ));
        }
    };
    let dest = dest_dir.join(lib);
    let bytes = download_to(&url, &dest)?;
    println!("[ruzit] fetched {} ({} bytes)", dest.display(), bytes);
    Ok(())
}

#[cfg(unix)]
fn make_executable(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    if let Ok(meta) = fs::metadata(path) {
        let mut perms = meta.permissions();
        perms.set_mode(0o755);
        let _ = fs::set_permissions(path, perms);
    }
}
#[cfg(not(unix))]
fn make_executable(_path: &Path) {}

pub fn cmd_update(arg: Option<&String>) -> Result<(), String> {
    let exe_dir = env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(PathBuf::from))
        .ok_or("update: could not locate current exe")?;
    let dest_dir = match arg {
        Some(p) => PathBuf::from(p),
        None => exe_dir.clone(),
    };
    let base = env::var("RUZIT_RELEASE_BASE").unwrap_or_else(|_| {
        String::from("https://github.com/thekingofspace/Ruzit/releases/latest/download")
    });
    let platform = if cfg!(target_os = "windows") {
        "windows"
    } else if cfg!(target_os = "linux") {
        "linux"
    } else if cfg!(target_os = "macos") {
        "macos"
    } else {
        return Err("update: unsupported platform".into());
    };
    let exe_ext = if cfg!(target_os = "windows") {
        ".exe"
    } else {
        ""
    };

    let archive_url = format!("{base}/ruzit-{platform}.zip");

    fs::create_dir_all(&dest_dir).map_err(|e| format!("mkdir {}: {e}", dest_dir.display()))?;

    let archive_bytes = download_bytes(&archive_url)?;

    let runtime_inner = format!("ruzitrun{exe_ext}");
    let runtime_dest = dest_dir.join(&runtime_inner);

    let runtime_bytes = extract_one_from_zip(&archive_bytes, &runtime_inner, &runtime_dest)?;
    make_executable(&runtime_dest);

    println!(
        "[ruzit] update complete from {}\n        â†’ {} ({} bytes)\n\
         [ruzit] CLI binary unchanged. Reinstall ruzit{exe_ext} manually if you need a new CLI.",
        archive_url,
        runtime_dest.display(),
        runtime_bytes,
    );
    Ok(())
}
