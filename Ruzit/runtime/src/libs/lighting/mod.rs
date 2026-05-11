use mlua::{AnyUserData, Lua, Table};

use crate::libs::primitives::{Color3, Vector};
use crate::libs::renderable;

mod lights;
pub use lights::{LightHandle, list_lights, pack_lights};

pub fn create(lua: &Lua) -> mlua::Result<Table> {
    let t = lua.create_table()?;

    t.set(
        "SetSun",
        lua.create_function(
            |_, (dir, color): (Vector, Option<AnyUserData>)| -> mlua::Result<()> {
                let c = match color {
                    Some(ud) => Some(*ud.borrow::<Color3>()?),
                    None => None,
                };
                renderable::set_sun(dir, c);
                Ok(())
            },
        )?,
    )?;
    t.set(
        "SetSunDirection",
        lua.create_function(|_, dir: Vector| -> mlua::Result<()> {
            renderable::set_sun(dir, None);
            Ok(())
        })?,
    )?;
    t.set(
        "SetSunColor",
        lua.create_function(|_, color: AnyUserData| -> mlua::Result<()> {
            let c = *color.borrow::<Color3>()?;
            renderable::set_sun_color(c);
            Ok(())
        })?,
    )?;
    t.set(
        "SetAmbient",
        lua.create_function(|_, color: AnyUserData| -> mlua::Result<()> {
            let c = *color.borrow::<Color3>()?;
            renderable::set_ambient(c);
            Ok(())
        })?,
    )?;
    t.set(
        "GetSunDirection",
        lua.create_function(|_, _: ()| -> mlua::Result<Vector> {
            Ok(renderable::lighting_snapshot().sun_direction)
        })?,
    )?;
    t.set(
        "GetSunColor",
        lua.create_function(|_, _: ()| -> mlua::Result<Color3> {
            Ok(renderable::lighting_snapshot().sun_color)
        })?,
    )?;
    t.set(
        "GetAmbient",
        lua.create_function(|_, _: ()| -> mlua::Result<Color3> {
            Ok(renderable::lighting_snapshot().ambient)
        })?,
    )?;
    t.set(
        "Get",
        lua.create_function(|lua, _: ()| -> mlua::Result<Table> {
            let snap = renderable::lighting_snapshot();
            let out = lua.create_table()?;
            out.set("SunDirection", snap.sun_direction)?;
            out.set("SunColor", snap.sun_color)?;
            out.set("Ambient", snap.ambient)?;
            Ok(out)
        })?,
    )?;

    t.set(
        "AddObject",
        lua.create_function(|_, part: AnyUserData| -> mlua::Result<()> {
            let handle = part.borrow::<renderable::PartHandle>().map_err(|_| {
                mlua::Error::RuntimeError(
                    "LightingService:AddObject expects a BasePart".into(),
                )
            })?;
            handle.state.lock().unwrap().lit = true;
            renderable::bump_parts_dirty();
            Ok(())
        })?,
    )?;
    t.set(
        "RemoveObject",
        lua.create_function(|_, part: AnyUserData| -> mlua::Result<()> {
            let handle = part.borrow::<renderable::PartHandle>().map_err(|_| {
                mlua::Error::RuntimeError(
                    "LightingService:RemoveObject expects a BasePart".into(),
                )
            })?;
            handle.state.lock().unwrap().lit = false;
            renderable::bump_parts_dirty();
            Ok(())
        })?,
    )?;
    t.set(
        "IsObjectManaged",
        lua.create_function(|_, part: AnyUserData| -> mlua::Result<bool> {
            let handle = part.borrow::<renderable::PartHandle>().map_err(|_| {
                mlua::Error::RuntimeError(
                    "LightingService:IsObjectManaged expects a BasePart".into(),
                )
            })?;
            Ok(handle.state.lock().unwrap().lit)
        })?,
    )?;
    t.set(
        "GenerateLightSource",
        lua.create_function(lights::generate_light_source)?,
    )?;
    t.set(
        "GetActiveLights",
        lua.create_function(|lua, _: ()| lights::active_lights_table(lua))?,
    )?;
    t.set(
        "GetLightCount",
        lua.create_function(|_, _: ()| -> mlua::Result<i64> {
            Ok(lights::list_lights().len() as i64)
        })?,
    )?;
    t.set(
        "UploadLights",
        lua.create_function(
            |_, buf: mlua::AnyUserData| -> mlua::Result<i64> {
                let buffer = buf.borrow::<crate::libs::gpu::GPUBuffer>().map_err(|_| {
                    mlua::Error::RuntimeError(
                        "LightingService.UploadLights: expected a GPUBuffer (allocate with GPU.NewBuffer)".into(),
                    )
                })?;
                let queue = crate::libs::gui::render::GPU_QUEUE.get().ok_or_else(|| {
                    mlua::Error::RuntimeError(
                        "LightingService.UploadLights: GPU not initialized (open a window first)".into(),
                    )
                })?;
                let (count, packed) = lights::pack_lights();
                let mut payload: Vec<f32> = Vec::with_capacity(packed.len() + 4);
                payload.push(f32::from_bits(count));
                payload.push(0.0);
                payload.push(0.0);
                payload.push(0.0);
                payload.extend_from_slice(&packed);
                if payload.len() > buffer.floats() {
                    return Err(mlua::Error::RuntimeError(format!(
                        "LightingService.UploadLights: buffer too small. Need {} floats (16 per light + 4 header), buffer has {}",
                        payload.len(),
                        buffer.floats()
                    )));
                }
                let bytes: &[u8] = bytemuck::cast_slice(&payload);
                queue.write_buffer(buffer.raw(), 0, bytes);
                Ok(count as i64)
            },
        )?,
    )?;

    Ok(t)
}
