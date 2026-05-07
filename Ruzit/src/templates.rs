pub const BUILD_TOML: &str = r#"Name = "{name}"
Version = "0.1.0"
Creator = ""

[configs]
"File Type" = "Relative"

[exe]
# name = "{name}"     # output exe name when running `Ruzit Build` (defaults to entry stem)
# icon = "logo"       # looks for <icon>.ico next to build.toml and embeds it as the exe icon
# windowed = true     # default. Launcher is windows-subsystem — no console
                      # window unless --console is passed. Set to false to
                      # ship a console-subsystem launcher whose stdout is
                      # always visible.
# compress = true     # shorthand: sets compress_scripts AND compress_assets.
# compress_scripts = true   # zstd-compress every Lua script before encryption.
# compress_assets = true    # zstd-compress every asset before encryption.
                            # Both decompress lazily on access (per-script on
                            # require, per-asset on Asset.GetAsset). Smaller
                            # `.managed` files at the cost of a one-time
                            # decompression per access.
# shard_assets = true       # split assets across `<id>.assets.shardNNNN.managed`
                            # files instead of one monolithic `<id>.assets.managed`,
                            # plus a small `.assets.manifest.managed` index. Shard
                            # size is auto-tuned (~ceil(sqrt(asset_count)) shards,
                            # 4-256 MB each) so patches only re-download the shards
                            # that actually changed — huge win for content updates
                            # over Steam Pipe / CDNs without drowning the OS in
                            # thousands of tiny files.

[steam]
# app_id = 480        # Steam app id used by `import("Steam")`. 480 is Spacewar
                      # (Valve's free dev test app). Replace with your own
                      # registered app id at ship time. Falls back to 480 if
                      # unset; the RUZIT_STEAM_APPID env var overrides this.
"#;

pub const MAIN_LUAU: &str = r#"--!strict

local IO = import("IO")

print("Hello from {name}!")
print("project root =", __dirname)
print("IO available:", typeof(IO) == "table")
"#;

pub const MANAGED_INFO_TOML: &str = r#"ID = "{id}"
Name = "{name}"
Version = "0.1.0"
Creator = ""
Entry = "init.luau"

[configs]
"File Type" = "Relative"
"#;

pub const MANAGED_INIT_LUAU: &str = r#"--!strict
--
-- {name} package entry point.
-- The host game loads this module via:
--     local Managed = import("Managed")
--     local pkg = Managed.GetPackage("{id}")
--     local mod = (require :: any)(pkg.Origin)
--
-- The require runs this file once and caches the returned table — every
-- caller (including other packages) sees the same instance, so any state
-- you put on `M` is shared across the whole program.

local M = {}

M.greeting = "hello from {id}"

function M.add(a: number, b: number): number
	return a + b
end

return M
"#;

pub const VSCODE_SETTINGS: &str = r#"{
    "luau-lsp.types.definitionFiles": [
        "./types.d.luau"
    ],
    "luau-lsp.require.mode": "relativeToFile",
    "files.associations": {
        "*.luau": "luau"
    }
}
"#;

pub const TYPES_DLUAU: &str = r#"

declare class IOHandle
	function read(self, format: string?): string
	function write(self, content: string): ()
	function close(self): ()
	function path(self): string
end

export type IO_API = {
	read: (path: string) -> string,
	write: (path: string, content: string) -> (),
	append: (path: string, content: string) -> (),
	exists: (path: string) -> boolean,
	remove: (path: string) -> (),
	mkdir: (path: string) -> (),
	list: (path: string) -> { string },
	open: (path: string, mode: ("r" | "w" | "a" | "r+" | "w+" | "a+")?) -> IOHandle,
	getpath: (path: string) -> string,
}

declare class TcpConnection
	function Send(self, data: string): ()
	function Receive(self, n: number?): string
	function Close(self): ()
	function Peer(self): string
end

declare class TcpListener
	function Accept(self): TcpConnection
	function Close(self): ()
	function Address(self): string
end

declare class UdpHandle
	function Send(self, addr: string, data: string): number
	function Receive(self, n: number?): (string, string)
	function Close(self): ()
	function Address(self): string
end

declare class IpcConnection
	function Send(self, data: string): ()
	function Receive(self, n: number?): string
	function Close(self): ()
end

declare class IpcListener
	function Accept(self): IpcConnection
	function Close(self): ()
end

declare class WebSocketConn
	function Send(self, msg: string): ()
	function Receive(self): string
	function Close(self): ()
end

declare class WebSocketListener
	function Accept(self): WebSocketConn
	function Close(self): ()
end

export type HttpRequest = { method: string, path: string, headers: { [string]: string }, body: string }
export type HttpResponse = { status: number, headers: { [string]: string }, body: string }

export type Net_API = {
	Serve: (addr: string, handler: (HttpRequest) -> (HttpResponse | string)) -> (),
	Request: (method: string, url: string, body: string?, headers: { [string]: string }?) -> HttpResponse,
	TCP: { Connect: (addr: string) -> TcpConnection, Host: (addr: string) -> TcpListener },
	UDP: { Bind: (addr: string) -> UdpHandle },
	IPC: { Connect: (name: string) -> IpcConnection, Host: (name: string) -> IpcListener },
	Socket: { Connect: (url: string) -> WebSocketConn, Host: (addr: string) -> WebSocketListener },
}

export type HashAlgo = "md5" | "sha1" | "sha224" | "sha256" | "sha384" | "sha512" | "sha3-256" | "sha3-512" | "keccak256" | "blake3"
export type CompressAlgo = "gzip" | "zlib" | "deflate" | "zstd"

export type Serde_API = {
	Encode: (format: ("json" | "toml" | "yaml"), data: any, pretty: boolean?) -> string,
	Decode: (format: ("json" | "toml" | "yaml"), text: string) -> any,
	Hash: (algo: HashAlgo, data: string, encoding: ("hex" | "base64" | "bytes")?) -> string,
	Compress: (algo: CompressAlgo, data: string, level: number?) -> string,
	Decompress: (algo: CompressAlgo, data: string) -> string,
}

export type Process_API = {
	Os: string,
	Arch: string,
	Family: string,
	Pid: number,
	CpuCount: number,
	IsBuilt: boolean,
	Env: ((name: string) -> string?) & (() -> { [string]: string }),
	SetEnv: (name: string, value: string) -> (),
	Close: (code: number?) -> never,
	Args: () -> { string },
	Memory: () -> { total: number, free: number, used: number, available: number, total_swap: number, used_swap: number },
	BindToHeart: (id: string, fn: (dt: number) -> ()) -> (),
	UnbindFromHeart: (id: string) -> (),
}

declare class ImageAsset
	function Width(self): number
	function Height(self): number
	function Source(self): string
	function Pixels(self): string
end

declare class SoundData
	function Source(self): string
	function ByteCount(self): number
end


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
	FromPixels: (width: number, height: number, rgba: string) -> ImageAsset,
}

export type WindowOptions = {
	title: string?, width: number?, height: number?,
	min_width: number?, min_height: number?, max_width: number?, max_height: number?,
	fullscreen: boolean?, borderless: boolean?, resizable: boolean?,
	maximized: boolean?, visible: boolean?, transparent: boolean?,
	always_on_top: boolean?, icon: ImageAsset?,
}

declare class Connection
	Connected: boolean
	function Disconnect(self): ()
end

-- Variadic generics on `declare class` aren't supported by the stable
-- luau-lsp parser yet, but generic type aliases ARE — so Signal lives as
-- a parameterized table type instead. Same call shape (signal:Connect(...))
-- with full callback-arg type narrowing: `Signal<number, string>` makes
-- `signal:Connect(function(a, b) ... end)` infer `a:number, b:string`.
export type Signal<T...> = {
	Connect: (self: Signal<T...>, fn: (T...) -> ()) -> Connection,
	Once: (self: Signal<T...>, fn: (T...) -> ()) -> Connection,
	Wait: (self: Signal<T...>) -> T...,
	Fire: (self: Signal<T...>, T...) -> (),
	DisconnectAll: (self: Signal<T...>) -> (),
}

export type Signal_API = { new: <T...>() -> Signal<T...> }

declare class WindowHandle
	-- Fires with the property name that changed: "Title", "Size", "Position", "Fullscreen", etc.
	Changed: Signal<string>
	OnFocus: Signal<>
	OnUnfocus: Signal<>
	function Close(self): ()
	function BindToClose(self, fn: () -> ()): ()
	function Resize(self, width: number, height: number): ()
	function SetTitle(self, title: string): ()
	-- Toggles "borderless fullscreen" — strips decorations AND enters
	-- Fullscreen::Borderless so the window covers the OS taskbar/dock.
	-- The Open-time `borderless = true` option does the same thing.
	function SetBorderless(self, borderless: boolean): ()
	-- Strips title bar / minimize / close buttons WITHOUT going fullscreen.
	-- Use this for decorationless tool overlays or floating panels.
	function SetDecorations(self, decorated: boolean): ()
	function SetFullscreen(self, fullscreen: boolean): ()
	function SetResizable(self, resizable: boolean): ()
	function SetMaximized(self, maximized: boolean): ()
	function SetMinimized(self, minimized: boolean): ()
	function SetVisible(self, visible: boolean): ()
	function SetAlwaysOnTop(self, on_top: boolean): ()
	function SetPosition(self, x: number, y: number): ()
	function Focus(self): ()
	function RequestRedraw(self): ()
	function Width(self): number
	function Height(self): number
	function Title(self): string
	function IsFullscreen(self): boolean
	function IsOpen(self): boolean
	
	
	function GetViewport(self): Camera
end

export type Window_API = { Open: (opts: WindowOptions?) -> WindowHandle }


declare class SoundShader end

declare class Sound
	Started: Signal<>
	Stopped: Signal<>
	Source: string
	function Play(self): ()
	function Stop(self): ()
	function IsPlaying(self): boolean
	-- Fluent built-in shaders: each one replaces any prior call to the same
	-- method on this Sound (so Volume(0.5) → Volume(0.8) ends at 0.8, no
	-- stacking). Call before :Play() — they apply on the next playback.
	function Volume(self, factor: number): ()
	function Speed(self, factor: number): ()
	function Pitch(self, factor: number): ()
	function Pan(self, amount: number): ()
	function LowPass(self, freq: number): ()
	function HighPass(self, freq: number): ()
	function FadeIn(self, seconds: number): ()
	function FadeOut(self, seconds: number): ()
	function Delay(self, seconds: number): ()
	function Loop(self): ()
	function Distortion(self, amount: number): ()
	function Echo(self, delay_ms: number, feedback: number?, mix: number?): ()
	function Reverb(self, mix: number?, decay: number?): ()
	function Tremolo(self, rate: number?, depth: number?): ()
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
	function AttachShader(self, asset: ShaderAsset | FragmentAsset): ()
	function DetachShader(self, asset: ShaderAsset | FragmentAsset): ()
	function SetData(self, asset: ShaderAsset | FragmentAsset, name: string, value: number): ()
	function GetData(self, asset: ShaderAsset | FragmentAsset, name: string): number?
	-- Fires every `interval` seconds with the elapsed time since playback began.
	function LinkToUpdate(self, interval: number): Signal<number>
end

export type SFX_API = {
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

declare class Package
	ID: string
	Name: string
	Version: string
	Creator: string
	Entry: string
	Origin: string
	function HasFile(self, key: string): boolean
	function HasAsset(self, key: string): boolean
	function Files(self): { string }
	function Assets(self): { string }
end

export type Managed_API = {
	IsPackage: (id: string) -> boolean,
	GetPackage: (id: string) -> Package?,
	List: () -> { string },
	Default: () -> Package?,
}

declare class Dim
	X: number
	Y: number
	function Lerp(self, other: Dim, t: number): Dim
	function __add(self, other: Dim): Dim
	function __sub(self, other: Dim): Dim
	
	
	function __mul(self, other: number): Dim
	function __div(self, other: number): Dim
	function __unm(self): Dim
end

declare class Color3
	R: number
	G: number
	B: number
	function Lerp(self, other: Color3, t: number): Color3
	
	function __add(self, other: Color3): Color3
	function __sub(self, other: Color3): Color3
end

declare class Vector
	X: number
	Y: number
	Z: number
	Magnitude: number
	function Lerp(self, other: Vector, t: number): Vector
	function __add(self, other: Vector): Vector
	function __sub(self, other: Vector): Vector
	function __mul(self, other: number): Vector
	function __div(self, other: number): Vector
	function __unm(self): Vector
end

declare class ModelAsset
	function VertexCount(self): number
	function TriangleCount(self): number
	function Source(self): string
end

declare class BasePart
	Shape: "Cube" | "Sphere" | "Model"
	CFrame: CFrame
	Size: Vector
	Color: Color3
	Render: boolean
	Texture: ImageAsset?
	-- Fires with the property name that was set: "CFrame", "Size", "Color", "Render", "Texture", "Destroyed".
	Changed: Signal<string>
	function Destroy(self): ()
	function AttachShader(self, asset: ShaderAsset | FragmentAsset): ()
	function DetachShader(self, asset: ShaderAsset | FragmentAsset): ()
	function ClearShaders(self): ()
	function SetData(self, asset: ShaderAsset | FragmentAsset, name: string, value: number): ()
	function GetData(self, asset: ShaderAsset | FragmentAsset, name: string): number?
end

declare class Camera
	CFrame: CFrame
	FOV: number
	Near: number
	Far: number
end

export type Renderable_API = {
	BasePart: (shape: ("Cube" | "Sphere")?) -> BasePart,
	BaseModel: (asset: ModelAsset) -> BasePart,
	Camera: Camera,
}

declare class CFrame
	Position: Vector
	Rotation: Vector
	function Lerp(self, other: CFrame, t: number): CFrame
	
	
	function __mul(self, other: CFrame | Vector): CFrame
	function __add(self, other: CFrame): CFrame
	function __sub(self, other: CFrame): CFrame
end

export type Dim_API = { new: (x: number, y: number) -> Dim }
export type Color3_API = {
	new: (r: number, g: number, b: number) -> Color3,
	fromHex: (hex: string) -> Color3,
}
export type Vector_API = {
	new: ((x: number?, y: number?, z: number?) -> Vector),
	zero: () -> Vector,
	one: () -> Vector,
}
export type CFrame_API = {
	new: (position: Vector?, rotation: Vector?) -> CFrame,
	Angles: (rx: number, ry: number, rz: number) -> CFrame,
}

export type Primitives_API = {
	Dim: Dim_API,
	Color3: Color3_API,
	Vector: Vector_API,
	CFrame: CFrame_API,
}

declare class Primitive
	Shape: "Square" | "Circle" | "Triangle" | "Image" | "Text"
	Size: Dim
	Position: Dim
	Color: Color3
	Transparency: number
	ZIndex: number
	Visible: boolean
	-- Fires with the property name that was set ("Position", "Size", "Text", etc.).
	Changed: Signal<string>
	
	Text: string
	TextSize: number
	TextColor: Color3
	function Destroy(self): ()
	function AttachShader(self, asset: ShaderAsset | FragmentAsset): ()
	function DetachShader(self, asset: ShaderAsset | FragmentAsset): ()
	function ClearShaders(self): ()
	function SetData(self, asset: ShaderAsset | FragmentAsset, name: string, value: number): ()
	function GetData(self, asset: ShaderAsset | FragmentAsset, name: string): number?
end

declare class SceneShader
	function SetData(self, name: string, value: number): ()
	function GetData(self, name: string): number?
	function Destroy(self): ()
end

export type GUI_API = {
	Basic: {
		Circle: () -> Primitive,
		Square: () -> Primitive,
		Triangle: () -> Primitive,
		
		
		Image: (asset: ImageAsset) -> Primitive,
		
		
		Font: (asset: FontAsset) -> Primitive,
	},
	
	
	SetSkybox: (asset: ShaderAsset | FragmentAsset) -> SceneShader,
	ClearSkybox: () -> (),
	SetPostEffect: (asset: ShaderAsset | FragmentAsset) -> SceneShader,
	ClearPostEffect: () -> (),
}

export type MouseButton = "MouseButton1" | "MouseButton2" | "MouseButton3" | "MouseButton4" | "MouseButton5"
export type CursorName = "default" | "pointer" | "text" | "crosshair" | "wait" | "progress" | "help"
	| "move" | "not_allowed" | "grab" | "grabbing"
	| "resize_n" | "resize_s" | "resize_e" | "resize_w" | "resize_ne" | "resize_nw" | "resize_se" | "resize_sw"
	| "resize_ns" | "resize_ew" | "resize_nesw" | "resize_nwse"
	| "context_menu" | "copy" | "alias" | "no_drop" | "all_scroll" | "zoom_in" | "zoom_out"
	| "vertical_text" | "cell"

export type Mouse_API = {
	Position: Dim,
	Visible: boolean,
	Locked: boolean,
	Cursor: CursorName,
	-- Fires with (position: Dim, delta: Dim).
	Moved: Signal<Dim, Dim>,
	-- Fires with (button: MouseButton, pressed: boolean).
	InputReceived: Signal<MouseButton, boolean>,
	-- Fires with (deltaX: number, deltaY: number) in lines (one notch of a
	-- standard mouse wheel = ±1). +deltaY is scroll up / away from the user;
	-- +deltaX is scroll right. Precise touchpads emit fractional values.
	Scrolled: Signal<number, number>,
	SetCursor: (self: Mouse_API, name: CursorName) -> (),
	Lock: (self: Mouse_API) -> (),
	Unlock: (self: Mouse_API) -> (),
	IsButtonDown: (self: Mouse_API, button: MouseButton) -> boolean,
}

export type Keyboard_API = {
	-- Fires with (id: number, name: string, pressed: boolean). `name` is the
	-- key name like "w", "Escape", "Space", "F1", "ArrowUp", etc.
	InputChanged: Signal<number, string, boolean>,
	IsKeyDown: (self: Keyboard_API, key: string | number) -> boolean,
	GetKeyId: (self: Keyboard_API, name: string) -> number,
}

export type RunService_API = {
	-- Per-frame signal carrying the dt (seconds since last tick).
	Heartbeat: Signal<number>,
	RenderStepped: Signal<number>,
}

declare class VoiceShader end

declare class VoiceCapture
	-- Fires with each Opus-encoded audio packet (~20ms = 50 packets/sec).
	-- Pass `bytes` to peers via Steam.Lobby chat / P2P / our Net library /
	-- whatever transport you're using.
	OnPacket: Signal<string>
	function Stop(self): ()
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
	IsPlaying: boolean
	-- Feed an Opus packet (received from a peer). Decoded and queued for
	-- playback. Auto-clamps backlog to ~1 second to prevent runaway latency.
	function Push(self, packet: string): ()
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
	function Push(self, packet: string): ()
	function Clear(self): ()
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

declare class SteamLobby
	ID: string
	Owner: string
	-- Fires with (memberSteamId: string).
	OnMemberJoined: Signal<string>
	-- Fires with (memberSteamId: string).
	OnMemberLeft: Signal<string>
	-- Fires with (memberSteamId: string?). Nil when lobby-wide data changed,
	-- otherwise the id of the member whose per-member data changed.
	OnDataUpdate: Signal<string?>
	function Members(self): { string }
	function SetData(self, key: string, value: string): ()
	function GetData(self, key: string): string?
	function SendChat(self, message: string): ()
	function Leave(self): ()
end

declare class SteamServer
	ID: string
	-- Fires with (clientSteamId: string) when a client successfully authenticates.
	OnClientAuthed: Signal<string>
	function SetServerName(self, name: string): ()
	function SetMapName(self, map: string): ()
	function SetMaxPlayers(self, max: number): ()
	function LogOnAnonymous(self): ()
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
	port: number?,
	queryPort: number?,
	mode: SteamServerMode?,
	version: string?,
	name: string?,
	map: string?,
	max: number?,
}

export type SteamCloudFile = { Name: string, Size: number }
export type SteamFriendInfo = {
	ID: string, Name: string, Nickname: string?, State: SteamPersonaState,
	Game: { AppID: number }?,
}

export type Steam_API = {
	User: {
		ID: string,
		Name: string,
		Level: number,
		AccountID: () -> number,
		LoggedOn: () -> boolean,
		GetFriends: () -> { SteamFriend },
		GetAvatar: (size: SteamAvatarSize?) -> ImageAsset?,
		GetFriendAvatar: (id: string, size: SteamAvatarSize?) -> ImageAsset?,
		-- Async lookup that works for ANY Steam ID, not just friends. Hits
		-- cache instantly when the avatar is already known; otherwise asks
		-- Steam for the user's profile data and fires the signal when it
		-- arrives (a frame or two later). Signal fires once with
		-- (ImageAsset?) — nil if Steam couldn't load the user.
		GetAvatarAsync: (id: string, size: SteamAvatarSize?) -> Signal<ImageAsset?>,
		GetFriendInfo: (id: string) -> SteamFriendInfo,
		RequestInfo: (id: string, nameOnly: boolean?) -> boolean,
		InviteToGame: (id: string, connectString: string) -> (),
	},
	Achievements: {
		Unlock: (name: string) -> (),
		Clear: (name: string) -> (),
		IsUnlocked: (name: string) -> boolean,
		Store: () -> (),
	},
	Stats: {
		SetInt: (name: string, value: number) -> (),
		SetFloat: (name: string, value: number) -> (),
		GetInt: (name: string) -> number,
		GetFloat: (name: string) -> number,
		Store: () -> (),
	},
	Lobby: {
		-- Returns a Signal that fires once with (lobby: SteamLobby?, err: string?).
		Create: (maxMembers: number?, kind: SteamLobbyKind?) -> Signal<SteamLobby?, string?>,
		-- Returns a Signal that fires once with (lobby: SteamLobby?, err: string?).
		Join: (id: string) -> Signal<SteamLobby?, string?>,
		-- Returns a Signal that fires once with the matching lobbies.
		List: () -> Signal<{ SteamLobbyInfo }>,
	},
	Server: {
		Start: (opts: SteamServerOptions) -> SteamServer,
	},
	Overlay: {
		Show: (dialog: SteamOverlayDialog) -> (),
		ShowFriends: () -> (),
		ShowAchievements: () -> (),
		ShowSettings: () -> (),
		ShowURL: (url: string) -> (),
		ShowStore: (appId: number?, mode: SteamOverlayStoreMode?) -> (),
		ShowUser: (id: string, dialog: SteamOverlayUserDialog?) -> (),
		ShowInvite: (lobbyId: string) -> (),
	},
	App: {
		IsAppInstalled: (appId: number) -> boolean,
		IsDlcInstalled: (appId: number) -> boolean,
		IsSubscribed: () -> boolean,
		IsVacBanned: () -> boolean,
		BuildId: () -> number,
		InstallDir: (appId: number) -> string,
		OwnerID: () -> string,
		CurrentLanguage: () -> string,
		AvailableLanguages: () -> { string },
		BetaName: () -> string?,
		LaunchCommandLine: () -> string,
	},
	Utils: {
		UILanguage: () -> string,
		IpCountry: () -> string,
		ServerTime: () -> number,
		IsSteamDeck: () -> boolean,
		AppID: () -> number,
		SetOverlayPosition: (position: SteamNotificationPosition) -> (),
	},
	RichPresence: {
		Set: (key: string, value: string?) -> boolean,
		Clear: () -> (),
	},
	Cloud: {
		IsEnabledForAccount: () -> boolean,
		IsEnabledForApp: () -> boolean,
		SetEnabledForApp: (enabled: boolean) -> (),
		Files: () -> { SteamCloudFile },
		Exists: (name: string) -> boolean,
		Read: (name: string) -> string?,
		Write: (name: string, data: string) -> (),
		Delete: (name: string) -> boolean,
	},
	Workshop: {
		SubscribedItems: () -> { string },
		ItemState: (id: string) -> {
			Subscribed: boolean, Installed: boolean, NeedsUpdate: boolean,
			Downloading: boolean, DownloadPending: boolean,
		},
		InstallInfo: (id: string) -> { Folder: string, SizeOnDisk: number, Timestamp: number }?,
		DownloadProgress: (id: string) -> { Downloaded: number, Total: number }?,
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
	RemotePlay: {
		Sessions: () -> { { UserID: string, ClientName: string?, Width: number?, Height: number? } },
	},
}


declare class Actor
	-- Hand args off to a worker thread. The args are deep-copied across the
	-- thread boundary, so mutating them after Push has no effect on the worker.
	-- Allowed types: nil, boolean, number, string, table (recursively, with
	-- only those types as keys/values). Functions, userdata, threads, etc.
	-- are rejected — workers can't share Luau state with the main thread.
	function Push(self, ...: any): ()
	-- Non-blocking. Returns the next ready result's values if a worker has
	-- finished, or no values if nothing is ready yet. Order is "first to
	-- finish, first out" (NOT Push order — work runs in parallel).
	-- Re-raises any error a worker hit while running its function.
	function Pop(self): ...any
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
	-- Spawn a worker pool that runs the given Luau chunk in parallel on
	-- other CPU cores. The chunk is plain Luau source (compiled once on the
	-- main thread, bytecode shipped to every worker) and MUST evaluate to
	-- a function — that function is what each Push invokes.
	--
	-- Workers run in completely isolated Luau states: they get math, table,
	-- string, bit32, buffer, utf8, coroutine, and the basic library — but
	-- NO `import`, `require`, `print`, `loadstring`, `dofile`, or
	-- `__dirname`. Workers cannot touch the main thread's globals, GUI,
	-- assets, sockets, or anything else import-shaped. Pass everything they
	-- need as arguments to Push.
	--
	-- threads defaults to the number of CPU cores. Pass an explicit count
	-- for fine control (e.g. 1 for serial, 2 for I/O-bound work, etc.).
	--
	-- Example:
	--   local fib = Actor.new([[
	--       return function(n)
	--           if n < 2 then return n end
	--           local a, b = 0, 1
	--           for _ = 2, n do a, b = b, a + b end
	--           return b
	--       end
	--   ]])
	--   for i = 1, 32 do fib:Push(i) end
	--   while fib:Pending() > 0 do
	--       local v = fib:Pop()
	--       if v ~= nil then print(v) end
	--   end
	new: (source: string, threads: number?) -> Actor,
}

export type Imports = {
	Actor: Actor_API,
	Asset: Asset_API,
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


declare import: <K>(name: keyof<Imports> & K) -> index<Imports, K>

declare __dirname: string
"#;
