//! Shared input state. Winit events on the window thread feed this module via
//! `on_*` hooks; the Mouse and Keyboard library facades read the state and
//! the heart loop drives `pump()` to fire queued signals on the Lua side.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};

use mlua::{Lua, MultiValue, Table, Value};
use winit::event::{ElementState, MouseButton};
use winit::keyboard::{Key, NamedKey, PhysicalKey};
use winit::window::{CursorIcon, Window as WinitWindow};

use crate::libs::primitives::Dim;
use crate::libs::signal;

// Signals stashed in the named registry so both the Mouse/Keyboard facades
// and `pump` can find them without threading state through every call.
const MOUSE_MOVED_KEY: &str = "ruzit_mouse_moved";
const MOUSE_INPUT_KEY: &str = "ruzit_mouse_input";
const KEYBOARD_INPUT_KEY: &str = "ruzit_keyboard_input";

thread_local! {
    static STATE: RefCell<InputState> = RefCell::new(InputState::default());
}

#[derive(Default)]
struct InputState {
    // Mouse — last cursor position in window-local pixels (top-left origin).
    cursor_x: f32,
    cursor_y: f32,
    // Buttons currently held. Stored as canonical names.
    mouse_down: HashSet<String>,
    // Keys currently held; both name and id are tracked so IsKeyDown can
    // accept either form without re-resolving on every check.
    keys_down: HashMap<String, u32>,
    // What the user asked for via Mouse:SetCursor / .Visible / .Locked.
    desired_cursor: CursorKind,
    desired_visible: bool,
    desired_locked: bool,
    focused: bool,
    // Center-lock baseline. While locked we re-center after each Moved and
    // report deltas relative to this; otherwise it tracks the actual cursor
    // and reported deltas come from the previous position.
    last_x: f32,
    last_y: f32,
    // Queued events drained by `pump` and fired on the Lua thread.
    pending_moves: Vec<MoveEvent>,
    pending_mouse_input: Vec<MouseInputEvent>,
    pending_key_input: Vec<KeyInputEvent>,
}

#[derive(Clone, Copy)]
struct MoveEvent {
    x: f32,
    y: f32,
    dx: f32,
    dy: f32,
}

#[derive(Clone)]
struct MouseInputEvent {
    button: String,
    pressed: bool,
}

#[derive(Clone)]
struct KeyInputEvent {
    id: u32,
    name: String,
    pressed: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum CursorKind {
    Default,
    Pointer,
    Crosshair,
    Text,
    Wait,
    Help,
    Move,
    NotAllowed,
    Grab,
    Grabbing,
}

impl Default for CursorKind {
    fn default() -> Self {
        Self::Default
    }
}

impl CursorKind {
    pub fn from_name(s: &str) -> mlua::Result<Self> {
        let lower = s.to_ascii_lowercase();
        Ok(match lower.as_str() {
            "default" | "arrow" => Self::Default,
            "pointer" | "hand" => Self::Pointer,
            "crosshair" | "cross" => Self::Crosshair,
            "text" | "ibeam" => Self::Text,
            "wait" | "loading" => Self::Wait,
            "help" => Self::Help,
            "move" | "all-scroll" | "allscroll" => Self::Move,
            "notallowed" | "no" => Self::NotAllowed,
            "grab" => Self::Grab,
            "grabbing" => Self::Grabbing,
            other => {
                return Err(mlua::Error::RuntimeError(format!(
                    "Mouse:SetCursor: unknown cursor '{other}'"
                )));
            }
        })
    }

    pub fn to_winit(self) -> CursorIcon {
        match self {
            Self::Default => CursorIcon::Default,
            Self::Pointer => CursorIcon::Pointer,
            Self::Crosshair => CursorIcon::Crosshair,
            Self::Text => CursorIcon::Text,
            Self::Wait => CursorIcon::Wait,
            Self::Help => CursorIcon::Help,
            Self::Move => CursorIcon::Move,
            Self::NotAllowed => CursorIcon::NotAllowed,
            Self::Grab => CursorIcon::Grab,
            Self::Grabbing => CursorIcon::Grabbing,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Default => "Default",
            Self::Pointer => "Pointer",
            Self::Crosshair => "Crosshair",
            Self::Text => "Text",
            Self::Wait => "Wait",
            Self::Help => "Help",
            Self::Move => "Move",
            Self::NotAllowed => "NotAllowed",
            Self::Grab => "Grab",
            Self::Grabbing => "Grabbing",
        }
    }
}

pub fn install(lua: &Lua) -> mlua::Result<()> {
    STATE.with(|s| {
        let mut st = s.borrow_mut();
        st.desired_cursor = CursorKind::Default;
        st.desired_visible = true;
        st.desired_locked = false;
    });
    let mouse_moved = signal::new_instance(lua)?;
    let mouse_input = signal::new_instance(lua)?;
    let keyboard_input = signal::new_instance(lua)?;
    lua.set_named_registry_value(MOUSE_MOVED_KEY, mouse_moved)?;
    lua.set_named_registry_value(MOUSE_INPUT_KEY, mouse_input)?;
    lua.set_named_registry_value(KEYBOARD_INPUT_KEY, keyboard_input)?;
    Ok(())
}

pub fn moved_signal(lua: &Lua) -> mlua::Result<Table> {
    lua.named_registry_value(MOUSE_MOVED_KEY)
}

pub fn mouse_input_signal(lua: &Lua) -> mlua::Result<Table> {
    lua.named_registry_value(MOUSE_INPUT_KEY)
}

pub fn keyboard_input_signal(lua: &Lua) -> mlua::Result<Table> {
    lua.named_registry_value(KEYBOARD_INPUT_KEY)
}

// ---------------------------------------------------------------------------
// Reads — Mouse / Keyboard facades use these
// ---------------------------------------------------------------------------

pub fn cursor_position() -> Dim {
    STATE.with(|s| {
        let st = s.borrow();
        Dim::new(st.cursor_x, st.cursor_y)
    })
}

pub fn is_mouse_button_down(name: &str) -> bool {
    STATE.with(|s| s.borrow().mouse_down.contains(name))
}

pub fn is_locked() -> bool {
    STATE.with(|s| s.borrow().desired_locked)
}

pub fn is_cursor_visible() -> bool {
    STATE.with(|s| s.borrow().desired_visible)
}

pub fn cursor_kind_name() -> &'static str {
    STATE.with(|s| s.borrow().desired_cursor.name())
}

pub fn is_key_down_by_name(name: &str) -> bool {
    STATE.with(|s| s.borrow().keys_down.contains_key(name))
}

pub fn is_key_down_by_id(id: u32) -> bool {
    STATE.with(|s| s.borrow().keys_down.values().any(|v| *v == id))
}

// ---------------------------------------------------------------------------
// Writes — Mouse facade applies these and hands the cursor to the window
// ---------------------------------------------------------------------------

pub fn set_cursor(window: Option<&WinitWindow>, kind: CursorKind) {
    STATE.with(|s| s.borrow_mut().desired_cursor = kind);
    if let Some(w) = window {
        // While unfocused we keep the OS cursor at default so the system arrow
        // shows over the title bar / outside the window — refocus reapplies it.
        let focused = STATE.with(|s| s.borrow().focused);
        if focused {
            w.set_cursor(kind.to_winit());
        }
    }
}

pub fn set_visible(window: Option<&WinitWindow>, visible: bool) {
    STATE.with(|s| s.borrow_mut().desired_visible = visible);
    if let Some(w) = window {
        let focused = STATE.with(|s| s.borrow().focused);
        if focused {
            w.set_cursor_visible(visible);
        }
    }
}

pub fn set_locked(window: Option<&WinitWindow>, locked: bool) {
    STATE.with(|s| s.borrow_mut().desired_locked = locked);
    // Snap the cursor to the center on lock so the first reported delta is 0
    // instead of jumping by however far it was from the center.
    if let Some(w) = window {
        if locked {
            recenter_cursor(w);
        }
    }
}

fn recenter_cursor(window: &WinitWindow) {
    let size = window.inner_size();
    let cx = (size.width / 2) as f64;
    let cy = (size.height / 2) as f64;
    let _ = window.set_cursor_position(winit::dpi::PhysicalPosition::new(cx, cy));
    STATE.with(|s| {
        let mut st = s.borrow_mut();
        st.cursor_x = cx as f32;
        st.cursor_y = cy as f32;
        st.last_x = cx as f32;
        st.last_y = cy as f32;
    });
}

// ---------------------------------------------------------------------------
// Winit hooks — called from window/mod.rs's WindowEvent handler
// ---------------------------------------------------------------------------

pub fn on_cursor_moved(window: &WinitWindow, x: f32, y: f32) {
    let (dx, dy, locked) = STATE.with(|s| {
        let mut st = s.borrow_mut();
        let dx = x - st.last_x;
        let dy = y - st.last_y;
        st.cursor_x = x;
        st.cursor_y = y;
        st.last_x = x;
        st.last_y = y;
        (dx, dy, st.desired_locked)
    });

    if dx == 0.0 && dy == 0.0 {
        return;
    }

    STATE.with(|s| {
        s.borrow_mut().pending_moves.push(MoveEvent {
            x,
            y,
            dx,
            dy,
        });
    });

    if locked {
        // Re-center after we've recorded the delta so the next move event's
        // dx/dy is measured from the center, not from wherever the cursor
        // landed after we moved it.
        recenter_cursor(window);
    }
}

pub fn on_mouse_input(button: MouseButton, state: ElementState) {
    let name = match button {
        MouseButton::Left => "MouseButton1",
        MouseButton::Right => "MouseButton2",
        MouseButton::Middle => "MouseButton3",
        MouseButton::Back => "MouseButton4",
        MouseButton::Forward => "MouseButton5",
        // Other(u16) keeps all the unusual buttons (gaming mice, etc.). We map
        // them to MouseButton<N+5> so callers can still match by name.
        MouseButton::Other(n) => {
            let owned = format!("MouseButton{}", (n as u32) + 5);
            STATE.with(|s| {
                let mut st = s.borrow_mut();
                let pressed = matches!(state, ElementState::Pressed);
                if pressed {
                    st.mouse_down.insert(owned.clone());
                } else {
                    st.mouse_down.remove(&owned);
                }
                st.pending_mouse_input.push(MouseInputEvent {
                    button: owned,
                    pressed,
                });
            });
            return;
        }
    };
    let pressed = matches!(state, ElementState::Pressed);
    STATE.with(|s| {
        let mut st = s.borrow_mut();
        if pressed {
            st.mouse_down.insert(name.to_string());
        } else {
            st.mouse_down.remove(name);
        }
        st.pending_mouse_input.push(MouseInputEvent {
            button: name.to_string(),
            pressed,
        });
    });
}

pub fn on_keyboard_input(physical: PhysicalKey, logical: &Key, state: ElementState, repeat: bool) {
    if repeat {
        // Suppress key-repeats so InputChanged fires once per physical press.
        return;
    }
    let name = key_name(logical, physical);
    let id = fnv1a_32(&name);
    let pressed = matches!(state, ElementState::Pressed);
    STATE.with(|s| {
        let mut st = s.borrow_mut();
        if pressed {
            st.keys_down.insert(name.clone(), id);
        } else {
            st.keys_down.remove(&name);
        }
        st.pending_key_input.push(KeyInputEvent { id, name, pressed });
    });
}

pub fn on_focus(window: &WinitWindow, focused: bool) {
    STATE.with(|s| s.borrow_mut().focused = focused);
    if focused {
        // Re-apply user prefs that the OS may have reset while we were away.
        let (cursor, visible, locked) = STATE.with(|s| {
            let st = s.borrow();
            (st.desired_cursor, st.desired_visible, st.desired_locked)
        });
        window.set_cursor(cursor.to_winit());
        window.set_cursor_visible(visible);
        if locked {
            recenter_cursor(window);
        }
    } else {
        // Drop currently-held inputs so a focus-loss stuck key doesn't linger.
        STATE.with(|s| {
            let mut st = s.borrow_mut();
            st.mouse_down.clear();
            st.keys_down.clear();
        });
        // Restore the system default cursor while unfocused. set_cursor_visible
        // matters because some OSs keep the hidden state across focus changes.
        window.set_cursor(CursorIcon::Default);
        window.set_cursor_visible(true);
    }
}

// Called every tick by the heart loop. Drains the queued events and fires
// the corresponding signals on the Lua thread.
pub fn pump(lua: &Lua) {
    let (moves, m_inputs, k_inputs) = STATE.with(|s| {
        let mut st = s.borrow_mut();
        (
            std::mem::take(&mut st.pending_moves),
            std::mem::take(&mut st.pending_mouse_input),
            std::mem::take(&mut st.pending_key_input),
        )
    });

    if !moves.is_empty() {
        if let Ok(sig) = lua.named_registry_value::<Table>(MOUSE_MOVED_KEY) {
            for m in moves {
                let mut args = MultiValue::new();
                let pos = match lua.create_userdata(Dim::new(m.x, m.y)) {
                    Ok(ud) => Value::UserData(ud),
                    Err(e) => {
                        eprintln!("[Mouse] create_userdata: {e}");
                        continue;
                    }
                };
                let delta = match lua.create_userdata(Dim::new(m.dx, m.dy)) {
                    Ok(ud) => Value::UserData(ud),
                    Err(e) => {
                        eprintln!("[Mouse] create_userdata: {e}");
                        continue;
                    }
                };
                args.push_back(pos);
                args.push_back(delta);
                if let Err(e) = signal::fire(lua, &sig, args) {
                    eprintln!("[Mouse] Moved fire: {e}");
                }
            }
        }
    }

    if !m_inputs.is_empty() {
        if let Ok(sig) = lua.named_registry_value::<Table>(MOUSE_INPUT_KEY) {
            for ev in m_inputs {
                let mut args = MultiValue::new();
                args.push_back(Value::String(match lua.create_string(&ev.button) {
                    Ok(s) => s,
                    Err(e) => {
                        eprintln!("[Mouse] create_string: {e}");
                        continue;
                    }
                }));
                args.push_back(Value::Boolean(ev.pressed));
                if let Err(e) = signal::fire(lua, &sig, args) {
                    eprintln!("[Mouse] InputReceived fire: {e}");
                }
            }
        }
    }

    if !k_inputs.is_empty() {
        if let Ok(sig) = lua.named_registry_value::<Table>(KEYBOARD_INPUT_KEY) {
            for ev in k_inputs {
                let mut args = MultiValue::new();
                args.push_back(Value::Number(ev.id as f64));
                args.push_back(Value::String(match lua.create_string(&ev.name) {
                    Ok(s) => s,
                    Err(e) => {
                        eprintln!("[Keyboard] create_string: {e}");
                        continue;
                    }
                }));
                args.push_back(Value::Boolean(ev.pressed));
                if let Err(e) = signal::fire(lua, &sig, args) {
                    eprintln!("[Keyboard] InputChanged fire: {e}");
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn key_name(logical: &Key, physical: PhysicalKey) -> String {
    match logical {
        Key::Named(named) => named_key_name(*named).to_string(),
        Key::Character(s) => s.to_string(),
        Key::Unidentified(_) | Key::Dead(_) => match physical {
            PhysicalKey::Code(code) => format!("{code:?}"),
            PhysicalKey::Unidentified(_) => "Unknown".into(),
        },
    }
}

fn named_key_name(named: NamedKey) -> &'static str {
    // Cover the keys most game code cares about; everything else falls
    // through to Debug below. Keep the names PascalCase for predictable
    // matching from Lua.
    match named {
        NamedKey::Enter => "Enter",
        NamedKey::Tab => "Tab",
        NamedKey::Space => "Space",
        NamedKey::Backspace => "Backspace",
        NamedKey::Escape => "Escape",
        NamedKey::Shift => "Shift",
        NamedKey::Control => "Control",
        NamedKey::Alt => "Alt",
        NamedKey::Super => "Super",
        NamedKey::CapsLock => "CapsLock",
        NamedKey::ArrowUp => "Up",
        NamedKey::ArrowDown => "Down",
        NamedKey::ArrowLeft => "Left",
        NamedKey::ArrowRight => "Right",
        NamedKey::Home => "Home",
        NamedKey::End => "End",
        NamedKey::PageUp => "PageUp",
        NamedKey::PageDown => "PageDown",
        NamedKey::Insert => "Insert",
        NamedKey::Delete => "Delete",
        NamedKey::F1 => "F1",
        NamedKey::F2 => "F2",
        NamedKey::F3 => "F3",
        NamedKey::F4 => "F4",
        NamedKey::F5 => "F5",
        NamedKey::F6 => "F6",
        NamedKey::F7 => "F7",
        NamedKey::F8 => "F8",
        NamedKey::F9 => "F9",
        NamedKey::F10 => "F10",
        NamedKey::F11 => "F11",
        NamedKey::F12 => "F12",
        _ => "Other",
    }
}

/// FNV-1a 32-bit. Stable across runs and platforms — exposed to Lua as the
/// "key id" so script code can match by either id or name interchangeably.
fn fnv1a_32(s: &str) -> u32 {
    const FNV_OFFSET: u32 = 0x811C9DC5;
    const FNV_PRIME: u32 = 16777619;
    let mut h = FNV_OFFSET;
    for b in s.as_bytes() {
        h ^= *b as u32;
        h = h.wrapping_mul(FNV_PRIME);
    }
    h
}

pub fn key_id_for_name(s: &str) -> u32 {
    fnv1a_32(s)
}
