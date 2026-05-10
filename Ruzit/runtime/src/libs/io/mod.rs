mod handle;

use std::fs;
use std::io::Write;
use std::path::PathBuf;

use mlua::{Lua, Table};

use crate::vfs::{self, Fs};

pub use handle::IoHandle;

pub fn create(lua: &Lua, fs: Fs, owner: String) -> mlua::Result<Table> {
    let t = lua.create_table()?;

    install_read(lua, &t, &fs, &owner)?;
    install_write(lua, &t, &fs, &owner)?;
    install_append(lua, &t, &fs, &owner)?;
    install_exists(lua, &t, &fs, &owner)?;
    install_remove(lua, &t, &fs, &owner)?;
    install_mkdir(lua, &t, &fs, &owner)?;
    install_list(lua, &t, &fs, &owner)?;
    install_open(lua, &t, &fs, &owner)?;
    install_getpath(lua, &t, &fs, &owner)?;
    install_system_paths(&t)?;

    Ok(t)
}

fn path_to_string(p: Option<PathBuf>) -> Option<String> {
    p.map(|pb| pb.to_string_lossy().into_owned())
}

fn install_system_paths(t: &Table) -> mlua::Result<()> {
    t.set("Home", path_to_string(dirs::home_dir()))?;
    t.set("Documents", path_to_string(dirs::document_dir()))?;
    t.set("Desktop", path_to_string(dirs::desktop_dir()))?;
    t.set("Downloads", path_to_string(dirs::download_dir()))?;
    t.set("Pictures", path_to_string(dirs::picture_dir()))?;
    t.set("Videos", path_to_string(dirs::video_dir()))?;
    t.set("Music", path_to_string(dirs::audio_dir()))?;
    // AppData (roaming on Windows, ~/.config on Linux,
    // ~/Library/Application Support on macOS).
    t.set("AppData", path_to_string(dirs::config_dir()))?;
    // LocalAppData (~/.local/share on Linux, %LOCALAPPDATA% on Windows,
    // ~/Library/Application Support on macOS).
    t.set("LocalAppData", path_to_string(dirs::data_local_dir()))?;
    // Cache (~/.cache on Linux, %LOCALAPPDATA% on Windows,
    // ~/Library/Caches on macOS).
    t.set("Cache", path_to_string(dirs::cache_dir()))?;
    // Temp dir for the OS.
    t.set(
        "Temp",
        std::env::temp_dir().to_string_lossy().into_owned(),
    )?;
    // Current working directory snapshot.
    t.set(
        "WorkingDir",
        std::env::current_dir()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default(),
    )?;
    // Directory the running executable lives in.
    t.set(
        "ExecutableDir",
        std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(|d| d.to_string_lossy().into_owned()))
            .unwrap_or_default(),
    )?;
    // Path separator. "\\" on Windows, "/" elsewhere.
    t.set("Separator", std::path::MAIN_SEPARATOR.to_string())?;
    // Platform tag.
    let platform = if cfg!(target_os = "windows") {
        "windows"
    } else if cfg!(target_os = "linux") {
        "linux"
    } else if cfg!(target_os = "macos") {
        "macos"
    } else {
        "unknown"
    };
    t.set("Platform", platform)?;
    Ok(())
}

fn install_getpath(lua: &Lua, t: &Table, fs: &Fs, owner: &str) -> mlua::Result<()> {
    let fs = fs.clone();
    let owner = owner.to_string();
    t.set(
        "getpath",
        lua.create_function(move |_, path: String| -> mlua::Result<String> {
            Ok(vfs::physical_path(&fs, &owner, &path)
                .to_string_lossy()
                .into_owned())
        })?,
    )
}

fn err(action: &str, p: &PathBuf, e: impl std::fmt::Display) -> mlua::Error {
    mlua::Error::RuntimeError(format!("IO.{action} {}: {e}", p.display()))
}

fn install_read(lua: &Lua, t: &Table, fs: &Fs, owner: &str) -> mlua::Result<()> {
    let fs = fs.clone();
    let owner = owner.to_string();
    t.set(
        "read",
        lua.create_function(move |_, path: String| -> mlua::Result<String> {
            let p = vfs::physical_path(&fs, &owner, &path);
            fs::read_to_string(&p).map_err(|e| err("read", &p, e))
        })?,
    )
}

fn install_write(lua: &Lua, t: &Table, fs: &Fs, owner: &str) -> mlua::Result<()> {
    let fs = fs.clone();
    let owner = owner.to_string();
    t.set(
        "write",
        lua.create_function(
            move |_, (path, content): (String, String)| -> mlua::Result<()> {
                let p = vfs::physical_path(&fs, &owner, &path);
                if let Some(d) = p.parent() {
                    let _ = fs::create_dir_all(d);
                }
                fs::write(&p, content).map_err(|e| err("write", &p, e))
            },
        )?,
    )
}

fn install_append(lua: &Lua, t: &Table, fs: &Fs, owner: &str) -> mlua::Result<()> {
    let fs = fs.clone();
    let owner = owner.to_string();
    t.set(
        "append",
        lua.create_function(
            move |_, (path, content): (String, String)| -> mlua::Result<()> {
                let p = vfs::physical_path(&fs, &owner, &path);
                if let Some(d) = p.parent() {
                    let _ = fs::create_dir_all(d);
                }
                let mut f = fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(&p)
                    .map_err(|e| err("append", &p, e))?;
                f.write_all(content.as_bytes())
                    .map_err(|e| err("append", &p, e))
            },
        )?,
    )
}

fn install_exists(lua: &Lua, t: &Table, fs: &Fs, owner: &str) -> mlua::Result<()> {
    let fs = fs.clone();
    let owner = owner.to_string();
    t.set(
        "exists",
        lua.create_function(move |_, path: String| -> mlua::Result<bool> {
            let p = vfs::physical_path(&fs, &owner, &path);
            Ok(p.exists())
        })?,
    )
}

fn install_remove(lua: &Lua, t: &Table, fs: &Fs, owner: &str) -> mlua::Result<()> {
    let fs = fs.clone();
    let owner = owner.to_string();
    t.set(
        "remove",
        lua.create_function(move |_, path: String| -> mlua::Result<()> {
            let p = vfs::physical_path(&fs, &owner, &path);
            if p.is_dir() {
                fs::remove_dir_all(&p).map_err(|e| err("remove", &p, e))
            } else {
                fs::remove_file(&p).map_err(|e| err("remove", &p, e))
            }
        })?,
    )
}

fn install_mkdir(lua: &Lua, t: &Table, fs: &Fs, owner: &str) -> mlua::Result<()> {
    let fs = fs.clone();
    let owner = owner.to_string();
    t.set(
        "mkdir",
        lua.create_function(move |_, path: String| -> mlua::Result<()> {
            let p = vfs::physical_path(&fs, &owner, &path);
            fs::create_dir_all(&p).map_err(|e| err("mkdir", &p, e))
        })?,
    )
}

fn install_list(lua: &Lua, t: &Table, fs: &Fs, owner: &str) -> mlua::Result<()> {
    let fs = fs.clone();
    let owner = owner.to_string();
    t.set(
        "list",
        lua.create_function(move |lua, path: String| -> mlua::Result<Table> {
            let p = vfs::physical_path(&fs, &owner, &path);
            let entries = fs::read_dir(&p).map_err(|e| err("list", &p, e))?;
            let arr = lua.create_table()?;
            let mut i = 1;
            for entry in entries {
                let entry = entry.map_err(|e| err("list", &p, e))?;
                arr.set(i, entry.file_name().to_string_lossy().into_owned())?;
                i += 1;
            }
            Ok(arr)
        })?,
    )
}

fn install_open(lua: &Lua, t: &Table, fs: &Fs, owner: &str) -> mlua::Result<()> {
    let fs = fs.clone();
    let owner = owner.to_string();
    t.set(
        "open",
        lua.create_function(
            move |_, (path, mode): (String, Option<String>)| -> mlua::Result<IoHandle> {
                let p = vfs::physical_path(&fs, &owner, &path);
                let mode = mode.unwrap_or_else(|| "r".to_string());
                let mut opts = fs::OpenOptions::new();
                match mode.as_str() {
                    "r" => {
                        opts.read(true);
                    }
                    "w" => {
                        opts.write(true).create(true).truncate(true);
                    }
                    "a" => {
                        opts.append(true).create(true);
                    }
                    "r+" => {
                        opts.read(true).write(true);
                    }
                    "w+" => {
                        opts.read(true).write(true).create(true).truncate(true);
                    }
                    "a+" => {
                        opts.read(true).append(true).create(true);
                    }
                    other => {
                        return Err(mlua::Error::RuntimeError(format!(
                            "IO.open: unsupported mode '{other}'"
                        )));
                    }
                }
                let f = opts.open(&p).map_err(|e| err("open", &p, e))?;
                Ok(IoHandle {
                    file: Some(f),
                    path: p,
                })
            },
        )?,
    )
}
