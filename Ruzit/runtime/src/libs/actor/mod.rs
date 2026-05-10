use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};

use mlua::{
    ChunkMode, Compiler, Function, Lua, LuaOptions, MultiValue, StdLib, Table, UserData,
    UserDataMethods, Value,
};

use crate::vfs::{self, Fs, read_module};

const MAX_DEPTH: u32 = 64;
const FALLBACK_THREADS: usize = 4;

pub fn create(lua: &Lua, fs: Fs, owner: String) -> mlua::Result<Table> {
    let t = lua.create_table()?;

    let fs_for_new = fs.clone();
    let owner_for_new = owner.clone();
    t.set(
        "new",
        lua.create_function(move |_lua, args: MultiValue| -> mlua::Result<ActorHandle> {
            actor_new(_lua, args, &fs_for_new, &owner_for_new)
        })?,
    )?;

    let fs_for_file = fs;
    let owner_for_file = owner;
    t.set(
        "FromFile",
        lua.create_function(move |_lua, args: MultiValue| -> mlua::Result<ActorHandle> {
            actor_from_file(_lua, args, &fs_for_file, &owner_for_file)
        })?,
    )?;

    Ok(t)
}

fn actor_new(_lua: &Lua, args: MultiValue, _fs: &Fs, _owner: &str) -> mlua::Result<ActorHandle> {
    let mut iter = args.into_iter();
    let body = iter.next().ok_or_else(|| {
        mlua::Error::RuntimeError(
            "Actor.new: pass a Luau source string (use Actor.FromFile for paths)".into(),
        )
    })?;
    let thread_arg = iter.next();

    let source = match body {
        Value::String(s) => s.to_str()?.to_string(),
        Value::Function(_) => {
            return Err(mlua::Error::RuntimeError(
                "Actor.new: passing a function is no longer supported \u{2014} \
                 use Actor.FromFile(\"path\") or pass the body as a string"
                    .into(),
            ));
        }
        other => {
            return Err(mlua::Error::RuntimeError(format!(
                "Actor.new: first arg must be a Luau source string (got {})",
                other.type_name()
            )));
        }
    };

    let threads = parse_thread_arg("Actor.new", thread_arg)?;
    spawn_actor("Actor.new", source.into_bytes(), threads)
}

fn actor_from_file(
    _lua: &Lua,
    args: MultiValue,
    fs: &Fs,
    owner: &str,
) -> mlua::Result<ActorHandle> {
    let mut iter = args.into_iter();
    let path = match iter.next() {
        Some(Value::String(s)) => s.to_str()?.to_string(),
        Some(other) => {
            return Err(mlua::Error::RuntimeError(format!(
                "Actor.FromFile: first arg must be a path string (got {})",
                other.type_name()
            )));
        }
        None => {
            return Err(mlua::Error::RuntimeError(
                "Actor.FromFile: missing path argument".into(),
            ));
        }
    };
    let thread_arg = iter.next();

    let resolved = vfs::resolve(fs, owner, &path).ok_or_else(|| {
        mlua::Error::RuntimeError(format!(
            "Actor.FromFile: file '{path}' not found (resolved from '{owner}')"
        ))
    })?;
    let source = read_module(fs, &resolved).ok_or_else(|| {
        mlua::Error::RuntimeError(format!("Actor.FromFile: could not read '{resolved}'"))
    })?;

    let threads = parse_thread_arg("Actor.FromFile", thread_arg)?;
    spawn_actor("Actor.FromFile", source, threads)
}

fn parse_thread_arg(api: &str, arg: Option<Value>) -> mlua::Result<usize> {
    match arg {
        Some(Value::Nil) | None => Ok(default_threads()),
        Some(Value::Integer(n)) if n >= 1 => Ok((n as usize).min(256)),
        Some(Value::Number(n)) if n >= 1.0 => Ok((n as usize).min(256)),
        Some(other) => Err(mlua::Error::RuntimeError(format!(
            "{api}: thread count must be a positive integer (got {})",
            other.type_name()
        ))),
    }
}

fn is_luau_bytecode(bytes: &[u8]) -> bool {
    matches!(bytes.first(), Some(&b) if b < 0x20 && b != b'\n' && b != b'\r' && b != b'\t')
}

fn spawn_actor(api: &str, source: Vec<u8>, threads: usize) -> mlua::Result<ActorHandle> {
    let bytecode = if is_luau_bytecode(&source) {
        source
    } else {
        Compiler::new()
            .compile(&source)
            .map_err(|e| mlua::Error::RuntimeError(format!("{api}: compile failed: {e}")))?
    };

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
            .map_err(|e| mlua::Error::RuntimeError(format!("{api}: spawn worker: {e}")))?;
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
    inbox: Mutex<Option<Sender<Vec<u8>>>>,
    outbox: Mutex<Receiver<WorkerResult>>,
    pending: Arc<AtomicUsize>,
    shutdown: Arc<AtomicBool>,
    handles: Mutex<Option<Vec<JoinHandle<()>>>>,
}

impl Drop for ActorInner {
    fn drop(&mut self) {
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
        m.add_method("Push", |_, this, args: MultiValue| -> mlua::Result<()> {
            let bytes = serialize_multi(&args)?;
            let guard = this.inner.inbox.lock().unwrap();
            let tx = guard
                .as_ref()
                .ok_or_else(|| mlua::Error::RuntimeError("Actor:Push: actor is closed".into()))?;
            this.inner.pending.fetch_add(1, Ordering::SeqCst);
            if tx.send(bytes).is_err() {
                this.inner.pending.fetch_sub(1, Ordering::SeqCst);
                return Err(mlua::Error::RuntimeError(
                    "Actor:Push: workers have all exited".into(),
                ));
            }
            Ok(())
        });

        m.add_method(
            "Pop",
            |lua, this, yield_wait: Option<bool>| -> mlua::Result<MultiValue> {
                let yield_wait = yield_wait.unwrap_or(false);
                let outbox = this.inner.outbox.lock().unwrap();
                let result = if yield_wait {
                    match outbox.recv() {
                        Ok(wr) => wr,
                        Err(_) => return Ok(MultiValue::new()),
                    }
                } else {
                    match outbox.try_recv() {
                        Ok(wr) => wr,
                        Err(TryRecvError::Empty) | Err(TryRecvError::Disconnected) => {
                            return Ok(MultiValue::new());
                        }
                    }
                };
                match result {
                    WorkerResult::Ok(bytes) => {
                        this.inner.pending.fetch_sub(1, Ordering::SeqCst);
                        deserialize_multi(lua, &bytes)
                    }
                    WorkerResult::Err(msg) => {
                        this.inner.pending.fetch_sub(1, Ordering::SeqCst);
                        Err(mlua::Error::RuntimeError(format!("Actor worker: {msg}")))
                    }
                }
            },
        );

        m.add_method("Pending", |_, this, _: ()| -> mlua::Result<i64> {
            Ok(this.inner.pending.load(Ordering::SeqCst) as i64)
        });

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
                    let _ = out_tx.send(WorkerResult::Err(format!("encode return: {e}")));
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
