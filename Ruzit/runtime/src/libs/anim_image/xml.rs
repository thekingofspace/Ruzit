use std::collections::HashMap;

pub struct ParsedFrame {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
    pub frame_x: i32,
    pub frame_y: i32,
    pub frame_width: u32,
    pub frame_height: u32,
}

pub struct ParsedAtlas {
    pub frames: Vec<(String, ParsedFrame)>,
    pub name_to_index: HashMap<String, usize>,
}

struct Tag<'a> {
    name: &'a str,
    attrs: HashMap<String, String>,
    closing: bool,
}

fn err(msg: impl Into<String>) -> mlua::Error {
    mlua::Error::RuntimeError(format!("AnimatedImage XML: {}", msg.into()))
}

pub fn parse_texture_atlas(text: &str) -> mlua::Result<ParsedAtlas> {
    let bytes = text.as_bytes();
    let mut i: usize = 0;
    let mut tags: Vec<Tag> = Vec::new();

    while i < bytes.len() {
        skip_ws(bytes, &mut i);
        if i >= bytes.len() {
            break;
        }

        if i + 4 <= bytes.len() && &bytes[i..i + 4] == b"<!--" {
            i += 4;
            while i + 3 <= bytes.len() && &bytes[i..i + 3] != b"-->" {
                i += 1;
            }
            i = i.saturating_add(3).min(bytes.len());
            continue;
        }

        if i + 2 <= bytes.len() && &bytes[i..i + 2] == b"<?" {
            while i + 2 <= bytes.len() && &bytes[i..i + 2] != b"?>" {
                i += 1;
            }
            i = i.saturating_add(2).min(bytes.len());
            continue;
        }

        if bytes[i] != b'<' {
            i += 1;
            continue;
        }

        let tag_start = i + 1;
        let mut closing = false;
        let mut p = tag_start;
        if p < bytes.len() && bytes[p] == b'/' {
            closing = true;
            p += 1;
        }
        let name_start = p;
        while p < bytes.len()
            && !bytes[p].is_ascii_whitespace()
            && bytes[p] != b'/'
            && bytes[p] != b'>'
        {
            p += 1;
        }
        let name = std::str::from_utf8(&bytes[name_start..p])
            .map_err(|_| err("non-UTF-8 in tag name"))?;
        let mut attrs: HashMap<String, String> = HashMap::new();

        loop {
            skip_ws(bytes, &mut p);
            if p >= bytes.len() {
                return Err(err("unexpected EOF inside tag"));
            }
            if bytes[p] == b'>' {
                p += 1;
                break;
            }
            if bytes[p] == b'/' {
                p += 1;
                while p < bytes.len() && bytes[p] != b'>' {
                    p += 1;
                }
                if p < bytes.len() {
                    p += 1;
                }
                break;
            }
            let k_start = p;
            while p < bytes.len()
                && bytes[p] != b'='
                && !bytes[p].is_ascii_whitespace()
                && bytes[p] != b'>'
                && bytes[p] != b'/'
            {
                p += 1;
            }
            let key = std::str::from_utf8(&bytes[k_start..p])
                .map_err(|_| err("non-UTF-8 in attr name"))?
                .to_string();
            skip_ws(bytes, &mut p);
            if p >= bytes.len() || bytes[p] != b'=' {
                attrs.insert(key, String::new());
                continue;
            }
            p += 1;
            skip_ws(bytes, &mut p);
            if p >= bytes.len() || (bytes[p] != b'"' && bytes[p] != b'\'') {
                return Err(err("expected quoted attribute value"));
            }
            let quote = bytes[p];
            p += 1;
            let v_start = p;
            while p < bytes.len() && bytes[p] != quote {
                p += 1;
            }
            if p >= bytes.len() {
                return Err(err("unterminated attribute value"));
            }
            let value = std::str::from_utf8(&bytes[v_start..p])
                .map_err(|_| err("non-UTF-8 in attr value"))?
                .to_string();
            p += 1;
            attrs.insert(key, value);
        }

        tags.push(Tag {
            name,
            attrs,
            closing,
        });
        i = p;
    }

    let _root = tags
        .iter()
        .find(|t| !t.closing && t.name.eq_ignore_ascii_case("TextureAtlas"))
        .ok_or_else(|| err("missing <TextureAtlas> root element"))?;

    let mut frames: Vec<(String, ParsedFrame)> = Vec::new();
    let mut name_to_index: HashMap<String, usize> = HashMap::new();

    for tag in &tags {
        if tag.closing || !tag.name.eq_ignore_ascii_case("SubTexture") {
            continue;
        }
        let name = tag
            .attrs
            .get("name")
            .cloned()
            .ok_or_else(|| err("<SubTexture> requires a name attribute"))?;
        let x = parse_u32(tag.attrs.get("x"), 0);
        let y = parse_u32(tag.attrs.get("y"), 0);
        let width = parse_u32(tag.attrs.get("width"), 1);
        let height = parse_u32(tag.attrs.get("height"), 1);
        let frame_x = parse_i32(tag.attrs.get("frameX"), 0);
        let frame_y = parse_i32(tag.attrs.get("frameY"), 0);
        let frame_width = parse_u32(tag.attrs.get("frameWidth"), width);
        let frame_height = parse_u32(tag.attrs.get("frameHeight"), height);

        let idx = frames.len();
        name_to_index.insert(name.clone(), idx);
        frames.push((
            name,
            ParsedFrame {
                x,
                y,
                width,
                height,
                frame_x,
                frame_y,
                frame_width,
                frame_height,
            },
        ));
    }

    Ok(ParsedAtlas {
        frames,
        name_to_index,
    })
}

fn parse_u32(s: Option<&String>, default: u32) -> u32 {
    s.and_then(|x| x.parse::<i64>().ok())
        .map(|n| n.max(0) as u32)
        .unwrap_or(default)
}

fn parse_i32(s: Option<&String>, default: i32) -> i32 {
    s.and_then(|x| x.parse::<i32>().ok()).unwrap_or(default)
}

fn skip_ws(bytes: &[u8], i: &mut usize) {
    while *i < bytes.len() && bytes[*i].is_ascii_whitespace() {
        *i += 1;
    }
}
