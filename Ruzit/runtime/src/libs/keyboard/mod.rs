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
                "Copy" => Ok(Value::Function(lua.create_function(
                    |_, (_self, text): (Value, String)| -> mlua::Result<()> {
                        let mut cb = arboard::Clipboard::new().map_err(|e| {
                            mlua::Error::RuntimeError(format!("Keyboard.Copy: clipboard init: {e}"))
                        })?;
                        cb.set_text(text).map_err(|e| {
                            mlua::Error::RuntimeError(format!("Keyboard.Copy: set_text: {e}"))
                        })?;
                        Ok(())
                    },
                )?)),
                "Paste" => Ok(Value::Function(lua.create_function(
                    |_, _self: Value| -> mlua::Result<String> {
                        let mut cb = arboard::Clipboard::new().map_err(|e| {
                            mlua::Error::RuntimeError(format!("Keyboard.Paste: clipboard init: {e}"))
                        })?;
                        cb.get_text().map_err(|e| {
                            mlua::Error::RuntimeError(format!("Keyboard.Paste: get_text: {e}"))
                        })
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
    t.set_metatable(Some(meta))?;
    Ok(t)
}
