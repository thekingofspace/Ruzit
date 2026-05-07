use std::net::{TcpListener, TcpStream};

use mlua::{Lua, Table, UserData, UserDataMethods};
use tungstenite::{Message, WebSocket, accept, client::IntoClientRequest, connect};

use super::rt;

pub fn create(lua: &Lua) -> mlua::Result<Table> {
    let t = lua.create_table()?;
    t.set(
        "Connect",
        lua.create_function(|_, url: String| -> mlua::Result<WebSocketConn> {
            let req = url.into_client_request().map_err(rt)?;
            let (ws, _) = connect(req).map_err(rt)?;
            Ok(WebSocketConn { ws: Some(ws) })
        })?,
    )?;
    t.set(
        "Host",
        lua.create_function(|_, addr: String| -> mlua::Result<WebSocketListenerHandle> {
            let listener = TcpListener::bind(&addr).map_err(rt)?;
            Ok(WebSocketListenerHandle { listener: Some(listener) })
        })?,
    )?;
    Ok(t)
}

type WsStream = WebSocket<tungstenite::stream::MaybeTlsStream<TcpStream>>;

pub struct WebSocketConn {
    ws: Option<WsStream>,
}

impl UserData for WebSocketConn {
    fn add_methods<M: UserDataMethods<Self>>(m: &mut M) {
        m.add_method_mut("Send", |_, this, msg: String| -> mlua::Result<()> {
            let ws = this
                .ws
                .as_mut()
                .ok_or_else(|| mlua::Error::RuntimeError("WebSocket closed".into()))?;
            ws.send(Message::Text(msg.into())).map_err(rt)
        });
        m.add_method_mut("Receive", |_, this, _: ()| -> mlua::Result<String> {
            let ws = this
                .ws
                .as_mut()
                .ok_or_else(|| mlua::Error::RuntimeError("WebSocket closed".into()))?;
            loop {
                let msg = ws.read().map_err(rt)?;
                match msg {
                    Message::Text(s) => return Ok(s.to_string()),
                    Message::Binary(b) => {
                        return Ok(String::from_utf8_lossy(&b).into_owned());
                    }
                    Message::Ping(p) => {
                        let _ = ws.send(Message::Pong(p));
                    }
                    Message::Pong(_) => continue,
                    Message::Close(_) => {
                        return Err(mlua::Error::RuntimeError("WebSocket closed by peer".into()));
                    }
                    Message::Frame(_) => continue,
                }
            }
        });
        m.add_method_mut("Close", |_, this, _: ()| -> mlua::Result<()> {
            if let Some(mut ws) = this.ws.take() {
                let _ = ws.close(None);
            }
            Ok(())
        });
    }
}

pub struct WebSocketServerConn {
    ws: Option<WebSocket<TcpStream>>,
}

impl UserData for WebSocketServerConn {
    fn add_methods<M: UserDataMethods<Self>>(m: &mut M) {
        m.add_method_mut("Send", |_, this, msg: String| -> mlua::Result<()> {
            let ws = this
                .ws
                .as_mut()
                .ok_or_else(|| mlua::Error::RuntimeError("WebSocket closed".into()))?;
            ws.send(Message::Text(msg.into())).map_err(rt)
        });
        m.add_method_mut("Receive", |_, this, _: ()| -> mlua::Result<String> {
            let ws = this
                .ws
                .as_mut()
                .ok_or_else(|| mlua::Error::RuntimeError("WebSocket closed".into()))?;
            loop {
                let msg = ws.read().map_err(rt)?;
                match msg {
                    Message::Text(s) => return Ok(s.to_string()),
                    Message::Binary(b) => {
                        return Ok(String::from_utf8_lossy(&b).into_owned());
                    }
                    Message::Ping(p) => {
                        let _ = ws.send(Message::Pong(p));
                    }
                    Message::Pong(_) => continue,
                    Message::Close(_) => {
                        return Err(mlua::Error::RuntimeError("WebSocket closed by peer".into()));
                    }
                    Message::Frame(_) => continue,
                }
            }
        });
        m.add_method_mut("Close", |_, this, _: ()| -> mlua::Result<()> {
            if let Some(mut ws) = this.ws.take() {
                let _ = ws.close(None);
            }
            Ok(())
        });
    }
}

pub struct WebSocketListenerHandle {
    listener: Option<TcpListener>,
}

impl UserData for WebSocketListenerHandle {
    fn add_methods<M: UserDataMethods<Self>>(m: &mut M) {
        m.add_method_mut(
            "Accept",
            |_, this, _: ()| -> mlua::Result<WebSocketServerConn> {
                let l = this
                    .listener
                    .as_mut()
                    .ok_or_else(|| mlua::Error::RuntimeError("WebSocket listener closed".into()))?;
                let (stream, _) = l.accept().map_err(rt)?;
                let ws = accept(stream).map_err(|e| rt(format!("{e:?}")))?;
                Ok(WebSocketServerConn { ws: Some(ws) })
            },
        );
        m.add_method_mut("Close", |_, this, _: ()| -> mlua::Result<()> {
            this.listener = None;
            Ok(())
        });
    }
}
