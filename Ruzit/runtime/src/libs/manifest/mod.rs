use std::sync::Arc;

use mlua::{AnyUserData, Lua, Table, UserData, UserDataMethods};

use crate::vfs::{Fs, LazyPackageRegistry};

pub fn install(lua: &Lua, fs: &Fs) -> mlua::Result<()> {
    let manifest = lua.create_table()?;

    let fs_load = fs.clone();
    manifest.set(
        "LoadManaged",
        lua.create_function(move |lua, raw_id: String| -> mlua::Result<AnyUserData> {
            let id = raw_id.trim_start_matches('@').to_string();
            match &fs_load {
                Fs::Bundle {
                    packages,
                    default_id,
                    ..
                } => load_managed(lua, packages.clone(), default_id.clone(), id),
                _ => Err(mlua::Error::RuntimeError(
                    "manifest.LoadManaged: only available when running a built game".into(),
                )),
            }
        })?,
    )?;

    let fs_loaded = fs.clone();
    manifest.set(
        "IsLoaded",
        lua.create_function(move |_, raw_id: String| -> mlua::Result<bool> {
            let id = raw_id.trim_start_matches('@').to_string();
            match &fs_loaded {
                Fs::Bundle {
                    packages,
                    default_id,
                    ..
                } => {
                    if id == *default_id || id.eq_ignore_ascii_case("game") {
                        return Ok(true);
                    }
                    if packages.is_test_mode() {
                        return Ok(packages.contains_key(&id));
                    }
                    Ok(packages.get(&id).map(|p| p.is_activated()).unwrap_or(false))
                }
                _ => Ok(true),
            }
        })?,
    )?;

    let fs_list = fs.clone();
    manifest.set(
        "List",
        lua.create_function(move |lua, _: ()| -> mlua::Result<Table> {
            let arr = lua.create_table()?;
            if let Fs::Bundle { packages, .. } = &fs_list {
                let mut ids: Vec<&String> = packages.keys().collect();
                ids.sort();
                for (i, id) in ids.iter().enumerate() {
                    arr.set((i + 1) as i64, (*id).clone())?;
                }
            }
            Ok(arr)
        })?,
    )?;

    let fs_flag = fs.clone();
    manifest.set(
        "CallFlag",
        lua.create_function(move |lua, flag: String| -> mlua::Result<Table> {
            let result = lua.create_table()?;
            let activated = lua.create_table()?;
            let loaded = lua.create_table()?;
            let errors = lua.create_table()?;
            if let Fs::Bundle { packages, .. } = &fs_flag {
                let summary = packages.call_flag(&flag);
                for (i, id) in summary.packages_activated.iter().enumerate() {
                    activated.set((i + 1) as i64, id.clone())?;
                }
                for (i, (id, n)) in summary.shards_loaded.iter().enumerate() {
                    let row = lua.create_table()?;
                    row.set("package", id.clone())?;
                    row.set("shards", *n as i64)?;
                    loaded.set((i + 1) as i64, row)?;
                }
                for (i, e) in summary.errors.iter().enumerate() {
                    errors.set((i + 1) as i64, e.clone())?;
                }
            }
            result.set("activated", activated)?;
            result.set("loaded", loaded)?;
            result.set("errors", errors)?;
            Ok(result)
        })?,
    )?;

    lua.globals().set("manifest", manifest)?;
    Ok(())
}

fn load_managed(
    lua: &Lua,
    packages: Arc<LazyPackageRegistry>,
    default_id: String,
    id: String,
) -> mlua::Result<AnyUserData> {
    if id == default_id || id.eq_ignore_ascii_case("game") {
        return lua.create_userdata(ManifestPackage {
            id: default_id,
            packages,
            test_mode: true,
        });
    }

    if packages.is_test_mode() {
        if !packages.contains_key(&id) {
            eprintln!("[manifest] warn: LoadManaged('{id}') called for unknown package in test mode \u{2014} continuing");
        }
        return lua.create_userdata(ManifestPackage {
            id,
            packages,
            test_mode: true,
        });
    }

    if !packages.contains_key(&id) {
        return Err(mlua::Error::RuntimeError(format!(
            "manifest.LoadManaged: package '{id}' is not registered in this build"
        )));
    }

    if let Some(existing) = packages.get(&id) {
        if existing.is_activated() {
            return Err(mlua::Error::RuntimeError(format!(
                "manifest.LoadManaged: package '{id}' is already loaded"
            )));
        }
    }

    packages
        .activate(&id)
        .map_err(mlua::Error::RuntimeError)?;

    lua.create_userdata(ManifestPackage {
        id,
        packages,
        test_mode: false,
    })
}

struct ManifestPackage {
    id: String,
    packages: Arc<LazyPackageRegistry>,
    test_mode: bool,
}

impl UserData for ManifestPackage {
    fn add_methods<M: UserDataMethods<Self>>(m: &mut M) {
        m.add_method("Free", |_, this, _: ()| {
            if !this.test_mode {
                this.packages.free_assets(&this.id);
            }
            Ok(())
        });
        m.add_method("ID", |_, this, _: ()| Ok(this.id.clone()));
        m.add_method("IsAssetsLoaded", |_, this, _: ()| {
            if let Some(pkg) = this.packages.get(&this.id) {
                Ok(pkg.assets_loaded())
            } else {
                Ok(false)
            }
        });
    }
}
