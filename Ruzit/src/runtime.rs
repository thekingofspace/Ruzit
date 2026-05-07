use mlua::{Lua, Table, Value};

use crate::heart;
use crate::libs;
use crate::vfs::{self, Fs, read_module, resolve};

const CACHE_KEY: &str = "ruzit_cache";

pub fn run_entry(fs: Fs, entry_key: &str) -> Result<(), String> {
    let lua = Lua::new();
    let cache = lua.create_table().map_err(|e| e.to_string())?;
    lua.set_named_registry_value(CACHE_KEY, cache)
        .map_err(|e| e.to_string())?;
    heart::ensure_registry(&lua).map_err(|e| format!("heart registry: {e}"))?;
    libs::signal::install(&lua).map_err(|e| format!("signal install: {e}"))?;

    load_module(&lua, &fs, entry_key)
        .map(|_| ())
        .map_err(|e| format!("Luau error: {e}"))?;

    heart::run_loop(&lua).map_err(|e| format!("heart loop: {e}"))
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

    Ok(env)
}

fn install_require(lua: &Lua, env: &Table, fs: &Fs, owner: &str) -> mlua::Result<()> {
    let fs = fs.clone();
    let owner = owner.to_string();
    let require = lua.create_function(move |lua, name: String| -> mlua::Result<Value> {
        let resolved = resolve(&fs, &owner, &name).ok_or_else(|| {
            mlua::Error::RuntimeError(format!(
                "module '{name}' not found (required from {owner})"
            ))
        })?;
        load_module(lua, &fs, &resolved)
    })?;
    env.set("require", require)
}

fn install_dirname(env: &Table, fs: &Fs, owner: &str) -> mlua::Result<()> {
    let dirname = vfs::caller_dir(fs, owner)
        .to_string_lossy()
        .into_owned();
    env.set("__dirname", dirname)
}

fn install_import(lua: &Lua, env: &Table, fs: &Fs, owner: &str) -> mlua::Result<()> {
    let fs = fs.clone();
    let owner = owner.to_string();
    let import = lua.create_function(move |lua, name: String| -> mlua::Result<Value> {
        match name.as_str() {
            "Asset" => Ok(Value::Table(libs::asset::create(
                lua,
                fs.clone(),
                owner.clone(),
            )?)),
            "IO" => Ok(Value::Table(libs::io::create(
                lua,
                fs.clone(),
                owner.clone(),
            )?)),
            "Managed" => Ok(Value::Table(libs::managed::create(lua, fs.clone())?)),
            "Net" => Ok(Value::Table(libs::net::create(lua)?)),
            "Process" => Ok(Value::Table(libs::process::create(lua)?)),
            "Serde" => Ok(Value::Table(libs::serde::create(lua)?)),
            "SFX" => Ok(Value::Table(libs::sfx::create(lua)?)),
            "Signal" => Ok(Value::Table(libs::signal::class(lua)?)),
            "Window" => Ok(Value::Table(libs::window::create(lua)?)),
            other => Err(mlua::Error::RuntimeError(format!(
                "import: unknown library '{other}'"
            ))),
        }
    })?;
    env.set("import", import)
}
