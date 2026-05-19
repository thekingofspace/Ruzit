# ruzit-ffi-counter

A minimal Rust DLL that demonstrates the **Ruzit FFI ABI**. Exposes a
process-global counter with `increment`, `decrement`, `get`, `set`,
`reset`, `echo`, and `version` exports.

## The ABI in three sentences

A Ruzit-loadable DLL exports exactly two `extern "C"` functions:

```rust
extern "C" fn ruzit_ffi_call(name: *const c_char, args: *const c_char) -> *mut c_char;
extern "C" fn ruzit_ffi_free(ptr: *mut c_char);
```

`ruzit_ffi_call` receives the export name (a C string the Luau side
asked for via `library:Call("name", ...)`) and a JSON-encoded args
string. It returns a JSON-encoded result as a `*mut c_char` that the
DLL allocated with `CString::into_raw` — the engine hands the pointer
back to `ruzit_ffi_free` once it's done reading the bytes.

Return `null` to indicate "no result" (Lua sees `nil`). Return a
JSON object like `{"error": "..."}` to surface an application-level
failure that Luau can pattern-match on.

## Build

```bash
cd Examples/rust-ffi-counter
cargo build --release
```

This produces:

- Windows: `target/release/ruzit_ffi_counter.dll`
- Linux:   `target/release/libruzit_ffi_counter.so`
- macOS:   `target/release/libruzit_ffi_counter.dylib`

## Drop it into a Ruzit project

Copy the produced library into your project's `bin/` folder:

```
my-game/
  Main.luau
  build.toml
  bin/
    ruzit_ffi_counter.dll   <-- here (Windows; .so / .dylib elsewhere)
```

The Luau side in [`Examples/luau-ffi-counter`](../luau-ffi-counter/)
loads it with `FFI.Load("ruzit_ffi_counter")` — the engine looks in
`bin/` first, both during `ruzit test` (project's `bin/`) and in a
built game (the `bin/` next to the launcher exe).

## Why JSON

Stable across ABI versions, language-agnostic, mixed-type, and works
fine for the kind of plumbing you'd reach for FFI for. If you need to
move megabytes per call there are better wire formats — but for "let
me run this hand-tuned vectorized routine in Rust and get the result
back", JSON's serialisation cost vanishes next to whatever you came
here to do.
