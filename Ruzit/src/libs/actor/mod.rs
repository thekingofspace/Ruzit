use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};

use mlua::{
    ChunkMode, Compiler, Function, Lua, LuaOptions, MultiValue, StdLib, Table, UserData,
    UserDataMethods, Value,
};

const MAX_DEPTH: u32 = 64;
const FALLBACK_THREADS: usize = 4;

pub fn create(lua: &Lua) -> mlua::Result<Table> {
    let t = lua.create_table()?;
    t.set("new", lua.create_function(actor_new)?)?;
    Ok(t)
}

fn actor_new(_lua: &Lua, args: MultiValue) -> mlua::Result<ActorHandle> {
    let mut iter = args.into_iter();
    let body = iter.next().ok_or_else(|| {
        mlua::Error::RuntimeError(
            "Actor.new: pass a Luau source string whose chunk returns a function".into(),
        )
    })?;
    let thread_arg = iter.next();

    let source = match body {
        Value::String(s) => s.to_str()?.to_string(),
        other => {
            return Err(mlua::Error::RuntimeError(format!(
                "Actor.new: first arg must be a Luau source string (got {}). \
                 Functions can't cross threads — Luau forbids dumping bytecode out of a live \
                 closure, so you pass the chunk text and the worker compiles it. Wrap your \
                 logic like: Actor.new([[ return function(...) ... end ]])",
                other.type_name()
            )));
        }
    };

    let threads = match thread_arg {
        Some(Value::Nil) | None => default_threads(),
        Some(Value::Integer(n)) if n >= 1 => (n as usize).min(256),
        Some(Value::Number(n)) if n >= 1.0 => (n as usize).min(256),
        Some(other) => {
            return Err(mlua::Error::RuntimeError(format!(
                "Actor.new: thread count must be a positive integer (got {})",
                other.type_name()
            )));
        }
    };

    // Compile once on the main thread and ship bytecode to every worker —
    // cheaper than recompiling per worker, and we surface syntax errors here
    // instead of inside an opaque worker thread.
    let bytecode = Compiler::new().compile(&source).map_err(|e| {
        mlua::Error::RuntimeError(format!("Actor.new: compile failed: {e}"))
    })?;

    let (in_tx, in_rx) = mpsc::channel::<Vec<u8>>();
    let in_rx = Arc::new(Mutex::new(in_rx));
    let (out_tx, out_rx) = mpsc::channel::<WorkerResult>();
    let pending = Arc::new(AtomicUsize::new(0));
    let shutdown = Arc::new(AtomicBool::new(false));

    let mut handles = Vec::with_capacity(threads);
    for idx in 0..threads {
        let bytecode = bytecode.clone();
        let in_rx = in_rx.clone();
        let out_tx = out_tx.clone();
        let pending = pending.clone();
        let shutdown = shutdown.clone();
        let h = thread::Builder::new()
            .name(format!("ruzit-actor-{idx}"))
            .spawn(move || worker_main(bytecode, in_rx, out_tx, pending, shutdown))
            .map_err(|e| {
                mlua::Error::RuntimeError(format!("Actor.new: spawn worker: {e}"))
            })?;
        handles.push(h);
    }

    Ok(ActorHandle {
        inner: Arc::new(ActorInner {
            inbox: Mutex::new(Some(in_tx)),
            outbox: Mutex::new(out_rx),
            pending,
            shutdown,
            handles: Mutex::new(Some(handles)),
        }),
    })
}

fn default_threads() -> usize {
    thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(FALLBACK_THREADS)
        .max(1)
}

enum WorkerResult {
    Ok(Vec<u8>),
    Err(String),
}

struct ActorInner {
    // Wrapped in Option so Close() can drop the sender, signaling EOF to
    // every worker via the shared receiver.
    inbox: Mutex<Option<Sender<Vec<u8>>>>,
    outbox: Mutex<Receiver<WorkerResult>>,
    pending: Arc<AtomicUsize>,
    shutdown: Arc<AtomicBool>,
    handles: Mutex<Option<Vec<JoinHandle<()>>>>,
}

impl Drop for ActorInner {
    fn drop(&mut self) {
        // Drop the sender → workers' recv() returns Err → they exit. We don't
        // join here; if a worker is mid-Lua-call we'd block the runtime, and
        // the OS reaps the threads on process exit anyway.
        self.shutdown.store(true, Ordering::SeqCst);
        if let Ok(mut g) = self.inbox.lock() {
            *g = None;
        }
    }
}

#[derive(Clone)]
pub struct ActorHandle {
    inner: Arc<ActorInner>,
}

impl UserData for ActorHandle {
    fn add_methods<M: UserDataMethods<Self>>(m: &mut M) {
        // Push(args...) — encode the args, hand them to the worker pool.
        // Returns nothing. Errors only on a closed actor or if you try to
        // pass an unsupported value (function, userdata, thread, etc.).
        m.add_method("Push", |_, this, args: MultiValue| -> mlua::Result<()> {
            let bytes = serialize_multi(&args)?;
            let guard = this.inner.inbox.lock().unwrap();
            let tx = guard.as_ref().ok_or_else(|| {
                mlua::Error::RuntimeError("Actor:Push: actor is closed".into())
            })?;
            this.inner.pending.fetch_add(1, Ordering::SeqCst);
            if tx.send(bytes).is_err() {
                this.inner.pending.fetch_sub(1, Ordering::SeqCst);
                return Err(mlua::Error::RuntimeError(
                    "Actor:Push: workers have all exited".into(),
                ));
            }
            Ok(())
        });

        // Pop() — non-blocking: returns the next ready result's values, or
        // nothing if the queue is empty. Order is "first to finish first
        // out", not Push order, since work runs in parallel. If a worker
        // raised an error, that error propagates here on Pop.
        m.add_method("Pop", |lua, this, _: ()| -> mlua::Result<MultiValue> {
            let outbox = this.inner.outbox.lock().unwrap();
            match outbox.try_recv() {
                Ok(WorkerResult::Ok(bytes)) => {
                    this.inner.pending.fetch_sub(1, Ordering::SeqCst);
                    deserialize_multi(lua, &bytes)
                }
                Ok(WorkerResult::Err(msg)) => {
                    this.inner.pending.fetch_sub(1, Ordering::SeqCst);
                    Err(mlua::Error::RuntimeError(format!("Actor worker: {msg}")))
                }
                Err(TryRecvError::Empty) | Err(TryRecvError::Disconnected) => {
                    Ok(MultiValue::new())
                }
            }
        });

        // Pending() — count of jobs in flight + queued results not yet
        // popped. Useful for "drain everything" loops:
        //   while actor:Pending() > 0 do
        //       local r = actor:Pop()
        //       if r ~= nil then ... end
        //   end
        m.add_method("Pending", |_, this, _: ()| -> mlua::Result<i64> {
            Ok(this.inner.pending.load(Ordering::SeqCst) as i64)
        });

        // Threads() — how many worker threads back this actor.
        m.add_method("Threads", |_, this, _: ()| -> mlua::Result<i64> {
            Ok(this
                .inner
                .handles
                .lock()
                .unwrap()
                .as_ref()
                .map(|v| v.len())
                .unwrap_or(0) as i64)
        });

        // Close() — shut the actor down. Workers stop accepting new jobs and
        // exit once they finish whatever they're currently running. Already-
        // queued results are still drainable via Pop after Close.
        m.add_method("Close", |_, this, _: ()| -> mlua::Result<()> {
            this.inner.shutdown.store(true, Ordering::SeqCst);
            *this.inner.inbox.lock().unwrap() = None;
            Ok(())
        });
    }
}

fn worker_main(
    bytecode: Vec<u8>,
    in_rx: Arc<Mutex<Receiver<Vec<u8>>>>,
    out_tx: Sender<WorkerResult>,
    pending: Arc<AtomicUsize>,
    shutdown: Arc<AtomicBool>,
) {
    // Each worker gets its own Lua state with a sandboxed stdlib — math,
    // table, string, bit32, buffer, utf8, coroutine, base. NO `io`, `os`,
    // `package`, `debug`, and (since these are Ruzit-specific globals
    // installed by runtime.rs and never seeded into a fresh Lua) NO
    // `import`, `require`, `__dirname`. We additionally nuke `print`,
    // `loadstring`, `dofile`, `loadfile` from the base library so workers
    // can't escape their sandbox or spam stdout from a hot loop.
    let lua = match Lua::new_with(StdLib::ALL_SAFE, LuaOptions::default()) {
        Ok(l) => l,
        Err(e) => {
            let _ = out_tx.send(WorkerResult::Err(format!("init Lua: {e}")));
            return;
        }
    };
    {
        let g = lua.globals();
        for k in ["print", "dofile", "loadfile", "loadstring", "require"] {
            let _ = g.set(k, Value::Nil);
        }
    }

    let func: Function = match lua
        .load(&bytecode[..])
        .set_mode(ChunkMode::Binary)
        .eval::<Value>()
    {
        Ok(Value::Function(f)) => f,
        Ok(other) => {
            let _ = out_tx.send(WorkerResult::Err(format!(
                "Actor.new chunk must return a function (got {})",
                other.type_name()
            )));
            return;
        }
        Err(e) => {
            let _ = out_tx.send(WorkerResult::Err(format!("load chunk: {e}")));
            return;
        }
    };

    loop {
        if shutdown.load(Ordering::SeqCst) {
            return;
        }
        let bytes = {
            let rx = in_rx.lock().unwrap();
            match rx.recv() {
                Ok(b) => b,
                Err(_) => return,
            }
        };
        let args = match deserialize_multi(&lua, &bytes) {
            Ok(a) => a,
            Err(e) => {
                pending.fetch_sub(1, Ordering::SeqCst);
                let _ = out_tx.send(WorkerResult::Err(format!("decode args: {e}")));
                continue;
            }
        };
        match func.call::<MultiValue>(args) {
            Ok(rv) => match serialize_multi(&rv) {
                Ok(out) => {
                    let _ = out_tx.send(WorkerResult::Ok(out));
                }
                Err(e) => {
                    let _ = out_tx
                        .send(WorkerResult::Err(format!("encode return: {e}")));
                }
            },
            Err(e) => {
                let _ = out_tx.send(WorkerResult::Err(format!("call: {e}")));
            }
        }
    }
}

fn serialize_multi(values: &MultiValue) -> mlua::Result<Vec<u8>> {
    let mut buf = Vec::with_capacity(64);
    buf.extend_from_slice(&(values.len() as u32).to_le_bytes());
    for v in values.iter() {
        serialize_value(&mut buf, v, 0)?;
    }
    Ok(buf)
}

fn deserialize_multi(lua: &Lua, bytes: &[u8]) -> mlua::Result<MultiValue> {
    let mut pos = 0;
    let count = read_u32(bytes, &mut pos)? as usize;
    let mut mv = MultiValue::with_capacity(count);
    for _ in 0..count {
        mv.push_back(deserialize_value(lua, bytes, &mut pos)?);
    }
    Ok(mv)
}

fn serialize_value(buf: &mut Vec<u8>, value: &Value, depth: u32) -> mlua::Result<()> {
    if depth > MAX_DEPTH {
        return Err(mlua::Error::RuntimeError(
            "Actor: value too deeply nested (cycles or >64 levels not supported)".into(),
        ));
    }
    match value {
        Value::Nil => buf.push(0),
        Value::Boolean(false) => buf.push(1),
        Value::Boolean(true) => buf.push(2),
        Value::Integer(i) => {
            buf.push(3);
            // mlua-luau's Integer is i32; widen to i64 in the wire format so
            // the encoding survives a future widening of the Lua int width
            // without breaking older actor payloads.
            buf.extend_from_slice(&(*i as i64).to_le_bytes());
        }
        Value::Number(n) => {
            buf.push(4);
            buf.extend_from_slice(&n.to_le_bytes());
        }
        Value::String(s) => {
            buf.push(5);
            let bytes = s.as_bytes();
            buf.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
            buf.extend_from_slice(&bytes);
        }
        Value::Table(t) => {
            buf.push(6);
            let count_at = buf.len();
            buf.extend_from_slice(&[0u8; 4]);
            let mut count = 0u32;
            for pair in t.pairs::<Value, Value>() {
                let (k, v) = pair?;
                serialize_value(buf, &k, depth + 1)?;
                serialize_value(buf, &v, depth + 1)?;
                count += 1;
            }
            buf[count_at..count_at + 4].copy_from_slice(&count.to_le_bytes());
        }
        other => {
            return Err(mlua::Error::RuntimeError(format!(
                "Actor: cannot transfer value of type '{}' (only nil, bool, number, string, \
                 table are allowed across actor boundaries)",
                other.type_name()
            )));
        }
    }
    Ok(())
}

fn deserialize_value(lua: &Lua, buf: &[u8], pos: &mut usize) -> mlua::Result<Value> {
    let tag = read_u8(buf, pos)?;
    match tag {
        0 => Ok(Value::Nil),
        1 => Ok(Value::Boolean(false)),
        2 => Ok(Value::Boolean(true)),
        3 => {
            let n = read_i64(buf, pos)?;
            // Demote back to mlua-luau's i32 Integer; if the original value
            // didn't fit (came from a hypothetical wider int), fall back to
            // a Number so we don't silently truncate.
            if let Ok(small) = i32::try_from(n) {
                Ok(Value::Integer(small))
            } else {
                Ok(Value::Number(n as f64))
            }
        }
        4 => Ok(Value::Number(read_f64(buf, pos)?)),
        5 => {
            let len = read_u32(buf, pos)? as usize;
            let bytes = read_slice(buf, pos, len)?;
            Ok(Value::String(lua.create_string(bytes)?))
        }
        6 => {
            let count = read_u32(buf, pos)?;
            let t = lua.create_table()?;
            for _ in 0..count {
                let k = deserialize_value(lua, buf, pos)?;
                let v = deserialize_value(lua, buf, pos)?;
                t.set(k, v)?;
            }
            Ok(Value::Table(t))
        }
        _ => Err(mlua::Error::RuntimeError(format!(
            "Actor: malformed payload (bad tag {tag})"
        ))),
    }
}

fn read_u8(buf: &[u8], pos: &mut usize) -> mlua::Result<u8> {
    if *pos >= buf.len() {
        return Err(mlua::Error::RuntimeError("Actor: short read".into()));
    }
    let v = buf[*pos];
    *pos += 1;
    Ok(v)
}
fn read_u32(buf: &[u8], pos: &mut usize) -> mlua::Result<u32> {
    if *pos + 4 > buf.len() {
        return Err(mlua::Error::RuntimeError("Actor: short read".into()));
    }
    let v = u32::from_le_bytes(buf[*pos..*pos + 4].try_into().unwrap());
    *pos += 4;
    Ok(v)
}
fn read_i64(buf: &[u8], pos: &mut usize) -> mlua::Result<i64> {
    if *pos + 8 > buf.len() {
        return Err(mlua::Error::RuntimeError("Actor: short read".into()));
    }
    let v = i64::from_le_bytes(buf[*pos..*pos + 8].try_into().unwrap());
    *pos += 8;
    Ok(v)
}
fn read_f64(buf: &[u8], pos: &mut usize) -> mlua::Result<f64> {
    if *pos + 8 > buf.len() {
        return Err(mlua::Error::RuntimeError("Actor: short read".into()));
    }
    let v = f64::from_le_bytes(buf[*pos..*pos + 8].try_into().unwrap());
    *pos += 8;
    Ok(v)
}
fn read_slice<'a>(buf: &'a [u8], pos: &mut usize, len: usize) -> mlua::Result<&'a [u8]> {
    if *pos + len > buf.len() {
        return Err(mlua::Error::RuntimeError("Actor: short read".into()));
    }
    let s = &buf[*pos..*pos + len];
    *pos += len;
    Ok(s)
}
