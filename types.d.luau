-- =============================================================================
-- Ruzit Luau type declarations
-- =============================================================================
-- This file is consumed by luau-lsp to give you hover docs, autocomplete, and
-- type checking against every API a Ruzit script can touch. It is NOT loaded
-- by the runtime — the actual implementations live in Rust. Don't edit the
-- type shapes unless you also patch Ruzit/src/templates.rs (which holds the
-- bundled copy that `Ruzit Init` writes into new projects).
--
-- Top-level usage from any Luau script:
--   local Window = import("Window")     -- pulls a typed library handle
--   local IO     = import("IO")         -- ...
--   local _      = require(__dirname .. "/Helpers")
--
-- `import(name)` is the only way to access engine subsystems — there are no
-- Lua globals for any of them. Every name listed in `Imports` at the bottom
-- of this file is valid; anything else throws.
-- =============================================================================


-- =============================================================================
-- IO — file system access (read, write, list, mkdir, persistent handles).
-- Disk paths are resolved relative to the project root in `Ruzit Test` and
-- relative to the launcher exe in a built game; pass an absolute path if you
-- need to escape that root (e.g. user profile, mod folders).
-- =============================================================================

declare class IOHandle
	-- Read from an open file. `format` defaults to "*a" (read all). Other
	-- values: "*l" (one line, no newline), "*L" (one line WITH newline),
	-- "*n" (read a Lua number), or a number N (read up to N bytes).
	function read(self, format: string?): string
	-- Append `content` at the file's current position. Use IO.write for
	-- one-shot writes; only reach for an IOHandle if you need streaming.
	function write(self, content: string): ()
	-- Flush + release the OS handle. Calling this twice is a no-op.
	function close(self): ()
	-- The path that was passed to IO.open, normalized.
	function path(self): string
end

export type IO_API = {
	-- One-shot read: opens, reads everything, closes. Throws if the file
	-- can't be opened.
	read: (path: string) -> string,
	-- One-shot overwrite: creates the file if missing, replaces contents
	-- if it exists.
	write: (path: string, content: string) -> (),
	-- One-shot append: like `write` but adds to the end without truncating.
	append: (path: string, content: string) -> (),
	-- True if the path exists (file OR directory).
	exists: (path: string) -> boolean,
	-- Delete a file. No-op if the file doesn't exist; throws on permission /
	-- "is a directory" errors.
	remove: (path: string) -> (),
	-- Recursively create the directory chain. No-op if it already exists.
	mkdir: (path: string) -> (),
	-- List directory entries (names only, not full paths). Order is OS-defined.
	list: (path: string) -> { string },
	-- Open a streaming handle. Mode follows fopen conventions: "r" read,
	-- "w" truncate-write, "a" append, plus "+" variants for read+write.
	-- Default mode is "r".
	open: (path: string, mode: ("r" | "w" | "a" | "r+" | "w+" | "a+")?) -> IOHandle,
	-- Resolve a (possibly relative) path against Ruzit's current root and
	-- return the absolute form. Useful when you want to print or pass paths
	-- to non-IO APIs.
	getpath: (path: string) -> string,
}


-- =============================================================================
-- Net — TCP, UDP, IPC, WebSocket, and HTTP. Everything is blocking from the
-- script's perspective; for true async work pass jobs through Actor.
-- =============================================================================

declare class TcpConnection
	-- Push raw bytes onto the connection. Blocks until the OS accepts them.
	function Send(self, data: string): ()
	-- Read up to `n` bytes (default 4096). Returns "" on a clean close.
	function Receive(self, n: number?): string
	-- Half-close the socket. Subsequent Send/Receive throws.
	function Close(self): ()
	-- The remote peer's "host:port" string.
	function Peer(self): string
end

declare class TcpListener
	-- Block until a client connects. Returns the new connection's handle.
	function Accept(self): TcpConnection
	-- Stop accepting new connections. Existing TcpConnections keep working.
	function Close(self): ()
	-- The local "host:port" we're listening on (useful when you bound to
	-- port 0 and want to know what the OS picked).
	function Address(self): string
end

declare class UdpHandle
	-- Datagram send. Returns the byte count actually delivered to the OS.
	function Send(self, addr: string, data: string): number
	-- Block for the next datagram. Returns (bytes, "host:port" of sender).
	function Receive(self, n: number?): (string, string)
	function Close(self): ()
	-- The local "host:port" we're bound to.
	function Address(self): string
end

declare class IpcConnection
	-- Local-only stream over a Windows named pipe / Unix domain socket.
	-- Same semantics as TcpConnection but inside the same machine.
	function Send(self, data: string): ()
	function Receive(self, n: number?): string
	function Close(self): ()
end

declare class IpcListener
	function Accept(self): IpcConnection
	function Close(self): ()
end

declare class WebSocketConn
	-- Frame-oriented send/receive. Each Send is one WebSocket message;
	-- Receive returns one full message (handles fragmentation for you).
	function Send(self, msg: string): ()
	function Receive(self): string
	function Close(self): ()
end

declare class WebSocketListener
	function Accept(self): WebSocketConn
	function Close(self): ()
end

-- Incoming HTTP request fields, passed to your Net.Serve handler.
export type HttpRequest = { method: string, path: string, headers: { [string]: string }, body: string }
-- Outgoing HTTP response. Return one of these from your Net.Serve handler,
-- or return a plain string and it'll be wrapped as "200 OK text/plain".
export type HttpResponse = { status: number, headers: { [string]: string }, body: string }

export type Net_API = {
	-- Run a blocking HTTP server on `addr` (e.g. "127.0.0.1:8080"). Calls
	-- `handler` for each request — return either an HttpResponse table or
	-- a plain string. Single-threaded by default; if you need concurrency,
	-- spawn handlers through Actor.
	Serve: (addr: string, handler: (HttpRequest) -> (HttpResponse | string)) -> (),
	-- Synchronous HTTP client. method = "GET" / "POST" / etc. Headers and
	-- body are optional. Throws on connection failure; non-2xx statuses
	-- come back through .status, not as exceptions.
	Request: (method: string, url: string, body: string?, headers: { [string]: string }?) -> HttpResponse,
	-- TCP primitives. Connect dials, Host binds + listens.
	TCP: { Connect: (addr: string) -> TcpConnection, Host: (addr: string) -> TcpListener },
	-- UDP — Bind opens a socket on the given local "host:port".
	UDP: { Bind: (addr: string) -> UdpHandle },
	-- IPC — same-machine stream. Pass a pipe/socket NAME, not a path; the
	-- OS-specific prefix is added for you.
	IPC: { Connect: (name: string) -> IpcConnection, Host: (name: string) -> IpcListener },
	-- WebSocket. Pass a `ws://` or `wss://` URL to Connect; pass an
	-- "addr:port" to Host (TLS isn't auto-enabled on Host — terminate it
	-- in front of Ruzit).
	Socket: { Connect: (url: string) -> WebSocketConn, Host: (addr: string) -> WebSocketListener },
}


-- =============================================================================
-- Serde — Encoding (JSON/TOML/YAML), hashing, and compression.
-- =============================================================================

export type HashAlgo = "md5" | "sha1" | "sha224" | "sha256" | "sha384" | "sha512" | "sha3-256" | "sha3-512" | "keccak256" | "blake3"
export type CompressAlgo = "gzip" | "zlib" | "deflate" | "zstd"

export type Serde_API = {
	-- Serialize Lua data to a textual format. `pretty` adds whitespace
	-- (default false). Tables with sequential 1-based integer keys become
	-- arrays; otherwise they become objects with stringified keys.
	Encode: (format: ("json" | "toml" | "yaml"), data: any, pretty: boolean?) -> string,
	-- Inverse of Encode. Returns nested tables / numbers / strings / bools /
	-- nils. Throws on malformed input.
	Decode: (format: ("json" | "toml" | "yaml"), text: string) -> any,
	-- Cryptographic hash. `encoding` controls the return form: "hex"
	-- (default), "base64", or "bytes" (raw binary string).
	Hash: (algo: HashAlgo, data: string, encoding: ("hex" | "base64" | "bytes")?) -> string,
	-- Compress a string. `level` is 0..9 for gzip/zlib/deflate, 1..22 for
	-- zstd. Default is each codec's mid-level. Output is a binary string.
	Compress: (algo: CompressAlgo, data: string, level: number?) -> string,
	-- Inverse of Compress. Throws on truncated / bad-magic data.
	Decompress: (algo: CompressAlgo, data: string) -> string,
}


-- =============================================================================
-- Process — info about and control over the running process.
-- =============================================================================

export type Process_API = {
	-- "windows" / "linux" / "macos" / "freebsd" / etc. Match against this
	-- to gate platform-specific code paths.
	Os: string,
	-- "x86_64" / "aarch64" / etc.
	Arch: string,
	-- "windows" / "unix" / "wasm".
	Family: string,
	-- The OS-assigned process id.
	Pid: number,
	-- Logical CPU count (hyperthreads included). Useful for sizing Actor
	-- pools or work batches.
	CpuCount: number,
	-- True when running from a built game launcher (.exe + Managed/), false
	-- under `Ruzit Test`. Gate dev-only debug overlays etc. behind this.
	IsBuilt: boolean,
	-- Read environment variables. Two forms:
	--   Process.Env("PATH") → "..."  (or nil if unset)
	--   Process.Env()       → { ["PATH"] = "...", ... }  (whole map)
	Env: ((name: string) -> string?) & (() -> { [string]: string }),
	-- Set an env var on the running process (and inherited children).
	SetEnv: (name: string, value: string) -> (),
	-- Hard-exit the process. `code` defaults to 0. Doesn't run BindToClose
	-- callbacks — use Window:Close for graceful shutdown when you have
	-- a window open.
	Close: (code: number?) -> never,
	-- The argv passed to Ruzit (or the launcher), with element [1] being
	-- the executable name.
	Args: () -> { string },
	-- A snapshot of system memory in bytes. `available` is what the OS is
	-- willing to give a new allocation; `free` is the strict "currently
	-- unallocated" number (typically much smaller).
	Memory: () -> { total: number, free: number, used: number, available: number, total_swap: number, used_swap: number },
	-- Subscribe a function to the engine heart-tick. `id` is your own
	-- string label — use it later with UnbindFromHeart, and also as the
	-- de-dup key (registering the same id twice replaces the old fn).
	-- The callback gets dt (seconds since the previous tick). For most
	-- per-frame logic prefer RunService.Heartbeat — BindToHeart is for
	-- background tasks that need to run even when no window is open.
	BindToHeart: (id: string, fn: (dt: number) -> ()) -> (),
	UnbindFromHeart: (id: string) -> (),
}


-- =============================================================================
-- Asset — typed loaders for images, sounds, shaders, fonts, models, files.
-- =============================================================================

declare class ImageAsset
	-- Pixel width.
	function Width(self): number
	-- Pixel height.
	function Height(self): number
	-- Origin string (file path or "<pixels:WxH>" / "<string:Image>" for
	-- in-memory variants). Useful for logging.
	function Source(self): string
	-- Raw RGBA8 pixel buffer (Width * Height * 4 bytes). Access only when
	-- you need to inspect or mutate pixels — most uses just hand the asset
	-- to a GUI / Renderable.
	function Pixels(self): string
end

declare class SoundData
	function Source(self): string
	-- Encoded byte length (NOT decoded sample count). Decoding happens on
	-- :Play() in SFX.LoadSound.
	function ByteCount(self): number
end


-- =============================================================================
-- Shaders
-- =============================================================================
-- Ruzit shaders are written in WGSL — wgpu's native shader language.
-- (If you've used GLSL/HLSL, WGSL reads similarly: typed structs, explicit
-- uniform buffers, attribute-declared entry points.) The runtime is wgpu 22
-- on top of DX12 (Windows) / Vulkan / Metal / GL.
--
-- There are two stages, each loaded as its own asset kind:
--   * ShaderAsset    — VERTEX stage (.shader / .glsl / .wgsl / .hlsl /
--                      .vert / .metal extensions). Most projects never
--                      override this; the engine's default vertex stage
--                      handles transform / projection / interpolation.
--   * FragmentAsset  — FRAGMENT (pixel) stage (.frag / .fragment / .fs /
--                      .glslf extensions). This is where ~all custom shader
--                      work lives — surface lighting, dissolve effects,
--                      stylized lighting, scrolling textures, post effects.
--
-- Where shaders go (the four hook points):
--   * BasePart:AttachShader(asset)        — runs once per pixel of a 3D part.
--   * Primitive:AttachShader(asset)       — runs per pixel of a 2D GUI shape.
--   * Sound:AttachShader(asset)           — DSP "shader" for audio (separate
--                                           contract — see Sound docs).
--   * GUI.SetSkybox(asset) / .SetPostEffect(asset) — fullscreen scene passes.
--
-- Shaders written for one hook can't be reused on another; the available
-- bindings differ. Each section below documents the contract.
--
-- Custom uniforms (`// @ruzit param`):
--   Declare named float parameters in your shader source via single-line
--   comments. Up to 16 floats per shader.
--
--     // @ruzit param speed
--     // @ruzit param amplitude
--     // @ruzit param tint_g
--
--   Then read them in WGSL via the `p(idx)` helper (slot order = decl order):
--
--     let speed     = p(0u);
--     let amplitude = p(1u);
--     let tint_g    = p(2u);
--
--   Set / read from Lua:
--     part:SetData(shader, "speed", 1.5)
--     local v = part:GetData(shader, "speed")
--
--   Setting a name that isn't declared throws — typo guard.
--
-- ──────────────────────────────────────────────────────────────────────────
-- 2D primitive fragment contract
--   (FragmentAsset attached to Primitive, or fed to SetSkybox / SetPostEffect)
--
-- Available WGSL globals (the engine prepends these as a prelude — DON'T
-- redeclare them in your shader):
--
--   struct VsOut {
--       @builtin(position) clip: vec4<f32>,
--       @location(0) uv: vec2<f32>,    // 0..1, top-left origin
--   };
--   struct RuzitUni {
--       pos: vec2<f32>,                 // primitive top-left in pixels
--       size: vec2<f32>,                // primitive width/height
--       color: vec4<f32>,               // RGBA tint (alpha = 1 - Transparency)
--       resolution: vec2<f32>,          // window inner size in pixels
--       time: f32,                      // seconds since window opened
--       shape: u32,                     // 0=Square 1=Circle 2=Triangle 3=Image
--       params: array<vec4<f32>, 4>,    // your @ruzit params, packed
--   };
--   @group(0) @binding(0) var<uniform> U: RuzitUni;
--   @group(0) @binding(1) var IMG: texture_2d<f32>;        // bound asset, or
--                                                          // 1×1 white if none
--   @group(0) @binding(2) var IMG_SAMP: sampler;
--   fn p(idx: u32) -> f32                  // @ruzit param accessor
--   fn ruzit_inside_shape(uv, shape): bool // mask for the primitive's geometry
--   fn ruzit_apply_alpha(rgb): vec4<f32>   // wrap rgb in U.color.a
--
-- Required entry point:
--   @fragment
--   fn fs_main(in: VsOut) -> @location(0) vec4<f32> { ... }
--
-- Minimum useful shader (replicates the default — solid tint, shape-masked):
--   @fragment
--   fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
--       if (!ruzit_inside_shape(in.uv, U.shape)) { discard; }
--       let tex = textureSample(IMG, IMG_SAMP, in.uv);
--       return tex * U.color;
--   }
--
-- ──────────────────────────────────────────────────────────────────────────
-- 3D part fragment contract
--   (FragmentAsset attached to BasePart)
--
-- Same idea but with view/world data instead of window/screen data:
--
--   struct VsOut {
--       @builtin(position) clip: vec4<f32>,
--       @location(0) world_pos: vec3<f32>,
--       @location(1) world_normal: vec3<f32>,    // already normalized
--       @location(2) uv: vec2<f32>,
--   };
--   struct Frame {
--       view_proj: mat4x4<f32>,
--       light_dir: vec3<f32>,                    // engine sun direction
--       time: f32,
--       camera_pos: vec3<f32>,
--       _pad: f32,
--   };
--   struct Instance {
--       model: mat4x4<f32>,
--       color: vec4<f32>,                        // BasePart.Color
--       params: array<vec4<f32>, 4>,
--   };
--   @group(0) @binding(0) var<uniform> F: Frame;
--   @group(0) @binding(1) var<uniform> I: Instance;
--   @group(0) @binding(2) var IMG: texture_2d<f32>;     // BasePart.Texture
--   @group(0) @binding(3) var IMG_SAMP: sampler;
--   fn p(idx: u32) -> f32
--
-- Required entry point:
--   @fragment
--   fn fs_main(in: VsOut) -> @location(0) vec4<f32> { ... }
--
-- Minimum useful shader (replicates the default half-Lambert lighting):
--   @fragment
--   fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
--       let n = normalize(in.world_normal);
--       let nl = dot(n, -F.light_dir);
--       let lit = 0.25 + 0.75 * (nl * 0.5 + 0.5);   // half-Lambert + ambient
--       let tex = textureSample(IMG, IMG_SAMP, in.uv);
--       return vec4<f32>(I.color.rgb * tex.rgb * lit, I.color.a);
--   }
--
-- =============================================================================

declare class ShaderAsset end
declare class FragmentAsset end


declare class FontAsset
	function Source(self): string
end

export type Asset_API = {
	-- Load a typed asset from your project's assets/ folder (when running
	-- `Ruzit Test`) or from a bundled .managed package (in a built game).
	--
	-- kind picks both the loader and the return type. Each kind accepts a
	-- fixed set of file extensions, so you usually pass the path WITHOUT one:
	--   Image    .png .jpg .jpeg .bmp .gif .webp
	--   Sound    .ogg .mp3 .wav .flac
	--   Shader   .shader .glsl .wgsl .hlsl .vert .metal
	--   Fragment .frag .fragment .fs .glslf
	--   Model    .obj .fbx
	--   Font     .ttf .otf
	--   File     any file — returned as its raw UTF-8 string contents
	--
	-- path forms:
	--   "foo.bar.baz"     — same package as the caller. Dots are folder
	--                       separators, so this maps to foo/bar/baz under the
	--                       package's assets/ root.
	--   "@PkgId/foo/bar"  — explicit cross-package lookup. Use this to read an
	--                       asset out of another loaded .managed package by id.
	--   "foo/bar.png"     — a literal extension also works; the loader uses it
	--                       directly instead of probing the kind's ext list.
	--
	-- examples:
	--   Asset.GetAsset("Image", "ui.logo")              -- assets/ui/logo.png
	--   Asset.GetAsset("Model", "props.crate")          -- assets/props/crate.fbx (or .obj)
	--   Asset.GetAsset("Sound", "@SfxPack/explode")     -- from a sibling package
	--   Asset.GetAsset("File",  "data/levels.json")     -- raw text contents
	GetAsset: ((kind: "Image", path: string) -> ImageAsset)
		& ((kind: "Sound", path: string) -> SoundData)
		& ((kind: "Shader", path: string) -> ShaderAsset)
		& ((kind: "Fragment", path: string) -> FragmentAsset)
		& ((kind: "Model", path: string) -> ModelAsset)
		& ((kind: "Font", path: string) -> FontAsset)
		& ((kind: "File", path: string) -> string),
	-- Build an asset from a raw byte string in memory, no filesystem touch.
	-- `label` is shown by Source(); it has no functional effect. Useful for
	-- assets you fetched over the network or assembled procedurally.
	FromString: ((kind: "Image", data: string, label: string?) -> ImageAsset)
		& ((kind: "Sound", data: string, label: string?) -> SoundData)
		& ((kind: "Shader", data: string, label: string?) -> ShaderAsset)
		& ((kind: "Fragment", data: string, label: string?) -> FragmentAsset)
		& ((kind: "Model", data: string, label: string?) -> ModelAsset)
		& ((kind: "Font", data: string, label: string?) -> FontAsset)
		& ((kind: "File", data: string, label: string?) -> string),
	-- Load an asset from any disk path (mod folders, Workshop items, user
	-- profile, etc.). Pass "Auto" or "" as the kind to infer it from the
	-- file extension. Useful for mods that drop loose files alongside the
	-- game and load them at runtime.
	ImportAsset: ((kind: "Image", path: string) -> ImageAsset)
		& ((kind: "Sound", path: string) -> SoundData)
		& ((kind: "Shader", path: string) -> ShaderAsset)
		& ((kind: "Fragment", path: string) -> FragmentAsset)
		& ((kind: "Model", path: string) -> ModelAsset)
		& ((kind: "Font", path: string) -> FontAsset)
		& ((kind: "File", path: string) -> string)
		& ((kind: "Auto", path: string) -> any),
	-- Build an ImageAsset from raw RGBA8 pixels. `rgba` must be exactly
	-- width * height * 4 bytes. Useful for procedural textures or pixel
	-- buffers handed back from another system (Steam avatars, etc.).
	FromPixels: (width: number, height: number, rgba: string) -> ImageAsset,
}


-- =============================================================================
-- Window — open a window, listen for size/focus changes, toggle modes.
-- =============================================================================

-- Options bag passed to Window.Open. All fields are optional with sensible
-- defaults (800x600 windowed, decorated, resizable, visible).
export type WindowOptions = {
	-- Caption shown in the title bar / taskbar.
	title: string?,
	-- Initial inner-area size in logical pixels (DPI-scaled by the OS).
	width: number?, height: number?,
	-- Inner-size constraints. Optional pairs — set both width and height
	-- of a constraint or neither.
	min_width: number?, min_height: number?, max_width: number?, max_height: number?,
	-- Exclusive fullscreen. Covers the whole monitor and changes display
	-- mode. Most games should prefer `borderless = true` instead.
	fullscreen: boolean?,
	-- Borderless fullscreen — strips decorations AND covers the OS taskbar
	-- via Fullscreen::Borderless. The window stays at normal z-level so
	-- Alt+Tab works cleanly and Steam/Discord overlays render on top.
	borderless: boolean?,
	-- Allow user-resizing via window edges. Default true.
	resizable: boolean?,
	-- Start maximized.
	maximized: boolean?,
	-- Start visible. Set to false if you need to do work before showing it
	-- (e.g. preload assets), then call Window:SetVisible(true).
	visible: boolean?,
	-- Allow per-pixel alpha. Requires a compositor (Windows / macOS / most
	-- Linux WMs).
	transparent: boolean?,
	-- Pin above other windows. Ignored when `fullscreen` or `borderless`
	-- is on (those force normal z-level).
	always_on_top: boolean?,
	-- ImageAsset used for the title-bar / taskbar icon. PNG works best.
	icon: ImageAsset?,
}

declare class Connection
	-- True until :Disconnect() is called (or the signal is GC'd).
	Connected: boolean
	-- Stop receiving fires from the signal. Idempotent.
	function Disconnect(self): ()
end

-- Variadic generics on `declare class` aren't supported by the stable
-- luau-lsp parser yet, but generic type aliases ARE — so Signal lives as
-- a parameterized table type instead. Same call shape (signal:Connect(...))
-- with full callback-arg type narrowing: `Signal<number, string>` makes
-- `signal:Connect(function(a, b) ... end)` infer `a:number, b:string`.
export type Signal<T...> = {
	-- Subscribe a callback. Returns a Connection — keep it if you ever
	-- need to disconnect; otherwise let it live as long as the signal does.
	Connect: (self: Signal<T...>, fn: (T...) -> ()) -> Connection,
	-- Like Connect, but fires at most once and auto-disconnects.
	Once: (self: Signal<T...>, fn: (T...) -> ()) -> Connection,
	-- Block the current coroutine until the next fire, then return its args.
	Wait: (self: Signal<T...>) -> T...,
	-- Manually fire the signal. Most engine-provided signals are one-way
	-- (they fire on you, not from you), but Signal_API.new gives you ones
	-- you can fire yourself.
	Fire: (self: Signal<T...>, T...) -> (),
	-- Drop every connection at once.
	DisconnectAll: (self: Signal<T...>) -> (),
}

-- Factory for user-defined signals.
--   local Signal = import("Signal")
--   local s = Signal.new()           -- Signal<>
--   local hit = Signal.new()         -- Signal<...>; first :Fire pins T...
export type Signal_API = { new: <T...>() -> Signal<T...> }

declare class WindowHandle
	-- Property-change signal. Fires with the property name that changed:
	-- "Title", "Size", "Position", "Fullscreen", "Maximized", etc. Use it
	-- when you want one handler for many possible changes.
	Changed: Signal<string>
	-- Fires when the window gains keyboard / mouse focus.
	OnFocus: Signal<>
	-- Fires when the window loses focus. Useful for releasing mouse lock,
	-- pausing the game, etc.
	OnUnfocus: Signal<>
	-- Programmatically close the window. Triggers BindToClose, then
	-- terminates the process (the heart loop unwinds first).
	function Close(self): ()
	-- Run `fn` exactly once just before the process exits because the
	-- window was closed. Use it to flush save state, persist settings, etc.
	function BindToClose(self, fn: () -> ()): ()
	-- Resize the inner area in logical pixels. Honors min/max constraints.
	function Resize(self, width: number, height: number): ()
	-- Change the title-bar caption.
	function SetTitle(self, title: string): ()
	-- Toggles "borderless fullscreen" — strips decorations AND enters
	-- Fullscreen::Borderless so the window covers the OS taskbar/dock.
	-- The Open-time `borderless = true` option does the same thing.
	function SetBorderless(self, borderless: boolean): ()
	-- Strips title bar / minimize / close buttons WITHOUT going fullscreen.
	-- Use this for decorationless tool overlays or floating panels.
	function SetDecorations(self, decorated: boolean): ()
	-- Enter / exit borderless-fullscreen mode at runtime.
	function SetFullscreen(self, fullscreen: boolean): ()
	-- Allow / disallow user resize via edges.
	function SetResizable(self, resizable: boolean): ()
	-- Maximize or restore.
	function SetMaximized(self, maximized: boolean): ()
	-- Minimize to the taskbar / dock.
	function SetMinimized(self, minimized: boolean): ()
	-- Show / hide the window without closing it.
	function SetVisible(self, visible: boolean): ()
	-- Pin above other windows. Note that borderless mode forces this off.
	function SetAlwaysOnTop(self, on_top: boolean): ()
	-- Move the window. (x, y) = top-left in screen coords.
	function SetPosition(self, x: number, y: number): ()
	-- Bring the window to the front and give it keyboard focus.
	function Focus(self): ()
	-- Ask the OS to redraw the window on the next frame. You rarely need
	-- this — the engine repaints automatically when the GUI/Renderable
	-- scene changes.
	function RequestRedraw(self): ()
	-- Current inner-area dimensions in logical pixels.
	function Width(self): number
	function Height(self): number
	-- Current title.
	function Title(self): string
	-- True while in borderless or exclusive fullscreen.
	function IsFullscreen(self): boolean
	-- True until Close has been called and the runtime has unwound.
	function IsOpen(self): boolean
	-- The active 3D viewport's camera. The same Camera object that
	-- Renderable.Camera returns — exposed here so a Lua module that only
	-- has a WindowHandle can still drive the view.
	function GetViewport(self): Camera
end

export type Window_API = {
	-- Open the application's window. Only one is allowed at a time;
	-- calling twice throws. Returns a WindowHandle for runtime control.
	Open: (opts: WindowOptions?) -> WindowHandle,
}


-- =============================================================================
-- SFX — stream and shape audio. Built on rodio + cpal.
-- =============================================================================

-- Built shader instance produced by SFX.Volume / SFX.Echo / etc. Stack via
-- Sound:ApplyShader for compound effects.
declare class SoundShader end

declare class Sound
	-- Fires when :Play() actually starts feeding audio.
	Started: Signal<>
	-- Fires when playback ends (natural completion or :Stop).
	Stopped: Signal<>
	-- Source label inherited from the SoundData.
	Source: string
	-- Begin playback. Re-applying shaders or :Volume / :Speed first changes
	-- how this playback sounds — settings don't carry between :Play calls
	-- unless you re-apply them.
	function Play(self): ()
	-- Halt the currently-playing instance. Fires Stopped.
	function Stop(self): ()
	function IsPlaying(self): boolean
	-- Fluent built-in shaders: each one replaces any prior call to the same
	-- method on this Sound (so Volume(0.5) → Volume(0.8) ends at 0.8, no
	-- stacking). Call before :Play() — they apply on the next playback.
	-- Linear gain. 1.0 = unchanged, 0.0 = silence, >1 = boosted.
	function Volume(self, factor: number): ()
	-- Time scaling. 1.0 = real-time, 2.0 = 2x faster (also pitches up an octave).
	function Speed(self, factor: number): ()
	-- Pure pitch shift via resampling. Same audible effect as Speed but
	-- sample-rate-aware so the timeline length doesn't change.
	function Pitch(self, factor: number): ()
	-- Stereo pan. -1 = left, 0 = center, +1 = right.
	function Pan(self, amount: number): ()
	-- Lowpass / highpass biquad. `freq` is the cutoff in Hz.
	function LowPass(self, freq: number): ()
	function HighPass(self, freq: number): ()
	-- Linear envelope. Volume ramps from 0→1 over `seconds` from playback start.
	function FadeIn(self, seconds: number): ()
	-- Volume ramps from 1→0 over the trailing `seconds` of playback.
	function FadeOut(self, seconds: number): ()
	-- Wait `seconds` before producing any audio. Useful for staggered SFX.
	function Delay(self, seconds: number): ()
	-- Mark this sound as looping. Each loop fires Started/Stopped fresh.
	function Loop(self): ()
	-- Soft saturation. `amount` in roughly 0..1; values >1 hard-clip.
	function Distortion(self, amount: number): ()
	-- Single-tap echo. `delay_ms` is one tap, `feedback` is the per-tap
	-- gain decay (0..0.95 typical), `mix` is the wet/dry blend (0..1).
	function Echo(self, delay_ms: number, feedback: number?, mix: number?): ()
	-- Diffuse reverb. `mix` 0..1 wet/dry, `decay` is the RT60-ish tail.
	function Reverb(self, mix: number?, decay: number?): ()
	-- Periodic amplitude wobble. `rate` Hz, `depth` 0..1.
	function Tremolo(self, rate: number?, depth: number?): ()
	-- Drop ALL fluent shaders and (custom) attachments — back to a clean state.
	function Reset(self): ()
	-- 3D world position. Listener = the active Renderable.Camera (the
	-- viewport). Distance falloff defaults to 20 world units; pass an
	-- explicit value to tighten or widen. Call ClearPosition() to revert
	-- to non-positional (centered) playback.
	function SetPosition(self, x: number, y: number, z: number, falloff: number?): ()
	function ClearPosition(self): ()
	-- Legacy / advanced: build shader instances yourself.
	function ApplyShader(self, shader: SoundShader): ()
	function ClearShaders(self): ()
	-- Custom shader assets — write your own DSP in WGSL/GLSL-like text and
	-- attach it. SetData / GetData expose `uniform`-style numeric inputs.
	function AttachShader(self, asset: ShaderAsset | FragmentAsset): ()
	function DetachShader(self, asset: ShaderAsset | FragmentAsset): ()
	function SetData(self, asset: ShaderAsset | FragmentAsset, name: string, value: number): ()
	function GetData(self, asset: ShaderAsset | FragmentAsset, name: string): number?
	-- Fires every `interval` seconds with the elapsed time since playback began.
	function LinkToUpdate(self, interval: number): Signal<number>
end

export type SFX_API = {
	-- Wrap a SoundData into a playable Sound. Same SoundData can back
	-- many independent Sounds — each has its own shader chain and
	-- Play/Stop state.
	LoadSound: (data: SoundData) -> Sound,
	-- Shader factories: same set as Sound's fluent methods, but as standalone
	-- userdata you can stack on any sound via :ApplyShader. Sometimes useful
	-- for building shader presets.
	Volume: (factor: number) -> SoundShader,
	Speed: (factor: number) -> SoundShader,
	Pan: (amount: number) -> SoundShader,
	FadeIn: (seconds: number) -> SoundShader,
	FadeOut: (seconds: number) -> SoundShader,
	LowPass: (freq: number) -> SoundShader,
	HighPass: (freq: number) -> SoundShader,
	Delay: (seconds: number) -> SoundShader,
	Repeat: () -> SoundShader,
	Distortion: (amount: number) -> SoundShader,
	Echo: (delay_ms: number, feedback: number?, mix: number?) -> SoundShader,
	Reverb: (mix: number?, decay: number?) -> SoundShader,
	Tremolo: (rate: number?, depth: number?) -> SoundShader,
}


-- =============================================================================
-- Managed — runtime info about loaded .managed packages (the project itself,
-- DLCs, and anything dropped into the Packages/ folder).
-- =============================================================================

declare class Package
	-- The package id (used in @PkgId/... cross-package references).
	ID: string
	-- Display name — set in build.toml's Name or the package's ManagedInfo.
	Name: string
	-- Semantic version string from build.toml / ManagedInfo.toml.
	Version: string
	-- Author/creator string, free-form.
	Creator: string
	-- The entry script's vfs key (typically "Main.luau" or "init.luau").
	Entry: string
	-- A vfs prefix you can pass to `require` to load this package's modules
	-- (e.g. "@OtherPkg/sub/module"). For the calling package, this is the
	-- empty string — use `require(__dirname .. "/...")` for self.
	Origin: string
	-- True if the script with this vfs key exists in the package.
	function HasFile(self, key: string): boolean
	-- True if the asset key exists in the package.
	function HasAsset(self, key: string): boolean
	-- List every script's vfs key. Useful for editors / debug overlays.
	function Files(self): { string }
	-- List every asset key.
	function Assets(self): { string }
end

export type Managed_API = {
	-- True if a package with this id is loaded.
	IsPackage: (id: string) -> boolean,
	-- Look up a package by id. Returns nil if unknown.
	GetPackage: (id: string) -> Package?,
	-- All loaded package ids — your own, every DLC, every Packages/ import.
	List: () -> { string },
	-- The default (entry-point) package. In a built game this is the
	-- launcher's primary package; under `Ruzit Test` it's the project at
	-- the working directory.
	Default: () -> Package?,
}


-- =============================================================================
-- Primitives — math types: Dim (2D), Color3, Vector (3D), CFrame (rigid pose).
-- All immutable: arithmetic returns new instances, never mutates in place.
-- =============================================================================

declare class Dim
	-- 2D component. .X is horizontal, .Y is vertical (screen-space-ish:
	-- +Y is down, matching pixel coords).
	X: number
	Y: number
	-- Linearly interpolate between self and other. t=0 → self, t=1 → other.
	function Lerp(self, other: Dim, t: number): Dim
	function __add(self, other: Dim): Dim
	function __sub(self, other: Dim): Dim
	-- Scalar multiply / divide / negate.
	function __mul(self, other: number): Dim
	function __div(self, other: number): Dim
	function __unm(self): Dim
end

declare class Color3
	-- Linear 0..1 RGB channels. The constructors clamp out-of-range inputs.
	-- For 8-bit (0..255) channels use Color3.FromRGB; for hex strings use
	-- Color3.FromHex.
	R: number
	G: number
	B: number
	-- Component-wise interpolation. t=0 returns self, t=1 returns other.
	function Lerp(self, other: Color3, t: number): Color3
	-- Channel-wise add / subtract. Result is clamped to 0..1.
	function __add(self, other: Color3): Color3
	function __sub(self, other: Color3): Color3
end

declare class Vector
	-- 3D component vector.
	X: number
	Y: number
	Z: number
	-- Cached euclidean length (sqrt(x^2 + y^2 + z^2)).
	Magnitude: number
	function Lerp(self, other: Vector, t: number): Vector
	function __add(self, other: Vector): Vector
	function __sub(self, other: Vector): Vector
	-- Scalar multiply / divide / negate.
	function __mul(self, other: number): Vector
	function __div(self, other: number): Vector
	function __unm(self): Vector
end

declare class ModelAsset
	-- Total per-vertex count after triangulation.
	function VertexCount(self): number
	-- Triangle count (== indices/3).
	function TriangleCount(self): number
	function Source(self): string
end

declare class CFrame
	-- World-space position.
	Position: Vector
	-- Euler-angle orientation in radians (YXZ-ish). For controlled rotations
	-- prefer CFrame.Angles(...).
	Rotation: Vector
	-- Linear interpolation. Position lerps; Rotation lerps component-wise.
	function Lerp(self, other: CFrame, t: number): CFrame
	-- CFrame * CFrame composes poses (right-applies). CFrame * Vector
	-- transforms a point from local to world space.
	function __mul(self, other: CFrame | Vector): CFrame
	function __add(self, other: CFrame): CFrame
	function __sub(self, other: CFrame): CFrame
end

export type Dim_API = {
	-- Dim.new(x, y) — typed 2D component.
	new: (x: number, y: number) -> Dim,
}
export type Color3_API = {
	-- Linear 0..1 channels. Out-of-range values are clamped.
	-- Example: Color3.new(0.18, 0.55, 0.28) = a forest green.
	new: (r: number, g: number, b: number) -> Color3,
	-- 8-bit (0..255) channels — convenient for colors copied from a paint
	-- program or design tool. Internally divided by 255 to the 0..1 range.
	-- Example: Color3.FromRGB(46, 140, 71).
	FromRGB: (r: number, g: number, b: number) -> Color3,
	-- camelCase alias for FromRGB.
	fromRGB: (r: number, g: number, b: number) -> Color3,
	-- Parse a CSS-style hex literal. Accepts "#RRGGBB", "RRGGBB",
	-- "#RGB" (each digit doubled), or "RGB". Throws on bad lengths /
	-- non-hex chars.
	-- Examples: Color3.FromHex("#2e8c47"), Color3.FromHex("ff0").
	FromHex: (hex: string) -> Color3,
	-- camelCase alias for FromHex.
	fromHex: (hex: string) -> Color3,
}
export type Vector_API = {
	-- Vector.new(x, y, z) — any axis defaults to 0.
	new: ((x: number?, y: number?, z: number?) -> Vector),
	-- (0, 0, 0) shorthand.
	zero: () -> Vector,
	-- (1, 1, 1) shorthand.
	one: () -> Vector,
}
export type CFrame_API = {
	-- CFrame.new(position?, rotation?). Either argument is optional.
	new: (position: Vector?, rotation: Vector?) -> CFrame,
	-- Build a rotation-only CFrame from Euler angles (radians, applied X→Y→Z).
	Angles: (rx: number, ry: number, rz: number) -> CFrame,
}

export type Primitives_API = {
	Dim: Dim_API,
	Color3: Color3_API,
	Vector: Vector_API,
	CFrame: CFrame_API,
}


-- =============================================================================
-- Renderable — 3D scene. Parts, models, the camera.
-- =============================================================================

declare class BasePart
	-- Geometry kind. "Cube" / "Sphere" use a built-in mesh; "Model" means
	-- the part was created via Renderable.BaseModel(asset).
	Shape: "Cube" | "Sphere" | "Model"
	-- World pose (position + rotation).
	CFrame: CFrame
	-- Axis-aligned object-space scale, in world units.
	Size: Vector
	-- Albedo when no Texture is set, tint multiplier when one is.
	Color: Color3
	-- Set false to keep the part in the scene graph but skip drawing it.
	-- Useful for trigger / collision-only volumes (when those land).
	Render: boolean
	-- Optional surface texture. Uses the part's Color as a tint.
	Texture: ImageAsset?
	-- Property-change signal. Fires with one of: "CFrame", "Size", "Color",
	-- "Render", "Texture", "Destroyed".
	Changed: Signal<string>
	-- Remove the part from the scene. Connections to Changed still receive
	-- one final "Destroyed" fire.
	function Destroy(self): ()
	-- Custom shader hooks — same shape as Sound's, but for the GPU stage
	-- that draws this part.
	function AttachShader(self, asset: ShaderAsset | FragmentAsset): ()
	function DetachShader(self, asset: ShaderAsset | FragmentAsset): ()
	function ClearShaders(self): ()
	function SetData(self, asset: ShaderAsset | FragmentAsset, name: string, value: number): ()
	function GetData(self, asset: ShaderAsset | FragmentAsset, name: string): number?
end

declare class Camera
	-- World pose. Position is the eye; Rotation aims the look vector.
	CFrame: CFrame
	-- Field of view in degrees (vertical).
	FOV: number
	-- Near clipping plane distance.
	Near: number
	-- Far clipping plane distance.
	Far: number
end

export type Renderable_API = {
	-- Spawn a primitive cube/sphere. Defaults to "Cube" if shape isn't given.
	-- Returns the part — keep the reference to mutate or destroy it later.
	BasePart: (shape: ("Cube" | "Sphere")?) -> BasePart,
	-- Spawn a part backed by an arbitrary mesh (.obj / .fbx ModelAsset).
	BaseModel: (asset: ModelAsset) -> BasePart,
	-- The active 3D camera. Same instance the window uses; mutate it to move
	-- the view. Also exposed as WindowHandle:GetViewport().
	Camera: Camera,
}


-- =============================================================================
-- GUI — 2D overlay primitives, post-effects, skybox.
-- =============================================================================

declare class Primitive
	-- Geometry / kind. The set is fixed at creation; switch by destroying
	-- and rebuilding via the appropriate GUI.Basic factory.
	Shape: "Square" | "Circle" | "Triangle" | "Image" | "Text"
	-- Pixel-space size. For Text this is (0,0) — text size is .TextSize.
	Size: Dim
	-- Top-left position in screen coords (origin at the top-left of the window).
	Position: Dim
	-- Tint when Texture-less / image tint when textured / glyph color for Text.
	Color: Color3
	-- 0 (opaque) … 1 (fully transparent).
	Transparency: number
	-- Higher = drawn on top. Defaults to 0.
	ZIndex: number
	-- Hide without destroying. Equivalent to setting Render=false on parts.
	Visible: boolean
	-- Fires with the property name that was set ("Position", "Size", "Text", etc.).
	Changed: Signal<string>
	-- Text-only properties (read/write on Square/Circle/etc. is a no-op).
	Text: string
	-- Glyph height in pixels.
	TextSize: number
	-- Glyph color. Color (above) tints the box itself; TextColor tints
	-- letters specifically.
	TextColor: Color3
	-- Remove from the scene. Fires "Destroyed" on Changed first.
	function Destroy(self): ()
	-- Custom shader hooks for this primitive's draw call.
	function AttachShader(self, asset: ShaderAsset | FragmentAsset): ()
	function DetachShader(self, asset: ShaderAsset | FragmentAsset): ()
	function ClearShaders(self): ()
	function SetData(self, asset: ShaderAsset | FragmentAsset, name: string, value: number): ()
	function GetData(self, asset: ShaderAsset | FragmentAsset, name: string): number?
end

-- A shader handle bound to either the skybox slot or the post-effect slot.
-- Returned by GUI.SetSkybox / GUI.SetPostEffect; let it drop or call
-- :Destroy() to remove the shader from its slot.
declare class SceneShader
	function SetData(self, name: string, value: number): ()
	function GetData(self, name: string): number?
	function Destroy(self): ()
end

export type GUI_API = {
	-- 2D overlay primitives. Each factory returns a Primitive you can then
	-- position, size, color, etc. on screen.
	Basic: {
		Circle: () -> Primitive,
		Square: () -> Primitive,
		Triangle: () -> Primitive,
		-- Textured rectangle. The asset's pixels become the surface.
		Image: (asset: ImageAsset) -> Primitive,
		-- Text label. Uses the FontAsset to rasterize glyphs at runtime
		-- (sized by .TextSize / colored by .TextColor).
		Font: (asset: FontAsset) -> Primitive,
	},
	-- ── Skybox ────────────────────────────────────────────────────────
	-- Install a fragment shader that runs across the whole window before
	-- the 3D scene draws. Replaces the flat clear color with whatever the
	-- shader produces — gradient sky, procedural stars, animated clouds,
	-- a screenspace ray-march, anything. The 3D scene draws on top of
	-- it normally, so opaque parts naturally occlude it.
	--
	-- Contract: same FragmentAsset shape as a 2D primitive shader (uses
	-- the RuzitUni / IMG / p(idx) prelude). The shader is run on a
	-- fullscreen quad; in.uv is 0..1 over the window. The texture
	-- binding (IMG) is a 1×1 white default — there's no scene texture to
	-- sample in the skybox pass. Useful uniforms:
	--   U.resolution  — window pixel size for aspect-correct math.
	--   U.time        — seconds since window opened, for animation.
	--   p(0u)..p(15u) — your `@ruzit param` floats, controllable from Lua.
	--
	-- Returns a SceneShader handle. Keep it around to call :SetData /
	-- :GetData / :Destroy. Letting it drop AND calling ClearSkybox both
	-- remove the skybox.
	--
	-- Example (vertical gradient sky):
	--   // @ruzit param horizon_r
	--   // @ruzit param horizon_g
	--   // @ruzit param horizon_b
	--   @fragment
	--   fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
	--       let zenith = vec3<f32>(0.10, 0.20, 0.55);
	--       let horizon = vec3<f32>(p(0u), p(1u), p(2u));
	--       let t = clamp(in.uv.y, 0.0, 1.0);
	--       return vec4<f32>(mix(zenith, horizon, t), 1.0);
	--   }
	SetSkybox: (asset: ShaderAsset | FragmentAsset) -> SceneShader,
	-- Remove the active skybox; reverts to the flat clear color.
	ClearSkybox: () -> (),
	-- ── Post-processing ──────────────────────────────────────────────
	-- Install a fragment shader that runs ONCE per frame as the final
	-- pass — every cube, model, GUI primitive, skybox, and shader is
	-- composited into a single 2D image first, then your post shader
	-- transforms that image and writes the result to the swapchain.
	-- This is the right hook for:
	--   * Color grading / LUT-style remapping (warm/cold, sepia, ...)
	--   * Vignette, film grain, CRT, scanlines, chromatic aberration
	--   * Bloom (sample neighbors via in.uv offsets)
	--   * Pixelation, posterize, dithering
	--   * Damage flashes, low-health red-vignette overlays
	--   * Distortion / heat haze (offset the uv before sampling)
	--
	-- Contract: same FragmentAsset shape as a 2D primitive shader, with
	-- ONE crucial difference — IMG is bound to the offscreen scene
	-- texture (the rendered frame BEFORE this pass). textureSample(IMG,
	-- IMG_SAMP, in.uv) returns the pixel that would have ended up on
	-- screen, so a no-op shader is literally:
	--   return textureSample(IMG, IMG_SAMP, in.uv);
	--
	-- The shader runs on a fullscreen quad covering the whole window,
	-- in.uv ∈ [0,1] with top-left origin. Use U.resolution if you need
	-- pixel-space math (e.g. neighbor taps for blur), U.time for
	-- animation, and `p(idx)` to read live params.
	--
	-- Returns a SceneShader handle. Hold onto it for runtime tweaks
	-- (e.g. ramping vignette strength when the player takes damage).
	--
	-- Example (CRT-ish vignette + scanlines):
	--   // @ruzit param strength
	--   @fragment
	--   fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
	--       let scene = textureSample(IMG, IMG_SAMP, in.uv).rgb;
	--       let d = in.uv - vec2<f32>(0.5);
	--       let vignette = 1.0 - dot(d, d) * 1.5 * p(0u);
	--       let scan = 0.92 + 0.08 * sin(in.uv.y * U.resolution.y * 3.14);
	--       return vec4<f32>(scene * vignette * scan, 1.0);
	--   }
	--   -- main.luau:
	--   local effect = GUI.SetPostEffect(Asset.GetAsset("Fragment", "post.crt"))
	--   effect:SetData("strength", 0.6)
	SetPostEffect: (asset: ShaderAsset | FragmentAsset) -> SceneShader,
	-- Remove the active post effect; the rendered scene goes straight to
	-- the swapchain again.
	ClearPostEffect: () -> (),
}


-- =============================================================================
-- Mouse — cursor position, button state, scroll, lock/visibility.
-- =============================================================================

export type MouseButton = "MouseButton1" | "MouseButton2" | "MouseButton3" | "MouseButton4" | "MouseButton5"
-- All cursor names winit accepts. Pass to Mouse:SetCursor or set
-- Mouse.Cursor directly.
export type CursorName = "default" | "pointer" | "text" | "crosshair" | "wait" | "progress" | "help"
	| "move" | "not_allowed" | "grab" | "grabbing"
	| "resize_n" | "resize_s" | "resize_e" | "resize_w" | "resize_ne" | "resize_nw" | "resize_se" | "resize_sw"
	| "resize_ns" | "resize_ew" | "resize_nesw" | "resize_nwse"
	| "context_menu" | "copy" | "alias" | "no_drop" | "all_scroll" | "zoom_in" | "zoom_out"
	| "vertical_text" | "cell"

export type Mouse_API = {
	-- Cursor position in window-local pixels (read-only — write does nothing).
	Position: Dim,
	-- Whether the cursor is shown. Read or assign directly.
	Visible: boolean,
	-- Whether the cursor is locked (recentered every frame). Read or assign.
	Locked: boolean,
	-- Active cursor sprite. Read or assign with one of the CursorName strings.
	Cursor: CursorName,
	-- Fires with (position: Dim, delta: Dim).
	Moved: Signal<Dim, Dim>,
	-- Fires with (button: MouseButton, pressed: boolean).
	InputReceived: Signal<MouseButton, boolean>,
	-- Fires with (deltaX: number, deltaY: number) in lines (one notch of a
	-- standard mouse wheel = ±1). +deltaY is scroll up / away from the user;
	-- +deltaX is scroll right. Precise touchpads emit fractional values.
	Scrolled: Signal<number, number>,
	-- Same as assigning Mouse.Cursor.
	SetCursor: (self: Mouse_API, name: CursorName) -> (),
	-- Lock the cursor to the window center every frame. Combine with
	-- Visible=false for FPS-style mouselook.
	Lock: (self: Mouse_API) -> (),
	-- Release the lock.
	Unlock: (self: Mouse_API) -> (),
	-- Snapshot whether a button is currently held. Cheaper than tracking
	-- InputReceived state yourself.
	IsButtonDown: (self: Mouse_API, button: MouseButton) -> boolean,
}


-- =============================================================================
-- Keyboard — InputChanged signal + IsKeyDown query.
-- =============================================================================

export type Keyboard_API = {
	-- Fires with (id: number, name: string, pressed: boolean). `name` is the
	-- key name like "w", "Escape", "Space", "F1", "ArrowUp", etc.
	InputChanged: Signal<number, string, boolean>,
	-- Snapshot whether a key is currently held. Pass either the name string
	-- ("w", "Space") or the numeric id you got from InputChanged / GetKeyId.
	-- Numeric ids are stable across runs and faster to compare.
	IsKeyDown: (self: Keyboard_API, key: string | number) -> boolean,
	-- Resolve a name string to its stable numeric id (FNV-1a-32).
	GetKeyId: (self: Keyboard_API, name: string) -> number,
}


-- =============================================================================
-- RunService — engine-loop signals.
-- =============================================================================

export type RunService_API = {
	-- Per-frame signal carrying the dt (seconds since last tick).
	Heartbeat: Signal<number>,
	-- Same dt, but fires from inside the render pump (slightly later in the
	-- frame than Heartbeat). Use it for camera updates that need to land
	-- after gameplay state has finalized.
	RenderStepped: Signal<number>,
}


-- =============================================================================
-- Voice — microphone capture + per-peer playback (Opus over your transport).
-- =============================================================================

declare class VoiceShader end

declare class VoiceCapture
	-- Fires with each Opus-encoded audio packet (~20ms = 50 packets/sec).
	-- Pass `bytes` to peers via Steam.Lobby chat / P2P / our Net library /
	-- whatever transport you're using.
	OnPacket: Signal<string>
	-- Stop and release the microphone. After Stop, calling any other method
	-- is a no-op.
	function Stop(self): ()
	-- True between StartCapture and Stop.
	function IsActive(self): boolean
	-- Push-to-talk: Pause() to stop emitting packets, Resume() to start
	-- again. The mic stream stays open the whole time so toggling is
	-- cheap (one atomic). SetActive(true|false) is an alias.
	function Pause(self): ()
	function Resume(self): ()
	function SetActive(self, on: boolean): ()
	function IsPaused(self): boolean
	-- Voice activation: peak amplitude (0..1). 0 = always send (default).
	-- Typical: 0.02..0.05 just above ambient noise. 0.1 = pretty hot mic.
	-- Frames whose peak amplitude is below the threshold are dropped before
	-- they hit the Opus encoder.
	function SetThreshold(self, amplitude: number): ()
	function GetThreshold(self): number
end

declare class VoiceChannel
	-- True while at least one packet is queued for playback.
	IsPlaying: boolean
	-- Feed an Opus packet (received from a peer). Decoded and queued for
	-- playback. Auto-clamps backlog to ~1 second to prevent runaway latency.
	function Push(self, packet: string): ()
	-- Begin the audio mixer for this channel. New incoming packets after
	-- :Stop go to a stalled queue until you :Play again.
	function Play(self): ()
	function Stop(self): ()
	-- Voice-side shaders: same idea as SFX. Apply Voice.Volume / Voice.Speed
	-- / Voice.Spatial to alter playback. Stack multiple for compound effects.
	function ApplyShader(self, shader: VoiceShader): ()
	function ClearShaders(self): ()
	-- 3D position is STATIC — set it once and the channel stays anchored to
	-- that world point as the camera moves around (just like Renderable parts).
	-- Listener = the active Renderable.Camera, so turning the camera also
	-- rotates where the voice comes from. Pass nil (or no args) to clear.
	function SetPosition(self, x: number?, y: number?, z: number?): ()
	-- Same idea but also installs/replaces the Spatial shader with this
	-- distance falloff in one call. Pass nil to clear the spatial shader.
	function SetSpatial(self, x: number?, y: number?, z: number?, falloff: number?): ()
end

declare class VoiceRecorder
	-- Append one Opus packet to the recorded stream.
	function Push(self, packet: string): ()
	-- Drop everything captured so far.
	function Clear(self): ()
	-- Total packet count in the recording.
	function Length(self): number
	-- Total recorded duration in seconds.
	function Duration(self): number
	-- Length-prefixed binary serialization. Save with IO.write or to Steam
	-- Cloud, send over the network, etc. Reload with Voice.LoadRecording.
	function Serialize(self): string
	-- Snapshot the current packet list as a Recording without serializing.
	function ToRecording(self): VoiceRecording
end

declare class VoiceRecording
	function PacketCount(self): number
	function Duration(self): number
	-- Streams the recording's packets into `channel` at the original
	-- ~50 packets/sec pacing. Returns immediately — playback is dripped
	-- by the heart pump. Options: { loop = true, speed = 0.25..4.0 }.
	function PlayInto(self, channel: VoiceChannel, opts: { loop: boolean?, speed: number? }?): ()
end

export type Voice_API = {
	-- Open the default microphone, encode 20ms Opus frames, fire OnPacket
	-- for each one. Stop with `:Stop()` when done.
	StartCapture: () -> VoiceCapture,
	-- Create a per-peer playback channel. Each remote speaker gets one.
	CreateChannel: () -> VoiceChannel,
	-- Fire-and-forget: spin up a transient channel, decode + play the
	-- packet, optionally pinned to a world position. Best for one-off
	-- received messages. For ongoing peer voice prefer CreateChannel().
	PlayPacket: (packet: string, opts: { x: number?, y: number?, z: number?, falloff: number? }?) -> VoiceChannel,
	-- Empty packet collector. Hook to mic.OnPacket OR to received-from-peer
	-- packets — each :Push(bytes) appends to the recorded stream. Save it
	-- with :Serialize() when done.
	NewRecorder: () -> VoiceRecorder,
	-- Inverse of Recorder:Serialize(). Returns a replayable Recording.
	LoadRecording: (data: string) -> VoiceRecording,
	-- Built-in shaders. `Volume(0.5)` halves loudness, `Speed(1.2)` makes
	-- them sound 20% faster, `Spatial(x, y, z)` pans + attenuates by
	-- distance from the listener (the active Renderable.Camera, or set
	-- explicitly via Voice.ListenerPosition).
	Volume: (factor: number) -> VoiceShader,
	Speed: (factor: number) -> VoiceShader,
	Spatial: (x: number, y: number, z: number, falloff: number?) -> VoiceShader,
	-- Override the listener (defaults to the active Renderable.Camera).
	-- Read by Spatial shaders. Pass (0, 0, 0) to revert to camera-tracking.
	ListenerPosition: (x: number, y: number, z: number) -> (),
}


-- =============================================================================
-- Steam — Steamworks integration. User, Friends, Achievements, Stats, Lobby,
-- Server, Overlay, App/Utils, Rich Presence, Cloud, Workshop, Remote Play.
-- All requires the Steam runtime to be present and the right app id to be
-- set in build.toml's [steam] table (or RUZIT_STEAM_APPID env var).
-- =============================================================================

declare class SteamLobby
	-- The lobby's Steam id (cast as a decimal string).
	ID: string
	-- The lobby owner's Steam id.
	Owner: string
	-- Fires with (memberSteamId: string).
	OnMemberJoined: Signal<string>
	-- Fires with (memberSteamId: string).
	OnMemberLeft: Signal<string>
	-- Fires with (memberSteamId: string?). Nil when lobby-wide data changed,
	-- otherwise the id of the member whose per-member data changed.
	OnDataUpdate: Signal<string?>
	-- Snapshot of every member's Steam id.
	function Members(self): { string }
	-- Lobby-wide key/value store. Visible to everyone in the lobby.
	function SetData(self, key: string, value: string): ()
	function GetData(self, key: string): string?
	-- Broadcast a chat message to every member.
	function SendChat(self, message: string): ()
	-- Leave the lobby. The handle goes inert after this.
	function Leave(self): ()
end

declare class SteamServer
	-- The server's Steam id.
	ID: string
	-- Fires with (clientSteamId: string) when a client successfully authenticates.
	OnClientAuthed: Signal<string>
	-- Server-list metadata. Players see this in the in-game / web server browser.
	function SetServerName(self, name: string): ()
	function SetMapName(self, map: string): ()
	function SetMaxPlayers(self, max: number): ()
	-- Anonymous logon — no game-server-account credentials required.
	function LogOnAnonymous(self): ()
	-- Stop and clean up the server. Idempotent.
	function Stop(self): ()
end

export type SteamPersonaState = "Offline" | "Online" | "Busy" | "Away" | "Snooze" | "LookingToTrade" | "LookingToPlay"
export type SteamAvatarSize = "small" | "medium" | "large"
export type SteamLobbyKind = "Public" | "Private" | "FriendsOnly" | "Invisible"
export type SteamServerMode = "NoAuthentication" | "Authentication" | "AuthenticationAndSecure"
export type SteamOverlayDialog = "Friends" | "Community" | "Players" | "Settings" | "Stats" | "Achievements" | "OfficialGameGroup"
export type SteamOverlayUserDialog = "steamid" | "chat" | "jointrade" | "stats" | "achievements"
	| "friendadd" | "friendremove" | "friendrequestaccept" | "friendrequestignore"
export type SteamOverlayStoreMode = "None" | "AddToCart" | "AddToCartAndShow"
export type SteamNotificationPosition = "TopLeft" | "TopRight" | "BottomLeft" | "BottomRight"

export type SteamFriend = { ID: string, Name: string, State: SteamPersonaState }
export type SteamLobbyInfo = { ID: string, MemberCount: number?, Name: string? }

export type SteamServerOptions = {
	-- Game-server port. Default Valve recommendation is 27015.
	port: number?,
	-- Server-query port (the master-server protocol). Default 27016.
	queryPort: number?,
	-- Auth strictness. NoAuthentication for LAN/dev, AuthenticationAndSecure
	-- for ranked/anti-cheat-relevant matches.
	mode: SteamServerMode?,
	-- Free-form version string Steam uses for compatibility filtering.
	version: string?,
	-- Display name shown in server browsers.
	name: string?,
	-- Map / mode tag.
	map: string?,
	-- Slot count.
	max: number?,
}

export type SteamCloudFile = { Name: string, Size: number }
export type SteamFriendInfo = {
	ID: string, Name: string, Nickname: string?, State: SteamPersonaState,
	Game: { AppID: number }?,
}

export type Steam_API = {
	-- Local user / friends.
	User: {
		-- Local user's steam id.
		ID: string,
		-- Local user's persona name.
		Name: string,
		-- Steam community level.
		Level: number,
		-- AccountID portion of the steam id (the lower 32 bits).
		AccountID: () -> number,
		-- True while we're connected to the Steam backend.
		LoggedOn: () -> boolean,
		-- Snapshot of the local user's friend list.
		GetFriends: () -> { SteamFriend },
		-- Local user's avatar at the given size (default "medium").
		GetAvatar: (size: SteamAvatarSize?) -> ImageAsset?,
		-- A friend's avatar. Only works for users on your friend list /
		-- in the same lobby — otherwise returns nil. For arbitrary Steam
		-- IDs use GetAvatarAsync.
		GetFriendAvatar: (id: string, size: SteamAvatarSize?) -> ImageAsset?,
		-- Async lookup that works for ANY Steam ID, not just friends. Hits
		-- cache instantly when the avatar is already known; otherwise asks
		-- Steam for the user's profile data and fires the signal when it
		-- arrives (a frame or two later). Signal fires once with
		-- (ImageAsset?) — nil if Steam couldn't load the user.
		GetAvatarAsync: (id: string, size: SteamAvatarSize?) -> Signal<ImageAsset?>,
		-- Persona state, nickname, current game, etc.
		GetFriendInfo: (id: string) -> SteamFriendInfo,
		-- Force Steam to re-fetch a user's profile data. Returns true when
		-- a request was actually sent (returns false when cached data is
		-- already fresh). Listen for SteamPersonaStateChange via the user-
		-- info APIs.
		RequestInfo: (id: string, nameOnly: boolean?) -> boolean,
		-- Send a Steam invite to your current game. `connectString` is
		-- whatever your game expects (often a lobby id).
		InviteToGame: (id: string, connectString: string) -> (),
	},
	-- Steam achievement bookkeeping. `name` is the API id you set in the
	-- partner site (NOT the display name).
	Achievements: {
		Unlock: (name: string) -> (),
		Clear: (name: string) -> (),
		IsUnlocked: (name: string) -> boolean,
		-- Push pending unlocks to Steam's backend. Required after Unlock /
		-- Clear or values won't sync.
		Store: () -> (),
	},
	-- Persistent stats. Same `name` model as achievements.
	Stats: {
		SetInt: (name: string, value: number) -> (),
		SetFloat: (name: string, value: number) -> (),
		GetInt: (name: string) -> number,
		GetFloat: (name: string) -> number,
		-- Push pending stats to Steam.
		Store: () -> (),
	},
	-- Lobby matchmaking.
	Lobby: {
		-- Returns a Signal that fires once with (lobby: SteamLobby?, err: string?).
		Create: (maxMembers: number?, kind: SteamLobbyKind?) -> Signal<SteamLobby?, string?>,
		-- Returns a Signal that fires once with (lobby: SteamLobby?, err: string?).
		Join: (id: string) -> Signal<SteamLobby?, string?>,
		-- Returns a Signal that fires once with the matching lobbies.
		List: () -> Signal<{ SteamLobbyInfo }>,
	},
	-- Game server hosting.
	Server: {
		-- Spin up a Steam game server. Returns a SteamServer handle for
		-- runtime control. Logon mode is set with :LogOnAnonymous() (or
		-- the credential variants once we expose them).
		Start: (opts: SteamServerOptions) -> SteamServer,
	},
	-- The in-game Steam overlay.
	Overlay: {
		-- Open one of the built-in overlay panels.
		Show: (dialog: SteamOverlayDialog) -> (),
		ShowFriends: () -> (),
		ShowAchievements: () -> (),
		ShowSettings: () -> (),
		-- Open the user's browser inside the overlay.
		ShowURL: (url: string) -> (),
		-- Open the store page for an app. `mode` controls cart behavior.
		ShowStore: (appId: number?, mode: SteamOverlayStoreMode?) -> (),
		-- Open a user's profile / chat / etc.
		ShowUser: (id: string, dialog: SteamOverlayUserDialog?) -> (),
		-- Pop up the friend invite UI for a specific lobby.
		ShowInvite: (lobbyId: string) -> (),
	},
	-- Static info about the running app + currently-installed apps.
	App: {
		IsAppInstalled: (appId: number) -> boolean,
		IsDlcInstalled: (appId: number) -> boolean,
		-- True if the local user owns / is subscribed to this app.
		IsSubscribed: () -> boolean,
		IsVacBanned: () -> boolean,
		-- Steam build id of the currently-installed version.
		BuildId: () -> number,
		-- Absolute filesystem path of an app's install folder.
		InstallDir: (appId: number) -> string,
		-- Steam id of the user that owns this app's install (often == the
		-- local user, but family-sharing changes that).
		OwnerID: () -> string,
		-- Steam interface language (e.g. "english", "german").
		CurrentLanguage: () -> string,
		AvailableLanguages: () -> { string },
		-- Branch name for users on a beta branch, or nil for the main
		-- branch (default).
		BetaName: () -> string?,
		-- The full launch command line as Steam invoked Ruzit with.
		LaunchCommandLine: () -> string,
	},
	-- Runtime-environment info via the SteamUtils interface.
	Utils: {
		UILanguage: () -> string,
		IpCountry: () -> string,
		-- Steam's authoritative wall-clock time (Unix seconds).
		ServerTime: () -> number,
		IsSteamDeck: () -> boolean,
		-- Steam app id this process is running under.
		AppID: () -> number,
		-- Where Steam pop-ups (achievement-unlock toasts etc.) appear.
		SetOverlayPosition: (position: SteamNotificationPosition) -> (),
	},
	-- Discord-style "playing X" / "in lobby Y" status. Set displays in
	-- friend lists; Clear blanks them.
	RichPresence: {
		Set: (key: string, value: string?) -> boolean,
		Clear: () -> (),
	},
	-- Per-user / per-app cloud save. Files are scoped to YOUR app's quota.
	Cloud: {
		-- The user's account-wide cloud-sync setting.
		IsEnabledForAccount: () -> boolean,
		-- The current app's per-app cloud-sync setting.
		IsEnabledForApp: () -> boolean,
		-- Toggle the per-app cloud-sync setting.
		SetEnabledForApp: (enabled: boolean) -> (),
		Files: () -> { SteamCloudFile },
		Exists: (name: string) -> boolean,
		Read: (name: string) -> string?,
		Write: (name: string, data: string) -> (),
		Delete: (name: string) -> boolean,
	},
	-- Steam Workshop subscriptions / installs.
	Workshop: {
		-- Item ids the user is subscribed to.
		SubscribedItems: () -> { string },
		-- Live install state for an item.
		ItemState: (id: string) -> {
			Subscribed: boolean, Installed: boolean, NeedsUpdate: boolean,
			Downloading: boolean, DownloadPending: boolean,
		},
		-- Local install info — nil while not installed.
		InstallInfo: (id: string) -> { Folder: string, SizeOnDisk: number, Timestamp: number }?,
		-- Live download progress — nil if not downloading.
		DownloadProgress: (id: string) -> { Downloaded: number, Total: number }?,
		-- Force-trigger a download. `highPriority` jumps the Steam queue.
		DownloadItem: (id: string, highPriority: boolean?) -> boolean,
		-- Combined fetch: returns a table with state flags AND install info if
		-- the item is on disk, plus a Files() helper that lists every file
		-- in the install folder (absolute paths — pass them straight to
		-- Asset.ImportAsset).
		GetItem: (id: string) -> {
			ID: string,
			Subscribed: boolean, Installed: boolean, NeedsUpdate: boolean,
			Downloading: boolean, DownloadPending: boolean,
			Folder: string?, SizeOnDisk: number?, Timestamp: number?,
			DownloadProgress: { Downloaded: number, Total: number }?,
			Files: (() -> { string })?,
		},
	},
	-- Steam Remote Play streaming sessions.
	RemotePlay: {
		Sessions: () -> { { UserID: string, ClientName: string?, Width: number?, Height: number? } },
	},
}


-- =============================================================================
-- GPU — info about the active graphics adapter, frame stats, and a CPU-side
-- raycast against the 3D scene. Useful for click-to-select, line-of-sight
-- gameplay checks, and surfacing hardware capabilities to settings menus.
-- =============================================================================

-- Snapshot of one ray hit returned by GPU.Raycast. The Position / Normal are
-- in world space; Distance is the world-space length from the ray's origin.
export type RaycastHit = {
	Part: BasePart,
	Position: Vector,
	Distance: number,
	Normal: Vector,
}

-- Static info about the GPU Ruzit ended up running on. All fields are filled
-- once at window-open time and don't change for the rest of the process.
export type GPUInfo = {
	-- Display name reported by the driver (e.g. "NVIDIA GeForce RTX 4070").
	Name: string,
	-- Friendly vendor label: "NVIDIA" / "AMD" / "Intel" / "Apple" / "ARM" /
	-- "Qualcomm" / "Imagination" / "Microsoft" / "Other" / "Unknown". The
	-- raw PCI vendor id is also exposed as VendorID.
	Vendor: string,
	VendorID: number,
	DeviceID: number,
	-- Graphics backend wgpu chose at boot: "Dx12" on Windows by default,
	-- "Vulkan" / "Metal" / "Gl" / "BrowserWebGpu" elsewhere.
	Backend: string,
	-- Driver version string, free-form.
	Driver: string,
	-- Extra driver detail (build, date, etc.) — may be empty.
	DriverInfo: string,
	-- "DiscreteGpu" / "IntegratedGpu" / "VirtualGpu" / "Cpu" / "Other".
	-- Useful for picking quality presets at startup.
	DeviceType: string,
}

-- Hard caps reported by the wgpu device. Useful for picking texture sizes,
-- splitting work across compute dispatches, etc. wgpu doesn't expose total
-- VRAM portably; MaxBufferSize is the closest proxy for a single allocation.
export type GPULimits = {
	-- Largest 2D texture dimension you can create (e.g. 16384 on most GPUs).
	MaxTextureSize: number,
	-- Largest single buffer allocation in bytes. Closest stand-in for "VRAM
	-- per allocation" — actual VRAM is usually larger but split across many
	-- allocations.
	MaxBufferSize: number,
	-- Max simultaneously bound bind groups in one pipeline.
	MaxBindGroups: number,
	-- Max vertex buffers attached to one pipeline.
	MaxVertexBuffers: number,
	-- Max compute workgroup size per axis (when compute is added).
	MaxComputeWorkgroup: { X: number, Y: number, Z: number },
}

-- Live frame stats. Smoothed dt is an EMA so the FPS reading doesn't strobe.
export type GPUFrameStats = {
	-- Smoothed frames-per-second.
	FPS: number,
	-- Smoothed frame time in seconds.
	FrameTime: number,
	-- Total frames drawn since process start.
	FrameCount: number,
	-- Wall-clock seconds since the GPU module started tracking.
	Uptime: number,
	-- Live count of renderable parts in the scene.
	PartCount: number,
}

export type GPU_API = {
	-- Static adapter info (name, vendor, backend, driver, device type). Cached
	-- — calling repeatedly is free. Useful for logging, settings menus, and
	-- gating high-cost features behind DeviceType == "DiscreteGpu".
	Info: () -> GPUInfo,
	-- Hard limits the active GPU device reported. wgpu doesn't expose total
	-- VRAM portably across DX12/Vulkan/Metal — MaxBufferSize is the closest
	-- single number you can rely on for "biggest thing I can allocate".
	Limits: () -> GPULimits,
	-- FPS / frame time / scene part count. Updated once per heart tick.
	-- Cheap; safe to poll every frame for a debug overlay.
	FrameStats: () -> GPUFrameStats,
	-- CPU-side ray test against every renderable BasePart in the scene.
	--
	--   origin      : world-space ray start (Vector).
	--   direction   : world-space ray direction (Vector). Length doesn't
	--                 matter — Ruzit normalizes it. Zero-length errors.
	--   filter      : optional callback. Hits are visited in nearest-first
	--                 order. For each hit, `filter(part)` is invoked; return
	--                 true to accept it as the result, return false (or nil)
	--                 to skip past it and check the next-nearest hit. If
	--                 omitted, the first hit is always accepted.
	--   maxDistance : optional cap in world units. Hits past this distance
	--                 are ignored. Defaults to ~1e6.
	--
	-- Returns a RaycastHit for the first accepted intersection, or nil if
	-- nothing within range passed the filter.
	--
	-- Geometry: cubes/models are tested as oriented bounding boxes derived
	-- from the part's CFrame + Size. Spheres are tested as ellipsoids with
	-- the same CFrame + Size. Parts with Render = false are skipped.
	--
	-- Example — pick a part under the mouse, ignoring red ones:
	--   local Mouse = import("Mouse")
	--   local origin, dir = GPU.ScreenToRay(Mouse.Position)
	--   local hit = GPU.Raycast(origin, dir, function(part)
	--       return part.Color ~= Color3.new(1, 0, 0)
	--   end)
	--   if hit then
	--       print("clicked", hit.Part, "at", hit.Position, "distance", hit.Distance)
	--   end
	Raycast: (
		origin: Vector,
		direction: Vector,
		filter: ((part: BasePart) -> boolean)?,
		maxDistance: number?
	) -> RaycastHit?,
	-- Convert a screen pixel (Mouse.Position style) to a world-space ray.
	-- Returns (origin, direction) — feed straight into GPU.Raycast.
	-- Uses the active Renderable.Camera as the eye + projection.
	ScreenToRay: (point: Dim) -> (Vector, Vector),
	-- Project a world-space point onto the screen. Returns:
	--   (Dim, false) — point is in front of the camera; Dim is the pixel.
	--   (nil, true)  — point is at or behind the near plane; no projection.
	-- Useful for billboards, world-anchored UI, off-screen indicators.
	WorldToScreen: (world: Vector) -> (Dim?, boolean),
}


-- =============================================================================
-- Actor — parallel CPU work. Spin up a worker pool; Push args to run a
-- function on another core; Pop the results back on the main thread.
-- =============================================================================

declare class Actor
	-- Hand args off to a worker thread. The args are deep-copied across the
	-- thread boundary, so mutating them after Push has no effect on the worker.
	-- Allowed types: nil, boolean, number, string, table (recursively, with
	-- only those types as keys/values). Functions, userdata, threads, etc.
	-- are rejected — workers can't share Luau state with the main thread.
	function Push(self, ...: any): ()
	-- Pop the next ready result.
	--   Pop()  / Pop(false) — non-blocking. Returns no values if nothing
	--                          is ready. Use this from a heart tick or
	--                          render loop you can't afford to stall.
	--   Pop(true)            — blocking. Sleeps until a worker finishes
	--                          (or every worker has exited, in which case
	--                          it returns no values). The main thread
	--                          stops pumping while it waits, so only use
	--                          this from a script that's intentionally
	--                          waiting (e.g. a startup loader).
	-- Order is "first to finish, first out" (NOT Push order — work runs
	-- in parallel). Re-raises any error a worker hit while running.
	function Pop(self, yield_for_result: boolean?): ...any
	-- Number of jobs currently in flight + ready results not yet popped.
	-- Drain pattern:
	--   while actor:Pending() > 0 do
	--       local r = actor:Pop()
	--       if r ~= nil then ... end
	--   end
	function Pending(self): number
	-- How many worker threads back this actor.
	function Threads(self): number
	-- Stop accepting new jobs and let workers exit once they finish their
	-- current call. Already-completed results stay drainable via Pop after
	-- Close. Idempotent.
	function Close(self): ()
end

export type Actor_API = {
	-- Spawn a worker pool that runs the given function in parallel on other
	-- CPU cores. You can pass either the function directly (Actor reads its
	-- source out of the script and recompiles it for each worker) or a Luau
	-- source string whose chunk evaluates to a function.
	--
	-- Restrictions on the function form:
	--   * Must be defined inline in a .luau script, not via loadstring.
	--   * Must NOT reference upvalues from the outer scope. Workers run in
	--     an isolated Luau state and can't see your locals — pass anything
	--     they need as arguments to Push.
	--   * Method shorthand (`function obj:m(...)`) isn't supported.
	--
	-- Workers run in completely isolated Luau states: they get math, table,
	-- string, bit32, buffer, utf8, coroutine, and the basic library — but
	-- NO `import`, `require`, `print`, `loadstring`, `dofile`, or
	-- `__dirname`. Workers cannot touch the main thread's globals, GUI,
	-- assets, sockets, or anything else import-shaped.
	--
	-- threads defaults to the number of CPU cores. Pass an explicit count
	-- for fine control (e.g. 1 for serial, 2 for I/O-bound work, etc.).
	--
	-- Example (function form):
	--   local fib = Actor.new(function(n)
	--       if n < 2 then return n end
	--       local a, b = 0, 1
	--       for _ = 2, n do a, b = b, a + b end
	--       return b
	--   end)
	--   for i = 1, 32 do fib:Push(i) end
	--
	-- Example (string form, useful when you want to do worker-local setup):
	--   local fib = Actor.new([[
	--       local memo = {}
	--       return function(n)
	--           if memo[n] then return memo[n] end
	--           local a, b = 0, 1
	--           for _ = 2, n do a, b = b, a + b end
	--           memo[n] = b
	--           return b
	--       end
	--   ]])
	new: ((fn: (...any) -> ...any, threads: number?) -> Actor)
		& ((source: string, threads: number?) -> Actor),
	-- Actor.FromFile(path, threads?) — load worker code from a separate
	-- .luau file. Path resolution is identical to `require`, including
	-- the project-wide file-mode setting from build.toml ("File Type"):
	--
	-- Relative mode (the default; matching most projects):
	--   "./worker"          — relative to the calling script's directory.
	--   "../shared/work"    — parent-relative.
	--   "subdir/helper"     — relative-without-./ also works.
	--   "@PkgId/jobs/heavy" — explicit cross-package lookup.
	--   "@self/jobs/heavy"  — same package as the caller, root-relative.
	--
	-- Global mode (when build.toml sets `"File Type" = "Global"`):
	--   "workers/fib"       — resolved from the project root, NOT the
	--                          caller's directory.
	--   "@PkgId/..."        — same as in Relative mode.
	--
	-- The .luau / .lua extension is appended automatically; pass the
	-- bare module name. `init.luau` is honored for folder-style modules.
	-- DLC packages and Packages/ imports each use THEIR OWN file-mode
	-- setting (from each package's ManagedInfo.toml), so a Relative-mode
	-- DLC keeps resolving relative to the caller even inside a Global
	-- host project.
	--
	-- The file's raw text is read and used verbatim as the actor's
	-- source — same rules as the string form of Actor.new (the chunk
	-- must evaluate to a function, and the function must not reference
	-- any upvalue from the surrounding script).
	--
	-- Useful when worker code grows past a one-liner — keep it in its
	-- own file with full LSP support and full Luau syntax instead of
	-- buried inside a `[[...]]` literal.
	--
	-- Example:
	--   -- workers/fib.luau
	--   local memo = {}
	--   return function(n)
	--       if memo[n] then return memo[n] end
	--       local a, b = 0, 1
	--       for _ = 2, n do a, b = b, a + b end
	--       memo[n] = b
	--       return b
	--   end
	--
	--   -- main.luau
	--   local fib = Actor.FromFile("./workers/fib")
	--   for i = 1, 32 do fib:Push(i) end
	FromFile: (path: string, threads: number?) -> Actor,
}


-- =============================================================================
-- The full set of names you can pass to `import(...)`. Anything not in this
-- table throws "import: unknown library 'X'". The mapped value is the API
-- shape; LSP follows it through `import(...)` calls so hover docs work
-- everywhere.
-- =============================================================================

export type Imports = {
	Actor: Actor_API,
	Asset: Asset_API,
	GPU: GPU_API,
	GUI: GUI_API,
	IO: IO_API,
	Keyboard: Keyboard_API,
	Managed: Managed_API,
	Mouse: Mouse_API,
	Net: Net_API,
	Primitives: Primitives_API,
	Process: Process_API,
	Renderable: Renderable_API,
	RunService: RunService_API,
	Serde: Serde_API,
	SFX: SFX_API,
	Signal: Signal_API,
	Steam: Steam_API,
	Voice: Voice_API,
	Window: Window_API,
}


-- The global import function. Pass any name from `Imports` and you get a
-- typed handle for that subsystem. Unknown names throw at runtime.
declare import: <K>(name: keyof<Imports> & K) -> index<Imports, K>

-- The directory of the calling script as an absolute path. Combine with
-- `require(__dirname .. "/...")` to load sibling Luau modules.
declare __dirname: string
