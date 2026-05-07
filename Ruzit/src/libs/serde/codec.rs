use mlua::{Function, Lua, Table, Value};
use serde_json::Value as JsonValue;

pub fn encode_fn(lua: &Lua) -> mlua::Result<Function> {
    lua.create_function(
        |_, (format, data, pretty): (String, Value, Option<bool>)| -> mlua::Result<String> {
            let json = lua_to_json(data)?;
            let pretty = pretty.unwrap_or(false);
            match format.to_lowercase().as_str() {
                "json" => {
                    if pretty {
                        serde_json::to_string_pretty(&json).map_err(rt)
                    } else {
                        serde_json::to_string(&json).map_err(rt)
                    }
                }
                "toml" => toml::to_string(&json).map_err(rt),
                "yaml" | "yml" => serde_yaml::to_string(&json).map_err(rt),
                other => Err(mlua::Error::RuntimeError(format!(
                    "Serde.Encode: unknown format '{other}'"
                ))),
            }
        },
    )
}

pub fn decode_fn(lua: &Lua) -> mlua::Result<Function> {
    lua.create_function(
        |lua, (format, text): (String, String)| -> mlua::Result<Value> {
            let json: JsonValue = match format.to_lowercase().as_str() {
                "json" => serde_json::from_str(&text).map_err(rt)?,
                "toml" => toml::from_str(&text).map_err(rt)?,
                "yaml" | "yml" => serde_yaml::from_str(&text).map_err(rt)?,
                other => {
                    return Err(mlua::Error::RuntimeError(format!(
                        "Serde.Decode: unknown format '{other}'"
                    )));
                }
            };
            json_to_lua(lua, &json)
        },
    )
}

pub(crate) fn rt<E: std::fmt::Display>(e: E) -> mlua::Error {
    mlua::Error::RuntimeError(e.to_string())
}

fn lua_to_json(v: Value) -> mlua::Result<JsonValue> {
    match v {
        Value::Nil => Ok(JsonValue::Null),
        Value::Boolean(b) => Ok(JsonValue::Bool(b)),
        Value::Integer(i) => Ok(JsonValue::Number(i.into())),
        Value::Number(n) => Ok(serde_json::Number::from_f64(n)
            .map(JsonValue::Number)
            .unwrap_or(JsonValue::Null)),
        Value::String(s) => Ok(JsonValue::String(s.to_str()?.to_string())),
        Value::Table(t) => table_to_json(&t),
        other => Err(mlua::Error::RuntimeError(format!(
            "Serde: unsupported value type '{}'",
            other.type_name()
        ))),
    }
}

fn table_to_json(t: &Table) -> mlua::Result<JsonValue> {
    let len = t.raw_len();
    let mut numeric_keys = 0usize;
    let mut total_keys = 0usize;
    for pair in t.pairs::<Value, Value>() {
        let (k, _) = pair?;
        total_keys += 1;
        if matches!(k, Value::Integer(_)) {
            numeric_keys += 1;
        }
    }

    let is_array = total_keys > 0 && numeric_keys == total_keys && len as usize == total_keys;

    if is_array {
        let mut arr = Vec::with_capacity(len as usize);
        for i in 1..=len {
            let v: Value = t.raw_get(i)?;
            arr.push(lua_to_json(v)?);
        }
        Ok(JsonValue::Array(arr))
    } else {
        let mut map = serde_json::Map::new();
        for pair in t.pairs::<Value, Value>() {
            let (k, v) = pair?;
            let key = match k {
                Value::String(s) => s.to_str()?.to_string(),
                Value::Integer(i) => i.to_string(),
                Value::Number(n) => n.to_string(),
                Value::Boolean(b) => b.to_string(),
                other => {
                    return Err(mlua::Error::RuntimeError(format!(
                        "Serde: unsupported key type '{}'",
                        other.type_name()
                    )));
                }
            };
            map.insert(key, lua_to_json(v)?);
        }
        Ok(JsonValue::Object(map))
    }
}

fn json_to_lua(lua: &Lua, v: &JsonValue) -> mlua::Result<Value> {
    match v {
        JsonValue::Null => Ok(Value::Nil),
        JsonValue::Bool(b) => Ok(Value::Boolean(*b)),
        JsonValue::Number(n) => {
            if let Some(f) = n.as_f64() {
                Ok(Value::Number(f))
            } else {
                Ok(Value::Nil)
            }
        }
        JsonValue::String(s) => Ok(Value::String(lua.create_string(s)?)),
        JsonValue::Array(arr) => {
            let t = lua.create_table()?;
            for (i, item) in arr.iter().enumerate() {
                t.set((i + 1) as i64, json_to_lua(lua, item)?)?;
            }
            Ok(Value::Table(t))
        }
        JsonValue::Object(obj) => {
            let t = lua.create_table()?;
            for (k, v) in obj {
                t.set(k.clone(), json_to_lua(lua, v)?)?;
            }
            Ok(Value::Table(t))
        }
    }
}
