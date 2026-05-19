--!strict

local FFI        = import("FFI")
local GUI        = import("GUI")
local Primitives = import("Primitives")
local Process    = import("Process")
local Signal     = import("Signal")
local Task       = import("Task")
local Window     = import("Window")

local Vector = Primitives.Vector
local Dim    = Primitives.Dim
local Color3 = Primitives.Color3

print("[demo] FFI bin directory:", FFI.BinDirectory() or "(none)")
print("[demo] discovered libs:  ", table.concat(FFI.List(), ", "))

local counter = FFI.Load("ruzit_ffi_counter")
print("[demo] loaded from:", counter.Source)

local info = counter:Call("version") :: { name: string, version: string }
print(string.format("[demo] DLL says version = %s v%s", info.name, info.version))

------------------------------------------------------------------------
-- 1) Stateful counter, the original demo. Synchronous calls only.
------------------------------------------------------------------------
print("[demo] initial:", counter:Call("get"))
for _ = 1, 5 do counter:Call("increment") end
print("[demo] after 5x increment:", counter:Call("get"))
print("[demo] increment(step=10):", counter:Call("increment", { step = 10 }))
print("[demo] decrement(step=3): ", counter:Call("decrement", { step = 3 }))
counter:Call("set", { value = 100 })
print("[demo] after set(100):", counter:Call("get"))
counter:Call("reset")
print("[demo] after reset:    ", counter:Call("get"))

------------------------------------------------------------------------
-- 2) Async call: yield the current coroutine while a heavy compute
--    runs on a worker thread inside the DLL. The main heart loop
--    keeps ticking; this script only resumes when the DLL is done.
------------------------------------------------------------------------
print("[demo] kicking off slow_compute on background thread...")
Task.Spawn(function()
    local t0 = os.clock()
    local out = FFI.CallAsync(counter, "slow_compute", { ms = 750, n = 2000000 }) :: {
        result_squared_sum: number,
        elapsed_ms: number,
        thread: string,
    }
    local elapsed_main = (os.clock() - t0) * 1000
    print(string.format(
        "[demo] async returned in %.0fms main-thread wall time (DLL worker said %.0fms on %s)",
        elapsed_main, out.elapsed_ms, out.thread
    ))
    print(string.format("[demo]   squared_sum = %.3e", out.result_squared_sum))
end)

-- Run a few synchronous calls in parallel to prove the heart loop is
-- still ticking while the worker is sleeping inside the DLL.
Task.Spawn(function()
    for i = 1, 4 do
        Task.Wait(0.15)
        print(string.format("[demo]   heart tick %d (counter = %d)", i, counter:Call("get")))
        counter:Call("increment")
    end
end)

------------------------------------------------------------------------
-- 3) Passing Ruzit objects (Vector, CFrame, Color3) across the FFI
--    boundary. The engine's JSON marshaller recognises these primitive
--    userdata types and tags them with "_type". On the Rust side, the
--    DLL stores a registry of particles, advances them with gravity,
--    and ships state back as Vector-shaped JSON.
------------------------------------------------------------------------
Task.Spawn(function()
    Task.Wait(0.05) -- let the header land first

    counter:Call("clear_world")
    counter:Call("set_gravity", { g = -9.81 })

    -- Three particles, real Ruzit Vector values passed through verbatim.
    local a = counter:Call("spawn_particle", {
        name     = "alpha",
        position = Vector.new(0, 10, 0),
        velocity = Vector.new(1, 5, 0),
    }) :: { id: number, name: string, position: { x: number, y: number, z: number } }
    counter:Call("spawn_particle", {
        name     = "beta",
        position = Vector.new(5, 8, 0),
        velocity = Vector.new(-1, 2, 0),
    })
    counter:Call("spawn_particle", {
        name     = "gamma",
        position = Vector.new(-3, 12, 1),
        velocity = Vector.new(0, 0, -2),
    })

    print(string.format("[demo] spawned particle id=%d name=%s", a.id, a.name))

    -- Advance the rust-side world 30 frames at 1/60 dt.
    for _ = 1, 30 do counter:Call("tick_world", { dt = 1 / 60 }) end

    -- The result {x, y, z} can be wrapped back into a Ruzit Vector on
    -- our side -- the rust DLL has no way to allocate engine userdata,
    -- so it ships fields and we reconstruct.
    local snap = counter:Call("get_particle", { name = "alpha" }) :: {
        id: number,
        position: { x: number, y: number, z: number },
        velocity: { x: number, y: number, z: number },
    }
    local pos = Vector.new(snap.position.x, snap.position.y, snap.position.z)
    local vel = Vector.new(snap.velocity.x, snap.velocity.y, snap.velocity.z)
    print(string.format("[demo] alpha after 30 ticks: pos=(%.2f, %.2f, %.2f) vel=(%.2f, %.2f, %.2f)",
        pos.X, pos.Y, pos.Z, vel.X, vel.Y, vel.Z))

    local all = counter:Call("all_particles") :: { { id: number, position: { x: number, y: number, z: number } } }
    print(string.format("[demo] world holds %d particles after gravity sim", #all))

    counter:Call("clear_world")
    print("[demo] world cleared")

    Task.Wait(0.05) -- let the async demo finish first

    --------------------------------------------------------------
    -- 4) Host vtable: pass a real Ruzit GUI Primitive to the DLL
    --    and let it mutate the primitive directly via the
    --    RuzitHost callback. We open a window so the rendered
    --    primitive is actually visible, hand the userdata to the
    --    rust side (the marshaller registers a handle), and ask
    --    the DLL to tween its color from red to blue on a worker
    --    thread. The DLL drives the engine, not the other way
    --    around -- watch the square change color while the main
    --    script does other work.
    --------------------------------------------------------------
    local win = Window.Open({ title = "FFI Host Demo", width = 480, height = 240 })

    local square = GUI.Basic.Square()
    square.Size     = Dim.new(160, 160)
    square.Position = Dim.new(160, 40)
    square.Color    = Color3.new(1, 0, 0)
    square.Visible  = true

    print("[demo] handing primitive to DLL for color tween...")
    local started = counter:Call("tween_color", {
        primitive    = square,
        from         = { r = 1.0, g = 0.0, b = 0.0 },
        to           = { r = 0.0, g = 0.4, b = 1.0 },
        duration_ms  = 1500,
        steps        = 60,
    }) :: { started: boolean, handle: number, duration_ms: number }
    print(string.format("[demo]   DLL accepted; handle=%d duration=%dms",
        started.handle, started.duration_ms))

    -- Watch the engine read back the primitive's color while the DLL
    -- is still tweening it. The DLL is mutating the SAME PrimitiveState
    -- the engine renders from -- there's no per-frame Luau round-trip
    -- driving the animation.
    Task.Wait(1.7)
    print(string.format("[demo] final color = (%.2f, %.2f, %.2f)",
        square.Color.R, square.Color.G, square.Color.B))

    --------------------------------------------------------------
    -- 5) Cross-thread shared state (FFI World k/v) and named
    --    signals fired BACK from the DLL into Luau via the
    --    main-thread queue.
    --------------------------------------------------------------
    local orbit_done = Signal.new()
    local paint_done = Signal.new()
    FFI.RegisterSignal("ffi.orbit_done", orbit_done)
    FFI.RegisterSignal("ffi.paint_done", paint_done)

    orbit_done:Connect(function(info)
        print(string.format("[demo] DLL orbit_done -> handle=%d elapsed=%dms",
            (info :: any).handle, (info :: any).elapsed_ms))
    end)
    paint_done:Connect(function(info)
        print(string.format("[demo] DLL paint_done -> style=%s", (info :: any).style))
    end)

    FFI.SetData("player.name", "Alice")
    FFI.SetData("player.hp", 100)
    counter:Call("world_set", { key = "boss.hp", value = 500 })
    local boss_hp = FFI.GetData("boss.hp")
    print(string.format("[demo] World shared store: player.name=%s, boss.hp=%s",
        tostring(FFI.GetData("player.name")), tostring(boss_hp)))

    Task.Wait(0.05)
    counter:Call("clear_world")
    counter:Unload()
    print("[demo] unloaded, Alive =", counter.Alive)
    win:Close()
    Process.Close(0)
end)