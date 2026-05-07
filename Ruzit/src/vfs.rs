use std::collections::HashMap;
use std::env;
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use crate::config::FileType;

#[derive(Clone)]
pub enum Fs {
    Disk {
        root: PathBuf,
        file_type: FileType,
    },
    Bundle {
        files: Arc<HashMap<String, String>>,
        assets: Arc<HashMap<String, Vec<u8>>>,
        file_type: FileType,
    },
}

impl Fs {
    pub fn file_type(&self) -> FileType {
        match self {
            Fs::Disk { file_type, .. } | Fs::Bundle { file_type, .. } => *file_type,
        }
    }
}

pub fn normalize(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            Component::Normal(s) => out.push(s),
            Component::RootDir => out.push("/"),
            Component::Prefix(prefix) => out.push(prefix.as_os_str()),
        }
    }
    out
}

pub fn strip_anchors(name: &str) -> &str {
    let mut s = name;
    loop {
        if let Some(rest) = s.strip_prefix("./") {
            s = rest;
        } else if let Some(rest) = s.strip_prefix("../") {
            s = rest;
        } else if let Some(rest) = s.strip_prefix('/') {
            s = rest;
        } else {
            break;
        }
    }
    s
}

pub fn resolve(fs: &Fs, caller: &str, name: &str) -> Option<String> {
    match fs {
        Fs::Disk {
            file_type: FileType::Relative,
            ..
        } => disk_relative(caller, name),
        Fs::Disk {
            root,
            file_type: FileType::Global,
        } => disk_global(root, name),
        Fs::Bundle {
            files,
            file_type: FileType::Relative,
            ..
        } => bundle_relative(files, caller, name),
        Fs::Bundle {
            files,
            file_type: FileType::Global,
            ..
        } => bundle_global(files, name),
    }
}

pub fn read_module(fs: &Fs, key: &str) -> Option<String> {
    match fs {
        Fs::Disk { .. } => fs::read_to_string(key).ok(),
        Fs::Bundle { files, .. } => files.get(key).cloned(),
    }
}

pub fn caller_dir(fs: &Fs, owner: &str) -> PathBuf {
    match fs {
        Fs::Disk { .. } => Path::new(owner)
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_default(),
        Fs::Bundle { .. } => {
            let exe_dir = exe_dir();
            let v_parent = Path::new(owner).parent().unwrap_or(Path::new(""));
            exe_dir.join(v_parent)
        }
    }
}

pub fn fs_root(fs: &Fs) -> PathBuf {
    match fs {
        Fs::Disk { root, .. } => root.clone(),
        Fs::Bundle { .. } => exe_dir(),
    }
}

pub fn physical_path(fs: &Fs, owner: &str, path: &str) -> PathBuf {
    let p = Path::new(path);
    if p.is_absolute() {
        return p.to_path_buf();
    }
    let resolved = match fs.file_type() {
        FileType::Relative => caller_dir(fs, owner).join(path),
        FileType::Global => fs_root(fs).join(strip_anchors(path)),
    };
    normalize(&resolved)
}

fn exe_dir() -> PathBuf {
    env::current_exe()
        .ok()
        .and_then(|e| e.parent().map(Path::to_path_buf))
        .unwrap_or_default()
}

fn disk_relative(caller: &str, name: &str) -> Option<String> {
    let dir = Path::new(caller).parent()?;
    disk_lookup(&dir.join(name))
}

fn disk_global(root: &Path, name: &str) -> Option<String> {
    disk_lookup(&root.join(strip_anchors(name)))
}

fn disk_lookup(base: &Path) -> Option<String> {
    for ext in ["luau", "lua"] {
        let mut p = base.to_path_buf();
        p.set_extension(ext);
        if p.is_file() {
            return p.canonicalize().ok().map(path_to_string);
        }
    }
    for init in ["init.luau", "init.lua"] {
        let p = base.join(init);
        if p.is_file() {
            return p.canonicalize().ok().map(path_to_string);
        }
    }
    None
}

fn bundle_relative(files: &HashMap<String, String>, caller: &str, name: &str) -> Option<String> {
    let dir = Path::new(caller).parent().unwrap_or(Path::new(""));
    bundle_lookup(files, &dir.join(name))
}

fn bundle_global(files: &HashMap<String, String>, name: &str) -> Option<String> {
    bundle_lookup(files, Path::new(strip_anchors(name)))
}

fn bundle_lookup(files: &HashMap<String, String>, base: &Path) -> Option<String> {
    let normalized = normalize(base);
    let base_str = normalized.to_string_lossy().replace('\\', "/");
    for ext in ["luau", "lua"] {
        let key = if base_str.is_empty() {
            format!(".{ext}")
        } else {
            format!("{base_str}.{ext}")
        };
        if files.contains_key(&key) {
            return Some(key);
        }
    }
    for init in ["init.luau", "init.lua"] {
        let key = if base_str.is_empty() {
            init.to_string()
        } else {
            format!("{base_str}/{init}")
        };
        if files.contains_key(&key) {
            return Some(key);
        }
    }
    None
}

fn path_to_string(p: PathBuf) -> String {
    p.to_string_lossy().into_owned()
}
