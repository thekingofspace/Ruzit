mod http;
mod ipc;
mod socket;
mod tcp;
mod udp;

use mlua::{Lua, Table};

pub fn create(lua: &Lua) -> mlua::Result<Table> {
    let t = lua.create_table()?;

    t.set("Serve", http::serve_fn(lua)?)?;
    t.set("Request", http::request_fn(lua)?)?;
    t.set("TCP", tcp::create(lua)?)?;
    t.set("UDP", udp::create(lua)?)?;
    t.set("IPC", ipc::create(lua)?)?;
    t.set("Socket", socket::create(lua)?)?;

    Ok(t)
}

pub(crate) fn rt<E: std::fmt::Display>(e: E) -> mlua::Error {
    mlua::Error::RuntimeError(e.to_string())
}
