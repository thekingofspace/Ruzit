use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

use gilrs::ff::{BaseEffect, BaseEffectType, Effect, EffectBuilder, Replay, Ticks};
use gilrs::{Axis, Button, Event, EventType, Gamepad as GpHandle, GamepadId, Gilrs};
use mlua::{Lua, MultiValue, Table, Value};

use crate::libs::signal;

const SIGNALS_KEY: &str = "ruzit_gamepad_signals";

struct PadState {
    axes: HashMap<&'static str, f32>,

    buttons_down: std::collections::HashSet<String>,
}

static GILRS: OnceLock<Mutex<Option<Gilrs>>> = OnceLock::new();
static PADS: OnceLock<Mutex<HashMap<u32, PadState>>> = OnceLock::new();

static FF_ACTIVE: OnceLock<Mutex<Vec<(Effect, Instant)>>> = OnceLock::new();

fn gilrs_lock() -> &'static Mutex<Option<Gilrs>> {
    GILRS.get_or_init(|| Mutex::new(None))
}

fn pads_lock() -> &'static Mutex<HashMap<u32, PadState>> {
    PADS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn ff_active_lock() -> &'static Mutex<Vec<(Effect, Instant)>> {
    FF_ACTIVE.get_or_init(|| Mutex::new(Vec::new()))
}

fn pad_index(id: GamepadId) -> u32 {
    let s = format!("{id}");
    s.parse().unwrap_or(0)
}

fn button_name(b: Button) -> Option<&'static str> {
    Some(match b {
        Button::South => "South",
        Button::East => "East",
        Button::North => "North",
        Button::West => "West",
        Button::C => "C",
        Button::Z => "Z",
        Button::LeftTrigger => "LeftBumper",
        Button::LeftTrigger2 => "LeftTrigger",
        Button::RightTrigger => "RightBumper",
        Button::RightTrigger2 => "RightTrigger",
        Button::Select => "Select",
        Button::Start => "Start",
        Button::Mode => "Mode",
        Button::LeftThumb => "LeftStick",
        Button::RightThumb => "RightStick",
        Button::DPadUp => "DPadUp",
        Button::DPadDown => "DPadDown",
        Button::DPadLeft => "DPadLeft",
        Button::DPadRight => "DPadRight",
        Button::Unknown => return None,
    })
}

fn button_from_name(name: &str) -> Option<Button> {
    Some(match name {
        "South" | "A" => Button::South,
        "East" | "B" => Button::East,
        "North" | "Y" => Button::North,
        "West" | "X" => Button::West,
        "LeftBumper" => Button::LeftTrigger,
        "LeftTrigger" => Button::LeftTrigger2,
        "RightBumper" => Button::RightTrigger,
        "RightTrigger" => Button::RightTrigger2,
        "Select" => Button::Select,
        "Start" => Button::Start,
        "Mode" => Button::Mode,
        "LeftStick" => Button::LeftThumb,
        "RightStick" => Button::RightThumb,
        "DPadUp" => Button::DPadUp,
        "DPadDown" => Button::DPadDown,
        "DPadLeft" => Button::DPadLeft,
        "DPadRight" => Button::DPadRight,
        _ => return None,
    })
}

fn axis_name(a: Axis) -> Option<&'static str> {
    Some(match a {
        Axis::LeftStickX => "LeftStickX",
        Axis::LeftStickY => "LeftStickY",
        Axis::RightStickX => "RightStickX",
        Axis::RightStickY => "RightStickY",
        Axis::LeftZ => "LeftZ",
        Axis::RightZ => "RightZ",
        Axis::DPadX => "DPadX",
        Axis::DPadY => "DPadY",
        Axis::Unknown => return None,
    })
}

pub fn pump(lua: &Lua) {
    {
        let now = Instant::now();
        let mut active = ff_active_lock().lock().unwrap();
        active.retain(|(_, deadline)| *deadline > now);
    }

    let gilrs_cell = gilrs_lock();
    let mut guard = gilrs_cell.lock().unwrap();
    let Some(gilrs) = guard.as_mut() else {
        return;
    };
    let pads_mutex = pads_lock();

    let mut events: Vec<(u32, EventType)> = Vec::new();
    while let Some(Event { id, event, .. }) = gilrs.next_event() {
        events.push((pad_index(id), event));
    }
    drop(guard);

    let signals: Table = match lua.named_registry_value(SIGNALS_KEY) {
        Ok(t) => t,
        Err(_) => return,
    };

    for (idx, ev) in events {
        let mut pads = pads_mutex.lock().unwrap();
        let entry = pads.entry(idx).or_insert_with(|| PadState {
            axes: HashMap::new(),
            buttons_down: Default::default(),
        });
        match ev {
            EventType::Connected => {
                drop(pads);
                if let Ok(sig) = signals.get::<Table>("Connected") {
                    let mut args = MultiValue::new();
                    args.push_back(Value::Integer(idx as i32));
                    let _ = signal::fire(lua, &sig, args);
                }
            }
            EventType::Disconnected => {
                pads.remove(&idx);
                drop(pads);
                if let Ok(sig) = signals.get::<Table>("Disconnected") {
                    let mut args = MultiValue::new();
                    args.push_back(Value::Integer(idx as i32));
                    let _ = signal::fire(lua, &sig, args);
                }
            }
            EventType::ButtonPressed(btn, _) => {
                if let Some(name) = button_name(btn) {
                    entry.buttons_down.insert(name.to_string());
                    drop(pads);
                    fire_input(lua, &signals, idx, "Button", name, 1.0);
                }
            }
            EventType::ButtonReleased(btn, _) => {
                if let Some(name) = button_name(btn) {
                    entry.buttons_down.remove(name);
                    drop(pads);
                    fire_input(lua, &signals, idx, "Button", name, 0.0);
                }
            }
            EventType::ButtonChanged(btn, value, _) => {
                if let Some(name) = button_name(btn) {
                    drop(pads);
                    fire_input(lua, &signals, idx, "Analog", name, value);
                }
            }
            EventType::AxisChanged(axis, value, _) => {
                if let Some(name) = axis_name(axis) {
                    entry.axes.insert(name, value);
                    drop(pads);
                    fire_input(lua, &signals, idx, "Axis", name, value);
                }
            }
            _ => {}
        }
    }
}

fn fire_input(lua: &Lua, signals: &Table, idx: u32, kind: &str, name: &str, value: f32) {
    let Ok(sig) = signals.get::<Table>("InputChanged") else {
        return;
    };
    let payload = match lua.create_table() {
        Ok(t) => t,
        Err(_) => return,
    };
    let _ = payload.set("Pad", idx as i64);
    let _ = payload.set("Kind", kind);
    let _ = payload.set("Name", name);
    let _ = payload.set("Value", value);

    let state = match kind {
        "Button" => {
            if value > 0.5 {
                "Begin"
            } else {
                "End"
            }
        }
        "Analog" => {
            if value > 0.5 {
                "Begin"
            } else {
                "End"
            }
        }
        _ => "Change",
    };
    let _ = payload.set("State", state);
    let mut args = MultiValue::new();
    args.push_back(Value::Table(payload));
    let _ = signal::fire(lua, &sig, args);
}

pub fn create(lua: &Lua) -> mlua::Result<Table> {
    {
        let cell = gilrs_lock();
        let mut guard = cell.lock().unwrap();
        if guard.is_none() {
            match Gilrs::new() {
                Ok(g) => *guard = Some(g),
                Err(e) => {
                    eprintln!(
                        "[Gamepad] could not initialise gilrs: {e}; controller events disabled"
                    );
                }
            }
        }
    }

    let signals: Table = match lua.named_registry_value::<Value>(SIGNALS_KEY)? {
        Value::Table(t) => t,
        _ => {
            let t = lua.create_table()?;
            t.set("Connected", signal::new_instance(lua)?)?;
            t.set("Disconnected", signal::new_instance(lua)?)?;
            t.set("InputChanged", signal::new_instance(lua)?)?;
            lua.set_named_registry_value(SIGNALS_KEY, t.clone())?;
            t
        }
    };

    let api = lua.create_table()?;
    api.set("Connected", signals.get::<Table>("Connected")?)?;
    api.set("Disconnected", signals.get::<Table>("Disconnected")?)?;
    api.set("InputChanged", signals.get::<Table>("InputChanged")?)?;

    api.set(
        "GetGamepads",
        lua.create_function(|lua, _: ()| -> mlua::Result<Table> {
            let out = lua.create_table()?;
            let cell = gilrs_lock();
            let guard = cell.lock().unwrap();
            let Some(g) = guard.as_ref() else {
                return Ok(out);
            };
            for (id, pad) in g.gamepads() {
                let entry = lua.create_table()?;
                entry.set("Id", pad_index(id) as i64)?;
                entry.set("Name", pad.name().to_string())?;
                entry.set("Connected", pad.is_connected())?;
                let uuid_bytes = pad.uuid();
                let uuid_hex: String = uuid_bytes.iter().map(|b| format!("{b:02x}")).collect();
                entry.set("UUID", uuid_hex)?;
                out.set(out.raw_len() + 1, entry)?;
            }
            Ok(out)
        })?,
    )?;

    api.set(
        "IsButtonDown",
        lua.create_function(|_, (pad, name): (i64, String)| -> mlua::Result<bool> {
            let cell = gilrs_lock();
            let guard = cell.lock().unwrap();
            let Some(g) = guard.as_ref() else {
                return Ok(false);
            };
            let Some(btn) = button_from_name(&name) else {
                return Ok(false);
            };
            for (id, gp) in g.gamepads() {
                if pad_index(id) as i64 == pad {
                    return Ok(gp.is_pressed(btn));
                }
            }
            Ok(false)
        })?,
    )?;

    api.set(
        "GetAxis",
        lua.create_function(|_, (pad, name): (i64, String)| -> mlua::Result<f32> {
            let pads = pads_lock().lock().unwrap();
            if let Some(p) = pads.get(&(pad as u32)) {
                if let Some(v) = p.axes.get(name.as_str()) {
                    return Ok(*v);
                }
            }

            let cell = gilrs_lock();
            let guard = cell.lock().unwrap();
            let Some(g) = guard.as_ref() else {
                return Ok(0.0);
            };
            let axis_kind = match name.as_str() {
                "LeftStickX" => Axis::LeftStickX,
                "LeftStickY" => Axis::LeftStickY,
                "RightStickX" => Axis::RightStickX,
                "RightStickY" => Axis::RightStickY,
                "LeftZ" => Axis::LeftZ,
                "RightZ" => Axis::RightZ,
                _ => return Ok(0.0),
            };
            for (id, gp) in g.gamepads() {
                if pad_index(id) as i64 == pad {
                    return Ok(gp.axis_data(axis_kind).map(|d| d.value()).unwrap_or(0.0));
                }
            }
            Ok(0.0)
        })?,
    )?;

    api.set(
        "SetVibration",
        lua.create_function(
            |_, (pad, low, high, duration): (i64, f32, f32, f32)| -> mlua::Result<bool> {
                Ok(start_rumble(pad, low, high, duration))
            },
        )?,
    )?;
    api.set(
        "Vibrate",
        lua.create_function(
            |_, (pad, intensity, duration): (i64, f32, f32)| -> mlua::Result<bool> {
                Ok(start_rumble(pad, intensity, intensity, duration))
            },
        )?,
    )?;
    api.set(
        "StopVibration",
        lua.create_function(|_, _pad: Option<i64>| -> mlua::Result<()> {
            stop_all_rumble();
            Ok(())
        })?,
    )?;
    api.set(
        "GetBatteryLevel",
        lua.create_function(|_, pad: i64| -> mlua::Result<Option<f32>> {
            let cell = gilrs_lock();
            let guard = cell.lock().unwrap();
            let Some(g) = guard.as_ref() else {
                return Ok(None);
            };
            for (id, gp) in g.gamepads() {
                if pad_index(id) as i64 == pad {
                    return Ok(battery_level(&gp));
                }
            }
            Ok(None)
        })?,
    )?;

    Ok(api)
}

fn start_rumble(pad: i64, low: f32, high: f32, duration: f32) -> bool {
    let cell = gilrs_lock();
    let mut guard = cell.lock().unwrap();
    let Some(g) = guard.as_mut() else {
        return false;
    };

    let mut target: Option<GamepadId> = None;
    let mut supports_ff = false;
    for (id, gp) in g.gamepads() {
        if pad_index(id) as i64 == pad {
            target = Some(id);
            supports_ff = gp.is_ff_supported();
            break;
        }
    }
    let Some(id) = target else {
        return false;
    };
    if !supports_ff {
        return false;
    }

    let dur_ms = (duration * 1000.0).clamp(0.0, 60_000.0) as u32;
    if dur_ms == 0 {
        return false;
    }
    let ticks = Ticks::from_ms(dur_ms);

    let strong = (low.clamp(0.0, 1.0) * u16::MAX as f32) as u16;
    let weak = (high.clamp(0.0, 1.0) * u16::MAX as f32) as u16;

    let mut builder = EffectBuilder::new();
    let pads = [id];
    builder.gamepads(&pads);
    if strong > 0 {
        builder.add_effect(BaseEffect {
            kind: BaseEffectType::Strong { magnitude: strong },
            scheduling: Replay {
                play_for: ticks,
                with_delay: Ticks::from_ms(0),
                ..Default::default()
            },
            envelope: Default::default(),
        });
    }
    if weak > 0 {
        builder.add_effect(BaseEffect {
            kind: BaseEffectType::Weak { magnitude: weak },
            scheduling: Replay {
                play_for: ticks,
                with_delay: Ticks::from_ms(0),
                ..Default::default()
            },
            envelope: Default::default(),
        });
    }
    let effect = match builder.finish(g) {
        Ok(e) => e,
        Err(_) => return false,
    };
    if effect.play().is_err() {
        return false;
    }
    let deadline = Instant::now() + std::time::Duration::from_millis(dur_ms as u64 + 50);
    ff_active_lock().lock().unwrap().push((effect, deadline));
    true
}

fn stop_all_rumble() {
    ff_active_lock().lock().unwrap().clear();
}

fn battery_level(gp: &GpHandle<'_>) -> Option<f32> {
    use gilrs::PowerInfo;
    match gp.power_info() {
        PowerInfo::Discharging(p) | PowerInfo::Charging(p) => Some(p as f32 / 100.0),
        _ => None,
    }
}

#[allow(dead_code)]
fn _gp_helper(_g: &GpHandle<'_>) {}
