#![cfg_attr(windows, windows_subsystem = "windows")]

use std::env;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;

use ruzit::config::{BuildConfig, ManagedInfo};
use ruzit::package::{self, LauncherInfo};
use ruzit::vfs::{Fs, LazyPackageRegistry, Package};
use ruzit::{console, runtime};

fn print_usage() {
    eprintln!("ruzitrun (runtime) — engine binary used as the launcher template");
    eprintln!("for built games. Normally not invoked directly.");
    eprintln!();
    eprintln!("For dev commands (init / test / build / package / fetch-deps");
    eprintln!("/ update), use the `ruzit` CLI tool from this workspace.");
    eprintln!();
    eprintln!("Direct invocations:");
    eprintln!("  --run <project-root> [<entry>]   run a project from disk");
    eprintln!("  --console                        attach/allocate a console");
}

fn parse_run_args(args: &[String]) -> Option<(PathBuf, Option<String>)> {
    let mut iter = args.iter();
    while let Some(a) = iter.next() {
        if a == "--run" {
            let path = iter.next().cloned()?;
            let entry = iter.next().cloned();
            return Some((PathBuf::from(path), entry));
        }
    }
    None
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

fn run_disk_project(root: PathBuf, entry: Option<String>) -> Result<(), String> {
    let entry_rel = entry.unwrap_or_else(|| "Main.luau".to_string());
    let config = BuildConfig::load(&root)?;
    apply_steam_app_id(&config);
    ruzit::libs::ffi::set_project_root(root.clone());

    println!("[Ruzit] Test → {}/{}", root.display(), entry_rel);
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

    let exe_stem = config.exe_name.clone().unwrap_or_else(|| {
        PathBuf::from(&entry_rel)
            .file_stem()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned()
    });
    let default_id = exe_stem.clone();

    let mut registry = LazyPackageRegistry::new();
    let (def_files, def_assets) = package::collect_project(&root)?;
    registry.insert_ready(
        default_id.clone(),
        Arc::new(Package {
            id: default_id.clone(),
            name: if config.name.is_empty() {
                default_id.clone()
            } else {
                config.name.clone()
            },
            version: config.version.clone(),
            creator: config.creator.clone(),
            entry: entry_rel.clone(),
            file_type: config.file_type,
            physical_root: Some(root.clone()),
            files: def_files,
            assets: package::encode_assets_b64(def_assets),
            scripts_compressed: false,
            assets_compressed: false,
            scripts_bytecode: false,
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
        registry.insert_ready(
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
                assets: package::encode_assets_b64(dlc_assets),
                scripts_compressed: false,
                assets_compressed: false,
                scripts_bytecode: false,
            }),
        );
    }

    let packages_dir = root.join(package::PACKAGES_DIR_NAME);
    if packages_dir.is_dir() {
        let groups = package::discover_managed_files(&packages_dir)?;
        if !groups.is_empty() {
            println!(
                "[Ruzit] queued {} external package(s) from {}/ for lazy load: {}",
                groups.len(),
                package::PACKAGES_DIR_NAME,
                groups.keys().cloned().collect::<Vec<_>>().join(", ")
            );
        }
        for (id, paths) in groups {
            if registry.contains_key(&id) {
                eprintln!(
                    "[Ruzit] warn: external package id '{id}' clashes with the project or a DLC — skipping"
                );
                continue;
            }
            registry.insert_pending(
                id,
                paths.into_iter().map(package::ManagedSource::Disk).collect(),
            );
        }
    }

    let registry = Arc::new(registry);
    ruzit::vfs::spawn_lazy_loaders(registry.clone(), num_cpus_or_default());
    let fs = Fs::Bundle {
        packages: registry,
        default_id,
        file_type: config.file_type,
        physical_root: root,
    };
    runtime::run_entry(fs, &entry_rel)
}

fn run_launcher(info: LauncherInfo) -> Result<(), String> {
    if env::var_os("RUZIT_STEAM_APPID").is_none() {
        if let Some(id) = info.steam_app_id {
            unsafe {
                env::set_var("RUZIT_STEAM_APPID", id.to_string());
            }
        }
    }
    let exe_dir = env::current_exe()
        .ok()
        .and_then(|e| e.parent().map(PathBuf::from))
        .ok_or("could not determine launcher dir")?;
    let managed_dir = exe_dir.join("Managed");
    if !managed_dir.is_dir() {
        return Err(format!(
            "Managed/ folder not found next to launcher (expected {})",
            managed_dir.display()
        ));
    }
    let groups = package::discover_managed_files(&managed_dir)?;
    if groups.is_empty() {
        return Err(format!(
            "no .managed packages found in {}",
            managed_dir.display()
        ));
    }
    let default_paths = groups.get(&info.default_id).ok_or_else(|| {
        format!(
            "default package '{}' not found in {} (discovered: {})",
            info.default_id,
            managed_dir.display(),
            groups.keys().cloned().collect::<Vec<_>>().join(", ")
        )
    })?;
    let default_sources: Vec<package::ManagedSource> = default_paths
        .iter()
        .cloned()
        .map(package::ManagedSource::Disk)
        .collect();
    let default_pkg = package::load_single_managed_package(&info.default_id, &default_sources)?;
    let default_entry = default_pkg.entry.clone();
    let default_file_type = default_pkg.file_type;

    println!(
        "[Ruzit] Launcher → {} v{} by {}  (default: {}, packages: {})",
        if info.name.is_empty() {
            &info.default_id
        } else {
            &info.name
        },
        if info.version.is_empty() {
            "?"
        } else {
            &info.version
        },
        if info.creator.is_empty() {
            "(no creator)"
        } else {
            &info.creator
        },
        info.default_id,
        groups.len()
    );

    let mut registry = LazyPackageRegistry::new();
    registry.insert_ready(
        info.default_id.clone(),
        Arc::new(Package::from_loaded(default_pkg)),
    );
    for (id, paths) in groups {
        if id == info.default_id {
            continue;
        }
        registry.insert_pending(
            id,
            paths.into_iter().map(package::ManagedSource::Disk).collect(),
        );
    }

    let registry = Arc::new(registry);
    ruzit::vfs::spawn_lazy_loaders(registry.clone(), num_cpus_or_default());
    let fs_layer = Fs::Bundle {
        packages: registry,
        default_id: info.default_id.clone(),
        file_type: default_file_type,
        physical_root: exe_dir,
    };
    runtime::run_entry(fs_layer, &default_entry)
}

fn num_cpus_or_default() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
        .max(2)
        .min(8)
}

fn main() -> ExitCode {
    let raw_args: Vec<String> = env::args().collect();
    let console_flag = raw_args.iter().any(|a| a == "--console");
    let args: Vec<String> = raw_args.into_iter().filter(|a| a != "--console").collect();

    let launcher = package::try_self_launcher();
    console::setup(console_flag);

    if let Some(info) = launcher {
        return match run_launcher(info) {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("[Ruzit] {e}");
                ExitCode::FAILURE
            }
        };
    }

    if let Some((path, entry)) = parse_run_args(&args[1..]) {
        return match run_disk_project(path, entry) {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("[Ruzit] {e}");
                ExitCode::FAILURE
            }
        };
    }

    print_usage();
    ExitCode::FAILURE
}
