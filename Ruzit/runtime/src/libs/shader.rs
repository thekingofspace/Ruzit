use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use mlua::AnyUserData;

use crate::libs::asset::{FragmentAsset, ShaderAsset};

pub type Params = Arc<Mutex<HashMap<String, f32>>>;

#[derive(Clone)]
pub struct AttachedShader {
    pub id: u64,
    pub kind: String,
    pub params: Params,
    /// Position in the audio DSP chain. Lower = earlier (closer to source);
    /// higher = later (closer to output). Defaults to 0; ties preserve the
    /// order AttachShader was called in.
    pub priority: i32,
}

pub fn read_param(params: &Params, key: &str, default: f32) -> f32 {
    params.lock().unwrap().get(key).copied().unwrap_or(default)
}

pub fn parse_shader(code: &str, source: &str) -> mlua::Result<(String, HashMap<String, f32>)> {
    let mut iter = code.lines().filter_map(|raw| {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with("//") {
            None
        } else {
            Some(line)
        }
    });
    let kind = iter
        .next()
        .ok_or_else(|| {
            mlua::Error::RuntimeError(format!(
                "shader '{source}' is empty — first line must be the effect kind"
            ))
        })?
        .to_string();
    let mut params = HashMap::new();
    for line in iter {
        if let Some((k, v)) = line.split_once('=') {
            if let Ok(n) = v.trim().parse::<f32>() {
                params.insert(k.trim().to_string(), n);
            }
        }
    }
    Ok((kind, params))
}

pub fn shader_id(asset: &AnyUserData) -> mlua::Result<u64> {
    if let Ok(s) = asset.borrow::<ShaderAsset>() {
        return Ok(s.id);
    }
    if let Ok(f) = asset.borrow::<FragmentAsset>() {
        return Ok(f.id);
    }
    Err(mlua::Error::RuntimeError(
        "expected a Shader or Fragment asset".into(),
    ))
}

pub fn shader_attach_spec(
    asset: &AnyUserData,
    priority: i32,
) -> mlua::Result<AttachedShader> {
    if let Ok(s) = asset.borrow::<ShaderAsset>() {
        let (kind, params) = parse_shader(&s.code, &s.source)?;
        return Ok(AttachedShader {
            id: s.id,
            kind,
            params: Arc::new(Mutex::new(params)),
            priority,
        });
    }
    if let Ok(f) = asset.borrow::<FragmentAsset>() {
        let (kind, params) = parse_shader(&f.code, &f.source)?;
        return Ok(AttachedShader {
            id: f.id,
            kind,
            params: Arc::new(Mutex::new(params)),
            priority,
        });
    }
    Err(mlua::Error::RuntimeError(
        "expected a Shader or Fragment asset".into(),
    ))
}
