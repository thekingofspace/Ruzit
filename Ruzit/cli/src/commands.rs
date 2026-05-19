use std::collections::HashMap;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use ruzit_core::config::{BuildConfig, ManagedInfo};
use ruzit_core::package;

use crate::templates;

fn prepare_script_bytes(
    sources: &HashMap<String, String>,
    compile_bytecode: bool,
) -> Result<HashMap<String, Vec<u8>>, String> {
    let mut out = HashMap::with_capacity(sources.len());
    if !compile_bytecode {
        for (k, v) in sources {
            out.insert(k.clone(), v.as_bytes().to_vec());
        }
        return Ok(out);
    }
    let compiler = mlua::Compiler::new()
        .set_optimization_level(2)
        .set_debug_level(1);
    for (k, v) in sources {
        if k.ends_with(".luau") || k.ends_with(".lua") {
            let bytecode = compiler
                .compile(v.as_bytes())
                .map_err(|e| format!("compile {k}: {e}"))?;
            out.insert(k.clone(), bytecode);
        } else {
            out.insert(k.clone(), v.as_bytes().to_vec());
        }
    }
    Ok(out)
}

pub fn cmd_test(arg: Option<&String>) -> Result<(), String> {
    let entry = resolve_entry_arg(arg)?;
    let root = entry
        .parent()
        .ok_or("entry has no parent directory")?
        .to_path_buf();

    let entry_rel = entry
        .strip_prefix(&root)
        .map_err(|e| e.to_string())?
        .to_string_lossy()
        .replace('\\', "/");

    let runtime_bin = locate_ruzitrun()?;
    let status = std::process::Command::new(&runtime_bin)
        .arg("--run")
        .arg(&root)
        .arg(&entry_rel)
        .status()
        .map_err(|e| format!("failed to spawn {}: {e}", runtime_bin.display()))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!(
            "{} exited with {}",
            runtime_bin.display(),
            status.code().unwrap_or(-1)
        ))
    }
}

fn locate_ruzitrun() -> Result<PathBuf, String> {
    let cli_exe = env::current_exe().map_err(|e| e.to_string())?;
    let dir = cli_exe
        .parent()
        .ok_or_else(|| format!("could not derive parent of {}", cli_exe.display()))?;
    let candidate = if cfg!(target_os = "windows") {
        dir.join("ruzitrun.exe")
    } else {
        dir.join("ruzitrun")
    };
    if candidate.is_file() {
        return Ok(candidate);
    }
    Err(format!(
        "could not find ruzitrun{} alongside {}. The runtime binary must be installed next to the CLI for `test` to spawn it.",
        if cfg!(target_os = "windows") {
            ".exe"
        } else {
            ""
        },
        cli_exe.display()
    ))
}

fn copy_bin_folder(src_root: &Path, generated_dir: &Path, label: &str) {
    let src = src_root.join("bin");
    if !src.is_dir() {
        return;
    }
    let dst = generated_dir.join("bin");
    match copy_dir_recursive(&src, &dst) {
        Ok(n) => {
            if n > 0 {
                println!("        bin/  ({} files{})", n, label_suffix(label));
            }
        }
        Err(e) => eprintln!("[Ruzit] warn: copy bin/ from {}: {e}", src.display()),
    }
}

fn copy_include_paths(src_root: &Path, generated_dir: &Path, paths: &[String], label: &str) {
    for raw in paths {
        let trimmed = raw.trim().trim_start_matches("./").trim_start_matches(".\\");
        if trimmed.is_empty()
            || trimmed.contains("..")
            || trimmed.starts_with('/')
            || trimmed.starts_with('\\')
        {
            eprintln!("[Ruzit] warn: skipping unsafe include path '{raw}'");
            continue;
        }
        let src = src_root.join(trimmed);
        if !src.exists() {
            eprintln!(
                "[Ruzit] warn: include '{raw}' not found at {}",
                src.display()
            );
            continue;
        }
        let rel = trimmed.trim_end_matches('/').trim_end_matches('\\');
        let dst = generated_dir.join(rel);
        if src.is_dir() {
            if let Some(parent) = dst.parent() {
                let _ = fs::create_dir_all(parent);
            }
            match copy_dir_recursive(&src, &dst) {
                Ok(n) => println!("        {}/  ({} files{})", rel, n, label_suffix(label)),
                Err(e) => eprintln!("[Ruzit] warn: copy {}: {e}", src.display()),
            }
        } else {
            if let Some(parent) = dst.parent() {
                let _ = fs::create_dir_all(parent);
            }
            match fs::copy(&src, &dst) {
                Ok(_) => println!("        {}{}", rel, label_suffix(label)),
                Err(e) => eprintln!("[Ruzit] warn: copy {}: {e}", src.display()),
            }
        }
    }
}

fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<usize, String> {
    fs::create_dir_all(dst).map_err(|e| format!("mkdir {}: {e}", dst.display()))?;
    let mut count = 0;
    for entry in fs::read_dir(src).map_err(|e| format!("read_dir {}: {e}", src.display()))? {
        let entry = entry.map_err(|e| e.to_string())?;
        let path = entry.path();
        let name = match path.file_name() {
            Some(n) => n.to_os_string(),
            None => continue,
        };
        let target = dst.join(&name);
        if path.is_dir() {
            count += copy_dir_recursive(&path, &target)?;
        } else if path.is_file() {
            fs::copy(&path, &target)
                .map_err(|e| format!("copy {} -> {}: {e}", path.display(), target.display()))?;
            count += 1;
        }
    }
    Ok(count)
}

fn label_suffix(label: &str) -> String {
    if label.is_empty() {
        String::new()
    } else {
        format!("  ({label})")
    }
}

fn copy_steam_redist(generated_dir: &Path) {
    let Ok(self_exe) = env::current_exe() else {
        return;
    };
    let Some(self_dir) = self_exe.parent() else {
        return;
    };

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

    let exe_stem = config
        .exe_name
        .clone()
        .unwrap_or_else(|| default_stem.clone());
    let pkg_id = exe_stem.clone();

    let generated_dir = match output {
        Some(o) => PathBuf::from(o),
        None => package::default_generated_dir()?,
    };
    if generated_dir.is_dir() {
        fs::remove_dir_all(&generated_dir)
            .map_err(|e| format!("clear {}: {e}", generated_dir.display()))?;
    }
    let managed_dir = generated_dir.join("Managed");
    fs::create_dir_all(&managed_dir)
        .map_err(|e| format!("mkdir {}: {e}", managed_dir.display()))?;

    let icon_bytes = match &config.exe_icon {
        Some(name) => Some(load_icon(root, name)?),
        None => None,
    };

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
    copy_bin_folder(root, &generated_dir, "");
    copy_include_paths(root, &generated_dir, &config.include, "");

    let scripts_path = managed_dir.join(format!("{pkg_id}.scripts.managed"));
    let script_bytes = prepare_script_bytes(&files, config.compile_bytecode)?;
    package::write_scripts_managed(
        &scripts_path,
        &pkg_id,
        &config.name,
        &config.version,
        &config.creator,
        &entry_rel,
        config.file_type,
        &script_bytes,
        config.compress_scripts,
        config.compile_bytecode,
    )?;

    let assets_path = managed_dir.join(format!("{pkg_id}.assets.managed"));
    if !assets.is_empty() {
        if config.shard_assets {
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
                &assets_path,
                &pkg_id,
                &config.name,
                &assets,
                config.compress_assets,
            )?;
        }
    } else if assets_path.exists() {
        let _ = fs::remove_file(&assets_path);
    }

    config.print_banner();
    let scripts_size = fs::metadata(&scripts_path).map(|m| m.len()).unwrap_or(0);
    println!("[Ruzit] Build → {}", generated_dir.display());
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
            &generated_dir,
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
            Err(e) => eprintln!("[Ruzit] failed to package DLC at {}: {e}", dlc.display()),
        }
    }

    copy_external_packages(root, &managed_dir, &generated_dir, &config)?;

    Ok(())
}

fn copy_external_packages(
    root: &Path,
    managed_dir: &Path,
    generated_dir: &Path,
    config: &BuildConfig,
) -> Result<(), String> {
    let src_dir = root.join(package::PACKAGES_DIR_NAME);
    if !src_dir.is_dir() {
        return Ok(());
    }
    for entry in
        fs::read_dir(&src_dir).map_err(|e| format!("read_dir {}: {e}", src_dir.display()))?
    {
        let entry = entry.map_err(|e| e.to_string())?;
        let src = entry.path();
        if src.is_dir() {
            if !src.join("ManagedInfo.toml").is_file() {
                continue;
            }
            match build_dlc(
                managed_dir,
                generated_dir,
                &src,
                config.compress_scripts,
                config.compress_assets,
                config.shard_assets,
            ) {
                Ok((id, n_files, n_assets)) => {
                    println!(
                        "        Managed/{}.scripts.managed  (package, {} files{})",
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
                    "[Ruzit] failed to build folder package at {}: {e}",
                    src.display()
                ),
            }
            continue;
        }
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
        let bytes = fs::copy(&src, &dst).map_err(|e| format!("copy {}: {e}", src.display()))?;
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
    generated_dir: &Path,
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
    let label = format!("from {}", info.id);
    copy_bin_folder(folder, generated_dir, &label);
    copy_include_paths(folder, generated_dir, &info.include, &label);
    let scripts_path = managed_dir.join(format!("{}.scripts.managed", info.id));
    let script_bytes = prepare_script_bytes(&files, false)?;
    package::write_scripts_managed(
        &scripts_path,
        &info.id,
        &info.name,
        &info.version,
        &info.creator,
        &info.entry,
        info.file_type,
        &script_bytes,
        compress_scripts,
        false,
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
                &assets_path,
                &info.id,
                &info.name,
                &assets,
                compress_assets,
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
        return Err(format!(
            "Package: '{}' is not a directory",
            folder.display()
        ));
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
    fs::create_dir_all(&out_dir).map_err(|e| format!("mkdir {}: {e}", out_dir.display()))?;

    let scripts_path = out_dir.join(format!("{}.scripts.managed", info.id));
    let script_bytes = prepare_script_bytes(&files, false)?;
    package::write_scripts_managed(
        &scripts_path,
        &info.id,
        &info.name,
        &info.version,
        &info.creator,
        &info.entry,
        info.file_type,
        &script_bytes,
        false,
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
        if info.creator.is_empty() {
            "(no creator)"
        } else {
            &info.creator
        },
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
            return std::fs::read(path).map_err(|e| format!("read icon {}: {e}", path.display()));
        }
    }
    Err(format!(
        "icon '{name}' not found in {} (looked for {})",
        root.display(),
        candidates
            .iter()
            .map(|p| p
                .file_name()
                .map(|f| f.to_string_lossy().into_owned())
                .unwrap_or_default())
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
        None => find_default_main().ok_or_else(|| {
            "no entry given and no Main.luau found near the exe or in CWD".to_string()
        }),
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
    fs::create_dir_all(&target).map_err(|e| format!("create {}: {e}", target.display()))?;

    let project_name = target
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("MyProject")
        .to_string();

    let entries: &[(&str, &str)] = &[
        ("build.toml", templates::BUILD_TOML),
        ("Main.luau", templates::MAIN_LUAU),
        (".vscode/settings.json", templates::VSCODE_SETTINGS),
        (".luaurc", templates::LUAURC),
    ];

    let assets_dir = target.join("assets");
    let packages_dir = target.join(package::PACKAGES_DIR_NAME);
    let bin_dir = target.join("bin");

    println!(
        "[Ruzit] init → {} (name: {})",
        target.display(),
        project_name
    );

    let mut created = 0;
    let mut skipped = 0;
    for dir in [&assets_dir, &packages_dir, &bin_dir] {
        if dir.exists() {
            skipped += 1;
        } else {
            fs::create_dir_all(dir).map_err(|e| format!("mkdir {}: {e}", dir.display()))?;
            println!("  create {}/", display_rel(&target, dir));
            created += 1;
        }
    }
    for (rel, template) in entries {
        let path = target.join(rel);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|e| format!("mkdir {}: {e}", parent.display()))?;
        }
        let content = template.replace("{name}", &project_name);
        let already_existed = path.exists();
        if already_existed {
            println!("  skip   {}", display_rel(&target, &path));
            skipped += 1;
        } else {
            fs::write(&path, content).map_err(|e| format!("write {}: {e}", path.display()))?;
            println!("  create  {}", display_rel(&target, &path));
            created += 1;
        }
    }

    let types_path = target.join("types.d.luau");
    let types_existed = types_path.exists();
    match crate::update::fetch_types_dluau() {
        Ok(body) => {
            fs::write(&types_path, body)
                .map_err(|e| format!("write {}: {e}", types_path.display()))?;
            let verb = if types_existed { "rewrite" } else { "create " };
            println!(
                "  {verb} {}  (fetched from {})",
                display_rel(&target, &types_path),
                crate::update::types_dluau_url()
            );
            created += 1;
        }
        Err(e) => {
            if types_existed {
                println!(
                    "  skip   {}  (network: {e}; keeping existing copy)",
                    display_rel(&target, &types_path)
                );
                skipped += 1;
            } else {
                let stub = format!(
                    "-- types.d.luau\n\
                     -- Failed to fetch from {}\n\
                     --   {e}\n\
                     -- Run `ruzit refresh-types` once you're online or set RUZIT_TYPES_URL\n\
                     -- to a reachable mirror.\n",
                    crate::update::types_dluau_url()
                );
                fs::write(&types_path, stub)
                    .map_err(|e| format!("write {}: {e}", types_path.display()))?;
                println!(
                    "  stub   {}  (fetch failed: {e})",
                    display_rel(&target, &types_path)
                );
                created += 1;
            }
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
    fs::create_dir_all(&target).map_err(|e| format!("create {}: {e}", target.display()))?;

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

    let assets_dir = target.join("assets");
    let bin_dir = target.join("bin");

    println!(
        "[Ruzit] init package → {} (id: {}, name: {})",
        target.display(),
        id,
        display_name
    );

    let mut created = 0;
    let mut skipped = 0;
    for dir in [&assets_dir, &bin_dir] {
        if dir.exists() {
            skipped += 1;
        } else {
            fs::create_dir_all(dir).map_err(|e| format!("mkdir {}: {e}", dir.display()))?;
            println!("  create {}/", display_rel(&target, dir));
            created += 1;
        }
    }
    for (rel, template) in entries {
        let path = target.join(rel);
        if path.exists() {
            println!("  skip   {}", display_rel(&target, &path));
            skipped += 1;
        } else {
            let content = template
                .replace("{id}", &id)
                .replace("{name}", &display_name);
            fs::write(&path, content).map_err(|e| format!("write {}: {e}", path.display()))?;
            println!("  create {}", display_rel(&target, &path));
            created += 1;
        }
    }

    println!("[Ruzit] init package done: {created} created, {skipped} skipped.");
    println!(
        "        Drop this folder next to your project's build.toml — Build will auto-package it."
    );
    Ok(())
}

pub fn cmd_scaffold(arg: Option<&String>) -> Result<(), String> {
    let target = match arg {
        Some(s) => PathBuf::from(s),
        None => env::current_dir().map_err(|e| format!("cwd: {e}"))?,
    };
    if !target.is_dir() {
        return Err(format!("{} is not a directory", target.display()));
    }

    println!("[Ruzit] scaffold → {} (scanning recursively)", target.display());

    let mut aliases: Vec<(String, String)> = vec![("Game".into(), "./".into())];
    let mut found: Vec<(PathBuf, ManagedInfo)> = Vec::new();
    walk_for_manifests(&target, &target, &mut found);
    found.sort_by(|a, b| a.0.cmp(&b.0));

    for (folder, info) in found {
        let entry_path = folder.join(&info.entry);
        if !entry_path.is_file() {
            println!(
                "  skip   {} (entry '{}' not found)",
                display_rel(&target, &folder),
                info.entry
            );
            continue;
        }
        let rel = display_rel(&target, &folder);
        let alias_path = format!("./{rel}");
        println!("  alias  {} → {alias_path}", info.id);
        aliases.push((info.id, alias_path));
    }

    let luaurc_path = target.join(".luaurc");
    let mut body = String::from("{\n    \"languageMode\": \"strict\",\n    \"aliases\": {\n");
    for (i, (k, v)) in aliases.iter().enumerate() {
        let comma = if i + 1 == aliases.len() { "" } else { "," };
        body.push_str(&format!("        \"{k}\": \"{v}\"{comma}\n"));
    }
    body.push_str("    }\n}\n");
    fs::write(&luaurc_path, body)
        .map_err(|e| format!("write {}: {e}", luaurc_path.display()))?;
    println!(
        "[Ruzit] scaffold done: wrote {} ({} alias{})",
        display_rel(&target, &luaurc_path),
        aliases.len(),
        if aliases.len() == 1 { "" } else { "es" }
    );
    Ok(())
}

fn walk_for_manifests(
    root: &Path,
    dir: &Path,
    out: &mut Vec<(PathBuf, ManagedInfo)>,
) {
    let manifest = dir.join("ManagedInfo.toml");
    if manifest.is_file() {
        match ManagedInfo::load(dir) {
            Ok(info) => {
                out.push((dir.to_path_buf(), info));
                return;
            }
            Err(e) => {
                println!("  skip   {} ({e})", display_rel(root, dir));
            }
        }
    }

    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    for e in entries.flatten() {
        let path = e.path();
        if !path.is_dir() {
            continue;
        }
        if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
            if name.starts_with('.')
                || name == "Generated"
                || name == "target"
                || name == "node_modules"
            {
                continue;
            }
        }
        walk_for_manifests(root, &path, out);
    }
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
