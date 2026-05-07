use std::env;
use std::fs;
use std::path::PathBuf;

fn main() {
    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    let target_dir = out_dir
        .ancestors()
        .find(|p| {
            p.file_name()
                .map(|n| n == "debug" || n == "release")
                .unwrap_or(false)
        })
        .map(|p| p.to_path_buf());

    let sys_out = locate_steamworks_sys_out(&out_dir);

    if let Some(sys_out) = &sys_out {
        let candidates = [
            "steam_api64.dll",
            "steam_api64.lib",
            "steam_api.dll",
            "steam_api.lib",
        ];
        if let Some(target_dir) = &target_dir {
            for name in candidates {
                let src = sys_out.join(name);
                if src.is_file() {
                    let dst = target_dir.join(name);
                    let _ = fs::copy(&src, &dst);
                }
            }
        }
        for name in ["steam_api64.dll", "steam_api.dll"] {
            let src = sys_out.join(name);
            if src.is_file() {
                let dst = out_dir.join(name);
                let _ = fs::copy(&src, &dst);
            }
        }
    }

    for name in ["steam_api64.dll", "steam_api.dll"] {
        let f = out_dir.join(name);
        if !f.is_file() {
            let _ = fs::write(&f, &[]);
        }
    }

    if cfg!(target_env = "msvc") {
        println!("cargo:rustc-link-arg-bin=Ruzit=/DELAYLOAD:steam_api64.dll");
        println!("cargo:rustc-link-arg-bin=Ruzit=delayimp.lib");
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
