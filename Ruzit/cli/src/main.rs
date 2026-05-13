mod commands;
mod templates;
mod update;

use std::env;
use std::process::ExitCode;

fn print_usage() {
    eprintln!("ruzit — Ruzit engine dev tool\n");
    eprintln!("Usage:");
    eprintln!("  ruzit init           [path]        scaffold a new project");
    eprintln!(
        "  ruzit initpackage    [path]        scaffold a Managed package folder (ManagedInfo.toml + init.luau)"
    );
    eprintln!(
        "  ruzit scaffold       [path]        regenerate .luaurc aliases from Packages/* manifests"
    );
    eprintln!(
        "  ruzit test           [path]        run a Luau file (default: Main.luau in the cwd)"
    );
    eprintln!(
        "  ruzit build          [path] [-o]   produce Generated/<exe> + Generated/Managed/*.managed"
    );
    eprintln!(
        "  ruzit package        [folder] [-o] produce <id>.scripts.managed + <id>.assets.managed from a folder containing ManagedInfo.toml"
    );
    eprintln!();
    eprintln!("Tooling:");
    eprintln!(
        "  ruzit fetch-deps     [path]        download steam_api into the given dir (default: cwd)"
    );
    eprintln!(
        "  ruzit refresh-types  [path]        re-download types.d.luau into the given dir (default: cwd)"
    );
    eprintln!(
        "  ruzit update         [path]        refresh the local ruzitrun binary from the configured release base (CLI itself is left alone)"
    );
}

fn dispatch(args: &[String]) -> Result<(), String> {
    let cmd = args.first().map(String::as_str).unwrap_or("help");
    match cmd {
        "Init" | "init" => commands::cmd_init(args.get(1)),
        "InitPackage" | "initpackage" | "Init-Package" | "init-package" => {
            commands::cmd_init_package(args.get(1))
        }
        "Scaffold" | "scaffold" => commands::cmd_scaffold(args.get(1)),
        "Test" | "test" => commands::cmd_test(args.get(1)),
        "Build" | "build" => {
            let (entry, output) = parse_path_and_output(&args[1..]);
            commands::cmd_build(entry.as_ref(), output.as_ref())
        }
        "Package" | "package" => {
            let (folder, output) = parse_path_and_output(&args[1..]);
            commands::cmd_package(folder.as_ref(), output.as_ref())
        }
        "FetchDeps" | "fetchdeps" | "fetch-deps" => update::cmd_fetch_deps(args.get(1)),
        "RefreshTypes" | "refreshtypes" | "refresh-types" => update::cmd_refresh_types(args.get(1)),
        "Update" | "update" | "SelfUpdate" | "selfupdate" | "self-update" => {
            update::cmd_update(args.get(1))
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

fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();
    match dispatch(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("[ruzit] {e}");
            ExitCode::FAILURE
        }
    }
}
