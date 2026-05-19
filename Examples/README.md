# Ruzit Examples

End-to-end samples that demonstrate engine features in isolation.

## Current contents

| Folder | What it shows |
|---|---|
| [`rust-ffi-counter/`](rust-ffi-counter/) | Minimal Rust DLL implementing the Ruzit FFI ABI (`ruzit_ffi_call` + `ruzit_ffi_free`). |
| [`luau-ffi-counter/`](luau-ffi-counter/) | Luau project that loads the DLL with `FFI.Load`, drives every export, and round-trips a nested table through `echo`. |

## Trying the FFI pair end-to-end

```bash
# 1) build the rust side
cd Examples/rust-ffi-counter
cargo build --release

# 2) copy the produced cdylib into the luau project's bin/
#    (Windows path shown; .so / .dylib paths analogous)
cp target/release/ruzit_ffi_counter.dll \
   ../luau-ffi-counter/bin/

# 3) run the luau side
cd ../luau-ffi-counter
ruzit test
```

The FFI loader probes the project's `bin/` first when running under
`ruzit test`, then the launcher's `bin/` next to the exe in a built
game — so the same `FFI.Load("ruzit_ffi_counter")` call works in
both worlds without code changes.
