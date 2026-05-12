use std::collections::HashMap;

use super::FrameRect;

pub enum FrameRef {
    Index(usize),
    Name(String),
}

pub struct ParsedAnimation {
    pub frames: Vec<FrameRef>,
    pub fps: f32,
    pub looped: bool,
}

pub struct ParsedAnim {
    pub frames: Vec<FrameRect>,
    pub name_to_frame: HashMap<String, usize>,
    pub animations: HashMap<String, ParsedAnimation>,
}

struct Tag<'a> {
    name: &'a str,
    attrs: HashMap<String, String>,
    closing: bool,
}

fn err(msg: impl Into<String>) -> mlua::Error {
    mlua::Error::RuntimeError(format!("AnimatedImage XML: {}", msg.into()))
}

pub fn parse_animated_xml(
    text: &str,
    image_w: u32,
    image_h: u32,
) -> mlua::Result<ParsedAnim> {
    let bytes = text.as_bytes();
    let mut i: usize = 0;
    let mut tags: Vec<Tag> = Vec::new();

    while i < bytes.len() {
        skip_whitespace(bytes, &mut i);
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
            skip_whitespace(bytes, &mut p);
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
            skip_whitespace(bytes, &mut p);
            if p >= bytes.len() || bytes[p] != b'=' {
                attrs.insert(key, String::new());
                continue;
            }
            p += 1;
            skip_whitespace(bytes, &mut p);
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

    let mut frames: Vec<FrameRect> = Vec::new();
    let mut name_to_frame: HashMap<String, usize> = HashMap::new();
    let mut animations: HashMap<String, ParsedAnimation> = HashMap::new();

    let root = tags
        .iter()
        .find(|t| !t.closing && t.name == "animated")
        .ok_or_else(|| err("missing <animated> root element"))?;

    let frame_width = root.attrs.get("frame_width").and_then(|s| s.parse::<u32>().ok());
    let frame_height = root
        .attrs
        .get("frame_height")
        .and_then(|s| s.parse::<u32>().ok());
    let columns = root.attrs.get("columns").and_then(|s| s.parse::<u32>().ok());

    let grid_mode = frame_width.is_some() && frame_height.is_some();
    if grid_mode {
        let fw = frame_width.unwrap().max(1);
        let fh = frame_height.unwrap().max(1);
        let cols = columns.unwrap_or((image_w / fw).max(1));
        let rows = (image_h / fh).max(1);
        for r in 0..rows {
            for c in 0..cols {
                frames.push(FrameRect {
                    x: c * fw,
                    y: r * fh,
                    w: fw,
                    h: fh,
                });
            }
        }
    }

    for tag in &tags {
        if tag.closing || tag.name != "frame" {
            continue;
        }
        let x = tag.attrs.get("x").and_then(|s| s.parse::<u32>().ok()).unwrap_or(0);
        let y = tag.attrs.get("y").and_then(|s| s.parse::<u32>().ok()).unwrap_or(0);
        let w = tag
            .attrs
            .get("w")
            .or_else(|| tag.attrs.get("width"))
            .and_then(|s| s.parse::<u32>().ok())
            .unwrap_or(1);
        let h = tag
            .attrs
            .get("h")
            .or_else(|| tag.attrs.get("height"))
            .and_then(|s| s.parse::<u32>().ok())
            .unwrap_or(1);
        let idx = frames.len();
        frames.push(FrameRect { x, y, w, h });
        if let Some(n) = tag.attrs.get("name") {
            name_to_frame.insert(n.clone(), idx);
        }
    }

    for tag in &tags {
        if tag.closing || tag.name != "animation" {
            continue;
        }
        let name = tag
            .attrs
            .get("name")
            .cloned()
            .ok_or_else(|| err("<animation> requires a name attribute"))?;
        let fps = tag
            .attrs
            .get("fps")
            .and_then(|s| s.parse::<f32>().ok())
            .unwrap_or(10.0)
            .max(0.001);
        let looped = tag
            .attrs
            .get("looped")
            .map(|s| matches!(s.as_str(), "true" | "1" | "yes" | "True"))
            .unwrap_or(false);

        let frames_attr = tag.attrs.get("frames").cloned().unwrap_or_default();
        let mut frame_refs: Vec<FrameRef> = Vec::new();
        for part in frames_attr.split(',') {
            let p = part.trim();
            if p.is_empty() {
                continue;
            }
            if let Ok(n) = p.parse::<usize>() {
                frame_refs.push(FrameRef::Index(n));
            } else if let Some(range) = p.split_once('-') {
                if let (Ok(a), Ok(b)) =
                    (range.0.trim().parse::<usize>(), range.1.trim().parse::<usize>())
                {
                    let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
                    for k in lo..=hi {
                        frame_refs.push(FrameRef::Index(k));
                    }
                } else {
                    frame_refs.push(FrameRef::Name(p.to_string()));
                }
            } else {
                frame_refs.push(FrameRef::Name(p.to_string()));
            }
        }

        animations.insert(
            name,
            ParsedAnimation {
                frames: frame_refs,
                fps,
                looped,
            },
        );
    }

    Ok(ParsedAnim {
        frames,
        name_to_frame,
        animations,
    })
}

fn skip_whitespace(bytes: &[u8], i: &mut usize) {
    while *i < bytes.len() && bytes[*i].is_ascii_whitespace() {
        *i += 1;
    }
}
