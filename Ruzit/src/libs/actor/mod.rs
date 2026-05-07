use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};

use mlua::{
    ChunkMode, Compiler, Function, Lua, LuaOptions, MultiValue, StdLib, Table, UserData,
    UserDataMethods, Value,
};

use crate::vfs::{Fs, read_module};

const MAX_DEPTH: u32 = 64;
const FALLBACK_THREADS: usize = 4;

pub fn create(lua: &Lua, fs: Fs) -> mlua::Result<Table> {
    let t = lua.create_table()?;
    let fs_clone = fs.clone();
    t.set(
        "new",
        lua.create_function(move |lua, args: MultiValue| -> mlua::Result<ActorHandle> {
            actor_new(lua, args, &fs_clone)
        })?,
    )?;
    Ok(t)
}

fn actor_new(lua: &Lua, args: MultiValue, fs: &Fs) -> mlua::Result<ActorHandle> {
    let mut iter = args.into_iter();
    let body = iter.next().ok_or_else(|| {
        mlua::Error::RuntimeError(
            "Actor.new: pass a function or a Luau source string".into(),
        )
    })?;
    let thread_arg = iter.next();

    let source = match body {
        Value::String(s) => s.to_str()?.to_string(),
        Value::Function(f) => function_to_source(lua, &f, fs)?,
        other => {
            return Err(mlua::Error::RuntimeError(format!(
                "Actor.new: first arg must be a function or a Luau source string (got {})",
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

fn function_to_source(lua: &Lua, f: &Function, fs: &Fs) -> mlua::Result<String> {
    if has_upvalues(lua, f)? {
        return Err(mlua::Error::RuntimeError(
            "Actor.new: this function captures upvalues from its outer scope. Workers run \
             in an isolated Luau state and can't see those — make the function self-contained \
             (no references to outer locals) or pass the captured values as arguments to Push."
                .into(),
        ));
    }

    let info = f.info();
    let source_name = info.source.as_deref().ok_or_else(|| {
        mlua::Error::RuntimeError(
            "Actor.new: function has no source info. Define it inline in a script — Actor \
             can't recover the body of a function created via loadstring / dynamic loaders."
                .into(),
        )
    })?;
    let line_defined = info.line_defined.unwrap_or(0);
    if line_defined <= 0 {
        return Err(mlua::Error::RuntimeError(format!(
            "Actor.new: missing line info for function from '{source_name}' — pass a string \
             form like Actor.new([[ return function(...) ... end ]]) instead"
        )));
    }

    let key = source_name.strip_prefix('@').ok_or_else(|| {
        mlua::Error::RuntimeError(format!(
            "Actor.new: function source '{source_name}' isn't a script — can't recover its \
             body. Move it into a .luau file or pass a string form."
        ))
    })?;

    let text = read_module(fs, key).ok_or_else(|| {
        mlua::Error::RuntimeError(format!(
            "Actor.new: could not read script '{key}' to extract the function body"
        ))
    })?;

    let extracted = extract_function_block(&text, line_defined as u32).ok_or_else(|| {
        mlua::Error::RuntimeError(format!(
            "Actor.new: could not isolate the function literal in '{key}' starting at line \
             {line_defined}. Method-syntax (`function obj:m(...)`) isn't supported — declare \
             it as a regular `function name(...)` or pass a string form."
        ))
    })?;

    Ok(format!("return {extracted}"))
}

fn has_upvalues(lua: &Lua, f: &Function) -> mlua::Result<bool> {
    let debug: Table = match lua.globals().get("debug") {
        Ok(t) => t,
        Err(_) => return Ok(false),
    };
    let getupvalue: Function = match debug.get("getupvalue") {
        Ok(f) => f,
        Err(_) => return Ok(false),
    };
    let v: Value = getupvalue.call((f.clone(), 1i32))?;
    Ok(!matches!(v, Value::Nil))
}

fn extract_function_block(source: &str, line_defined: u32) -> Option<String> {
    let mut line_starts = vec![0usize];
    for (i, b) in source.bytes().enumerate() {
        if b == b'\n' {
            line_starts.push(i + 1);
        }
    }
    line_starts.push(source.len());

    let start_line_idx = (line_defined as usize).saturating_sub(1);
    if start_line_idx >= line_starts.len() {
        return None;
    }

    let scan_start = line_starts[start_line_idx];
    let segment = &source[scan_start..];

    let mut tok = LuauTokens::new(segment);
    let mut start_off: Option<usize> = None;
    let mut end_off: Option<usize> = None;
    let mut depth: i32 = 0;
    let mut pending_do: i32 = 0;

    while let Some((kw, off, len)) = tok.next_keyword() {
        match kw {
            "function" => {
                if start_off.is_none() {
                    start_off = Some(off);
                }
                depth += 1;
            }
            "if" => {
                depth += 1;
            }
            "while" | "for" => {
                depth += 1;
                pending_do += 1;
            }
            "do" => {
                if pending_do > 0 {
                    pending_do -= 1;
                } else {
                    depth += 1;
                }
            }
            "repeat" => {
                depth += 1;
            }
            "until" => {
                depth -= 1;
            }
            "end" => {
                depth -= 1;
                if depth == 0 && start_off.is_some() {
                    end_off = Some(off + len);
                    break;
                }
            }
            _ => {}
        }
    }

    let s = start_off?;
    let e = end_off?;
    let raw = &segment[s..e];
    strip_function_name(raw)
}

fn strip_function_name(text: &str) -> Option<String> {
    if !text.starts_with("function") {
        return None;
    }
    let bytes = text.as_bytes();
    let mut p = "function".len();
    while p < bytes.len() && bytes[p].is_ascii_whitespace() {
        p += 1;
    }
    if p < bytes.len() && bytes[p] == b'(' {
        return Some(text.to_string());
    }
    let mut saw_colon = false;
    loop {
        if p >= bytes.len() {
            return None;
        }
        let b = bytes[p];
        if b == b'_' || b.is_ascii_alphabetic() {
            while p < bytes.len()
                && (bytes[p] == b'_' || bytes[p].is_ascii_alphanumeric())
            {
                p += 1;
            }
        } else if b == b'.' {
            p += 1;
        } else if b == b':' {
            saw_colon = true;
            p += 1;
        } else if b.is_ascii_whitespace() {
            p += 1;
        } else if b == b'(' {
            break;
        } else {
            return None;
        }
    }
    if saw_colon {
        return None;
    }
    Some(format!("function{}", &text[p..]))
}

struct LuauTokens<'a> {
    src: &'a [u8],
    pos: usize,
}

impl<'a> LuauTokens<'a> {
    fn new(src: &'a str) -> Self {
        Self { src: src.as_bytes(), pos: 0 }
    }

    fn next_keyword(&mut self) -> Option<(&'static str, usize, usize)> {
        loop {
            self.skip_ws_comments();
            if self.pos >= self.src.len() {
                return None;
            }
            let c = self.src[self.pos];

            if c == b'"' || c == b'\'' {
                self.skip_short_string(c);
                continue;
            }
            if c == b'[' {
                let saved = self.pos;
                if let Some(level) = self.try_long_open() {
                    self.skip_long_close(level);
                    continue;
                }
                self.pos = saved;
            }

            if c == b'_' || c.is_ascii_alphabetic() {
                let start = self.pos;
                while self.pos < self.src.len()
                    && (self.src[self.pos] == b'_'
                        || self.src[self.pos].is_ascii_alphanumeric())
                {
                    self.pos += 1;
                }
                let word = &self.src[start..self.pos];
                if let Some(kw) = match_kw(word) {
                    return Some((kw, start, word.len()));
                }
                continue;
            }

            if c.is_ascii_digit() {
                while self.pos < self.src.len()
                    && (self.src[self.pos].is_ascii_alphanumeric()
                        || self.src[self.pos] == b'.')
                {
                    self.pos += 1;
                }
                continue;
            }

            self.pos += 1;
        }
    }

    fn skip_ws_comments(&mut self) {
        loop {
            while self.pos < self.src.len() && self.src[self.pos].is_ascii_whitespace() {
                self.pos += 1;
            }
            if self.pos + 1 < self.src.len()
                && self.src[self.pos] == b'-'
                && self.src[self.pos + 1] == b'-'
            {
                self.pos += 2;
                let saved = self.pos;
                if let Some(level) = self.try_long_open() {
                    self.skip_long_close(level);
                    continue;
                }
                self.pos = saved;
                while self.pos < self.src.len() && self.src[self.pos] != b'\n' {
                    self.pos += 1;
                }
                continue;
            }
            return;
        }
    }

    fn try_long_open(&mut self) -> Option<usize> {
        if self.pos >= self.src.len() || self.src[self.pos] != b'[' {
            return None;
        }
        let mut p = self.pos + 1;
        let mut level = 0usize;
        while p < self.src.len() && self.src[p] == b'=' {
            level += 1;
            p += 1;
        }
        if p < self.src.len() && self.src[p] == b'[' {
            self.pos = p + 1;
            return Some(level);
        }
        None
    }

    fn skip_long_close(&mut self, level: usize) {
        loop {
            while self.pos < self.src.len() && self.src[self.pos] != b']' {
                self.pos += 1;
            }
            if self.pos >= self.src.len() {
                return;
            }
            self.pos += 1;
            let mut count = 0usize;
            while self.pos < self.src.len() && self.src[self.pos] == b'=' {
                count += 1;
                self.pos += 1;
            }
            if count == level && self.pos < self.src.len() && self.src[self.pos] == b']' {
                self.pos += 1;
                return;
            }
        }
    }

    fn skip_short_string(&mut self, quote: u8) {
        self.pos += 1;
        while self.pos < self.src.len() {
            let c = self.src[self.pos];
            if c == b'\\' {
                self.pos = (self.pos + 2).min(self.src.len());
                continue;
            }
            self.pos += 1;
            if c == quote || c == b'\n' {
                return;
            }
        }
    }
}

fn match_kw(word: &[u8]) -> Option<&'static str> {
    match word {
        b"function" => Some("function"),
        b"end" => Some("end"),
        b"if" => Some("if"),
        b"while" => Some("while"),
        b"for" => Some("for"),
        b"do" => Some("do"),
        b"repeat" => Some("repeat"),
        b"until" => Some("until"),
        _ => None,
    }
}
