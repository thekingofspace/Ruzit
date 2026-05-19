# luau-ffi-counter

The Luau half of the FFI example. Loads
[`ruzit_ffi_counter`](../rust-ffi-counter/) and drives every export to
prove the round-trip works.

## Run it

1. Build the Rust DLL first:

   ```bash
   cd ../rust-ffi-counter
   cargo build --release
   ```

2. Drop the produced library into this project's `bin/` folder:

   ```
   bin/
     ruzit_ffi_counter.dll       # Windows
     # or
     libruzit_ffi_counter.so     # Linux
     # or
     libruzit_ffi_counter.dylib  # macOS
   ```

3. From this folder:

   ```bash
   ruzit test
   ```

   You should see something like:

   ```
   [demo] FFI bin directory: C:\...\luau-ffi-counter\bin
   [demo] discovered libs:   ruzit_ffi_counter.dll
   [demo] loaded from:       C:\...\luau-ffi-counter\bin\ruzit_ffi_counter.dll
   [demo] DLL says version = ruzit-ffi-counter v0.1.0
   [demo] initial:           0
   [demo] after 5x increment: 5
   [demo] after increment(step=10): 15
   [demo] after decrement(step=3):  12
   [demo] after set(100):    100
   [demo] after reset:       0
   [demo] echo Player.Name = Alice
   [demo] echo Inventory[2] = potion
   [demo] unloaded, Alive = false
   ```

## Shipping it

`ruzit build` from this folder packages the project and copies the
contents of `bin/` to `Generated/bin/` alongside the launcher exe.
End users get a directory layout that mirrors what `ruzit test` was
seeing, so the same `FFI.Load("ruzit_ffi_counter")` call works
unchanged in the built game.
