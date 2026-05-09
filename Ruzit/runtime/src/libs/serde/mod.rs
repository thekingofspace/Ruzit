mod codec;
mod compress;
mod hash;

use mlua::{Lua, Table};

pub fn create(lua: &Lua) -> mlua::Result<Table> {
    let t = lua.create_table()?;
    t.set("Encode", codec::encode_fn(lua)?)?;
    t.set("Decode", codec::decode_fn(lua)?)?;
    t.set("Hash", hash::hash_fn(lua)?)?;
    t.set("Compress", compress::compress_fn(lua)?)?;
    t.set("Decompress", compress::decompress_fn(lua)?)?;
    Ok(t)
}
