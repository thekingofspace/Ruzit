# Ruzit

A Luau-scripted game engine. Write your game in Luau, and Ruzit handles
the window, GPU, audio, networking, Steam, and shipping a single-file
launcher with everything bundled.

```luau
local Window     = import("Window")
local Renderable = import("Renderable")
local Primitives = import("Primitives")

local win = Window.Open({ title = "Hello", width = 1280, height = 720 })

local cube = Renderable.BasePart("Cube")
cube.CFrame = Primitives.CFrame.new(Primitives.Vector.new(0, 0, -5))

import("RunService").Heartbeat:Connect(function(dt)
    cube.CFrame = cube.CFrame * Primitives.CFrame.Angles(0, dt, 0)
end)
```

That's a complete program. `Ruzit Test` runs it; `Ruzit Build` ships it
as a single `.exe` plus a `Managed/` folder of encrypted asset bundles.


## Highlights

- **Luau on top of Rust.** mlua + the Luau VM, so you get strict typing,
  generics, and the luau-lsp tooling. Ruzit ships a `types.d.luau` next
  to your project that drives autocomplete and hover docs for every
  engine API.
- **Single-file launcher.** `Ruzit Build` produces `Generated/<name>.exe`
  + `Generated/Managed/*.managed`. The `.exe` is your game's launcher;
  it loads everything from the encrypted bundles next to it.
- **Encrypted, compressed, sharded packages.** Scripts and assets are
  written into AES-GCM-encrypted `.managed` files, optionally
  zstd-compressed, with assets optionally split into sqrt(N)-sized shards
  so Steam Pipe / CDN updates only re-download what changed.
- **Real Steamworks integration.** User, Friends, Achievements, Stats,
  Lobby matchmaking, Game Server hosting, Overlay, Cloud, Workshop,
  Rich Presence, Remote Play. `steam_api64.dll` is embedded into the
  launcher and self-extracts on first run.
- **Built-in voice chat.** cpal-driven mic capture, Opus encode at
  20ms / 48kHz, push-to-talk and voice-activation gating, opus decode
  + camera-aware spatial playback on the receiving side.
- **3D + 2D in one scene.** Cubes, spheres, .obj / .fbx models;
  GUI primitives (square / circle / triangle / image / text) with a
  shared shader pipeline. Custom shaders for parts, primitives, sound,
  voice, plus full-screen post effects and skybox shaders.
- **GPU introspection + CPU raycasts.** `import("GPU")` exposes adapter
  info (name / vendor / backend / driver / device type), wgpu device
  limits, live frame stats, and a filterable raycast against every
  renderable part — pair with `Mouse.Position` + `GPU.ScreenToRay` for
  click-to-select.
- **Parallel CPU.** `import("Actor")` spawns a worker pool. Pass a
  function (Ruzit reads its source out of your script and recompiles
  it for each worker), Push args, Pop results in finish-order. Workers
  run in fully isolated sandboxes — no shared state, no `import`,
  no `print`.
- **Mod-friendly.** `Packages/` next to your project — drop someone
  else's `.managed` in and `Ruzit Test` loads it alongside your code,
  `Ruzit Build` ships it. `Asset.ImportAsset` loads loose files from
  arbitrary disk paths (mod folders, Workshop installs, etc.).


## Quickstart

```
Ruzit Init [path]               # scaffold a new project
Ruzit Test [path]               # run a project from source
Ruzit Build [path] [-o out]     # produce a shippable exe + Managed/
Ruzit Package [folder] [-o out] # bake one folder into a .managed bundle
Ruzit InitPackage [path]        # scaffold a Managed package folder
```

Bare `Ruzit.exe` prints the help screen.

```
> Ruzit Init MyGame
[Ruzit] init -> MyGame (name: MyGame)
  create assets/
  create Packages/
  create build.toml
  create Main.luau
  create types.d.luau
  create .vscode/settings.json

> cd MyGame
> Ruzit Test
[Ruzit] Test -> MyGame/Main.luau
[Ruzit] MyGame v0.1.0  (require mode: Relative)
Hello from MyGame!
```


## Project layout

```
MyGame/
  build.toml             # name, version, exe options, compression, steam app id
  Main.luau              # entry script
  types.d.luau           # Luau type declarations (LSP autocomplete + docs)
  .vscode/settings.json  # tells luau-lsp where types.d.luau lives
  assets/                # auto-bundled, reachable via Asset.GetAsset
  Packages/              # drop pre-built .managed files here for Ruzit Test/Build
  Generated/             # produced by Ruzit Build (your shippable output)
```

Subfolders inside any folder that holds a `ManagedInfo.toml` become
DLC packages — packaged automatically by `Ruzit Build` and loaded
alongside the main project at runtime.


## API at a glance

Every subsystem is reached through `import(name)`. See
[types.d.luau](types.d.luau) for full hover-doc-quality reference; the
short version:

| import         | what it does                                                       |
|----------------|--------------------------------------------------------------------|
| `Window`       | open the window, resize / fullscreen / focus / close hooks         |
| `Renderable`   | 3D scene: BasePart (Cube/Sphere), BaseModel (.obj/.fbx), Camera    |
| `GUI`          | 2D primitives (Square / Circle / Triangle / Image / Text)          |
| `Mouse`        | position, buttons, scroll, lock / cursor                           |
| `Keyboard`     | key state + InputChanged signal                                    |
| `Asset`        | load images / sounds / shaders / models / fonts / files            |
| `GPU`          | adapter info, device limits, frame stats, scene raycast            |
| `SFX`          | play sounds, fluent Volume / Speed / Echo / Reverb / Spatial / ... |
| `Voice`        | mic capture (Opus), per-peer playback channels, recordings         |
| `Steam`        | User, Achievements, Stats, Lobby, Server, Overlay, Cloud, Workshop |
| `Net`          | TCP / UDP / IPC / WebSocket / HTTP                                 |
| `IO`           | filesystem read / write / list / streaming handles                 |
| `Serde`        | JSON / TOML / YAML, hashing, gzip / zstd compression               |
| `Process`      | os / arch / pid / args / env, BindToHeart                          |
| `RunService`   | Heartbeat + RenderStepped per-frame signals                        |
| `Signal`       | factory for user-defined typed signals                             |
| `Primitives`   | Dim, Color3, Vector, CFrame                                        |
| `Managed`      | runtime info about loaded packages (yours, DLCs, third-party)      |
| `Actor`        | parallel CPU worker pool, function-as-task / source-as-task        |


## Build pipeline

`Ruzit Build` walks your project, encrypts each side into `.managed`
JSON-payload files inside `Generated/Managed/`, and writes a launcher
exe that knows how to load them.

- **`<name>.exe`** — windows-subsystem launcher (toggle to console with
  `[exe] windowed = false` or pass `--console` at runtime). Embeds
  `steam_api64.dll` and writes it next to itself on first run.
- **`<name>.scripts.managed`** — every `.luau` script in the project
  (filtered to skip `*.d.luau`).
- **`<name>.assets.managed`** — every file under `assets/`. Optional
  variants:
  - `compress_scripts = true` / `compress_assets = true` in build.toml
    enable zstd compression. Each side records a `"compressed"` header
    in its file so third-party `.managed` packages with different
    compression settings still load correctly.
  - `shard_assets = true` splits assets across
    `<name>.assets.shardNNNN.managed` files plus a small
    `.assets.manifest.managed`. Shard size is auto-tuned
    (~ceil(sqrt(asset_count)) shards, 4-256 MB each) so a Steam patch
    only re-downloads the shards that actually changed.
- **DLC folders + `Packages/`** — DLC subfolders are packaged the same
  way as the main project; loose `.managed` files in `Packages/` are
  copied through verbatim into `Generated/Managed/`.

`Ruzit Test` mirrors the same loading model in-process, so what you
test is what you ship.


## Stack

Rust, with:
- **mlua** + Luau (luau-lsp tooling for the script side)
- **wgpu 22** (DX12 backend on Windows)
- **winit 0.30** (window + input pump)
- **fontdue** (font rasterization)
- **rodio + cpal** (audio out / mic in)
- **opus** (voice codec)
- **fbxcel-dom + a hand-rolled ASCII FBX reader** (Blockbench-friendly)
- **steamworks 0.11** (with the embedded redistributable)
- **AES-GCM + zstd + base64 + serde_json** for the `.managed` format


## Status

Early but functional. Tracked features land iteratively in
`Ruzit/src/`. The Luau side is the public surface — anything that needs
an API change has a matching `types.d.luau` entry in the same commit.
