use mlua::{Lua, Table, Value};

use crate::libs::input::{self, CursorKind};
use crate::libs::window;

pub fn create(lua: &Lua) -> mlua::Result<Table> {
    let t = lua.create_table()?;

    let meta = lua.create_table()?;
    meta.set(
        "__index",
        lua.create_function(|lua, (_t, key): (Table, String)| -> mlua::Result<Value> {
            match key.as_str() {
                "Position" => Ok(Value::UserData(
                    lua.create_userdata(input::cursor_position())?,
                )),
                "Visible" => Ok(Value::Boolean(input::is_cursor_visible())),
                "Locked" => Ok(Value::Boolean(input::is_locked())),
                "Cursor" => Ok(Value::String(lua.create_string(input::cursor_kind_name())?)),
                "Moved" => Ok(Value::Table(input::moved_signal(lua)?)),
                "InputReceived" => Ok(Value::Table(input::mouse_input_signal(lua)?)),
                "Scrolled" => Ok(Value::Table(input::mouse_scroll_signal(lua)?)),
                "SetCursor" => Ok(Value::Function(lua.create_function(
                    |_, (_self, name): (Value, String)| -> mlua::Result<()> {
                        let kind = CursorKind::from_name(&name)?;
                        window::with_window_static(|w| input::set_cursor(Some(w), kind));
                        Ok(())
                    },
                )?)),
                "Lock" => Ok(Value::Function(lua.create_function(
                    |_, _self: Value| -> mlua::Result<()> {
                        window::with_window_static(|w| input::set_locked(Some(w), true));
                        Ok(())
                    },
                )?)),
                "Unlock" => Ok(Value::Function(lua.create_function(
                    |_, _self: Value| -> mlua::Result<()> {
                        window::with_window_static(|w| input::set_locked(Some(w), false));
                        Ok(())
                    },
                )?)),
                "IsButtonDown" => Ok(Value::Function(lua.create_function(
                    |_, (_self, name): (Value, String)| -> mlua::Result<bool> {
                        Ok(input::is_mouse_button_down(&name))
                    },
                )?)),
                _ => Ok(Value::Nil),
            }
        })?,
    )?;
    meta.set(
        "__newindex",
        lua.create_function(
            |_, (_t, key, value): (Table, String, Value)| -> mlua::Result<()> {
                match key.as_str() {
                    "Visible" => match value {
                        Value::Boolean(b) => {
                            window::with_window_static(|w| input::set_visible(Some(w), b));
                            Ok(())
                        }
                        _ => Err(mlua::Error::RuntimeError(
                            "Mouse.Visible expects a boolean".into(),
                        )),
                    },
                    "Locked" => match value {
                        Value::Boolean(b) => {
                            window::with_window_static(|w| input::set_locked(Some(w), b));
                            Ok(())
                        }
                        _ => Err(mlua::Error::RuntimeError(
                            "Mouse.Locked expects a boolean".into(),
                        )),
                    },
                    "Cursor" => match value {
                        Value::String(s) => {
                            let kind = CursorKind::from_name(&s.to_str()?)?;
                            window::with_window_static(|w| input::set_cursor(Some(w), kind));
                            Ok(())
                        }
                        _ => Err(mlua::Error::RuntimeError(
                            "Mouse.Cursor expects a string".into(),
                        )),
                    },
                    "Position" | "Moved" | "InputReceived" | "Scrolled" => Err(
                        mlua::Error::RuntimeError(format!("Mouse.{key} is read-only")),
                    ),
                    other => Err(mlua::Error::RuntimeError(format!(
                        "Mouse: unknown property '{other}'"
                    ))),
                }
            },
        )?,
    )?;
    t.set_metatable(Some(meta));
    Ok(t)
}
