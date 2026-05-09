use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};

use mlua::{Lua, Table, UserData, UserDataMethods};

use super::rt;

pub fn create(lua: &Lua) -> mlua::Result<Table> {
    let t = lua.create_table()?;
    t.set(
        "Connect",
        lua.create_function(|_, addr: String| -> mlua::Result<TcpConnection> {
            let stream = TcpStream::connect(&addr).map_err(rt)?;
            Ok(TcpConnection {
                stream: Some(stream),
            })
        })?,
    )?;
    t.set(
        "Host",
        lua.create_function(|_, addr: String| -> mlua::Result<TcpListenerHandle> {
            let listener = TcpListener::bind(&addr).map_err(rt)?;
            Ok(TcpListenerHandle {
                listener: Some(listener),
            })
        })?,
    )?;
    Ok(t)
}

pub struct TcpConnection {
    stream: Option<TcpStream>,
}

impl UserData for TcpConnection {
    fn add_methods<M: UserDataMethods<Self>>(m: &mut M) {
        m.add_method_mut("Send", |_, this, data: String| -> mlua::Result<()> {
            let s = this
                .stream
                .as_mut()
                .ok_or_else(|| mlua::Error::RuntimeError("TCP connection closed".into()))?;
            s.write_all(data.as_bytes()).map_err(rt)
        });
        m.add_method_mut(
            "Receive",
            |_, this, n: Option<usize>| -> mlua::Result<String> {
                let s = this
                    .stream
                    .as_mut()
                    .ok_or_else(|| mlua::Error::RuntimeError("TCP connection closed".into()))?;
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
        m.add_method("Peer", |_, this, _: ()| -> mlua::Result<String> {
            let s = this
                .stream
                .as_ref()
                .ok_or_else(|| mlua::Error::RuntimeError("TCP connection closed".into()))?;
            Ok(s.peer_addr().map_err(rt)?.to_string())
        });
    }
}

pub struct TcpListenerHandle {
    listener: Option<TcpListener>,
}

impl UserData for TcpListenerHandle {
    fn add_methods<M: UserDataMethods<Self>>(m: &mut M) {
        m.add_method_mut("Accept", |_, this, _: ()| -> mlua::Result<TcpConnection> {
            let l = this
                .listener
                .as_mut()
                .ok_or_else(|| mlua::Error::RuntimeError("TCP listener closed".into()))?;
            let (stream, _) = l.accept().map_err(rt)?;
            Ok(TcpConnection {
                stream: Some(stream),
            })
        });
        m.add_method_mut("Close", |_, this, _: ()| -> mlua::Result<()> {
            this.listener = None;
            Ok(())
        });
        m.add_method("Address", |_, this, _: ()| -> mlua::Result<String> {
            let l = this
                .listener
                .as_ref()
                .ok_or_else(|| mlua::Error::RuntimeError("TCP listener closed".into()))?;
            Ok(l.local_addr().map_err(rt)?.to_string())
        });
    }
}
