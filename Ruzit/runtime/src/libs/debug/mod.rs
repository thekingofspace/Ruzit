use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use mlua::{Lua, MultiValue, RegistryKey, Table, UserData, UserDataMethods, Value};
use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System};

use crate::libs::signal;

const SAMPLE_INTERVAL: Duration = Duration::from_millis(500);

static FRAME_DROP_THRESHOLD_MS: AtomicU64 = AtomicU64::new(25);
static FRAME_DROP_COUNT: AtomicU64 = AtomicU64::new(0);
static FRAME_PEAK_MS_X1000: AtomicU64 = AtomicU64::new(0);
static LATEST_MEMORY_BYTES: AtomicU64 = AtomicU64::new(0);
static LATEST_CPU_X100: AtomicU64 = AtomicU64::new(0);
static GPU_SUBMIT_NS_SUM: AtomicU64 = AtomicU64::new(0);
static GPU_SUBMIT_COUNT: AtomicU64 = AtomicU64::new(0);
static SAMPLER_STARTED: AtomicBool = AtomicBool::new(false);

thread_local! {
    static DROP_LISTENERS: RefCell<Vec<DropListener>> = const { RefCell::new(Vec::new()) };
    static PENDING_DROPS: RefCell<Vec<PendingDrop>> = const { RefCell::new(Vec::new()) };
    static IS_TEST: RefCell<bool> = const { RefCell::new(true) };
}

struct DropListener {
    threshold_ms: f64,
    key: Arc<RegistryKey>,
}

struct PendingDrop {
    dt_ms: f64,
}

pub fn record_frame(dt_ms: f64) {
    let threshold = FRAME_DROP_THRESHOLD_MS.load(Ordering::Relaxed) as f64;
    let int_ms = (dt_ms * 1000.0) as u64;
    let prev_peak = FRAME_PEAK_MS_X1000.load(Ordering::Relaxed);
    if int_ms > prev_peak {
        let _ = FRAME_PEAK_MS_X1000.compare_exchange(
            prev_peak,
            int_ms,
            Ordering::Relaxed,
            Ordering::Relaxed,
        );
    }
    if dt_ms > threshold {
        FRAME_DROP_COUNT.fetch_add(1, Ordering::Relaxed);
        PENDING_DROPS.with(|c| c.borrow_mut().push(PendingDrop { dt_ms }));
    }
}

pub fn record_gpu_submit(ns: u64) {
    GPU_SUBMIT_NS_SUM.fetch_add(ns, Ordering::Relaxed);
    GPU_SUBMIT_COUNT.fetch_add(1, Ordering::Relaxed);
}

pub fn pump(lua: &Lua) {
    if !IS_TEST.with(|c| *c.borrow()) {
        return;
    }
    let drops: Vec<PendingDrop> = PENDING_DROPS.with(|c| std::mem::take(&mut *c.borrow_mut()));
    if drops.is_empty() {
        return;
    }
    let listeners: Vec<(f64, Arc<RegistryKey>)> = DROP_LISTENERS.with(|c| {
        c.borrow()
            .iter()
            .map(|l| (l.threshold_ms, l.key.clone()))
            .collect()
    });
    for d in drops {
        for (thresh, key) in &listeners {
            if d.dt_ms < *thresh {
                continue;
            }
            match lua.registry_value::<Table>(key) {
                Ok(signal_t) => {
                    let mut args = MultiValue::new();
                    args.push_back(Value::Number(d.dt_ms));
                    if let Err(e) = signal::fire(lua, &signal_t, args) {
                        eprintln!("[Debug] frame-drop signal fire: {e}");
                    }
                }
                Err(e) => eprintln!("[Debug] frame-drop signal lookup: {e}"),
            }
        }
    }
}

fn ensure_sampler() {
    if SAMPLER_STARTED.swap(true, Ordering::Relaxed) {
        return;
    }
    std::thread::Builder::new()
        .name("ruzit-debug-sampler".into())
        .spawn(|| {
            let pid = Pid::from_u32(std::process::id());
            let mut system = System::new();
            loop {
                system.refresh_processes_specifics(
                    ProcessesToUpdate::Some(&[pid]),
                    true,
                    ProcessRefreshKind::new().with_memory().with_cpu(),
                );
                if let Some(p) = system.process(pid) {
                    LATEST_MEMORY_BYTES.store(p.memory(), Ordering::Relaxed);
                    LATEST_CPU_X100.store((p.cpu_usage() * 100.0) as u64, Ordering::Relaxed);
                }
                std::thread::sleep(SAMPLE_INTERVAL);
            }
        })
        .ok();
}

#[derive(Clone, Copy, Debug)]
struct Snapshot {
    time: Instant,
    memory_bytes: u64,
    cpu_x100: u64,
    frame_drops: u64,
    gpu_ns_sum: u64,
    gpu_count: u64,
}

impl Snapshot {
    fn now() -> Self {
        Self {
            time: Instant::now(),
            memory_bytes: LATEST_MEMORY_BYTES.load(Ordering::Relaxed),
            cpu_x100: LATEST_CPU_X100.load(Ordering::Relaxed),
            frame_drops: FRAME_DROP_COUNT.load(Ordering::Relaxed),
            gpu_ns_sum: GPU_SUBMIT_NS_SUM.load(Ordering::Relaxed),
            gpu_count: GPU_SUBMIT_COUNT.load(Ordering::Relaxed),
        }
    }
}

pub struct Flag {
    name: String,
    state: Mutex<FlagState>,
    active_in_test: bool,
}

#[derive(Default)]
struct FlagState {
    baseline: Option<Snapshot>,
    history: HashMap<String, f64>,
}

impl Flag {
    fn new(name: String, active_in_test: bool) -> Self {
        Self {
            name,
            state: Mutex::new(FlagState::default()),
            active_in_test,
        }
    }
}

impl UserData for Flag {
    fn add_methods<M: UserDataMethods<Self>>(m: &mut M) {
        m.add_method("Begin", |_, this, _: ()| -> mlua::Result<()> {
            if !this.active_in_test {
                return Ok(());
            }
            let mut s = this.state.lock().unwrap();
            s.baseline = Some(Snapshot::now());
            Ok(())
        });

        m.add_method("End", |lua, this, _: ()| -> mlua::Result<Table> {
            let report = lua.create_table()?;
            if !this.active_in_test {
                return Ok(report);
            }
            let baseline = {
                let mut s = this.state.lock().unwrap();
                s.baseline.take()
            };
            let Some(b) = baseline else {
                return Err(mlua::Error::RuntimeError(format!(
                    "Debug.Flag('{}'):End called without a matching :Begin",
                    this.name
                )));
            };
            let now = Snapshot::now();
            let ms = (now.time - b.time).as_secs_f64() * 1000.0;
            let mem_delta_mb =
                (now.memory_bytes as f64 - b.memory_bytes as f64) / 1024.0 / 1024.0;
            let cpu_pct = now.cpu_x100 as f64 / 100.0;
            let frame_drops = now.frame_drops.saturating_sub(b.frame_drops);
            let gpu_count = now.gpu_count.saturating_sub(b.gpu_count);
            let gpu_avg_ms = if gpu_count > 0 {
                (now.gpu_ns_sum.saturating_sub(b.gpu_ns_sum)) as f64
                    / gpu_count as f64
                    / 1_000_000.0
            } else {
                0.0
            };

            report.set("name", this.name.clone())?;
            report.set("ms", ms)?;
            report.set("mem_delta_mb", mem_delta_mb)?;
            report.set("cpu_pct", cpu_pct)?;
            report.set("frame_drops", frame_drops as i64)?;
            report.set("gpu_avg_ms", gpu_avg_ms)?;

            {
                let mut s = this.state.lock().unwrap();
                s.history.insert("last_ms".into(), ms);
                s.history.insert("last_mem_delta_mb".into(), mem_delta_mb);
                s.history.insert("last_frame_drops".into(), frame_drops as f64);
            }

            println!(
                "[Debug] {}: {:.2} ms | Δmem {:+.2} MB | cpu {:.1}% | drops {} | gpu submit avg {:.2} ms",
                this.name, ms, mem_delta_mb, cpu_pct, frame_drops, gpu_avg_ms
            );
            Ok(report)
        });

        m.add_method("Last", |lua, this, _: ()| -> mlua::Result<Table> {
            let out = lua.create_table()?;
            if !this.active_in_test {
                return Ok(out);
            }
            let s = this.state.lock().unwrap();
            for (k, v) in &s.history {
                out.set(k.clone(), *v)?;
            }
            Ok(out)
        });

        m.add_method("Name", |_, this, _: ()| Ok(this.name.clone()));
    }
}

pub fn create(lua: &Lua) -> mlua::Result<Table> {
    let active = !crate::package::try_self_launcher().is_some();
    IS_TEST.with(|c| *c.borrow_mut() = active);
    if active {
        ensure_sampler();
    }

    let t = lua.create_table()?;
    t.set("IsTest", active)?;

    t.set(
        "Flag",
        lua.create_function(
            move |lua, name: String| -> mlua::Result<mlua::AnyUserData> {
                lua.create_userdata(Flag::new(name, active))
            },
        )?,
    )?;

    t.set(
        "Memory",
        lua.create_function(|_, _: ()| -> mlua::Result<f64> {
            Ok(LATEST_MEMORY_BYTES.load(Ordering::Relaxed) as f64)
        })?,
    )?;

    t.set(
        "MemoryMB",
        lua.create_function(|_, _: ()| -> mlua::Result<f64> {
            Ok(LATEST_MEMORY_BYTES.load(Ordering::Relaxed) as f64 / 1024.0 / 1024.0)
        })?,
    )?;

    t.set(
        "CpuPercent",
        lua.create_function(|_, _: ()| -> mlua::Result<f64> {
            Ok(LATEST_CPU_X100.load(Ordering::Relaxed) as f64 / 100.0)
        })?,
    )?;

    t.set(
        "FrameDrops",
        lua.create_function(|_, _: ()| -> mlua::Result<i64> {
            Ok(FRAME_DROP_COUNT.load(Ordering::Relaxed) as i64)
        })?,
    )?;

    t.set(
        "PeakFrameMs",
        lua.create_function(|_, _: ()| -> mlua::Result<f64> {
            Ok(FRAME_PEAK_MS_X1000.load(Ordering::Relaxed) as f64 / 1000.0)
        })?,
    )?;

    t.set(
        "ResetPeak",
        lua.create_function(|_, _: ()| -> mlua::Result<()> {
            FRAME_PEAK_MS_X1000.store(0, Ordering::Relaxed);
            Ok(())
        })?,
    )?;

    t.set(
        "SetFrameDropThreshold",
        lua.create_function(|_, ms: f64| -> mlua::Result<()> {
            FRAME_DROP_THRESHOLD_MS.store(ms.max(0.0) as u64, Ordering::Relaxed);
            Ok(())
        })?,
    )?;

    t.set(
        "MonitorFrameDrops",
        lua.create_function(
            move |lua, threshold_ms: Option<f64>| -> mlua::Result<Table> {
                let signal = signal::new_instance(lua)?;
                if !active {
                    return Ok(signal);
                }
                let key = Arc::new(lua.create_registry_value(signal.clone())?);
                DROP_LISTENERS.with(|c| {
                    c.borrow_mut().push(DropListener {
                        threshold_ms: threshold_ms.unwrap_or_else(|| {
                            FRAME_DROP_THRESHOLD_MS.load(Ordering::Relaxed) as f64
                        }),
                        key,
                    });
                });
                Ok(signal)
            },
        )?,
    )?;

    t.set(
        "GpuSubmitAvgMs",
        lua.create_function(|_, _: ()| -> mlua::Result<f64> {
            let count = GPU_SUBMIT_COUNT.load(Ordering::Relaxed);
            if count == 0 {
                return Ok(0.0);
            }
            let sum = GPU_SUBMIT_NS_SUM.load(Ordering::Relaxed);
            Ok(sum as f64 / count as f64 / 1_000_000.0)
        })?,
    )?;

    t.set(
        "ResetGpuStats",
        lua.create_function(|_, _: ()| -> mlua::Result<()> {
            GPU_SUBMIT_NS_SUM.store(0, Ordering::Relaxed);
            GPU_SUBMIT_COUNT.store(0, Ordering::Relaxed);
            Ok(())
        })?,
    )?;

    Ok(t)
}
