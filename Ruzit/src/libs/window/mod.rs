use std::cell::RefCell;
use std::num::NonZeroU32;
use std::sync::Arc;
use std::time::Duration;

use mlua::{
    AnyUserData, Function, Lua, MultiValue, RegistryKey, Table, UserData, UserDataFields,
    UserDataMethods, Value,
};
use winit::application::ApplicationHandler;
use winit::dpi::{LogicalPosition, LogicalSize};
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::platform::pump_events::EventLoopExtPumpEvents;
use winit::window::{Fullscreen, Icon, Window as WinitWindow, WindowId, WindowLevel};

use crate::libs::asset::ImageAsset;
use crate::libs::signal;

const CHANGED_KEY: &str = "ruzit_window_changed";

#[derive(Debug)]
enum WindowChange {
    Resized { width: u32, height: u32 },
    Moved { x: i32, y: i32 },
    Focused(bool),
    ScaleFactor(f64),
}

thread_local! {
    static EVENT_LOOP: RefCell<Option<EventLoop<()>>> = const { RefCell::new(None) };
    static APP: RefCell<Option<WindowApp>> = const { RefCell::new(None) };
    static CLOSE_CB: RefCell<Option<RegistryKey>> = const { RefCell::new(None) };
}

pub fn create(lua: &Lua) -> mlua::Result<Table> {
    let t = lua.create_table()?;
    t.set("Open", lua.create_function(open)?)?;
    Ok(t)
}

pub fn is_open() -> bool {
    APP.with(|a| a.borrow().is_some())
}

pub fn pump(lua: &Lua) {
    // Pump winit events into our app, then snapshot what happened.
    let (close_now, pending) = EVENT_LOOP.with(|el_cell| {
        APP.with(|app_cell| {
            let mut el_ref = el_cell.borrow_mut();
            let mut app_ref = app_cell.borrow_mut();
            match (el_ref.as_mut(), app_ref.as_mut()) {
                (Some(el), Some(app)) => {
                    let _ = el.pump_app_events(Some(Duration::ZERO), app);
                    let drained = std::mem::take(&mut app.pending);
                    (app.close_requested, drained)
                }
                _ => (false, Vec::new()),
            }
        })
    });

    // Fire Changed events first (skip if we're already closing — handlers shouldn't run mid-shutdown).
    if !close_now && !pending.is_empty() {
        if let Ok(signal) = lua.named_registry_value::<Table>(CHANGED_KEY) {
            for change in pending {
                if let Err(e) = fire_change(lua, &signal, change) {
                    eprintln!("[Window] Changed fire error: {e}");
                }
            }
        }
    }

    if close_now {
        let cb_key = CLOSE_CB.with(|c| c.borrow_mut().take());
        if let Some(key) = cb_key {
            if let Ok(func) = lua.registry_value::<Function>(&key) {
                if let Err(e) = func.call::<()>(()) {
                    eprintln!("[Window] BindToClose error: {e}");
                }
            }
        }
        APP.with(|a| *a.borrow_mut() = None);
        EVENT_LOOP.with(|el| *el.borrow_mut() = None);
        std::process::exit(0);
    }
}

fn fire_change(lua: &Lua, signal: &Table, change: WindowChange) -> mlua::Result<()> {
    let mut args = MultiValue::new();
    match change {
        WindowChange::Resized { width, height } => {
            args.push_back(Value::String(lua.create_string("Resized")?));
            args.push_back(Value::Number(width as f64));
            args.push_back(Value::Number(height as f64));
        }
        WindowChange::Moved { x, y } => {
            args.push_back(Value::String(lua.create_string("Moved")?));
            args.push_back(Value::Number(x as f64));
            args.push_back(Value::Number(y as f64));
        }
        WindowChange::Focused(focused) => {
            args.push_back(Value::String(lua.create_string("Focused")?));
            args.push_back(Value::Boolean(focused));
        }
        WindowChange::ScaleFactor(scale) => {
            args.push_back(Value::String(lua.create_string("ScaleFactor")?));
            args.push_back(Value::Number(scale));
        }
    }
    signal::fire(lua, signal, args)
}

#[derive(Default)]
struct Opts {
    title: String,
    width: u32,
    height: u32,
    min_width: Option<u32>,
    min_height: Option<u32>,
    max_width: Option<u32>,
    max_height: Option<u32>,
    fullscreen: bool,
    borderless: bool,
    resizable: bool,
    maximized: bool,
    visible: bool,
    transparent: bool,
    always_on_top: bool,
    icon: Option<Icon>,
}

fn opt_or<T: mlua::FromLua>(t: Option<&Table>, key: &str, default: T) -> T {
    t.and_then(|tt| tt.get::<Option<T>>(key).ok().flatten())
        .unwrap_or(default)
}
fn opt_get<T: mlua::FromLua>(t: Option<&Table>, key: &str) -> Option<T> {
    t.and_then(|tt| tt.get::<Option<T>>(key).ok().flatten())
}

fn parse_opts(t: Option<&Table>) -> mlua::Result<Opts> {
    let mut opts = Opts {
        title: opt_or(t, "title", "Ruzit".to_string()),
        width: opt_or(t, "width", 800u32),
        height: opt_or(t, "height", 600u32),
        min_width: opt_get(t, "min_width"),
        min_height: opt_get(t, "min_height"),
        max_width: opt_get(t, "max_width"),
        max_height: opt_get(t, "max_height"),
        fullscreen: opt_or(t, "fullscreen", false),
        borderless: opt_or(t, "borderless", false),
        resizable: opt_or(t, "resizable", true),
        maximized: opt_or(t, "maximized", false),
        visible: opt_or(t, "visible", true),
        transparent: opt_or(t, "transparent", false),
        always_on_top: opt_or(t, "always_on_top", false),
        icon: None,
    };
    if let Some(icon_val) = t.and_then(|tt| tt.get::<Value>("icon").ok()) {
        if let Value::UserData(ud) = icon_val {
            opts.icon = build_icon_from_userdata(&ud);
        }
    }
    Ok(opts)
}

fn build_icon_from_userdata(ud: &AnyUserData) -> Option<Icon> {
    let img = ud.borrow::<ImageAsset>().ok()?;
    Icon::from_rgba(img.data.clone(), img.width, img.height).ok()
}

fn open(lua: &Lua, opts_arg: Option<Table>) -> mlua::Result<WindowHandle> {
    if APP.with(|a| a.borrow().is_some()) {
        return Err(mlua::Error::RuntimeError(
            "Window.Open: a window is already open".into(),
        ));
    }
    let opts = parse_opts(opts_arg.as_ref())?;

    let mut event_loop = EventLoop::new()
        .map_err(|e| mlua::Error::RuntimeError(format!("EventLoop::new: {e}")))?;
    let mut app = WindowApp::new(opts);
    // Drive once so resumed() runs and the window/surface come up before Open returns.
    let _ = event_loop.pump_app_events(Some(Duration::ZERO), &mut app);

    EVENT_LOOP.with(|c| *c.borrow_mut() = Some(event_loop));
    APP.with(|c| *c.borrow_mut() = Some(app));

    // Spin up a fresh Changed signal for this window and stash it where pump() can find it.
    let changed = signal::new_instance(lua)?;
    lua.set_named_registry_value(CHANGED_KEY, changed)?;

    Ok(WindowHandle)
}

struct WindowApp {
    opts: Opts,
    window: Option<Arc<WinitWindow>>,
    surface: Option<softbuffer::Surface<Arc<WinitWindow>, Arc<WinitWindow>>>,
    close_requested: bool,
    pending: Vec<WindowChange>,
}

impl WindowApp {
    fn new(opts: Opts) -> Self {
        Self {
            opts,
            window: None,
            surface: None,
            close_requested: false,
            pending: Vec::new(),
        }
    }

    fn paint_black(&mut self) {
        let (Some(window), Some(surface)) = (&self.window, self.surface.as_mut()) else {
            return;
        };
        let size = window.inner_size();
        let (Some(w), Some(h)) = (NonZeroU32::new(size.width), NonZeroU32::new(size.height))
        else {
            return;
        };
        if surface.resize(w, h).is_err() {
            return;
        }
        if let Ok(mut buf) = surface.buffer_mut() {
            for px in buf.iter_mut() {
                *px = 0;
            }
            let _ = buf.present();
        }
    }
}

impl ApplicationHandler for WindowApp {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }

        let mut attrs = WinitWindow::default_attributes()
            .with_title(&self.opts.title)
            .with_inner_size(LogicalSize::new(self.opts.width, self.opts.height))
            .with_resizable(self.opts.resizable)
            .with_decorations(!self.opts.borderless)
            .with_visible(self.opts.visible)
            .with_transparent(self.opts.transparent)
            .with_maximized(self.opts.maximized)
            .with_window_level(if self.opts.always_on_top {
                WindowLevel::AlwaysOnTop
            } else {
                WindowLevel::Normal
            });

        if let (Some(w), Some(h)) = (self.opts.min_width, self.opts.min_height) {
            attrs = attrs.with_min_inner_size(LogicalSize::new(w, h));
        }
        if let (Some(w), Some(h)) = (self.opts.max_width, self.opts.max_height) {
            attrs = attrs.with_max_inner_size(LogicalSize::new(w, h));
        }
        if self.opts.fullscreen {
            attrs = attrs.with_fullscreen(Some(Fullscreen::Borderless(None)));
        }
        if let Some(icon) = self.opts.icon.clone() {
            attrs = attrs.with_window_icon(Some(icon));
        }

        let window = match event_loop.create_window(attrs) {
            Ok(w) => Arc::new(w),
            Err(e) => {
                eprintln!("[Window] create failed: {e}");
                self.close_requested = true;
                return;
            }
        };
        let context = match softbuffer::Context::new(window.clone()) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("[Window] softbuffer context: {e}");
                self.close_requested = true;
                return;
            }
        };
        let surface = match softbuffer::Surface::new(&context, window.clone()) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("[Window] softbuffer surface: {e}");
                self.close_requested = true;
                return;
            }
        };

        self.window = Some(window);
        self.surface = Some(surface);
        self.paint_black();
    }

    fn window_event(
        &mut self,
        _event_loop: &ActiveEventLoop,
        _window_id: WindowId,
        event: WindowEvent,
    ) {
        match event {
            WindowEvent::CloseRequested => self.close_requested = true,
            WindowEvent::Resized(size) => {
                self.pending.push(WindowChange::Resized {
                    width: size.width,
                    height: size.height,
                });
                self.paint_black();
            }
            WindowEvent::RedrawRequested => self.paint_black(),
            WindowEvent::Moved(pos) => {
                self.pending.push(WindowChange::Moved {
                    x: pos.x,
                    y: pos.y,
                });
            }
            WindowEvent::Focused(focused) => {
                self.pending.push(WindowChange::Focused(focused));
            }
            WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                self.pending.push(WindowChange::ScaleFactor(scale_factor));
            }
            _ => {}
        }
    }
}

pub struct WindowHandle;

impl UserData for WindowHandle {
    fn add_fields<F: UserDataFields<Self>>(f: &mut F) {
        f.add_field_method_get("Changed", |lua, _| -> mlua::Result<Table> {
            lua.named_registry_value(CHANGED_KEY)
        });
    }

    fn add_methods<M: UserDataMethods<Self>>(m: &mut M) {
        m.add_method("Close", |_, _, _: ()| {
            APP.with(|a| {
                if let Some(app) = a.borrow_mut().as_mut() {
                    app.close_requested = true;
                }
            });
            Ok(())
        });

        m.add_method("BindToClose", |lua, _, func: Function| -> mlua::Result<()> {
            let key = lua.create_registry_value(func)?;
            CLOSE_CB.with(|c| *c.borrow_mut() = Some(key));
            Ok(())
        });

        m.add_method("Resize", |_, _, (w, h): (u32, u32)| {
            with_window(|win| {
                let _ = win.request_inner_size(LogicalSize::new(w, h));
            });
            Ok(())
        });
        m.add_method("SetTitle", |_, _, title: String| {
            with_window(|win| win.set_title(&title));
            Ok(())
        });
        m.add_method("SetBorderless", |_, _, b: bool| {
            with_window(|win| win.set_decorations(!b));
            Ok(())
        });
        m.add_method("SetFullscreen", |_, _, b: bool| {
            with_window(|win| {
                win.set_fullscreen(if b {
                    Some(Fullscreen::Borderless(None))
                } else {
                    None
                });
            });
            Ok(())
        });
        m.add_method("SetResizable", |_, _, b: bool| {
            with_window(|win| win.set_resizable(b));
            Ok(())
        });
        m.add_method("SetMaximized", |_, _, b: bool| {
            with_window(|win| win.set_maximized(b));
            Ok(())
        });
        m.add_method("SetMinimized", |_, _, b: bool| {
            with_window(|win| win.set_minimized(b));
            Ok(())
        });
        m.add_method("SetVisible", |_, _, b: bool| {
            with_window(|win| win.set_visible(b));
            Ok(())
        });
        m.add_method("SetAlwaysOnTop", |_, _, b: bool| {
            with_window(|win| {
                win.set_window_level(if b {
                    WindowLevel::AlwaysOnTop
                } else {
                    WindowLevel::Normal
                });
            });
            Ok(())
        });
        m.add_method("SetPosition", |_, _, (x, y): (i32, i32)| {
            with_window(|win| win.set_outer_position(LogicalPosition::new(x, y)));
            Ok(())
        });
        m.add_method("Focus", |_, _, _: ()| {
            with_window(|win| win.focus_window());
            Ok(())
        });
        m.add_method("RequestRedraw", |_, _, _: ()| {
            with_window(|win| win.request_redraw());
            Ok(())
        });

        m.add_method("Width", |_, _, _: ()| {
            Ok(with_window_get(|w| w.inner_size().width as i64).unwrap_or(0))
        });
        m.add_method("Height", |_, _, _: ()| {
            Ok(with_window_get(|w| w.inner_size().height as i64).unwrap_or(0))
        });
        m.add_method("Title", |_, _, _: ()| {
            Ok(with_window_get(|w| w.title()).unwrap_or_default())
        });
        m.add_method("IsFullscreen", |_, _, _: ()| {
            Ok(with_window_get(|w| w.fullscreen().is_some()).unwrap_or(false))
        });
        m.add_method("IsOpen", |_, _, _: ()| Ok(is_open()));
    }
}

fn with_window<F: FnOnce(&Arc<WinitWindow>)>(f: F) {
    APP.with(|a| {
        if let Some(app) = a.borrow().as_ref() {
            if let Some(window) = app.window.as_ref() {
                f(window);
            }
        }
    });
}

fn with_window_get<R, F: FnOnce(&Arc<WinitWindow>) -> R>(f: F) -> Option<R> {
    APP.with(|a| {
        a.borrow()
            .as_ref()
            .and_then(|app| app.window.as_ref().map(f))
    })
}
