
-- =============================================================================
-- Ruzit Luau type declarations
-- =============================================================================
-- This file is consumed by luau-lsp to give you hover docs, autocomplete, and
-- type checking against every API a Ruzit script can touch. It is NOT loaded
-- by the runtime, the actual implementations live in Rust. Don't edit the
-- type shapes unless you also patch Ruzit/src/templates.rs (which holds the
-- bundled copy that `Ruzit Init` writes into new projects).
--
-- Top-level usage from any Luau script:
--   local Window = import("Window")     -- pulls a typed library handle
--   local IO     = import("IO")         -- ...
--   local _      = require(__dirname .. "/Helpers")
--
-- `import(name)` is the only way to access engine subsystems, there are no
-- Lua globals for any of them. Every name listed in `Imports` at the bottom
-- of this file is valid; anything else throws.
-- =============================================================================

-- =============================================================================
-- IO, file system access (read, write, list, mkdir, persistent handles).
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
	-- System paths. All resolved per-OS using XDG conventions on Linux,
	-- standard %APPDATA% / known-folders on Windows, and ~/Library on
	-- macOS. Each can be `nil` if the OS doesn't expose that location
	-- (rare). Read-only — these are snapshots taken when IO was first
	-- imported.
	-- User home directory (`~` on Linux/macOS, `C:\Users\<user>` on Windows).
	Home: string?,
	-- "My Documents" (Windows) / `~/Documents` (Linux/macOS).
	Documents: string?,
	Desktop: string?,
	Downloads: string?,
	Pictures: string?,
	Videos: string?,
	Music: string?,
	-- Roaming app config:
	--   Windows  %APPDATA%
	--   Linux    $XDG_CONFIG_HOME or ~/.config
	--   macOS    ~/Library/Application Support
	AppData: string?,
	-- Local (per-machine) app data:
	--   Windows  %LOCALAPPDATA%
	--   Linux    $XDG_DATA_HOME or ~/.local/share
	--   macOS    ~/Library/Application Support
	LocalAppData: string?,
	-- Cache directory:
	--   Windows  %LOCALAPPDATA%
	--   Linux    $XDG_CACHE_HOME or ~/.cache
	--   macOS    ~/Library/Caches
	Cache: string?,
	-- OS temp directory (always non-nil).
	Temp: string,
	-- The process's current working directory at the moment IO loaded.
	WorkingDir: string,
	-- Folder containing the running executable.
	ExecutableDir: string,
	-- Path separator: "\\" on Windows, "/" elsewhere.
	Separator: string,
	-- "windows" | "linux" | "macos" | "unknown".
	Platform: ("windows" | "linux" | "macos" | "unknown"),
}

-- =============================================================================
-- Net, TCP, UDP, IPC, WebSocket, and HTTP. Everything is blocking from the
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
	-- `handler` for each request, return either an HttpResponse table or
	-- a plain string. Single-threaded by default; if you need concurrency,
	-- spawn handlers through Actor.
	Serve: (addr: string, handler: (HttpRequest) -> (HttpResponse | string)) -> (),
	-- Synchronous HTTP client. method = "GET" / "POST" / etc. Headers and
	-- body are optional. Throws on connection failure; non-2xx statuses
	-- come back through .status, not as exceptions.
	Request: (method: string, url: string, body: string?, headers: { [string]: string }?) -> HttpResponse,
	-- TCP primitives. Connect dials, Host binds + listens.
	TCP: { Connect: (addr: string) -> TcpConnection, Host: (addr: string) -> TcpListener },
	-- UDP, Bind opens a socket on the given local "host:port".
	UDP: { Bind: (addr: string) -> UdpHandle },
	-- IPC, same-machine stream. Pass a pipe/socket NAME, not a path; the
	-- OS-specific prefix is added for you.
	IPC: { Connect: (name: string) -> IpcConnection, Host: (name: string) -> IpcListener },
	-- WebSocket. Pass a `ws://` or `wss://` URL to Connect; pass an
	-- "addr:port" to Host (TLS isn't auto-enabled on Host, terminate it
	-- in front of Ruzit).
	Socket: { Connect: (url: string) -> WebSocketConn, Host: (addr: string) -> WebSocketListener },
}

-- =============================================================================
-- Serde, Encoding (JSON/TOML/YAML), hashing, and compression.
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
-- Process, info about and control over the running process.
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
	-- True when running from a built game launcher (.exe + Managed/),
	-- false under `Ruzit Test`. Gate dev-only debug overlays etc. behind
	-- this (or behind InTest, which is exactly the inverse).
	IsBuilt: boolean,
	-- Inverse of IsBuilt, true when running via `Ruzit Test`. Reads
	-- naturally for guards like `if Process.InTest then ... end`.
	InTest: boolean,
	-- Per-frame signal carrying dt (seconds since last tick). Fires
	-- before the render pump, use it for simulation logic that produces
	-- the state the renderer reads this frame. (Merged from the old
	-- RunService import as of v3.3.)
	Heartbeat: Signal<number>,
	-- Same dt, but fires from inside the render pump (slightly later in
	-- the frame than Heartbeat). Use it for camera updates that need to
	-- land after gameplay state has finalised.
	RenderStepped: Signal<number>,
	-- Read environment variables. Two forms:
	--   Process.Env("PATH") → "..."  (or nil if unset)
	--   Process.Env()       → { ["PATH"] = "...", ... }  (whole map)
	Env: ((name: string) -> string?) & (() -> { [string]: string }),
	-- Set an env var on the running process (and inherited children).
	SetEnv: (name: string, value: string) -> (),
	-- Hard-exit the process. `code` defaults to 0. Doesn't run BindToClose
	-- callbacks, use Window:Close for graceful shutdown when you have
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
	-- string label, use it later with UnbindFromHeart, and also as the
	-- de-dup key (registering the same id twice replaces the old fn).
	-- The callback gets dt (seconds since the previous tick). For most
	-- per-frame logic prefer Process.Heartbeat, BindToHeart is for
	-- background tasks that need to run even when no window is open.
	BindToHeart: (id: string, fn: (dt: number) -> ()) -> (),
	UnbindFromHeart: (id: string) -> (),
}

-- =============================================================================
-- Asset, typed loaders for images, sounds, shaders, fonts, models, files.
-- =============================================================================

-- =============================================================================
-- :Free()
-- Every asset userdata (ImageAsset, SoundData, ShaderAsset, FragmentAsset,
-- ModelAsset, FontAsset) exposes a :Free() method. It's the per-instance
-- counterpart of Asset.Drop:
--   * Asset.Drop("Image:foo.bar") frees the asset behind a path.
--   * asset:Free() frees the engine references to *this exact handle*,
--     no path needed, works on procedurally-generated assets too
--     (Asset.FromString, Asset.FromPixels, Renderable.SimplifyMesh).
--
-- What it actually does:
--   * Detaches every BasePart / Primitive / Sound currently using this asset
--     (textures, models, fonts, shader attachments, sound voices),
--   * Drops the entry from the engine's asset cache so a future GetAsset
--     re-loads from disk,
--   * Leaves your local Lua variable alone, you can keep a reference, the
--     engine just stops tracking it. Memory is reclaimed once nothing in
--     Lua holds it either.
--
-- Use :Free() when you've still got the asset in a local but want to nuke
-- engine usage of it (e.g. clearing every Part textured with the same image).
-- Use Asset.Drop when you only know the path.
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
	-- you need to inspect or mutate pixels, most uses just hand the asset
	-- to a GUI / Renderable.
	function Pixels(self): string
	-- Per-instance free, see top of section. Detaches every Part / Primitive
	-- using this image and drops it from the asset cache.
	function Free(self): ()
end

declare class SoundData
	function Source(self): string
	-- Encoded byte length (NOT decoded sample count). Decoding happens on
	-- :Play() in SFX.LoadSound.
	function ByteCount(self): number
	-- Per-instance free, see top of section. Stops every Sound currently
	-- backed by this data and drops it from the asset cache. Existing
	-- Sound wrappers go silent on next :Play().
	function Free(self): ()
end

-- =============================================================================
-- Shaders
-- =============================================================================
-- Ruzit shaders are written in WGSL, wgpu's native shader language.
-- (If you've used GLSL/HLSL, WGSL reads similarly: typed structs, explicit
-- uniform buffers, attribute-declared entry points.) The runtime is wgpu 22
-- on top of DX12 (Windows) / Vulkan / Metal / GL.
--
-- There are two stages, each loaded as its own asset kind:
--   * ShaderAsset, VERTEX stage (.shader / .glsl / .wgsl / .hlsl /
--                      .vert / .metal extensions). Most projects never
--                      override this; the engine's default vertex stage
--                      handles transform / projection / interpolation.
--   * FragmentAsset, FRAGMENT (pixel) stage (.frag / .fragment / .fs /
--                      .glslf extensions). This is where ~all custom shader
--                      work lives, surface lighting, dissolve effects,
--                      stylized lighting, scrolling textures, post effects.
--
-- Where shaders go (the four hook points):
--   * BasePart:AttachShader(asset), runs once per pixel of a 3D part.
--   * Primitive:AttachShader(asset), runs per pixel of a 2D GUI shape.
--   * Sound:AttachShader(asset), DSP "shader" for audio (separate
--                                           contract, see Sound docs).
--   * GUI.SetSkybox(asset) / .SetPostEffect(asset), fullscreen scene passes.
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
--   Setting a name that isn't declared throws, typo guard.
--
-- ──────────────────────────────────────────────────────────────────────────
-- 2D primitive fragment contract
--   (FragmentAsset attached to Primitive, or fed to SetSkybox / SetPostEffect)
--
-- Available WGSL globals (the engine prepends these as a prelude, DON'T
-- redeclare them in your shader):
--
--   struct VsOut {
--       @builtin(position) clip: vec4<f32>,
--       @location(0) uv: vec2<f32>, // 0..1, top-left origin
--   };
--   struct RuzitUni {
--       pos: vec2<f32>, // primitive top-left in pixels
--       size: vec2<f32>, // primitive width/height
--       color: vec4<f32>, // RGBA tint (alpha = 1 - Transparency)
--       resolution: vec2<f32>, // window inner size in pixels
--       time: f32, // seconds since window opened
--       shape: u32, // 0=Square 1=Circle 2=Triangle 3=Image
--       params: array<vec4<f32>, 4>, // your @ruzit params, packed
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
-- Minimum useful shader (replicates the default, solid tint, shape-masked):
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
-- Vertex inputs (already populated by the engine's vertex stage, read in
-- the fragment as in.world_pos / in.world_normal / in.uv):
--
--   struct VsOut {
--       @builtin(position) clip: vec4<f32>,
--       @location(0) world_pos: vec3<f32>,
--       @location(1) world_normal: vec3<f32>, // already normalized
--       @location(2) uv: vec2<f32>,
--   };
--
-- Per-frame uniform, same value for every part this frame:
--
--   struct Frame {
--       view_proj: mat4x4<f32>,
--       light_dir: vec3<f32>, // sun direction (engine or
--                                    // LightingService.SetSun)
--       time: f32, // seconds since window opened
--       camera_pos: vec3<f32>,
--       frame_index: u32, // increments every drawn frame,
--                                    // useful for hash seeds, TAA jitter,
--                                    // ping-pong patterns
--       sun_color: vec3<f32>, // engine or LightingService.SetSunColor
--       ambient: vec3<f32>, // engine or LightingService.SetAmbient
--       viewport: vec2<f32>, // window inner pixel size
--   };
--
-- Per-part uniform, different for every BasePart drawn:
--
--   struct Instance {
--       model: mat4x4<f32>, // CFrame * Size
--       color: vec4<f32>, // BasePart.Color
--       params: array<vec4<f32>, 4>, // your @ruzit param floats
--       // Render hints, 0 or 1 each, set per-part from Lua:
--       cast_shadow: u32, // BasePart.CastShadow
--       receive_shadow: u32, // BasePart.ReceiveShadow
--       _flag2: u32, // reserved
--       _flag3: u32, // reserved
--   };
--
-- Branching on flags inside a shader (manual shadows / stylized lighting):
--   if (I.receive_shadow == 0u) {
--       // emissive / sky / debug, skip darkening, return albedo as-is
--       return vec4<f32>(I.color.rgb, I.color.a);
--   }
--   if (I.cast_shadow == 0u) {
--       // hint that this part shouldn't darken its neighbors. Useful if
--       // you implement contact-shadow-style AO that samples nearby parts
--       //, bail early so this one contributes nothing.
--   }
--
-- Available bindings:
--   @group(0) @binding(0) var<uniform> F: Frame;
--   @group(0) @binding(1) var<uniform> I: Instance;
--   @group(0) @binding(2) var IMG: texture_2d<f32>;     // BasePart.Texture,
--                                                       // or 1×1 white
--                                                       // when Texture=nil
--   @group(0) @binding(3) var IMG_SAMP: sampler;
--   @group(0) @binding(4) var<storage, read> SDATA: array<f32>;
--                            -- Optional read-only storage buffer.
--                            -- Default is a 1-float stub. Populate from
--                            -- Lua via GPU.NewBuffer + GPU.SetBuffer to
--                            -- pass arbitrary float data, LUTs, particle
--                            -- positions, animation curves, custom mesh
--                            -- attributes. Up to your GPU's
--                            -- max_storage_buffer_binding_size (typically
--                            -- ≥ 128 MB on desktop). Indexed as
--                            -- `SDATA[idx]`.
--
-- Engine-provided WGSL helpers (declared in the prelude, call directly):
--   p(idx: u32) -> f32
--     Read a `// @ruzit param` float by declaration index.
--
--   view_dir(world_pos) -> vec3<f32>
--     Normalized vector from a pixel toward the camera. Use for fresnel +
--     specular: `let v = view_dir(in.world_pos);`.
--
--   tangent_basis(n: vec3<f32>) -> mat3x3<f32>
--     Cheap orthonormal frame (T, B, N) from a world-space normal. Stable
--     across the surface; suitable for simple tangent-space tricks. Not
--     UV-derivative based, so it won't perfectly match a baked normal map.
--
--   fresnel_schlick(cos_theta, F0: vec3<f32>) -> vec3<f32>
--     Schlick Fresnel reflectance approximation. cos_theta = max(dot(N,V),0).
--     F0 ≈ vec3(0.04) for dielectrics, metal albedo for metals.
--
--   to_screen_uv(world_pos) -> vec3<f32>
--     Project a world point to (uv.xy, ndc.z). Top-left-origin uv. Returns
--     vec3(-1,-1,-1) for points behind the camera.
--
--   linearize_depth(d, near, far) -> f32
--     Convert a depth-buffer value (0..1) to linear view-space distance.
--
--   srgb_to_linear(c) -> vec3<f32>
--   linear_to_srgb(c) -> vec3<f32>
--     Standard gamma curves. The swap-chain is sRGB so you usually don't
--     need these, included for shaders that want to do their own grading
--     in linear space.
--
--   hash3(p: vec3<f32>) -> vec3<f32>
--     Cheap deterministic [0,1] hash, three components. Seed it from
--     in.world_pos * scale for stylized noise. Not cryptographic.
--
-- Required entry point:
--   @fragment
--   fn fs_main(in: VsOut) -> @location(0) vec4<f32> { ... }
--
-- Minimum useful shader (replicates the engine default, sun + ambient
-- half-Lambert lighting, sun color tint):
--   @fragment
--   fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
--       let n = normalize(in.world_normal);
--       let nl = dot(n, -F.light_dir);
--       let half_lambert = nl * 0.5 + 0.5;
--       let lit = F.ambient + F.sun_color * half_lambert;
--       let tex = textureSample(IMG, IMG_SAMP, in.uv);
--       return vec4<f32>(I.color.rgb * tex.rgb * lit, I.color.a);
--   }
--
-- Storage-buffer example, color-grading LUT keyed by pixel luma:
--   // @ruzit param size              // number of LUT entries
--   @fragment
--   fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
--       let base = textureSample(IMG, IMG_SAMP, in.uv).rgb * I.color.rgb;
--       let lum = dot(base, vec3<f32>(0.299, 0.587, 0.114));
--       let n = u32(p(0u));
--       let i = u32(clamp(lum * f32(n - 1u), 0.0, f32(n - 1u)));
--       let r = SDATA[i * 3u + 0u];
--       let g = SDATA[i * 3u + 1u];
--       let b = SDATA[i * 3u + 2u];
--       return vec4<f32>(r, g, b, I.color.a);
--   }
--
--   -- Lua side:
--   local lut = GPU.NewBuffer(256 * 3)
--   for i = 0, 255 do
--       local t = i / 255
--       lut:Write(i * 3, { t, t * 0.5 + 0.25, 1.0 - t })
--   end
--   GPU.SetBuffer(lut)
--   part:SetData(myShader, "size", 256)
--
-- ──────────────────────────────────────────────────────────────────────────
-- Coming later (NOT yet implemented):
--   * Render-to-texture: render the 3D scene from any camera into a
--     reusable Texture you can sample from another shader (mirrors,
--     security cams, planar reflections, refraction). Needs a ping-pong
--     scene-target system in the renderer.
--   * Real shadow maps: an automatic light-POV depth pass + shadow
--     comparison sampler binding. The Frame uniform will gain a
--     `shadow_view_proj`, and a new `texture_depth_2d` + `sampler_comparison`
--     binding will appear. Existing shaders won't break, those bindings
--     just won't exist until the feature lands.
--
-- =============================================================================

declare class ShaderAsset
	function Source(self): string
	-- Per-instance free, see top of Asset section. Detaches this shader
	-- from every Part / Primitive / scene hook and drops it from the
	-- asset cache.
	function Free(self): ()
end
declare class FragmentAsset
	function Source(self): string
	-- Per-instance free, see top of Asset section. Detaches this fragment
	-- shader from every Part / Primitive / SetSkybox / SetPostEffect hook
	-- and drops it from the asset cache.
	function Free(self): ()
end

declare class FontAsset
	function Source(self): string
	-- Per-instance free, see top of Asset section. Drops every text
	-- Primitive backed by this font and clears it from the cache.
	function Free(self): ()
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
	--   File     any file, returned as its raw UTF-8 string contents
	--
	-- path forms:
	--   "foo.bar.baz", same package as the caller. Dots are folder
	--                       separators, so this maps to foo/bar/baz under the
	--                       package's assets/ root.
	--   "@PkgId/foo/bar", explicit cross-package lookup. Use this to read an
	--                       asset out of another loaded .managed package by id.
	--   "foo/bar.png", a literal extension also works; the loader uses it
	--                       directly instead of probing the kind's ext list.
	--
	-- examples:
	--   Asset.GetAsset("Image", "ui.logo")              -- assets/ui/logo.png
	--   Asset.GetAsset("Model", "props.crate")          -- assets/props/crate.fbx (or .obj)
	--   Asset.GetAsset("Sound", "@SfxPack/explode")     -- from a sibling package
	--   Asset.GetAsset("File", "data/levels.json")     -- raw text contents
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

	-- ── Bulk loading / unloading ──────────────────────────────────────────
	-- The Preload / BulkLoad / Drop trio shares a SINGLE process-wide
	-- asset cache, keyed by resolved (target package, kind, path), not
	-- by which script called the loader. Any script can preload, look
	-- up, or drop any asset, and the same Image / Sound / Model
	-- userdata is shared across the whole game. Same-named bare paths
	-- in different packages naturally stay separate (the cache key
	-- includes the resolved package id), and "@PkgId/..." paths always
	-- resolve to the explicit package they name.
	--
	-- Preload a list of assets in the background and fire the returned
	-- signal when every entry has finished decoding. Each entry is a
	-- "Kind:path" string, e.g. {"Image:ui.logo", "Sound:sfx.boom",
	-- "Model:props.crate"}. Reads (and zstd-decompression for bundled
	-- packages) happen on a worker thread; final parsing into the typed
	-- userdata happens on the main thread on the next heart tick. Once
	-- preloaded, subsequent Asset.GetAsset calls return the cached asset
	-- without touching the disk / package. The signal fires once with no
	-- arguments.
	PreloadAsync: (entries: { string }) -> Signal<>,

	-- Synchronous bulk load. Blocks the caller until every asset is
	-- decoded, then returns a dictionary keyed by the original "Kind:path"
	-- string. Same caching effect as PreloadAsync (subsequent
	-- GetAsset hits the cache), but you also get the values directly.
	BulkLoad: (entries: { string }) -> { [string]: any },

	-- Forcefully unload a previously-loaded asset by its "Kind:path"
	-- string. The cache slot is cleared and the strong-ref is released
	-- globally, any script that loaded the same asset loses access to
	-- the cached entry on its next GetAsset, which will then re-decrypt
	-- and re-decompress from disk. Behaviour for live consumers depends
	-- on the asset kind:
	--   Content assets (Image, Sound, Model, Font), every consumer is
	--   destroyed. BaseParts whose .Texture or .Model match the dropped
	--   asset are destroyed and fire "Destroyed". GUI Primitives backed
	--   by a dropped Image / Font are destroyed. Sounds loaded from a
	--   dropped SoundData stop and become unplayable (subsequent :Play()
	--   errors).
	--   Shader / Fragment, NOTHING is destroyed. The shader is detached
	--   from every BasePart / Primitive it was attached to (those keep
	--   rendering with the engine default) and cleared from the skybox /
	--   post-effect slot if it was bound there. Useful for hot-swapping
	--   post effects without disturbing scene geometry.
	-- Use this to free a level's content between scenes when you want
	-- the next GetAsset to genuinely re-decrypt + re-decompress from disk.
	Drop: (entry: string) -> (),

	-- Load a video plus its companion audio in one call. Returns
	-- (Video, SoundData?), the second value is nil for sources that
	-- don't carry audio. Pass the SoundData to SFX.LoadSound if you
	-- want it to play. Supported formats:
	--   .gif        animated GIF, no audio. Pure-Rust decoder, always
	--                available, no system deps.
	--   .mp4 / .mov ISO Base Media. Decoded by shelling out to
	--                `ffmpeg` on PATH at load time, video frames come
	--                back as RGBA, audio as 16-bit PCM WAV that's
	--                consumable by SFX.LoadSound. Requires ffmpeg
	--                installed on the host system.
	GetVideo: (path: string) -> (Video, SoundData?),
}

-- =============================================================================
-- Window, open a window, listen for size/focus changes, toggle modes.
-- =============================================================================

-- Options bag passed to Window.Open. All fields are optional with sensible
-- defaults (800x600 windowed, decorated, resizable, visible).
export type WindowOptions = {
	-- Caption shown in the title bar / taskbar.
	title: string?,
	-- Initial inner-area size in logical pixels (DPI-scaled by the OS).
	width: number?, height: number?,
	-- Inner-size constraints. Optional pairs, set both width and height
	-- of a constraint or neither.
	min_width: number?, min_height: number?, max_width: number?, max_height: number?,
	-- Exclusive fullscreen. Covers the whole monitor and changes display
	-- mode. Most games should prefer `borderless = true` instead.
	fullscreen: boolean?,
	-- Borderless fullscreen, strips decorations AND covers the OS taskbar
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
-- luau-lsp parser yet, but generic type aliases ARE, so Signal lives as
-- a parameterized table type instead. Same call shape (signal:Connect(...))
-- with full callback-arg type narrowing: `Signal<number, string>` makes
-- `signal:Connect(function(a, b) ... end)` infer `a:number, b:string`.
export type Signal<T...> = {
	-- Subscribe a callback. Returns a Connection, keep it if you ever
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
	-- DEPRECATED as of v3.3: a single fan-out "something changed" signal.
	-- Fires (changeName, ...payload) for engine-side events the OS told us
	-- about: "Resized" (w, h), "Moved" (x, y), "Focused" (boolean),
	-- "ScaleFactor" (scale). Setter-side changes (SetTitle, SetFullscreen,
	-- etc.) never fired this signal even when the docs claimed they did.
	-- Use the per-property *Changed signals below. As of v3.4 this signal
	-- is frozen, it keeps firing for the four OS events listed above but
	-- won't gain new fire conditions for any future window state. Code
	-- that connects keeps working; new behaviour goes through the typed
	-- per-property signals.
	Changed: Signal<string, ...any>
	-- Fires when the window gains keyboard / mouse focus.
	OnFocus: Signal<>
	-- Fires when the window loses focus. Useful for releasing mouse lock,
	-- pausing the game, etc.
	OnUnfocus: Signal<>

	-- Per-property change signals. Each fires only when its specific
	-- property changes; replaces the generic Changed signal as of v3.3.

	-- Inner-area size changed. Args: width, height (logical pixels).
	-- Fires from the OS Resized event (user dragged the corner / window
	-- manager forced it / Resize() actually applied).
	SizeChanged: Signal<number, number>
	-- Outer position changed. Args: x, y (screen coords of the top-left).
	-- Fires from the OS Moved event.
	PositionChanged: Signal<number, number>
	-- DPI / HiDPI scale factor changed. Arg: new scale (e.g. 1.0, 1.5, 2.0).
	-- Fires when the window moves between monitors of different DPI.
	ScaleFactorChanged: Signal<number>
	-- Window gained or lost keyboard focus. Arg: now-focused boolean.
	-- Symmetrical pair of OnFocus / OnUnfocus rolled into one signal so a
	-- single handler can branch on the value.
	FocusChanged: Signal<boolean>
	-- Title-bar caption changed via SetTitle. Arg: the new title.
	TitleChanged: Signal<string>
	-- Fullscreen state changed via SetFullscreen or SetBorderless. Arg:
	-- now-fullscreen boolean.
	FullscreenChanged: Signal<boolean>
	-- Borderless-fullscreen state changed via SetBorderless. Arg:
	-- now-borderless boolean. Note SetBorderless also fires
	-- DecorationsChanged + FullscreenChanged because it toggles those too.
	BorderlessChanged: Signal<boolean>
	-- Decoration state changed via SetDecorations or SetBorderless. Arg:
	-- now-decorated boolean.
	DecorationsChanged: Signal<boolean>
	-- Resize-via-edges enabled state changed via SetResizable.
	ResizableChanged: Signal<boolean>
	-- Maximized state changed via SetMaximized.
	MaximizedChanged: Signal<boolean>
	-- Minimized (in taskbar / dock) state changed via SetMinimized.
	MinimizedChanged: Signal<boolean>
	-- Visibility changed via SetVisible.
	VisibleChanged: Signal<boolean>
	-- Always-on-top pinning changed via SetAlwaysOnTop.
	AlwaysOnTopChanged: Signal<boolean>
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
	-- Toggles "borderless fullscreen", strips decorations AND enters
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
	-- this, the engine repaints automatically when the GUI/Renderable
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
	-- Renderable.Camera returns, exposed here so a Lua module that only
	-- has a WindowHandle can still drive the view.
	function GetViewport(self): Camera
end

export type Window_API = {
	-- Open the application's window. Only one is allowed at a time;
	-- calling twice throws. Returns a WindowHandle for runtime control.
	Open: (opts: WindowOptions?) -> WindowHandle,
}

-- =============================================================================
-- SFX, stream and shape audio. Built on rodio + cpal.
-- =============================================================================

-- Built shader instance produced by SFX.Volume / SFX.Echo / etc. Stack via
-- Sound:ApplyShader for compound effects.
declare class SoundShader end

declare class Sound
	-- Fires when :Play() actually starts feeding audio.
	Started: Signal<>
	-- Fires when playback ends (natural completion or :Stop).
	Stopped: Signal<>
	-- Fires every time a looped sound wraps back to the start (only fires when
	-- Looped = true). The first :Play() does NOT fire this; subsequent loop
	-- restarts do. If the SFX.Repeat shader is used instead of Looped, this
	-- signal won't fire (the source repeats internally with no boundary the
	-- engine can observe).
	DidLoop: Signal<>
	-- Source label inherited from the SoundData.
	Source: string
	-- When true, playback restarts at the beginning each time the source
	-- ends, and DidLoop fires on each wrap. Toggle live: setting to false
	-- mid-play lets the current iteration finish naturally and then stops.
	Looped: boolean
	-- Current playback position in seconds. Reads track real-time progress
	-- (modulo total duration when Looped is true). Writing while playing
	-- seeks immediately; writing while stopped sets the start offset for
	-- the next :Play() call.
	TimePosition: number
	-- Modifier properties. Each is a single live slot; assigning replaces
	-- the prior value (no stacking). Assign nil / 0 / false to remove.
	-- Changes apply on the next :Play() and are kept between plays.
	-- Linear gain. 1.0 unchanged, 0.0 silence, >1 boosted.
	Volume: number
	-- Time + pitch. 2.0 = 2x faster (also pitched up an octave).
	Speed: number
	-- Alias for Speed; read/write the same underlying value.
	Pitch: number
	-- Stereo pan. -1 left / 0 center / +1 right.
	Pan: number
	-- Lowpass / highpass biquad. Set to nil to disable.
	LowPass: number?
	HighPass: number?
	-- Linear fade envelope length in seconds. Set to 0 to disable.
	FadeIn: number
	FadeOut: number
	-- Wait before producing audio, in seconds. Set to 0 to disable.
	Delay: number
	-- Legacy in-source repeat. Prefer `Looped` for engine-observed loops
	-- with DidLoop fire.
	Loop: boolean
	-- Soft saturation amount. Set to 0 to disable; >1 hard-clips.
	Distortion: number
	-- Single-tap echo. Set to nil to disable.
	Echo: { Delay: number, Feedback: number?, Mix: number? }?
	-- Diffuse reverb. Set to nil to disable.
	Reverb: { Mix: number?, Decay: number? }?
	-- Periodic amplitude wobble. Set to nil to disable.
	Tremolo: { Rate: number?, Depth: number? }?
	-- 3D world position. Listener = the active Renderable.Camera. Set to
	-- nil to revert to non-positional (centered) playback.
	Position: Vector?
	-- Inner / outer falloff radii. Distances ≤ MinFalloff play at full
	-- volume; distances ≥ MaxFalloff are silent; in between is a smooth
	-- roll-off. Defaults: 0 / 20.
	MinFalloff: number
	MaxFalloff: number
	-- Begin playback. Modifier properties / shaders set before this carry
	-- into the next play.
	function Play(self): ()
	-- Halt the currently-playing instance. Fires Stopped.
	function Stop(self): ()
	function IsPlaying(self): boolean
	-- Drop ALL fluent shaders and (custom) attachments, back to a clean state.
	function Reset(self): ()
	-- Convenience setters; equivalent to assigning the Position /
	-- MinFalloff / MaxFalloff properties above. `max_falloff` falls back
	-- to the current MaxFalloff (or 20 if unset).
	function SetPosition(self, x: number, y: number, z: number, max_falloff: number?): ()
	function ClearPosition(self): ()
	-- Legacy / advanced: build shader instances yourself.
	function ApplyShader(self, shader: SoundShader): ()
	function ClearShaders(self): ()
	-- Custom shader assets, write your own DSP in WGSL/GLSL-like text and
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
	-- many independent Sounds, each has its own shader chain and
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
-- SoundByte — graph-routed audio system. The replacement for the old SFX/Voice
-- modules (which remain importable for back-compat but are now deprecated).
--
-- Build a graph by linking sources (Player, VoiceChannel) through modifiers
-- into sinks (OutputNode for speakers, ByteSink for serialized strings).
-- Every link is `SoundByte.Link(source, ..modifiers, sink)` and returns a
-- LinkHandle with `:Unlink()`. Modifiers store Enabled / Min / Max / Value;
-- everything in the graph is tweenable via TweenService.
--
--   local SoundByte = import("SoundByte")
--   local Asset = import("Asset")
--   local sfx = Asset.GetAsset("Sound", "sfx.boom")
--
--   local player  = SoundByte.NewPlayer(sfx)
--   local output  = SoundByte.NewOutput()
--   local reverb  = SoundByte.GetModifier("Reverb")
--   reverb.Min, reverb.Max, reverb.Value = 0, 1, 0.5
--   local link = SoundByte.Link(player, reverb, output)
--   player:Play()
--   ...
--   link:Unlink()
-- =============================================================================

declare class SoundByteModifier
	-- One of: "Volume", "Speed", "Pitch", "Pan", "Distortion", "LowPass",
	-- "HighPass", "Echo", "Reverb", "Tremolo", "FadeIn", "FadeOut",
	-- "Delay", "PlaybackSpeed".
	Kind: string
	-- When false the modifier is in the link graph but doesn't apply.
	Enabled: boolean
	-- Min / Max are the clamp range for `Value`. Setting Min above Max
	-- (or vice versa) auto-corrects the other side. `Value` is the
	-- currently-applied amount, clamped to [Min, Max].
	Min: number
	Max: number
	Value: number
end

declare class SoundBytePlayer
	Source: string
	-- Fires when the player starts emitting audio after a :Play().
	Started: Signal<>
	-- Fires when playback drains (natural end or :Stop()).
	Stopped: Signal<>
	-- Fires every wrap when `Looped = true`. The first :Play() doesn't
	-- fire it; only subsequent loop restarts do.
	DidLoop: Signal<>
	-- True between :Play() and the next Stopped.
	IsPlaying: boolean
	Looped: boolean
	-- Begin playback. Requires at least one linked sink (OutputNode or
	-- ByteSink). Calls re-trigger from the start.
	function Play(self): ()
	function Stop(self): ()
end

declare class SoundByteOutput
	-- 0 = silent. >1 boosts.
	Volume: number
	-- Inner / outer falloff radii. Distance ≤ MinDistance plays at full
	-- volume; ≥ MaxDistance is silent; smooth roll-off between.
	FalloffMinDistance: number
	FalloffMaxDistance: number
	-- Pose this output in 3D. Setting to nil reverts to non-spatial.
	-- `Position` is also writable; `CFrame` takes precedence if both
	-- are set.
	CFrame: CFrame?
	Position: Vector
	-- Anchor to a BasePart. The output's position is copied from the
	-- part each heart tick. Equivalent to repeatedly setting Position
	-- from the part's CFrame.position. Call :Unfollow to release.
	function Follow(self, part: BasePart): ()
	function Unfollow(self): ()
end

declare class SoundByteByte
	-- Fires once per audio packet with a binary string of little-endian
	-- f32 samples (PCM, interleaved if channels > 1). Hook into Net /
	-- Steam P2P / disk to stream the audio elsewhere.
	OnPacket: Signal<string>
	SampleRate: number
	Channels: number
	QueueLength: number
	-- Drop any pending packets that haven't fired yet.
	function Drain(self): number
end

declare class SoundByteVoiceChannel
	Enabled: boolean
	-- Voice activation amplitude threshold. 0 = always send.
	Threshold: number
end

declare class SoundByteByteSource
	-- When false, SendInput is a no-op and queued audio stops flowing
	-- into downstream sinks.
	Enabled: boolean
	-- Sample rate the queued samples are interpreted at (Hz).
	SampleRate: number
	-- Number of interleaved channels in the queue.
	Channels: number
	-- Current number of buffered f32 samples waiting to be played.
	QueueLength: number
	-- QueueLength / (SampleRate * Channels) — how much audio is buffered
	-- ahead of the playhead.
	BufferSeconds: number
	-- Feed a packet of little-endian f32 samples (same format as
	-- SoundByteByte.OnPacket). Length must be a multiple of 4.
	function SendInput(self, packet: string): ()
	-- Feed a Lua array of raw f32 samples (already decoded).
	function SendSamples(self, samples: { number }): ()
	-- Empty the playback queue without tearing down any links.
	function Clear(self): ()
	-- Stop all downstream sinks routed from this source and clear the
	-- queue. Links remain attached until the next :SendInput.
	function Stop(self): ()
	-- Stop and unregister this source. After Destroy any further
	-- SendInput calls do nothing.
	function Destroy(self): ()
end

declare class SoundByteLink
	-- Detach this link. The source no longer routes through these
	-- modifiers to this sink. If the source has no remaining links a
	-- subsequent Play() will error.
	function Unlink(self): ()
end

export type SoundByte_API = {
	-- True when this build was compiled with the voice feature enabled
	-- (`--features voice`). When false, `GetVoiceChannel` errors.
	VoiceFlagEnabled: boolean,
	-- Wrap a SoundData asset into a playable Player.
	NewPlayer: (data: SoundData) -> SoundBytePlayer,
	-- Allocate a new speaker output. Default Volume 1.0, non-spatial.
	NewOutput: () -> SoundByteOutput,
	-- Allocate a new byte sink. Connect a source via `Link` to start
	-- receiving PCM packets on `.OnPacket`.
	NewByte: () -> SoundByteByte,
	-- Allocate a queue-fed audio source. Feed it the byte packets that
	-- arrive from a remote SoundByteByte via :SendInput and Link it into
	-- an OutputNode to play. Defaults: 48000 Hz, 1 channel — pass
	-- (sampleRate, channels) to match the producing SoundByteByte.
	NewByteSource: (sampleRate: number?, channels: number?) -> SoundByteByteSource,
	-- Build a modifier of the given kind. See `SoundByteModifier.Kind`
	-- for valid strings.
	GetModifier: (kind: string) -> SoundByteModifier,
	-- Allocate a new voice channel. Errors if VoiceFlagEnabled is false.
	GetVoiceChannel: () -> SoundByteVoiceChannel,
	-- Wire a source → modifiers → sink. Order is significant for
	-- modifiers (left-to-right in the call). Returns a LinkHandle that
	-- can :Unlink() to detach.
	Link: (...any) -> SoundByteLink,
}

-- =============================================================================
-- Managed, runtime info about loaded .managed packages (the project itself,
-- DLCs, and anything dropped into the Packages/ folder).
-- =============================================================================

declare class Package
	-- The package id (used in @PkgId/... cross-package references).
	ID: string
	-- Display name, set in build.toml's Name or the package's ManagedInfo.
	Name: string
	-- Semantic version string from build.toml / ManagedInfo.toml.
	Version: string
	-- Author/creator string, free-form.
	Creator: string
	-- The entry script's vfs key (typically "Main.luau" or "init.luau").
	Entry: string
	-- A vfs prefix you can pass to `require` to load this package's modules
	-- (e.g. "@OtherPkg/sub/module"). For the calling package, this is the
	-- empty string, use `require(__dirname .. "/...")` for self.
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
	-- All loaded package ids, your own, every DLC, every Packages/ import.
	List: () -> { string },
	-- The default (entry-point) package. In a built game this is the
	-- launcher's primary package; under `Ruzit Test` it's the project at
	-- the working directory.
	Default: () -> Package?,
}

-- =============================================================================
-- Primitives, math types: Dim (2D), Color3, Vector (3D), CFrame (rigid pose).
-- All immutable: arithmetic returns new instances, never mutates in place.
-- =============================================================================

declare class Dim
	-- 2D component. .X is horizontal, .Y is vertical (screen-space-ish:
	-- +Y is down, matching pixel coords).
	X: number
	Y: number
	-- Linearly interpolate between self and other. t=0 → self, t=1 → other.
	function Lerp(self, other: Dim, t: number): Dim
	-- Rescale this Dim from one reference size to another. Useful for
	-- prebuilt UI: design at a fixed reference (say 400x400), then call
	-- `:Scale(reference, target)` at runtime to map the layout to the
	-- current window size.
	--   reference   — the design-time size the dim was authored against.
	--   target      — the runtime size to map into (e.g. Window.GetSize()).
	--   maintainRatio — when true, both axes use the smaller of the two
	--                   scale factors so the aspect ratio is preserved
	--                   (good for letter-boxed UI). Default false.
	function Scale(self, reference: Dim, target: Dim, maintainRatio: boolean?): Dim
	function __add(self, other: Dim): Dim
	function __sub(self, other: Dim): Dim
	-- Scalar multiply / divide / negate.
	function __mul(self, other: number): Dim
	function __div(self, other: number): Dim
	function __unm(self): Dim
	-- Component-wise comparisons. `==`, `<`, `<=` return true only when
	-- BOTH axes match the relation (so `<` is partial-order, not total).
	function __eq(self, other: Dim): boolean
	function __lt(self, other: Dim): boolean
	function __le(self, other: Dim): boolean
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
	-- Component-wise comparisons. `==`, `<`, `<=` return true only when
	-- ALL THREE channels match the relation.
	function __eq(self, other: Color3): boolean
	function __lt(self, other: Color3): boolean
	function __le(self, other: Color3): boolean
end

declare class Vector
	-- 3D component vector.
	X: number
	Y: number
	Z: number
	-- Cached euclidean length (sqrt(x^2 + y^2 + z^2)).
	Magnitude: number
	function Lerp(self, other: Vector, t: number): Vector
	-- Dot product. Positive when vectors point similar directions,
	-- negative when opposite, zero when perpendicular.
	function Dot(self, other: Vector): number
	-- Right-handed cross product, perpendicular to both inputs.
	function Cross(self, other: Vector): Vector
	-- Returns a unit-length copy. Returns the original if magnitude is
	-- effectively zero.
	function Normalized(self): Vector
	-- Euclidean distance between two points.
	function Distance(self, other: Vector): number
	function __add(self, other: Vector): Vector
	function __sub(self, other: Vector): Vector
	-- Scalar multiply / divide / negate.
	function __mul(self, other: number): Vector
	function __div(self, other: number): Vector
	function __unm(self): Vector
	-- Component-wise comparisons. `==`, `<`, `<=` return true only when
	-- ALL THREE axes match the relation (so `<` is partial-order, not
	-- magnitude-based — use `.Magnitude` for length comparisons).
	function __eq(self, other: Vector): boolean
	function __lt(self, other: Vector): boolean
	function __le(self, other: Vector): boolean
end

declare class ModelAsset
	-- Total per-vertex count after triangulation.
	function VertexCount(self): number
	-- Triangle count (== indices/3).
	function TriangleCount(self): number
	function Source(self): string
	-- Per-instance free, see top of Asset section. Destroys every
	-- BasePart spawned from this model (BaseModel / SimplifyMesh
	-- output) and drops it from the asset cache.
	function Free(self): ()
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
	-- Strict component-wise equality. Both position and rotation must
	-- match exactly. For "are these CFrames close enough?" comparisons
	-- prefer :FuzzyEq, since ordinary equality on floats is brittle.
	function __eq(self, other: CFrame): boolean
	-- True if every component (3 position + 3 rotation) is within
	-- `epsilon` of the corresponding component on `other`. Default
	-- epsilon is 1e-5. Use this for "is the part roughly at this
	-- pose?" checks where direct == would fail due to floating-point
	-- jitter.
	-- Note: rotation is compared as raw Euler radians, not as angular
	-- distance — so two visually identical rotations that differ by a
	-- multiple of 2π will NOT compare equal. Wrap your bounds yourself
	-- if you care.
	function FuzzyEq(self, other: CFrame, epsilon: number?): boolean
end

declare class UDim
	-- Scale-based 2D component. Each axis is a fraction (typically 0..1)
	-- of a parent dimension. Pair with a Dim parent to resolve to pixels
	-- via :ToDim(parent).
	X: number
	Y: number
	-- Multiply by a parent Dim to get the absolute Dim. e.g.
	--   UDim.new(0.5, 0.25):ToDim(Dim.new(800, 600)) == Dim.new(400, 150)
	function ToDim(self, parent: Dim): Dim
	function Lerp(self, other: UDim, t: number): UDim
	function __add(self, other: UDim): UDim
	function __sub(self, other: UDim): UDim
	function __mul(self, other: number): UDim
	function __div(self, other: number): UDim
	function __unm(self): UDim
	-- Component-wise comparisons. `==`, `<`, `<=` return true only when
	-- BOTH axes match the relation.
	function __eq(self, other: UDim): boolean
	function __lt(self, other: UDim): boolean
	function __le(self, other: UDim): boolean
end

export type Dim_API = {
	-- Dim.new(x, y), typed 2D component.
	new: (x: number, y: number) -> Dim,
}
export type Color3_API = {
	-- Linear 0..1 channels. Out-of-range values are clamped.
	-- Example: Color3.new(0.18, 0.55, 0.28) = a forest green.
	new: (r: number, g: number, b: number) -> Color3,
	-- 8-bit (0..255) channels, convenient for colors copied from a paint
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
	-- Vector.new(x, y, z), any axis defaults to 0.
	new: ((x: number?, y: number?, z: number?) -> Vector),
	-- (0, 0, 0) shorthand.
	zero: () -> Vector,
	-- (1, 1, 1) shorthand.
	one: () -> Vector,
	-- Convert a Luau native `vector` (the built-in primitive returned by
	-- `vector(x, y, z)` and the `vector.*` library) into a Ruzit Vector
	-- userdata. Useful when you've got a native vector from a math
	-- helper or external module and need it in a form Ruzit's APIs
	-- (BasePart.Size, CFrame.new, etc.) accept.
	--     Vector.FromNative(vector(1, 2, 3)) == Vector.new(1, 2, 3)
	FromNative: (native: vector) -> Vector,
}
export type CFrame_API = {
	-- CFrame.new(position?, rotation?). Either argument is optional.
	new: (position: Vector?, rotation: Vector?) -> CFrame,
	-- Build a rotation-only CFrame from Euler angles (radians, applied X→Y→Z).
	Angles: (rx: number, ry: number, rz: number) -> CFrame,
	-- Build a CFrame at `eye`, oriented so its forward axis points at
	-- `target`. `up` defaults to (0, 1, 0). The returned rotation is in
	-- Euler form (radians); compose it with CFrame.new(pos) for movement.
	LookAt: (eye: Vector, target: Vector, up: Vector?) -> CFrame,
}

export type UDim_API = {
	-- UDim.new(x, y), each in scale units (0..1 maps to 0%..100% of parent).
	new: ((x: number?, y: number?) -> UDim),
	zero: () -> UDim,
	one: () -> UDim,
	-- One-shot scale-to-pixels. Equivalent to
	-- UDim.new(scaleX, scaleY):ToDim(parent), but reads more naturally
	-- when the scale values are constants:
	--     UDim.FromDim(0.2, 0.2, parentDim)
	-- means "give me a Dim that's 20% of parentDim on each axis."
	-- Use it for sizing buttons or tiles relative to a container
	-- without first constructing a UDim value.
	FromDim: (scaleX: number, scaleY: number, parent: Dim) -> Dim,
}

export type Primitives_API = {
	Dim: Dim_API,
	Color3: Color3_API,
	Vector: Vector_API,
	CFrame: CFrame_API,
	UDim: UDim_API,
}

-- =============================================================================
-- Renderable, 3D scene. Parts, models, the camera.
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
	-- Accepts:
	--   * ImageAsset (loaded from disk / generated)
	--   * DynImg     (live-composited canvas of GUI primitives)
	--   * DrawableImg (CPU-mutable image you draw to with shape
	--                  primitives — see GUI.DrawableImg)
	-- For DynImg / DrawableImg the texture refreshes whenever the
	-- underlying buffer changes, so a UI you keep mutating in Lua
	-- becomes a live texture you can wallpaper onto a 3D part.
	Texture: (ImageAsset | DynImg | DrawableImg)?
	-- Render-hint flags. None of these change the engine's default
	-- lighting yet, they're plumbed straight through to every 3D
	-- fragment shader as `I.cast_shadow` / `I.receive_shadow` (u32 0/1)
	-- so user shaders (and eventually the engine's automatic shadow
	-- pass) can branch on them. Defaults: both true.
	-- Turn ReceiveShadow off on skies, emissive geometry, and pure
	-- debug overlays so they don't pick up your manual shadow term;
	-- turn CastShadow off on selection outlines / fx-only meshes that
	-- shouldn't darken anything else.
	CastShadow: boolean
	ReceiveShadow: boolean
	-- When true, GPU.Raycast skips this part entirely. Default false.
	-- Use it for "ghost" parts (debug overlays, UI anchors, fx-only
	-- meshes) that shouldn't catch mouse picks. Unrelated to shadows
	-- mechanically, grouped here because it's the third in the family
	-- of render-hint toggles.
	IgnoreInRaycast: boolean
	-- When true, the default 3D shader applies the engine sun + ambient
	-- term to this part. When false (the default for newly-spawned
	-- BaseParts), the default shader outputs flat color so you can wire
	-- your own lighting (custom Frag3D shader, LightSources via SDATA,
	-- etc.). Also surfaced inside user shaders as `I.lit` (u32 0/1).
	-- The cleaner way to toggle this in bulk is
	-- `LightingService:AddObject(part)` / `:RemoveObject(part)`.
	Lit: boolean
	-- Property-change signal. Fires with one of: "CFrame", "Size",
	-- "Color", "Render", "Texture", "CastShadow", "ReceiveShadow",
	-- "IgnoreInRaycast", "Lit", "Destroyed".
	Changed: Signal<string>
	-- Per-property signal that fires with the new value as the only
	-- argument every time `propName` changes. Lazily created — properties
	-- nobody listens to cost zero per tick. Useful for reactive code that
	-- only cares about one specific field instead of filtering Changed by
	-- name. For PhysicsService-driven parts the "CFrame" signal also
	-- fires each frame the simulation moves the part (skipped
	-- automatically when nobody is listening).
	-- Examples:
	--   part:GetPropertyChanged("CFrame"):Connect(function(cf) ... end)
	--   part:GetPropertyChanged("Color"):Connect(function(c) ... end)
	function GetPropertyChanged(self, propName: string): Signal<any>
	-- Remove the part from the scene. Connections to Changed still receive
	-- one final "Destroyed" fire.
	function Destroy(self): ()
	-- Custom shader hooks, same shape as Sound's, but for the GPU stage
	-- that draws this part.
	function AttachShader(self, asset: ShaderAsset | FragmentAsset): ()
	function DetachShader(self, asset: ShaderAsset | FragmentAsset): ()
	function ClearShaders(self): ()
	function SetData(self, asset: ShaderAsset | FragmentAsset, name: string, value: number): ()
	function GetData(self, asset: ShaderAsset | FragmentAsset, name: string): number?

	-- ── Mesh deformation ──────────────────────────────────────────────────
	-- Push every vertex within `envelope` units of `target.Position` by
	-- `displacement * smoothstep(distance / envelope)`. Smooth radial
	-- falloff, per-Part copy of the mesh, deforming one Part doesn't
	-- affect other Parts that share the same source ModelAsset.
	-- Returns the number of vertices touched.
	function Deform(self, target: CFrame, envelope: number, displacement: Vector): number
	-- Drop the per-Part vertex override and snap back to the source mesh.
	-- Animations on this part still tick, call track:Reset() to clear
	-- those.
	function ResetDeformation(self): ()

	-- ── Local <-> world helpers ──────────────────────────────────────────
	-- Convert an offset in the part's local space to a world-space
	-- position. `Vector.new(0, 3, 0)` returns the world point 3 units
	-- "above" the part (along its rotated local Y axis), accounting for
	-- the part's CFrame.
	function LocalToWorld(self, local_offset: Vector): Vector
	-- Inverse of LocalToWorld. Convert a world-space position into the
	-- part's local frame.
	function WorldToLocal(self, world: Vector): Vector

	-- ── Animation tracks ─────────────────────────────────────────────────
	-- Get (or create) an AnimationTrack scoped to this part. If the source
	-- ModelAsset is an FBX with a matching AnimationStack, the track's
	-- keyframes are auto-populated from the FBX curves on first creation.
	function GetTrack(self, name: string): AnimationTrack
	-- Every track currently attached to the part (FBX-derived or
	-- user-created via GetTrack).
	function GetTracks(self): { AnimationTrack }
	-- Names of every animation clip baked into the source FBX file. Empty
	-- for OBJ / synthesised meshes / FBX exports without animations.
	function GetTrackNames(self): { string }

	-- ── Audio anchoring ──────────────────────────────────────────────────
	-- Pin an SFX Sound or VoiceChannel to this part. While linked, the
	-- engine copies `part.CFrame.Position` into the audio's `Position`
	-- property every heart tick, so the source follows the part in 3D
	-- without any per-frame Lua. Existing MinFalloff / MaxFalloff are
	-- preserved (defaults 0 / 20 for SFX, 0 / 8 for Voice if the audio
	-- had no Position yet). Linking the same audio again replaces the
	-- previous binding. Destroying the part drops the link automatically.
	function LinkInput(self, audio: Sound | VoiceChannel): ()
	-- Drop a prior LinkInput binding. The audio's Position freezes at
	-- the last synced value, assign `audio.Position = nil` if you want
	-- to revert to non-positional playback.
	function UnlinkInput(self, audio: Sound | VoiceChannel): ()
end

-- =============================================================================
-- AnimationTrack, playable animation clip for a single BasePart.
-- =============================================================================
-- Tracks come from one of two places:
--   * `part:GetTrack("clipName")` on an FBX-backed Part, the track is
--     pre-loaded with translation/rotation keyframes parsed from the FBX
--     AnimationStack matching that name.
--   * `part:GetTrack("custom")` followed by `track:AddKeyframe(...)`,
--     fully scripted keyframes you author at runtime.
--
-- Tracks tick in the engine heart loop at ~60 Hz. While a track is playing
-- it overrides `part.CFrame` for CFrame keyframes and the part's mesh for
-- Deform keyframes. `track:Reset()` snaps everything back to whatever the
-- part looked like the moment `track:Play()` was last called.
declare class AnimationTrack
	Name: string
	FadeTime: number
	Looped: boolean
	Speed: number
	Length: number
	TimePosition: number
	IsPlaying: boolean
	IsPaused: boolean
	-- Fires when :Play() is called and the track starts feeding samples.
	Played: Signal<>
	-- Fires when the track stops, either naturally at the end (when
	-- Looped is false) or because :Stop was called.
	Stopped: Signal<>
	-- Fires every time a looped track wraps from end to start (only
	-- fires when Looped is true).
	DidLoop: Signal<>

	function Play(self): ()
	function Pause(self): ()
	function Resume(self): ()
	function Stop(self): ()
	function Reset(self): ()
	function AddKeyframe(self, time: number, kind: "CFrame" | "Deform", ...: any): ()
	function ClearKeyframes(self): ()
	-- Returns a Signal that fires every `interval` track-time seconds with
	-- the elapsed playback time. Useful for syncing footstep sound effects,
	-- particle bursts, or gameplay events to specific moments in an
	-- animation. Mirrors Sound:LinkToUpdate.
	function LinkToUpdate(self, interval: number): Signal<number>
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
	-- Returns the part, keep the reference to mutate or destroy it later.
	BasePart: (shape: ("Cube" | "Sphere")?) -> BasePart,
	-- Spawn a part backed by an arbitrary mesh (.obj / .fbx ModelAsset).
	BaseModel: (asset: ModelAsset) -> BasePart,
	-- The active 3D camera. Same instance the window uses; mutate it to move
	-- the view. Also exposed as WindowHandle:GetViewport().
	Camera: Camera,

	-- Polygon reduction (level-of-detail). Returns a NEW ModelAsset whose
	-- geometry is a quantization-clustered LOD of the input. `ratio` is a
	-- 0..1 hint (default 0.5 ≈ ~half the vertex count); the result tends
	-- to be coarser than the ratio suggests on small meshes, since the
	-- grid is uniform. The returned asset inherits the source's animation
	-- clips so swapping it in for distant parts keeps `:GetTrack` working.
	SimplifyMesh: (asset: ModelAsset, ratio: number?) -> ModelAsset,

	-- Define a region inside a part's mesh that you can move / rotate /
	-- scale to deform whatever vertices fall within it. The classic
	-- use-case is ad-hoc bones, rotate a head box to turn a head,
	-- shrink an eyelid box to blink an eye, stretch a torso box to
	-- elongate a body, all without authoring a real skeleton.
	--
	-- Arguments are interpreted in the part's LOCAL mesh space (the
	-- unit-cube space the engine scales by part.Size at render time):
	--   * `cframe` is the box's center + rotation. Pick the spot in
	--     model space the box should anchor at.
	--   * `size` is the box's full extent on each axis. Vertices whose
	--     model-space position falls inside the cframe-rotated box of
	--     this size are captured.
	--
	-- Captured vertices remember their position relative to the box's
	-- frame (normalized to [-0.5, 0.5] on each axis). Updating
	-- `box.CFrame` rotates / translates them around the box centre;
	-- updating `box.Size` scales them along the box's local axes.
	-- An identity transform (CFrame and Size unchanged from creation)
	-- leaves vertices untouched.
	--
	-- Multiple boxes can coexist on the same part. They apply in
	-- creation order, with later boxes overriding earlier ones for any
	-- vertices both captured. So `head` first, then `eyelid_top` /
	-- `eyelid_bottom` per eye gives nested control.
	--
	-- `box:Destroy()` freezes the box, captured vertices stay at
	-- whatever displaced position the box left them at, and the box
	-- stops accepting CFrame / Size writes. The mesh is NOT restored,
	-- this is by design so multi-pass distortions don't snap back.
	-- To restore, hold onto the part's source ModelAsset and rebuild,
	-- or flip a fresh DistortionBox over the same area with the
	-- original CFrame / Size.
	DistortionBox: (part: BasePart, cframe: CFrame, size: Vector) -> DistortionBox,

	-- Spawn an invisible 3D volume that emits sprite particles. Modeled
	-- after Roblox's ParticleEmitter: set CFrame / Size to describe the
	-- spawn region, hand it an image, tune the property knobs, and let
	-- it run. Particles are simulated on the Rust heart tick and rendered
	-- as billboarded textured quads (FaceCamera = true by default).
	-- Pass an ImageAsset / DynImg / DrawableImg at construction time, or
	-- assign `.Image` later.
	EffectVolume: (image: (ImageAsset | DynImg | DrawableImg)?) -> EffectVolume,
}

-- =============================================================================
-- EffectVolume — 3D particle emitter. Lifetime is in seconds, Rate is in
-- particles/sec. The volume is invisible; only the particles draw.
-- =============================================================================

-- Shared shape for {Min, Max} ranges. Setters also accept a plain number
-- (interpreted as {n, n}) or a {min, max} array.
type NumberRange = { Min: number, Max: number }
-- Color sequence input formats:
--   { [0] = c1, [0.5] = c2, [1] = c3 }            keyed table (preferred)
--   { { 0, c1 }, { 0.5, c2 }, { 1, c3 } }         list of {t, Color3}
--   Color3                                        single constant
type ColorSequenceInput = Color3 | { [number]: Color3 } | { { number | Color3 } }
-- Number sequence input formats (same options, but values are numbers).
type NumberSequenceInput = number | { [number]: number } | { { number } }

declare class EffectVolume
	IsAlive: boolean
	-- Live count of currently-alive particles.
	ParticleCount: number
	-- While false the emitter spawns nothing; alive particles keep
	-- simulating and fade out naturally.
	Enabled: boolean
	-- If true (default) particles billboard toward the active camera. If
	-- false they stay axis-aligned to the volume's CFrame rotation.
	FaceCamera: boolean

	-- Emission region: particles spawn at a random point inside the
	-- CFrame-oriented box of the given Size. Set CFrame to move the
	-- volume; Size scales the spawn region.
	CFrame: CFrame
	Size: Vector

	-- Sprite texture. Accepts ImageAsset / DynImg / DrawableImg. Read
	-- returns true if an image is set, false otherwise.
	Image: (ImageAsset | DynImg | DrawableImg)?

	-- Spawn rate in particles/sec. 0 disables spawning (alive particles
	-- still finish their lifetimes).
	Rate: number
	-- Hard cap on alive particles. Once hit, new spawns are dropped.
	-- Default 2048, max 100000.
	MaxParticles: number

	-- Per-particle randomized ranges.
	Lifetime: NumberRange
	Speed: NumberRange
	ParticleSize: NumberRange
	Rotation: NumberRange
	RotSpeed: NumberRange

	-- World-space constant force applied each tick (same for every particle).
	Acceleration: Vector
	-- Per-particle randomized force, sampled once at spawn and added to
	-- Acceleration each tick. Each axis is an independent {min, max} range
	-- in world units/sec². Missing axes default to {0, 0}.
	-- Example: `{ X = {min = -1, max = 1}, Y = {min = 4, max = 8} }`
	RandomizeForce: { X: NumberRange?, Y: NumberRange?, Z: NumberRange? }
	-- Per-second velocity damping. 0 = none, 1 = halve per second.
	Drag: number
	-- Cone half-angle in DEGREES around EmissionDirection. 0 = laser, 180 = full sphere.
	Spread: number
	-- Initial velocity direction in the volume's local space. Default is
	-- {0, 1, 0} (up).
	EmissionDirection: Vector

	-- Over-life sequences. Color/Transparency/SizeOverLife and their
	-- TimeScale* aliases share the same underlying storage; assign either
	-- name and read either name.
	Color: ColorSequenceInput
	TimeScaleColor: ColorSequenceInput
	Transparency: NumberSequenceInput
	TimeScaleTransparency: NumberSequenceInput
	SizeOverLife: NumberSequenceInput

	-- Force-spawn `n` particles right now, ignoring Rate (still bounded
	-- by MaxParticles). Default 1.
	function Emit(self, n: number?): ()
	-- Kill every alive particle immediately.
	function Clear(self): ()
	-- Stop everything and free the volume.
	function Destroy(self): ()

	-- Same WGSL contract as BasePart's. Attached shaders modify the
	-- particle fragment output; SetData / GetData address `// @ruzit
	-- param` slots.
	function AttachShader(self, asset: ShaderAsset | FragmentAsset): ()
	function DetachShader(self, asset: ShaderAsset | FragmentAsset): ()
	function ClearShaders(self): ()
	function SetData(self, asset: ShaderAsset | FragmentAsset, name: string, value: number): ()
	function GetData(self, asset: ShaderAsset | FragmentAsset, name: string): number?
end

declare class DistortionBox
	-- Current center + rotation of the box, in part-local model
	-- space. Writable, mutating it transforms the captured vertices
	-- on the next tick.
	CFrame: CFrame
	-- Current full extent of the box. Writable, scales captured
	-- vertices on the next tick.
	Size: Vector
	-- The box's CFrame at creation. Used as the reference frame for
	-- the captured vertex offsets. Read-only.
	InitialCFrame: CFrame
	-- The box's Size at creation. Read-only.
	InitialSize: Vector
	-- Number of vertices the box captured at creation. Useful for
	-- debugging "why isn't this region deforming?" cases (typically
	-- the box was placed off-center or sized too small to enclose
	-- any vertices).
	VertexCount: number
	IsAlive: boolean

	-- Freeze the box. Captured vertices stay where they currently
	-- are; subsequent CFrame / Size writes are ignored. The mesh is
	-- intentionally NOT restored.
	function Destroy(self): ()
end

-- =============================================================================
-- GUI, 2D overlay primitives, post-effects, skybox.
-- =============================================================================

declare class Primitive
	-- Geometry / kind. The set is fixed at creation; switch by destroying
	-- and rebuilding via the appropriate GUI.Basic factory. "Clippable"
	-- is a special invisible container that clips its `:AddClippable`
	-- children to its Position+Size rect.
	Shape: "Square" | "Circle" | "Triangle" | "Image" | "Text" | "Clippable"
	-- Pixel-space size. For Text this is (0,0), text size is .TextSize.
	Size: Dim
	-- Top-left position in screen coords (origin at the top-left of the window).
	Position: Dim
	-- Rotation around the primitive's center, in DEGREES. 0 is the natural
	-- orientation; positive values rotate clockwise (screen-space, Y-down).
	-- Applies to Square / Image / Font / Triangle / Circle / Clippable
	-- and to AnimatedImage and DrawableImg (since both rendered through
	-- Primitive). Spatial queries (GUI.Raycast, GUI.OverlapBox, etc.)
	-- still test against the un-rotated AABB for now — visual rotation
	-- only.
	Rotation: number
	-- Tint when Texture-less / image tint when textured / glyph color for Text.
	Color: Color3
	-- 0 (opaque) … 1 (fully transparent).
	Transparency: number
	-- Higher = drawn on top. Defaults to 0.
	ZIndex: number
	-- Hide without destroying. Equivalent to setting Render=false on parts.
	-- DEFAULTS TO FALSE on every freshly-created Primitive — set this to
	-- true once you've finished configuring size / position / image /
	-- text so half-built primitives don't flash on screen.
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
	-- Per-property change signal. Fires with the new value as the only
	-- argument every time `propName` changes — `Position` fires with a
	-- Dim, `Color` with a Color3, `Transparency` with a number, etc.
	-- Lazy: properties no one listens to cost zero.
	function GetPropertyChanged(self, propName: string): Signal<any>
	-- Remove from the scene. Fires "Destroyed" on Changed first.
	function Destroy(self): ()
	-- Custom shader hooks for this primitive's draw call.
	function AttachShader(self, asset: ShaderAsset | FragmentAsset): ()
	function DetachShader(self, asset: ShaderAsset | FragmentAsset): ()
	function ClearShaders(self): ()
	function SetData(self, asset: ShaderAsset | FragmentAsset, name: string, value: number): ()
	function GetData(self, asset: ShaderAsset | FragmentAsset, name: string): number?

	-- ── ClippableUI methods (only valid when Shape == "Clippable") ───────
	-- Register `child` to be clipped to this Clippable's current
	-- Position+Size every frame. Pixels of `child` outside this rect are
	-- cut at the wgpu scissor stage. Reading the Clippable's Size /
	-- Position retroactively re-clips every child on the next snapshot —
	-- you don't need to re-add them. Children added to the wrong shape
	-- raise an error. Last-add-wins for multi-parenting.
	function AddClippable(self, child: Primitive): ()
	-- Remove `child` from this Clippable so it renders unclipped again.
	-- No-op if `child` wasn't added to this specific Clippable.
	function RemoveClippable(self, child: Primitive): ()
end

-- A shader handle bound to either the skybox slot or the post-effect slot.
-- Returned by GUI.SetSkybox / GUI.SetPostEffect; let it drop or call
-- :Destroy() to remove the shader from its slot.
declare class SceneShader
	function SetData(self, name: string, value: number): ()
	function GetData(self, name: string): number?
	function Destroy(self): ()
end

-- =============================================================================
-- DynImg, a CPU-composited "canvas" of GUI primitives that you can apply to a
-- 3D BasePart as its Texture. Each frame the engine re-rasterizes the
-- canvas, so any mutation to the underlying primitives (image, color,
-- transparency, position, size, baked text) shows up on the textured 3D
-- part the next frame.
--
-- Conceptual flow:
--   1. Build GUI primitives the same way you'd build on-screen UI
--      (`GUI.Basic.Image`, `GUI.Basic.Square`, `GUI.Basic.Font`, ...).
--   2. Hand them to `DynImg.New(width, height, { ... })`. The engine
--      flips their `.Visible` to false and tags them as canvas-owned, so
--      they no longer appear on the regular GUI overlay.
--   3. Assign the resulting DynImg as a part's `.Texture`. The 3D
--      part displays the live canvas.
--
-- Each canvas is a fixed-size pixel buffer (width × height RGBA8). Layers
-- are sorted by `Primitive.ZIndex` (lower = drawn first), then alpha-blended.
-- Primitives' `.Position` and `.Size` are read in pixel-space, with
-- (0, 0) at the top-left of the canvas (NOT the screen).
--
-- Limitations of v1:
--   * Image, Square, and Text primitives composite as colored / tinted
--     rectangles. Circle and Triangle render as their bounding rectangle.
--   * Per-primitive shader attachments are NOT applied; canvas
--     primitives can't have their own 2D fragment shader. Use 3D
--     shaders on the consuming BasePart instead.
--   * A single primitive can only belong to ONE DynImg at a time.
-- =============================================================================

declare class DynImg
	Width: number
	Height: number
	IsAlive: boolean

	-- Add an existing GUI primitive to this canvas. Sets the
	-- primitive's `.Visible = false` and routes it into this DynImg
	-- only.
	function Add(self, primitive: Primitive): ()
	-- Remove a primitive from this canvas. Returns true if it was
	-- in the list. Restores the primitive's owner tag, but leaves
	-- `.Visible` at false (set it back yourself if you want it on
	-- the regular GUI again).
	function Remove(self, primitive: Primitive): boolean
	-- Tear down the canvas. Owned primitives stay alive but are
	-- released from this DynImg.
	function Destroy(self): ()
end

-- =============================================================================
-- DrawableImg, a CPU-mutable image you can draw to with primitive shapes.
-- Same accessors as ImageAsset (Width, Height, Source, Pixels) so it's a
-- drop-in for ImageAsset on `BasePart.Texture`. Drawing methods
-- alpha-blend the new pixels over the existing buffer; "Set"-style
-- overwrites are available via `Fill` (whole image) and `WritePixel`
-- (single pixel).
--
-- Coordinate convention: top-left is (0, 0), x increases right, y
-- increases down. All coordinates are integer pixels and clamped to
-- the buffer.
--
-- Lifecycle: not cached. Each `GUI.DrawableImg(...)` produces a fresh
-- instance with its own pixel buffer; once nothing references it
-- (Lua variable + any part using it as a Texture), the buffer is
-- freed.
-- =============================================================================

declare class DrawableImg
	-- Pixel width.
	function Width(self): number
	-- Pixel height.
	function Height(self): number
	-- Origin label, "<drawable:WxH>". Useful for logging.
	function Source(self): string
	-- Raw RGBA8 buffer (Width * Height * 4 bytes). Snapshot of the
	-- current pixels.
	function Pixels(self): string
	function IsAlive(self): boolean

	-- Set the pixel at (x, y) to `color` with the given transparency
	-- (default 0 = opaque). Overwrites whatever was there.
	function WritePixel(self, x: number, y: number, color: Color3, transparency: number?): ()
	-- Alpha-blend a filled rectangle of (w, h) at (x, y). Default
	-- transparency 0 = fully opaque. `DrawCube` is a synonym for
	-- callers who think in 3D-ish terms.
	function DrawRect(self, x: number, y: number, w: number, h: number, color: Color3, transparency: number?): ()
	function DrawCube(self, x: number, y: number, w: number, h: number, color: Color3, transparency: number?): ()
	-- Bresenham line from (x0, y0) to (x1, y1).
	function DrawLine(self, x0: number, y0: number, x1: number, y1: number, color: Color3, transparency: number?): ()
	-- Outline (default) or filled circle of `radius` pixels centered
	-- at (cx, cy). Pass `filled = true` for a solid disc.
	function DrawCircle(self, cx: number, cy: number, radius: number, color: Color3, transparency: number?, filled: boolean?): ()
	-- Overwrite the entire buffer with one color + alpha. Doesn't
	-- blend.
	function Fill(self, color: Color3, transparency: number?): ()
	-- Reset every pixel to (0, 0, 0, 0) — fully transparent black.
	function Clear(self): ()
	-- Alpha-blend a `CanvasBuffer`'s contents onto this drawable at
	-- the given offset (default (0, 0)).
	function Apply(self, buffer: CanvasBuffer, atX: number?, atY: number?): ()
	-- Mark dead. The pixels are zeroed and any consuming part
	-- shows transparent black until you point its Texture
	-- elsewhere.
	function Destroy(self): ()
end

-- =============================================================================
-- CanvasBuffer, a scratch RGBA8 buffer with the same drawing methods as
-- DrawableImg. Mutations stay local until you call :Apply on a
-- DrawableImg. Use this when you want to rebuild a sprite over many
-- draw calls without triggering a texture re-upload per call, do
-- the work in the buffer, then commit once.
-- =============================================================================

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
	-- Alpha-blend this buffer onto `target` at the given offset.
	function Apply(self, target: DrawableImg, atX: number?, atY: number?): ()
end

export type DynImg_API = {
	-- Build a fresh DynImg of the given pixel size. The primitives
	-- list is optional; you can also `:Add` later. Order in the
	-- list doesn't matter, ZIndex is what governs draw order at
	-- composition time.
	New: (width: number, height: number, primitives: { Primitive }?) -> DynImg,
}

-- =============================================================================
-- AnimatedImage — TextureAtlas-driven sprite-sheet animation primitive
-- (returned by `GUI.AnimatedImage(xml, source)`).
--
-- The XML is Sparrow / Flash-style: a `<TextureAtlas>` root with named
-- `<SubTexture>` frames. The XML itself does NOT define animations — it
-- just chops the spritesheet into named pieces. You build animations at
-- runtime via `:CreateTrack(frames, opts)` which gives you a track with
-- its own FPS / Looped / Priority and DidLoop / Ended signals.
--
-- The source can be a static Image asset OR a live DynImg (re-sampled
-- each tick, so procedural / streamed atlases work). frameX / frameY /
-- frameWidth / frameHeight on the SubTexture handle trimmed sprites
-- correctly (transparent halo restored to the original cell size, so
-- frames don't jitter between trimmed sub-textures).
-- =============================================================================

export type AnimatedImageTrack = {
	-- Frames per second this track advances at.
	FPS: number,
	-- When true, the track wraps and fires DidLoop. When false, it
	-- stops at the end and fires Ended.
	Looped: boolean,
	-- Higher priority wins when multiple tracks are Playing
	-- simultaneously. When the high-priority track ends (non-looped)
	-- the next-highest still-Playing track takes over the display.
	-- Default 1.
	Priority: number,
	-- Read-only frame count.
	FrameCount: number,
	-- "Idle", "Playing", "Stopped".
	State: string,
	-- Seconds elapsed in the current play cycle.
	Elapsed: number,
	-- Fires every time a looped track wraps. No args.
	DidLoop: Signal<>,
	-- Fires once when a non-looped track ends. No args. Does not fire
	-- on :Stop().
	Ended: Signal<>,
	-- Reset elapsed to 0 and set state = Playing.
	Play: (self: AnimatedImageTrack) -> (),
	-- Stop the track. No more signal fires.
	Stop: (self: AnimatedImageTrack) -> (),
	-- Pause without resetting (state goes Idle). Resume continues.
	Pause: (self: AnimatedImageTrack) -> (),
	Resume: (self: AnimatedImageTrack) -> (),
	-- Block the current coroutine until Ended fires.
	Wait: (self: AnimatedImageTrack) -> (),
}

export type AnimatedImage = {
	-- Mirror fields of the underlying GUI Image primitive. Resizing
	-- squashes / stretches the currently-displayed frame just like a
	-- regular Image primitive.
	Size: Dim,
	Position: Dim,
	-- 2D rotation in degrees. Applied around the primitive's center.
	Rotation: number,
	ZIndex: number,
	Visible: boolean,
	Color: Color3,
	Transparency: number,
	-- Read-only: total number of `<SubTexture>` frames parsed.
	FrameCount: number,
	Alive: boolean,
	-- Build an AnimatedImageTrack from a sequence of SubTexture names.
	-- `frames` accepts either an array (`{"Down0000", "Down0001"}`) or
	-- a comma-separated string (`"Down0000,Down0001,Down0002"`). Order
	-- is the playback order. Unknown frame names error.
	--
	-- `opts` (optional) is `{ FPS = 12, Looped = true, Priority = 1 }`.
	-- Defaults: FPS = 12, Looped = false, Priority = 1. All three are
	-- live-editable on the returned track.
	--
	-- Each call returns a fresh track with its own signals — useful
	-- for layered animation (a base "idle" and a transient "hurt").
	-- Tracks belong to this AnimatedImage; destroying the image stops
	-- every track.
	CreateTrack: (
		self: AnimatedImage,
		frames: { string } | string,
		opts: { FPS: number?, Looped: boolean?, Priority: number? }?
	) -> AnimatedImageTrack,
	-- Snap the displayed frame to a specific SubTexture (by name) or
	-- by numeric index. Stays visible until a track is :Play()'d on
	-- top (priority blending wins over a manual pin). Useful for
	-- video-game icons that just flip between named static frames.
	JumpToFrame: (self: AnimatedImage, frame: string | number) -> (),
	-- Release the manual frame override.
	ClearJump: (self: AnimatedImage) -> (),
	-- Whether the parsed atlas contains a SubTexture with this name.
	HasFrame: (self: AnimatedImage, name: string) -> boolean,
	-- Every SubTexture name in source order.
	GetFrameNames: (self: AnimatedImage) -> { string },
	-- The underlying GUI Image primitive. Useful for clipping
	-- (`clippable:AddClippable(animimg:GetPrimitive())`) or other
	-- primitive-level ops.
	GetPrimitive: (self: AnimatedImage) -> any,
	-- Destroy the underlying primitive and stop all tracks.
	Destroy: (self: AnimatedImage) -> (),
}

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
		-- Invisible clip container with a pixel-space Position+Size.
		-- Doesn't render itself — its purpose is to be a parent for
		-- children added via `:AddClippable(child)`. Anything inside
		-- those children that falls outside the Clippable's rect is
		-- cut at the GPU scissor stage. Resizing or moving the
		-- Clippable retroactively re-clips every child the next frame,
		-- so it works as a scrolling viewport, masked overlay, draggable
		-- panel, etc. Works on every other primitive type — including
		-- DynImg-backed Image primitives, since they're already a
		-- single textured rectangle by the time clipping runs.
		Clippable: () -> Primitive,
	},

	-- ── DrawableImg / CanvasBuffer ────────────────────────────────────
	-- Build a fixed-size CPU-side mutable image. Has all the standard
	-- image accessors (Width, Height, Source, Pixels) plus drawing
	-- methods (DrawRect / DrawCube, DrawLine, DrawCircle, WritePixel,
	-- Fill, Clear). Each draw call mutates pixels in place and bumps
	-- a version counter; if you've assigned the DrawableImg as a
	-- BasePart's `.Texture`, the texture re-uploads on the next
	-- frame.
	--
	-- DrawableImgs are NOT cached (unlike Asset.GetAsset images). The
	-- pixel buffer lives for as long as you hold the userdata; once
	-- the last reference drops (and any consuming part has cleared
	-- its Texture), the buffer is released.
	DrawableImg: (width: number, height: number) -> DrawableImg,

	-- A scratch RGBA8 buffer with the same drawing methods as
	-- DrawableImg, but the changes don't go anywhere on their own.
	-- When you're done staging, call `buffer:Apply(target, x?, y?)`
	-- (or `target:Apply(buffer, x?, y?)`) to alpha-blend the buffer
	-- onto a DrawableImg. This avoids per-call texture re-uploads
	-- when you're rebuilding a complex sprite, you stage into the
	-- buffer, then commit in one shot.
	CanvasBuffer: (width: number, height: number) -> CanvasBuffer,
	-- Sprite-sheet animation primitive driven by a Sparrow / Flash-style
	-- TextureAtlas XML. The XML defines named frames (SubTextures); you
	-- build animations at runtime by calling `:CreateTrack(frames, opts)`
	-- on the returned AnimatedImage.
	--
	--   <?xml version="1.0" encoding="utf-8"?>
	--   <TextureAtlas imagePath="Test.png">
	--       <SubTexture name="Down0000" x="404" y="0" width="404" height="404"
	--                   frameX="2" frameY="2" frameWidth="400" frameHeight="400"/>
	--       <SubTexture name="Up0000"   x="0"   y="0" width="404" height="404"
	--                   frameX="2" frameY="2" frameWidth="400" frameHeight="400"/>
	--   </TextureAtlas>
	--
	-- Each SubTexture's (x, y, width, height) is the sub-image rectangle
	-- in the source sheet. (frameX, frameY, frameWidth, frameHeight)
	-- restore trimmed sprites to their original cell size so frames
	-- don't jitter between trimmed sub-textures of different bounds.
	--
	-- `source` is an Image asset or a DynImg. DynImg sources are re-read
	-- each tick.
	AnimatedImage: (xml: string, source: any) -> AnimatedImage,

	-- 2D analog of Renderable.EffectVolume. Spawns sprite particles in
	-- screen-space at the configured Position / Size (a 2D Dim region).
	-- Same property surface as the 3D version, minus 3D-only things:
	-- positions and velocities are 2D pixel coordinates, EmissionDirection
	-- is a 2D vector. Particles render in the GUI pass with a ZIndex.
	UIEffectVolume: (image: (ImageAsset | DynImg | DrawableImg)?) -> UIEffectVolume,

	-- 2D analog of GPU.Raycast. GPU-dispatched ray test against every
	-- visible GUI primitive. One compute thread per primitive runs in
	-- parallel:
	--   * Square / Image / Font / Clippable hit-tests as an AABB
	--     (Position .. Position + Size in screen pixels).
	--   * Circle hit-tests as an ellipse inscribed in the AABB.
	--   * Triangle hit-tests as the actual triangle (apex at the top-
	--     middle of the AABB, base across the bottom).
	-- Primitives with Visible = false are skipped. Falls back silently
	-- if the GPU has not been initialized yet (no window opened).
	--
	-- origin / direction are 2D pixel-space (Dim or { X, Y } table).
	-- direction is normalized internally; zero-length errors.
	-- filter visits hits nearest-first; return true to accept, false /
	-- nil to skip. maxDistance caps the ray length (default ~1e6).
	-- Returns nil when nothing was hit.
	Raycast: (
		origin: Dim | { number },
		direction: Dim | { number },
		filter: ((primitive: any) -> boolean)?,
		maxDistance: number?
	) -> { Primitive: any, Distance: number, Position: Dim }?,

	-- Every visible primitive whose AABB overlaps the AABB centered at
	-- `center` with full extents `size` (both in pixels).
	CheckArea: (
		center: Dim | { number },
		size: Dim | { number },
		quality: string?,
		filter: ((primitive: any) -> boolean)?
	) -> { any },

	-- Every visible primitive whose AABB intersects the circle.
	OverlapSphere: (
		center: Dim | { number },
		radius: number,
		filter: ((primitive: any) -> boolean)?
	) -> { any },

	-- Every visible primitive whose AABB intersects the OBB at `center`
	-- with full extents `size`, rotated by `rotation` radians (defaults
	-- to 0). Test is exact OBB-vs-AABB on the GPU.
	OverlapBox: (
		center: Dim | { number },
		size: Dim | { number },
		rotation: number?,
		filter: ((primitive: any) -> boolean)?
	) -> { any },

	-- Every visible primitive whose AABB intersects the current viewport
	-- rect (0, 0) .. (window width, height). 2D analog of the 3D camera
	-- frustum query.
	OverlapFrustum: (filter: ((primitive: any) -> boolean)?) -> { any },

	-- Oriented zone query. `cframe` accepts a CFrame (uses .position.xy
	-- and .rotation.z), a Dim, or a { X, Y } table. `size` is full
	-- extents. Equivalent to OverlapBox on the 2D system since GUI
	-- primitives don't carry sub-quad geometry.
	GetItemsInZone: (
		cframe: CFrame | Dim | { number },
		size: Dim | { number },
		filter: ((primitive: any) -> boolean)?
	) -> { any },
}

-- =============================================================================
-- UIEffectVolume — 2D particle emitter for screen-space effects.
-- =============================================================================

-- 2D vector input. Accepts a Dim or any table { X = .., Y = .. } / { x, y }.
type Vec2Input = Dim | { X: number, Y: number } | { number }

declare class UIEffectVolume
	IsAlive: boolean
	ParticleCount: number
	Enabled: boolean

	-- Emitter region in screen-space pixels. Position is the center,
	-- Size is the full width/height of the spawn box.
	Position: Dim
	Size: Dim
	-- Drawing order against other GUI primitives.
	ZIndex: number

	Image: (ImageAsset | DynImg | DrawableImg)?

	Rate: number
	MaxParticles: number

	Lifetime: NumberRange
	Speed: NumberRange
	ParticleSize: NumberRange
	Rotation: NumberRange
	RotSpeed: NumberRange

	-- 2D pixel-space force (same for every particle).
	Acceleration: Vec2Input
	-- Per-particle randomized force, sampled once at spawn and added to
	-- Acceleration each tick. Pixels/sec². 2D only — Z is silently
	-- ignored if you pass it.
	RandomizeForce: { X: NumberRange?, Y: NumberRange? }
	Drag: number
	-- Cone half-angle in DEGREES around EmissionDirection.
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

	-- ── Skybox ────────────────────────────────────────────────────────
	-- Install a fragment shader that runs across the whole window before
	-- the 3D scene draws. Replaces the flat clear color with whatever the
	-- shader produces, gradient sky, procedural stars, animated clouds,
	-- a screenspace ray-march, anything. The 3D scene draws on top of
	-- it normally, so opaque parts naturally occlude it.
	--
	-- Contract: same FragmentAsset shape as a 2D primitive shader (uses
	-- the RuzitUni / IMG / p(idx) prelude). The shader is run on a
	-- fullscreen quad; in.uv is 0..1 over the window. The texture
	-- binding (IMG) is a 1×1 white default, there's no scene texture to
	-- sample in the skybox pass. Useful uniforms:
	--   U.resolution, window pixel size for aspect-correct math.
	--   U.time, seconds since window opened, for animation.
	--   p(0u)..p(15u), your `@ruzit param` floats, controllable from Lua.
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
	-- pass, every cube, model, GUI primitive, skybox, and shader is
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
	-- ONE crucial difference, IMG is bound to the offscreen scene
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
-- Mouse, cursor position, button state, scroll, lock/visibility.
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
	-- Cursor position in window-local pixels (read-only, write does nothing).
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
	-- Fires (button, state) where state is "Begin" on press and "End" on
	-- release. As of Ruzit 3.2 the legacy `pressed` boolean was dropped,
	-- the State string is now the single source of truth. See the
	-- "Deprecated" section in the Mouse docs for a one-line migration.
	InputReceived: Signal<MouseButton, "Begin" | "End">,
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
-- Keyboard, InputChanged signal + IsKeyDown query.
-- =============================================================================

-- =============================================================================
-- LightingService — thin wrapper around the engine's directional sun + ambient
-- term, plus an opt-in registry for which parts get that lighting applied.
--
-- New BaseParts default to `obj.Lit = false` (flat colored output). Call
-- `LightingService:AddObject(part)` to include a part in engine-managed
-- lighting; the default 3D shader will then apply the sun + ambient term to
-- it. This way, parts that you want to light yourself (custom Frag3D shader,
-- LightSources via SDATA, etc.) aren't fighting the built-in sun.
--
-- Shadow-cast flags are opt-in per BasePart via `obj.CastShadow` and
-- `obj.ReceiveShadow` (both default true). They surface inside user 3D shaders
-- as `I.cast_shadow` / `I.receive_shadow` (u32, 0 or 1) so you can branch on
-- them. The "lit" hint is also exposed as `I.lit` for custom shaders.
-- =============================================================================

-- Live light source created by `LightingService.GenerateLightSource`. Edit
-- the fields freely between frames; the next `UploadLights` call will pick
-- up the changes. Spotlight-only fields are tagged below.
export type LightSource = {
	-- Read-only. "PointLight" or "Spotlight".
	Kind: string,
	-- World pose. The light shines from CFrame.Position. For a Spotlight,
	-- the rotation determines the cone direction (forward = -Z of the
	-- CFrame, same convention as Renderable.Camera).
	CFrame: CFrame,
	-- RGB color of emitted light. Combined with Brightness to produce
	-- the final per-pixel contribution.
	Color: Color3,
	-- Intensity multiplier. 1.0 is roughly "bright office bulb at 1m".
	-- 0 disables the light.
	Brightness: number,
	-- Maximum reach. Pixels beyond Range receive zero contribution.
	Range: number,
	-- Hint flag for shadow casting. Read by shaders as a 0/1 value via
	-- the packed light buffer. The default 3D shader doesn't act on
	-- this yet — custom shaders can.
	CastShadow: boolean,
	-- Distance attenuation exponent. 2.0 is physical inverse-square in
	-- the demo shader (built on `(1 - d / Range)^Falloff`). Lower
	-- values give a softer falloff.
	Falloff: number,
	-- Spotlight-only. Inner cone angle in radians: at angles smaller
	-- than this the light is at full intensity.
	ConeInner: number,
	-- Spotlight-only. Outer cone angle in radians: at angles larger
	-- than this the light is zero. The smoothstep between Inner and
	-- Outer is what gives a soft cone edge.
	ConeOuter: number,
	-- Read-only. False after `:Destroy()` was called.
	Alive: boolean,
	-- Free the light and remove it from the active registry. Next
	-- `UploadLights` won't include it.
	Destroy: (self: LightSource) -> (),
	-- Attach a custom Frag3D shader asset. The shader id is stored on
	-- the light and exposed to user code that wants to render with
	-- per-light shader variants. The default pipeline doesn't dispatch
	-- it — this is data you can read from your own shader / Lua code.
	AttachShader: (self: LightSource, shader: any) -> (),
	-- Remove a previously attached shader.
	DetachShader: (self: LightSource) -> (),
	-- Write one of the light's 16 shader-param floats. Slot is clamped
	-- to 0..15. Read alongside the light data from custom shaders.
	SetShaderParam: (self: LightSource, slot: number, value: number) -> (),
	-- Convenience: world-space forward vector derived from CFrame.Rotation.
	-- For a Spotlight this is the cone's pointing direction.
	Direction: (self: LightSource) -> Vector,
}

-- Options table accepted by `LightingService.GenerateLightSource`. Every
-- field is optional — the defaults give you a white point light at the
-- origin with brightness 1 and range 16.
export type LightSourceOptions = {
	CFrame: CFrame?,
	Color: Color3?,
	Brightness: number?,
	Range: number?,
	CastShadow: boolean?,
	Falloff: number?,
	-- Spotlight-only. Ignored for "PointLight".
	ConeInner: number?,
	-- Spotlight-only. Ignored for "PointLight".
	ConeOuter: number?,
}

export type LightingService_API = {
	-- Set the directional sun vector. `direction` points FROM the sun
	-- TOWARD the scene (so (0, -1, 0) means sun straight overhead).
	-- Optional color tints the sun; pass nil to leave it unchanged.
	SetSun: (direction: Vector, color: Color3?) -> (),
	-- Update sun direction only (keeps current color).
	SetSunDirection: (direction: Vector) -> (),
	-- Update sun color only (keeps current direction).
	SetSunColor: (color: Color3) -> (),
	-- Set the ambient light term added to every lit pixel before the sun
	-- contribution. Default Color3.new(0.25, 0.25, 0.25).
	SetAmbient: (color: Color3) -> (),
	-- Current sun direction.
	GetSunDirection: () -> Vector,
	-- Current sun color.
	GetSunColor: () -> Color3,
	-- Current ambient term.
	GetAmbient: () -> Color3,
	-- Snapshot all three at once. Useful for debug HUDs or saving / loading
	-- a level's lighting state.
	Get: () -> { SunDirection: Vector, SunColor: Color3, Ambient: Color3 },
	-- Mark a BasePart as participating in engine-managed lighting. After
	-- this call, the default 3D shader applies sun + ambient to the part.
	-- New parts default to NOT lit, so you have to opt in explicitly
	-- (this lets you build your own lighting on parts you don't add).
	-- Equivalent to setting `part.Lit = true`.
	AddObject: (self: LightingService_API, part: BasePart) -> (),
	-- Remove a previously-added BasePart from engine-managed lighting.
	-- The default shader will output flat color again. Equivalent to
	-- setting `part.Lit = false`.
	RemoveObject: (self: LightingService_API, part: BasePart) -> (),
	-- Whether the part is currently in engine-managed lighting.
	IsObjectManaged: (self: LightingService_API, part: BasePart) -> boolean,
	-- ── Skybox ─────────────────────────────────────────────────────────
	-- Install a fragment shader that runs across the whole window before
	-- the 3D scene draws. Replaces the flat clear color with whatever the
	-- shader produces (gradient sky, procedural stars, animated clouds,
	-- screenspace ray-march, etc.). The 3D scene draws on top of it.
	-- Returns a SceneShader you keep a reference to so you can update
	-- params at runtime. Calling ClearSkybox or letting the handle drop
	-- both remove the skybox.
	SetSkybox: (asset: ShaderAsset | FragmentAsset) -> SceneShader,
	-- Remove the active skybox; reverts to the flat clear color.
	ClearSkybox: () -> (),
	-- ── Post-processing ────────────────────────────────────────────────
	-- Install a fragment shader that runs ONCE per frame as the final
	-- pass. The composited scene is bound as IMG; sample via
	-- textureSample(IMG, IMG_SAMP, in.uv). Used for color grading,
	-- vignette, CRT, bloom, etc.
	SetPostEffect: (asset: ShaderAsset | FragmentAsset) -> SceneShader,
	-- Remove the active post effect.
	ClearPostEffect: () -> (),
	-- Spawn a new LightSource and add it to the active light registry.
	-- `kind` is "PointLight" or "Spotlight". `opts` is a table of
	-- LightSourceOptions. The returned LightSource is mutable —
	-- assign to .CFrame / .Color / .Brightness etc. between frames
	-- and the next UploadLights call picks up the changes.
	GenerateLightSource: (kind: "PointLight" | "Spotlight", opts: LightSourceOptions?) -> LightSource,
	-- Every alive LightSource as a Lua array.
	GetActiveLights: () -> { LightSource },
	-- Count of alive LightSources. Cheap.
	GetLightCount: () -> number,
	-- Pack the current set of LightSources into a GPU storage buffer.
	-- Returns the number of lights written. Buffer layout (floats):
	--   [0]    bitcast<u32> of light_count
	--   [1..3] padding
	--   [4 + i*16 .. 4 + (i+1)*16 - 1]  16 floats per light:
	--     +0..2  position.xyz       +3   kind (0 = point, 1 = spot)
	--     +4..6  direction.xyz      +7   brightness
	--     +8..10 color.rgb          +11  range
	--     +12    cos(cone_inner)    +13  cos(cone_outer)
	--     +14    falloff            +15  cast_shadow (0/1)
	-- Errors if the buffer is too small. Allocate at least
	-- `4 + maxLights * 16` floats via `GPU.NewBuffer`.
	UploadLights: (buffer: GPUBuffer) -> number,
}

-- =============================================================================
-- TweenService — Rust-side tween/interpolation engine. Tweens are created
-- once from Lua then stepped entirely on the Rust heart loop, so the per-frame
-- interpolation never crosses the Lua VM. The Lua VM only re-enters when a
-- tween's `Completed` signal fires (once per tween, not per frame).
-- =============================================================================

-- Easing family. Pick one of: "Linear", "Sine", "Quad", "Cubic", "Quart",
-- "Quint", "Expo", "Circ", "Back", "Elastic", "Bounce".
export type TweenStyle = string
-- Easing direction. "Out" is the default if omitted / nil.
export type TweenDirection = "In" | "Out" | "InOut"

export type Tween = {
	-- Signal fired when the tween reaches its end. Does not fire when the
	-- tween is :Cancel()'d. Fires inside the heart loop, on the main Lua
	-- thread.
	Completed: Signal<>,
	-- Current state machine label: "Idle", "Playing", "Paused",
	-- "Completed", or "Cancelled".
	State: string,
	-- Seconds elapsed in the current play cycle. Resets when :Play() is
	-- called on a Completed / Cancelled / Idle tween.
	Elapsed: number,
	-- The duration the tween was created with, in seconds.
	Duration: number,
	-- Begin playing. If the tween was previously Completed or Cancelled,
	-- this re-snapshots the target's current property values as the new
	-- start state and runs the easing again. Calling :Play() on a
	-- Playing tween is a no-op.
	Play: (self: Tween) -> (),
	-- Pause stops advancing time but keeps the tween registered. Resume
	-- continues from the same elapsed time.
	Pause: (self: Tween) -> (),
	Resume: (self: Tween) -> (),
	-- Cancel marks the tween as Cancelled. The target's properties stay
	-- wherever they were when Cancel was called (no snap to start or
	-- end). Completed does NOT fire. Calling :Play() afterward re-runs
	-- from the current state.
	Cancel: (self: Tween) -> (),
	-- Block the current coroutine until the tween's Completed signal
	-- fires. If the tween is already Completed or Cancelled, returns
	-- immediately. Must be called from inside a coroutine.
	Wait: (self: Tween) -> (),
}

export type TweenService_API = {
	-- Create a tween. `target` is a tweenable userdata. `duration` is
	-- in seconds. `style` is the easing family ("Linear", "Bounce",
	-- "Cubic", …) and `direction` is the direction prefix ("In", "Out",
	-- "InOut") — both are separate so you don't have to concatenate
	-- them. `direction` defaults to `"Out"` if omitted or nil.
	-- `properties` is a table of `{ propertyName = goalValue }`.
	--
	-- Supported targets and properties:
	--   BasePart        — CFrame (CFrame), Color (Color3), Size (Vector)
	--   DistortionBox   — CFrame (CFrame), Size (Vector)
	--   GUI primitive   — Position (Dim), Size (Dim), Color (Color3),
	--                     Transparency (number), ZIndex (number),
	--                     Rotation (number, degrees)
	--   SFX Sound       — Volume (number), Pitch / Speed (number),
	--                     Pan (number), Distortion (number),
	--                     Position (Vector), MinFalloff (number),
	--                     MaxFalloff (number)
	--   VoiceChannel    — Volume (number), Pitch / Speed (number),
	--                     Position (Vector), MinFalloff (number),
	--                     MaxFalloff (number)
	--   Movable         — CFrame (CFrame, 3D mode) or Position (Dim, 2D
	--                     mode). Children move/rotate by the delta.
	--   Sizable         — Size (Vector for 3D, Dim for 2D). Children
	--                     scale by the ratio.
	--   Billboard       — Position (Vector world-space), Size (Dim
	--                     pixel canvas).
	--   SoundByte
	--    Modifier       — Value, Min, Max (all numbers).
	--    OutputNode     — Volume (number), FalloffMinDistance,
	--                     FalloffMaxDistance, Position (Vector),
	--                     CFrame (CFrame).
	--
	-- The returned Tween starts in "Idle". Call `:Play()` to begin.
	-- Start values are snapshotted at Play time, not at construction
	-- time, so you can build the tween early and play it later from
	-- wherever the target currently is.
	new: (
		target: any,
		duration: number,
		style: TweenStyle,
		direction: TweenDirection?,
		properties: { [string]: any }
	) -> Tween,
	-- Alias for `new` (Roblox-style spelling).
	Create: (
		target: any,
		duration: number,
		style: TweenStyle,
		direction: TweenDirection?,
		properties: { [string]: any }
	) -> Tween,
	-- Single-shot interpolation with an easing curve. Same easing
	-- families as `new` but applied to a one-off `(start, goal, alpha)`
	-- and a value is returned, no Tween object is created. Supported
	-- value types: `number`, `Vector`, `Color3`, `CFrame`, `Dim`. Both
	-- start and goal must be the same type. `style` defaults to
	-- `"Linear"`, `direction` defaults to `"Out"`.
	Lerp: (start: any, goal: any, alpha: number, style: TweenStyle?, direction: TweenDirection?) -> any,
}

export type Keyboard_API = {
	-- Fires with (id: number, name: string, pressed: boolean). `name` is the
	-- key name like "w", "Escape", "Space", "F1", "ArrowUp", etc.
	-- Fires (id, name, state) where state is "Begin" on press and "End"
	-- on release. As of Ruzit 3.2 the legacy `pressed` boolean was
	-- dropped, the State string is now the single source of truth. See
	-- the "Deprecated" section in the Keyboard docs for migration.
	InputChanged: Signal<number, string, "Begin" | "End">,
	-- Snapshot whether a key is currently held. Pass either the name string
	-- ("w", "Space") or the numeric id you got from InputChanged / GetKeyId.
	-- Numeric ids are stable across runs and faster to compare.
	IsKeyDown: (self: Keyboard_API, key: string | number) -> boolean,
	-- Resolve a name string to its stable numeric id (FNV-1a-32).
	GetKeyId: (self: Keyboard_API, name: string) -> number,
}

-- =============================================================================
-- Voice, microphone capture + per-peer playback (Opus over your transport).
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
	-- Modifier properties. Same shape as SFX.Sound's, but only the portable
	-- subset that the Voice mixer supports (no per-channel Lowpass / Echo /
	-- etc.). Assigning replaces the prior value (no stacking).
	-- Linear gain. 1.0 unchanged, 0 silence, >1 boost.
	Volume: number
	-- Time + pitch. Clamps to 0.25..4.
	Speed: number
	-- Alias for Speed.
	Pitch: number
	-- 3D world anchor. Set to nil to revert to non-positional.
	Position: Vector?
	-- Inner / outer falloff radii. Distances ≤ MinFalloff play at full
	-- volume; distances ≥ MaxFalloff are silent; smooth roll-off in
	-- between. Defaults: 0 / 8.
	MinFalloff: number
	MaxFalloff: number
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
	-- Convenience setter; equivalent to assigning `Position`. Pass nil (or
	-- no args) to clear.
	function SetPosition(self, x: number?, y: number?, z: number?): ()
	-- Same idea but also installs/replaces the Spatial shader with this
	-- distance falloff in one call. Pass nil to clear the spatial shader.
	function SetSpatial(self, x: number?, y: number?, z: number?, max_falloff: number?): ()
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
	-- ~50 packets/sec pacing. Returns immediately, playback is dripped
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
	PlayPacket: (packet: string, opts: { x: number?, y: number?, z: number?, falloff: number?, min_falloff: number? }?) -> VoiceChannel,
	-- Empty packet collector. Hook to mic.OnPacket OR to received-from-peer
	-- packets, each :Push(bytes) appends to the recorded stream. Save it
	-- with :Serialize() when done.
	NewRecorder: () -> VoiceRecorder,
	-- Inverse of Recorder:Serialize(). Returns a replayable Recording.
	LoadRecording: (data: string) -> VoiceRecording,
	-- One-shot mic recording. Opens the default input, captures Opus frames
	-- for `duration` seconds, stops automatically, and fires the returned
	-- Signal once with (recording, serialized).
	--   * `recording` is a VoiceRecording you can pipe straight into a
	--     channel via `recording:PlayInto(channel)`, or send to
	--     Voice.PlayRecording for a fire-and-forget channel.
	--   * `serialized` is the same recording as bytes (a string) — use this
	--     to send over the network, write to disk, or stash for later
	--     replay via Voice.LoadRecording.
	-- Duration is clamped to 1 hour. The returned Signal fires at most once.
	Record: (duration: number) -> Signal<VoiceRecording, string>,
	-- Fire-and-forget recording playback. Accepts either a VoiceRecording
	-- handle (e.g. from Voice.Record / NewRecorder:ToRecording) OR a
	-- serialized string (e.g. from Recorder:Serialize / Voice.Record's
	-- second arg). Spins up a transient VoiceChannel, plays the recording,
	-- and returns the channel so you can apply shaders / stop early /
	-- watch IsPlaying. `loop` repeats forever; `speed` (0.25..4) re-times.
	PlayRecording: (
		data: string | VoiceRecording,
		opts: { ["loop"]: boolean?, speed: number? }?
	) -> VoiceChannel,
	-- Built-in shaders. `Volume(0.5)` halves loudness, `Speed(1.2)` makes
	-- them sound 20% faster, `Spatial(x, y, z)` pans + attenuates by
	-- distance from the listener (the active Renderable.Camera, or set
	-- explicitly via Voice.ListenerPosition).
	Volume: (factor: number) -> VoiceShader,
	Speed: (factor: number) -> VoiceShader,
	Spatial: (x: number, y: number, z: number, max_falloff: number?) -> VoiceShader,
	-- Override the listener (defaults to the active Renderable.Camera).
	-- Read by Spatial shaders. Pass (0, 0, 0) to revert to camera-tracking.
	ListenerPosition: (x: number, y: number, z: number) -> (),
}

-- =============================================================================
-- Steam, Steamworks integration. User, Friends, Achievements, Stats, Lobby,
-- Server, Overlay, App/Utils, Rich Presence, Cloud, Workshop, Remote Play.
-- All requires the Steam runtime to be present and the right app id to be
-- set in build.toml's [steam] table (or RUZIT_STEAM_APPID env var).
-- =============================================================================

declare class SteamLobby
	-- The lobby's Steam id (cast as a decimal string).
	ID: string
	-- The lobby owner's Steam id.
	Owner: string
	-- True if the local user owns this lobby.
	IsOwner: boolean
	-- True if the lobby has been closed (set by the owner via :Close()).
	IsClosed: boolean
	-- Fires with (memberSteamId: string).
	OnMemberJoined: Signal<string>
	-- Fires with (memberSteamId: string).
	OnMemberLeft: Signal<string>
	-- Fires with (memberSteamId: string?). Nil when lobby-wide data changed,
	-- otherwise the id of the member whose per-member data changed.
	OnDataUpdate: Signal<string?>
	-- Fires when the LOCAL user is involuntarily removed from the lobby —
	-- i.e. lost network connection, was kicked, or was banned. Does NOT
	-- fire when you call :Leave() yourself. Argument is the reason:
	-- "Disconnected" | "Kicked" | "Banned". After this fires the lobby
	-- handle is inert; create or join a new lobby to keep playing.
	OnDisconnected: Signal<SteamDisconnectReason>
	-- Fires when the lobby has been gracefully closed by the owner via
	-- :Close(). Fires on every member (including the owner). Use this to
	-- show "host ended the lobby" UI. After this fires the local user has
	-- already auto-left and the lobby handle is inert.
	OnClose: Signal<>
	-- Fires when the LOCAL user has been kicked by the lobby owner via
	-- owner:Kick(myId). Argument is the kicker's Steam id (usually the
	-- lobby owner). Use this to show a "you were kicked" popup. After
	-- this fires the local user has already auto-left and the lobby
	-- handle is inert. Does NOT fire if you :Leave() yourself.
	OnKick: Signal<string>
	-- Fires when another lobby member sends a P2P packet via :Send or
	-- :Broadcast. Args: (senderSteamId: string, bytes: string). The
	-- bytes are the exact payload the sender passed in — Ruzit doesn't
	-- impose any framing or serialization on top. P2P uses Steam's
	-- relay-network: NAT-friendly, encrypted, identified by SteamID
	-- (no IPs ever exposed). Up to ~1 MB per message; reliable channel
	-- maintains ordering and won't drop, unreliable may drop and
	-- reorder. Auto-routed: messages from peers who aren't a current
	-- member of any lobby you're in are silently dropped.
	OnMessage: Signal<string, string>
	-- Snapshot of every member's Steam id.
	function Members(self): { string }
	-- Lobby-wide key/value store. Visible to everyone in the lobby.
	-- Keys starting with "_ruzit_" are reserved by the engine and will
	-- raise an error.
	function SetData(self, key: string, value: string): ()
	function GetData(self, key: string): string?
	-- Delete a key from the lobby-wide store. Returns true if it existed.
	function DeleteData(self, key: string): boolean
	-- Snapshot of every visible lobby-wide key/value pair (engine-reserved
	-- "_ruzit_" keys are filtered out).
	function GetAllData(self): { [string]: string }
	-- Configured maximum member count, or nil if Steam hasn't pushed the
	-- value yet (you can also see this in SteamLobbyInfo from List()).
	function MemberLimit(self): number?
	-- Broadcast a chat message to every member.
	function SendChat(self, message: string): ()
	-- Owner-only. Toggle whether other users can join via Lobby.Join.
	-- Returns true on success.
	function SetJoinable(self, joinable: boolean): boolean
	-- Owner-only. Politely close the lobby:
	--   1. Mark the lobby unjoinable.
	--   2. Broadcast a "closed" flag every member observes.
	--   3. Fire OnClose on every member (including the owner).
	--   4. Auto-leave for every member (including the owner).
	-- After this, the lobby handle is inert. Use this instead of :Leave()
	-- when the host wants to end the session for everyone.
	function Close(self): ()
	-- Owner-only. Kick a member by Steam id. The targeted member's client
	-- fires OnKick and auto-leaves. Other members observe the leave via
	-- OnMemberLeft. Cannot kick yourself — use :Close() or :Leave() instead.
	function Kick(self, userId: string): ()
	-- Open Steam's invite-friends overlay for this lobby.
	function ShowInviteDialog(self): ()
	-- P2P send to one specific lobby member. Uses Steam Networking
	-- Sockets (relay-fallback, encrypted, NAT-friendly). The connection
	-- is established lazily on the first send to that peer.
	-- `data` is raw bytes — anything you can serialize into a string
	-- (JSON, msgpack, your own format). `reliability` is "Reliable"
	-- (default — ordered, no-drop, like TCP) or "Unreliable" (fast,
	-- lossy, no order — best for hot-path packets like positions).
	-- "ReliableNoNagle" / "UnreliableNoDelay" disable the Nagle
	-- buffering for latency-sensitive RPCs.
	-- The peer must be a current member of this lobby; raises
	-- otherwise. Max payload ~1 MB.
	function Send(
		self,
		memberId: string,
		data: string | buffer,
		reliability: SteamReliability?
	): ()
	-- P2P broadcast to every other member of this lobby. Equivalent to
	-- iterating :Members() and calling :Send on each (skipping yourself).
	-- Same reliability semantics as :Send.
	function Broadcast(self, data: string | buffer, reliability: SteamReliability?): ()
	-- Leave the lobby. The handle goes inert after this. Use :Close() if
	-- you're the owner and want every member to leave.
	function Leave(self): ()
end

declare class SteamServer
	-- The server's Steam id.
	ID: string
	-- Fires with (clientSteamId: string) when a client successfully authenticates.
	OnClientAuthed: Signal<string>
	-- Fires with (reason: string) when this hosted server's connection to the
	-- Steam backend is lost. The server is no longer reachable through Steam
	-- matchmaking until OnConnected fires again.
	OnDisconnected: Signal<string>
	-- Fires with no arguments when the hosted server (re)establishes its
	-- connection to the Steam backend.
	OnConnected: Signal<>
	-- Fires with (reason: string, stillRetrying: boolean) when the hosted
	-- server fails to reach the Steam backend. If stillRetrying is true,
	-- Steam will keep attempting; otherwise the server has given up.
	OnConnectFailure: Signal<string, boolean>
	-- Fires with no arguments when the server is being shut down via :Close()
	-- or :Stop(). Use this to clean up moderation/scoreboard state.
	OnClose: Signal<>
	-- Server-list metadata. Players see this in the in-game / web server browser.
	function SetServerName(self, name: string): ()
	function SetMapName(self, map: string): ()
	function SetMaxPlayers(self, max: number): ()
	-- Anonymous logon, no game-server-account credentials required.
	function LogOnAnonymous(self): ()
	-- Revoke a connected client's auth ticket. The Steam backend will no
	-- longer regard the user as authenticated; your game's networking
	-- layer should also drop the underlying socket / NetConnection.
	function Kick(self, userId: string): ()
	-- Gracefully shut down the server. Fires OnClose, then tears down the
	-- Steam server instance. Identical to :Stop(); use whichever name reads
	-- better in your code. Idempotent.
	function Close(self): ()
	-- Stop and clean up the server. Fires OnClose. Idempotent.
	function Stop(self): ()
end

export type SteamPersonaState = "Offline" | "Online" | "Busy" | "Away" | "Snooze" | "LookingToTrade" | "LookingToPlay"
-- Reason the local user was removed from a SteamLobby.
-- "Disconnected" — network drop / Steam client lost connection.
-- "Kicked"       — owner removed you.
-- "Banned"       — owner kicked AND banned you (rejoining will fail).
export type SteamDisconnectReason = "Disconnected" | "Kicked" | "Banned"
-- Reliability tier for SteamLobby:Send / :Broadcast.
-- "Reliable"          — ordered, no-loss, like TCP. Default.
-- "Unreliable"        — fast, lossy, no ordering. Best for positions / inputs.
-- "ReliableNoNagle"   — reliable but flush immediately (no batching). Use for
--                        latency-sensitive RPCs where you don't want to wait
--                        for the Nagle timer to coalesce small writes.
-- "UnreliableNoDelay" — unreliable AND don't queue if the connection isn't
--                        ready. Drops the message instead of buffering.
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
	-- Live member count (0 means lobby ended).
	MemberCount: number?,
	-- Owner's Steam id. Useful for filtering Lobby.List results to
	-- friends-only by intersecting with Steam.User.GetFriends().
	Owner: string?,
	-- Convenience read of lobby data key "name" (set via SetData("name", ...)).
	-- Use a unique value (e.g. your game id) to filter your lobbies out
	-- of public AppID test traffic.
	Name: string?,
	-- Full lobby-wide key/value snapshot, minus engine-reserved
	-- "_ruzit_*" keys. Use this to read whatever metadata the host set
	-- (e.g. host_id, map, mode) without having to actually Join.
	Data: { [string]: string }?,
}

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
	-- True if the Steam SDK redistributable (steam_api64.dll on Windows,
	-- libsteam_api.so on Linux, libsteam_api.dylib on macOS) is sitting
	-- next to the executable. Use `ruzit fetch-deps` to drop it in.
	-- The AppID (from your build.toml [steam] table, or 480 if unset)
	-- is baked into the runtime — at SDK init the runtime briefly
	-- writes `steam_appid.txt` next to the exe, lets Steam SDK read
	-- it, then immediately deletes it, so the game can boot without
	-- being launched through the Steam client and without leaving
	-- a file on disk. The user's Steam client must still be running
	-- and logged in. Actual Steam API calls may still fail (Steam not
	-- running, user doesn't own the AppID, etc.); each API call
	-- returns / errors accordingly.
	-- Reading is free; gate every Steam call behind this flag so the
	-- game still runs cleanly when the DLL is absent:
	--   if Steam.SteamPresent then
	--       Steam.RichPresence.Set(...)
	--   end
	SteamPresent: boolean,
	-- Fires with (reason: string) when the local Steam client loses its
	-- connection to the Steam backend. Affects EVERY Steam-backed feature
	-- (lobbies, matchmaking, friends, achievements/stat sync, cloud, etc.)
	-- — gracefully pause networking until OnConnected fires.
	OnDisconnected: Signal<string>,
	-- Fires with no arguments when the local Steam client (re)connects to
	-- the Steam backend after a disconnect.
	OnConnected: Signal<>,
	-- Fires with (reason: string, stillRetrying: boolean) when the Steam
	-- client fails to reach the backend. If stillRetrying is true, Steam
	-- will keep attempting; otherwise it has given up.
	OnConnectFailure: Signal<string, boolean>,
	-- Force-close every P2P connection and the listen socket. Normally
	-- you don't need this — connections clean up automatically on lobby
	-- close/leave/disconnect, and the engine calls Shutdown for you when
	-- the heart loop exits. Call manually if you're about to do an
	-- abrupt exit (os.exit, panic, etc.) and want clean Steam teardown
	-- with no "sockets still open" warnings.
	Shutdown: () -> (),
	-- Fires with (lobbyId: string, friendId: string) when a friend
	-- accepts a Steam invite to YOUR game while you're already running.
	-- Steam doesn't auto-join — you decide what to do (auto-join, prompt
	-- the user, etc.). Typical handler:
	--     Steam.OnLobbyJoinRequested:Connect(function(lobbyId)
	--         Steam.Lobby.Join(lobbyId):Connect(function(lobby)
	--             -- ...switch to lobby UI / start match...
	--         end)
	--     end)
	-- For cold launches (game wasn't running when the invite was clicked),
	-- Steam puts "+connect_lobby <id>" in the launch command line — read it
	-- via Steam.Launch.GetQueryParam("connect_lobby") at startup.
	OnLobbyJoinRequested: Signal<string, string>,
	-- Bring up Valve's relay-network for P2P (cross-NAT punch-through,
	-- relay fallback when direct fails). Call ONCE at startup if your
	-- game ships to players on different networks. Skip it for local
	-- LAN / same-machine testing — direct P2P works without it, and
	-- skipping avoids spinning up the relay sockets (which Steam's SDK
	-- can't fully tear down before process exit, leading to the
	-- "still have sockets open" shutdown warning). Costs a few seconds
	-- on init while the SDK pings relay servers.
	InitRelayNetwork: () -> (),
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
		-- in the same lobby, otherwise returns nil. For arbitrary Steam
		-- IDs use GetAvatarAsync.
		GetFriendAvatar: (id: string, size: SteamAvatarSize?) -> ImageAsset?,
		-- Async lookup that works for ANY Steam ID, not just friends. Hits
		-- cache instantly when the avatar is already known; otherwise asks
		-- Steam for the user's profile data and fires the signal when it
		-- arrives (a frame or two later). Signal fires once with
		-- (ImageAsset?), nil if Steam couldn't load the user.
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
		-- Local install info, nil while not installed.
		InstallInfo: (id: string) -> { Folder: string, SizeOnDisk: number, Timestamp: number }?,
		-- Live download progress, nil if not downloading.
		DownloadProgress: (id: string) -> { Downloaded: number, Total: number }?,
		-- Force-trigger a download. `highPriority` jumps the Steam queue.
		DownloadItem: (id: string, highPriority: boolean?) -> boolean,
		-- Combined fetch: returns a table with state flags AND install info if
		-- the item is on disk, plus a Files() helper that lists every file
		-- in the install folder (absolute paths, pass them straight to
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

	-- Launch metadata: how the game got started, what arguments came in,
	-- what beta branch the user is on, etc. Useful for game-side handling
	-- of "Join Game" rich-presence links and for branching on dev vs.
	-- shipped runs.
	Launch: {
		-- Steam URL launch parameters (e.g. "+connect 1.2.3.4 +map dust"
		-- when Steam invoked via steam://run/<appid>//+connect ...). Empty
		-- string when the game wasn't launched via a Steam URL.
		GetCommandLine: () -> string,
		-- Pull a single +key value pair out of the Steam launch command
		-- line. Returns nil if the key isn't present. Case-insensitive.
		GetQueryParam: (key: string) -> string?,
		-- Beta branch the user is playing on, or nil for the public branch.
		GetCurrentBeta: () -> string?,
		-- Aggregate run metadata. Source is one of:
		--   "steam-url"  : came in via a steam:// deep link
		--   "steam"      : launched normally from the Steam client
		--   "direct"     : ran the executable directly (dev / debug / no Steam)
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

-- =============================================================================
-- Gamepad, XInput / DirectInput / udev / IOKit / SDL2 wrapped via gilrs.
-- =============================================================================
-- Pads are identified by an integer Id. Layout names are normalised across
-- platforms: face buttons are South / East / North / West (Xbox A/B/Y/X,
-- PlayStation Cross/Circle/Triangle/Square); shoulders are LeftBumper /
-- RightBumper / LeftTrigger / RightTrigger; sticks are LeftStick* /
-- RightStick* with X/Y axes and an optional click (LeftStick / RightStick
-- as buttons). Trigger pulls also surface as analog buttons via the Analog
-- kind in InputChanged.

export type GamepadInfo = {
	Id: number,
	Name: string,
	Connected: boolean,
	UUID: string?,
}

export type GamepadInputEvent = {
	Pad: number,
	-- "Button", digital press / release (Value is 0 or 1)
	-- "Analog", partial trigger pull (Value 0..1)
	-- "Axis", stick / dpad axis (Value -1..1 for sticks, 0..1 for triggers)
	Kind: "Button" | "Analog" | "Axis",
	Name: string,
	Value: number,
	-- Begin/End state. "Begin" on a press / trigger crossing 0.5,
	-- "End" on a release / trigger falling below 0.5, "Change" for
	-- analog axis updates (sticks, dpads) where there's no
	-- press/release semantics.
	State: "Begin" | "End" | "Change",
}

export type Gamepad_API = {
	-- Enumerate every connected pad. Cheap (cached state), safe to call
	-- every frame from a settings UI.
	GetGamepads: () -> { GamepadInfo },
	IsButtonDown: (pad: number, name: string) -> boolean,
	GetAxis: (pad: number, name: string) -> number,

	-- Force-feedback. low and high are 0..1 motor intensities (low =
	-- strong / low-frequency rumble, high = weak / high-frequency buzz);
	-- duration is in seconds. Returns true when the rumble was actually
	-- scheduled, XInput pads on Windows always support FF; Bluetooth-
	-- paired controllers and most pads outside Windows often don't, so
	-- treat the bool as a "did the haptic actually fire" probe.
	SetVibration: (pad: number, low: number, high: number, duration: number) -> boolean,
	-- Convenience wrapper: drives both motors at the same intensity.
	-- `gamepad.Vibrate(padId, 0.5, 0.1)` is a 100ms half-strength buzz.
	Vibrate: (pad: number, intensity: number, duration: number) -> boolean,
	-- Cancel any pending rumble effects. The `pad` argument is accepted
	-- for symmetry but is ignored, gilrs only tracks effects globally.
	StopVibration: (pad: number?) -> (),
	-- Battery charge level for the given pad (0..1) or nil if the
	-- driver doesn't report it (wired pads, generic HID gamepads).
	GetBatteryLevel: (pad: number) -> number?,

	Connected: Signal<number>, -- pad id
	Disconnected: Signal<number>,
	InputChanged: Signal<GamepadInputEvent>,
}

-- =============================================================================
-- VR, head-mounted display + motion controllers.
-- =============================================================================
-- Two-step lifecycle: detect, then link.
--
--   local VR = import("VR")
--   if not VR.IsVrPresent() then return end
--   local cam = VR.LinkVRView()  -- engine camera now follows BodyCframe + HeadCframe
--   cam.BodyCframe = CFrame.new(Vector.new(0, 1.6, 0), Vector.new(0, 0, 0))
--   cam.HeadMoved:Connect(function(cf) ... end)
--   local pads = cam:GetControllers()
--   pads.Right.OnInput:Connect(function(name, value, state)
--       -- state is "Begin" or "End"; name is "Trigger" / "Grip" / "A" / etc.
--   end)
--   ...
--   cam:Unlink()
--
-- Coordinate space: BodyCframe is world-relative, it's where the player's
-- VR rig sits in the scene. HeadCframe is the headset pose; the engine
-- composes Body ∘ Head onto the active 3D camera each frame so existing
-- BasePart / Renderable code keeps working without any VR-aware changes.
--
-- GUI: 2D primitives created via the GUI library stay screen-locked in VR
-- mode, they render at the same monitor positions they would on a normal
-- window, which is the right default for HUDs and menus.

export type VRController = {
	-- "Left" or "Right", which hand this controller represents.
	Side: "Left" | "Right",
	-- Current world-space pose of the controller. Read-only, driven by
	-- the runtime each frame.
	CFrame: CFrame,
	-- World-space linear velocity in metres per second. Useful for
	-- throwing physics: at the moment of release, multiply by mass and
	-- apply as initial velocity.
	Velocity: Vector,
	-- Angular velocity in degrees per second around X / Y / Z.
	AngularVelocity: Vector,
	-- True while the runtime reports the controller as tracked + paired.
	-- Goes false on disconnect, runtime suspend, or when the user holsters
	-- a hand-tracked controller.
	IsConnected: boolean,
	-- 0..1 battery charge, or nil if the runtime doesn't report it
	-- (wired devices, hand tracking, etc.).
	BatteryLevel: number?,
	-- Cached input axes from the most recent OnInput event. Reading these
	-- is the polling-style alternative to subscribing to OnInput, useful
	-- for "while held" loops driven from RunService.
	Trigger: number, -- 0..1
	Grip: number, -- 0..1
	Thumbstick: Vector, -- x = -1..1 horizontal, y = -1..1 vertical, z = 0
	-- Fires when the runtime reports a pose update. Signal payload is the
	-- new CFrame.
	Moved: Signal<CFrame>,
	-- Per-controller input events. Fires (name, value, state) where:
	--   name  is the input identifier ("Trigger", "Grip", "A", "B",
	--         "ThumbstickX", "ThumbstickY", "Menu", etc.)
	--   value is 0..1 for analog buttons / triggers, -1..1 for stick axes,
	--         and 0 or 1 for digital buttons.
	--   state is "Begin" when the input crossed into the active range
	--         (button press / trigger past half-pull) and "End" when it
	--         left the active range (release / trigger below half-pull).
	OnInput: Signal<string, number, "Begin" | "End">,

	-- Fire a haptic pulse. duration is in seconds; frequency (Hz) and
	-- amplitude (0..1) default to a "click" pulse if omitted. Returns
	-- true when the pulse was scheduled, false on devices that don't
	-- support haptics (some hand-tracked controllers, some cheaper
	-- 3DoF pads). Multiple overlapping calls layer additively.
	Vibrate: (
		self: VRController,
		duration: number,
		frequency: number?,
		amplitude: number?
	) -> boolean,
	-- Cancel any pulses currently playing on this controller.
	StopVibration: (self: VRController) -> (),
}

export type VRPlayArea = {
	-- Width / Depth in metres. Both 0 when the runtime hasn't reported a
	-- play area (seated mode, runtime not calibrated, hand-tracking-only
	-- session).
	Width: number,
	Depth: number,
}

export type VRCamera = {
	-- Player body pose in the world. Writable, set this to teleport,
	-- snap-turn, or otherwise move the VR rig around. The engine adds
	-- HeadCframe on top each frame so the user's actual head movement is
	-- preserved on top of whatever you script.
	BodyCframe: CFrame,
	-- Headset pose. Read-only, writing this would let your code lie to
	-- itself about where the headset really is.
	HeadCframe: CFrame,
	-- True while LinkVRView() is active. Becomes false after Unlink().
	IsLinked: boolean,
	-- Detected VR runtime: "SteamVR" / "Oculus" / "OpenXR" / "Unknown".
	-- Read-only, set at link time.
	RuntimeName: string,
	-- Headset refresh rate in Hz (90 by default; the runtime usually
	-- reports 72/90/120/144 depending on device + user settings).
	RefreshRate: number,
	-- Proximity-sensor state: true while the headset is on the user's
	-- head, false otherwise. Most modern HMDs have this; older ones
	-- always report true.
	IsHeadsetWorn: boolean,
	-- Interpupillary distance in metres. Default 0.063 (human average).
	IPD: number,

	-- Fires every time the runtime reports a head pose update. Payload
	-- is the new HeadCframe.
	HeadMoved: Signal<CFrame>,
	-- Fires when the headset connects, disconnects, or the proximity
	-- sensor changes state. Payload: a "Connected" / "Disconnected" /
	-- "Worn" / "Removed" string.
	Connected: Signal<string>,

	-- Release the engine camera back to user-driven control and tear
	-- down the VR session. Idempotent, calling on an already-unlinked
	-- camera is a no-op.
	Unlink: (self: VRCamera) -> (),
	-- Re-anchor the play space so the user's current head pose becomes
	-- the origin facing +Z. Common after a teleport or seated/standing
	-- transition.
	Recenter: (self: VRCamera) -> (),
	-- World-space pose of either eye, derived from HeadCframe + IPD.
	-- Useful for stereo-aware effects (depth-of-field, lens distortion
	-- previews), the engine handles eye-buffer rendering itself.
	GetEyePose: (self: VRCamera, side: "Left" | "Right") -> CFrame,
	-- Chaperone / guardian play-space dimensions in metres. (0, 0) when
	-- the runtime hasn't reported a play area.
	GetPlayArea: (self: VRCamera) -> VRPlayArea,
	-- Fade the headset view to/from a solid colour over `duration`
	-- seconds. Useful for teleport transitions and motion-sickness
	-- reduction. Pass duration < 0 to interrupt an in-flight fade.
	Fade: (self: VRCamera, color: Color3, duration: number) -> (),
	-- Fetch the pair of controller handles. Returns the same logical
	-- controllers on every call (the userdata is freshly allocated but
	-- the underlying state is shared) so signals connected on one
	-- handle still fire when the runtime reports input on the same
	-- controller.
	GetControllers: (self: VRCamera) -> {
		Left: VRController,
		Right: VRController,
	},
}

export type VR_API = {
	-- True if a VR runtime (SteamVR, OpenXR, etc.) is installed on this
	-- host. Does NOT require a headset to be currently plugged in,
	-- it's the "should we even offer the VR option in the menu" check.
	IsVrPresent: () -> boolean,
	-- Hand the engine camera over to the VR pipeline and return a
	-- VRCamera handle. Calling LinkVRView() while already linked is
	-- safe and returns a fresh handle observing the same state.
	LinkVRView: () -> VRCamera,
}

-- =============================================================================
-- GPU, info about the active graphics adapter, frame stats, and a CPU-side
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
	-- Extra driver detail (build, date, etc.), may be empty.
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
	-- per allocation", actual VRAM is usually larger but split across many
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
	--, calling repeatedly is free. Useful for logging, settings menus, and
	-- gating high-cost features behind DeviceType == "DiscreteGpu".
	Info: () -> GPUInfo,
	-- Hard limits the active GPU device reported. wgpu doesn't expose total
	-- VRAM portably across DX12/Vulkan/Metal, MaxBufferSize is the closest
	-- single number you can rely on for "biggest thing I can allocate".
	Limits: () -> GPULimits,
	-- FPS / frame time / scene part count. Updated once per heart tick.
	-- Cheap; safe to poll every frame for a debug overlay.
	FrameStats: () -> GPUFrameStats,
	-- CPU-side ray test against every renderable BasePart in the scene.
	--
	--   origin      : world-space ray start (Vector).
	--   direction   : world-space ray direction (Vector). Length doesn't
	--                 matter, Ruzit normalizes it. Zero-length errors.
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
	-- Geometry: dispatched on the GPU as a compute pass, one thread per
	-- part. Cubes are tested as oriented bounding boxes (CFrame + Size).
	-- Spheres are tested as ellipsoids (CFrame + Size). Models with mesh
	-- data are tested per-triangle via Möller-Trumbore in part-local
	-- space, so corners and concavities hit precisely instead of the
	-- bounding box. Models without mesh data fall back to OBB. Parts
	-- with Render = false or IgnoreInRaycast = true are skipped. Falls
	-- back to a CPU loop if the GPU is not yet initialized.
	--
	-- Example, pick a part under the mouse, ignoring red ones:
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
	-- Returns (origin, direction), feed straight into GPU.Raycast.
	-- Uses the active Renderable.Camera as the eye + projection.
	ScreenToRay: (point: Dim) -> (Vector, Vector),
	-- Project a world-space point onto the screen. Returns:
	--   (Dim, false), point is in front of the camera; Dim is the pixel.
	--   (nil, true), point is at or behind the near plane; no projection.
	-- Useful for billboards, world-anchored UI, off-screen indicators.
	WorldToScreen: (world: Vector) -> (Dim?, boolean),
	-- Allocate a GPU storage buffer of `size` floats. Memory lives on the
	-- GPU and is read-only inside shaders, available as `SDATA: array<f32>`
	-- at @group(0) @binding(4) in any 3D fragment shader once you call
	-- GPU.SetBuffer. The buffer is freed when the GPUBuffer userdata is
	-- garbage-collected.
	--
	-- Example:
	--   local lut = GPU.NewBuffer(256 * 3)
	--   for i = 0, 255 do
	--       local t = i / 255
	--       lut:Write(i * 3, { t, t * 0.5, 1.0 - t })
	--   end
	--   GPU.SetBuffer(lut)
	NewBuffer: (size: number) -> GPUBuffer,
	-- Bind a GPUBuffer as the active SDATA storage. Cheap to swap between
	-- frames, bind groups are rebuilt only when the binding identity
	-- changes. Until cleared, every 3D shader sees this buffer's data.
	SetBuffer: (buffer: GPUBuffer) -> (),
	-- Unbind the active storage buffer. SDATA reverts to a 1-float stub
	-- so shaders that don't reference it keep working.
	ClearBuffer: () -> (),
	-- Returns one or more world-space AABBs covering a BasePart. `quality`
	-- controls how many boxes you get back:
	--   1 (default): a single box that encloses the entire part. Cheapest;
	--                works for any shape (cube, sphere, model, deformed,
	--                animated). Always accurate for cubes/spheres.
	--   2+        : split a model's vertices into N bins along the longest
	--                axis and return a tight AABB per bin. Useful for tighter
	--                collision approximations on long thin meshes (a sword,
	--                a fence, a tunnel). Cubes / spheres always return one
	--                box regardless of quality.
	-- Each entry is { Position = Vector (center), Size = Vector (extents),
	-- Min = Vector, Max = Vector }. Honors deformations and current
	-- animation state, so the returned AABBs follow the live mesh.
	-- Cap or uncap the heart-loop tick rate. Pass nil (or 0) to remove
	-- the cap and run as fast as the system allows; pass a positive
	-- number to throttle to that frames-per-second target. The
	-- engine ships UNCAPPED by default — frames run as fast as
	-- the renderer + game logic allow, bounded only by the swap-
	-- chain present mode (see SetVSync). The cap is applied by
	-- sleeping at the end of each tick if the frame finished
	-- early, so it has no effect on a system that can't keep up.
	-- Cheap to call repeatedly, atomic store, can be flipped from
	-- a settings menu live.
	SetMaxFrameRate: (fps: number?) -> (),
	-- Read the current cap. Returns nil when uncapped.
	GetMaxFrameRate: () -> number?,
	-- Toggle vertical sync on the swapchain. Default is OFF —
	-- the engine prefers Mailbox (uncapped, tear-free) when the
	-- driver supports it, falling back to Immediate, then
	-- FifoRelaxed, then Fifo. Turning VSync on hard-caps presents
	-- to the monitor's refresh rate (Fifo / FifoRelaxed). Useful
	-- for laptop battery life or to eliminate tearing on monitors
	-- that don't have a tear-free uncapped mode. The change takes
	-- effect on the next rendered frame.
	SetVSync: (on: boolean) -> (),
	-- True if the swapchain is currently in a VSync mode.
	GetVSync: () -> boolean,

	-- Set the GPU power mode. Affects which adapter the engine
	-- requests at startup (and so which clock-speed bias the
	-- driver applies).
	--   "Quality"     — HighPerformance adapter. Discrete GPU on
	--                   dual-GPU laptops, max-FPS bias. Best for
	--                   shooters / action games where you want
	--                   the highest sustained FPS the hardware
	--                   can give. Default.
	--   "Performance" — LowPower adapter. Integrated GPU when
	--                   present, battery-friendly. Use for
	--                   stylized / lightweight games where
	--                   battery life matters more than peak FPS.
	--   "Auto"        — Whatever the driver picks by default
	--                   (often LowPower on laptops).
	-- Adapter selection happens at startup, so calling this
	-- AFTER the window is open only updates the value reported
	-- by GetPowerMode and persists it for the next launch — the
	-- live adapter doesn't change. Set this before opening the
	-- window (or in build.toml's startup script) for the change
	-- to take effect this run.
	SetPowerMode: (mode: "Quality" | "Performance" | "Auto") -> (),
	-- Read the current power-mode preference.
	GetPowerMode: () -> "Quality" | "Performance" | "Auto",
	GetBounds: (part: BasePart, quality: number?) -> { { Position: Vector, Size: Vector, Min: Vector, Max: Vector } },
	-- Returns every renderable BasePart whose bounds (computed at the
	-- given quality) overlap the AABB centered at `center` with extents
	-- `size`. Same quality semantics as GetBounds. Skips parts with
	-- Render = false. Honors deformed / animated meshes and DynMesh
	-- offsets.
	CheckArea: (center: Vector, size: Vector, quality: number?) -> { BasePart },
	-- GPU-accelerated sphere overlap query. One compute thread per part
	-- tests the part's world AABB against the sphere; matches are written
	-- to a packed atomic-counter buffer and read back. Optional filter
	-- function runs on each candidate (return true to accept). Returns
	-- the parts whose bounds intersect the sphere. Skips parts with
	-- Render = false.
	OverlapSphere: (center: Vector, radius: number, filter: ((BasePart) -> boolean)?) -> { BasePart },
	-- GPU-accelerated oriented-box overlap query. `rotation` is an Euler
	-- Vector in radians (defaults to no rotation). Test is conservative
	-- AABB-vs-OBB-AABB on the GPU, so a rotated thin box may include a
	-- few false positives near the corners; tighten with the filter
	-- function if you need exact OBB-OBB on a small candidate set.
	OverlapBox: (center: Vector, size: Vector, rotation: Vector?, filter: ((BasePart) -> boolean)?) -> { BasePart },
	-- GPU-accelerated frustum cull against the current camera. Returns
	-- every part whose AABB is inside or intersects the camera's view
	-- frustum (uses Renderable.Camera CFrame, FOV, Near and Far). Useful
	-- as a fast first pass for "what would be drawn this frame" or for
	-- area-of-interest streaming.
	OverlapFrustum: (filter: ((BasePart) -> boolean)?) -> { BasePart },
	-- GPU-accelerated, mesh-precise zone query. Returns every BasePart
	-- whose geometry intersects the oriented zone defined by `cframe`
	-- and `size` (full extents along the zone's local X/Y/Z). Each
	-- part's eight world-space corners are tested, then for models
	-- their actual mesh vertices (deformed verts when present, falling
	-- back to the source mesh) are transformed and tested too, so a
	-- spike sticking out of a model registers even if the body's
	-- bounding box doesn't reach the zone. Also catches the case where
	-- the zone is fully inside a large part. Optional filter callback,
	-- same shape as the other Overlap queries.
	GetItemsInZone: (cframe: CFrame, size: Vector, filter: ((BasePart) -> boolean)?) -> { BasePart },
	-- ── Shadow configuration ─────────────────────────────────────────
	-- The default 3D shader reads these values from the Frame uniform
	-- and adjusts its terminator + receive-shadow behavior. With
	-- shadows enabled, the lambert term becomes a smoothstep at the
	-- light terminator (sharper, more "shadowy" look), receivers get a
	-- darker self-shadow floor proportional to MapQuality, the PCF
	-- knob widens the smoothstep band to simulate softness, and
	-- ShadowDistance fades shadow term into the default half-Lambert
	-- past that distance.
	--
	-- Real depth-map sampling (per-light render passes, comparison
	-- samplers, cascade splits) is scaffolded for a future pass — this
	-- API ships now so games can author their lighting against it
	-- already.
	SetShadowsEnabled: (on: boolean) -> (),
	GetShadowsEnabled: () -> boolean,
	-- Texture resolution that future shadow maps will be rendered at.
	-- Clamped to [64, 8192] and rounded to the next power of two.
	-- Today the value also affects the stylized shadow term in the
	-- default shader (higher = more contrasty receive-shadow floor).
	-- Default 1024.
	SetShadowMapQuality: (size: number) -> (),
	GetShadowMapQuality: () -> number,
	-- World-space radius (from the camera) beyond which the shadow
	-- term is faded out and replaced by the regular half-Lambert. Set
	-- to 0 to disable distance fade. Default 80.
	SetShadowDistance: (distance: number) -> (),
	GetShadowDistance: () -> number,
	-- Depth comparison bias for future shadow-map sampling. Already
	-- packed into the Frame uniform so user shaders can read it.
	SetShadowBias: (bias: number) -> (),
	GetShadowBias: () -> number,
	-- PCF (Percentage Closer Filtering) tap count. 1 = hard shadows,
	-- 3 / 5 / 9 = progressively softer. Today the value drives the
	-- softness of the default shader's stylized terminator.
	SetShadowPCF: (taps: number) -> (),
	GetShadowPCF: () -> number,
}

declare class GPUBuffer
	-- Number of f32 slots. Same value as the `size` arg passed to
	-- GPU.NewBuffer; doesn't change after construction.
	function Size(self): number
	-- Write a list of f32 values into the buffer at `offset` (in floats,
	-- not bytes). Bounds-checked, writing past the end errors instead of
	-- corrupting adjacent GPU memory. Cheap; just one queue.write_buffer
	-- under the hood, no submit/wait.
	function Write(self, offset: number, values: { number }): ()
	-- Fill the entire buffer with one value. Convenience for clearing
	-- (Fill(0)) or initializing to a sentinel.
	function Fill(self, value: number): ()
end

-- =============================================================================
-- Actor, parallel CPU work. Spin up a worker pool; Push args to run a
-- function on another core; Pop the results back on the main thread.
-- =============================================================================

declare class Actor
	-- Hand args off to a worker thread. The args are deep-copied across the
	-- thread boundary, so mutating them after Push has no effect on the worker.
	-- Allowed types: nil, boolean, number, string, table (recursively, with
	-- only those types as keys/values). Functions, userdata, threads, etc.
	-- are rejected, workers can't share Luau state with the main thread.
	function Push(self, ...: any): ()
	-- Pop the next ready result.
	--   Pop()  / Pop(false), non-blocking. Returns no values if nothing
	--                          is ready. Use this from a heart tick or
	--                          render loop you can't afford to stall.
	--   Pop(true), blocking. Sleeps until a worker finishes
	--                          (or every worker has exited, in which case
	--                          it returns no values). The main thread
	--                          stops pumping while it waits, so only use
	--                          this from a script that's intentionally
	--                          waiting (e.g. a startup loader).
	-- Order is "first to finish, first out" (NOT Push order, work runs
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
	-- Spawn a worker pool that runs the given Luau source in parallel on
	-- other CPU cores. The source must be a string whose chunk evaluates
	-- to a function, the function returned is what each worker invokes
	-- per Push.
	--
	-- Workers run in completely isolated Luau states: they get math, table,
	-- string, bit32, buffer, utf8, coroutine, and the basic library, but
	-- NO `import`, `require`, `print`, `loadstring`, `dofile`, or
	-- `__dirname`. Workers cannot touch the main thread's globals, GUI,
	-- assets, sockets, or anything else import-shaped.
	--
	-- threads defaults to the number of CPU cores. Pass an explicit count
	-- for fine control (e.g. 1 for serial, 2 for I/O-bound work).
	--
	-- Note: passing a Luau function directly is no longer supported. Use
	-- `Actor.FromFile` to load a worker from a sibling .luau file, or pass
	-- the function body as a string. Earlier versions of the engine
	-- introspected the caller's chunk to recover the function source,
	-- that path was fragile (closures over locals couldn't be carried) and
	-- has been removed.
	--
	-- Example:
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
	new: (source: string, threads: number?) -> Actor,
	-- Actor.FromFile(path, threads?), load worker code from a separate
	-- .luau file. Path resolution is identical to `require`, including
	-- the project-wide file-mode setting from build.toml ("File Type"):
	--
	-- Relative mode (the default; matching most projects):
	--   "./worker", relative to the calling script's directory.
	--   "../shared/work", parent-relative.
	--   "subdir/helper", relative-without-./ also works.
	--   "@PkgId/jobs/heavy", explicit cross-package lookup.
	--   "@self/jobs/heavy", same package as the caller, root-relative.
	--
	-- Global mode (when build.toml sets `"File Type" = "Global"`):
	--   "workers/fib", resolved from the project root, NOT the
	--                          caller's directory.
	--   "@PkgId/...", same as in Relative mode.
	--
	-- The .luau / .lua extension is appended automatically; pass the
	-- bare module name. `init.luau` is honored for folder-style modules.
	-- DLC packages and Packages/ imports each use THEIR OWN file-mode
	-- setting (from each package's ManagedInfo.toml), so a Relative-mode
	-- DLC keeps resolving relative to the caller even inside a Global
	-- host project.
	--
	-- The file's raw text is read and used verbatim as the actor's
	-- source, same rules as the string form of Actor.new (the chunk
	-- must evaluate to a function, and the function must not reference
	-- any upvalue from the surrounding script).
	--
	-- Useful when worker code grows past a one-liner, keep it in its
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

-- =============================================================================
-- Register, named shared tables. Two scripts that have never seen each
-- other can coordinate by agreeing on a name: Register.new("foo") always
-- returns the SAME table for the same name across the entire Lua state.
-- =============================================================================

-- =============================================================================
-- Debug, in-test profiler. Flag-based timings, frame-drop monitoring,
-- live memory and CPU readouts. Methods are silent no-ops in a built game,
-- so profiling code can stay in your scripts without affecting shipped
-- performance. Background sampler runs on its own thread; the main loop
-- never waits on sysinfo refreshes.
-- =============================================================================

declare class DebugFlag
	-- Snapshot baseline metrics (time, memory, frame-drop count). Pair
	-- with :End to get a delta report.
	function Begin(self): ()
	-- Read the delta since :Begin and return a report:
	--   { name, ms, mem_delta_mb, cpu_pct, frame_drops, gpu_avg_ms }
	-- Also prints a one-line summary to stdout. Errors if :Begin wasn't
	-- called first.
	function End(self): { name: string, ms: number, mem_delta_mb: number, cpu_pct: number, frame_drops: number, gpu_avg_ms: number }
	-- The most recent { last_ms, last_mem_delta_mb, last_frame_drops }
	-- from this flag. Empty until the first :End.
	function Last(self): { [string]: number }
	-- The flag's name as passed to Debug.Flag.
	function Name(self): string
end

export type Debug_API = {
	-- True under `Ruzit Test`, false in a built game. All other methods
	-- are no-ops when this is false, so leaving Debug calls in shipped
	-- code is safe.
	IsTest: boolean,
	-- Allocate a named flag for measuring a span. Repeated Begin / End
	-- pairs on the same flag work fine and overwrite Last(). Naming the
	-- flag makes it easy to grep the log.
	Flag: (name: string) -> DebugFlag,
	-- Process resident memory in bytes / megabytes (latest sample).
	Memory: () -> number,
	MemoryMB: () -> number,
	-- Process CPU usage as a percentage (0..100 * cpu_count). Latest
	-- sample, refreshed every ~500 ms by the sampler thread.
	CpuPercent: () -> number,
	-- Total number of frames the heart loop has flagged as drops since
	-- process start (frame time exceeded the threshold).
	FrameDrops: () -> number,
	-- Largest single-frame time in milliseconds since the last
	-- ResetPeak. Useful for spike hunting.
	PeakFrameMs: () -> number,
	ResetPeak: () -> (),
	-- Set the threshold (in milliseconds) above which a heart tick
	-- counts as a frame drop. Default is 25 (≈ a missed 60 Hz tick).
	SetFrameDropThreshold: (ms: number) -> (),
	-- Subscribe to frame-drop events. Returns a Signal that fires once
	-- per dropped frame with the frame time in milliseconds. If
	-- threshold_ms is omitted, fires on any frame above the global
	-- SetFrameDropThreshold value.
	MonitorFrameDrops: (threshold_ms: number?) -> Signal<number>,
	-- Average milliseconds spent in the GPU queue submit since the last
	-- ResetGpuStats. Cheap proxy for GPU pressure (true GPU utilization
	-- needs platform-specific APIs). 0 until at least one frame submits.
	GpuSubmitAvgMs: () -> number,
	ResetGpuStats: () -> (),
}

-- =============================================================================
-- DynMesh, a runtime "skeleton" wrapped around a base BasePart. Weld extra
-- parts to fixed local-space points, to specific vertices on the source
-- mesh (vertex follows live deformation / animation), or stretch a part
-- between multiple anchors. Useful for character attachments (swords,
-- helmets, capes, glow effects) and for skin-style customization where
-- pieces snap onto a shared base mesh.
-- =============================================================================

declare class DynMesh
	-- Weld a BasePart to this mesh. Returns a numeric weld id you can
	-- pass to :GetPart / :Detach later. The opts table accepts:
	--   kind     = "Static" | "Attach" | "Joint", default "Static"
	--                "Static" / "Attach" — rigid follow. The welded
	--                  part's CFrame is captured at link time (its
	--                  current world position becomes the local
	--                  offset), then the part tracks the base each
	--                  tick. If you reassign target.CFrame between
	--                  ticks, the new pose is captured as a new local
	--                  offset, useful for eyes that look around while
	--                  staying welded to a head, or limbs that pose
	--                  locally while the body moves. Animations
	--                  PLAYING on the welded part still get
	--                  overwritten by the weld.
	--                "Joint" — hierarchical follow. The welded part
	--                  is dragged along by the base's per-tick motion
	--                  but keeps its own pose on top, so animation
	--                  tracks playing on the welded part keep working
	--                  as a "local pose" relative to the welded
	--                  position. Use this when a child part has its
	--                  own FBX animation (upper / lower body, jointed
	--                  limbs, etc.).
	--   point    = Vector                 -- offset in base part's
	--                                        local space; setting
	--                                        this overrides the
	--                                        capture-on-link
	--                                        behaviour
	--   rotation = Vector                 -- additional Euler offset;
	--                                        same override semantics
	--   vertex   = number                 -- live-tracked vertex index
	--                                        on the base mesh; mutually
	--                                        exclusive with point/rotation
	--   name     = string                 -- optional label for
	--                                        :GetPartByName / :DetachByName
	-- Welding the same target multiple times is allowed; the last
	-- weld processed in a given tick wins for that target's pose.
	-- On detach (:Detach / :DetachByName / :Clear / :DetachPart) the
	-- welded part stays at whatever world CFrame the weld last set,
	-- so it retains its pose instead of snapping back to its
	-- pre-weld location.
	function Weld(self, target: BasePart, opts: { kind: ("Static" | "Attach" | "Joint")?, point: Vector?, rotation: Vector?, vertex: number?, name: string? }?): number
	-- Stretch a target part along a polyline of two or more anchor
	-- points. Each anchor is `{ point = Vector }` (offset in the base
	-- part's local space) or `{ vertex = number }` (live-tracked
	-- vertex on the base mesh, deform / animation aware).
	-- Per tick the engine:
	--   * Resolves every anchor to a world-space point.
	--   * Sets the target's CFrame.Position to the centroid (average)
	--     of all anchor points, so adding mid-path waypoints recenters
	--     the part.
	--   * Sets the target's CFrame.Rotation to face first → last
	--     anchor (yaw + pitch). The part itself stays straight.
	--   * Sets target.Size.x to the TOTAL polyline length (sum of
	--     consecutive anchor-to-anchor distances), so a sequence of
	--     non-collinear waypoints stretches the part further than the
	--     straight first-to-last span.
	-- Use this for shoulder-strap / sleeve / cape stretches between
	-- live mesh vertices, electric arcs that bend through environment
	-- contact points, or rope segments that thread through pulleys.
	-- Detach restores the target's original X size.
	function Stretch(self, target: BasePart, anchors: { { point: Vector?, vertex: number? } }): number
	-- Remove a weld by id. The welded BasePart stays alive in the
	-- scene at its last computed pose, it just stops being driven by
	-- this DynMesh. Stretch welds restore the target's original X
	-- size; Joint welds release their parent-follow state. The
	-- target's animation tracks (if any) keep running and now own
	-- the part's CFrame uncontested. Returns true if a weld with that
	-- id existed.
	function Detach(self, weld_id: number): boolean
	-- Same as Detach but matches by the optional `name` passed at
	-- weld time. Removes ALL welds with that name (a Lua chain like
	-- `:Weld({ name = "extra" })` twice produces two welds with the
	-- same name; both come off in one call). Returns true if any
	-- welds were removed.
	function DetachByName(self, name: string): boolean
	-- Same as Detach but matches by the BasePart userdata directly.
	-- Useful when you have the part in hand but don't remember its
	-- weld id.
	function DetachPart(self, target: BasePart): boolean
	-- Detach every weld at once. Returns the number removed. Equivalent
	-- to looping over GetParts and calling DetachPart on each, but
	-- O(welds) and atomic. Useful when changing skin/customization
	-- presets, swap the welds rather than rebuilding the rig.
	function Clear(self): number
	-- Return the BasePart welded under `weld_id`, or nil if no such
	-- weld exists. The BasePart is the one you originally passed to
	-- :Weld / :Stretch, you get the same userdata back.
	function GetPart(self, weld_id: number): BasePart?
	-- Look up a welded part by the `name` you passed at weld time.
	-- Returns nil if no weld with that name exists. Useful for
	-- character rigs where you want to grab "left_arm" or "helmet"
	-- without remembering the numeric id.
	function GetPartByName(self, name: string): BasePart?
	-- All current welds as { { Id = number, Name = string?, Part = BasePart }, ... }.
	function GetParts(self): { { Id: number, Name: string?, Part: BasePart } }
	-- Return the underlying base BasePart this DynMesh is anchored to.
	function GetBase(self): BasePart
	-- True if both the DynMesh and its base part are still alive.
	-- Goes false automatically if the base BasePart is destroyed
	-- (e.g. via Asset.Drop on the base mesh's ModelAsset), so you
	-- can poll this to detect that the rig has been torn down out
	-- from under you.
	function IsAlive(self): boolean
	-- Stop running the per-tick weld update for this DynMesh. The
	-- welded parts stay alive and where they were last placed, they
	-- just stop following the base part. Asset.Drop on the base
	-- mesh's ModelAsset implicitly does this.
	function Destroy(self): ()
end

export type DynMesh_API = {
	-- Create a DynMesh anchored to an existing BasePart. Welds and
	-- stretches added to the returned handle update once per heart tick
	-- relative to the base part's pose.
	new: (base: BasePart) -> DynMesh,
}

-- =============================================================================
-- Video, animated frames + optional audio companion. Asset.GetVideo
-- returns BOTH a Video and a SoundData (the audio side, nil for sources
-- that don't carry audio). Hand the SoundData to SFX.LoadSound to play
-- it; the Video drives texture updates on welded BaseParts.
--
-- Supported formats:
--   .gif         Animated GIF, no audio (companion is nil). Pure-Rust
--                 decode via the `image` crate. Always available.
--   .mp4 / .mov  ISO Base Media containers. Decoded at load time by
--                 shelling out to `ffmpeg` on PATH. Video frames are
--                 read back as raw RGBA and the audio track is
--                 extracted as 16-bit PCM WAV that you pass to
--                 SFX.LoadSound. Requires ffmpeg installed on the
--                 host system; install from https://ffmpeg.org/.
-- =============================================================================

declare class Video
	-- Whether the video advances back to frame 0 when it reaches the
	-- end. Toggle live; setting false mid-play lets the current frame
	-- run out then stops.
	Looped: boolean
	-- The frame index currently being shown. Read at any time; writing
	-- seeks the playhead immediately.
	CurrentFrame: number

	function Width(self): number
	function Height(self): number
	function FrameCount(self): number
	function FrameDelayMs(self): number
	-- Total length in seconds (FrameCount * FrameDelayMs / 1000).
	function Length(self): number
	function Source(self): string

	function Play(self): ()
	function Pause(self): ()
	function Stop(self): ()
	-- Build an ImageAsset of the current frame. Cached, repeated calls
	-- in the same frame return the same cheap ImageAsset.
	function CurrentImage(self): ImageAsset
	-- Bind a BasePart so its Texture auto-updates each frame the video
	-- advances. The engine swaps the part's Texture with fresh frame
	-- pixels on every video tick. Each BasePart can only be linked to
	-- one Video at a time, calling LinkPart again with a different
	-- video transparently re-routes the binding.
	function LinkPart(self, part: BasePart): ()
	-- Stop driving texture updates on the part; its current Texture
	-- value is left as the last frame that was uploaded.
	function UnlinkPart(self, part: BasePart): ()
end

export type Task_API = {
	-- Run `fn` immediately on a fresh Lua coroutine. The caller doesn't
	-- block: if `fn` calls Task.Wait it suspends and resumes later;
	-- otherwise it runs synchronously to completion right here. Returns
	-- the underlying thread so you can pass it to Task.Cancel later.
	-- Errors inside `fn` are caught and printed.
	Spawn: (fn: () -> ()) -> thread,
	-- Run `fn` at the END of the current tick's scheduler pass — after
	-- every Spawn / Delay / Schedule / Repeat that is due this tick.
	-- Useful when you want to schedule cleanup that should run AFTER
	-- everyone else has had a chance to react to this frame's events.
	-- If a Defer'd callback calls Defer again, the new one runs on the
	-- NEXT tick (not the current one) — so you can't accidentally
	-- infinite-loop within a single frame. Returns the thread so you
	-- can cancel before it fires.
	Defer: (fn: () -> ()) -> thread,
	-- Run `fn` once after `seconds` real time on its own coroutine.
	-- Returns the thread so it can be canceled via Task.Cancel(thread)
	-- before it fires. The thread can also call Task.Wait inside `fn`
	-- to chain further sleeps.
	Delay: (seconds: number, fn: () -> ()) -> thread,
	-- Yield the current coroutine for `seconds` real time, then resume
	-- it on a future scheduler pump. The engine runs every script in
	-- a coroutine — entry script, signal handlers, Spawn / Delay /
	-- Defer threads — so Wait is always callable, no setup needed.
	-- Returns no values; use it as a fire-and-forget sleep:
	--     play_intro()
	--     Task.Wait(1.5)
	--     show_menu()
	Wait: (seconds: number?) -> (),
	-- Run `fn` once after `seconds` real time, tagged with `id`. Pass
	-- `nil` for `id` to auto-generate one. Returns the id string so
	-- you can later Cancel / Reschedule. Re-using an existing id
	-- replaces (and cancels) the previous schedule under that id.
	Schedule: (id: string?, seconds: number, fn: () -> ()) -> string,
	-- Reset the timer for an existing scheduled task to fire `seconds`
	-- from now. Returns true if `id` existed (and was rescheduled),
	-- false otherwise.
	Reschedule: (id: string, seconds: number) -> boolean,
	-- Run `fn` repeatedly every `interval` seconds. Pass `nil` for `id`
	-- to auto-generate one. Returns the id. Re-using an existing id
	-- replaces (and cancels) the previous repeat under that id.
	-- Re-entry safe: a callback that throws prints the error and the
	-- repeat continues firing.
	Repeat: (id: string?, interval: number, fn: () -> ()) -> string,
	-- Change the firing rate of an existing repeat. The next fire
	-- happens `interval` seconds from now (i.e. the timer resets).
	-- Returns true if `id` existed.
	SetInterval: (id: string, interval: number) -> boolean,
	-- Cancel a task by string id OR by thread reference.
	--   * string  — cancel a Schedule / Repeat by id (existing behavior).
	--   * thread  — cancel a Spawn / Delay / Defer by the thread it
	--               returned. The thread is marked for cancellation;
	--               on the next scheduler pump it's skipped (instead
	--               of being resumed) and its registration cleared.
	-- Returns true if something matching was canceled, false otherwise.
	-- Safe on unknown ids / unrelated threads.
	Cancel: (target: string | thread) -> boolean,
	-- True if a Schedule or Repeat with this id is currently active.
	Exists: (id: string) -> boolean,
	-- Seconds until the named task next fires, or nil if no task with
	-- that id exists. For Repeat tasks, this is the time until the
	-- next tick; for Schedule, the time until the (only) fire.
	TimeLeft: (id: string) -> number?,
}

export type Register_API = {
	-- Get (or create) the named register. First call with a given name
	-- creates a fresh empty table; every subsequent call with the same
	-- name returns the same instance, regardless of which script asks.
	-- Mutate the table normally, additions / deletions are visible to
	-- every other script holding a handle to it.
	new: (name: string) -> { [any]: any },
	-- True if a register with this name already exists. Useful for
	-- "first-script-in-wins" seeding patterns.
	Has: (name: string) -> boolean,
	-- Release the named register. Existing handles already returned by
	-- `new` keep working; freshly-fetched ones get a new empty table.
	-- Returns true if the register existed before this call.
	Drop: (name: string) -> boolean,
	-- Every currently-registered name. Handy for debug overlays.
	Names: () -> { string },
}

-- =============================================================================
-- PhysicsService — Rust-side rigid-body simulation that wraps existing
-- `Renderable.BasePart`s. All state and integration live in Rust; the only
-- Lua-side cost is a single field read or method call. Each `PhysicsPlane`
-- can run on its own rayon thread pool (set via the `Threads` option) and
-- writes simulated CFrames into a per-part override cell so the heavy
-- `BasePart` mutex is never locked per-tick.
-- =============================================================================

export type PhysicsPlaneOptions = {
	-- Default acceleration applied to every non-anchored object (units/sec²).
	-- Defaults to Vector(0, -50, 0) which feels like Earth gravity at game scale.
	Gravity: Vector?,
	-- Number of rayon worker threads to spin up for this plane's simulation
	-- step. Defaults to 1 (no thread pool, single-threaded inline). Set to
	-- the number of CPU cores you want to dedicate. Object integration and
	-- physics→part commits parallelize across this pool when the plane has
	-- enough objects (≥ 64) to make the dispatch overhead worth it.
	Threads: number?,
	-- Per-second linear velocity damping. Each tick:
	-- `velocity *= max(0, 1 - LinearDamping * dt)`. Default 0.10. Higher
	-- values make objects come to rest faster (think air drag).
	LinearDamping: number?,
	-- Same as LinearDamping but for angular velocity. Default 0.30 — kills
	-- tumbling fairly quickly so stacked piles settle visually.
	AngularDamping: number?,
	-- Quadratic drag coefficient. Each tick, every non-anchored body has
	-- `F = -Drag * |v| * v` applied as an additional force. Unlike the
	-- linear `LinearDamping` term, drag scales with v² so at low speed it
	-- barely matters but at high speed it dominates — exactly what you
	-- want for "feels like underwater": small impulses barely move, big
	-- impulses still travel a long way, things never quite stop the same
	-- way they do in air. Default 0.0 (off). Reasonable underwater values
	-- are around 1.0 – 4.0.
	Drag: number?,
	-- Constant upward acceleration applied per non-anchored body each
	-- tick. Positive values counteract gravity (set to about `-Gravity.Y`
	-- to make objects float neutrally; lower to "weighed-down sinking"
	-- and higher to "they float up"). Negative values pull down harder
	-- than gravity. Default 0.0 (off).
	Buoyancy: number?,
	-- Constant world-space force applied to every non-anchored body in
	-- the plane each tick, on top of gravity / drag / buoyancy. Units
	-- are force (mass·units/sec²), so a heavier body accelerates less
	-- from the same value. Useful for global wind, conveyor pushes,
	-- magnetic fields, etc. Defaults to Vector(0, 0, 0).
	ConstantForce: Vector?,
	-- Velocity-along-collision-normal threshold below which a colliding
	-- object's normal-axis velocity snaps to zero instead of bouncing.
	-- Default 0.5 (units/sec). This is what makes piles actually settle:
	-- without it, gravity and Bounciness oscillate forever and stacks
	-- look like floating soup. Set to 0 to disable (every collision
	-- always reflects with Bounciness, even at near-zero speeds).
	RestThreshold: number?,
	-- Number of constraint-solver iterations rapier runs each tick.
	-- Default 4. Bump to 8–16 for tall stacks or dense piles where
	-- residual penetration shows up; clamped to [1, 64]. When
	-- `LoopSolver` is true this is ignored (rapier runs at the high
	-- internal cap).
	SolverIterations: number?,
	-- When true, the solver is pinned to a high fixed iteration count
	-- (64) so any reasonable scene reaches the residual below which
	-- rapier's Projected Gauss-Seidel solver does negligible work per
	-- pass — effectively "iterate until stable" with the safety of a
	-- hard upper bound. Trades a modest extra per-tick CPU cost for
	-- visibly stable stacks. Use it when you don't want to manually
	-- tune `SolverIterations` for the worst pile in your scene.
	LoopSolver: boolean?,
	-- Disable rapier's sleep optimisation. By default a body whose
	-- kinetic energy falls below a threshold is put to sleep — its
	-- forces, contacts, and integration are skipped each tick until
	-- something wakes it back up. With `NeverSleep = true` every body
	-- in this plane stays active every tick: gravity, friction, and
	-- contact response are continuously applied even at rest. Use it
	-- when you need every object to react instantly to scripted forces
	-- (`obj:ApplyImpulse`, `obj.ImpulseDirection`, runtime
	-- `obj.CFrame` writes) without spending a frame waking the body
	-- back up. Cost: more CPU per tick on otherwise-stable scenes,
	-- since rapier can't optimise away the resting bodies. Default false.
	NeverSleep: boolean?,
	-- Pick the simulation backend.
	--   • `false` (default) — runs rapier3d on the CPU. Real rigid-body
	--     physics: proper OBB collision, contact manifolds, sleeping
	--     islands, friction + restitution that obey Coulomb / restitution
	--     laws, stable stacking, no floating soup. Use this for accurate
	--     scenes (player movement, vehicles, balanced piles).
	--   • `true` — runs the wgpu compute shader I wrote earlier. Each
	--     frame the buffer is packed CPU→GPU, two compute dispatches run
	--     (integrate, then collide) ping-ponging between storage buffers,
	--     and the result is mapped back. **Faster for bulk scenes** —
	--     N²-on-GPU still beats CPU rayon for thousands of independent
	--     objects — **but contact resolution is coarser**: AABB-only
	--     collision, single-point contacts, no Baumgarte stabilisation,
	--     no sleeping. Corners can clip slightly, stacks are less stable,
	--     and rotation is locked for non-Sphere shapes. Use it when you
	--     need throughput more than accuracy: particle showers, debris
	--     fields, decoration that physically interacts but doesn't need
	--     to be load-bearing.
	-- Requires the wgpu device to be initialised (a Window has been
	-- opened) — set it on a plane created before the window and the
	-- first tick after the device exists will pick up the GPU path.
	DeferGpu: boolean?,
}

declare class PhysicsObject
	-- Pose (Position + Rotation) as a single CFrame, mirroring BasePart's
	-- API. Writing teleports the object instantly. Use `cf.Position` /
	-- `cf.Rotation` on the read side if you only care about one component.
	CFrame: CFrame
	-- Linear velocity (units/sec).
	Velocity: Vector
	-- Angular velocity per axis (rad/sec).
	AngularVelocity: Vector
	-- Persistent per-tick acceleration applied on top of the plane's gravity.
	-- Useful for thrusters / wind / steady-state forces. Set to zero to clear.
	ImpulseDirection: Vector
	-- World-space force applied just to this object each tick, on top of
	-- the plane's ConstantForce and the object's ImpulseDirection. Where
	-- `ImpulseDirection` is per-tick acceleration (mass-independent),
	-- `ConstantForce` is a true force, so a heavier object accelerates
	-- less from the same value. Useful for per-object wind catch (a
	-- sail-shaped part with stronger pull than a rock), thruster jets,
	-- magnetic pulls on a single body. Defaults to Vector(0, 0, 0).
	ConstantForce: Vector
	-- Total mass (kg). Drives `:ApplyImpulse` (heavier = smaller velocity
	-- change for the same impulse). When set, the simulator picks a
	-- collider density of `Weight ÷ volume`.
	Weight: number
	-- Material density (kg / unit³). Lets you specify "this part is
	-- iron, that part is balsa wood" without computing volumes by hand.
	-- When set non-zero, density wins over Weight: rapier rebuilds the
	-- collider's mass and inertia tensor as `density × volume`. Setting
	-- it to 0 reverts to the Weight-driven path. Reading Density always
	-- returns the effective density rapier is using right now (either
	-- the explicit value, or `Weight / current volume`).
	Density: number
	-- Restitution coefficient used in collisions (0 = absorb, 1 = elastic).
	Bounciness: number
	-- Coulomb friction coefficient, [0, 1]. The pair coefficient used when
	-- two objects touch is `sqrt(Friction_a * Friction_b)`. Drives both
	-- static (parts at rest don't slide off each other under gravity)
	-- and kinetic (sliding parts decelerate naturally) friction. Default
	-- 0.5 — feels like wood-on-wood. Set close to 0 for ice / smooth
	-- surfaces, close to 1 for rubber / grippy surfaces.
	Friction: number
	-- When true, the object doesn't integrate (no gravity, no velocity step).
	-- Other objects still collide with it. Setting Anchored = true clears
	-- the current velocity + angular velocity.
	Anchored: boolean
	-- When true, this object participates in collision resolution against
	-- other CanCollide objects in the same plane.
	CanCollide: boolean
	-- Simulation-fidelity dial for this object (clamped 1–32, default 1).
	-- Two behaviours stack as it rises:
	--  • **Sub-stepping.** The plane sub-steps the whole simulation at
	--    `max(HitBoxQuality)` across its objects, so finer integration
	--    means smoother gravity, less velocity drift, and fast-moving
	--    parts don't tunnel through thin walls. Friction also integrates
	--    more accurately (less per-tick slip), so things stick where
	--    they should.
	--  • **Contact-driven rotation.** Collision detection is AABB-only
	--    (axis-aligned bounding box) and doesn't follow rotated
	--    geometry, so contact impulses produce rotation ONLY on Spheres
	--    at `HitBoxQuality > 1`. Cubes, models, and low-quality spheres
	--    are treated as having infinite rotational inertia for contacts
	--    — they translate but don't spin from collisions, which
	--    prevents the visually-broken "spinning in place while sliding"
	--    artifact you'd otherwise get on a flat AABB cube. Direct
	--    rotation (writing `AngularVelocity`, calling `RotateTo`,
	--    assigning a rotated CFrame) still works on every shape.
	-- The plane sub-steps at `max(HitBoxQuality)`, so one high-quality
	-- object lifts the cost for everyone — keep it on the parts that
	-- need precision (the player's ball, the bullet, the falling crate)
	-- and leave static props at 1.
	HitBoxQuality: number
	-- Read-through to the wrapped BasePart's Size. Setting also resizes the
	-- BasePart and fires its Changed signal (Size affects collision AABB).
	Size: Vector
	-- Body-local center-of-mass offset from the BasePart's origin. Default
	-- `Vector(0, 0, 0)` (CoM coincides with origin → balanced object,
	-- no contact-driven rotation). When set non-zero, the simulator
	-- switches the object to real rotational inertia, and contact
	-- patches that don't sit under the CoM produce tipping torque
	-- under gravity. This is the knob for "weld a heavy part to one
	-- side and watch it tip" — set it to wherever the welded mass sits
	-- relative to your part's origin (e.g. `Vector(0, 0, 1)` for a
	-- weight stuck to the +Z face), or call
	-- `:RecalculateCenterOfMass()` to derive it from the BasePart's
	-- mesh vertices automatically (works for DynMesh / Deformed
	-- meshes whose geometry is asymmetric).
	CenterOfMass: Vector
	-- True until the object is `:Unlink`ed, `:Destroy`ed, or its plane is
	-- destroyed.
	Alive: boolean

	-- Velocity-space impulse: velocity += vector / Weight. Anchored objects
	-- absorb impulses without moving.
	function ApplyImpulse(self, vector: Vector): ()

	-- Constrain which axes the object is allowed to move / rotate around.
	-- Pass any subset of the lock flags; unspecified flags are left as-is.
	-- Useful for top-down 2.5D games (lock Y), conveyor parts (lock X+Z),
	-- doors (lock everything except one rotation axis), etc.
	function FieldRestraint(self, opts: {
		LockX: boolean?,    LockY: boolean?,    LockZ: boolean?,
		LockRotX: boolean?, LockRotY: boolean?, LockRotZ: boolean?,
	}): ()

	-- Drive the object's angular velocity toward `target` (Euler radians)
	-- using a proportional controller. Higher `strength` snaps faster.
	-- Pair with `LockRot*` flags via FieldRestraint to clamp specific axes.
	function RotateTo(self, target: Vector, strength: number?): ()
	-- Stop trying to rotate toward a target. Existing angular velocity stays.
	function StopRotateTo(self): ()

	-- Walks the wrapped BasePart's mesh and writes the vertex centroid
	-- into `CenterOfMass`. Picks the deformed mesh first (if `Deform`
	-- is active) and falls back to the source model. Useful after a
	-- DynMesh / Deform mutation that asymmetrically reshapes the part —
	-- call this to refresh the CoM so physics tipping behaviour matches
	-- the new mass distribution. No-op for parts without a model
	-- (set `CenterOfMass` directly in that case).
	function RecalculateCenterOfMass(self): ()

	-- Returns the underlying Renderable.BasePart this object wraps.
	function BasePart(self): BasePart

	-- Detach from the plane WITHOUT destroying the BasePart. The part keeps
	-- its last simulated CFrame (frozen in place). Use when you want to
	-- hand the part back to manual control.
	function Unlink(self): ()
	-- Detach AND destroy the wrapped BasePart. After this both the physics
	-- object and the BasePart are gone.
	function Destroy(self): ()
end

declare class PhysicsPlane
	-- Acceleration applied to every non-anchored object every tick.
	Gravity: Vector
	-- When false, the plane skips integration + collision (objects freeze).
	-- BasePart CFrames stay where they last were.
	Enabled: boolean
	-- Number of rayon threads simulating this plane (read-only).
	Threads: number
	-- Per-second linear / angular velocity damping (read/write, see
	-- PhysicsPlaneOptions). Tune at runtime to adjust how quickly objects
	-- come to rest in this plane.
	LinearDamping: number
	AngularDamping: number
	-- Quadratic drag coefficient. See PhysicsPlaneOptions.Drag.
	Drag: number
	-- Constant upward acceleration term. See PhysicsPlaneOptions.Buoyancy.
	Buoyancy: number
	-- World-space force applied to every non-anchored body in this plane
	-- each tick. See PhysicsPlaneOptions.ConstantForce. Writable, live;
	-- e.g. ramp this from `Vector(0, 0, 0)` to `Vector(15, 0, 0)` over a
	-- few seconds to "turn on the wind".
	ConstantForce: Vector
	-- Velocity threshold below which a colliding normal-axis velocity is
	-- snapped to zero instead of bouncing. Required for stable piles.
	RestThreshold: number
	-- Number of constraint-solver iterations rapier runs per tick
	-- (1–64). Higher = better convergence on dense piles, more CPU.
	-- Default 4. Ignored when LoopSolver is true.
	SolverIterations: number
	-- Pin solver iterations at a high fixed cap (functionally
	-- "iterate until stopped"). See PhysicsPlaneOptions.LoopSolver.
	LoopSolver: boolean
	-- When true, every body keeps simulating every tick instead of
	-- being put to sleep when at rest. See PhysicsPlaneOptions.NeverSleep.
	NeverSleep: boolean
	-- Toggle the GPU compute physics path on/off at runtime. When true,
	-- integration + collision run as wgpu compute dispatches; when false,
	-- the rayon CPU path runs. See PhysicsPlaneOptions.DeferGpu.
	DeferGpu: boolean
	-- True until `:Destroy` is called.
	Alive: boolean
	-- Live count of physics objects in this plane.
	ObjectCount: number

	-- Wrap a BasePart with physics. Returns a PhysicsObject. The part's
	-- CFrame becomes lazily driven by the simulation — the heavy BasePart
	-- mutex is never locked per-tick; only a small Mutex<CFrame> override
	-- is updated. Reading `part.CFrame` from Lua transparently returns the
	-- simulated value.
	function Add(self, part: BasePart): PhysicsObject
	-- Toggle `Enabled` (matches the field setter).
	function SetEnabled(self, on: boolean): ()
	-- Manually advance the simulation by `dt` seconds. Mostly useful when
	-- you've disabled the auto-tick or want sub-stepping. Does nothing if
	-- the plane is destroyed or disabled.
	function Step(self, dt: number): ()
	-- Destroy the plane: every PhysicsObject is removed and its wrapped
	-- BasePart is left frozen at its last simulated CFrame (BaseParts are
	-- NOT destroyed — only the physics goes away).
	function Destroy(self): ()
end

-- =============================================================================
-- Objects — misc library of useful non-specific composition objects.
--
-- `ClonableContainer` holds a child set and can be deep-cloned (including
-- deformations, color changes, attached shaders and their param state).
--
-- `Movable` and `Sizable` are transform groupers: attach children, move /
-- resize the container, every child moves / scales relative to its own
-- baseline. Each child records the container's state AT THE MOMENT it was
-- added, so adding a shrunken child to an already-shrunken container does
-- not double-shrink; subsequent container changes apply as deltas.
--
-- Destroying any of these containers does NOT destroy the children. If you
-- pass a removeFn when calling :AddChild, it's invoked on :Destroy with the
-- child as its only argument, but the child object itself is left alive.
-- =============================================================================

export type ClonableContainer = {
	Alive: boolean,
	ChildCount: number,
	-- Attach a child. `removeFn` is called once with the child userdata
	-- when the container is :Destroy()'d (the container does NOT
	-- destroy the child itself; the callback is your hook to do that
	-- if you want).
	AddChild: (self: ClonableContainer, child: any, removeFn: ((any) -> ())?) -> (),
	-- Remove a previously-added child from the container's list.
	-- Returns true if the child was found and removed. Fires the
	-- child's removeFn callback (if any was provided to AddChild) on
	-- removal. Does NOT destroy the child userdata itself.
	RemoveChild: (self: ClonableContainer, child: any) -> boolean,
	GetChildren: (self: ClonableContainer) -> { any },
	-- Deep-clone the container. Every child is re-spawned with the
	-- same CFrame / Size / Color / Render flags / Lit / shadow flags,
	-- the same model + texture references (shared Arcs), the same
	-- deformed mesh data, and each attached shader is cloned with an
	-- independent copy of its param block (so updating the original
	-- doesn't bleed into the clone).
	Clone: (self: ClonableContainer) -> ClonableContainer,
	Destroy: (self: ClonableContainer) -> (),
}

export type Movable = {
	Alive: boolean,
	ChildCount: number,
	-- "3D" (BasePart children, locked after the first :AddChild),
	-- "2D" (GUI primitive children), or "Unset" (no children yet).
	Mode: string,
	-- 3D mode only. World pose of the container. Writing it moves every
	-- 3D child by the delta from each child's baseline-container CFrame.
	CFrame: CFrame,
	-- 2D mode only. Screen position. Writing it offsets every GUI
	-- child by the delta from each child's baseline-container Position.
	Position: Dim,
	-- 2D mode only. Rotation of the container in degrees. Writing it applies
	-- a rigid 2D rotation of the WHOLE child subtree around the container's
	-- own Position: each descendant's position rotates around that pivot and
	-- each descendant's own rotation gains the same delta. So a Movable whose
	-- children sit at top / bottom / left / right keeps them on the same
	-- sides relative to the container after a rotation.
	Rotation: number,
	-- When the container is empty AND the first child you add is a
	-- BasePart (3D mode) or GUI primitive (2D mode), the Movable's
	-- CFrame / Position is set to the child's at that moment, so the
	-- first add is always a no-op move. Subsequent moves apply deltas
	-- from there.
	AddChild: (self: Movable, child: any, removeFn: ((any) -> ())?) -> (),
	-- Detach a child. Returns true if found. Fires the child's
	-- removeFn callback if one was registered. Does NOT destroy the
	-- child itself.
	RemoveChild: (self: Movable, child: any) -> boolean,
	GetChildren: (self: Movable) -> { any },
	Destroy: (self: Movable) -> (),
}

export type Sizable = {
	Alive: boolean,
	ChildCount: number,
	Mode: string,
	-- Pass a Vector to size 3D children, or a Dim to size GUI children.
	-- Vectors accept both the userdata Vector and Luau's built-in vec3
	-- via Primitives.value_to_vector_opt. Writing it scales every child
	-- by the ratio of `Size` to each child's baseline container size.
	Size: any,
	-- Same inherit-on-first-child as Movable: an empty Sizable adopts
	-- its first leaf's Size (or its first nested Sizable's Size) so
	-- subsequent resizes apply as ratios from a sensible baseline.
	AddChild: (self: Sizable, child: any, removeFn: ((any) -> ())?) -> (),
	-- Detach a child. Returns true if found. Fires the child's
	-- removeFn callback if one was registered. Does NOT destroy the
	-- child itself.
	RemoveChild: (self: Sizable, child: any) -> boolean,
	GetChildren: (self: Sizable) -> { any },
	Destroy: (self: Sizable) -> (),
}

-- =============================================================================
-- Billboard, world-anchored UI surface. Link GUI primitives to a Billboard and
-- they render in the 3D scene at the Billboard's Position instead of on
-- screen. Each linked primitive's Position is interpreted as its offset within
-- the Billboard's local canvas (in pixels). Behind-camera Billboards are culled.
--
-- A primitive can be linked to at most one Billboard at a time. Setting
-- :AddChild on a primitive already linked to a different Billboard errors.
-- Destroying the Billboard unlinks all its children (they fall back to normal
-- screen-space rendering at their existing Position).
-- =============================================================================
export type Billboard = {
	Alive: boolean,
	ChildCount: number,
	-- World-space anchor point.
	Position: Vector,
	-- Pixel size of the canvas the children lay out inside. Each child's
	-- Position is treated as an offset (in pixels) from the canvas's
	-- top-left, like a normal screen GUI but parented to the canvas.
	Size: Dim,
	-- If false (default), children render at a fixed pixel size on
	-- screen no matter how far away the camera is — the classic "always
	-- the same size" billboard. If true, the canvas (and every child)
	-- scales with distance like a real 3D object: walking closer makes
	-- it bigger, walking away makes it smaller.
	ScaleWithCamera: boolean,
	-- Link a GUI primitive to render inside this Billboard. The
	-- primitive's existing Position becomes its offset inside the canvas
	-- (0,0 = canvas top-left).
	AddChild: (self: Billboard, child: any) -> (),
	-- Unlink a child. The primitive falls back to normal screen-space
	-- rendering using its current Position.
	RemoveChild: (self: Billboard, child: any) -> (),
	GetChildren: (self: Billboard) -> { any },
	Destroy: (self: Billboard) -> (),
}

export type Objects_API = {
	ClonableContainer: () -> ClonableContainer,
	Movable: () -> Movable,
	Sizable: () -> Sizable,
	Billboard: () -> Billboard,
}

export type PhysicsService_API = {
	-- Spin up a new physics plane with optional gravity + thread count.
	New: (opts: PhysicsPlaneOptions?) -> PhysicsPlane,
}

export type Imports = {
	Actor: Actor_API,
	Asset: Asset_API,
	Debug: Debug_API,
	DynImg: DynImg_API,
	DynMesh: DynMesh_API,
	Gamepad: Gamepad_API,
	GPU: GPU_API,
	GUI: GUI_API,
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
	Window: Window_API,
}

-- The global import function. Pass any name from `Imports` and you get a
-- typed handle for that subsystem. Unknown names throw at runtime.
declare import: <K>(name: keyof<Imports> & K) -> index<Imports, K>

-- The directory of the calling script as an absolute path. Combine with
-- `require(__dirname .. "/...")` to load sibling Luau modules.
declare __dirname: string

-- The global `load(path)`, sibling to `require` that takes a raw
-- filesystem path instead of a vfs key. Useful for loading mods, user-
-- content scripts, or anything outside the project bundle. Loaded
-- modules get the full runtime env (require / import / __dirname / load
-- / print) with file resolution rooted at the loaded file's own
-- directory, so nested loads resolve relative to the loaded file rather
-- than to the host project. Returns whatever the loaded chunk returns
-- (or `true` for bare side-effect modules).
--
--   load("C:/mods/init.luau")
--   load("./helper")           -- relative to the calling script's __dirname
--   load("/usr/local/share/ruzit/script.luau")
declare load: (path: string) -> any

