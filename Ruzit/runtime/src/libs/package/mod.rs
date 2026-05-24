use std::cell::RefCell;
use std::collections::HashMap;
use std::ffi::{CStr, CString};
use std::os::raw::c_void;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use libffi::middle::{Arg, Cif, CodePtr, Type as FfiType};
use libloading::Library;
use mlua::{
    AnyUserData, Function, Lua, MultiValue, RegistryKey, Table, Thread, UserData, UserDataFields,
    UserDataMethods, Value,
};

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum CType {
    Void,
    Int8,
    UInt8,
    Int16,
    UInt16,
    Int32,
    UInt32,
    Int64,
    UInt64,
    ISize,
    USize,
    Float,
    Double,
    Bool,
    Pointer,
    CString,
}

impl CType {
    fn to_ffi(self) -> FfiType {
        match self {
            Self::Void => FfiType::void(),
            Self::Int8 => FfiType::i8(),
            Self::UInt8 => FfiType::u8(),
            Self::Int16 => FfiType::i16(),
            Self::UInt16 => FfiType::u16(),
            Self::Int32 => FfiType::i32(),
            Self::UInt32 => FfiType::u32(),
            Self::Int64 => FfiType::i64(),
            Self::UInt64 => FfiType::u64(),
            Self::ISize => FfiType::isize(),
            Self::USize => FfiType::usize(),
            Self::Float => FfiType::f32(),
            Self::Double => FfiType::f64(),
            Self::Bool => FfiType::i32(),
            Self::Pointer | Self::CString => FfiType::pointer(),
        }
    }
    fn as_str(self) -> &'static str {
        match self {
            Self::Void => "Void",
            Self::Int8 => "Int8",
            Self::UInt8 => "UInt8",
            Self::Int16 => "Int16",
            Self::UInt16 => "UInt16",
            Self::Int32 => "Int32",
            Self::UInt32 => "UInt32",
            Self::Int64 => "Int64",
            Self::UInt64 => "UInt64",
            Self::ISize => "ISize",
            Self::USize => "USize",
            Self::Float => "Float",
            Self::Double => "Double",
            Self::Bool => "Bool",
            Self::Pointer => "Pointer",
            Self::CString => "CString",
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct CTypeTag(pub CType);

impl UserData for CTypeTag {
    fn add_fields<F: UserDataFields<Self>>(f: &mut F) {
        f.add_field_method_get("Name", |_, this| Ok(this.0.as_str()));
    }
    fn add_methods<M: UserDataMethods<Self>>(m: &mut M) {
        m.add_meta_method("__tostring", |_, this, _: ()| {
            Ok(format!("PackageType({})", this.0.as_str()))
        });
    }
}

#[derive(Clone, Debug)]
pub struct CTypeValue {
    pub ctype: CType,
    pub value: CTypeValueInner,
}

#[derive(Clone, Debug)]
pub enum CTypeValueInner {
    Integer(i128),
    Number(f64),
    Boolean(bool),
    String(String),
    Nil,
}

impl UserData for CTypeValue {
    fn add_fields<F: UserDataFields<Self>>(f: &mut F) {
        f.add_field_method_get("Type", |_, this| Ok(this.ctype.as_str()));
        f.add_field_method_get("Value", |lua, this| -> mlua::Result<Value> {
            Ok(match &this.value {
                CTypeValueInner::Integer(i) => {
                    if *i >= i64::MIN as i128 && *i <= i64::MAX as i128 {
                        Value::Integer(*i as i64)
                    } else {
                        Value::Number(*i as f64)
                    }
                }
                CTypeValueInner::Number(f) => Value::Number(*f),
                CTypeValueInner::Boolean(b) => Value::Boolean(*b),
                CTypeValueInner::String(s) => Value::String(lua.create_string(s)?),
                CTypeValueInner::Nil => Value::Nil,
            })
        });
    }
    fn add_methods<M: UserDataMethods<Self>>(m: &mut M) {
        m.add_meta_method("__tostring", |_, this, _: ()| {
            Ok(format!(
                "PackageValue({}, {:?})",
                this.ctype.as_str(),
                this.value
            ))
        });
    }
}

fn wrap_value(ctype: CType, v: Option<Value>) -> mlua::Result<CTypeValue> {
    let value = match v {
        None | Some(Value::Nil) => CTypeValueInner::Nil,
        Some(Value::Integer(n)) => CTypeValueInner::Integer(n as i128),
        Some(Value::Number(n)) => CTypeValueInner::Number(n),
        Some(Value::Boolean(b)) => CTypeValueInner::Boolean(b),
        Some(Value::String(s)) => CTypeValueInner::String(s.to_str()?.to_string()),
        Some(other) => {
            return Err(mlua::Error::RuntimeError(format!(
                "Types.{}(): can't wrap a {} value",
                ctype.as_str(),
                other.type_name()
            )));
        }
    };
    Ok(CTypeValue { ctype, value })
}

pub struct LoadedLib {
    pub lib: Library,
    pub source: String,
}

unsafe impl Send for LoadedLib {}
unsafe impl Sync for LoadedLib {}

// ───── Buffer: owned native memory for out-args / list reads ─────────

pub struct PackageBuffer {
    ptr: Arc<Mutex<BufferStorage>>,
}

struct BufferStorage {
    data: Vec<u8>,
    alive: bool,
}

impl PackageBuffer {
    fn new(size: usize) -> Self {
        Self {
            ptr: Arc::new(Mutex::new(BufferStorage {
                data: vec![0u8; size],
                alive: true,
            })),
        }
    }
    fn with_slice<R>(&self, f: impl FnOnce(&mut [u8]) -> mlua::Result<R>) -> mlua::Result<R> {
        let mut g = self.ptr.lock().unwrap();
        if !g.alive {
            return Err(mlua::Error::RuntimeError(
                "Buffer: this buffer has been freed".into(),
            ));
        }
        f(&mut g.data)
    }
    fn raw_ptr(&self) -> i64 {
        let g = self.ptr.lock().unwrap();
        if !g.alive {
            return 0;
        }
        g.data.as_ptr() as usize as i64
    }
}

fn coerce_buf_int(v: Value) -> mlua::Result<i128> {
    match v {
        Value::Integer(n) => Ok(n as i128),
        Value::Number(n) => Ok(n as i128),
        Value::Boolean(b) => Ok(if b { 1 } else { 0 }),
        _ => Err(mlua::Error::RuntimeError(
            "Buffer:Write*: value must be a number/integer/boolean".into(),
        )),
    }
}

fn coerce_buf_f64(v: Value) -> mlua::Result<f64> {
    match v {
        Value::Integer(n) => Ok(n as f64),
        Value::Number(n) => Ok(n),
        _ => Err(mlua::Error::RuntimeError(
            "Buffer:Write*: value must be a number".into(),
        )),
    }
}

fn check_range(off: usize, n: usize, len: usize) -> mlua::Result<()> {
    if off.checked_add(n).map(|end| end > len).unwrap_or(true) {
        return Err(mlua::Error::RuntimeError(format!(
            "Buffer: offset {off}+{n} out of range (size {len})"
        )));
    }
    Ok(())
}

impl UserData for PackageBuffer {
    fn add_fields<F: UserDataFields<Self>>(f: &mut F) {
        f.add_field_method_get("Pointer", |_, this| Ok(this.raw_ptr()));
        f.add_field_method_get("Size", |_, this| {
            Ok(this.ptr.lock().unwrap().data.len() as i64)
        });
        f.add_field_method_get("Alive", |_, this| Ok(this.ptr.lock().unwrap().alive));
    }

    fn add_methods<M: UserDataMethods<Self>>(m: &mut M) {
        macro_rules! read_method {
            ($name:expr, $ty:ty, $into:expr) => {
                m.add_method($name, |_, this, off: i64| -> mlua::Result<Value> {
                    let off = off.max(0) as usize;
                    this.with_slice(|d| {
                        let n = std::mem::size_of::<$ty>();
                        check_range(off, n, d.len())?;
                        let mut buf = [0u8; std::mem::size_of::<$ty>()];
                        buf.copy_from_slice(&d[off..off + n]);
                        let v: $ty = <$ty>::from_le_bytes(buf);
                        Ok($into(v))
                    })
                });
            };
        }
        macro_rules! write_method {
            ($name:expr, $ty:ty, $from:expr) => {
                m.add_method(
                    $name,
                    |_, this, (off, val): (i64, Value)| -> mlua::Result<()> {
                        let off = off.max(0) as usize;
                        let v: $ty = $from(val)?;
                        this.with_slice(|d| {
                            let n = std::mem::size_of::<$ty>();
                            check_range(off, n, d.len())?;
                            d[off..off + n].copy_from_slice(&v.to_le_bytes());
                            Ok(())
                        })
                    },
                );
            };
        }
        read_method!("ReadInt8", i8, |v: i8| Value::Integer(v as i64));
        read_method!("ReadUInt8", u8, |v: u8| Value::Integer(v as i64));
        read_method!("ReadInt16", i16, |v: i16| Value::Integer(v as i64));
        read_method!("ReadUInt16", u16, |v: u16| Value::Integer(v as i64));
        read_method!("ReadInt32", i32, |v: i32| Value::Integer(v as i64));
        read_method!("ReadUInt32", u32, |v: u32| Value::Integer(v as i64));
        read_method!("ReadInt64", i64, Value::Integer);
        read_method!("ReadUInt64", u64, |v: u64| {
            if v <= i64::MAX as u64 {
                Value::Integer(v as i64)
            } else {
                Value::Number(v as f64)
            }
        });
        read_method!("ReadFloat", f32, |v: f32| Value::Number(v as f64));
        read_method!("ReadDouble", f64, Value::Number);
        read_method!("ReadPointer", usize, |v: usize| Value::Integer(v as i64));
        read_method!("ReadBool", u32, |v: u32| Value::Boolean(v != 0));

        write_method!("WriteInt8", i8, |v: Value| coerce_buf_int(v).map(|n| n as i8));
        write_method!("WriteUInt8", u8, |v: Value| coerce_buf_int(v).map(|n| n as u8));
        write_method!("WriteInt16", i16, |v: Value| coerce_buf_int(v).map(|n| n as i16));
        write_method!("WriteUInt16", u16, |v: Value| coerce_buf_int(v).map(|n| n as u16));
        write_method!("WriteInt32", i32, |v: Value| coerce_buf_int(v).map(|n| n as i32));
        write_method!("WriteUInt32", u32, |v: Value| coerce_buf_int(v).map(|n| n as u32));
        write_method!("WriteInt64", i64, |v: Value| coerce_buf_int(v).map(|n| n as i64));
        write_method!("WriteUInt64", u64, |v: Value| coerce_buf_int(v).map(|n| n as u64));
        write_method!("WriteFloat", f32, |v: Value| coerce_buf_f64(v).map(|n| n as f32));
        write_method!("WriteDouble", f64, coerce_buf_f64);
        write_method!("WritePointer", usize, |v: Value| coerce_buf_int(v)
            .map(|n| n as usize));
        write_method!("WriteBool", u32, |v: Value| {
            match v {
                Value::Boolean(b) => Ok(if b { 1u32 } else { 0 }),
                _ => coerce_buf_int(v).map(|n| if n != 0 { 1u32 } else { 0 }),
            }
        });

        m.add_method(
            "WriteString",
            |_, this, (off, s): (i64, String)| -> mlua::Result<()> {
                let off = off.max(0) as usize;
                this.with_slice(|d| {
                    let bytes = s.as_bytes();
                    check_range(off, bytes.len() + 1, d.len())?;
                    d[off..off + bytes.len()].copy_from_slice(bytes);
                    d[off + bytes.len()] = 0;
                    Ok(())
                })
            },
        );
        m.add_method(
            "ReadCString",
            |lua, this, off: i64| -> mlua::Result<Value> {
                let off = off.max(0) as usize;
                this.with_slice(|d| {
                    if off >= d.len() {
                        return Err(mlua::Error::RuntimeError(format!(
                            "Buffer:ReadCString: offset {off} out of range (size {})",
                            d.len()
                        )));
                    }
                    let end = d[off..].iter().position(|&b| b == 0).map(|p| off + p).unwrap_or(d.len());
                    let s = std::str::from_utf8(&d[off..end]).map_err(|_| {
                        mlua::Error::RuntimeError("Buffer:ReadCString: bytes are not UTF-8".into())
                    })?;
                    Ok(Value::String(lua.create_string(s)?))
                })
            },
        );
        m.add_method("Zero", |_, this, _: ()| -> mlua::Result<()> {
            this.with_slice(|d| {
                for b in d.iter_mut() {
                    *b = 0;
                }
                Ok(())
            })
        });
        m.add_method("Free", |_, this, _: ()| -> mlua::Result<()> {
            let mut g = this.ptr.lock().unwrap();
            g.alive = false;
            g.data.clear();
            g.data.shrink_to_fit();
            Ok(())
        });
        m.add_meta_method("__tostring", |_, this, _: ()| {
            let g = this.ptr.lock().unwrap();
            Ok(format!(
                "Buffer(size={}, ptr=0x{:X}, alive={})",
                g.data.len(),
                if g.alive { g.data.as_ptr() as usize } else { 0 },
                g.alive
            ))
        });
    }
}

pub struct PackageHandle {
    pub state: Arc<Mutex<Option<LoadedLib>>>,
}

impl UserData for PackageHandle {
    fn add_fields<F: UserDataFields<Self>>(f: &mut F) {
        f.add_field_method_get("Alive", |_, this| {
            Ok(this.state.lock().unwrap().is_some())
        });
        f.add_field_method_get("Source", |_, this| {
            Ok(this
                .state
                .lock()
                .unwrap()
                .as_ref()
                .map(|l| l.source.clone())
                .unwrap_or_default())
        });
    }

    fn add_methods<M: UserDataMethods<Self>>(m: &mut M) {
        m.add_method(
            "Bind",
            |_, this, (name, ret, args): (String, AnyUserData, Option<Table>)| -> mlua::Result<BoundFn> {
                let ret_ty = ret
                    .borrow::<CTypeTag>()
                    .map_err(|_| {
                        mlua::Error::RuntimeError(
                            "Package:Bind: second argument must be a Package.Types.* tag".into(),
                        )
                    })?
                    .0;
                let mut arg_tys: Vec<CType> = Vec::new();
                if let Some(t) = args {
                    let len = t.raw_len() as usize;
                    for i in 1..=len {
                        let v: Value = t.get(i as i64)?;
                        match v {
                            Value::UserData(ud) => {
                                let tag = ud.borrow::<CTypeTag>().map_err(|_| {
                                    mlua::Error::RuntimeError(format!(
                                        "Package:Bind: arg #{i} type must be a Package.Types.* tag"
                                    ))
                                })?;
                                if tag.0 == CType::Void {
                                    return Err(mlua::Error::RuntimeError(format!(
                                        "Package:Bind: arg #{i} cannot be Void"
                                    )));
                                }
                                arg_tys.push(tag.0);
                            }
                            _ => {
                                return Err(mlua::Error::RuntimeError(format!(
                                    "Package:Bind: arg #{i} must be a Package.Types.* tag"
                                )));
                            }
                        }
                    }
                }
                let g = this.state.lock().unwrap();
                let loaded = g.as_ref().ok_or_else(|| {
                    mlua::Error::RuntimeError("Package:Bind: library has been unloaded".into())
                })?;
                let symbol_bytes = CString::new(name.as_bytes()).map_err(|_| {
                    mlua::Error::RuntimeError("Package:Bind: symbol name must not contain NUL".into())
                })?;
                let code_ptr: usize = unsafe {
                    let sym: libloading::Symbol<*const c_void> = loaded
                        .lib
                        .get(symbol_bytes.as_bytes_with_nul())
                        .map_err(|e| {
                            mlua::Error::RuntimeError(format!(
                                "Package:Bind: symbol '{name}' not found: {e}"
                            ))
                        })?;
                    *sym as usize
                };
                Ok(BoundFn {
                    lib_state: this.state.clone(),
                    symbol_name: name,
                    code_ptr,
                    return_type: ret_ty,
                    arg_types: arg_tys,
                })
            },
        );
        m.add_method("Exports", |lua, this, _: ()| -> mlua::Result<Table> {
            let source = {
                let g = this.state.lock().unwrap();
                let loaded = g.as_ref().ok_or_else(|| {
                    mlua::Error::RuntimeError("Package:Exports: library unloaded".into())
                })?;
                loaded.source.clone()
            };
            let names = read_exports(&source).map_err(mlua::Error::RuntimeError)?;
            let t = lua.create_table()?;
            for (i, n) in names.iter().enumerate() {
                t.set(i as i64 + 1, n.as_str())?;
            }
            Ok(t)
        });
        m.add_method("Unload", |_, this, _: ()| -> mlua::Result<()> {
            this.state.lock().unwrap().take();
            Ok(())
        });
    }
}

pub struct BoundFn {
    pub lib_state: Arc<Mutex<Option<LoadedLib>>>,
    pub symbol_name: String,
    pub code_ptr: usize,
    pub return_type: CType,
    pub arg_types: Vec<CType>,
}

unsafe impl Send for BoundFn {}
unsafe impl Sync for BoundFn {}

impl UserData for BoundFn {
    fn add_fields<F: UserDataFields<Self>>(f: &mut F) {
        f.add_field_method_get("Name", |_, this| Ok(this.symbol_name.clone()));
        f.add_field_method_get("Arity", |_, this| Ok(this.arg_types.len() as i64));
        f.add_field_method_get("ReturnType", |_, this| Ok(this.return_type.as_str()));
        f.add_field_method_get("ArgTypes", |lua, this| -> mlua::Result<Table> {
            let t = lua.create_table()?;
            for (i, ty) in this.arg_types.iter().enumerate() {
                t.set(i as i64 + 1, ty.as_str())?;
            }
            Ok(t)
        });
    }

    fn add_methods<M: UserDataMethods<Self>>(m: &mut M) {
        m.add_meta_method("__call", |lua, this, args: MultiValue| {
            call_blocking_lua(lua, this, args)
        });
        m.add_method("Call", |lua, this, args: MultiValue| {
            call_blocking_lua(lua, this, args)
        });
        m.add_method(
            "Async",
            |_, this, args: MultiValue| -> mlua::Result<PromiseHandle> {
                spawn_async(this, args)
            },
        );
        m.add_meta_method("__tostring", |_, this, _: ()| {
            Ok(format!(
                "BoundFn({}, ret={}, arity={})",
                this.symbol_name,
                this.return_type.as_str(),
                this.arg_types.len()
            ))
        });
    }
}

fn call_blocking_lua(lua: &Lua, bf: &BoundFn, args: MultiValue) -> mlua::Result<Value> {
    let v = call_blocking(bf, args)?;
    if bf.return_type == CType::CString {
        if let Value::Integer(addr) = v {
            if addr == 0 {
                return Ok(Value::Nil);
            }
            let p = addr as usize as *const i8;
            let s = unsafe { CStr::from_ptr(p) }
                .to_string_lossy()
                .into_owned();
            return Ok(Value::String(lua.create_string(&s)?));
        }
    }
    Ok(v)
}

fn call_blocking(bf: &BoundFn, args: MultiValue) -> mlua::Result<Value> {
    {
        let g = bf.lib_state.lock().unwrap();
        if g.is_none() {
            return Err(mlua::Error::RuntimeError(
                "BoundFn: library has been unloaded".into(),
            ));
        }
    }
    let argv: Vec<Value> = args.into_iter().collect();
    if argv.len() != bf.arg_types.len() {
        return Err(mlua::Error::RuntimeError(format!(
            "{}: expected {} arg(s), got {}",
            bf.symbol_name,
            bf.arg_types.len(),
            argv.len()
        )));
    }
    let storage = build_arg_storage(&bf.arg_types, &argv, &bf.symbol_name)?;
    let cif = build_cif(&bf.arg_types, bf.return_type);
    let code = CodePtr::from_ptr(bf.code_ptr as *mut _);
    let arg_refs = storage.as_args();
    unsafe { invoke(&cif, code, &arg_refs, bf.return_type) }
}

fn build_cif(args: &[CType], ret: CType) -> Cif {
    let arg_tys: Vec<FfiType> = args.iter().map(|a| a.to_ffi()).collect();
    Cif::new(arg_tys, ret.to_ffi())
}

#[derive(Debug)]
enum ArgCell {
    I8(i8),
    U8(u8),
    I16(i16),
    U16(u16),
    I32(i32),
    U32(u32),
    I64(i64),
    U64(u64),
    ISize(isize),
    USize(usize),
    F32(f32),
    F64(f64),
    Ptr(usize),
    CStr(CString),
}

struct ArgStorage {
    cells: Vec<ArgCell>,
}

impl ArgStorage {
    fn as_args(&self) -> Vec<Arg> {
        let mut out: Vec<Arg> = Vec::with_capacity(self.cells.len());
        for c in &self.cells {
            match c {
                ArgCell::I8(v) => out.push(Arg::new(v)),
                ArgCell::U8(v) => out.push(Arg::new(v)),
                ArgCell::I16(v) => out.push(Arg::new(v)),
                ArgCell::U16(v) => out.push(Arg::new(v)),
                ArgCell::I32(v) => out.push(Arg::new(v)),
                ArgCell::U32(v) => out.push(Arg::new(v)),
                ArgCell::I64(v) => out.push(Arg::new(v)),
                ArgCell::U64(v) => out.push(Arg::new(v)),
                ArgCell::ISize(v) => out.push(Arg::new(v)),
                ArgCell::USize(v) => out.push(Arg::new(v)),
                ArgCell::F32(v) => out.push(Arg::new(v)),
                ArgCell::F64(v) => out.push(Arg::new(v)),
                ArgCell::Ptr(v) => out.push(Arg::new(v)),
                ArgCell::CStr(s) => {
                    let p = s.as_ptr();
                    let boxed = Box::leak(Box::new(p));
                    out.push(Arg::new(boxed));
                }
            }
        }
        out
    }
}

fn build_arg_storage(
    types: &[CType],
    values: &[Value],
    name: &str,
) -> mlua::Result<ArgStorage> {
    let mut cells: Vec<ArgCell> = Vec::with_capacity(types.len());
    for (i, (ty, raw_v)) in types.iter().zip(values.iter()).enumerate() {
        if let Value::UserData(ud) = raw_v {
            if let Ok(wrapped) = ud.borrow::<CTypeValue>() {
                if wrapped.ctype != *ty {
                    return Err(mlua::Error::RuntimeError(format!(
                        "{name}: arg #{} declared {} but wrapped value is {}",
                        i + 1,
                        ty.as_str(),
                        wrapped.ctype.as_str()
                    )));
                }
                if matches!(ty, CType::CString) {
                    let s = if let CTypeValueInner::String(s) = &wrapped.value {
                        s.clone()
                    } else if matches!(wrapped.value, CTypeValueInner::Nil) {
                        cells.push(ArgCell::Ptr(0));
                        continue;
                    } else {
                        return Err(mlua::Error::RuntimeError(format!(
                            "{name}: arg #{} CString wrap must hold a string",
                            i + 1
                        )));
                    };
                    let cs = CString::new(s.into_bytes()).map_err(|e| {
                        mlua::Error::RuntimeError(format!(
                            "{name}: arg #{} CString contains NUL: {e}",
                            i + 1
                        ))
                    })?;
                    cells.push(ArgCell::CStr(cs));
                    continue;
                }
                let cell = coerce_inner(ty, &wrapped.value, name, i)?;
                cells.push(cell);
                continue;
            }
        }
        let v = raw_v;
        let n_int = match v {
            Value::Integer(n) => Some(*n as i128),
            Value::Number(n) => Some(*n as i128),
            Value::Boolean(b) => Some(if *b { 1 } else { 0 }),
            _ => None,
        };
        let n_float = match v {
            Value::Integer(n) => Some(*n as f64),
            Value::Number(n) => Some(*n),
            Value::Boolean(b) => Some(if *b { 1.0 } else { 0.0 }),
            _ => None,
        };
        let cell = match ty {
            CType::Int8 => ArgCell::I8(
                n_int.ok_or_else(|| ty_err(name, i, "Int8", v))? as i8,
            ),
            CType::UInt8 => ArgCell::U8(
                n_int.ok_or_else(|| ty_err(name, i, "UInt8", v))? as u8,
            ),
            CType::Int16 => ArgCell::I16(
                n_int.ok_or_else(|| ty_err(name, i, "Int16", v))? as i16,
            ),
            CType::UInt16 => ArgCell::U16(
                n_int.ok_or_else(|| ty_err(name, i, "UInt16", v))? as u16,
            ),
            CType::Int32 => ArgCell::I32(
                n_int.ok_or_else(|| ty_err(name, i, "Int32", v))? as i32,
            ),
            CType::UInt32 => ArgCell::U32(
                n_int.ok_or_else(|| ty_err(name, i, "UInt32", v))? as u32,
            ),
            CType::Int64 => ArgCell::I64(
                n_int.ok_or_else(|| ty_err(name, i, "Int64", v))? as i64,
            ),
            CType::UInt64 => ArgCell::U64(
                n_int.ok_or_else(|| ty_err(name, i, "UInt64", v))? as u64,
            ),
            CType::ISize => ArgCell::ISize(
                n_int.ok_or_else(|| ty_err(name, i, "ISize", v))? as isize,
            ),
            CType::USize => ArgCell::USize(
                n_int.ok_or_else(|| ty_err(name, i, "USize", v))? as usize,
            ),
            CType::Float => ArgCell::F32(
                n_float.ok_or_else(|| ty_err(name, i, "Float", v))? as f32,
            ),
            CType::Double => ArgCell::F64(
                n_float.ok_or_else(|| ty_err(name, i, "Double", v))?,
            ),
            CType::Bool => ArgCell::I32(
                match v {
                    Value::Boolean(b) => {
                        if *b {
                            1
                        } else {
                            0
                        }
                    }
                    Value::Nil => 0,
                    _ => n_int.ok_or_else(|| ty_err(name, i, "Bool", v))? as i32,
                },
            ),
            CType::Pointer => match v {
                Value::Integer(n) => ArgCell::Ptr(*n as usize),
                Value::Number(n) => ArgCell::Ptr(*n as usize),
                Value::Nil => ArgCell::Ptr(0),
                _ => return Err(ty_err(name, i, "Pointer", v)),
            },
            CType::CString => match v {
                Value::String(s) => {
                    let bytes = s.as_bytes().to_vec();
                    let cs = CString::new(bytes).map_err(|e| {
                        mlua::Error::RuntimeError(format!(
                            "{name}: arg #{} CString contains NUL: {e}",
                            i + 1
                        ))
                    })?;
                    ArgCell::CStr(cs)
                }
                Value::Nil => ArgCell::Ptr(0),
                _ => return Err(ty_err(name, i, "CString", v)),
            },
            CType::Void => {
                return Err(mlua::Error::RuntimeError(format!(
                    "{name}: arg #{} cannot be declared Void",
                    i + 1
                )));
            }
        };
        cells.push(cell);
    }
    Ok(ArgStorage { cells })
}

fn coerce_inner(
    ty: &CType,
    inner: &CTypeValueInner,
    fn_name: &str,
    i: usize,
) -> mlua::Result<ArgCell> {
    let as_int = |fallback: &str| -> mlua::Result<i128> {
        match inner {
            CTypeValueInner::Integer(n) => Ok(*n),
            CTypeValueInner::Number(f) => Ok(*f as i128),
            CTypeValueInner::Boolean(b) => Ok(if *b { 1 } else { 0 }),
            CTypeValueInner::Nil => Ok(0),
            _ => Err(mlua::Error::RuntimeError(format!(
                "{fn_name}: arg #{} wrapped value can't convert to {}",
                i + 1,
                fallback
            ))),
        }
    };
    let as_float = |fallback: &str| -> mlua::Result<f64> {
        match inner {
            CTypeValueInner::Integer(n) => Ok(*n as f64),
            CTypeValueInner::Number(f) => Ok(*f),
            CTypeValueInner::Boolean(b) => Ok(if *b { 1.0 } else { 0.0 }),
            CTypeValueInner::Nil => Ok(0.0),
            _ => Err(mlua::Error::RuntimeError(format!(
                "{fn_name}: arg #{} wrapped value can't convert to {}",
                i + 1,
                fallback
            ))),
        }
    };
    Ok(match ty {
        CType::Int8 => ArgCell::I8(as_int("Int8")? as i8),
        CType::UInt8 => ArgCell::U8(as_int("UInt8")? as u8),
        CType::Int16 => ArgCell::I16(as_int("Int16")? as i16),
        CType::UInt16 => ArgCell::U16(as_int("UInt16")? as u16),
        CType::Int32 => ArgCell::I32(as_int("Int32")? as i32),
        CType::UInt32 => ArgCell::U32(as_int("UInt32")? as u32),
        CType::Int64 => ArgCell::I64(as_int("Int64")? as i64),
        CType::UInt64 => ArgCell::U64(as_int("UInt64")? as u64),
        CType::ISize => ArgCell::ISize(as_int("ISize")? as isize),
        CType::USize => ArgCell::USize(as_int("USize")? as usize),
        CType::Float => ArgCell::F32(as_float("Float")? as f32),
        CType::Double => ArgCell::F64(as_float("Double")?),
        CType::Bool => ArgCell::I32(if as_int("Bool")? != 0 { 1 } else { 0 }),
        CType::Pointer => ArgCell::Ptr(as_int("Pointer")? as usize),
        CType::CString | CType::Void => {
            return Err(mlua::Error::RuntimeError(format!(
                "{fn_name}: arg #{} CString/Void should be handled separately",
                i + 1
            )));
        }
    })
}

fn ty_err(fn_name: &str, i: usize, ty: &str, v: &Value) -> mlua::Error {
    mlua::Error::RuntimeError(format!(
        "{fn_name}: arg #{} expected {}, got {}",
        i + 1,
        ty,
        v.type_name()
    ))
}

unsafe fn invoke(cif: &Cif, code: CodePtr, args: &[Arg], ret: CType) -> mlua::Result<Value> {
    Ok(match ret {
        CType::Void => {
            let _: () = unsafe { cif.call(code, args) };
            Value::Nil
        }
        CType::Int8 => Value::Integer(unsafe { cif.call::<i8>(code, args) } as i64),
        CType::UInt8 => Value::Integer(unsafe { cif.call::<u8>(code, args) } as i64),
        CType::Int16 => Value::Integer(unsafe { cif.call::<i16>(code, args) } as i64),
        CType::UInt16 => Value::Integer(unsafe { cif.call::<u16>(code, args) } as i64),
        CType::Int32 => Value::Integer(unsafe { cif.call::<i32>(code, args) } as i64),
        CType::UInt32 => Value::Integer(unsafe { cif.call::<u32>(code, args) } as i64),
        CType::Int64 => Value::Integer(unsafe { cif.call::<i64>(code, args) }),
        CType::UInt64 => {
            let r: u64 = unsafe { cif.call(code, args) };
            if r <= i64::MAX as u64 {
                Value::Integer(r as i64)
            } else {
                Value::Number(r as f64)
            }
        }
        CType::ISize => Value::Integer(unsafe { cif.call::<isize>(code, args) } as i64),
        CType::USize => {
            let r: usize = unsafe { cif.call(code, args) };
            if r <= i64::MAX as usize {
                Value::Integer(r as i64)
            } else {
                Value::Number(r as f64)
            }
        }
        CType::Float => Value::Number(unsafe { cif.call::<f32>(code, args) } as f64),
        CType::Double => Value::Number(unsafe { cif.call::<f64>(code, args) }),
        CType::Bool => Value::Boolean(unsafe { cif.call::<i32>(code, args) } != 0),
        CType::Pointer => {
            let r: *const c_void = unsafe { cif.call(code, args) };
            Value::Integer(r as usize as i64)
        }
        CType::CString => {
            let r: *const c_void = unsafe { cif.call(code, args) };
            Value::Integer(r as usize as i64)
        }
    })
}

// ───── Promise / async ─────────────────────────────────────────────────

static NEXT_PROMISE_ID: AtomicU64 = AtomicU64::new(1);

thread_local! {
    static PENDING_AWAITS: RefCell<HashMap<u64, RegistryKey>> = RefCell::new(HashMap::new());
}

struct CompletedAsync {
    id: u64,
    result: Result<AsyncResult, String>,
}

enum AsyncResult {
    Nil,
    Integer(i64),
    Number(f64),
    Pointer(i64),
    Boolean(bool),
}

fn completed_queue() -> &'static Mutex<Vec<CompletedAsync>> {
    static Q: OnceLock<Mutex<Vec<CompletedAsync>>> = OnceLock::new();
    Q.get_or_init(|| Mutex::new(Vec::new()))
}

pub struct PromiseHandle {
    id: u64,
    inner: Arc<Mutex<PromiseInner>>,
}

struct PromiseInner {
    state: PromiseState,
    then_cbs: Vec<RegistryKey>,
    catch_cbs: Vec<RegistryKey>,
}

#[derive(Clone)]
enum PromiseState {
    Pending,
    Resolved(AsyncResultOwned),
    Rejected(String),
}

#[derive(Clone)]
enum AsyncResultOwned {
    Nil,
    Integer(i64),
    Number(f64),
    Pointer(i64),
    Boolean(bool),
}

impl From<&AsyncResult> for AsyncResultOwned {
    fn from(r: &AsyncResult) -> Self {
        match r {
            AsyncResult::Nil => Self::Nil,
            AsyncResult::Integer(i) => Self::Integer(*i),
            AsyncResult::Number(f) => Self::Number(*f),
            AsyncResult::Pointer(p) => Self::Pointer(*p),
            AsyncResult::Boolean(b) => Self::Boolean(*b),
        }
    }
}

fn async_to_value(r: &AsyncResultOwned) -> Value {
    match r {
        AsyncResultOwned::Nil => Value::Nil,
        AsyncResultOwned::Integer(i) => Value::Integer(*i),
        AsyncResultOwned::Number(f) => Value::Number(*f),
        AsyncResultOwned::Pointer(p) => Value::Integer(*p),
        AsyncResultOwned::Boolean(b) => Value::Boolean(*b),
    }
}

fn spawn_async(bf: &BoundFn, args: MultiValue) -> mlua::Result<PromiseHandle> {
    {
        let g = bf.lib_state.lock().unwrap();
        if g.is_none() {
            return Err(mlua::Error::RuntimeError(
                "BoundFn:Async: library has been unloaded".into(),
            ));
        }
    }
    let argv: Vec<Value> = args.into_iter().collect();
    if argv.len() != bf.arg_types.len() {
        return Err(mlua::Error::RuntimeError(format!(
            "{}:Async: expected {} arg(s), got {}",
            bf.symbol_name,
            bf.arg_types.len(),
            argv.len()
        )));
    }
    let storage = build_arg_storage(&bf.arg_types, &argv, &bf.symbol_name)?;
    let arg_types = bf.arg_types.clone();
    let return_type = bf.return_type;
    let code = bf.code_ptr;
    let name = bf.symbol_name.clone();
    let lib_state = bf.lib_state.clone();

    let id = NEXT_PROMISE_ID.fetch_add(1, Ordering::Relaxed);
    let inner = Arc::new(Mutex::new(PromiseInner {
        state: PromiseState::Pending,
        then_cbs: Vec::new(),
        catch_cbs: Vec::new(),
    }));

    let inner_for_worker = inner.clone();

    std::thread::spawn(move || {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let g = lib_state.lock().unwrap();
            if g.is_none() {
                return Err::<AsyncResult, String>("library unloaded mid-call".into());
            }
            drop(g);
            let cif = build_cif(&arg_types, return_type);
            let code_ptr = CodePtr::from_ptr(code as *mut _);
            let arg_refs = storage.as_args();
            let r = unsafe { invoke_to_async(&cif, code_ptr, &arg_refs, return_type) };
            Ok(r)
        }));
        let final_result: Result<AsyncResult, String> = match result {
            Ok(Ok(r)) => Ok(r),
            Ok(Err(e)) => Err(e),
            Err(_) => Err(format!("{}: native call panicked", name)),
        };
        // Stash on the promise inner AND publish to the queue for await
        let owned: Result<AsyncResultOwned, String> = match &final_result {
            Ok(r) => Ok(r.into()),
            Err(e) => Err(e.clone()),
        };
        {
            let mut g = inner_for_worker.lock().unwrap();
            g.state = match owned {
                Ok(r) => PromiseState::Resolved(r),
                Err(e) => PromiseState::Rejected(e),
            };
        }
        completed_queue().lock().unwrap().push(CompletedAsync {
            id,
            result: final_result,
        });
    });

    Ok(PromiseHandle { id, inner })
}

unsafe fn invoke_to_async(
    cif: &Cif,
    code: CodePtr,
    args: &[Arg],
    ret: CType,
) -> AsyncResult {
    match ret {
        CType::Void => {
            let _: () = unsafe { cif.call(code, args) };
            AsyncResult::Nil
        }
        CType::Int8 => AsyncResult::Integer(unsafe { cif.call::<i8>(code, args) } as i64),
        CType::UInt8 => AsyncResult::Integer(unsafe { cif.call::<u8>(code, args) } as i64),
        CType::Int16 => AsyncResult::Integer(unsafe { cif.call::<i16>(code, args) } as i64),
        CType::UInt16 => AsyncResult::Integer(unsafe { cif.call::<u16>(code, args) } as i64),
        CType::Int32 => AsyncResult::Integer(unsafe { cif.call::<i32>(code, args) } as i64),
        CType::UInt32 => AsyncResult::Integer(unsafe { cif.call::<u32>(code, args) } as i64),
        CType::Int64 => AsyncResult::Integer(unsafe { cif.call::<i64>(code, args) }),
        CType::UInt64 => {
            let r: u64 = unsafe { cif.call(code, args) };
            if r <= i64::MAX as u64 {
                AsyncResult::Integer(r as i64)
            } else {
                AsyncResult::Number(r as f64)
            }
        }
        CType::ISize => AsyncResult::Integer(unsafe { cif.call::<isize>(code, args) } as i64),
        CType::USize => {
            let r: usize = unsafe { cif.call(code, args) };
            if r <= i64::MAX as usize {
                AsyncResult::Integer(r as i64)
            } else {
                AsyncResult::Number(r as f64)
            }
        }
        CType::Float => AsyncResult::Number(unsafe { cif.call::<f32>(code, args) } as f64),
        CType::Double => AsyncResult::Number(unsafe { cif.call::<f64>(code, args) }),
        CType::Bool => AsyncResult::Boolean(unsafe { cif.call::<i32>(code, args) } != 0),
        CType::Pointer | CType::CString => {
            let r: *const c_void = unsafe { cif.call(code, args) };
            AsyncResult::Pointer(r as usize as i64)
        }
    }
}

impl UserData for PromiseHandle {
    fn add_fields<F: UserDataFields<Self>>(f: &mut F) {
        f.add_field_method_get("Done", |_, this| {
            Ok(!matches!(this.inner.lock().unwrap().state, PromiseState::Pending))
        });
        f.add_field_method_get("Resolved", |_, this| {
            Ok(matches!(this.inner.lock().unwrap().state, PromiseState::Resolved(_)))
        });
        f.add_field_method_get("Rejected", |_, this| {
            Ok(matches!(this.inner.lock().unwrap().state, PromiseState::Rejected(_)))
        });
    }

    fn add_methods<M: UserDataMethods<Self>>(m: &mut M) {
        m.add_method(
            "AndThen",
            |lua, this, cb: Function| -> mlua::Result<PromiseHandle> {
                let state = this.inner.lock().unwrap().state.clone();
                match state {
                    PromiseState::Resolved(r) => {
                        let _ = cb.call::<MultiValue>(async_to_value(&r));
                    }
                    PromiseState::Pending => {
                        let key = lua.create_registry_value(cb)?;
                        this.inner.lock().unwrap().then_cbs.push(key);
                    }
                    PromiseState::Rejected(_) => {}
                }
                Ok(PromiseHandle {
                    id: this.id,
                    inner: this.inner.clone(),
                })
            },
        );
        m.add_method(
            "Catch",
            |lua, this, cb: Function| -> mlua::Result<PromiseHandle> {
                let state = this.inner.lock().unwrap().state.clone();
                match state {
                    PromiseState::Rejected(e) => {
                        let _ = cb.call::<MultiValue>(e);
                    }
                    PromiseState::Pending => {
                        let key = lua.create_registry_value(cb)?;
                        this.inner.lock().unwrap().catch_cbs.push(key);
                    }
                    PromiseState::Resolved(_) => {}
                }
                Ok(PromiseHandle {
                    id: this.id,
                    inner: this.inner.clone(),
                })
            },
        );
        m.add_method("Await", |lua, this, _: ()| -> mlua::Result<Value> {
            let state = this.inner.lock().unwrap().state.clone();
            match state {
                PromiseState::Resolved(r) => Ok(async_to_value(&r)),
                PromiseState::Rejected(e) => Err(mlua::Error::RuntimeError(e)),
                PromiseState::Pending => {
                    // Park current thread; the heart pump resumes us when the
                    // promise's completion lands on the queue.
                    let thread = lua.current_thread();
                    let key = lua.create_registry_value(thread)?;
                    PENDING_AWAITS.with(|c| c.borrow_mut().insert(this.id, key));
                    Err(mlua::Error::RuntimeError(
                        "<package_await_yield>".to_string(),
                    ))
                }
            }
        });
        m.add_meta_method("__tostring", |_, this, _: ()| {
            let s = match &this.inner.lock().unwrap().state {
                PromiseState::Pending => "Pending",
                PromiseState::Resolved(_) => "Resolved",
                PromiseState::Rejected(_) => "Rejected",
            };
            Ok(format!("Promise(id={}, {})", this.id, s))
        });
    }
}

pub fn pump(lua: &Lua) {
    let drained: Vec<CompletedAsync> = {
        let mut q = completed_queue().lock().unwrap();
        std::mem::take(&mut *q)
    };
    for done in drained {
        let key = PENDING_AWAITS.with(|c| c.borrow_mut().remove(&done.id));
        if let Some(key) = key {
            let thread: Thread = match lua.registry_value(&key) {
                Ok(t) => t,
                Err(_) => continue,
            };
            let _ = lua.remove_registry_value(key);
            let _ = match done.result {
                Ok(r) => {
                    let v = match &r {
                        AsyncResult::Nil => Value::Nil,
                        AsyncResult::Integer(i) => Value::Integer(*i),
                        AsyncResult::Number(f) => Value::Number(*f),
                        AsyncResult::Pointer(p) => Value::Integer(*p),
                        AsyncResult::Boolean(b) => Value::Boolean(*b),
                    };
                    thread.resume::<MultiValue>(v)
                }
                Err(e) => {
                    let mut args = MultiValue::new();
                    args.push_back(Value::Nil);
                    args.push_back(Value::String(
                        lua.create_string(&format!("__package_async_error__:{e}"))
                            .unwrap_or_else(|_| lua.create_string("").unwrap()),
                    ));
                    thread.resume::<MultiValue>(args)
                }
            };
        }
    }
}

// ───── Path resolution ─────────────────────────────────────────────────

fn exe_dir() -> Option<PathBuf> {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(PathBuf::from))
}

fn validate_and_resolve(name: &str) -> mlua::Result<PathBuf> {
    if name.is_empty() {
        return Err(mlua::Error::RuntimeError(
            "Package.Load: empty path".into(),
        ));
    }
    if name.contains("..") {
        return Err(mlua::Error::RuntimeError(format!(
            "Package.Load: '{name}' contains '..' — directory traversal is forbidden"
        )));
    }
    if std::path::Path::new(name).is_absolute() {
        return Err(mlua::Error::RuntimeError(format!(
            "Package.Load: '{name}' is absolute — only paths relative to the exe are allowed (use './name')"
        )));
    }
    let rel = name
        .strip_prefix("./")
        .or_else(|| name.strip_prefix(".\\"))
        .unwrap_or(name);
    let dir = exe_dir().ok_or_else(|| {
        mlua::Error::RuntimeError("Package.Load: could not derive exe directory".into())
    })?;
    let candidates = candidate_filenames(rel);
    for cand in candidates {
        let p = dir.join(&cand);
        if p.is_file() {
            return Ok(p);
        }
    }
    Err(mlua::Error::RuntimeError(format!(
        "Package.Load: could not find native library for '{name}' next to the exe. Tried platform-specific extensions in {}",
        dir.display()
    )))
}

fn candidate_filenames(name: &str) -> Vec<String> {
    let lower = name.to_ascii_lowercase();
    let has_ext = lower.ends_with(".dll")
        || lower.ends_with(".so")
        || lower.ends_with(".dylib")
        || lower.contains('.');
    let mut out: Vec<String> = Vec::new();
    if has_ext {
        out.push(name.to_string());
    } else if cfg!(target_os = "windows") {
        out.push(format!("{name}.dll"));
    } else if cfg!(target_os = "macos") {
        out.push(format!("lib{name}.dylib"));
        out.push(format!("{name}.dylib"));
    } else {
        out.push(format!("lib{name}.so"));
        out.push(format!("{name}.so"));
    }
    out
}

fn read_exports(path: &str) -> Result<Vec<String>, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("read {path}: {e}"))?;
    #[cfg(target_os = "windows")]
    {
        pe_exports(&bytes).map_err(|e| format!("PE parse: {e}"))
    }
    #[cfg(not(target_os = "windows"))]
    {
        elf_or_macho_exports(&bytes).map_err(|e| format!("ELF/Mach-O parse: {e}"))
    }
}

#[cfg(target_os = "windows")]
fn pe_exports(bytes: &[u8]) -> Result<Vec<String>, String> {
    if bytes.len() < 0x40 {
        return Err("file too small".into());
    }
    let e_lfanew = u32::from_le_bytes(bytes[0x3C..0x40].try_into().unwrap()) as usize;
    if e_lfanew + 0x18 > bytes.len() || &bytes[e_lfanew..e_lfanew + 4] != b"PE\0\0" {
        return Err("not a PE file".into());
    }
    let opt_off = e_lfanew + 0x18;
    let magic = u16::from_le_bytes(bytes[opt_off..opt_off + 2].try_into().unwrap());
    let is_pe32_plus = match magic {
        0x10B => false,
        0x20B => true,
        _ => return Err(format!("unknown PE magic 0x{magic:X}")),
    };
    let export_dir_off = opt_off + if is_pe32_plus { 0x70 } else { 0x60 };
    if export_dir_off + 8 > bytes.len() {
        return Err("export directory offset out of range".into());
    }
    let export_rva =
        u32::from_le_bytes(bytes[export_dir_off..export_dir_off + 4].try_into().unwrap()) as usize;
    if export_rva == 0 {
        return Ok(Vec::new());
    }
    let num_sections = u16::from_le_bytes(bytes[e_lfanew + 6..e_lfanew + 8].try_into().unwrap())
        as usize;
    let size_of_opt = u16::from_le_bytes(bytes[e_lfanew + 0x14..e_lfanew + 0x16].try_into().unwrap())
        as usize;
    let section_table = e_lfanew + 0x18 + size_of_opt;
    let mut sections: Vec<(u32, u32, u32)> = Vec::new();
    for i in 0..num_sections {
        let s = section_table + i * 40;
        if s + 40 > bytes.len() {
            return Err("section table out of range".into());
        }
        let virt_addr = u32::from_le_bytes(bytes[s + 12..s + 16].try_into().unwrap());
        let virt_size = u32::from_le_bytes(bytes[s + 8..s + 12].try_into().unwrap());
        let raw_off = u32::from_le_bytes(bytes[s + 20..s + 24].try_into().unwrap());
        sections.push((virt_addr, virt_size, raw_off));
    }
    let rva_to_off = |rva: u32| -> Option<usize> {
        for &(va, sz, off) in &sections {
            if rva >= va && rva < va + sz {
                return Some((rva - va) as usize + off as usize);
            }
        }
        None
    };
    let dir_off = rva_to_off(export_rva as u32).ok_or("export RVA outside any section")?;
    if dir_off + 40 > bytes.len() {
        return Err("export directory out of range".into());
    }
    let num_names =
        u32::from_le_bytes(bytes[dir_off + 24..dir_off + 28].try_into().unwrap()) as usize;
    let names_rva =
        u32::from_le_bytes(bytes[dir_off + 32..dir_off + 36].try_into().unwrap()) as usize;
    let names_off = rva_to_off(names_rva as u32).ok_or("names RVA outside any section")?;
    let mut out: Vec<String> = Vec::with_capacity(num_names);
    for i in 0..num_names {
        if names_off + i * 4 + 4 > bytes.len() {
            break;
        }
        let name_rva = u32::from_le_bytes(
            bytes[names_off + i * 4..names_off + i * 4 + 4]
                .try_into()
                .unwrap(),
        );
        if let Some(name_off) = rva_to_off(name_rva) {
            let end = bytes[name_off..]
                .iter()
                .position(|&b| b == 0)
                .map(|p| name_off + p)
                .unwrap_or(bytes.len());
            if let Ok(s) = std::str::from_utf8(&bytes[name_off..end]) {
                out.push(s.to_string());
            }
        }
    }
    Ok(out)
}

#[cfg(not(target_os = "windows"))]
fn elf_or_macho_exports(bytes: &[u8]) -> Result<Vec<String>, String> {
    if bytes.starts_with(b"\x7FELF") {
        elf_exports(bytes)
    } else if bytes.starts_with(&[0xFE, 0xED, 0xFA, 0xCE])
        || bytes.starts_with(&[0xCE, 0xFA, 0xED, 0xFE])
        || bytes.starts_with(&[0xFE, 0xED, 0xFA, 0xCF])
        || bytes.starts_with(&[0xCF, 0xFA, 0xED, 0xFE])
    {
        Err("Mach-O export parsing is not implemented in this build; the function still loads and Bind works by name".into())
    } else {
        Err("unrecognised binary format".into())
    }
}

#[cfg(not(target_os = "windows"))]
fn elf_exports(bytes: &[u8]) -> Result<Vec<String>, String> {
    if bytes.len() < 64 {
        return Err("ELF header truncated".into());
    }
    let is_64 = bytes[4] == 2;
    if !is_64 {
        return Err("32-bit ELF parsing is not supported in this build".into());
    }
    let e_shoff = u64::from_le_bytes(bytes[40..48].try_into().unwrap()) as usize;
    let e_shentsize = u16::from_le_bytes(bytes[58..60].try_into().unwrap()) as usize;
    let e_shnum = u16::from_le_bytes(bytes[60..62].try_into().unwrap()) as usize;
    let e_shstrndx = u16::from_le_bytes(bytes[62..64].try_into().unwrap()) as usize;
    if e_shoff + e_shnum * e_shentsize > bytes.len() {
        return Err("section header table out of range".into());
    }
    let shstr_off = e_shoff + e_shstrndx * e_shentsize;
    let shstr_section_offset = u64::from_le_bytes(bytes[shstr_off + 24..shstr_off + 32].try_into().unwrap()) as usize;

    let read_cstring = |start: usize| -> &str {
        let end = bytes[start..]
            .iter()
            .position(|&b| b == 0)
            .map(|p| start + p)
            .unwrap_or(bytes.len());
        std::str::from_utf8(&bytes[start..end]).unwrap_or("")
    };

    let mut dynsym_off = None;
    let mut dynsym_size = 0_usize;
    let mut dynsym_entsize = 0_usize;
    let mut dynstr_off = None;
    for i in 0..e_shnum {
        let s = e_shoff + i * e_shentsize;
        let name_idx = u32::from_le_bytes(bytes[s..s + 4].try_into().unwrap()) as usize;
        let sh_type = u32::from_le_bytes(bytes[s + 4..s + 8].try_into().unwrap());
        let sh_off = u64::from_le_bytes(bytes[s + 24..s + 32].try_into().unwrap()) as usize;
        let sh_size = u64::from_le_bytes(bytes[s + 32..s + 40].try_into().unwrap()) as usize;
        let sh_entsize = u64::from_le_bytes(bytes[s + 56..s + 64].try_into().unwrap()) as usize;
        let name = read_cstring(shstr_section_offset + name_idx);
        if sh_type == 11 && name == ".dynsym" {
            dynsym_off = Some(sh_off);
            dynsym_size = sh_size;
            dynsym_entsize = sh_entsize;
        }
        if sh_type == 3 && name == ".dynstr" {
            dynstr_off = Some(sh_off);
        }
    }
    let dynsym = dynsym_off.ok_or("no .dynsym")?;
    let dynstr = dynstr_off.ok_or("no .dynstr")?;
    let mut out = Vec::new();
    let mut i = 0;
    while i + 24 <= dynsym_size {
        let entry = dynsym + i;
        let name_idx = u32::from_le_bytes(bytes[entry..entry + 4].try_into().unwrap()) as usize;
        let info = bytes[entry + 4];
        let value = u64::from_le_bytes(bytes[entry + 8..entry + 16].try_into().unwrap());
        let bind = info >> 4;
        if (bind == 1 || bind == 2) && value != 0 && name_idx > 0 {
            let s = read_cstring(dynstr + name_idx);
            if !s.is_empty() {
                out.push(s.to_string());
            }
        }
        i += dynsym_entsize.max(24);
    }
    Ok(out)
}

pub fn create(lua: &Lua) -> mlua::Result<Table> {
    let t = lua.create_table()?;

    let types = lua.create_table()?;
    macro_rules! ty_ctor {
        ($name:expr, $variant:expr) => {{
            let f = lua.create_function(
                |lua, v: Option<Value>| -> mlua::Result<Value> {
                    match v {
                        None | Some(Value::Nil) => Ok(Value::UserData(
                            lua.create_userdata(CTypeTag($variant))?,
                        )),
                        Some(actual) => {
                            let wrapped = wrap_value($variant, Some(actual))?;
                            Ok(Value::UserData(lua.create_userdata(wrapped)?))
                        }
                    }
                },
            )?;
            types.set($name, f)?;
        }};
    }
    ty_ctor!("Void", CType::Void);
    ty_ctor!("Int8", CType::Int8);
    ty_ctor!("UInt8", CType::UInt8);
    ty_ctor!("Int16", CType::Int16);
    ty_ctor!("UInt16", CType::UInt16);
    ty_ctor!("Int32", CType::Int32);
    ty_ctor!("UInt32", CType::UInt32);
    ty_ctor!("Int64", CType::Int64);
    ty_ctor!("UInt64", CType::UInt64);
    ty_ctor!("ISize", CType::ISize);
    ty_ctor!("USize", CType::USize);
    ty_ctor!("Float", CType::Float);
    ty_ctor!("Double", CType::Double);
    ty_ctor!("Bool", CType::Bool);
    ty_ctor!("Pointer", CType::Pointer);
    ty_ctor!("CString", CType::CString);
    ty_ctor!("Char", CType::Int8);
    ty_ctor!("UChar", CType::UInt8);
    ty_ctor!("Byte", CType::UInt8);
    ty_ctor!("Short", CType::Int16);
    ty_ctor!("UShort", CType::UInt16);
    ty_ctor!("Int", CType::Int32);
    ty_ctor!("UInt", CType::UInt32);
    ty_ctor!("Long", CType::ISize);
    ty_ctor!("ULong", CType::USize);
    ty_ctor!("LongLong", CType::Int64);
    ty_ctor!("ULongLong", CType::UInt64);
    ty_ctor!("Size", CType::USize);
    ty_ctor!("SSize", CType::ISize);
    ty_ctor!("IntPtr", CType::ISize);
    ty_ctor!("UIntPtr", CType::USize);
    t.set("Types", types)?;

    t.set(
        "Load",
        lua.create_function(|_, name: String| -> mlua::Result<PackageHandle> {
            let resolved = validate_and_resolve(&name)?;
            let lib = unsafe { Library::new(&resolved) }.map_err(|e| {
                mlua::Error::RuntimeError(format!(
                    "Package.Load: failed to open '{}': {e}",
                    resolved.display()
                ))
            })?;
            Ok(PackageHandle {
                state: Arc::new(Mutex::new(Some(LoadedLib {
                    lib,
                    source: resolved.to_string_lossy().into_owned(),
                }))),
            })
        })?,
    )?;

    t.set(
        "ReadCString",
        lua.create_function(|lua, ptr: i64| -> mlua::Result<Value> {
            if ptr == 0 {
                return Ok(Value::Nil);
            }
            let p = ptr as usize as *const i8;
            let s = unsafe { CStr::from_ptr(p) }
                .to_string_lossy()
                .into_owned();
            Ok(Value::String(lua.create_string(&s)?))
        })?,
    )?;

    t.set(
        "Buffer",
        lua.create_function(|_, size: i64| -> mlua::Result<PackageBuffer> {
            if size < 0 || size > 1024 * 1024 * 1024 {
                return Err(mlua::Error::RuntimeError(format!(
                    "Package.Buffer: invalid size {size} (0..=1 GiB)"
                )));
            }
            Ok(PackageBuffer::new(size as usize))
        })?,
    )?;

    macro_rules! read_mem {
        ($name:expr, $ty:ty, $into:expr) => {
            t.set(
                $name,
                lua.create_function(
                    |_, (ptr, off): (i64, Option<i64>)| -> mlua::Result<Value> {
                        if ptr == 0 {
                            return Ok(Value::Nil);
                        }
                        let off = off.unwrap_or(0).max(0) as usize;
                        let p = (ptr as usize + off) as *const $ty;
                        let v: $ty = unsafe { p.read_unaligned() };
                        Ok($into(v))
                    },
                )?,
            )?;
        };
    }
    read_mem!("ReadMemoryInt8", i8, |v: i8| Value::Integer(v as i64));
    read_mem!("ReadMemoryUInt8", u8, |v: u8| Value::Integer(v as i64));
    read_mem!("ReadMemoryInt16", i16, |v: i16| Value::Integer(v as i64));
    read_mem!("ReadMemoryUInt16", u16, |v: u16| Value::Integer(v as i64));
    read_mem!("ReadMemoryInt32", i32, |v: i32| Value::Integer(v as i64));
    read_mem!("ReadMemoryUInt32", u32, |v: u32| Value::Integer(v as i64));
    read_mem!("ReadMemoryInt64", i64, Value::Integer);
    read_mem!("ReadMemoryUInt64", u64, |v: u64| {
        if v <= i64::MAX as u64 {
            Value::Integer(v as i64)
        } else {
            Value::Number(v as f64)
        }
    });
    read_mem!("ReadMemoryFloat", f32, |v: f32| Value::Number(v as f64));
    read_mem!("ReadMemoryDouble", f64, Value::Number);
    read_mem!("ReadMemoryPointer", usize, |v: usize| Value::Integer(v as i64));
    read_mem!("ReadMemoryBool", u32, |v: u32| Value::Boolean(v != 0));

    Ok(t)
}
