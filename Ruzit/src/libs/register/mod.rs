//! Register — named shared tables. `Register.new("foo")` always returns the
//! same table for the same name across the entire Lua state, so two
//! scripts that have never seen each other can coordinate by agreeing on
//! a name.
//!
//! Implementation: a single Lua table held in the named registry, keyed
//! by the string name. We deliberately store the *Lua-side* table (not a
//! Rust HashMap) so the registers themselves participate in normal Luau
//! semantics — metatables, weak references, equality — and so we don't
//! have to round-trip values through Rust on every read/write.

use mlua::{Lua, Table, Value};

const REGISTRY_KEY: &str = "ruzit_register_store";

fn ensure_store(lua: &Lua) -> mlua::Result<Table> {
    if let Ok(t) = lua.named_registry_value::<Table>(REGISTRY_KEY) {
        return Ok(t);
    }
    let store = lua.create_table()?;
    lua.set_named_registry_value(REGISTRY_KEY, store.clone())?;
    Ok(store)
}

pub fn create(lua: &Lua) -> mlua::Result<Table> {
    let api = lua.create_table()?;

    api.set(
        "new",
        lua.create_function(|lua, name: String| -> mlua::Result<Table> {
            let store = ensure_store(lua)?;
            match store.get::<Value>(name.clone())? {
                Value::Table(t) => Ok(t),
                _ => {
                    let fresh = lua.create_table()?;
                    store.set(name, fresh.clone())?;
                    Ok(fresh)
                }
            }
        })?,
    )?;

    api.set(
        "Has",
        lua.create_function(|lua, name: String| -> mlua::Result<bool> {
            let store = ensure_store(lua)?;
            Ok(matches!(store.get::<Value>(name)?, Value::Table(_)))
        })?,
    )?;

    api.set(
        "Drop",
        lua.create_function(|lua, name: String| -> mlua::Result<bool> {
            let store = ensure_store(lua)?;
            let existed = matches!(store.get::<Value>(name.clone())?, Value::Table(_));
            store.set(name, Value::Nil)?;
            Ok(existed)
        })?,
    )?;

    api.set(
        "Names",
        lua.create_function(|lua, _: ()| -> mlua::Result<Table> {
            let store = ensure_store(lua)?;
            let out = lua.create_table()?;
            for (i, pair) in store.clone().pairs::<Value, Value>().enumerate() {
                let (k, _) = pair?;
                if let Value::String(s) = k {
                    out.set(i + 1, s)?;
                }
            }
            Ok(out)
        })?,
    )?;

    Ok(api)
}
