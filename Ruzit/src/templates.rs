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

pub const TYPES_DLUAU: &str = r#"--!strict
-- Ruzit type declarations
-- The Luau LSP picks this file up via .vscode/settings.json.

declare class IOHandle
	function read(self, format: string?): string
	function write(self, content: string): ()
	function close(self): ()
	function path(self): string
end

type IO_API = {
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

type HttpRequest = { method: string, path: string, headers: { [string]: string }, body: string }
type HttpResponse = { status: number, headers: { [string]: string }, body: string }

type Net_API = {
	Serve: (addr: string, handler: (HttpRequest) -> (HttpResponse | string)) -> (),
	Request: (method: string, url: string, body: string?, headers: { [string]: string }?) -> HttpResponse,
	TCP: { Connect: (addr: string) -> TcpConnection, Host: (addr: string) -> TcpListener },
	UDP: { Bind: (addr: string) -> UdpHandle },
	IPC: { Connect: (name: string) -> IpcConnection, Host: (name: string) -> IpcListener },
	Socket: { Connect: (url: string) -> WebSocketConn, Host: (addr: string) -> WebSocketListener },
}

type HashAlgo = "md5" | "sha1" | "sha224" | "sha256" | "sha384" | "sha512" | "sha3-256" | "sha3-512" | "keccak256" | "blake3"
type CompressAlgo = "gzip" | "zlib" | "deflate" | "zstd"

type Serde_API = {
	Encode: (format: ("json" | "toml" | "yaml"), data: any, pretty: boolean?) -> string,
	Decode: (format: ("json" | "toml" | "yaml"), text: string) -> any,
	Hash: (algo: HashAlgo, data: string, encoding: ("hex" | "base64" | "bytes")?) -> string,
	Compress: (algo: CompressAlgo, data: string, level: number?) -> string,
	Decompress: (algo: CompressAlgo, data: string) -> string,
}

type Process_API = {
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

-- Opaque shader handles. Loaded from disk/bundle and attached to host objects
-- (sounds, meshes, UI). Their data is exchanged through the host's :SetData.
declare class ShaderAsset end
declare class FragmentAsset end

declare class FontAsset
	function Source(self): string
end

type Asset_API = {
	GetAsset: ((kind: "Image", path: string) -> ImageAsset)
		& ((kind: "Sound", path: string) -> SoundData)
		& ((kind: "Shader", path: string) -> ShaderAsset)
		& ((kind: "Fragment", path: string) -> FragmentAsset)
		& ((kind: "Model", path: string) -> ModelAsset)
		& ((kind: "Font", path: string) -> FontAsset)
		& ((kind: "File", path: string) -> string),
}

type WindowOptions = {
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

declare class Signal
	function Connect(self, fn: (...any) -> ()): Connection
	function Once(self, fn: (...any) -> ()): Connection
	function Wait(self): ...any
	function Fire(self, ...: any): ()
	function DisconnectAll(self): ()
end

type Signal_API = { new: () -> Signal }

declare class WindowHandle
	Changed: Signal
	OnFocus: Signal
	OnUnfocus: Signal
	function Close(self): ()
	function BindToClose(self, fn: () -> ()): ()
	function Resize(self, width: number, height: number): ()
	function SetTitle(self, title: string): ()
	function SetBorderless(self, borderless: boolean): ()
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

type Window_API = { Open: (opts: WindowOptions?) -> WindowHandle }

-- Built-in transformations applied via :ApplyShader. Distinct from ShaderAsset,
-- which is loaded from disk and uses :AttachShader / :SetData.
declare class SoundShader end

declare class Sound
	Started: Signal
	Stopped: Signal
	Source: string
	function Play(self): ()
	function Stop(self): ()
	function IsPlaying(self): boolean
	function ApplyShader(self, shader: SoundShader): ()
	function ClearShaders(self): ()
	function AttachShader(self, asset: ShaderAsset | FragmentAsset): ()
	function DetachShader(self, asset: ShaderAsset | FragmentAsset): ()
	function SetData(self, asset: ShaderAsset | FragmentAsset, name: string, value: number): ()
	function GetData(self, asset: ShaderAsset | FragmentAsset, name: string): number?
	function LinkToUpdate(self, interval: number): Signal
end

type SFX_API = {
	LoadSound: (data: SoundData) -> Sound,
	-- Built-in transformations (applied via :ApplyShader, snapshot at Play).
	Volume: (factor: number) -> SoundShader,
	Speed: (factor: number) -> SoundShader,
	FadeIn: (seconds: number) -> SoundShader,
	LowPass: (freq: number) -> SoundShader,
	Delay: (seconds: number) -> SoundShader,
	Repeat: () -> SoundShader,
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

type Managed_API = {
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
	Shape: string
	CFrame: CFrame
	Size: Vector
	Color: Color3
	Render: boolean
	Texture: ImageAsset?
	Changed: Signal
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

type Renderable_API = {
	BasePart: (shape: string?) -> BasePart,
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

type Dim_API = { new: (x: number, y: number) -> Dim }
type Color3_API = {
	new: (r: number, g: number, b: number) -> Color3,
	fromHex: (hex: string) -> Color3,
}
type Vector_API = {
	new: ((x: number?, y: number?, z: number?) -> Vector),
	zero: () -> Vector,
	one: () -> Vector,
}
type CFrame_API = {
	new: (position: Vector?, rotation: Vector?) -> CFrame,
	Angles: (rx: number, ry: number, rz: number) -> CFrame,
}

type Primitives_API = {
	Dim: Dim_API,
	Color3: Color3_API,
	Vector: Vector_API,
	CFrame: CFrame_API,
}

declare class Primitive
	Shape: string
	Size: Dim
	Position: Dim
	Color: Color3
	Transparency: number
	ZIndex: number
	Visible: boolean
	Changed: Signal
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

type GUI_API = {
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

type Mouse_API = {
	Position: Dim,
	Visible: boolean,
	Locked: boolean,
	Cursor: string,
	Moved: Signal,
	InputReceived: Signal,
	SetCursor: (self: Mouse_API, name: string) -> (),
	Lock: (self: Mouse_API) -> (),
	Unlock: (self: Mouse_API) -> (),
	IsButtonDown: (self: Mouse_API, button: string) -> boolean,
}

type Keyboard_API = {
	InputChanged: Signal,
	IsKeyDown: (self: Keyboard_API, key: string | number) -> boolean,
	GetKeyId: (self: Keyboard_API, name: string) -> number,
}

type RunService_API = {
	Heartbeat: Signal,
	RenderStepped: Signal,
}

-- One row per importable lib; the solver indexes into this rather than
-- trying to pick a winner from a 13-way function intersection (which blew
-- the type-complexity limit and made `import(...)` unresolvable).
type Imports = {
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
	Window: Window_API,
}

-- `keyof<Imports> & K` constrains the literal arg to a key, `index<Imports, K>`
-- looks up the matching row's type. Requires the new Luau type solver
-- (luau-lsp >= 1.50). If your LSP is older, replace with `(name: string) -> any`.
declare import: <K>(name: keyof<Imports> & K) -> index<Imports, K>

declare __dirname: string
"#;
