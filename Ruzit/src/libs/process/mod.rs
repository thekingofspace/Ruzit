use std::env;
use std::process;

use mlua::{Function, Lua, Table, Value};

use crate::heart;

pub fn create(lua: &Lua) -> mlua::Result<Table> {
    let t = lua.create_table()?;

    install_constants(&t)?;
    install_env(lua, &t)?;
    install_close(lua, &t)?;
    install_memory(lua, &t)?;
    install_heart(lua, &t)?;
    install_args(lua, &t)?;

    Ok(t)
}

fn install_constants(t: &Table) -> mlua::Result<()> {
    t.set("Os", env::consts::OS)?;
    t.set("Arch", env::consts::ARCH)?;
    t.set("Family", env::consts::FAMILY)?;
    t.set("Pid", process::id() as i64)?;
    let cpu_count = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1);
    t.set("CpuCount", cpu_count as i64)?;
    Ok(())
}

fn install_env(lua: &Lua, t: &Table) -> mlua::Result<()> {
    t.set(
        "Env",
        lua.create_function(|lua, name: Option<String>| -> mlua::Result<Value> {
            match name {
                Some(n) => match env::var(&n) {
                    Ok(v) => Ok(Value::String(lua.create_string(&v)?)),
                    Err(_) => Ok(Value::Nil),
                },
                None => {
                    let table = lua.create_table()?;
                    for (k, v) in env::vars() {
                        table.set(k, v)?;
                    }
                    Ok(Value::Table(table))
                }
            }
        })?,
    )?;

    t.set(
        "SetEnv",
        lua.create_function(|_, (name, value): (String, String)| -> mlua::Result<()> {
            // SAFETY: Process is single-threaded for env mutation; mlua state is non-Send.
            unsafe { env::set_var(&name, &value) };
            Ok(())
        })?,
    )?;

    Ok(())
}

fn install_close(lua: &Lua, t: &Table) -> mlua::Result<()> {
    t.set(
        "Close",
        lua.create_function(|_, code: Option<i32>| -> mlua::Result<()> {
            process::exit(code.unwrap_or(0));
        })?,
    )?;
    Ok(())
}

fn install_memory(lua: &Lua, t: &Table) -> mlua::Result<()> {
    t.set(
        "Memory",
        lua.create_function(|lua, _: ()| -> mlua::Result<Table> {
            let mut sys = sysinfo::System::new();
            sys.refresh_memory();
            let m = lua.create_table()?;
            m.set("total", sys.total_memory() as f64)?;
            m.set("free", sys.free_memory() as f64)?;
            m.set("used", sys.used_memory() as f64)?;
            m.set("available", sys.available_memory() as f64)?;
            m.set("total_swap", sys.total_swap() as f64)?;
            m.set("used_swap", sys.used_swap() as f64)?;
            Ok(m)
        })?,
    )?;
    Ok(())
}

fn install_heart(lua: &Lua, t: &Table) -> mlua::Result<()> {
    let bind = lua.create_function(
        |lua, (id, func): (String, Function)| -> mlua::Result<()> {
            let registry: Table = lua.named_registry_value(heart::HEART_KEY)?;
            registry.set(id, func)
        },
    )?;
    t.set("BindToHeart", bind)?;

    let unbind = lua.create_function(|lua, id: String| -> mlua::Result<()> {
        let registry: Table = lua.named_registry_value(heart::HEART_KEY)?;
        registry.set(id, Value::Nil)
    })?;
    t.set("UnbindFromHeart", unbind)?;

    Ok(())
}

fn install_args(lua: &Lua, t: &Table) -> mlua::Result<()> {
    t.set(
        "Args",
        lua.create_function(|lua, _: ()| -> mlua::Result<Table> {
            let arr = lua.create_table()?;
            for (i, arg) in env::args().enumerate() {
                arr.set((i + 1) as i64, arg)?;
            }
            Ok(arr)
        })?,
    )?;
    Ok(())
}
