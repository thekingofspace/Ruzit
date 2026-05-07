use std::io::{Read, Write};

use interprocess::local_socket::{
    GenericNamespaced, Listener as IpcListener, ListenerOptions, Stream as IpcStream, ToNsName,
    prelude::*,
};

use mlua::{Lua, Table, UserData, UserDataMethods};

use super::rt;

pub fn create(lua: &Lua) -> mlua::Result<Table> {
    let t = lua.create_table()?;
    t.set(
        "Connect",
        lua.create_function(|_, name: String| -> mlua::Result<IpcConnection> {
            let ns = name.clone().to_ns_name::<GenericNamespaced>().map_err(rt)?;
            let stream = IpcStream::connect(ns).map_err(rt)?;
            Ok(IpcConnection { stream: Some(stream) })
        })?,
    )?;
    t.set(
        "Host",
        lua.create_function(|_, name: String| -> mlua::Result<IpcListenerHandle> {
            let ns = name.clone().to_ns_name::<GenericNamespaced>().map_err(rt)?;
            let listener = ListenerOptions::new()
                .name(ns)
                .create_sync()
                .map_err(rt)?;
            Ok(IpcListenerHandle { listener: Some(listener) })
        })?,
    )?;
    Ok(t)
}

pub struct IpcConnection {
    stream: Option<IpcStream>,
}

impl UserData for IpcConnection {
    fn add_methods<M: UserDataMethods<Self>>(m: &mut M) {
        m.add_method_mut("Send", |_, this, data: String| -> mlua::Result<()> {
            let s = this
                .stream
                .as_mut()
                .ok_or_else(|| mlua::Error::RuntimeError("IPC connection closed".into()))?;
            s.write_all(data.as_bytes()).map_err(rt)
        });
        m.add_method_mut(
            "Receive",
            |_, this, n: Option<usize>| -> mlua::Result<String> {
                let s = this
                    .stream
                    .as_mut()
                    .ok_or_else(|| mlua::Error::RuntimeError("IPC connection closed".into()))?;
                let n = n.unwrap_or(4096);
                let mut buf = vec![0u8; n];
                let read = s.read(&mut buf).map_err(rt)?;
                Ok(String::from_utf8_lossy(&buf[..read]).into_owned())
            },
        );
        m.add_method_mut("Close", |_, this, _: ()| -> mlua::Result<()> {
            this.stream = None;
            Ok(())
        });
    }
}

pub struct IpcListenerHandle {
    listener: Option<IpcListener>,
}

impl UserData for IpcListenerHandle {
    fn add_methods<M: UserDataMethods<Self>>(m: &mut M) {
        m.add_method_mut(
            "Accept",
            |_, this, _: ()| -> mlua::Result<IpcConnection> {
                let l = this
                    .listener
                    .as_mut()
                    .ok_or_else(|| mlua::Error::RuntimeError("IPC listener closed".into()))?;
                let stream = l.accept().map_err(rt)?;
                Ok(IpcConnection { stream: Some(stream) })
            },
        );
        m.add_method_mut("Close", |_, this, _: ()| -> mlua::Result<()> {
            this.listener = None;
            Ok(())
        });
    }
}
