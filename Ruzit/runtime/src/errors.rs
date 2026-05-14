use mlua::Lua;

use crate::vfs::{Fs, split_owner};

pub const IMPORT_LIBS: &[&str] = &[
    "Actor",
    "Asset",
    "Debug",
    "DynMesh",
    "Gamepad",
    "GPU",
    "GUI",
    "IO",
    "Keyboard",
    "Managed",
    "Mouse",
    "Net",
    "Primitives",
    "Process",
    "Register",
    "Renderable",
    "Serde",
    "SFX",
    "Signal",
    "Steam",
    "Voice",
    "VR",
    "VirtualReality",
    "Window",
];

pub fn levenshtein(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let (n, m) = (a.len(), b.len());
    if n == 0 {
        return m;
    }
    if m == 0 {
        return n;
    }
    let mut prev: Vec<usize> = (0..=m).collect();
    let mut curr = vec![0usize; m + 1];
    for i in 1..=n {
        curr[0] = i;
        for j in 1..=m {
            let cost = if a[i - 1] == b[j - 1] { 0 } else { 1 };
            curr[j] = (prev[j] + 1).min(curr[j - 1] + 1).min(prev[j - 1] + cost);
        }
        std::mem::swap(&mut prev, &mut curr);
    }
    prev[m]
}

pub fn closest_match<'a, I>(needle: &str, candidates: I) -> Option<String>
where
    I: IntoIterator<Item = &'a str>,
{
    let needle_lc = needle.to_lowercase();
    let threshold = (needle.chars().count() / 3).max(1).min(3);
    let mut best: Option<(usize, &str)> = None;
    for c in candidates {
        let d = levenshtein(&needle_lc, &c.to_lowercase());
        if d <= threshold {
            best = match best {
                None => Some((d, c)),
                Some((bd, _)) if d < bd => Some((d, c)),
                other => other,
            };
        }
    }
    best.map(|(_, s)| s.to_string())
}

pub fn package_label(fs: &Fs, key: &str) -> String {
    match fs {
        Fs::Disk { .. } => "project".to_string(),
        Fs::Bundle {
            packages,
            default_id,
            ..
        } => {
            let (pkg_id, _) = split_owner(key, default_id);
            let Some(pkg) = packages.get(pkg_id) else {
                return format!("unknown package '{pkg_id}'");
            };
            let display = if pkg.name.is_empty() {
                pkg.id.clone()
            } else {
                pkg.name.clone()
            };
            if pkg_id == default_id {
                format!("project \"{display}\"")
            } else if pkg.physical_root.is_some() {
                format!("DLC \"{display}\" (id: {pkg_id})")
            } else {
                format!("package \"{display}\" (id: {pkg_id})")
            }
        }
    }
}

fn script_display(key: &str) -> String {
    if let Some(rest) = key.strip_prefix('@') {
        if let Some((_, inner)) = rest.split_once('/') {
            return inner.to_string();
        }
    }
    key.to_string()
}

fn annotate_first_location(fs: &Fs, msg: &str) -> Option<(String, String)> {
    if let Some(start) = msg.find("[string \"") {
        let rest = &msg[start + "[string \"".len()..];
        if let Some(end_q) = rest.find('"') {
            let raw_chunk = &rest[..end_q];
            let after = &rest[end_q + 1..];
            if let Some(stripped) = after.strip_prefix("]:") {
                if let Some(colon) = stripped.find(':') {
                    let line = &stripped[..colon];
                    if line.chars().all(|c| c.is_ascii_digit()) {
                        let key = raw_chunk.trim_start_matches('@');
                        return Some((
                            format!("[string \"{raw_chunk}\"]:{line}:"),
                            format_location(fs, key, line),
                        ));
                    }
                }
            }
        }
    }

    for (i, _) in msg.match_indices(".lua") {
        let prefix = &msg[..i];
        let ext_end = i
            + ".lua".len()
            + if msg[i + ".lua".len()..].starts_with('u') {
                1
            } else {
                0
            };
        let after = &msg[ext_end..];
        let Some(stripped) = after.strip_prefix(':') else {
            continue;
        };
        let Some(colon) = stripped.find(':') else {
            continue;
        };
        let line = &stripped[..colon];
        if !line.chars().all(|c| c.is_ascii_digit()) {
            continue;
        }
        let chunk_start = prefix
            .rfind(|c: char| c.is_whitespace() || c == '"' || c == '[')
            .map(|p| p + 1)
            .unwrap_or(0);
        let chunk = &msg[chunk_start..ext_end];
        let key = chunk.trim_start_matches('@');
        let needle = format!("{chunk}:{line}:");
        return Some((needle, format_location(fs, key, line)));
    }
    None
}

fn format_location(fs: &Fs, key: &str, line: &str) -> String {
    let script = script_display(key);
    let pkg = package_label(fs, key);
    format!("{script}:{line} (in {pkg}):")
}

fn append_suggestions(lua: &Lua, fs: &Fs, owner: &str, msg: &str) -> String {
    if let Some(rest) = msg.split("import: unknown library '").nth(1) {
        if let Some(end) = rest.find('\'') {
            let needle = &rest[..end];
            if let Some(hit) = closest_match(needle, IMPORT_LIBS.iter().copied()) {
                return format!("{msg}\n  hint: did you mean import(\"{hit}\")?");
            }
        }
    }

    if let Some(rest) = msg.split("module '").nth(1) {
        if let Some(end) = rest.find("' not found") {
            let needle = &rest[..end];
            if let Some(hit) = require_suggestion(fs, owner, needle) {
                return format!("{msg}\n  hint: did you mean require(\"{hit}\")?");
            }
        }
    }

    let markers = ["(global '", "(global \""];
    for marker in markers {
        if let Some(rest) = msg.split(marker).nth(1) {
            let close = match marker.as_bytes()[marker.len() - 1] {
                b'\'' => '\'',
                _ => '"',
            };
            if let Some(end) = rest.find(close) {
                let needle = &rest[..end];
                if let Some(hit) = global_suggestion(lua, needle) {
                    return format!("{msg}\n  hint: did you mean '{hit}'?");
                }
            }
        }
    }

    msg.to_string()
}

fn global_suggestion(lua: &Lua, needle: &str) -> Option<String> {
    let globals = lua.globals();
    let mut names: Vec<String> = Vec::new();
    for pair in globals.clone().pairs::<mlua::Value, mlua::Value>() {
        let Ok((k, _)) = pair else { continue };
        if let mlua::Value::String(s) = k {
            if let Ok(s) = s.to_str() {
                names.push(s.to_string());
            }
        }
    }
    closest_match(needle, names.iter().map(|s| s.as_str()))
}

fn require_suggestion(fs: &Fs, owner: &str, needle: &str) -> Option<String> {
    match fs {
        Fs::Disk { .. } => None,
        Fs::Bundle {
            packages,
            default_id,
            ..
        } => {
            let (pkg_id, _) = split_owner(owner, default_id);
            let pkg = packages.get(pkg_id)?;
            let leaf = needle
                .rsplit('/')
                .next()
                .unwrap_or(needle)
                .trim_end_matches(".luau")
                .trim_end_matches(".lua");
            let mut candidates: Vec<String> = Vec::new();
            for k in pkg.files.keys() {
                let stem = k
                    .trim_end_matches(".luau")
                    .trim_end_matches(".lua")
                    .to_string();
                candidates.push(stem);
            }
            closest_match(leaf, candidates.iter().map(|s| s.as_str()))
        }
    }
}

pub fn pretty_format(lua: &Lua, fs: &Fs, owner: &str, top_key: &str, err: &mlua::Error) -> String {
    let raw = err.to_string();
    pretty_format_msg(lua, fs, owner, top_key, &raw)
}

pub fn pretty_format_msg(lua: &Lua, fs: &Fs, owner: &str, top_key: &str, raw: &str) -> String {
    let trimmed = raw
        .strip_prefix("runtime error: ")
        .or_else(|| raw.strip_prefix("Luau error: "))
        .unwrap_or(raw)
        .to_string();

    let mut out = trimmed;

    if let Some((needle, replacement)) = annotate_first_location(fs, &out) {
        if let Some(pos) = out.find(&needle) {
            let prefix = &out[..pos];
            let suffix = &out[pos + needle.len()..];
            out = format!("{prefix}{replacement}{suffix}");
        }
    } else {
        let pkg = package_label(fs, top_key);
        let script = script_display(top_key);
        out = format!("{script} (in {pkg}): {out}");
    }

    out = append_suggestions(lua, fs, owner, &out);
    out
}

pub const FS_REGISTRY_KEY: &str = "ruzit_fs";

pub struct FsHandle(pub Fs);

impl mlua::UserData for FsHandle {}

pub fn install_fs(lua: &Lua, fs: &Fs) -> mlua::Result<()> {
    let ud = lua.create_userdata(FsHandle(fs.clone()))?;
    lua.set_named_registry_value(FS_REGISTRY_KEY, ud)
}

pub fn fs_from_registry(lua: &Lua) -> Option<Fs> {
    let ud: mlua::AnyUserData = lua.named_registry_value(FS_REGISTRY_KEY).ok()?;
    let handle = ud.borrow::<FsHandle>().ok()?;
    Some(handle.0.clone())
}

pub fn pretty_format_loose(lua: &Lua, owner_hint: &str, err: &mlua::Error) -> String {
    match fs_from_registry(lua) {
        Some(fs) => pretty_format(lua, &fs, owner_hint, owner_hint, err),
        None => err.to_string(),
    }
}
