use std::fs;
use std::io::{Read, Write};
use std::path::PathBuf;

use mlua::{UserData, UserDataMethods, Value};

pub struct IoHandle {
    pub file: Option<fs::File>,
    pub path: PathBuf,
}

fn err(action: &str, p: &PathBuf, e: impl std::fmt::Display) -> mlua::Error {
    mlua::Error::RuntimeError(format!("IO.{action} {}: {e}", p.display()))
}

impl UserData for IoHandle {
    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        methods.add_method_mut("read", |_, this, _: Value| -> mlua::Result<String> {
            let f = this
                .file
                .as_mut()
                .ok_or_else(|| mlua::Error::RuntimeError("file is closed".to_string()))?;
            let mut buf = String::new();
            f.read_to_string(&mut buf)
                .map_err(|e| err("read", &this.path, e))?;
            Ok(buf)
        });
        methods.add_method_mut("write", |_, this, content: String| -> mlua::Result<()> {
            let f = this
                .file
                .as_mut()
                .ok_or_else(|| mlua::Error::RuntimeError("file is closed".to_string()))?;
            f.write_all(content.as_bytes())
                .map_err(|e| err("write", &this.path, e))
        });
        methods.add_method_mut("close", |_, this, _: ()| -> mlua::Result<()> {
            this.file = None;
            Ok(())
        });
        methods.add_method("path", |_, this, _: ()| -> mlua::Result<String> {
            Ok(this.path.to_string_lossy().into_owned())
        });
    }
}
