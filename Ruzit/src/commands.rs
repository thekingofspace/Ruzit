use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use crate::config::BuildConfig;
use crate::package;
use crate::runtime;
use crate::templates;
use crate::vfs::Fs;

pub fn cmd_test(arg: Option<&String>) -> Result<(), String> {
    let entry = resolve_entry_arg(arg)?;
    let root = entry
        .parent()
        .ok_or("entry has no parent directory")?
        .to_path_buf();
    let config = BuildConfig::load(&root)?;

    println!("[Ruzit] Test → {}", entry.display());
    config.print_banner();

    let fs_layer = Fs::Disk {
        root,
        file_type: config.file_type,
    };
    runtime::run_entry(fs_layer, &entry.to_string_lossy())
}

pub fn cmd_build(arg: Option<&String>, output: Option<&String>) -> Result<(), String> {
    let entry = resolve_entry_arg(arg)?;
    let root = entry.parent().ok_or("entry has no parent directory")?;
    let entry_rel = entry
        .strip_prefix(root)
        .map_err(|e| e.to_string())?
        .to_string_lossy()
        .replace('\\', "/");

    let config = BuildConfig::load(root)?;

    let (files, assets) = package::collect_project(root)?;
    if !files.contains_key(&entry_rel) {
        return Err(format!(
            "entry {entry_rel} not found while collecting sources"
        ));
    }

    // Pick the output stem: explicit override first, then [exe].name, then entry filename.
    let default_stem = entry
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "Main".to_string());
    let stem = config.exe_name.clone().unwrap_or(default_stem);

    let out_path = match output {
        Some(o) => PathBuf::from(o),
        None => package::default_output_path(&stem)?,
    };

    // Find icon bytes if [exe].icon is set.
    let icon_bytes = match &config.exe_icon {
        Some(name) => Some(load_icon(root, name)?),
        None => None,
    };

    let payload = package::write_packaged_exe(
        &files,
        &assets,
        &entry_rel,
        &config,
        icon_bytes.as_deref(),
        &out_path,
    )?;
    config.print_banner();
    let asset_bytes: usize = assets.values().map(|v| v.len()).sum();
    println!(
        "[Ruzit] Build → {} ({} sources, {} assets ({} KB), {} B payload, require mode: {}{})",
        out_path.display(),
        files.len(),
        assets.len(),
        asset_bytes / 1024,
        payload,
        config.file_type.as_str(),
        if icon_bytes.is_some() {
            ", icon embedded"
        } else {
            ""
        }
    );
    Ok(())
}

fn load_icon(root: &Path, name: &str) -> Result<Vec<u8>, String> {
    let candidates = if name.to_lowercase().ends_with(".ico") {
        vec![root.join(name)]
    } else {
        vec![root.join(format!("{name}.ico")), root.join(name)]
    };
    for path in &candidates {
        if path.is_file() {
            return std::fs::read(path)
                .map_err(|e| format!("read icon {}: {e}", path.display()));
        }
    }
    Err(format!(
        "icon '{name}' not found in {} (looked for {})",
        root.display(),
        candidates
            .iter()
            .map(|p| p.file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default())
            .collect::<Vec<_>>()
            .join(", ")
    ))
}

fn resolve_entry_arg(arg: Option<&String>) -> Result<PathBuf, String> {
    match arg {
        Some(s) => {
            let p = PathBuf::from(s);
            let target = if p.is_dir() { p.join("Main.luau") } else { p };
            target
                .canonicalize()
                .map_err(|e| format!("bad entry path '{}': {e}", target.display()))
        }
        None => find_default_main()
            .ok_or_else(|| "no entry given and no Main.luau found near the exe or in CWD".to_string()),
    }
}

fn find_default_main() -> Option<PathBuf> {
    // 1. Next to the exe (drop Ruzit.exe into a project folder)
    if let Ok(exe) = env::current_exe() {
        if let Some(dir) = exe.parent() {
            let here = dir.join("Main.luau");
            if here.is_file() {
                return here.canonicalize().ok();
            }
        }
    }

    // 2. Current working directory
    if let Ok(cwd) = env::current_dir() {
        let here = cwd.join("Main.luau");
        if here.is_file() {
            return here.canonicalize().ok();
        }
    }

    // 3. Walk up from CWD and exe_dir, accepting either ./Main.luau or ./test/Main.luau
    let mut starts: Vec<PathBuf> = Vec::new();
    if let Ok(cwd) = env::current_dir() {
        starts.push(cwd);
    }
    if let Ok(exe) = env::current_exe() {
        if let Some(p) = exe.parent() {
            starts.push(p.to_path_buf());
        }
    }
    for start in starts {
        let mut dir = start;
        loop {
            for candidate in [dir.join("Main.luau"), dir.join("test").join("Main.luau")] {
                if candidate.is_file() {
                    return candidate.canonicalize().ok();
                }
            }
            if !dir.pop() {
                break;
            }
        }
    }
    None
}

pub fn cmd_init(arg: Option<&String>) -> Result<(), String> {
    let target = match arg {
        Some(s) => PathBuf::from(s),
        None => env::current_dir().map_err(|e| format!("cwd: {e}"))?,
    };
    fs::create_dir_all(&target)
        .map_err(|e| format!("create {}: {e}", target.display()))?;

    let project_name = target
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("MyProject")
        .to_string();

    let entries: &[(&str, &str)] = &[
        ("build.toml", templates::BUILD_TOML),
        ("Main.luau", templates::MAIN_LUAU),
        ("types.d.luau", templates::TYPES_DLUAU),
        (".vscode/settings.json", templates::VSCODE_SETTINGS),
    ];

    println!("[Ruzit] init → {} (name: {})", target.display(), project_name);

    let mut created = 0;
    let mut skipped = 0;
    for (rel, template) in entries {
        let path = target.join(rel);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| format!("mkdir {}: {e}", parent.display()))?;
        }
        if path.exists() {
            println!("  skip   {}", display_rel(&target, &path));
            skipped += 1;
        } else {
            let content = template.replace("{name}", &project_name);
            fs::write(&path, content)
                .map_err(|e| format!("write {}: {e}", path.display()))?;
            println!("  create {}", display_rel(&target, &path));
            created += 1;
        }
    }

    println!(
        "[Ruzit] init done: {created} created, {skipped} skipped. Run `Ruzit Test {}` to try it.",
        target.display()
    );
    Ok(())
}

fn display_rel(base: &Path, p: &Path) -> String {
    p.strip_prefix(base)
        .unwrap_or(p)
        .to_string_lossy()
        .replace('\\', "/")
}
