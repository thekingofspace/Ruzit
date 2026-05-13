pub const BUILD_TOML: &str = r#"Name = "{name}"
Version = "0.1.0"
Creator = ""

[configs]
"File Type" = "Relative"

[exe]
# name = "{name}"     # output exe name when running `Ruzit Build` (defaults to entry stem)
# icon = "logo"       # looks for <icon>.ico next to build.toml and embeds it as the exe icon
# windowed = true     # default. Launcher is windows-subsystem, no console
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
                            # that actually changed, huge win for content updates
                            # over Steam Pipe / CDNs without drowning the OS in
                            # thousands of tiny files.
# bytecode = true           # compile every .luau / .lua to Luau bytecode at build
                            # time and ship the bytecode in place of source. Faster
                            # startup (no parse/compile at runtime), smaller
                            # `.scripts.managed`, and the original source is no
                            # longer recoverable from the bundle. Runtime errors
                            # still report file:line, but the engine no longer has
                            # the source text to print the offending line snippet.
                            # Keep off during active development, flip on for
                            # release builds. Default false.

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
-- The require runs this file once and caches the returned table, every
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
    "luau-lsp.platform.type": "standard",
    "files.associations": {
        "*.luau": "luau"
    }
}
"#;

pub const LUAURC: &str = r#"{
    "languageMode": "strict",
    "aliases": {
        "Game": "./"
    }
}
"#;
