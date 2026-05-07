mod commands;
mod config;
mod console;
mod heart;
mod icon;
mod libs;
mod package;
mod runtime;
mod templates;
mod vfs;

use std::env;
use std::process::ExitCode;
use std::sync::Arc;

use crate::vfs::Fs;

fn print_usage() {
    eprintln!("Ruzit — Luau runner & packager\n");
    eprintln!("Usage:");
    eprintln!("  Ruzit Init  [path]            scaffold a new project (build.toml, Main.luau, types, .vscode)");
    eprintln!("  Ruzit Test  [path]            run a Luau file (default: Main.luau next to the exe / in CWD)");
    eprintln!("  Ruzit Build [path] [-o out]   package a Luau project into a standalone exe");
    eprintln!("\nGlobal flags:");
    eprintln!("  --console                     attach/allocate a console window (for windowed builds)");
    eprintln!("\nIf [path] is a directory, Main.luau inside it is used.");
    eprintln!("If a build.toml sits next to the entry, its [configs].\"File Type\" controls require resolution:");
    eprintln!("  Relative — ./foo is relative to the calling file (default)");
    eprintln!("  Global   — ./foo is always relative to the project root");
}

fn dispatch(args: &[String]) -> Result<(), String> {
    let cmd = args.get(1).map(String::as_str).unwrap_or("Test");
    match cmd {
        "Init" | "init" => commands::cmd_init(args.get(2)),
        "Test" | "test" => commands::cmd_test(args.get(2)),
        "Build" | "build" => {
            let mut entry: Option<String> = None;
            let mut output: Option<String> = None;
            let mut i = 2;
            while i < args.len() {
                match args[i].as_str() {
                    "-o" | "--output" => {
                        output = args.get(i + 1).cloned();
                        i += 2;
                    }
                    _ => {
                        entry = Some(args[i].clone());
                        i += 1;
                    }
                }
            }
            commands::cmd_build(entry.as_ref(), output.as_ref())
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

fn main() -> ExitCode {
    let raw_args: Vec<String> = env::args().collect();
    let console_flag = raw_args.iter().any(|a| a == "--console");
    let args: Vec<String> = raw_args.into_iter().filter(|a| a != "--console").collect();

    let bundled = package::try_self_bundle();
    // Dev-tool mode (no bundle) always wants console output;
    // packaged-game mode only attaches a console when --console is passed.
    let want_console = bundled.is_none() || console_flag;
    console::setup(want_console);

    if let Some(bundle) = bundled {
        bundle.config.print_banner();
        let fs_layer = Fs::Bundle {
            files: Arc::new(bundle.files),
            assets: Arc::new(bundle.assets),
            file_type: bundle.config.file_type,
        };
        return match runtime::run_entry(fs_layer, &bundle.entry) {
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
