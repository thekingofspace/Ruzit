use std::env;
use std::fs;
use std::path::PathBuf;

fn main() {
    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let target_dir = out_dir
        .ancestors()
        .find(|p| {
            p.file_name()
                .map(|n| n == "debug" || n == "release")
                .unwrap_or(false)
        })
        .map(|p| p.to_path_buf());

    let sys_out = locate_steamworks_sys_out(&out_dir);

    // Per-OS list of redistributable file names that steamworks-sys drops in
    // its OUT_DIR. On Windows we also ship the import library so /DELAYLOAD
    // resolves at link time; on Linux/macOS the dynamic loader handles it via
    // rpath at runtime.
    let candidates: &[&str] = match target_os.as_str() {
        "windows" => &[
            "steam_api64.dll",
            "steam_api64.lib",
            "steam_api.dll",
            "steam_api.lib",
        ],
        "linux" => &["libsteam_api.so"],
        "macos" => &["libsteam_api.dylib"],
        _ => &[],
    };

    if let Some(sys_out) = &sys_out {
        if let Some(target_dir) = &target_dir {
            for name in candidates {
                let src = sys_out.join(name);
                if src.is_file() {
                    let dst = target_dir.join(name);
                    let _ = fs::copy(&src, &dst);
                }
            }
        }
        // Also stage the runtime-loadable library inside OUT_DIR so include_bytes!
        // in main.rs can pick it up.
        let runtime_libs: &[&str] = match target_os.as_str() {
            "windows" => &["steam_api64.dll", "steam_api.dll"],
            "linux" => &["libsteam_api.so"],
            "macos" => &["libsteam_api.dylib"],
            _ => &[],
        };
        for name in runtime_libs {
            let src = sys_out.join(name);
            if src.is_file() {
                let dst = out_dir.join(name);
                let _ = fs::copy(&src, &dst);
            }
        }
    }

    // include_bytes! in main.rs requires the file to exist even if it's empty
    // (e.g. Steamworks isn't being linked, or we're cross-compiling without the
    // matching SDK). An empty stub makes EMBEDDED_STEAM_LIB.is_empty() true at
    // runtime and the Steam plumbing silently no-ops.
    let stub_libs: &[&str] = match target_os.as_str() {
        "windows" => &["steam_api64.dll", "steam_api.dll"],
        "linux" => &["libsteam_api.so"],
        "macos" => &["libsteam_api.dylib"],
        _ => &[],
    };
    for name in stub_libs {
        let f = out_dir.join(name);
        if !f.is_file() {
            let _ = fs::write(&f, &[]);
        }
    }

    // Linker plumbing so the Steamworks redistributable is found next to the
    // launcher executable instead of via a system-wide search path.
    //   * Windows/MSVC: DELAYLOAD postpones binding until first call, so a
    //     missing DLL never aborts startup; delayimp.lib provides the helper.
    //   * Linux: $ORIGIN rpath looks in the directory containing the binary.
    //   * macOS: @executable_path serves the same purpose.
    if cfg!(target_env = "msvc") {
        println!("cargo:rustc-link-arg-bin=Ruzit=/DELAYLOAD:steam_api64.dll");
        println!("cargo:rustc-link-arg-bin=Ruzit=delayimp.lib");
    }
    match target_os.as_str() {
        "linux" => {
            println!("cargo:rustc-link-arg=-Wl,-rpath,$ORIGIN");
        }
        "macos" => {
            println!("cargo:rustc-link-arg=-Wl,-rpath,@executable_path");
        }
        _ => {}
    }

    println!("cargo:rerun-if-changed=build.rs");
}

fn locate_steamworks_sys_out(out_dir: &PathBuf) -> Option<PathBuf> {
    let mut build_dir = out_dir.clone();
    build_dir.pop();
    build_dir.pop();
    let entries = fs::read_dir(&build_dir).ok()?;
    for entry in entries.flatten() {
        let p = entry.path();
        if p.file_name()
            .and_then(|n| n.to_str())
            .map(|n| n.starts_with("steamworks-sys-"))
            .unwrap_or(false)
        {
            let candidate = p.join("out");
            if candidate.is_dir() {
                return Some(candidate);
            }
        }
    }
    None
}
