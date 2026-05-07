

#![cfg_attr(windows, windows_subsystem = "windows")]

mod commands;
mod config;
mod console;
mod heart;
mod icon;
mod libs;
mod managed;
mod package;
mod runtime;
mod templates;
mod vfs;

use std::collections::HashMap;
use std::env;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;

use crate::vfs::{Fs, Package};

fn print_usage() {
    eprintln!("Ruzit — Luau runner & packager\n");
    eprintln!("Usage:");
    eprintln!("  Ruzit Init        [path]        scaffold a new project");
    eprintln!("  Ruzit InitPackage [path]        scaffold a Managed package folder (ManagedInfo.toml + init.luau)");
    eprintln!("  Ruzit Test        [path]        run a Luau file (default: Main.luau next to the exe / in CWD)");
    eprintln!("  Ruzit Build       [path] [-o]   produce Generated/<exe> + Generated/Managed/*.managed");
    eprintln!("  Ruzit Package     [folder] [-o] produce <id>.scripts.managed + <id>.assets.managed from a folder containing ManagedInfo.toml");
    eprintln!("\nGlobal flags:");
    eprintln!("  --console                       attach/allocate a console window (for windowed launchers)");
}

fn dispatch(args: &[String]) -> Result<(), String> {
    let cmd = args.get(1).map(String::as_str).unwrap_or("Test");
    match cmd {
        "Init" | "init" => commands::cmd_init(args.get(2)),
        "InitPackage" | "initpackage" | "Init-Package" | "init-package" => {
            commands::cmd_init_package(args.get(2))
        }
        "Test" | "test" => commands::cmd_test(args.get(2)),
        "Build" | "build" => {
            let (entry, output) = parse_path_and_output(&args[2..]);
            commands::cmd_build(entry.as_ref(), output.as_ref())
        }
        "Package" | "package" => {
            let (folder, output) = parse_path_and_output(&args[2..]);
            commands::cmd_package(folder.as_ref(), output.as_ref())
        }
        "-h" | "--help" | "help" => {
            print_usage();
            Ok(())
        }
        other => {
            print_usage();
            Err(format!("unknown subcommand: {other}"))
        }
    }
}

fn parse_path_and_output(args: &[String]) -> (Option<String>, Option<String>) {
    let mut path: Option<String> = None;
    let mut output: Option<String> = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "-o" | "--output" => {
                output = args.get(i + 1).cloned();
                i += 2;
            }
            _ => {
                path = Some(args[i].clone());
                i += 1;
            }
        }
    }
    (path, output)
}

fn run_launcher(info: package::LauncherInfo) -> Result<(), String> {
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
    let loaded = package::load_managed_dir(&managed_dir)?;
    if loaded.is_empty() {
        return Err(format!(
            "no .managed packages found in {}",
            managed_dir.display()
        ));
    }
    let default = loaded
        .get(&info.default_id)
        .ok_or_else(|| {
            format!(
                "default package '{}' not found in {} (loaded: {})",
                info.default_id,
                managed_dir.display(),
                loaded.keys().cloned().collect::<Vec<_>>().join(", ")
            )
        })?;

    println!(
        "[Ruzit] Launcher → {} v{} by {}  (default: {}, packages: {})",
        if info.name.is_empty() { &info.default_id } else { &info.name },
        if info.version.is_empty() { "?" } else { &info.version },
        if info.creator.is_empty() { "(no creator)" } else { &info.creator },
        info.default_id,
        loaded.len()
    );

    let default_entry = default.entry.clone();
    let default_file_type = default.file_type;

    let mut packages: HashMap<String, Arc<Package>> = HashMap::new();
    for (id, pkg) in loaded.into_iter() {
        packages.insert(
            id.clone(),
            Arc::new(Package {
                id: pkg.id,
                name: pkg.name,
                version: pkg.version,
                creator: pkg.creator,
                entry: pkg.entry,
                file_type: pkg.file_type,
                physical_root: pkg.physical_root,
                files: pkg.files,
                assets: pkg.assets,
            }),
        );
    }

    let fs_layer = Fs::Bundle {
        packages: Arc::new(packages),
        default_id: info.default_id.clone(),
        file_type: default_file_type,
        physical_root: exe_dir,
    };
    runtime::run_entry(fs_layer, &default_entry)
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

    match dispatch(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("[Ruzit] {e}");
            ExitCode::FAILURE
        }
    }
}

