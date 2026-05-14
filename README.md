# Ruzit

A Luau-scripted game engine. Write your game in Luau; Ruzit handles the
window, GPU, audio, physics, networking, Steam, VR, and shipping a
single-file launcher with everything bundled.

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

That's a complete program. `ruzit test` runs it; `ruzit build` ships it
as a single `.exe` plus a `Managed/` folder of encrypted asset bundles.


## Repo layout

```
Ruzit/                       cargo workspace
  cli/                       the `ruzit` binary (init / test / build / package)
  core/                      shared types: vfs, package format, build config
  runtime/                   the `ruzitrun` binary + engine library
    src/lib.rs               library entry (used by `cli` to spawn tests)
    src/main.rs              `ruzitrun` binary entry
    src/heart.rs             per-frame tick loop
    src/runtime.rs           Luau VM setup + `import` dispatch
    src/errors.rs            known-import list
    src/libs/                every import lives here (one folder per module)
docs/                        rendered API reference (HTML)
types.d.luau                 Luau type declarations consumed by luau-lsp
CHANGELOG.md                 release notes
```


## Build

### Prerequisites

- **Rust** stable 1.92+ (`rustup update stable`). The `vr` feature pulls
  in `wgpu 28` via `indite`, which requires 1.92; the rest of the engine
  compiles on older toolchains but bumping is the easy path.
- **Linux** also needs system libs:
  `build-essential pkg-config cmake libasound2-dev libudev-dev libfontconfig1-dev libx11-dev libxkbcommon-dev libxkbcommon-x11-dev libwayland-dev libxcursor-dev libxrandr-dev libxi-dev libgl1-mesa-dev libssl-dev`

### Compile

```bash
cd Ruzit
cargo build --release
```

That produces:
- `Ruzit/target/release/ruzit(.exe)` — the dev CLI.
- `Ruzit/target/release/ruzitrun(.exe)` — the runtime + launcher template.

Drop both somewhere on your `PATH` (or use them from the target dir).
The CLI calls `ruzitrun` as a library for `ruzit test` and copies the
binary as a launcher template for `ruzit build`.

### Feature flags

The `runtime` crate ships three opt-in Cargo features. They're additive
and have no effect on the Lua API surface (the library always loads;
the relevant `HasXxxFlag` property reports whether the backend was
compiled in).

| Feature   | Pulls in                          | Unlocks                                                                                                  |
|-----------|-----------------------------------|----------------------------------------------------------------------------------------------------------|
| `steam`   | `steamworks` + `steamworks-sys`   | `import("Steam")` — P2P, lobbies, achievements, Workshop, Cloud.                                         |
| `voice`   | `cpal` + `opus`                   | Mic capture for `SoundByte.GetVoiceChannel` and the legacy `Voice` module.                               |
| `vr`      | `indite` (OpenXR ↔ WGPU bridge)   | The legacy `import("VR")` module + real head/controller pose feed for `import("VirtualReality")`.        |

Combine with commas:

```bash
cargo build --release --features voice,steam,vr
```

For shipped games, list the same set in `build.toml`'s `[features]`
table and `ruzit build` will compile the launcher with them on.


## CLI workflow

```
ruzit init           [path]        scaffold a new project
ruzit initpackage    [path]        scaffold a Managed package folder
ruzit scaffold       [path]        regenerate .luaurc aliases from every manifest in the tree
ruzit test           [path]        run a project from source
ruzit build          [path] [-o]   produce Generated/<exe> + Generated/Managed/*.managed
ruzit package        [folder] [-o] bake one folder into a .managed bundle
ruzit fetch-deps     [path]        download steam_api into the given dir
ruzit refresh-types  [path]        re-download types.d.luau
ruzit update         [path]        refresh the bundled ruzitrun from the configured release base
```

Bare `ruzit` prints the help. See `docs/cli.html` for the full reference.

```
> ruzit init MyGame
[Ruzit] init -> MyGame (name: MyGame)
  create assets/
  create Packages/
  create build.toml
  create Main.luau
  create types.d.luau
  create .vscode/settings.json
  create .luaurc

> cd MyGame
> ruzit test
[Ruzit] Test -> MyGame/Main.luau
Hello from MyGame!
```


## Adding a new import

Every Luau-visible subsystem is a Rust module under
`Ruzit/runtime/src/libs/`. A minimal one is six lines; you can copy
`libs/keyboard/mod.rs` as a starter. Here's the whole loop end-to-end.
We'll add a hypothetical `import("Clock")` that exposes `Clock.Now()`.

### 1. Write the module

Create `Ruzit/runtime/src/libs/clock.rs` (or `libs/clock/mod.rs` if it
grows). The only required export is a `create` function that returns
a `Table` to be handed back to Lua:

```rust
use mlua::{Lua, Table};

pub fn create(lua: &Lua) -> mlua::Result<Table> {
    let t = lua.create_table()?;
    t.set(
        "Now",
        lua.create_function(|_, _: ()| -> mlua::Result<f64> {
            Ok(std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs_f64())
                .unwrap_or(0.0))
        })?,
    )?;
    Ok(t)
}
```

For larger modules, return userdata types via
`lua.create_userdata(MyHandle { ... })` and implement
`UserData for MyHandle` to get methods + getters. See `libs/soundbyte/mod.rs`
or `libs/physics/mod.rs` for real-size examples.

### 2. Declare the module

Add it to `runtime/src/libs.rs`:

```rust
pub mod clock;
```

If the module should only compile under a Cargo feature, gate it:

```rust
#[cfg(feature = "myfeature")]
pub mod clock;
```

### 3. Wire it into the import dispatch

`runtime/src/runtime.rs` is where the `import(name)` Lua function maps
strings to module loaders. Find the big `match` and add:

```rust
"Clock" => libs::clock::create(lua)?,
```

The arm runs once per `import("Clock")` call per Lua state, then the
returned table is cached.

### 4. Tell the unknown-import suggester about it

`runtime/src/errors.rs` has an `IMPORT_LIBS` array used to spellcheck
typos in `import("X")`. Add `"Clock"` to it (alphabetical-ish):

```rust
"Camera",
"Clock",
"Debug",
```

### 5. (Optional) Hook a per-tick pump

If the module needs to do work every frame — fire signals, update state,
advance animations — add a `pub fn pump(lua: &Lua) { ... }` in your
module and call it from `runtime/src/heart.rs`'s `run_loop`:

```rust
crate::libs::clock::pump(lua);
```

Look at `libs::renderable::tick_animations`, `libs::physics::tick`, and
`libs::soundbyte::pump` for the common shapes (signal-firing, dt-based
sim, GPU dirty bumps).

### 6. Add a Luau type

So games get autocomplete + hover docs, drop a declaration in
`types.d.luau`:

```luau
export type Clock_API = {
    -- Unix epoch seconds, fractional. Monotonic in practice; do not
    -- use for security tokens.
    Now: () -> number,
}
```

Then add `Clock: Clock_API` to the `Imports` table near the bottom of
the file. `import("Clock")` now has typed return.

### 7. (Optional) Document it

Make `docs/clock.html` (copy any existing module page as a starter) and
list it in `docs/_sidebar.html`. The docs site is plain HTML — no build
step.

### 8. Build + test

```bash
cd Ruzit
cargo build
```

Then in any `ruzit init`'d project:

```luau
local Clock = import("Clock")
print(Clock.Now())
```

That's the whole loop. The 30-odd modules under `libs/` are all built
this way — `signal.rs` is one of the smaller real examples (single
factory function + a UserData type), `soundbyte/mod.rs` is one of the
larger ones (registry of stateful audio nodes, per-tick pump, ~2300
lines).


## Project layout

```
MyGame/
  build.toml             # name, version, exe options, compression, steam app id, features
  Main.luau              # entry script
  types.d.luau           # Luau type declarations (LSP autocomplete + docs)
  .vscode/settings.json  # wires luau-lsp to types.d.luau + platform = standard
  .luaurc                # strict mode + aliases (Game = ./, plus any packages)
  assets/                # auto-bundled, reachable via Asset.GetAsset
  Packages/              # drop pre-built .managed files here for ruzit test / build
  Generated/             # produced by ruzit build (your shippable output)
```

Subfolders inside any folder that holds a `ManagedInfo.toml` become
DLC packages — packaged automatically by `ruzit build` and loaded
alongside the main project at runtime.


## API at a glance

Every subsystem is reached through `import(name)`. See `types.d.luau`
for the typed reference and `docs/` for hover-doc-quality pages.

| import            | what it does                                                            |
|-------------------|-------------------------------------------------------------------------|
| `Window`          | open the window, resize / fullscreen / focus / close hooks              |
| `Renderable`      | 3D scene: BasePart, BaseModel, Camera, DistortionBox, AnimationTrack    |
| `DynMesh`         | weld / stretch BaseParts to a shared rig                                |
| `DynImg`          | runtime-mutable images (set per-pixel from Lua)                         |
| `GUI`             | 2D primitives (Square / Circle / Triangle / Image / Text) + post-fx     |
| `Mouse`           | position, buttons, scroll, lock / cursor                                |
| `Keyboard`        | key state + InputChanged signal                                         |
| `Gamepad`         | controller state + button / axis signals                                |
| `VirtualReality`  | head + controller poses, Attatch, OpenXR-backed                         |
| `Asset`           | load images / sounds / shaders / models / animations / fonts / files    |
| `GPU`             | adapter info, device limits, frame stats, scene raycast                 |
| `SoundByte`       | graph-routed audio: Player / VoiceChannel / ByteSink / Modifier / Link  |
| `Steam`           | User, Achievements, Stats, Lobby, Server, Overlay, Cloud, Workshop      |
| `Net`             | TCP / UDP / IPC / WebSocket / HTTP                                      |
| `IO`              | filesystem read / write / list / streaming handles                      |
| `PhysicsService`  | rapier-backed rigid-body planes: New / Add / SetPin                     |
| `TweenService`    | tween any numeric / Vector / CFrame / Color3 property                   |
| `Serde`           | JSON / TOML / YAML, hashing, gzip / zstd compression                    |
| `Process`         | os / arch / pid / args / env, BindToHeart                               |
| `RunService`      | Heartbeat + RenderStepped per-frame signals                             |
| `Signal`          | factory for user-defined typed signals                                  |
| `Primitives`      | Color3, Vector, CFrame                                                  |
| `Managed`         | runtime info about loaded packages (yours, DLCs, third-party)           |
| `Actor`           | parallel CPU worker pool                                                |
| `Task`            | Wait / Spawn / Delay coroutines on the engine clock                     |
| `Video`           | GIF / MP4 frame iterators (MP4 via ffmpeg shellout)                     |

Deprecated but still loadable: `SFX`, `Voice`, `VR`.


## Build pipeline

`ruzit build` walks your project, encrypts each side into `.managed`
files inside `Generated/Managed/`, and writes a launcher exe that knows
how to load them.

- **`<name>.exe`** — windows-subsystem launcher (toggle to console with
  `[exe] windowed = false` or pass `--console` at runtime). Embeds
  `steam_api64.dll` and writes it next to itself on first run.
- **`<name>.scripts.managed`** — every `.luau` script in the project
  (filtered to skip `*.d.luau`).
- **`<name>.assets.managed`** — every file under `assets/`. Optional
  variants:
  - `compress_scripts = true` / `compress_assets = true` in build.toml
    enable zstd compression. Each side records a `"compressed"` header
    so third-party `.managed` packages with different compression
    settings still load correctly.
  - `shard_assets = true` splits assets across
    `<name>.assets.shardNNNN.managed` files plus a small
    `.assets.manifest.managed`. Shard size is auto-tuned
    (~ceil(sqrt(asset_count)) shards, 4-256 MB each) so a Steam patch
    only re-downloads the shards that actually changed.
- **DLC folders + `Packages/`** — DLC subfolders are packaged the same
  way as the main project; loose `.managed` files in `Packages/` are
  copied through verbatim into `Generated/Managed/`.

`ruzit test` mirrors the same loading model in-process, so what you
test is what you ship.


## Stack

Rust, with:
- **mlua** + Luau (luau-lsp tooling for the script side)
- **wgpu 22** (DX12 / Vulkan / Metal backends via winit 0.30)
- **rapier3d** physics
- **rodio** (audio output)
- **cpal + opus** (mic capture + voice codec — `voice` feature)
- **steamworks 0.11** with the embedded redistributable (`steam` feature)
- **indite** (OpenXR ↔ WGPU bridge for VR — `vr` feature)
- **fontdue** font rasterization
- **fbxcel-dom** + a hand-rolled ASCII FBX reader (Blockbench-friendly)
- **AES-GCM + zstd + base64 + serde_json** for the `.managed` format


## Releases

`CHANGELOG.md` tracks each version. Releases are cut manually via the
`Build Ruzit` workflow in GitHub Actions (Actions → workflow_dispatch),
which compiles Windows + Linux binaries, packages them as zips, and
publishes a GitHub release tagged from the version in
`Ruzit/runtime/Cargo.toml`.
