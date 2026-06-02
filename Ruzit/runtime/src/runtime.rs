use std::cell::RefCell;
use std::collections::HashMap;
use std::io::Write;
use std::rc::Rc;

use mlua::{Lua, MultiValue, RegistryKey, Table, Value};

use crate::errors;
use crate::heart;
use crate::libs;
use crate::vfs::{self, Fs, read_module, resolve, split_owner};

const CACHE_KEY: &str = "ruzit_cache";

pub fn run_entry(fs: Fs, entry_key: &str) -> Result<(), String> {
    let lua = Lua::new();
    lua.set_compiler(
        mlua::Compiler::new()
            .set_optimization_level(2)
            .set_debug_level(1),
    );
    lua.enable_jit(true);
    let cache = lua.create_table().map_err(|e| e.to_string())?;
    lua.set_named_registry_value(CACHE_KEY, cache)
        .map_err(|e| e.to_string())?;
    errors::install_fs(&lua, &fs).map_err(|e| format!("fs registry: {e}"))?;
    heart::ensure_registry(&lua).map_err(|e| format!("heart registry: {e}"))?;
    libs::signal::install(&lua).map_err(|e| format!("signal install: {e}"))?;
    libs::input::install(&lua).map_err(|e| format!("input install: {e}"))?;
    libs::runservice::install(&lua).map_err(|e| format!("runservice install: {e}"))?;
    install_stats_proxy(&lua).map_err(|e| format!("stats proxy: {e}"))?;

    let entry_owned = entry_key.to_string();
    run_entry_in_thread(&lua, &fs, entry_key)
        .map_err(|e| errors::pretty_format(&lua, &fs, &entry_owned, &entry_owned, &e))?;

    heart::run_loop(&lua)
        .map_err(|e| errors::pretty_format(&lua, &fs, &entry_owned, &entry_owned, &e))
}

fn run_entry_in_thread(lua: &Lua, fs: &Fs, key: &str) -> mlua::Result<()> {
    let cache: Table = lua.named_registry_value(CACHE_KEY)?;
    cache.set(key.to_string(), Value::Boolean(true))?;
    let source = read_module(fs, key)
        .ok_or_else(|| mlua::Error::RuntimeError(format!("could not read entry: {key}")))?;
    let env = build_env(lua, fs.clone(), key.to_string())?;
    let chunk_name = format!("@{key}");
    let entry_fn = lua
        .load(&source)
        .set_name(&chunk_name)
        .set_environment(env)
        .into_function()?;
    let entry_thread = lua.create_thread(entry_fn)?;
    entry_thread.resume::<mlua::MultiValue>(())?;
    Ok(())
}

fn load_module(lua: &Lua, fs: &Fs, key: &str) -> mlua::Result<Value> {
    let cache: Table = lua.named_registry_value(CACHE_KEY)?;
    let cached: Value = cache.get(key.to_string())?;
    if !matches!(cached, Value::Nil) {
        return Ok(cached);
    }

    let source = read_module(fs, key)
        .ok_or_else(|| mlua::Error::RuntimeError(format!("could not read module: {key}")))?;
    cache.set(key.to_string(), Value::Boolean(true))?;

    let env = build_env(lua, fs.clone(), key.to_string())?;
    let chunk_name = format!("@{key}");
    let result: Value = lua
        .load(&source)
        .set_name(&chunk_name)
        .set_environment(env)
        .eval()?;

    let final_value = if matches!(result, Value::Nil) {
        Value::Boolean(true)
    } else {
        result
    };
    cache.set(key.to_string(), final_value.clone())?;
    Ok(final_value)
}

fn build_env(lua: &Lua, fs: Fs, owner: String) -> mlua::Result<Table> {
    let env = lua.create_table()?;
    let meta = lua.create_table()?;
    meta.set("__index", lua.globals())?;
    env.set_metatable(Some(meta))?;

    install_require(lua, &env, &fs, &owner)?;
    install_dirname(&env, &fs, &owner)?;
    install_import(lua, &env, &fs, &owner)?;
    install_print(lua, &env)?;
    install_load(lua, &env, &fs, &owner)?;
    install_launch(lua, &env, &fs, &owner)?;

    Ok(env)
}

fn install_print(lua: &Lua, env: &Table) -> mlua::Result<()> {
    let print = lua.create_function(|lua, args: MultiValue| -> mlua::Result<()> {
        let mut buf = String::new();
        for (i, v) in args.iter().enumerate() {
            if i > 0 {
                buf.push('\t');
            }

            let s = lua.coerce_string(v.clone())?;
            match s {
                Some(s) => buf.push_str(&s.to_str()?),
                None => buf.push_str(&format!("{v:?}")),
            }
        }
        let mut out = std::io::stdout().lock();
        let _ = writeln!(out, "{buf}");
        let _ = out.flush();
        Ok(())
    })?;
    env.set("print", print)
}

fn install_require(lua: &Lua, env: &Table, fs: &Fs, owner: &str) -> mlua::Result<()> {
    let fs = fs.clone();
    let owner = owner.to_string();
    let require = lua.create_function(move |lua, name: String| -> mlua::Result<Value> {
        let lookup = unrebase_name(&fs, &name);
        let resolved = resolve(&fs, &owner, &lookup).ok_or_else(|| {
            let where_label = describe_owner(&fs, &owner);
            mlua::Error::RuntimeError(format!(
                "module '{name}' not found (required from {where_label})"
            ))
        })?;
        load_module(lua, &fs, &resolved)
    })?;
    env.set("require", require)
}

fn unrebase_name(fs: &Fs, name: &str) -> String {
    let normalized = name.replace('\\', "/");
    let path = std::path::Path::new(&normalized);
    if !path.is_absolute() {
        return normalized;
    }
    let root = vfs::fs_root(fs);
    let root_str = root.to_string_lossy().replace('\\', "/");
    let mut root_trim = root_str.as_str();
    while root_trim.ends_with('/') {
        root_trim = &root_trim[..root_trim.len() - 1];
    }
    if let Some(rest) = normalized.strip_prefix(root_trim) {
        let mut rest = rest;
        while rest.starts_with('/') {
            rest = &rest[1..];
        }
        if rest.is_empty() {
            return normalized;
        }
        return format!("./{rest}");
    }
    normalized
}

fn install_dirname(env: &Table, fs: &Fs, owner: &str) -> mlua::Result<()> {
    let dirname = vfs::caller_dir(fs, owner).to_string_lossy().into_owned();
    env.set("__dirname", dirname)
}

fn install_import(lua: &Lua, env: &Table, fs: &Fs, owner: &str) -> mlua::Result<()> {
    let fs = fs.clone();
    let owner = owner.to_string();
    let cache: Rc<RefCell<HashMap<String, RegistryKey>>> =
        Rc::new(RefCell::new(HashMap::new()));
    let import = lua.create_function(move |lua, name: String| -> mlua::Result<Value> {
        if let Some(key) = cache.borrow().get(&name) {
            if let Ok(t) = lua.registry_value::<Table>(key) {
                return Ok(Value::Table(t));
            }
        }
        let table = match name.as_str() {
            "Actor" => libs::actor::create(lua, fs.clone(), owner.clone())?,
            "Asset" => libs::asset::create(lua, fs.clone(), owner.clone())?,
            "Debug" => libs::debug::create(lua)?,
            "DynImg" => libs::dynimg::create(lua)?,
            "DynMesh" => libs::dynmesh::create(lua)?,
            "FFI" => libs::ffi::create(lua)?,
            "Gamepad" => libs::gamepad::create(lua)?,
            "GPU" => libs::gpu::create(lua)?,
            "GUI" => libs::gui::create(lua)?,
            "Image" => libs::image::create(lua)?,
            "IO" => libs::io::create(lua, fs.clone(), owner.clone())?,
            "Keyboard" => libs::keyboard::create(lua)?,
            "LightingService" => libs::lighting::create(lua)?,
            "Managed" => libs::managed::create(lua, fs.clone())?,
            "Mouse" => libs::mouse::create(lua)?,
            "Net" => libs::net::create(lua)?,
            "Objects" => libs::objects::create(lua)?,
            "Package" => libs::package::create(lua)?,
            "PhysicsService" => libs::physics::create(lua)?,
            "Primitives" => libs::primitives::create(lua)?,
            "Process" => libs::process::create(lua)?,
            "Register" => libs::register::create(lua)?,
            "Renderable" => libs::renderable::create(lua)?,
            "Serde" => libs::serde::create(lua)?,
            "SFX" => libs::sfx::create(lua)?,
            "SoundByte" => libs::soundbyte::create(lua)?,
            "Signal" => libs::signal::class(lua)?,
            #[cfg(feature = "steam")]
            "Steam" => libs::steam::create(lua)?,
            "Task" => libs::task::create(lua)?,
            "TweenService" => libs::tween::create(lua)?,
            #[cfg(feature = "voice")]
            "Voice" => libs::voice::create(lua)?,
            #[cfg(feature = "vr")]
            "VR" => libs::vr::create(lua)?,
            "VirtualReality" => libs::virtual_reality::create(lua)?,
            "Window" => libs::window::create(lua)?,
            other => {
                return Err(mlua::Error::RuntimeError(format!(
                    "import: unknown library '{other}' (called from {})",
                    describe_owner(&fs, &owner)
                )));
            }
        };
        if let Ok(key) = lua.create_registry_value(table.clone()) {
            cache.borrow_mut().insert(name, key);
        }
        Ok(Value::Table(table))
    })?;
    env.set("import", import)
}

fn install_load(lua: &Lua, env: &Table, fs: &Fs, owner: &str) -> mlua::Result<()> {
    let caller_fs = fs.clone();
    let caller_owner = owner.to_string();
    let load = lua.create_function(move |lua, raw: String| -> mlua::Result<Value> {
        let abs = resolve_load_path(&caller_fs, &caller_owner, &raw)?;
        let source = std::fs::read_to_string(&abs).map_err(|e| {
            mlua::Error::RuntimeError(format!("load: read {}: {e}", abs.display()))
        })?;

        let parent = abs
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| std::path::PathBuf::from("/"));
        let (host_packages, host_default_id) = host_package_view(&caller_fs);
        let game_root = vfs::fs_root(&caller_fs);
        let host_file_type = caller_fs.file_type();

        let load_root = match host_file_type {
            crate::config::FileType::Global => game_root,
            crate::config::FileType::Relative => parent.clone(),
        };

        let load_fs = Fs::Disk {
            root: load_root,
            file_type: host_file_type,
            packages: host_packages,
            default_id: host_default_id,
        };
        let owner_str = abs.to_string_lossy().into_owned();
        let env = build_env(lua, load_fs, owner_str.clone())?;
        let chunk_name = format!("@{owner_str}");
        let result: Value = lua
            .load(&source)
            .set_name(&chunk_name)
            .set_environment(env)
            .eval()?;
        Ok(if matches!(result, Value::Nil) {
            Value::Boolean(true)
        } else {
            result
        })
    })?;
    env.set("load", load)
}

fn resolve_load_path(fs: &Fs, owner: &str, raw: &str) -> mlua::Result<std::path::PathBuf> {
    let p = std::path::Path::new(raw);
    let exe_relative = raw.starts_with("./") || raw.starts_with(".\\");
    let mut candidate = if p.is_absolute() {
        p.to_path_buf()
    } else if exe_relative {
        let exe_dir = std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(|d| d.to_path_buf()))
            .ok_or_else(|| {
                mlua::Error::RuntimeError(
                    "load: './' is exe-relative but the current_exe path is unavailable".into(),
                )
            })?;
        let stripped = &raw[2..];
        exe_dir.join(stripped)
    } else {
        vfs::caller_dir(fs, owner).join(raw)
    };
    candidate = vfs::normalize(&candidate);
    if candidate.is_file() {
        return Ok(candidate);
    }
    if candidate.extension().is_none() {
        for ext in ["luau", "lua"] {
            let mut probed = candidate.clone();
            probed.set_extension(ext);
            if probed.is_file() {
                return Ok(probed);
            }
        }
    }
    Err(mlua::Error::RuntimeError(format!(
        "load: file not found '{raw}' (resolved to '{}')",
        candidate.display()
    )))
}

fn host_package_view(
    fs: &Fs,
) -> (
    Option<std::sync::Arc<crate::vfs::LazyPackageRegistry>>,
    Option<String>,
) {
    match fs {
        Fs::Bundle {
            packages,
            default_id,
            ..
        } => (Some(packages.clone()), Some(default_id.clone())),
        Fs::Disk {
            packages,
            default_id,
            ..
        } => (packages.clone(), default_id.clone()),
    }
}

fn describe_owner(fs: &Fs, owner: &str) -> String {
    let pkg = errors::package_label(fs, owner);
    let inner = match fs {
        Fs::Disk { .. } => owner.to_string(),
        Fs::Bundle { default_id, .. } => {
            let (_, inner) = split_owner(owner, default_id);
            inner.to_string()
        }
    };
    format!("{inner} in {pkg}")
}

use std::sync::atomic::{AtomicBool, Ordering as AOrdering};

thread_local! {
    static LAUNCH_STATS: RefCell<Vec<LaunchStatsEntry>> = const { RefCell::new(Vec::new()) };
}

struct LaunchStatsEntry {
    thread: RegistryKey,
    stats: RegistryKey,
}

fn register_launch_stats(
    lua: &Lua,
    thread: &mlua::Thread,
    stats: &Table,
) -> mlua::Result<()> {
    let t_key = lua.create_registry_value(thread.clone())?;
    let s_key = lua.create_registry_value(stats.clone())?;
    LAUNCH_STATS.with(|m| {
        m.borrow_mut().push(LaunchStatsEntry {
            thread: t_key,
            stats: s_key,
        });
    });
    Ok(())
}

fn unregister_launch_stats(lua: &Lua, thread: &mlua::Thread) {
    LAUNCH_STATS.with(|m| {
        let mut entries = m.borrow_mut();
        entries.retain(|e| {
            if let Ok(t) = lua.registry_value::<mlua::Thread>(&e.thread) {
                if t == *thread {
                    return false;
                }
            }
            true
        });
    });
}


fn current_launch_stats(lua: &Lua) -> mlua::Result<Option<Table>> {
    let current = lua.current_thread();
    let mut result: Option<Table> = None;
    LAUNCH_STATS.with(|m| -> mlua::Result<()> {
        let mut entries = m.borrow_mut();
        for entry in entries.iter().rev() {
            if let Ok(t) = lua.registry_value::<mlua::Thread>(&entry.thread) {
                if t == current {
                    result = Some(lua.registry_value::<Table>(&entry.stats)?);
                    break;
                }
            }
        }

        entries.retain(|e| {
            match lua.registry_value::<mlua::Thread>(&e.thread) {
                Ok(t) => !matches!(t.status(), mlua::ThreadStatus::Finished),
                Err(_) => false,
            }
        });
        Ok(())
    })?;
    Ok(result)
}

fn install_stats_proxy(lua: &Lua) -> mlua::Result<()> {
    let stats_table = lua.create_table()?;
    let mt = lua.create_table()?;

    let idx = lua.create_function(
        |lua, (_proxy, key): (Table, Value)| -> mlua::Result<Value> {
            match current_launch_stats(lua)? {
                Some(stats) => stats.get::<Value>(key),
                None => Ok(Value::Nil),
            }
        },
    )?;
    let newidx = lua.create_function(
        |lua, (_proxy, key, value): (Table, Value, Value)| -> mlua::Result<()> {
            match current_launch_stats(lua)? {
                Some(stats) => {
                    stats.set(key, value)?;
                    Ok(())
                }
                None => Err(mlua::Error::RuntimeError(
                    "stats can only be written from a thread spawned via `launch(...)`"
                        .into(),
                )),
            }
        },
    )?;

    mt.set("__index", idx)?;
    mt.set("__newindex", newidx)?;
    mt.set("__metatable", "stats")?;
    stats_table.set_metatable(Some(mt))?;
    lua.globals().set("stats", stats_table)?;
    Ok(())
}

pub struct SubProcess {
    pub env_key: RegistryKey,
    pub thread_key: RegistryKey,
    pub stats_key: RegistryKey,
    pub alive: std::sync::Arc<AtomicBool>,
    pub source_label: String,
}

impl mlua::UserData for SubProcess {
    fn add_fields<F: mlua::UserDataFields<Self>>(f: &mut F) {
        f.add_field_method_get("Alive", |_, this| Ok(this.alive.load(AOrdering::Relaxed)));
        f.add_field_method_get("Source", |_, this| Ok(this.source_label.clone()));
        f.add_field_method_get("Coroutine", |lua, this| -> mlua::Result<mlua::Thread> {
            lua.registry_value::<mlua::Thread>(&this.thread_key)
        });
        f.add_field_method_get("Stats", |lua, this| -> mlua::Result<Table> {
            lua.registry_value::<Table>(&this.stats_key)
        });
    }

    fn add_methods<M: mlua::UserDataMethods<Self>>(m: &mut M) {
        m.add_method("Kill", |lua, this, _: ()| -> mlua::Result<()> {
            this.alive.store(false, AOrdering::Relaxed);
            if let Ok(thread) = lua.registry_value::<mlua::Thread>(&this.thread_key) {
                unregister_launch_stats(lua, &thread);
                let noop = lua.create_function(|_, _: MultiValue| -> mlua::Result<()> { Ok(()) })?;
                let _ = thread.reset(noop);
            }
            Ok(())
        });
        m.add_method("WriteStat", |lua, this, (key, value): (Value, Value)| -> mlua::Result<()> {
            let stats: Table = lua.registry_value(&this.stats_key)?;
            stats.set(key, value)?;
            Ok(())
        });
        m.add_method("ReadStat", |lua, this, key: Value| -> mlua::Result<Value> {
            let stats: Table = lua.registry_value(&this.stats_key)?;
            stats.get::<Value>(key)
        });
        m.add_method("Resume", |lua, this, args: MultiValue| -> mlua::Result<MultiValue> {
            if !this.alive.load(AOrdering::Relaxed) {
                return Err(mlua::Error::RuntimeError(
                    "SubProcess:Resume: subprocess has been killed".into(),
                ));
            }
            let thread: mlua::Thread = lua.registry_value(&this.thread_key)?;
            thread.resume::<MultiValue>(args)
        });
        m.add_meta_method("__tostring", |_, this, _: ()| {
            Ok(format!(
                "SubProcess({}, alive={})",
                this.source_label,
                this.alive.load(AOrdering::Relaxed)
            ))
        });
    }
}

fn install_launch(lua: &Lua, env: &Table, fs: &Fs, owner: &str) -> mlua::Result<()> {
    let fs = fs.clone();
    let owner = owner.to_string();
    let launch = lua.create_function(move |lua, args: MultiValue| -> mlua::Result<SubProcess> {
        let mut iter = args.into_iter();
        let path_v = iter.next().ok_or_else(|| {
            mlua::Error::RuntimeError("launch: first argument must be a path string".into())
        })?;
        let path = match path_v {
            Value::String(s) => s.to_str()?.to_string(),
            _ => {
                return Err(mlua::Error::RuntimeError(
                    "launch: first argument must be a path string".into(),
                ));
            }
        };
        let lookup = unrebase_name(&fs, &path);
        let resolved = resolve(&fs, &owner, &lookup).ok_or_else(|| {
            mlua::Error::RuntimeError(format!(
                "launch: module '{path}' not found (called from {})",
                describe_owner(&fs, &owner)
            ))
        })?;
        let source = read_module(&fs, &resolved).ok_or_else(|| {
            mlua::Error::RuntimeError(format!("launch: could not read module: {resolved}"))
        })?;

        let new_env = build_env(lua, fs.clone(), resolved.clone())?;
        let stats = lua.create_table()?;
        let rest: Vec<Value> = iter.collect();
        for (i, v) in rest.iter().enumerate() {
            stats.set(i as i64 + 1, v.clone())?;
        }

        let chunk_name = format!("@{resolved}");
        let entry_fn = lua
            .load(&source)
            .set_name(&chunk_name)
            .set_environment(new_env.clone())
            .into_function()?;
        let thread = lua.create_thread(entry_fn)?;
        register_launch_stats(lua, &thread, &stats)?;

        crate::libs::task::defer_thread(lua, thread.clone())?;

        let env_key = lua.create_registry_value(new_env)?;
        let thread_key = lua.create_registry_value(thread)?;
        let stats_key = lua.create_registry_value(stats)?;
        Ok(SubProcess {
            env_key,
            thread_key,
            stats_key,
            alive: std::sync::Arc::new(AtomicBool::new(true)),
            source_label: resolved,
        })
    })?;
    env.set("launch", launch)
}
