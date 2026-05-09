use std::net::UdpSocket;

use mlua::{Lua, MultiValue, Table, UserData, UserDataMethods, Value};

use super::rt;

pub fn create(lua: &Lua) -> mlua::Result<Table> {
    let t = lua.create_table()?;
    t.set(
        "Bind",
        lua.create_function(|_, addr: String| -> mlua::Result<UdpHandle> {
            let socket = UdpSocket::bind(&addr).map_err(rt)?;
            Ok(UdpHandle {
                socket: Some(socket),
            })
        })?,
    )?;
    Ok(t)
}

pub struct UdpHandle {
    socket: Option<UdpSocket>,
}

impl UserData for UdpHandle {
    fn add_methods<M: UserDataMethods<Self>>(m: &mut M) {
        m.add_method_mut(
            "Send",
            |_, this, (addr, data): (String, String)| -> mlua::Result<usize> {
                let s = this
                    .socket
                    .as_ref()
                    .ok_or_else(|| mlua::Error::RuntimeError("UDP socket closed".into()))?;
                Ok(s.send_to(data.as_bytes(), &addr).map_err(rt)?)
            },
        );
        m.add_method_mut(
            "Receive",
            |lua, this, n: Option<usize>| -> mlua::Result<MultiValue> {
                let s = this
                    .socket
                    .as_ref()
                    .ok_or_else(|| mlua::Error::RuntimeError("UDP socket closed".into()))?;
                let n = n.unwrap_or(65507);
                let mut buf = vec![0u8; n];
                let (read, addr) = s.recv_from(&mut buf).map_err(rt)?;
                let body = String::from_utf8_lossy(&buf[..read]).into_owned();
                let mut mv = MultiValue::new();
                mv.push_back(Value::String(lua.create_string(&body)?));
                mv.push_back(Value::String(lua.create_string(addr.to_string())?));
                Ok(mv)
            },
        );
        m.add_method_mut("Close", |_, this, _: ()| -> mlua::Result<()> {
            this.socket = None;
            Ok(())
        });
        m.add_method("Address", |_, this, _: ()| -> mlua::Result<String> {
            let s = this
                .socket
                .as_ref()
                .ok_or_else(|| mlua::Error::RuntimeError("UDP socket closed".into()))?;
            Ok(s.local_addr().map_err(rt)?.to_string())
        });
    }
}
