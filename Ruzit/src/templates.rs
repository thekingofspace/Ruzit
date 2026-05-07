pub const BUILD_TOML: &str = r#"Name = "{name}"
Version = "0.1.0"
Creator = ""

[configs]
"File Type" = "Relative"

[exe]
# name = "{name}"     # output exe name when running `Ruzit Build` (defaults to entry stem)
# icon = "logo"       # looks for <icon>.ico next to build.toml and embeds it as the exe icon
# windowed = false    # true → no console flash from Explorer (needs --console for prints).
                      #         Default leaves prints visible from cmd or Explorer.
"#;

pub const MAIN_LUAU: &str = r#"--!strict

local IO = import("IO")

print("Hello from {name}!")
print("project root =", __dirname)
print("IO available:", typeof(IO) == "table")
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

type Asset_API = {
	GetAsset: ((kind: "Image", path: string) -> ImageAsset)
		& ((kind: "Sound", path: string) -> SoundData)
		& ((kind: "Shader", path: string) -> ShaderAsset)
		& ((kind: "Fragment", path: string) -> FragmentAsset),
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

declare import: ((name: "Asset") -> Asset_API)
	& ((name: "IO") -> IO_API)
	& ((name: "Managed") -> Managed_API)
	& ((name: "Net") -> Net_API)
	& ((name: "Process") -> Process_API)
	& ((name: "Serde") -> Serde_API)
	& ((name: "SFX") -> SFX_API)
	& ((name: "Signal") -> Signal_API)
	& ((name: "Window") -> Window_API)

declare __dirname: string
"#;
