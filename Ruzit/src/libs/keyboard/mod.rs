use mlua::{Lua, Table, Value};

use crate::libs::input;

pub fn create(lua: &Lua) -> mlua::Result<Table> {
    let t = lua.create_table()?;

    let meta = lua.create_table()?;
    meta.set(
        "__index",
        lua.create_function(|lua, (_t, key): (Table, String)| -> mlua::Result<Value> {
            match key.as_str() {
                "InputChanged" => Ok(Value::Table(input::keyboard_input_signal(lua)?)),
                // IsKeyDown accepts either the canonical name (e.g. "W", "Space")
                // or the FNV-1a id we hand back via InputChanged. Strings hit
                // the name table; numbers walk the id values.
                "IsKeyDown" => Ok(Value::Function(lua.create_function(
                    |_, (_self, key): (Value, Value)| -> mlua::Result<bool> {
                        match key {
                            Value::String(s) => Ok(input::is_key_down_by_name(&s.to_str()?)),
                            Value::Number(n) => Ok(input::is_key_down_by_id(n as u32)),
                            Value::Integer(n) => Ok(input::is_key_down_by_id(n as u32)),
                            _ => Err(mlua::Error::RuntimeError(
                                "Keyboard:IsKeyDown expects a key name or id".into(),
                            )),
                        }
                    },
                )?)),
                "GetKeyId" => Ok(Value::Function(lua.create_function(
                    |_, (_self, name): (Value, String)| -> mlua::Result<i64> {
                        Ok(input::key_id_for_name(&name) as i64)
                    },
                )?)),
                _ => Ok(Value::Nil),
            }
        })?,
    )?;
    meta.set(
        "__newindex",
        lua.create_function(
            |_, (_t, key, _value): (Table, String, Value)| -> mlua::Result<()> {
                Err(mlua::Error::RuntimeError(format!(
                    "Keyboard.{key} is read-only"
                )))
            },
        )?,
    )?;
    t.set_metatable(Some(meta));
    Ok(t)
}
