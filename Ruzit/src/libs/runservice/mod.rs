//! Familiar Roblox-style heartbeat / render-step facade. Both signals fire
//! once per heart tick with the elapsed `dt` (seconds). RenderStepped fires
//! before user-bound heart handlers; Heartbeat fires after.

use std::cell::RefCell;

use mlua::{Lua, MultiValue, Table, Value};

use crate::libs::signal;

const HEARTBEAT_KEY: &str = "ruzit_runservice_heartbeat";
const RENDERSTEPPED_KEY: &str = "ruzit_runservice_renderstepped";

thread_local! {
    static INSTALLED: RefCell<bool> = const { RefCell::new(false) };
}

pub fn install(lua: &Lua) -> mlua::Result<()> {
    let already = INSTALLED.with(|c| *c.borrow());
    if already {
        return Ok(());
    }
    let heartbeat = signal::new_instance(lua)?;
    let render_stepped = signal::new_instance(lua)?;
    lua.set_named_registry_value(HEARTBEAT_KEY, heartbeat)?;
    lua.set_named_registry_value(RENDERSTEPPED_KEY, render_stepped)?;
    INSTALLED.with(|c| *c.borrow_mut() = true);
    Ok(())
}

pub fn is_installed() -> bool {
    INSTALLED.with(|c| *c.borrow())
}

/// Fired by the heart loop once per tick. RenderStepped goes first so movement
/// + render-driven code that wants to read input and write GUI state can do
/// so before any user-bound `Process.BindToHeart` handlers run.
pub fn fire_render_stepped(lua: &Lua, dt: f64) {
    if !is_installed() {
        return;
    }
    let Ok(sig) = lua.named_registry_value::<Table>(RENDERSTEPPED_KEY) else {
        return;
    };
    let mut args = MultiValue::new();
    args.push_back(Value::Number(dt));
    if let Err(e) = signal::fire(lua, &sig, args) {
        eprintln!("[RunService] RenderStepped fire: {e}");
    }
}

pub fn fire_heartbeat(lua: &Lua, dt: f64) {
    if !is_installed() {
        return;
    }
    let Ok(sig) = lua.named_registry_value::<Table>(HEARTBEAT_KEY) else {
        return;
    };
    let mut args = MultiValue::new();
    args.push_back(Value::Number(dt));
    if let Err(e) = signal::fire(lua, &sig, args) {
        eprintln!("[RunService] Heartbeat fire: {e}");
    }
}

pub fn create(lua: &Lua) -> mlua::Result<Table> {
    let t = lua.create_table()?;

    let heartbeat: Table = lua.named_registry_value(HEARTBEAT_KEY)?;
    let render_stepped: Table = lua.named_registry_value(RENDERSTEPPED_KEY)?;
    t.set("Heartbeat", heartbeat)?;
    t.set("RenderStepped", render_stepped)?;

    Ok(t)
}
