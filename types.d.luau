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
	Home: string?,
	Documents: string?,
	Desktop: string?,
	Downloads: string?,
	Pictures: string?,
	Videos: string?,
	Music: string?,
	AppData: string?,
	LocalAppData: string?,
	Cache: string?,
	Temp: string,
	WorkingDir: string,
	ExecutableDir: string,
	Separator: string,
	Platform: ("windows" | "linux" | "macos" | "unknown"),
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
	InTest: boolean,
	Heartbeat: Signal<number>,
	RenderStepped: Signal<number>,
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
	function Free(self): ()
end
declare class SoundData
	function Source(self): string
	function ByteCount(self): number
	function Free(self): ()
end
declare class ShaderAsset
	function Source(self): string
	function Free(self): ()
end
declare class FragmentAsset
	function Source(self): string
	function Free(self): ()
end
declare class FontAsset
	function Source(self): string
	function Free(self): ()
end
export type AssetByKind = {
	Image: ImageAsset,
	Sound: SoundData,
	Shader: ShaderAsset,
	Fragment: FragmentAsset,
	Model: ModelAsset,
	AnimationSet: AnimationSetAsset,
	Font: FontAsset,
	File: string,
}
export type AssetKind = keyof<AssetByKind>
export type Asset_API = {
	GetAsset: <K>(kind: AssetKind & K, path: string) -> index<AssetByKind, K>,
	FromString: <K>(kind: AssetKind & K, data: string, label: string?) -> index<AssetByKind, K>,
	ImportAsset: (<K>(kind: AssetKind & K, path: string) -> index<AssetByKind, K>)
		& ((kind: "Auto", path: string) -> any),
	FromPixels: (width: number, height: number, rgba: string) -> ImageAsset,
	PreloadAsync: (entries: { string }) -> Signal<>,
	BulkLoad: (entries: { string }) -> { [string]: any },
	Drop: (entry: string) -> (),
	GetVideo: (path: string) -> (Video, SoundData?),
}
export type WindowOptions = {
	title: string?,
	width: number?, height: number?,
	min_width: number?, min_height: number?, max_width: number?, max_height: number?,
	fullscreen: boolean?,
	borderless: boolean?,
	resizable: boolean?,
	maximized: boolean?,
	visible: boolean?,
	transparent: boolean?,
	always_on_top: boolean?,
	icon: ImageAsset?,
}
declare class Connection
	Connected: boolean
	function Disconnect(self): ()
end
export type Signal<T...> = {
	Connect: (self: Signal<T...>, fn: (T...) -> ()) -> Connection,
	Once: (self: Signal<T...>, fn: (T...) -> ()) -> Connection,
	Wait: (self: Signal<T...>) -> T...,
	Fire: (self: Signal<T...>, T...) -> (),
	DisconnectAll: (self: Signal<T...>) -> (),
}
export type Signal_API = { new: <T...>() -> Signal<T...> }
declare class WindowHandle
	Changed: Signal<string, ...any>
	OnFocus: Signal<>
	OnUnfocus: Signal<>
	SizeChanged: Signal<number, number>
	PositionChanged: Signal<number, number>
	ScaleFactorChanged: Signal<number>
	FocusChanged: Signal<boolean>
	TitleChanged: Signal<string>
	FullscreenChanged: Signal<boolean>
	BorderlessChanged: Signal<boolean>
	DecorationsChanged: Signal<boolean>
	ResizableChanged: Signal<boolean>
	MaximizedChanged: Signal<boolean>
	MinimizedChanged: Signal<boolean>
	VisibleChanged: Signal<boolean>
	AlwaysOnTopChanged: Signal<boolean>
	function Close(self): ()
	function BindToClose(self, fn: () -> ()): ()
	function Resize(self, width: number, height: number): ()
	function SetTitle(self, title: string): ()
	function SetBorderless(self, borderless: boolean): ()
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
export type Window_API = {
	Open: (opts: WindowOptions?) -> WindowHandle,
}
declare class SoundShader end
declare class Sound
	Started: Signal<>
	Stopped: Signal<>
	DidLoop: Signal<>
	Source: string
	Looped: boolean
	TimePosition: number
	Volume: number
	Speed: number
	Pitch: number
	Pan: number
	LowPass: number?
	HighPass: number?
	FadeIn: number
	FadeOut: number
	Delay: number
	Loop: boolean
	Distortion: number
	Echo: { Delay: number, Feedback: number?, Mix: number? }?
	Reverb: { Mix: number?, Decay: number? }?
	Tremolo: { Rate: number?, Depth: number? }?
	Position: Vector?
	MinFalloff: number
	MaxFalloff: number
	function Play(self): ()
	function Stop(self): ()
	function IsPlaying(self): boolean
	function Reset(self): ()
	function SetPosition(self, x: number, y: number, z: number, max_falloff: number?): ()
	function ClearPosition(self): ()
	function ApplyShader(self, shader: SoundShader): ()
	function ClearShaders(self): ()
	function AttachShader(self, asset: ShaderAsset | FragmentAsset): ()
	function DetachShader(self, asset: ShaderAsset | FragmentAsset): ()
	function SetData(self, asset: ShaderAsset | FragmentAsset, name: string, value: number): ()
	function GetData(self, asset: ShaderAsset | FragmentAsset, name: string): number?
	function LinkToUpdate(self, interval: number): Signal<number>
end
export type SFX_API = {
	LoadSound: (data: SoundData) -> Sound,
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
type ModifierKind =
	"Volume"
	| "Pan"
	| "Distortion"
	| "Tremolo"
	| "Vibrato"
	| "BitCrusher"
	| "NoiseGate"
	| "Compressor"
	| "Limiter"
	| "Saturation"
	| "RingMod"
	| "Chorus"
	| "Flanger"
	| "StereoWiden"
	| "Wobble"
	| "Telephone"
	| "Underwater"
	| "Mute"
	| "Speed"
	| "Pitch"
	| "PlaybackSpeed"
	| "LowPass"
	| "HighPass"
	| "BandPass"
	| "Echo"
	| "Reverb"
	| "FadeIn"
	| "FadeOut"
	| "Delay"
declare class SoundByteModifier
	Kind: string
	Enabled: boolean
	Min: number
	Max: number
	Value: number
end
declare class SoundBytePlayer
	Source: string
	Started: Signal<>
	Stopped: Signal<>
	DidLoop: Signal<>
	IsPlaying: boolean
	Looped: boolean
	function Play(self): ()
	function Stop(self): ()
end
declare class SoundByteOutput
	Volume: number
	FalloffMinDistance: number
	FalloffMaxDistance: number
	CFrame: CFrame?
	Position: Vector
	function Follow(self, part: BasePart): ()
	function Unfollow(self): ()
	function ListSelectableOutputs(self): { string }
	function GetDefaultOutput(self): string?
	function GetCurrentOutput(self): string?
	function SetOutput(self, name: string?): ()
end
declare class SoundByteByte
	OnPacket: Signal<string>
	SampleRate: number
	Channels: number
	QueueLength: number
	function Drain(self): number
end
declare class SoundByteVoiceChannel
	Enabled: boolean
	Threshold: number
	function ListSelectableInputs(self): { string }
	function GetDefaultInput(self): string?
	function GetCurrentInput(self): string?
	function SetInput(self, name: string?): ()
end
declare class SoundByteByteSource
	Enabled: boolean
	SampleRate: number
	Channels: number
	QueueLength: number
	BufferSeconds: number
	function SendInput(self, packet: string): ()
	function SendSamples(self, samples: { number }): ()
	function Clear(self): ()
	function Stop(self): ()
	function Destroy(self): ()
end
declare class SoundByteLink
	function Unlink(self): ()
end
export type SoundByte_API = {
	VoiceFlagEnabled: boolean,
	NewPlayer: (data: SoundData) -> SoundBytePlayer,
	NewOutput: () -> SoundByteOutput,
	NewByte: () -> SoundByteByte,
	NewByteSource: (sampleRate: number?, channels: number?) -> SoundByteByteSource,
	GetModifier: (kind: ModifierKind) -> SoundByteModifier,
	GetVoiceChannel: () -> SoundByteVoiceChannel,
	Link: (...any) -> SoundByteLink,
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
	function Scale(self, reference: Dim, target: Dim, maintainRatio: boolean?): Dim
	function __add(self, other: Dim): Dim
	function __sub(self, other: Dim): Dim
	function __mul(self, other: number): Dim
	function __div(self, other: number): Dim
	function __unm(self): Dim
	function __eq(self, other: Dim): boolean
	function __lt(self, other: Dim): boolean
	function __le(self, other: Dim): boolean
end
declare class Color3
	R: number
	G: number
	B: number
	function Lerp(self, other: Color3, t: number): Color3
	function __add(self, other: Color3): Color3
	function __sub(self, other: Color3): Color3
	function __eq(self, other: Color3): boolean
	function __lt(self, other: Color3): boolean
	function __le(self, other: Color3): boolean
end
declare class Vector
	X: number
	Y: number
	Z: number
	Magnitude: number
	function Lerp(self, other: Vector, t: number): Vector
	function Dot(self, other: Vector): number
	function Cross(self, other: Vector): Vector
	function Normalized(self): Vector
	function Distance(self, other: Vector): number
	function __add(self, other: Vector): Vector
	function __sub(self, other: Vector): Vector
	function __mul(self, other: number): Vector
	function __div(self, other: number): Vector
	function __unm(self): Vector
	function __eq(self, other: Vector): boolean
	function __lt(self, other: Vector): boolean
	function __le(self, other: Vector): boolean
end
declare class ModelAsset
	function VertexCount(self): number
	function TriangleCount(self): number
	function Source(self): string
	function LocalBounds(self): { Min: Vector, Max: Vector, Size: Vector, Center: Vector }
	function Free(self): ()
end
declare class AnimationSetAsset
	function Count(self): number
	function ListAnimations(self): { string }
	function GetAnimation(self, name: string): Animation
	function Source(self): string
	function Free(self): ()
end
declare class Animation
	function Name(self): string
	function Duration(self): number
	function Source(self): string
	function KeyframeCount(self): number
end
declare class CFrame
	Position: Vector
	Rotation: Vector
	function Lerp(self, other: CFrame, t: number): CFrame
	function __mul(self, other: CFrame | Vector): CFrame
	function __add(self, other: CFrame): CFrame
	function __sub(self, other: CFrame): CFrame
	function __eq(self, other: CFrame): boolean
	function FuzzyEq(self, other: CFrame, epsilon: number?): boolean
end
declare class UDim
	X: number
	Y: number
	function ToDim(self, parent: Dim): Dim
	function Lerp(self, other: UDim, t: number): UDim
	function __add(self, other: UDim): UDim
	function __sub(self, other: UDim): UDim
	function __mul(self, other: number): UDim
	function __div(self, other: number): UDim
	function __unm(self): UDim
	function __eq(self, other: UDim): boolean
	function __lt(self, other: UDim): boolean
	function __le(self, other: UDim): boolean
end
export type Dim_API = {
	new: (x: number, y: number) -> Dim,
}
export type Color3_API = {
	new: (r: number, g: number, b: number) -> Color3,
	FromRGB: (r: number, g: number, b: number) -> Color3,
	fromRGB: (r: number, g: number, b: number) -> Color3,
	FromHex: (hex: string) -> Color3,
	fromHex: (hex: string) -> Color3,
}
export type Vector_API = {
	new: ((x: number?, y: number?, z: number?) -> Vector),
	zero: () -> Vector,
	one: () -> Vector,
	FromNative: (native: vector) -> Vector,
}
export type CFrame_API = {
	new: (position: Vector?, rotation: Vector?) -> CFrame,
	Angles: (rx: number, ry: number, rz: number) -> CFrame,
	LookAt: (eye: Vector, target: Vector, up: Vector?) -> CFrame,
}
export type UDim_API = {
	new: ((x: number?, y: number?) -> UDim),
	zero: () -> UDim,
	one: () -> UDim,
	FromDim: (scaleX: number, scaleY: number, parent: Dim) -> Dim,
}
export type Primitives_API = {
	Dim: Dim_API,
	Color3: Color3_API,
	Vector: Vector_API,
	CFrame: CFrame_API,
	UDim: UDim_API,
}
export type NoiseLayerParams = {
	Type: ("value" | "perlin" | "gradient" | "fbm" | "ridged" | "white")?,
	Mode: ("add" | "set" | "replace" | "mul" | "multiply" | "max" | "min")?,
	Scale: number?,
	Amplitude: number?,
	Octaves: number?,
	Lacunarity: number?,
	Persistence: number?,
	Seed: number?,
	OffsetX: number?,
	OffsetY: number?,
}
declare class NoiseField
	function Width(self): number
	function Height(self): number
	function Clear(self, fill: number?): ()
	function Fill(self, value: number): ()
	function GetValueAt(self, x: number, y: number): number
	function SetValueAt(self, x: number, y: number, value: number): ()
	function GetColorAt(self, x: number, y: number): Color3
	function Normalize(self): ()
	function ToBytes(self): string
	function GenerateLayer(self, params: NoiseLayerParams): ()
end
export type Image_API = {
	GetColorAt: (image: ImageAsset, x: number, y: number) -> Color3,
	GetAlphaAt: (image: ImageAsset, x: number, y: number) -> number,
	GetPredominantColor: (image: ImageAsset) -> Color3,
	AverageColor: (image: ImageAsset) -> Color3,
	ToSeed: (image: ImageAsset) -> number,
	Noise: (width: number, height: number, fill: number?) -> NoiseField,
}
declare class BasePart
	Shape: "Cube" | "Sphere" | "Model"
	CFrame: CFrame
	Size: Vector
	Color: Color3
	Render: boolean
	Texture: (ImageAsset | DynImg | DrawableImg)?
	CastShadow: boolean
	ReceiveShadow: boolean
	IgnoreInRaycast: boolean
	Lit: boolean
	Changed: Signal<string>
	function GetPropertyChanged(self, propName: string): Signal<any>
	function Destroy(self): ()
	function AttachShader(self, asset: ShaderAsset | FragmentAsset): ()
	function DetachShader(self, asset: ShaderAsset | FragmentAsset): ()
	function ClearShaders(self): ()
	function SetData(self, asset: ShaderAsset | FragmentAsset, name: string, value: number): ()
	function GetData(self, asset: ShaderAsset | FragmentAsset, name: string): number?
	function Deform(self, target: CFrame, envelope: number, displacement: Vector): number
	function ResetDeformation(self): ()
	function LocalToWorld(self, local_offset: Vector): Vector
	function WorldToLocal(self, world: Vector): Vector
	function GetAnimatedTrack(self, animation: Animation): AnimationTrack
	function GetTracks(self): { AnimationTrack }
	function GetTrack(self, name: string): AnimationTrack
	function LinkInput(self, audio: Sound | VoiceChannel): ()
	function UnlinkInput(self, audio: Sound | VoiceChannel): ()
end
declare class AnimationTrack
	Name: string
	FadeTime: number
	Looped: boolean
	Speed: number
	Priority: number
	Length: number
	TimePosition: number
	IsPlaying: boolean
	IsPaused: boolean
	Played: Signal<>
	Stopped: Signal<>
	DidLoop: Signal<>
	function Play(self): ()
	function Pause(self): ()
	function Resume(self): ()
	function Stop(self): ()
	function Reset(self): ()
	function AddKeyframe(self, time: number, kind: "CFrame" | "Deform", ...: any): ()
	function ClearKeyframes(self): ()
	function LinkToUpdate(self, interval: number): Signal<number>
end
declare class Camera
	CFrame: CFrame
	FOV: number
	Near: number
	Far: number
	IsLinked: boolean
	function Follow(self, target: BasePart | PhysicsObject | Controller): ()
	function LinkTo(
		self,
		target: BasePart | PhysicsObject | Controller,
		opts: ({ Offset: CFrame?, Callback: ((dt: number, baseCFrame: CFrame) -> CFrame?)? } | CFrame | ((dt: number, baseCFrame: CFrame) -> CFrame?))?
	): ()
	function Unlink(self): ()
	function SetOffset(self, offset: CFrame): ()
	function SetLinkCallback(self, fn: ((dt: number, baseCFrame: CFrame) -> CFrame?)?): ()
end
export type Renderable_API = {
	BasePart: (shape: ("Cube" | "Sphere")?) -> BasePart,
	BaseModel: (asset: ModelAsset) -> BasePart,
	SimplifyMesh: (asset: ModelAsset, ratio: number?) -> ModelAsset,
	DistortionBox: (part: BasePart, cframe: CFrame, size: Vector) -> DistortionBox,
	EffectVolume: (image: (ImageAsset | DynImg | DrawableImg)?) -> EffectVolume,
}
type NumberRange = { Min: number, Max: number }
type ColorSequenceInput = Color3 | { [number]: Color3 } | { { number | Color3 } }
type NumberSequenceInput = number | { [number]: number } | { { number } }
declare class EffectVolume
	IsAlive: boolean
	ParticleCount: number
	Enabled: boolean
	FaceCamera: boolean
	CFrame: CFrame
	Size: Vector
	Image: (ImageAsset | DynImg | DrawableImg)?
	Rate: number
	MaxParticles: number
	Lifetime: NumberRange
	Speed: NumberRange
	ParticleSize: NumberRange
	Rotation: NumberRange
	RotSpeed: NumberRange
	Acceleration: Vector
	RandomizeForce: { X: NumberRange?, Y: NumberRange?, Z: NumberRange? }
	Drag: number
	Spread: number
	EmissionDirection: Vector
	Color: ColorSequenceInput
	TimeScaleColor: ColorSequenceInput
	Transparency: NumberSequenceInput
	TimeScaleTransparency: NumberSequenceInput
	SizeOverLife: NumberSequenceInput
	function Emit(self, n: number?): ()
	function Clear(self): ()
	function Destroy(self): ()
	function AttachShader(self, asset: ShaderAsset | FragmentAsset): ()
	function DetachShader(self, asset: ShaderAsset | FragmentAsset): ()
	function ClearShaders(self): ()
	function SetData(self, asset: ShaderAsset | FragmentAsset, name: string, value: number): ()
	function GetData(self, asset: ShaderAsset | FragmentAsset, name: string): number?
end
declare class DistortionBox
	CFrame: CFrame
	Size: Vector
	InitialCFrame: CFrame
	InitialSize: Vector
	VertexCount: number
	IsAlive: boolean
	function Destroy(self): ()
end
declare class Primitive
	Shape: "Square" | "Circle" | "Triangle" | "Image" | "Text" | "Clippable"
	Size: Dim
	Position: Dim
	Rotation: number
	Color: Color3
	Transparency: number
	ZIndex: number
	Visible: boolean
	Changed: Signal<string>
	Text: string
	TextSize: number
	FontStyle: string
	Underline: boolean
	Strikethrough: boolean
	function GetPropertyChanged(self, propName: string): Signal<any>
	function ListFontStyles(self): { string }
	function Destroy(self): ()
	function AttachShader(self, asset: ShaderAsset | FragmentAsset): ()
	function DetachShader(self, asset: ShaderAsset | FragmentAsset): ()
	function ClearShaders(self): ()
	function SetData(self, asset: ShaderAsset | FragmentAsset, name: string, value: number): ()
	function GetData(self, asset: ShaderAsset | FragmentAsset, name: string): number?
	function AddClippable(self, child: Primitive): ()
	function RemoveClippable(self, child: Primitive): ()
	function SetShape(self, shape: "Square" | "Circle" | "Triangle"): ()
end
declare class SceneShader
	function SetData(self, name: string, value: number): ()
	function GetData(self, name: string): number?
	function Destroy(self): ()
end
declare class DynImg
	Width: number
	Height: number
	IsAlive: boolean
	function Add(self, primitive: Primitive): ()
	function Remove(self, primitive: Primitive): boolean
	function Destroy(self): ()
end
declare class DrawableImg
	function Width(self): number
	function Height(self): number
	function Source(self): string
	function Pixels(self): string
	function IsAlive(self): boolean
	function WritePixel(self, x: number, y: number, color: Color3, transparency: number?): ()
	function DrawRect(self, x: number, y: number, w: number, h: number, color: Color3, transparency: number?): ()
	function DrawCube(self, x: number, y: number, w: number, h: number, color: Color3, transparency: number?): ()
	function DrawLine(self, x0: number, y0: number, x1: number, y1: number, color: Color3, transparency: number?): ()
	function DrawCircle(self, cx: number, cy: number, radius: number, color: Color3, transparency: number?, filled: boolean?): ()
	function Fill(self, color: Color3, transparency: number?): ()
	function Clear(self): ()
	function Apply(self, buffer: CanvasBuffer, atX: number?, atY: number?): ()
	function Destroy(self): ()
end
declare class CanvasBuffer
	function Width(self): number
	function Height(self): number
	function Pixels(self): string
	function WritePixel(self, x: number, y: number, color: Color3, transparency: number?): ()
	function DrawRect(self, x: number, y: number, w: number, h: number, color: Color3, transparency: number?): ()
	function DrawCube(self, x: number, y: number, w: number, h: number, color: Color3, transparency: number?): ()
	function DrawLine(self, x0: number, y0: number, x1: number, y1: number, color: Color3, transparency: number?): ()
	function DrawCircle(self, cx: number, cy: number, radius: number, color: Color3, transparency: number?, filled: boolean?): ()
	function Fill(self, color: Color3, transparency: number?): ()
	function Clear(self): ()
	function Apply(self, target: DrawableImg, atX: number?, atY: number?): ()
end
export type DynImg_API = {
	New: (width: number, height: number, primitives: { Primitive }?) -> DynImg,
}
export type AnimatedImageTrack = {
	FPS: number,
	Looped: boolean,
	Priority: number,
	FrameCount: number,
	State: string,
	Elapsed: number,
	DidLoop: Signal<>,
	Ended: Signal<>,
	Play: (self: AnimatedImageTrack) -> (),
	Stop: (self: AnimatedImageTrack) -> (),
	Pause: (self: AnimatedImageTrack) -> (),
	Resume: (self: AnimatedImageTrack) -> (),
	Wait: (self: AnimatedImageTrack) -> (),
}
export type AnimatedImage = {
	Size: Dim,
	Position: Dim,
	Rotation: number,
	ZIndex: number,
	Visible: boolean,
	Color: Color3,
	Transparency: number,
	FrameCount: number,
	Alive: boolean,
	CreateTrack: (
		self: AnimatedImage,
		frames: { string } | string,
		opts: { FPS: number?, Looped: boolean?, Priority: number? }?
	) -> AnimatedImageTrack,
	JumpToFrame: (self: AnimatedImage, frame: string | number) -> (),
	ClearJump: (self: AnimatedImage) -> (),
	HasFrame: (self: AnimatedImage, name: string) -> boolean,
	GetFrameNames: (self: AnimatedImage) -> { string },
	GetPrimitive: (self: AnimatedImage) -> any,
	Destroy: (self: AnimatedImage) -> (),
}
export type SplineNode = {
	Position: Dim,
	Thickness: number?,
	Angle: number?,
}
declare class Spline
	Alive: boolean
	Changed: Signal<string>
	Color: Color3
	Transparency: number
	ZIndex: number
	Visible: boolean
	Style: "Solid" | "Dashed" | "Dotted"
	Cap: "Rounded" | "Miter" | "Square"
	NodeCount: number
	function SetNodes(self, nodes: { SplineNode }): ()
	function GetNodes(self): { SplineNode }
	function InsertNode(self, index: number, node: SplineNode): ()
	function RemoveNode(self, index: number): ()
	function SetNode(self, index: number, node: SplineNode): ()
	function AttachShader(self, asset: FragmentAsset | ShaderAsset): ()
	function ClearShaders(self): ()
	function GetPropertyChangedSignal(self, name: string): Signal<...any>
	function Destroy(self): ()
end
export type GUI_API = {
	Basic: {
		Circle: () -> Primitive,
		Square: () -> Primitive,
		Triangle: () -> Primitive,
		Image: (asset: ImageAsset) -> Primitive,
		Font: (asset: FontAsset) -> Primitive,
		Clippable: () -> Primitive,
		Spline: () -> Spline,
	},
	DrawableImg: (width: number, height: number) -> DrawableImg,
	CanvasBuffer: (width: number, height: number) -> CanvasBuffer,
	AnimatedImage: (xml: string, source: any) -> AnimatedImage,
	UIEffectVolume: (image: (ImageAsset | DynImg | DrawableImg)?) -> UIEffectVolume,
	Raycast: (
		origin: Dim | { number },
		direction: Dim | { number },
		filter: ((primitive: any) -> boolean)?,
		maxDistance: number?
	) -> { Primitive: any, Distance: number, Position: Dim }?,
	CheckArea: (
		center: Dim | { number },
		size: Dim | { number },
		quality: string?,
		filter: ((primitive: any) -> boolean)?
	) -> { any },
	OverlapSphere: (
		center: Dim | { number },
		radius: number,
		filter: ((primitive: any) -> boolean)?
	) -> { any },
	OverlapBox: (
		center: Dim | { number },
		size: Dim | { number },
		rotation: number?,
		filter: ((primitive: any) -> boolean)?
	) -> { any },
	OverlapFrustum: (filter: ((primitive: any) -> boolean)?) -> { any },
	GetItemsInZone: (
		cframe: CFrame | Dim | { number },
		size: Dim | { number },
		filter: ((primitive: any) -> boolean)?
	) -> { any },
}
type Vec2Input = Dim | { X: number, Y: number } | { number }
declare class UIEffectVolume
	IsAlive: boolean
	ParticleCount: number
	Enabled: boolean
	Position: Dim
	Size: Dim
	ZIndex: number
	Image: (ImageAsset | DynImg | DrawableImg)?
	Rate: number
	MaxParticles: number
	Lifetime: NumberRange
	Speed: NumberRange
	ParticleSize: NumberRange
	Rotation: NumberRange
	RotSpeed: NumberRange
	Acceleration: Vec2Input
	RandomizeForce: { X: NumberRange?, Y: NumberRange? }
	Drag: number
	Spread: number
	EmissionDirection: Vec2Input
	Color: ColorSequenceInput
	TimeScaleColor: ColorSequenceInput
	Transparency: NumberSequenceInput
	TimeScaleTransparency: NumberSequenceInput
	SizeOverLife: NumberSequenceInput
	function Emit(self, n: number?): ()
	function Clear(self): ()
	function Destroy(self): ()
end
type _GuiLegacyMoved_ = {
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
	Moved: Signal<Dim, Dim>,
	InputReceived: Signal<MouseButton, "Begin" | "End">,
	Scrolled: Signal<number, number>,
	SetCursor: (self: Mouse_API, name: CursorName) -> (),
	Lock: (self: Mouse_API) -> (),
	Unlock: (self: Mouse_API) -> (),
	IsButtonDown: (self: Mouse_API, button: MouseButton) -> boolean,
}
export type LightSource = {
	Kind: string,
	CFrame: CFrame,
	Color: Color3,
	Brightness: number,
	Range: number,
	CastShadow: boolean,
	Falloff: number,
	ConeInner: number,
	ConeOuter: number,
	Alive: boolean,
	Destroy: (self: LightSource) -> (),
	AttachShader: (self: LightSource, shader: any) -> (),
	DetachShader: (self: LightSource) -> (),
	SetShaderParam: (self: LightSource, slot: number, value: number) -> (),
	Direction: (self: LightSource) -> Vector,
}
export type LightSourceOptions = {
	CFrame: CFrame?,
	Color: Color3?,
	Brightness: number?,
	Range: number?,
	CastShadow: boolean?,
	Falloff: number?,
	ConeInner: number?,
	ConeOuter: number?,
}
export type LightingService_API = {
	SetSun: (direction: Vector, color: Color3?) -> (),
	SetSunDirection: (direction: Vector) -> (),
	SetSunColor: (color: Color3) -> (),
	SetAmbient: (color: Color3) -> (),
	GetSunDirection: () -> Vector,
	GetSunColor: () -> Color3,
	GetAmbient: () -> Color3,
	Get: () -> { SunDirection: Vector, SunColor: Color3, Ambient: Color3 },
	AddObject: (self: LightingService_API, part: BasePart) -> (),
	RemoveObject: (self: LightingService_API, part: BasePart) -> (),
	IsObjectManaged: (self: LightingService_API, part: BasePart) -> boolean,
	SetSkybox: (asset: ShaderAsset | FragmentAsset) -> SceneShader,
	ClearSkybox: () -> (),
	SetPostEffect: (asset: ShaderAsset | FragmentAsset) -> SceneShader,
	ClearPostEffect: () -> (),
	GenerateLightSource: (kind: "PointLight" | "Spotlight", opts: LightSourceOptions?) -> LightSource,
	GetActiveLights: () -> { LightSource },
	GetLightCount: () -> number,
	UploadLights: (buffer: GPUBuffer) -> number,
}
export type TweenStyle = string
export type TweenDirection = "In" | "Out" | "InOut"
export type Tween = {
	Completed: Signal<>,
	State: string,
	Elapsed: number,
	Duration: number,
	Play: (self: Tween) -> (),
	Pause: (self: Tween) -> (),
	Resume: (self: Tween) -> (),
	Cancel: (self: Tween) -> (),
	Wait: (self: Tween) -> (),
}
export type TweenService_API = {
	new: (
		target: any,
		duration: number,
		style: TweenStyle,
		direction: TweenDirection?,
		properties: { [string]: any }
	) -> Tween,
	Create: (
		target: any,
		duration: number,
		style: TweenStyle,
		direction: TweenDirection?,
		properties: { [string]: any }
	) -> Tween,
	Lerp: (start: any, goal: any, alpha: number, style: TweenStyle?, direction: TweenDirection?) -> any,
}
export type Keyboard_API = {
	InputChanged: Signal<number, string, "Begin" | "End">,
	IsKeyDown: (self: Keyboard_API, key: string | number) -> boolean,
	GetKeyId: (self: Keyboard_API, name: string) -> number,
}
declare class VoiceShader end
declare class VoiceCapture
	OnPacket: Signal<string>
	function Stop(self): ()
	function IsActive(self): boolean
	function Pause(self): ()
	function Resume(self): ()
	function SetActive(self, on: boolean): ()
	function IsPaused(self): boolean
	function SetThreshold(self, amplitude: number): ()
	function GetThreshold(self): number
end
declare class VoiceChannel
	IsPlaying: boolean
	Volume: number
	Speed: number
	Pitch: number
	Position: Vector?
	MinFalloff: number
	MaxFalloff: number
	function Push(self, packet: string): ()
	function Play(self): ()
	function Stop(self): ()
	function ApplyShader(self, shader: VoiceShader): ()
	function ClearShaders(self): ()
	function SetPosition(self, x: number?, y: number?, z: number?): ()
	function SetSpatial(self, x: number?, y: number?, z: number?, max_falloff: number?): ()
end
declare class VoiceRecorder
	function Push(self, packet: string): ()
	function Clear(self): ()
	function Length(self): number
	function Duration(self): number
	function Serialize(self): string
	function ToRecording(self): VoiceRecording
end
declare class VoiceRecording
	function PacketCount(self): number
	function Duration(self): number
	function PlayInto(self, channel: VoiceChannel, opts: { loop: boolean?, speed: number? }?): ()
end
export type Voice_API = {
	StartCapture: () -> VoiceCapture,
	CreateChannel: () -> VoiceChannel,
	PlayPacket: (packet: string, opts: { x: number?, y: number?, z: number?, falloff: number?, min_falloff: number? }?) -> VoiceChannel,
	NewRecorder: () -> VoiceRecorder,
	LoadRecording: (data: string) -> VoiceRecording,
	Record: (duration: number) -> Signal<VoiceRecording, string>,
	PlayRecording: (
		data: string | VoiceRecording,
		opts: { ["loop"]: boolean?, speed: number? }?
	) -> VoiceChannel,
	Volume: (factor: number) -> VoiceShader,
	Speed: (factor: number) -> VoiceShader,
	Spatial: (x: number, y: number, z: number, max_falloff: number?) -> VoiceShader,
	ListenerPosition: (x: number, y: number, z: number) -> (),
}
declare class SteamLobby
	ID: string
	Owner: string
	IsOwner: boolean
	IsClosed: boolean
	OnMemberJoined: Signal<string>
	OnMemberLeft: Signal<string>
	OnDataUpdate: Signal<string?>
	OnDisconnected: Signal<SteamDisconnectReason>
	OnClose: Signal<>
	OnKick: Signal<string>
	OnMessage: Signal<string, string>
	function Members(self): { string }
	function SetData(self, key: string, value: string): ()
	function GetData(self, key: string): string?
	function DeleteData(self, key: string): boolean
	function GetAllData(self): { [string]: string }
	function MemberLimit(self): number?
	function SendChat(self, message: string): ()
	function SetJoinable(self, joinable: boolean): boolean
	function Close(self): ()
	function Kick(self, userId: string): ()
	function ShowInviteDialog(self): ()
	function Send(
		self,
		memberId: string,
		data: string | buffer,
		reliability: SteamReliability?
	): ()
	function Broadcast(self, data: string | buffer, reliability: SteamReliability?): ()
	function Leave(self): ()
end
declare class SteamServer
	ID: string
	OnClientAuthed: Signal<string>
	OnDisconnected: Signal<string>
	OnConnected: Signal<>
	OnConnectFailure: Signal<string, boolean>
	OnClose: Signal<>
	function SetServerName(self, name: string): ()
	function SetMapName(self, map: string): ()
	function SetMaxPlayers(self, max: number): ()
	function LogOnAnonymous(self): ()
	function Kick(self, userId: string): ()
	function Close(self): ()
	function Stop(self): ()
end
export type SteamPersonaState = "Offline" | "Online" | "Busy" | "Away" | "Snooze" | "LookingToTrade" | "LookingToPlay"
export type SteamDisconnectReason = "Disconnected" | "Kicked" | "Banned"
export type SteamReliability = "Reliable" | "Unreliable" | "ReliableNoNagle" | "UnreliableNoDelay"
export type SteamAvatarSize = "small" | "medium" | "large"
export type SteamLobbyKind = "Public" | "Private" | "FriendsOnly" | "Invisible"
export type SteamServerMode = "NoAuthentication" | "Authentication" | "AuthenticationAndSecure"
export type SteamOverlayDialog = "Friends" | "Community" | "Players" | "Settings" | "Stats" | "Achievements" | "OfficialGameGroup"
export type SteamOverlayUserDialog = "steamid" | "chat" | "jointrade" | "stats" | "achievements"
	| "friendadd" | "friendremove" | "friendrequestaccept" | "friendrequestignore"
export type SteamOverlayStoreMode = "None" | "AddToCart" | "AddToCartAndShow"
export type SteamNotificationPosition = "TopLeft" | "TopRight" | "BottomLeft" | "BottomRight"
export type SteamFriend = { ID: string, Name: string, State: SteamPersonaState }
export type SteamLobbyInfo = {
	ID: string,
	MemberCount: number?,
	Owner: string?,
	Name: string?,
	Data: { [string]: string }?,
}
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
	SteamPresent: boolean,
	Initialize: () -> boolean,
	OnDisconnected: Signal<string>,
	OnConnected: Signal<>,
	OnConnectFailure: Signal<string, boolean>,
	Shutdown: () -> (),
	OnLobbyJoinRequested: Signal<string, string>,
	InitRelayNetwork: () -> (),
	User: {
		ID: string,
		Name: string,
		Level: number,
		AccountID: () -> number,
		LoggedOn: () -> boolean,
		GetFriends: () -> { SteamFriend },
		GetAvatar: (size: SteamAvatarSize?) -> ImageAsset?,
		GetFriendAvatar: (id: string, size: SteamAvatarSize?) -> ImageAsset?,
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
		Create: (maxMembers: number?, kind: SteamLobbyKind?) -> Signal<SteamLobby?, string?>,
		Join: (id: string) -> Signal<SteamLobby?, string?>,
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
	Launch: {
		GetCommandLine: () -> string,
		GetQueryParam: (key: string) -> string?,
		GetCurrentBeta: () -> string?,
		GetRunType: () -> {
			Source: "steam-url" | "steam" | "direct",
			WasLaunchedFromSteam: boolean,
			CommandLine: string,
			AppId: number,
			BuildId: number?,
			OwnerID: string?,
			BetaBranch: string?,
			InstallDir: string?,
			Language: string?,
			ProcessArgs: { string },
			Env: { [string]: string },
		},
	},
}
export type GamepadInfo = {
	Id: number,
	Name: string,
	Connected: boolean,
	UUID: string?,
}
export type GamepadInputEvent = {
	Pad: number,
	Kind: "Button" | "Analog" | "Axis",
	Name: string,
	Value: number,
	State: "Begin" | "End" | "Change",
}
export type Gamepad_API = {
	GetGamepads: () -> { GamepadInfo },
	IsButtonDown: (pad: number, name: string) -> boolean,
	GetAxis: (pad: number, name: string) -> number,
	SetVibration: (pad: number, low: number, high: number, duration: number) -> boolean,
	Vibrate: (pad: number, intensity: number, duration: number) -> boolean,
	StopVibration: (pad: number?) -> (),
	GetBatteryLevel: (pad: number) -> number?,
	Connected: Signal<number>,
	Disconnected: Signal<number>,
	InputChanged: Signal<GamepadInputEvent>,
}
export type VRController = {
	Side: "Left" | "Right",
	CFrame: CFrame,
	Velocity: Vector,
	AngularVelocity: Vector,
	IsConnected: boolean,
	BatteryLevel: number?,
	Trigger: number,
	Grip: number,
	Thumbstick: Vector,
	Moved: Signal<CFrame>,
	OnInput: Signal<string, number, "Begin" | "End">,
	Vibrate: (
		self: VRController,
		duration: number,
		frequency: number?,
		amplitude: number?
	) -> boolean,
	StopVibration: (self: VRController) -> (),
}
export type VRPlayArea = {
	Width: number,
	Depth: number,
}
export type VRCamera = {
	BodyCframe: CFrame,
	HeadCframe: CFrame,
	IsLinked: boolean,
	RuntimeName: string,
	RefreshRate: number,
	IsHeadsetWorn: boolean,
	IPD: number,
	HeadMoved: Signal<CFrame>,
	Connected: Signal<string>,
	Unlink: (self: VRCamera) -> (),
	Recenter: (self: VRCamera) -> (),
	GetEyePose: (self: VRCamera, side: "Left" | "Right") -> CFrame,
	GetPlayArea: (self: VRCamera) -> VRPlayArea,
	Fade: (self: VRCamera, color: Color3, duration: number) -> (),
	GetControllers: (self: VRCamera) -> {
		Left: VRController,
		Right: VRController,
	},
}
export type VR_API = {
	IsVrPresent: () -> boolean,
	LinkVRView: () -> VRCamera,
}
export type VirtualRealityAttatchment = {
	Offset: CFrame,
	IsAlive: boolean,
	Side: "Left" | "Right" | "Detached",
	Destroy: (self: VirtualRealityAttatchment) -> (),
}
export type VirtualRealityController = {
	Side: "Left" | "Right",
	CFrame: CFrame,
	Trigger: number,
	Grip: number,
	Thumbstick: Vector,
	Velocity: Vector,
	AngularVelocity: Vector,
	IsConnected: boolean,
	BatteryLevel: number?,
	Moved: Signal<CFrame>,
	OnInput: Signal<string, number, "Begin" | "End">,
	ToWorldSpace: (self: VirtualRealityController) -> CFrame,
	Attatch: (
		self: VirtualRealityController,
		target: BasePart | DynMesh
	) -> VirtualRealityAttatchment,
	Vibrate: (
		self: VirtualRealityController,
		duration: number,
		frequency: number?,
		amplitude: number?
	) -> boolean,
	StopVibration: (self: VirtualRealityController) -> (),
}
export type VirtualReality_API = {
	HasVrFlag: boolean,
	IsVrPresent: boolean,
	BodyCFrame: CFrame,
	HeadCFrame: CFrame,
	IsLinked: boolean,
	HeadMoved: Signal<CFrame>,
	LinkCamera: (VirtualReality_API?) -> boolean,
	UnlinkCamera: (VirtualReality_API?) -> (),
	HeadToWorldSpace: (VirtualReality_API?) -> CFrame,
	Recenter: (VirtualReality_API?) -> (),
	GetEyePose: (VirtualReality_API?, side: "Left" | "Right") -> CFrame,
	GetControllers: (VirtualReality_API?) -> {
		[number]: VirtualRealityController,
		Left: VirtualRealityController,
		Right: VirtualRealityController,
	},
}
export type RaycastHit = {
	Part: BasePart,
	Position: Vector,
	Distance: number,
	Normal: Vector,
}
export type GPUInfo = {
	Name: string,
	Vendor: string,
	VendorID: number,
	DeviceID: number,
	Backend: string,
	Driver: string,
	DriverInfo: string,
	DeviceType: string,
}
export type GPULimits = {
	MaxTextureSize: number,
	MaxBufferSize: number,
	MaxBindGroups: number,
	MaxVertexBuffers: number,
	MaxComputeWorkgroup: { X: number, Y: number, Z: number },
}
export type GPUFrameStats = {
	FPS: number,
	FrameTime: number,
	FrameCount: number,
	Uptime: number,
	PartCount: number,
}
export type GPU_API = {
	Info: () -> GPUInfo,
	Limits: () -> GPULimits,
	FrameStats: () -> GPUFrameStats,
	Raycast: (
		origin: Vector,
		direction: Vector,
		filter: ((part: BasePart) -> boolean)?,
		maxDistance: number?
	) -> RaycastHit?,
	ScreenToRay: (point: Dim) -> (Vector, Vector),
	WorldToScreen: (world: Vector) -> (Dim?, boolean),
	NewBuffer: (size: number) -> GPUBuffer,
	SetBuffer: (buffer: GPUBuffer) -> (),
	ClearBuffer: () -> (),
	SetMaxFrameRate: (fps: number?) -> (),
	GetMaxFrameRate: () -> number?,
	SetVSync: (on: boolean) -> (),
	GetVSync: () -> boolean,
	SetPowerMode: (mode: "Quality" | "Performance" | "Auto") -> (),
	GetPowerMode: () -> "Quality" | "Performance" | "Auto",
	GetBounds: (part: BasePart, quality: number?) -> { { Position: Vector, Size: Vector, Min: Vector, Max: Vector } },
	CheckArea: (center: Vector, size: Vector, quality: number?) -> { BasePart },
	OverlapSphere: (center: Vector, radius: number, filter: ((BasePart) -> boolean)?) -> { BasePart },
	OverlapBox: (center: Vector, size: Vector, rotation: Vector?, filter: ((BasePart) -> boolean)?) -> { BasePart },
	OverlapFrustum: (filter: ((BasePart) -> boolean)?) -> { BasePart },
	GetItemsInZone: (cframe: CFrame, size: Vector, filter: ((BasePart) -> boolean)?) -> { BasePart },
	SetShadowsEnabled: (on: boolean) -> (),
	GetShadowsEnabled: () -> boolean,
	SetShadowMapQuality: (size: number) -> (),
	GetShadowMapQuality: () -> number,
	SetShadowDistance: (distance: number) -> (),
	GetShadowDistance: () -> number,
	SetShadowBias: (bias: number) -> (),
	GetShadowBias: () -> number,
	SetShadowPCF: (taps: number) -> (),
	GetShadowPCF: () -> number,
}
declare class GPUBuffer
	function Size(self): number
	function Write(self, offset: number, values: { number }): ()
	function Fill(self, value: number): ()
end
declare class Actor
	function Push(self, ...: any): ()
	function Pop(self, yield_for_result: boolean?): ...any
	function Pending(self): number
	function Threads(self): number
	function Close(self): ()
end
export type Actor_API = {
	new: (source: string, threads: number?) -> Actor,
	FromFile: (path: string, threads: number?) -> Actor,
}
declare class DebugFlag
	function Begin(self): ()
	function End(self): { name: string, ms: number, mem_delta_mb: number, cpu_pct: number, frame_drops: number, gpu_avg_ms: number }
	function Last(self): { [string]: number }
	function Name(self): string
end
export type Debug_API = {
	IsTest: boolean,
	Flag: (name: string) -> DebugFlag,
	Memory: () -> number,
	MemoryMB: () -> number,
	CpuPercent: () -> number,
	FrameDrops: () -> number,
	PeakFrameMs: () -> number,
	ResetPeak: () -> (),
	SetFrameDropThreshold: (ms: number) -> (),
	MonitorFrameDrops: (threshold_ms: number?) -> Signal<number>,
	GpuSubmitAvgMs: () -> number,
	ResetGpuStats: () -> (),
}
declare class DynMesh
	function Weld(self, target: BasePart, opts: { kind: ("Static" | "Attach" | "Joint")?, point: Vector?, rotation: Vector?, vertex: number?, name: string? }?): number
	function Stretch(self, target: BasePart, anchors: { { point: Vector?, vertex: number? } }): number
	function Detach(self, weld_id: number): boolean
	function DetachByName(self, name: string): boolean
	function DetachPart(self, target: BasePart): boolean
	function Clear(self): number
	function GetPart(self, weld_id: number): BasePart?
	function GetPartByName(self, name: string): BasePart?
	function GetParts(self): { { Id: number, Name: string?, Part: BasePart } }
	function GetBase(self): BasePart
	function IsAlive(self): boolean
	function Destroy(self): ()
end
export type DynMesh_API = {
	new: (base: BasePart) -> DynMesh,
}
export type FFIArg =
	nil
	| boolean
	| number
	| string
	| Vector
	| Color3
	| CFrame
	| { FFIArg }
	| { [string]: FFIArg }
declare class FFILibrary
	Alive: boolean
	Source: string
	function Call(self, name: string, args: FFIArg?): any
	function GetNative(self): { [string]: any }?
	function Unload(self): ()
end

export type FFI_API = {
	Load: (name: string) -> FFILibrary,
	BinDirectory: () -> string?,
	List: () -> { string },
	CallAsync: (library: FFILibrary, name: string, args: FFIArg?) -> any,
	_BeginAsyncCall: (library: FFILibrary, name: string, args: FFIArg?) -> (),

	RegisterSignal: (name: string, signal: Signal<...any>) -> (),
	UnregisterSignal: (name: string) -> boolean,

	SetData: (key: string, value: FFIArg) -> (),
	GetData: (key: string) -> any,
	HasData: (key: string) -> boolean,
	DeleteData: (key: string) -> boolean,
}
declare class Video
	Looped: boolean
	CurrentFrame: number
	function Width(self): number
	function Height(self): number
	function FrameCount(self): number
	function FrameDelayMs(self): number
	function Length(self): number
	function Source(self): string
	function Play(self): ()
	function Pause(self): ()
	function Stop(self): ()
	function CurrentImage(self): ImageAsset
	function LinkPart(self, part: BasePart): ()
	function UnlinkPart(self, part: BasePart): ()
end
export type Task_API = {
	Spawn: (fn: () -> ()) -> thread,
	Defer: (fn: () -> ()) -> thread,
	Delay: (seconds: number, fn: () -> ()) -> thread,
	Wait: (seconds: number?) -> (),
	Schedule: (id: string?, seconds: number, fn: () -> ()) -> string,
	Reschedule: (id: string, seconds: number) -> boolean,
	Repeat: (id: string?, interval: number, fn: () -> ()) -> string,
	SetInterval: (id: string, interval: number) -> boolean,
	Cancel: (target: string | thread) -> boolean,
	Exists: (id: string) -> boolean,
	TimeLeft: (id: string) -> number?,
}
export type Register_API = {
	new: (name: string) -> { [any]: any },
	Has: (name: string) -> boolean,
	Drop: (name: string) -> boolean,
	Names: () -> { string },
}
export type PhysicsPlaneOptions = {
	Gravity: Vector?,
	Threads: number?,
	LinearDamping: number?,
	AngularDamping: number?,
	Drag: number?,
	Buoyancy: number?,
	ConstantForce: Vector?,
	RestThreshold: number?,
	SolverIterations: number?,
	LoopSolver: boolean?,
	NeverSleep: boolean?,
	DeferGpu: boolean?,
}
declare class PhysicsObject
	CFrame: CFrame
	Velocity: Vector
	AngularVelocity: Vector
	ImpulseDirection: Vector
	ConstantForce: Vector
	Weight: number
	Density: number
	Bounciness: number
	Friction: number
	Anchored: boolean
	CanCollide: boolean
	HitBoxQuality: number
	Size: Vector
	CenterOfMass: Vector
	Alive: boolean
	function ApplyImpulse(self, vector: Vector): ()
	function FieldRestraint(self, opts: {
		LockX: boolean?,    LockY: boolean?,    LockZ: boolean?,
		LockRotX: boolean?, LockRotY: boolean?, LockRotZ: boolean?,
	}): ()
	function RotateTo(self, target: Vector, strength: number?): ()
	function StopRotateTo(self): ()
	function SetPin(
		self,
		force: number,
		relative: Vector,
		target: CFrame
	): ()
	function SetPin(self, force: nil): ()
	function RecalculateCenterOfMass(self): ()
	function BasePart(self): BasePart
	function Unlink(self): ()
	function Destroy(self): ()
end
declare class PhysicsPlane
	Gravity: Vector
	Enabled: boolean
	Threads: number
	LinearDamping: number
	AngularDamping: number
	Drag: number
	Buoyancy: number
	ConstantForce: Vector
	RestThreshold: number
	SolverIterations: number
	LoopSolver: boolean
	NeverSleep: boolean
	DeferGpu: boolean
	Alive: boolean
	ObjectCount: number
	function Add(self, part: BasePart): PhysicsObject
	function NewController(self, part: BasePart, opts: ControllerOptions?): Controller
	function NewControlledController(self, part: BasePart, opts: ControllerOptions?): Controller
	function SetEnabled(self, on: boolean): ()
	function Step(self, dt: number): ()
	function Destroy(self): ()
end
export type ControllerOptions = {
	WalkSpeed: number?,
	TurnSpeed: number?,
	JumpPower: number?,
	Gravity: number?,
	UsePlaneGravity: boolean?,
	MaxSlope: number?,
	MaxStepHeight: number?,
	GroundY: number?,
	UseGroundYFallback: boolean?,
	WaypointThreshold: number?,
	ProbeDistance: number?,
	CapsuleRadius: number?,
	CapsuleHalfHeight: number?,
	LookWhereMoving: boolean?,
}
declare class Controller
	Alive: boolean
	Position: Vector
	Velocity: Vector
	Rotation: Vector
	WalkSpeed: number
	TurnSpeed: number
	JumpPower: number
	Gravity: number
	UsePlaneGravity: boolean
	OnGround: boolean
	LookWhereMoving: boolean
	Controlled: boolean
	GroundY: number
	PathLength: number
	PathIndex: number
	Jumped: Signal<>
	Landed: Signal<>
	Moved: Signal<number, number, number>
	WaypointReached: Signal<number>
	PathFinished: Signal<>
	Changed: Signal<string>
	function BasePart(self): BasePart
	function WalkTo(self, target: Vector | CFrame): ()
	function ComputePath(self, waypoints: { Vector | CFrame }): number
	function AppendWaypoint(self, target: Vector | CFrame): ()
	function StopWalking(self): ()
	function Jump(self): boolean
	function Teleport(self, target: Vector | CFrame): ()
	function SetMoveVector(self, x: number, z: number): ()
	function MoveForward(self, intensity: number?): ()
	function MoveBackward(self, intensity: number?): ()
	function MoveLeft(self, intensity: number?): ()
	function MoveRight(self, intensity: number?): ()
	function ClearMoveIntent(self): ()
	function Destroy(self): ()
end
export type ClonableContainer = {
	Alive: boolean,
	ChildCount: number,
	AddChild: (self: ClonableContainer, child: any, removeFn: ((any) -> ())?) -> (),
	RemoveChild: (self: ClonableContainer, child: any) -> boolean,
	GetChildren: (self: ClonableContainer) -> { any },
	Clone: (self: ClonableContainer) -> ClonableContainer,
	Destroy: (self: ClonableContainer) -> (),
}
export type Movable = {
	Alive: boolean,
	ChildCount: number,
	Mode: string,
	CFrame: CFrame,
	Position: Dim,
	Rotation: number,
	AddChild: (self: Movable, child: any, removeFn: ((any) -> ())?) -> (),
	RemoveChild: (self: Movable, child: any) -> boolean,
	GetChildren: (self: Movable) -> { any },
	Destroy: (self: Movable) -> (),
}
export type Sizable = {
	Alive: boolean,
	ChildCount: number,
	Mode: string,
	Size: any,
	AddChild: (self: Sizable, child: any, removeFn: ((any) -> ())?) -> (),
	RemoveChild: (self: Sizable, child: any) -> boolean,
	GetChildren: (self: Sizable) -> { any },
	Destroy: (self: Sizable) -> (),
}
export type Billboard = {
	Alive: boolean,
	ChildCount: number,
	Position: Vector,
	Size: Dim,
	ScaleWithCamera: boolean,
	AddChild: (self: Billboard, child: any) -> (),
	RemoveChild: (self: Billboard, child: any) -> (),
	GetChildren: (self: Billboard) -> { any },
	Destroy: (self: Billboard) -> (),
}
declare class UIEnvironment
	Alive: boolean
	ChildCount: number
	ZIndex: number
	function AddChild(self, part: BasePart): ()
	function RemoveChild(self, part: BasePart): boolean
	function GetChildren(self): { BasePart }
	function Destroy(self): ()
end
export type Objects_API = {
	ClonableContainer: () -> ClonableContainer,
	Movable: () -> Movable,
	Sizable: () -> Sizable,
	Billboard: () -> Billboard,
	UIEnvironment: () -> UIEnvironment,
	BulkUpdate: (batch: { [any]: { [string]: any } }) -> number,
}
export type GuiPhysicsPlaneOptions = {
	Gravity: Dim?,
	LinearDamping: number?,
	AngularDamping: number?,
	Drag: number?,
	ConstantForce: Dim?,
	RestThreshold: number?,
	SolverIterations: number?,
	NeverSleep: boolean?,
	BoundsMin: Dim?,
	BoundsMax: Dim?,
}
export type GuiControllerOptions = {
	MoveSpeed: number?,
	Acceleration: number?,
	JumpStrength: number?,
	WaypointRadius: number?,
}
declare class PhysicsGuiObject
	Position: Dim
	Velocity: Dim
	Rotation: number
	AngularVelocity: number
	ImpulseDirection: Dim
	ConstantForce: Dim
	Mass: number
	Bounciness: number
	Friction: number
	Anchored: boolean
	CanCollide: boolean
	GravityScale: number
	LockedX: boolean
	LockedY: boolean
	LockedRotation: boolean
	ZIndex: number
	Size: Dim
	Shape: "Square" | "Circle" | "Triangle" | "Image" | "Text" | "Clippable"
	Sleeping: boolean
	Alive: boolean
	function ApplyImpulse(self, impulse: Dim): ()
	function ApplyForce(self, force: Dim): ()
	function Wake(self): ()
	function Destroy(self): ()
end
declare class GUIController
	MoveDirection: Dim
	MoveSpeed: number
	Acceleration: number
	JumpStrength: number
	OnGround: boolean
	Object: PhysicsGuiObject
	Alive: boolean
	WaypointReached: Signal<number>
	PathFinished: Signal<>
	Moved: Signal<number, number>
	GroundChanged: Signal<boolean>
	function Move(self, direction: Dim): ()
	function Jump(self): ()
	function SetWaypoints(self, points: { Dim }): ()
	function ClearWaypoints(self): ()
	function MoveTo(self, target: Dim): ()
	function Destroy(self): ()
end
declare class GuiPhysicsPlane
	Gravity: Dim
	Enabled: boolean
	LinearDamping: number
	AngularDamping: number
	Drag: number
	ConstantForce: Dim
	RestThreshold: number
	SolverIterations: number
	NeverSleep: boolean
	Alive: boolean
	ObjectCount: number
	function Add(self, primitive: Primitive): PhysicsGuiObject
	function NewController(self, primitive: Primitive, opts: GuiControllerOptions?): GUIController
	function NewUncontrolled(self, primitive: Primitive, opts: GuiControllerOptions?): GUIController
	function Step(self, dt: number): ()
	function Raycast(self, from: Dim, dir: Dim, maxDistance: number?): { Distance: number, Position: Dim, Normal: Dim, Object: PhysicsGuiObject }?
	function Destroy(self): ()
end
export type PhysicsService_API = {
	New: (opts: PhysicsPlaneOptions?) -> PhysicsPlane,
	NewGUI: (opts: GuiPhysicsPlaneOptions?) -> GuiPhysicsPlane,
}
export type Imports = {
	Actor: Actor_API,
	Asset: Asset_API,
	Debug: Debug_API,
	DynImg: DynImg_API,
	DynMesh: DynMesh_API,
	FFI: FFI_API,
	Gamepad: Gamepad_API,
	GPU: GPU_API,
	GUI: GUI_API,
	Image: Image_API,
	IO: IO_API,
	Keyboard: Keyboard_API,
	LightingService: LightingService_API,
	Managed: Managed_API,
	Mouse: Mouse_API,
	Net: Net_API,
	Objects: Objects_API,
	PhysicsService: PhysicsService_API,
	Primitives: Primitives_API,
	Process: Process_API,
	Register: Register_API,
	Renderable: Renderable_API,
	Serde: Serde_API,
	SFX: SFX_API,
	SoundByte: SoundByte_API,
	Signal: Signal_API,
	Steam: Steam_API,
	Task: Task_API,
	TweenService: TweenService_API,
	Voice: Voice_API,
	VR: VR_API,
	VirtualReality: VirtualReality_API,
	Window: Window_API,
}
declare import: <K>(name: keyof<Imports> & K) -> index<Imports, K>
declare __dirname: string
declare load: (path: string) -> any
