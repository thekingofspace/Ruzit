use std::cell::RefCell;
use std::collections::HashMap;

use mlua::{Function, Lua, RegistryKey, Table};

thread_local! {
    static STATE: RefCell<State> = RefCell::new(State::new());
}

struct State {
    clock: f64,
    next_anon: u64,
    delays: Vec<DelayEntry>,
    scheduled: HashMap<String, ScheduleEntry>,
    repeatables: HashMap<String, RepeatEntry>,
}

struct DelayEntry {
    fire_at: f64,
    cb: RegistryKey,
}

struct ScheduleEntry {
    fire_at: f64,
    cb: RegistryKey,
}

struct RepeatEntry {
    next_at: f64,
    interval: f64,
    cb: RegistryKey,
}

impl State {
    fn new() -> Self {
        State {
            clock: 0.0,
            next_anon: 0,
            delays: Vec::new(),
            scheduled: HashMap::new(),
            repeatables: HashMap::new(),
        }
    }

    fn anon_id(&mut self, prefix: &str) -> String {
        self.next_anon += 1;
        format!("__{prefix}_{}", self.next_anon)
    }
}

pub fn create(lua: &Lua) -> mlua::Result<Table> {
    let t = lua.create_table()?;

    t.set(
        "Spawn",
        lua.create_function(|lua, func: Function| -> mlua::Result<()> {
            let key = lua.create_registry_value(func)?;
            STATE.with(|s| {
                let mut state = s.borrow_mut();
                let fire_at = state.clock;
                state.delays.push(DelayEntry { fire_at, cb: key });
            });
            Ok(())
        })?,
    )?;

    t.set(
        "Delay",
        lua.create_function(|lua, (seconds, func): (f64, Function)| -> mlua::Result<()> {
            let secs = seconds.max(0.0);
            let key = lua.create_registry_value(func)?;
            STATE.with(|s| {
                let mut state = s.borrow_mut();
                let fire_at = state.clock + secs;
                state.delays.push(DelayEntry { fire_at, cb: key });
            });
            Ok(())
        })?,
    )?;

    t.set(
        "Schedule",
        lua.create_function(
            |lua,
             (id, seconds, func): (Option<String>, f64, Function)|
             -> mlua::Result<String> {
                let secs = seconds.max(0.0);
                let key = lua.create_registry_value(func)?;
                let resolved_id = STATE.with(|s| -> mlua::Result<String> {
                    let mut state = s.borrow_mut();
                    let id = id.unwrap_or_else(|| state.anon_id("sched"));
                    if let Some(prev) = state.scheduled.remove(&id) {
                        let _ = lua.remove_registry_value(prev.cb);
                    }
                    let fire_at = state.clock + secs;
                    state.scheduled.insert(
                        id.clone(),
                        ScheduleEntry { fire_at, cb: key },
                    );
                    Ok(id)
                })?;
                Ok(resolved_id)
            },
        )?,
    )?;

    t.set(
        "Reschedule",
        lua.create_function(|_, (id, seconds): (String, f64)| -> mlua::Result<bool> {
            let secs = seconds.max(0.0);
            Ok(STATE.with(|s| {
                let mut state = s.borrow_mut();
                let now = state.clock;
                if let Some(entry) = state.scheduled.get_mut(&id) {
                    entry.fire_at = now + secs;
                    return true;
                }
                false
            }))
        })?,
    )?;

    t.set(
        "Repeat",
        lua.create_function(
            |lua,
             (id, interval, func): (Option<String>, f64, Function)|
             -> mlua::Result<String> {
                let interval = interval.max(0.0);
                if interval <= 0.0 {
                    return Err(mlua::Error::RuntimeError(
                        "Task.Repeat: interval must be > 0".into(),
                    ));
                }
                let key = lua.create_registry_value(func)?;
                let resolved_id = STATE.with(|s| -> mlua::Result<String> {
                    let mut state = s.borrow_mut();
                    let id = id.unwrap_or_else(|| state.anon_id("rep"));
                    if let Some(prev) = state.repeatables.remove(&id) {
                        let _ = lua.remove_registry_value(prev.cb);
                    }
                    let next_at = state.clock + interval;
                    state.repeatables.insert(
                        id.clone(),
                        RepeatEntry {
                            next_at,
                            interval,
                            cb: key,
                        },
                    );
                    Ok(id)
                })?;
                Ok(resolved_id)
            },
        )?,
    )?;

    t.set(
        "SetInterval",
        lua.create_function(|_, (id, interval): (String, f64)| -> mlua::Result<bool> {
            if interval <= 0.0 {
                return Err(mlua::Error::RuntimeError(
                    "Task.SetInterval: interval must be > 0".into(),
                ));
            }
            Ok(STATE.with(|s| {
                let mut state = s.borrow_mut();
                let now = state.clock;
                if let Some(entry) = state.repeatables.get_mut(&id) {
                    entry.interval = interval;
                    entry.next_at = now + interval;
                    return true;
                }
                false
            }))
        })?,
    )?;

    t.set(
        "Cancel",
        lua.create_function(|lua, id: String| -> mlua::Result<bool> {
            Ok(STATE.with(|s| {
                let mut state = s.borrow_mut();
                let mut removed = false;
                if let Some(entry) = state.scheduled.remove(&id) {
                    let _ = lua.remove_registry_value(entry.cb);
                    removed = true;
                }
                if let Some(entry) = state.repeatables.remove(&id) {
                    let _ = lua.remove_registry_value(entry.cb);
                    removed = true;
                }
                removed
            }))
        })?,
    )?;

    t.set(
        "Exists",
        lua.create_function(|_, id: String| -> mlua::Result<bool> {
            Ok(STATE.with(|s| {
                let state = s.borrow();
                state.scheduled.contains_key(&id) || state.repeatables.contains_key(&id)
            }))
        })?,
    )?;

    t.set(
        "TimeLeft",
        lua.create_function(|_, id: String| -> mlua::Result<Option<f64>> {
            Ok(STATE.with(|s| {
                let state = s.borrow();
                let now = state.clock;
                if let Some(e) = state.scheduled.get(&id) {
                    return Some((e.fire_at - now).max(0.0));
                }
                if let Some(e) = state.repeatables.get(&id) {
                    return Some((e.next_at - now).max(0.0));
                }
                None
            }))
        })?,
    )?;

    Ok(t)
}

pub fn pump(lua: &Lua, dt: f64) {
    let now = STATE.with(|s| {
        let mut state = s.borrow_mut();
        state.clock += dt.max(0.0);
        state.clock
    });

    let due_delays: Vec<RegistryKey> = STATE.with(|s| {
        let mut state = s.borrow_mut();
        let mut due = Vec::new();
        let mut keep = Vec::with_capacity(state.delays.len());
        for entry in state.delays.drain(..) {
            if entry.fire_at <= now {
                due.push(entry.cb);
            } else {
                keep.push(entry);
            }
        }
        state.delays = keep;
        due
    });
    for cb_key in due_delays {
        if let Ok(func) = lua.registry_value::<Function>(&cb_key) {
            if let Err(e) = func.call::<()>(()) {
                eprintln!("[Task] Spawn/Delay callback error: {e}");
            }
        }
        let _ = lua.remove_registry_value(cb_key);
    }

    let due_scheduled_ids: Vec<String> = STATE.with(|s| {
        s.borrow()
            .scheduled
            .iter()
            .filter_map(|(k, v)| if v.fire_at <= now { Some(k.clone()) } else { None })
            .collect()
    });
    for id in due_scheduled_ids {
        let entry = STATE.with(|s| s.borrow_mut().scheduled.remove(&id));
        if let Some(entry) = entry {
            if let Ok(func) = lua.registry_value::<Function>(&entry.cb) {
                if let Err(e) = func.call::<()>(()) {
                    eprintln!("[Task] Schedule '{id}' callback error: {e}");
                }
            }
            let _ = lua.remove_registry_value(entry.cb);
        }
    }

    let due_repeat_ids: Vec<String> = STATE.with(|s| {
        s.borrow()
            .repeatables
            .iter()
            .filter_map(|(k, v)| if v.next_at <= now { Some(k.clone()) } else { None })
            .collect()
    });
    for id in due_repeat_ids {
        let func_opt = STATE.with(|s| {
            let state = s.borrow();
            state
                .repeatables
                .get(&id)
                .and_then(|e| lua.registry_value::<Function>(&e.cb).ok())
        });
        STATE.with(|s| {
            let mut state = s.borrow_mut();
            if let Some(entry) = state.repeatables.get_mut(&id) {
                if entry.interval > 0.0 {
                    while entry.next_at <= now {
                        entry.next_at += entry.interval;
                    }
                } else {
                    entry.next_at = now + 1.0;
                }
            }
        });
        if let Some(func) = func_opt {
            if let Err(e) = func.call::<()>(()) {
                eprintln!("[Task] Repeat '{id}' callback error: {e}");
            }
        }
    }
}
