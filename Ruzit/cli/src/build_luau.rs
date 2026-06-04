use std::cell::RefCell;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::{Arc, Mutex};

use mlua::{
    AnyUserData, Function, Lua, Table, UserData, UserDataFields, UserDataMethods, Value,
};

use ruzit_core::config::{
    AssetDisposition, BuildPlan, FileType, ManagedInfo, PackagePlan, ShardSpec,
};
use ruzit_core::package::PACKAGES_DIR_NAME;

#[derive(Debug, Clone)]
struct DiscoveredPackage {
    id: String,
    root: PathBuf,
    entry: String,
    name: String,
    version: String,
    creator: String,
    file_type: FileType,
    include: Vec<String>,
    is_default: bool,
}

#[derive(Debug, Default)]
struct PackageMutable {
    id: String,
    encryption_token: String,
    compress_scripts: bool,
    convert_to_byte: bool,
    flags: Vec<String>,
    shards: Vec<ShardSpec>,
    assets: HashMap<String, AssetDisposition>,
    parsed: bool,
}

#[derive(Clone)]
struct PackageHandle {
    state: Arc<Mutex<PackageMutable>>,
    root: PathBuf,
    #[allow(dead_code)]
    is_default: bool,
}

#[derive(Clone)]
struct ShardHandle {
    package_id: String,
    shard_id: u32,
    #[allow(dead_code)]
    state: Arc<Mutex<PackageMutable>>,
}

impl UserData for ShardHandle {
    fn add_fields<F: UserDataFields<Self>>(f: &mut F) {
        f.add_field_method_get("ID", |_, this| Ok(this.shard_id as i64));
        f.add_field_method_get("PackageID", |_, this| Ok(this.package_id.clone()));
    }

    fn add_methods<M: UserDataMethods<Self>>(m: &mut M) {
        m.add_method("ListenToFlag", |_, this, flag: String| -> mlua::Result<()> {
            let flag = flag.trim().to_string();
            if flag.is_empty() {
                return Err(mlua::Error::RuntimeError(
                    "ListenToFlag: flag name cannot be empty".into(),
                ));
            }
            let mut st = this.state.lock().unwrap();
            let entry = st
                .shards
                .iter_mut()
                .find(|s| s.id == this.shard_id)
                .ok_or_else(|| {
                    mlua::Error::RuntimeError(format!(
                        "ListenToFlag: shard {} no longer declared",
                        this.shard_id
                    ))
                })?;
            if !entry.flags.contains(&flag) {
                entry.flags.push(flag);
            }
            Ok(())
        });
    }
}

impl UserData for PackageHandle {
    fn add_fields<F: UserDataFields<Self>>(f: &mut F) {
        f.add_field_method_get("id", |_, this| Ok(this.state.lock().unwrap().id.clone()));
        f.add_field_method_get("EncryptionToken", |_, this| {
            Ok(this.state.lock().unwrap().encryption_token.clone())
        });
        f.add_field_method_set("EncryptionToken", |_, this, v: String| {
            this.state.lock().unwrap().encryption_token = v;
            Ok(())
        });
        f.add_field_method_get("CompressScripts", |_, this| {
            Ok(this.state.lock().unwrap().compress_scripts)
        });
        f.add_field_method_set("CompressScripts", |_, this, v: bool| {
            this.state.lock().unwrap().compress_scripts = v;
            Ok(())
        });
        f.add_field_method_get("ConvertToByte", |_, this| {
            Ok(this.state.lock().unwrap().convert_to_byte)
        });
        f.add_field_method_set("ConvertToByte", |_, this, v: bool| {
            this.state.lock().unwrap().convert_to_byte = v;
            Ok(())
        });
    }

    fn add_methods<M: UserDataMethods<Self>>(m: &mut M) {
        m.add_method("CreateShard", |lua, this, id: i64| -> mlua::Result<AnyUserData> {
            if id <= 0 || id > u32::MAX as i64 {
                return Err(mlua::Error::RuntimeError(format!(
                    "CreateShard: id must be a positive integer (got {id})"
                )));
            }
            let id = id as u32;
            let mut st = this.state.lock().unwrap();
            if st.shards.iter().any(|s| s.id == id) {
                return Err(mlua::Error::RuntimeError(format!(
                    "CreateShard: shard id {id} already declared for package '{}'",
                    st.id
                )));
            }
            st.shards.push(ShardSpec {
                id,
                flags: Vec::new(),
            });
            let handle = ShardHandle {
                package_id: st.id.clone(),
                shard_id: id,
                state: this.state.clone(),
            };
            lua.create_userdata(handle)
        });

        m.add_method("ListenToFlag", |_, this, flag: String| -> mlua::Result<()> {
            let flag = flag.trim().to_string();
            if flag.is_empty() {
                return Err(mlua::Error::RuntimeError(
                    "ListenToFlag: flag name cannot be empty".into(),
                ));
            }
            let mut st = this.state.lock().unwrap();
            if !st.flags.contains(&flag) {
                st.flags.push(flag);
            }
            Ok(())
        });

        m.add_method(
            "ParseAssets",
            |_, this, callback: Function| -> mlua::Result<()> {
                {
                    let mut st = this.state.lock().unwrap();
                    if st.parsed {
                        return Err(mlua::Error::RuntimeError(format!(
                            "ParseAssets: already called for package '{}'",
                            st.id
                        )));
                    }
                    st.parsed = true;
                }
                let pkg_id = this.state.lock().unwrap().id.clone();
                let assets_root = this.root.join("assets");
                if !assets_root.is_dir() {
                    return Ok(());
                }
                let mut files: Vec<(String, u64)> = Vec::new();
                walk_assets(&assets_root, &assets_root, &mut files)
                    .map_err(mlua::Error::RuntimeError)?;
                files.sort();
                for (rel, size) in &files {
                    let label = format!("@{pkg_id}/{rel}");
                    let mv = callback.call::<mlua::MultiValue>((label.clone(), *size as i64))?;
                    let mut it = mv.into_iter();
                    let shard_val = it.next();
                    let compress_val = it.next();
                    let enc_val = it.next();

                    let shard_id = match shard_val {
                        Some(Value::UserData(ud)) => {
                            let h = ud.borrow::<ShardHandle>().map_err(|_| {
                                mlua::Error::RuntimeError(format!(
                                    "ParseAssets: callback for '{label}' must return a shard (got non-shard userdata)"
                                ))
                            })?;
                            if h.package_id != pkg_id {
                                return Err(mlua::Error::RuntimeError(format!(
                                    "ParseAssets: callback for '{label}' returned a shard from a different package ('{}' vs '{pkg_id}')",
                                    h.package_id
                                )));
                            }
                            h.shard_id
                        }
                        Some(Value::Nil) | None => {
                            eprintln!(
                                "[Ruzit] warn: package '{pkg_id}' asset '{rel}' skipped (callback returned nil)"
                            );
                            continue;
                        }
                        _ => {
                            return Err(mlua::Error::RuntimeError(format!(
                                "ParseAssets: callback for '{label}' must return a shard as the first value"
                            )));
                        }
                    };

                    let compress = match compress_val {
                        Some(Value::Boolean(b)) => b,
                        Some(Value::Nil) | None => false,
                        _ => false,
                    };
                    let encryption = match enc_val {
                        Some(Value::String(s)) => s.to_str()?.to_string(),
                        Some(Value::Nil) | None => String::new(),
                        _ => String::new(),
                    };

                    let mut st = this.state.lock().unwrap();
                    st.assets.insert(
                        rel.clone(),
                        AssetDisposition {
                            shard_id,
                            compress,
                            encryption,
                        },
                    );
                }
                Ok(())
            },
        );
    }
}

fn walk_assets(root: &Path, dir: &Path, out: &mut Vec<(String, u64)>) -> Result<(), String> {
    for entry in fs::read_dir(dir).map_err(|e| format!("read_dir {}: {e}", dir.display()))? {
        let entry = entry.map_err(|e| e.to_string())?;
        let path = entry.path();
        if path.is_dir() {
            walk_assets(root, &path, out)?;
            continue;
        }
        if !path.is_file() {
            continue;
        }
        let rel = path
            .strip_prefix(root)
            .map_err(|e| e.to_string())?
            .to_string_lossy()
            .replace('\\', "/");
        let size = fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        out.push((rel, size));
    }
    Ok(())
}

pub fn build_luau_path(root: &Path) -> PathBuf {
    root.join("build.luau")
}

pub fn evaluate(root: &Path, default_id: &str, default_entry: &str) -> Result<Option<BuildPlan>, String> {
    let path = build_luau_path(root);
    if !path.is_file() {
        return Ok(None);
    }
    let text = fs::read_to_string(&path).map_err(|e| format!("read {}: {e}", path.display()))?;

    let discovered = discover_packages(root, default_id, default_entry)?;
    let lua = Lua::new();

    let build_tab = lua.create_table().map_err(|e| e.to_string())?;
    build_tab.set("name", "").map_err(|e| e.to_string())?;
    build_tab.set("version", "").map_err(|e| e.to_string())?;
    build_tab.set("creator", "").map_err(|e| e.to_string())?;
    build_tab.set("ExeName", default_id).map_err(|e| e.to_string())?;
    build_tab.set("Icon", Value::Nil).map_err(|e| e.to_string())?;
    build_tab.set("Windowed", true).map_err(|e| e.to_string())?;
    build_tab.set("FileType", "Relative").map_err(|e| e.to_string())?;
    build_tab.set("SteamID", Value::Nil).map_err(|e| e.to_string())?;
    let inc_tab = lua.create_table().map_err(|e| e.to_string())?;
    build_tab.set("include", inc_tab).map_err(|e| e.to_string())?;

    let handles: Vec<PackageHandle> = discovered
        .iter()
        .map(|d| PackageHandle {
            state: Arc::new(Mutex::new(PackageMutable {
                id: d.id.clone(),
                encryption_token: String::new(),
                compress_scripts: false,
                convert_to_byte: false,
                flags: Vec::new(),
                shards: Vec::new(),
                assets: HashMap::new(),
                parsed: false,
            })),
            root: d.root.clone(),
            is_default: d.is_default,
        })
        .collect();

    let handles_rc: Rc<RefCell<Vec<PackageHandle>>> = Rc::new(RefCell::new(handles));
    let handles_for_fn = handles_rc.clone();

    build_tab
        .set(
            "GetPackages",
            lua.create_function(move |lua, _: ()| -> mlua::Result<Table> {
                let arr = lua.create_table()?;
                for (i, h) in handles_for_fn.borrow().iter().enumerate() {
                    arr.set((i + 1) as i64, lua.create_userdata(h.clone())?)?;
                }
                Ok(arr)
            })
            .map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;

    lua.globals()
        .set("build", build_tab.clone())
        .map_err(|e| e.to_string())?;

    lua.load(&text)
        .set_name("@build.luau")
        .exec()
        .map_err(|e| format!("build.luau: {e}"))?;

    let name: String = build_tab.get("name").unwrap_or_default();
    let version: String = build_tab.get("version").unwrap_or_default();
    let creator: String = build_tab.get("creator").unwrap_or_default();
    let exe_name: String = build_tab.get("ExeName").unwrap_or_else(|_| default_id.to_string());
    let icon: Option<String> = match build_tab.get::<Value>("Icon").ok() {
        Some(Value::String(s)) => Some(s.to_str().map_err(|e| e.to_string())?.to_string()),
        _ => None,
    };
    let windowed: bool = build_tab.get("Windowed").unwrap_or(true);
    let file_type_s: String = build_tab.get("FileType").unwrap_or_else(|_| "Relative".to_string());
    let file_type = FileType::parse(&file_type_s)
        .ok_or_else(|| format!("build.luau: FileType must be 'Relative' or 'Global' (got '{file_type_s}')"))?;
    let steam_id: Option<u32> = match build_tab.get::<Value>("SteamID").ok() {
        Some(Value::Integer(i)) if i > 0 && i <= u32::MAX as i64 => Some(i as u32),
        Some(Value::Number(n)) if n > 0.0 && n <= u32::MAX as f64 => Some(n as u32),
        _ => None,
    };
    let include: Vec<String> = match build_tab.get::<Table>("include") {
        Ok(t) => t
            .sequence_values::<String>()
            .filter_map(|v| v.ok())
            .collect(),
        Err(_) => Vec::new(),
    };

    let exe_stem_default = if exe_name.is_empty() {
        default_id.to_string()
    } else {
        exe_name.clone()
    };

    let mut packages: Vec<PackagePlan> = Vec::new();
    for (h, disc) in handles_rc.borrow().iter().zip(discovered.iter()) {
        let st = h.state.lock().unwrap();
        validate_shards(&disc.id, &st.shards)?;
        let pkg_id = if disc.is_default {
            exe_stem_default.clone()
        } else {
            disc.id.clone()
        };
        for disp in st.assets.values() {
            if !st.shards.iter().any(|s| s.id == disp.shard_id) {
                return Err(format!(
                    "package '{}': asset routed to shard {} which was never CreateShard'd",
                    disc.id, disp.shard_id
                ));
            }
        }
        let pkg_name = if disc.is_default {
            if name.is_empty() {
                pkg_id.clone()
            } else {
                name.clone()
            }
        } else {
            disc.name.clone()
        };
        let pkg_version = if disc.is_default {
            version.clone()
        } else {
            disc.version.clone()
        };
        let pkg_creator = if disc.is_default {
            creator.clone()
        } else {
            disc.creator.clone()
        };
        packages.push(PackagePlan {
            id: pkg_id,
            root: disc.root.clone(),
            name: pkg_name,
            version: pkg_version,
            creator: pkg_creator,
            entry: disc.entry.clone(),
            file_type: if disc.is_default { file_type } else { disc.file_type },
            include: disc.include.clone(),
            encryption_token: st.encryption_token.clone(),
            compress_scripts: st.compress_scripts,
            convert_to_byte: st.convert_to_byte,
            flags: st.flags.clone(),
            shards: {
                let mut s = st.shards.clone();
                s.sort_by_key(|sh| sh.id);
                s
            },
            assets: st.assets.clone(),
        });
    }

    Ok(Some(BuildPlan {
        name,
        version,
        creator,
        exe_name: Some(exe_stem_default),
        exe_icon: icon,
        exe_windowed: windowed,
        file_type,
        steam_app_id: steam_id,
        include,
        compile_bytecode_default: false,
        packages,
        from_luau: true,
    }))
}

fn validate_shards(pkg_id: &str, shards: &[ShardSpec]) -> Result<(), String> {
    if shards.is_empty() {
        return Ok(());
    }
    let mut sorted: Vec<u32> = shards.iter().map(|s| s.id).collect();
    sorted.sort();
    for (i, v) in sorted.iter().enumerate() {
        let expected = (i as u32) + 1;
        if *v != expected {
            return Err(format!(
                "package '{pkg_id}': shard ids must form a contiguous run starting at 1 (got {:?})",
                sorted
            ));
        }
    }
    Ok(())
}

fn discover_packages(
    root: &Path,
    default_id: &str,
    default_entry: &str,
) -> Result<Vec<DiscoveredPackage>, String> {
    let mut out: Vec<DiscoveredPackage> = Vec::new();
    out.push(DiscoveredPackage {
        id: default_id.to_string(),
        root: root.to_path_buf(),
        entry: default_entry.to_string(),
        name: String::new(),
        version: String::new(),
        creator: String::new(),
        file_type: FileType::Relative,
        include: Vec::new(),
        is_default: true,
    });

    let dlc_folders = ruzit_core::package::find_dlc_folders(root)?;
    for dlc in &dlc_folders {
        let info = ManagedInfo::load(dlc)?;
        out.push(DiscoveredPackage {
            id: info.id,
            root: dlc.clone(),
            entry: info.entry,
            name: info.name,
            version: info.version,
            creator: info.creator,
            file_type: info.file_type,
            include: info.include,
            is_default: false,
        });
    }

    let pkg_dir = root.join(PACKAGES_DIR_NAME);
    if pkg_dir.is_dir() {
        for entry in fs::read_dir(&pkg_dir).map_err(|e| format!("read_dir {}: {e}", pkg_dir.display()))? {
            let entry = entry.map_err(|e| e.to_string())?;
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            if !path.join("ManagedInfo.toml").is_file() {
                continue;
            }
            let info = ManagedInfo::load(&path)?;
            out.push(DiscoveredPackage {
                id: info.id,
                root: path.clone(),
                entry: info.entry,
                name: info.name,
                version: info.version,
                creator: info.creator,
                file_type: info.file_type,
                include: info.include,
                is_default: false,
            });
        }
    }

    Ok(out)
}
