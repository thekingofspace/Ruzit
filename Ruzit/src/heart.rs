use std::time::{Duration, Instant};

use mlua::{Function, Lua, Table, Value};

pub const HEART_KEY: &str = "ruzit_heart";
const TICK_HZ: u64 = 60;
const TICK_INTERVAL: Duration = Duration::from_micros(1_000_000 / TICK_HZ);

pub fn ensure_registry(lua: &Lua) -> mlua::Result<()> {
    let existing: Value = lua.named_registry_value(HEART_KEY)?;
    if matches!(existing, Value::Nil) {
        let registry = lua.create_table()?;
        lua.set_named_registry_value(HEART_KEY, registry)?;
    }
    Ok(())
}

pub fn run_loop(lua: &Lua) -> mlua::Result<()> {
    let mut last = Instant::now();
    loop {
        // Pump window events first; this can call std::process::exit if the user
        // closed the window (after running BindToClose, if registered).
        crate::libs::window::pump(lua);
        crate::libs::sfx::pump(lua);

        let snapshot = snapshot_handlers(lua)?;
        let window_open = crate::libs::window::is_open();
        let sfx_active = crate::libs::sfx::is_active();

        if snapshot.is_empty() && !window_open && !sfx_active {
            return Ok(());
        }

        let now = Instant::now();
        let dt = (now - last).as_secs_f64();
        last = now;

        for (id, func) in snapshot {
            if let Err(e) = run_one(lua, &func, dt) {
                eprintln!("[Ruzit] heart '{id}' error: {e}");
                let registry: Table = lua.named_registry_value(HEART_KEY)?;
                registry.set(id, Value::Nil)?;
            }
        }

        let elapsed = last.elapsed();
        if elapsed < TICK_INTERVAL {
            std::thread::sleep(TICK_INTERVAL - elapsed);
        }
    }
}

fn snapshot_handlers(lua: &Lua) -> mlua::Result<Vec<(String, Function)>> {
    let registry: Table = lua.named_registry_value(HEART_KEY)?;
    let mut out = Vec::new();
    for pair in registry.pairs::<String, Function>() {
        out.push(pair?);
    }
    Ok(out)
}

fn run_one(lua: &Lua, func: &Function, dt: f64) -> mlua::Result<()> {
    let thread = lua.create_thread(func.clone())?;
    let _: mlua::MultiValue = thread.resume(dt)?;
    Ok(())
}
