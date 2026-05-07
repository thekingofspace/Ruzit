use std::collections::HashMap;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use base64::{Engine, engine::general_purpose::STANDARD as B64};

use crate::config::{BuildConfig, ManagedInfo};
use crate::package;
use crate::runtime;
use crate::templates;
use crate::vfs::{Fs, Package};

pub fn cmd_test(arg: Option<&String>) -> Result<(), String> {
    let entry = resolve_entry_arg(arg)?;
    let root = entry
        .parent()
        .ok_or("entry has no parent directory")?
        .to_path_buf();
    let config = BuildConfig::load(&root)?;
    apply_steam_app_id(&config);

    println!("[Ruzit] Test → {}", entry.display());
    config.print_banner();

    let dlc_folders = package::find_dlc_folders(&root)?;
    if !dlc_folders.is_empty() {
        println!(
            "[Ruzit] discovered {} DLC folder(s): {}",
            dlc_folders.len(),
            dlc_folders
                .iter()
                .filter_map(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
                .collect::<Vec<_>>()
                .join(", ")
        );
    }

    let entry_rel = entry
        .strip_prefix(&root)
        .map_err(|e| e.to_string())?
        .to_string_lossy()
        .replace('\\', "/");

    let exe_stem = config
        .exe_name
        .clone()
        .unwrap_or_else(|| entry.file_stem().unwrap_or_default().to_string_lossy().into_owned());
    let default_id = exe_stem.clone();

    let mut packages: HashMap<String, Arc<Package>> = HashMap::new();
    let (def_files, def_assets) = package::collect_project(&root)?;
    packages.insert(
        default_id.clone(),
        Arc::new(Package {
            id: default_id.clone(),
            name: if config.name.is_empty() { default_id.clone() } else { config.name.clone() },
            version: config.version.clone(),
            creator: config.creator.clone(),
            entry: entry_rel.clone(),
            file_type: config.file_type,
            physical_root: Some(root.clone()),
            files: def_files,
            assets: encode_assets_b64(def_assets),
            scripts_compressed: false,
            assets_compressed: false,
        }),
    );

    for dlc in &dlc_folders {
        let info = ManagedInfo::load(dlc)?;
        let (dlc_files, dlc_assets) = package::collect_project(dlc)?;
        if !dlc_files.contains_key(&info.entry) {
            return Err(format!(
                "DLC '{}' is missing entry '{}'",
                info.id, info.entry
            ));
        }
        packages.insert(
            info.id.clone(),
            Arc::new(Package {
                id: info.id.clone(),
                name: info.name.clone(),
                version: info.version.clone(),
                creator: info.creator.clone(),
                entry: info.entry.clone(),
                file_type: info.file_type,
                physical_root: Some(dlc.clone()),
                files: dlc_files,
                assets: encode_assets_b64(dlc_assets),
                scripts_compressed: false,
                assets_compressed: false,
            }),
        );
    }

    // Pull in pre-built .managed files from Packages/ (third-party packages,
    // mods, etc.) and merge them into the runtime alongside the project's
    // own bundle. Each becomes reachable via @PkgId/... from Lua. Same loader
    // as the launcher uses at ship time, so behavior matches `Ruzit Build`.
    let packages_dir = root.join(package::PACKAGES_DIR_NAME);
    if packages_dir.is_dir() {
        let external = package::load_managed_dir(&packages_dir)?;
        if !external.is_empty() {
            println!(
                "[Ruzit] imported {} external package(s) from {}/: {}",
                external.len(),
                package::PACKAGES_DIR_NAME,
                external.keys().cloned().collect::<Vec<_>>().join(", ")
            );
        }
        for (id, lp) in external {
            if packages.contains_key(&id) {
                eprintln!(
                    "[Ruzit] warn: external package id '{id}' clashes with the project or a DLC — skipping"
                );
                continue;
            }
            packages.insert(
                id,
                Arc::new(Package {
                    id: lp.id,
                    name: lp.name,
                    version: lp.version,
                    creator: lp.creator,
                    entry: lp.entry,
                    file_type: lp.file_type,
                    physical_root: lp.physical_root,
                    files: lp.files,
                    assets: lp.assets,
                    scripts_compressed: lp.scripts_compressed,
                    assets_compressed: lp.assets_compressed,
                }),
            );
        }
    }

    let fs_layer = Fs::Bundle {
        packages: Arc::new(packages),
        default_id,
        file_type: config.file_type,
        physical_root: root,
    };
    runtime::run_entry(fs_layer, &entry_rel)
}

fn encode_assets_b64(raw: HashMap<String, Vec<u8>>) -> HashMap<String, String> {
    raw.into_iter().map(|(k, v)| (k, B64.encode(v))).collect()
}

fn apply_steam_app_id(config: &BuildConfig) {
    if env::var_os("RUZIT_STEAM_APPID").is_some() {
        return;
    }
    if let Some(id) = config.steam_app_id {
        unsafe {
            env::set_var("RUZIT_STEAM_APPID", id.to_string());
        }
    }
}

/// Copy the Steamworks redistributable DLL into the build's Generated/ folder
/// so the launcher exe can load it at runtime. The DLL itself is dropped next
/// to the current Ruzit.exe by build.rs (it's pulled out of the
/// steamworks-sys cargo build cache). Silent best-effort — if it isn't there
/// we don't fail the build, since the user may not need Steam at runtime.
fn copy_steam_redist(generated_dir: &Path) {
    let Ok(self_exe) = env::current_exe() else { return };
    let Some(self_dir) = self_exe.parent() else { return };
    // Per-OS Steamworks redistributable filenames. The dynamic loader picks
    // these up at runtime via DELAYLOAD on Windows or rpath ($ORIGIN /
    // @executable_path) on Linux/macOS, so the file just has to land next to
    // the launcher.
    let names: &[&str] = if cfg!(target_os = "windows") {
        &["steam_api64.dll", "steam_api.dll"]
    } else if cfg!(target_os = "linux") {
        &["libsteam_api.so"]
    } else if cfg!(target_os = "macos") {
        &["libsteam_api.dylib"]
    } else {
        &[]
    };
    for name in names {
        let src = self_dir.join(name);
        if src.is_file() {
            let dst = generated_dir.join(name);
            match fs::copy(&src, &dst) {
                Ok(_) => println!("        {}  (Steam redist)", name),
                Err(e) => eprintln!("[Ruzit] warn: copy {}: {e}", src.display()),
            }
        }
    }
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

    let default_stem = entry
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "Main".to_string());
    
    let exe_stem = config.exe_name.clone().unwrap_or_else(|| default_stem.clone());
    let pkg_id = exe_stem.clone();

    let generated_dir = match output {
        Some(o) => PathBuf::from(o),
        None => package::default_generated_dir()?,
    };
    if generated_dir.is_dir() {
        fs::remove_dir_all(&generated_dir).map_err(|e| {
            format!("clear {}: {e}", generated_dir.display())
        })?;
    }
    let managed_dir = generated_dir.join("Managed");
    fs::create_dir_all(&managed_dir)
        .map_err(|e| format!("mkdir {}: {e}", managed_dir.display()))?;

    let icon_bytes = match &config.exe_icon {
        Some(name) => Some(load_icon(root, name)?),
        None => None,
    };

    // Launcher filename matches host OS conventions: Windows ships `.exe`,
    // Linux ships a bare ELF, macOS ships a bare Mach-O. Cross-OS launcher
    // builds aren't supported — `Ruzit Build` always copies *this* Ruzit
    // binary, so the launcher inherits whatever platform Ruzit was built for.
    let exe_filename = if cfg!(target_os = "windows") {
        format!("{exe_stem}.exe")
    } else {
        exe_stem.clone()
    };
    let exe_path = generated_dir.join(&exe_filename);
    package::write_launcher_exe(
        &exe_path,
        &package::LauncherInfo {
            default_id: pkg_id.clone(),
            name: config.name.clone(),
            version: config.version.clone(),
            creator: config.creator.clone(),
            steam_app_id: config.steam_app_id,
        },
        icon_bytes.as_deref(),
        config.exe_windowed,
    )?;

    // ELF/Mach-O launchers need the +x bit before they can be executed.
    // Windows uses file extensions instead, so the chmod is unix-only.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(meta) = fs::metadata(&exe_path) {
            let mut perms = meta.permissions();
            perms.set_mode(0o755);
            let _ = fs::set_permissions(&exe_path, perms);
        }
    }

    copy_steam_redist(&generated_dir);

    let scripts_path = managed_dir.join(format!("{pkg_id}.scripts.managed"));
    package::write_scripts_managed(
        &scripts_path,
        &pkg_id,
        &config.name,
        &config.version,
        &config.creator,
        &entry_rel,
        config.file_type,
        &files,
        config.compress_scripts,
    )?;

    let assets_path = managed_dir.join(format!("{pkg_id}.assets.managed"));
    if !assets.is_empty() {
        if config.shard_assets {
            // Sharded write — produces N shardNNNN.managed files plus a
            // <id>.assets.manifest.managed listing them. Patch updates only
            // re-download the touched shards instead of the whole archive.
            // No monolithic <id>.assets.managed is emitted in this mode.
            let n = package::write_assets_sharded(
                &managed_dir,
                &pkg_id,
                &config.name,
                &assets,
                config.compress_assets,
            )?;
            if assets_path.exists() {
                let _ = fs::remove_file(&assets_path);
            }
            println!(
                "        Managed/{}.assets.shardNNNN.managed  ({} shards)",
                pkg_id, n
            );
        } else {
            package::write_assets_managed(
                &assets_path, &pkg_id, &config.name, &assets, config.compress_assets,
            )?;
        }
    } else if assets_path.exists() {
        let _ = fs::remove_file(&assets_path);
    }

    config.print_banner();
    let scripts_size = fs::metadata(&scripts_path).map(|m| m.len()).unwrap_or(0);
    println!(
        "[Ruzit] Build → {}",
        generated_dir.display()
    );
    println!("        {}  (launcher)", exe_filename);
    println!(
        "        Managed/{}.scripts.managed  ({} files, {} KB)",
        pkg_id,
        files.len(),
        scripts_size / 1024
    );
    if !assets.is_empty() && !config.shard_assets {
        let assets_size = fs::metadata(&assets_path).map(|m| m.len()).unwrap_or(0);
        println!(
            "        Managed/{}.assets.managed  ({} assets, {} KB)",
            pkg_id,
            assets.len(),
            assets_size / 1024
        );
    }

    let dlc_folders = package::find_dlc_folders(root)?;
    for dlc in &dlc_folders {
        match build_dlc(
            &managed_dir,
            dlc,
            config.compress_scripts,
            config.compress_assets,
            config.shard_assets,
        ) {
            Ok((id, n_files, n_assets)) => {
                println!(
                    "        Managed/{}.scripts.managed  (DLC, {} files{})",
                    id,
                    n_files,
                    if n_assets > 0 {
                        format!(" + {n_assets} assets")
                    } else {
                        String::new()
                    }
                );
            }
            Err(e) => eprintln!(
                "[Ruzit] failed to package DLC at {}: {e}",
                dlc.display()
            ),
        }
    }

    // Copy any pre-built .managed files from Packages/ straight into
    // Generated/Managed/. They're already encrypted in the on-disk format the
    // launcher expects, so nothing to repack — bytes go through verbatim.
    // Skips files whose target name is already present (so a third-party
    // package can't clobber the project's own scripts/assets bundle).
    copy_external_packages(root, &managed_dir)?;

    Ok(())
}

fn copy_external_packages(root: &Path, managed_dir: &Path) -> Result<(), String> {
    let src_dir = root.join(package::PACKAGES_DIR_NAME);
    if !src_dir.is_dir() {
        return Ok(());
    }
    for entry in fs::read_dir(&src_dir)
        .map_err(|e| format!("read_dir {}: {e}", src_dir.display()))?
    {
        let entry = entry.map_err(|e| e.to_string())?;
        let src = entry.path();
        if !src.is_file() {
            continue;
        }
        if src.extension().and_then(|e| e.to_str()) != Some("managed") {
            continue;
        }
        let Some(name) = src.file_name() else {
            continue;
        };
        let dst = managed_dir.join(name);
        if dst.exists() {
            eprintln!(
                "[Ruzit] warn: '{}' from {}/ overlaps a project Managed file — skipping",
                name.to_string_lossy(),
                package::PACKAGES_DIR_NAME
            );
            continue;
        }
        let bytes = fs::copy(&src, &dst)
            .map_err(|e| format!("copy {}: {e}", src.display()))?;
        println!(
            "        Managed/{}  (imported, {} KB)",
            name.to_string_lossy(),
            bytes / 1024
        );
    }
    Ok(())
}

fn build_dlc(
    managed_dir: &Path,
    folder: &Path,
    compress_scripts: bool,
    compress_assets: bool,
    shard_assets: bool,
) -> Result<(String, usize, usize), String> {
    let info = ManagedInfo::load(folder)?;
    let (files, assets) = package::collect_project(folder)?;
    if !files.contains_key(&info.entry) {
        return Err(format!(
            "DLC '{}' is missing entry '{}'",
            info.id, info.entry
        ));
    }
    let scripts_path = managed_dir.join(format!("{}.scripts.managed", info.id));
    package::write_scripts_managed(
        &scripts_path,
        &info.id,
        &info.name,
        &info.version,
        &info.creator,
        &info.entry,
        info.file_type,
        &files,
        compress_scripts,
    )?;
    let assets_path = managed_dir.join(format!("{}.assets.managed", info.id));
    if !assets.is_empty() {
        if shard_assets {
            package::write_assets_sharded(
                managed_dir,
                &info.id,
                &info.name,
                &assets,
                compress_assets,
            )?;
            if assets_path.exists() {
                let _ = fs::remove_file(&assets_path);
            }
        } else {
            package::write_assets_managed(
                &assets_path, &info.id, &info.name, &assets, compress_assets,
            )?;
        }
    } else if assets_path.exists() {
        let _ = fs::remove_file(&assets_path);
    }
    Ok((info.id, files.len(), assets.len()))
}

pub fn cmd_package(arg: Option<&String>, output: Option<&String>) -> Result<(), String> {
    
    let folder = match arg {
        Some(s) => PathBuf::from(s)
            .canonicalize()
            .map_err(|e| format!("bad folder '{s}': {e}"))?,
        None => env::current_dir().map_err(|e| format!("cwd: {e}"))?,
    };
    if !folder.is_dir() {
        return Err(format!("Package: '{}' is not a directory", folder.display()));
    }

    let info = ManagedInfo::load(&folder)?;
    let (files, assets) = package::collect_project(&folder)?;

    let entry_present = files.contains_key(&info.entry);
    if !entry_present {
        return Err(format!(
            "Package entry '{}' not found in {} (looking for '{}')",
            info.entry,
            folder.display(),
            info.entry
        ));
    }

    let out_dir = match output {
        Some(o) => PathBuf::from(o),
        None => folder.join("Generated").join("Managed"),
    };
    fs::create_dir_all(&out_dir)
        .map_err(|e| format!("mkdir {}: {e}", out_dir.display()))?;

    let scripts_path = out_dir.join(format!("{}.scripts.managed", info.id));
    package::write_scripts_managed(
        &scripts_path,
        &info.id,
        &info.name,
        &info.version,
        &info.creator,
        &info.entry,
        info.file_type,
        &files,
        false,
    )?;

    let assets_path = out_dir.join(format!("{}.assets.managed", info.id));
    if !assets.is_empty() {
        package::write_assets_managed(&assets_path, &info.id, &info.name, &assets, false)?;
    } else if assets_path.exists() {
        let _ = fs::remove_file(&assets_path);
    }

    let scripts_size = fs::metadata(&scripts_path).map(|m| m.len()).unwrap_or(0);
    println!(
        "[Ruzit] Package '{}' v{} by {} → {}",
        info.name,
        info.version,
        if info.creator.is_empty() { "(no creator)" } else { &info.creator },
        out_dir.display()
    );
    println!(
        "        {}.scripts.managed  ({} files, {} KB)",
        info.id,
        files.len(),
        scripts_size / 1024
    );
    if !assets.is_empty() {
        let assets_size = fs::metadata(&assets_path).map(|m| m.len()).unwrap_or(0);
        println!(
            "        {}.assets.managed  ({} assets, {} KB)",
            info.id,
            assets.len(),
            assets_size / 1024
        );
    }
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
    
    if let Ok(exe) = env::current_exe() {
        if let Some(dir) = exe.parent() {
            let here = dir.join("Main.luau");
            if here.is_file() {
                return here.canonicalize().ok();
            }
        }
    }

    if let Ok(cwd) = env::current_dir() {
        let here = cwd.join("Main.luau");
        if here.is_file() {
            return here.canonicalize().ok();
        }
    }

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

    // Anything under `assets/` is auto-bundled by `Ruzit Build` and
    // reachable via Asset.GetAsset, so we make sure the top-level folder
    // exists. We deliberately don't pre-make subfolders (images/sounds/...)
    // so the project's organization is up to you — group by feature
    // (`SoundsGuns/`, `ItemsTier1/`) or by type, whichever fits.
    let assets_dir = target.join("assets");
    // `Packages/` is for dropping in pre-built .managed files (third-party
    // packages, mods you've published, etc.). `Ruzit Test` loads them
    // alongside the project, and `Ruzit Build` copies them into the final
    // Generated/Managed/ output.
    let packages_dir = target.join(package::PACKAGES_DIR_NAME);

    println!("[Ruzit] init → {} (name: {})", target.display(), project_name);

    let mut created = 0;
    let mut skipped = 0;
    for dir in [&assets_dir, &packages_dir] {
        if dir.exists() {
            skipped += 1;
        } else {
            fs::create_dir_all(dir)
                .map_err(|e| format!("mkdir {}: {e}", dir.display()))?;
            println!("  create {}/", display_rel(&target, dir));
            created += 1;
        }
    }
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

pub fn cmd_init_package(arg: Option<&String>) -> Result<(), String> {
    let target = match arg {
        Some(s) => PathBuf::from(s),
        None => env::current_dir().map_err(|e| format!("cwd: {e}"))?,
    };
    fs::create_dir_all(&target)
        .map_err(|e| format!("create {}: {e}", target.display()))?;

    let id = target
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("my-package")
        .to_string();
    
    let display_name = humanize(&id);

    let entries: &[(&str, &str)] = &[
        ("ManagedInfo.toml", templates::MANAGED_INFO_TOML),
        ("init.luau", templates::MANAGED_INIT_LUAU),
    ];

    println!(
        "[Ruzit] init package → {} (id: {}, name: {})",
        target.display(),
        id,
        display_name
    );

    let mut created = 0;
    let mut skipped = 0;
    for (rel, template) in entries {
        let path = target.join(rel);
        if path.exists() {
            println!("  skip   {}", display_rel(&target, &path));
            skipped += 1;
        } else {
            let content = template.replace("{id}", &id).replace("{name}", &display_name);
            fs::write(&path, content)
                .map_err(|e| format!("write {}: {e}", path.display()))?;
            println!("  create {}", display_rel(&target, &path));
            created += 1;
        }
    }

    println!(
        "[Ruzit] init package done: {created} created, {skipped} skipped."
    );
    println!(
        "        Drop this folder next to your project's build.toml — Build will auto-package it."
    );
    Ok(())
}

fn humanize(id: &str) -> String {
    let mut out = String::with_capacity(id.len());
    let mut start_of_word = true;
    for ch in id.chars() {
        if ch == '-' || ch == '_' {
            out.push(' ');
            start_of_word = true;
        } else if start_of_word {
            for u in ch.to_uppercase() {
                out.push(u);
            }
            start_of_word = false;
        } else {
            out.push(ch);
        }
    }
    out
}
