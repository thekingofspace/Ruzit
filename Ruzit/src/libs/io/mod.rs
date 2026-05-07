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

    Ok(t)
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
