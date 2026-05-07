use std::sync::Arc;

use mlua::{Lua, Table, UserData, UserDataFields, UserDataMethods, Value};

use crate::vfs::{Fs, Package};

pub fn create(lua: &Lua, fs: Fs) -> mlua::Result<Table> {
    let t = lua.create_table()?;

    let fs_a = fs.clone();
    t.set(
        "IsPackage",
        lua.create_function(move |_, id: String| -> mlua::Result<bool> {
            Ok(matches!(&fs_a, Fs::Bundle { packages, .. } if packages.contains_key(&id)))
        })?,
    )?;

    let fs_b = fs.clone();
    t.set(
        "GetPackage",
        lua.create_function(move |lua, id: String| -> mlua::Result<Value> {
            if let Fs::Bundle { packages, .. } = &fs_b {
                if let Some(pkg) = packages.get(&id) {
                    let ud = lua.create_userdata(PackageRef(pkg.clone()))?;
                    return Ok(Value::UserData(ud));
                }
            }
            Ok(Value::Nil)
        })?,
    )?;

    let fs_c = fs.clone();
    t.set(
        "List",
        lua.create_function(move |lua, _: ()| -> mlua::Result<Table> {
            let arr = lua.create_table()?;
            if let Fs::Bundle { packages, .. } = &fs_c {
                let mut ids: Vec<&String> = packages.keys().collect();
                ids.sort();
                for (i, id) in ids.iter().enumerate() {
                    arr.set((i + 1) as i64, (*id).clone())?;
                }
            }
            Ok(arr)
        })?,
    )?;

    let fs_d = fs.clone();
    t.set(
        "Default",
        lua.create_function(move |lua, _: ()| -> mlua::Result<Value> {
            if let Fs::Bundle {
                packages,
                default_id,
                ..
            } = &fs_d
            {
                if let Some(pkg) = packages.get(default_id) {
                    let ud = lua.create_userdata(PackageRef(pkg.clone()))?;
                    return Ok(Value::UserData(ud));
                }
            }
            Ok(Value::Nil)
        })?,
    )?;

    Ok(t)
}

pub struct PackageRef(pub Arc<Package>);

impl UserData for PackageRef {
    fn add_fields<F: UserDataFields<Self>>(f: &mut F) {
        f.add_field_method_get("ID", |_, this| Ok(this.0.id.clone()));
        f.add_field_method_get("Name", |_, this| Ok(this.0.name.clone()));
        f.add_field_method_get("Version", |_, this| Ok(this.0.version.clone()));
        f.add_field_method_get("Creator", |_, this| Ok(this.0.creator.clone()));
        f.add_field_method_get("Entry", |_, this| Ok(this.0.entry.clone()));
        f.add_field_method_get("Origin", |_, this| -> mlua::Result<String> {
            
            let stem = this
                .0
                .entry
                .trim_end_matches(".luau")
                .trim_end_matches(".lua");
            Ok(format!("@{}/{}", this.0.id, stem))
        });
    }

    fn add_methods<M: UserDataMethods<Self>>(m: &mut M) {
        m.add_method("HasFile", |_, this, key: String| {
            Ok(this.0.files.contains_key(&key))
        });
        m.add_method("HasAsset", |_, this, key: String| {
            Ok(this.0.assets.contains_key(&key))
        });
        m.add_method("Files", |lua, this, _: ()| -> mlua::Result<Table> {
            let arr = lua.create_table()?;
            let mut keys: Vec<&String> = this.0.files.keys().collect();
            keys.sort();
            for (i, k) in keys.iter().enumerate() {
                arr.set((i + 1) as i64, (*k).clone())?;
            }
            Ok(arr)
        });
        m.add_method("Assets", |lua, this, _: ()| -> mlua::Result<Table> {
            let arr = lua.create_table()?;
            let mut keys: Vec<&String> = this.0.assets.keys().collect();
            keys.sort();
            for (i, k) in keys.iter().enumerate() {
                arr.set((i + 1) as i64, (*k).clone())?;
            }
            Ok(arr)
        });
    }
}
