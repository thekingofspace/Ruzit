use std::io::Write;

use mlua::{Lua, MultiValue, Table, Value};

use crate::errors;
use crate::heart;
use crate::libs;
use crate::vfs::{self, Fs, read_module, resolve, split_owner};

const CACHE_KEY: &str = "ruzit_cache";

pub fn run_entry(fs: Fs, entry_key: &str) -> Result<(), String> {
    let lua = Lua::new();
    let cache = lua.create_table().map_err(|e| e.to_string())?;
    lua.set_named_registry_value(CACHE_KEY, cache)
        .map_err(|e| e.to_string())?;
    errors::install_fs(&lua, &fs).map_err(|e| format!("fs registry: {e}"))?;
    heart::ensure_registry(&lua).map_err(|e| format!("heart registry: {e}"))?;
    libs::signal::install(&lua).map_err(|e| format!("signal install: {e}"))?;
    libs::input::install(&lua).map_err(|e| format!("input install: {e}"))?;
    libs::runservice::install(&lua).map_err(|e| format!("runservice install: {e}"))?;

    let entry_owned = entry_key.to_string();
    load_module(&lua, &fs, entry_key)
        .map(|_| ())
        .map_err(|e| errors::pretty_format(&lua, &fs, &entry_owned, &entry_owned, &e))?;

    heart::run_loop(&lua)
        .map_err(|e| errors::pretty_format(&lua, &fs, &entry_owned, &entry_owned, &e))
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
    env.set_metatable(Some(meta));

    install_require(lua, &env, &fs, &owner)?;
    install_dirname(&env, &fs, &owner)?;
    install_import(lua, &env, &fs, &owner)?;
    install_print(lua, &env)?;

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
        let resolved = resolve(&fs, &owner, &name).ok_or_else(|| {
            let where_label = describe_owner(&fs, &owner);
            mlua::Error::RuntimeError(format!(
                "module '{name}' not found (required from {where_label})"
            ))
        })?;
        load_module(lua, &fs, &resolved)
    })?;
    env.set("require", require)
}

fn install_dirname(env: &Table, fs: &Fs, owner: &str) -> mlua::Result<()> {
    let dirname = vfs::caller_dir(fs, owner).to_string_lossy().into_owned();
    env.set("__dirname", dirname)
}

fn install_import(lua: &Lua, env: &Table, fs: &Fs, owner: &str) -> mlua::Result<()> {
    let fs = fs.clone();
    let owner = owner.to_string();
    let import = lua.create_function(move |lua, name: String| -> mlua::Result<Value> {
        match name.as_str() {
            "Actor" => Ok(Value::Table(libs::actor::create(
                lua,
                fs.clone(),
                owner.clone(),
            )?)),
            "Asset" => Ok(Value::Table(libs::asset::create(
                lua,
                fs.clone(),
                owner.clone(),
            )?)),
            "Gamepad" => Ok(Value::Table(libs::gamepad::create(lua)?)),
            "GPU" => Ok(Value::Table(libs::gpu::create(lua)?)),
            "GUI" => Ok(Value::Table(libs::gui::create(lua)?)),
            "IO" => Ok(Value::Table(libs::io::create(
                lua,
                fs.clone(),
                owner.clone(),
            )?)),
            "Keyboard" => Ok(Value::Table(libs::keyboard::create(lua)?)),
            "Managed" => Ok(Value::Table(libs::managed::create(lua, fs.clone())?)),
            "Mouse" => Ok(Value::Table(libs::mouse::create(lua)?)),
            "Net" => Ok(Value::Table(libs::net::create(lua)?)),
            "Primitives" => Ok(Value::Table(libs::primitives::create(lua)?)),
            "Process" => Ok(Value::Table(libs::process::create(lua)?)),
            "Renderable" => Ok(Value::Table(libs::renderable::create(lua)?)),
            "RunService" => Ok(Value::Table(libs::runservice::create(lua)?)),
            "Serde" => Ok(Value::Table(libs::serde::create(lua)?)),
            "SFX" => Ok(Value::Table(libs::sfx::create(lua)?)),
            "Signal" => Ok(Value::Table(libs::signal::class(lua)?)),
            "Steam" => Ok(Value::Table(libs::steam::create(lua)?)),
            "Voice" => Ok(Value::Table(libs::voice::create(lua)?)),
            "Window" => Ok(Value::Table(libs::window::create(lua)?)),
            other => Err(mlua::Error::RuntimeError(format!(
                "import: unknown library '{other}' (called from {})",
                describe_owner(&fs, &owner)
            ))),
        }
    })?;
    env.set("import", import)
}

/// Compact "<script> in <package>" descriptor for error messages emitted from
/// Rust callbacks (require / import) where we don't get a Luau line number.
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
