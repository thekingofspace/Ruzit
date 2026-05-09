use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

use mlua::{Function, Lua, Table, Value};

pub const HEART_KEY: &str = "ruzit_heart";
const TICK_HZ: u64 = 60;
const TICK_INTERVAL: Duration = Duration::from_micros(1_000_000 / TICK_HZ);

static MAX_FPS: AtomicU32 = AtomicU32::new(0);

pub fn set_max_fps(fps: Option<u32>) {
    MAX_FPS.store(fps.unwrap_or(0), Ordering::Relaxed);
}

pub fn get_max_fps() -> Option<u32> {
    let v = MAX_FPS.load(Ordering::Relaxed);
    if v == 0 { None } else { Some(v) }
}

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
        crate::libs::window::pump(lua);
        crate::libs::input::pump(lua);
        crate::libs::gamepad::pump(lua);
        crate::libs::vr::pump(lua);
        crate::libs::sfx::pump(lua);
        crate::libs::steam::pump(lua);
        crate::libs::voice::pump(lua);
        crate::libs::asset::pump(lua);
        crate::libs::debug::pump(lua);
        crate::libs::gpu::record_frame();

        let snapshot = snapshot_handlers(lua)?;
        let window_open = crate::libs::window::is_open();
        let sfx_active = crate::libs::sfx::is_active();
        let voice_active = crate::libs::voice::is_active();

        if snapshot.is_empty() && !window_open && !sfx_active && !voice_active {
            return Ok(());
        }

        let now = Instant::now();
        let dt = (now - last).as_secs_f64();
        last = now;
        crate::libs::debug::record_frame(dt * 1000.0);

        crate::libs::runservice::fire_render_stepped(lua, dt);
        crate::libs::renderable::tick_animations(lua, dt as f32);
        crate::libs::renderable::tick_distortion_boxes();
        crate::libs::dynmesh::tick();
        crate::libs::dynimg::tick();
        crate::libs::video::tick(dt * 1000.0);

        for (id, func) in snapshot {
            if let Err(e) = run_one(lua, &func, dt) {
                let pretty = crate::errors::pretty_format_loose(lua, &id, &e);
                eprintln!("[Ruzit] heart '{id}' error: {pretty}");
                let registry: Table = lua.named_registry_value(HEART_KEY)?;
                registry.set(id, Value::Nil)?;
            }
        }

        crate::libs::runservice::fire_heartbeat(lua, dt);

        let target_interval = match get_max_fps() {
            Some(fps) if fps > 0 => Duration::from_micros(1_000_000 / fps as u64),
            _ => TICK_INTERVAL,
        };
        let elapsed = last.elapsed();
        if elapsed < target_interval {
            std::thread::sleep(target_interval - elapsed);
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
