use mlua::{Function, Lua, Table, Value};

use super::rt;

pub fn serve_fn(lua: &Lua) -> mlua::Result<Function> {
    lua.create_function(|lua, (addr, handler): (String, Function)| -> mlua::Result<()> {
        let server = tiny_http::Server::http(&addr).map_err(rt)?;
        println!("[Net] HTTP listening on {addr}");
        for mut req in server.incoming_requests() {
            let req_table = build_req_table(lua, &mut req)?;
            let result: Value = handler.call(req_table)?;
            let response = build_response(result)?;
            let _ = req.respond(response);
        }
        Ok(())
    })
}

pub fn request_fn(lua: &Lua) -> mlua::Result<Function> {
    lua.create_function(
        |lua,
         (method, url, body, headers): (
            String,
            String,
            Option<String>,
            Option<Table>,
        )|
         -> mlua::Result<Table> {
            let mut req = ureq::request(&method, &url);
            if let Some(h) = headers {
                for pair in h.pairs::<String, String>() {
                    let (k, v) = pair?;
                    req = req.set(&k, &v);
                }
            }
            let response_result = match body {
                Some(b) => req.send_string(&b),
                None => req.call(),
            };
            let res = response_result.map_err(rt)?;
            let result = lua.create_table()?;
            result.set("status", res.status() as i64)?;
            let header_names: Vec<String> = res
                .headers_names()
                .into_iter()
                .map(|s| s.to_string())
                .collect();
            let headers_t = lua.create_table()?;
            for name in &header_names {
                if let Some(v) = res.header(name) {
                    headers_t.set(name.clone(), v.to_string())?;
                }
            }
            result.set("headers", headers_t)?;
            let body_text = res.into_string().map_err(rt)?;
            result.set("body", body_text)?;
            Ok(result)
        },
    )
}

fn build_req_table(lua: &Lua, req: &mut tiny_http::Request) -> mlua::Result<Table> {
    let t = lua.create_table()?;
    t.set("method", req.method().as_str().to_string())?;
    t.set("path", req.url().to_string())?;
    let headers_t = lua.create_table()?;
    for h in req.headers() {
        headers_t.set(h.field.as_str().to_string(), h.value.as_str().to_string())?;
    }
    t.set("headers", headers_t)?;
    let mut body = String::new();
    let _ = req.as_reader().read_to_string(&mut body);
    t.set("body", body)?;
    Ok(t)
}

fn build_response(value: Value) -> mlua::Result<tiny_http::Response<std::io::Cursor<Vec<u8>>>> {
    match value {
        Value::String(s) => {
            let bytes = s.as_bytes().to_vec();
            Ok(tiny_http::Response::from_data(bytes))
        }
        Value::Table(t) => {
            let status: i64 = t.get::<Option<i64>>("status")?.unwrap_or(200);
            let body: String = t.get::<Option<String>>("body")?.unwrap_or_default();
            let mut response = tiny_http::Response::from_data(body.into_bytes())
                .with_status_code(tiny_http::StatusCode(status as u16));
            if let Ok(headers) = t.get::<Table>("headers") {
                for pair in headers.pairs::<String, String>() {
                    let (k, v) = pair?;
                    if let Ok(h) = tiny_http::Header::from_bytes(k.as_bytes(), v.as_bytes()) {
                        response.add_header(h);
                    }
                }
            }
            Ok(response)
        }
        _ => Ok(tiny_http::Response::from_data(b"".to_vec())),
    }
}
