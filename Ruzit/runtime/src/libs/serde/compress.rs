use std::io::{Read, Write};

use flate2::Compression;
use flate2::read::{DeflateDecoder, GzDecoder, ZlibDecoder};
use flate2::write::{DeflateEncoder, GzEncoder, ZlibEncoder};

use mlua::{Function, Lua, String as LuaString};

use super::codec::rt;

pub fn compress_fn(lua: &Lua) -> mlua::Result<Function> {
    lua.create_function(
        |lua, (algo, data, level): (String, LuaString, Option<i32>)| -> mlua::Result<LuaString> {
            let bytes = data.as_bytes();
            let compressed: Vec<u8> = match algo.to_lowercase().as_str() {
                "gzip" => {
                    let lvl = flate_level(level);
                    let mut e = GzEncoder::new(Vec::new(), lvl);
                    e.write_all(&bytes).map_err(rt)?;
                    e.finish().map_err(rt)?
                }
                "zlib" => {
                    let lvl = flate_level(level);
                    let mut e = ZlibEncoder::new(Vec::new(), lvl);
                    e.write_all(&bytes).map_err(rt)?;
                    e.finish().map_err(rt)?
                }
                "deflate" => {
                    let lvl = flate_level(level);
                    let mut e = DeflateEncoder::new(Vec::new(), lvl);
                    e.write_all(&bytes).map_err(rt)?;
                    e.finish().map_err(rt)?
                }
                "zstd" => {
                    let lvl = level.unwrap_or(3);
                    zstd::stream::encode_all(bytes.as_ref(), lvl).map_err(rt)?
                }
                other => {
                    return Err(mlua::Error::RuntimeError(format!(
                        "Serde.Compress: unknown algorithm '{other}'"
                    )));
                }
            };
            lua.create_string(&compressed)
        },
    )
}

pub fn decompress_fn(lua: &Lua) -> mlua::Result<Function> {
    lua.create_function(
        |lua, (algo, data): (String, LuaString)| -> mlua::Result<LuaString> {
            let bytes = data.as_bytes();
            let mut out = Vec::new();
            match algo.to_lowercase().as_str() {
                "gzip" => {
                    let mut d = GzDecoder::new(bytes.as_ref());
                    d.read_to_end(&mut out).map_err(rt)?;
                }
                "zlib" => {
                    let mut d = ZlibDecoder::new(bytes.as_ref());
                    d.read_to_end(&mut out).map_err(rt)?;
                }
                "deflate" => {
                    let mut d = DeflateDecoder::new(bytes.as_ref());
                    d.read_to_end(&mut out).map_err(rt)?;
                }
                "zstd" => {
                    out = zstd::stream::decode_all(bytes.as_ref()).map_err(rt)?;
                }
                other => {
                    return Err(mlua::Error::RuntimeError(format!(
                        "Serde.Decompress: unknown algorithm '{other}'"
                    )));
                }
            }
            lua.create_string(&out)
        },
    )
}

fn flate_level(level: Option<i32>) -> Compression {
    match level {
        Some(l) => Compression::new(l.clamp(0, 9) as u32),
        None => Compression::default(),
    }
}
