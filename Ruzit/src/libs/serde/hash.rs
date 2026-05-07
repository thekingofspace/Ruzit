use base64::{Engine, engine::general_purpose::STANDARD as B64};
use mlua::{Function, Lua, String as LuaString};
use sha1::Digest;

use super::codec::rt;

pub fn hash_fn(lua: &Lua) -> mlua::Result<Function> {
    lua.create_function(
        |lua,
         (algo, data, encoding): (String, LuaString, Option<String>)|
         -> mlua::Result<LuaString> {
            let bytes = data.as_bytes();
            let digest: Vec<u8> = match algo.to_lowercase().as_str() {
                "md5" => {
                    let mut h = md5::Md5::new();
                    h.update(&bytes);
                    h.finalize().to_vec()
                }
                "sha1" => {
                    let mut h = sha1::Sha1::new();
                    h.update(&bytes);
                    h.finalize().to_vec()
                }
                "sha224" => {
                    let mut h = sha2::Sha224::new();
                    h.update(&bytes);
                    h.finalize().to_vec()
                }
                "sha256" => {
                    let mut h = sha2::Sha256::new();
                    h.update(&bytes);
                    h.finalize().to_vec()
                }
                "sha384" => {
                    let mut h = sha2::Sha384::new();
                    h.update(&bytes);
                    h.finalize().to_vec()
                }
                "sha512" => {
                    let mut h = sha2::Sha512::new();
                    h.update(&bytes);
                    h.finalize().to_vec()
                }
                "sha3-256" | "sha3_256" => {
                    let mut h = sha3::Sha3_256::new();
                    h.update(&bytes);
                    h.finalize().to_vec()
                }
                "sha3-512" | "sha3_512" => {
                    let mut h = sha3::Sha3_512::new();
                    h.update(&bytes);
                    h.finalize().to_vec()
                }
                "keccak256" => {
                    let mut h = sha3::Keccak256::new();
                    h.update(&bytes);
                    h.finalize().to_vec()
                }
                "blake3" => blake3::hash(&bytes).as_bytes().to_vec(),
                other => {
                    return Err(mlua::Error::RuntimeError(format!(
                        "Serde.Hash: unknown algorithm '{other}'"
                    )));
                }
            };

            let encoding = encoding
                .unwrap_or_else(|| "hex".to_string())
                .to_lowercase();
            match encoding.as_str() {
                "hex" => lua.create_string(hex::encode(&digest)),
                "hex-upper" | "HEX" => lua.create_string(hex::encode_upper(&digest)),
                "base64" | "b64" => lua.create_string(B64.encode(&digest)),
                "bytes" | "raw" => lua.create_string(&digest),
                other => Err(mlua::Error::RuntimeError(format!(
                    "Serde.Hash: unknown encoding '{other}' (try hex, base64, bytes)"
                ))),
            }
            .map_err(|e| rt(e))
        },
    )
}
