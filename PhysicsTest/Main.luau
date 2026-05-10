--!strict

local Window         = import("Window")
local Process        = import("Process")
local Renderable     = import("Renderable")
local Primitives     = import("Primitives")
local GUI            = import("GUI")
local Asset          = import("Asset")
local Keyboard       = import("Keyboard")
local PhysicsService = import("PhysicsService")

local Vector = Primitives.Vector
local CFrame = Primitives.CFrame
local Color3 = Primitives.Color3
local Dim    = Primitives.Dim

local CHECKPOINTS = { 1, 10, 100, 250, 500, 1000, 2500, 5000, 7500, 100000 }
local HOLD_SECONDS = 4.0
local PHYSICS_THREADS = 90
local FIELD_HALF = 40
local FLOOR_HALF = 60
local SPAWN_BATCH_PER_FRAME_MAX = 200

local win = Window.Open({
	title = "Ruzit Physics Stress Test",
	width = 1280,
	height = 720,
})

local cam = Renderable.Camera
cam.CFrame = CFrame.new(Vector.new(0, 38, 70), Vector.new(-0.4, 0, 0))
cam.FOV    = 60
cam.Near   = 0.1
cam.Far    = 800

Renderable.SetSun(Vector.new(-0.3, -1.0, -0.4), Color3.fromRGB(255, 250, 235))
Renderable.SetAmbient(Color3.fromRGB(70, 80, 100))

local floor = Renderable.BasePart("Cube")
floor.Color = Color3.fromRGB(45, 55, 65)
floor.Size  = Vector.new(FLOOR_HALF * 2, 1, FLOOR_HALF * 2)
floor.CFrame = CFrame.new(Vector.new(0, -0.5, 0), Vector.new(0, 0, 0))
floor.CastShadow = false
floor.ReceiveShadow = false

local backWall = Renderable.BasePart("Cube")
backWall.Color = Color3.fromRGB(35, 40, 55)
backWall.Size  = Vector.new(FLOOR_HALF * 2, 30, 0.5)
backWall.CFrame = CFrame.new(Vector.new(0, 14.75, -FLOOR_HALF), Vector.new(0, 0, 0))
backWall.CastShadow = false
backWall.ReceiveShadow = false

local world = PhysicsService.New({
	Gravity = Vector.new(0, -50, 0),
	Threads = PHYSICS_THREADS,
	DeferGpu = false,
	SolverIterations = 5000*5,
	loop_solver = true
})

local floorObj = world:Add(floor)
floorObj.Anchored = true
floorObj.CanCollide = true
floorObj.Bounciness = 0.2
floorObj.HitBoxQuality = 5

local objects: { any } = {}
local part_count = 0

local function rand_color(): Color3
	local r = math.random(120, 255)
	local g = math.random(120, 255)
	local b = math.random(120, 255)
	return Color3.fromRGB(r, g, b)
end

local function spawn_one()
	local p = Renderable.BasePart("Cube")
	p.Size  = Vector.new(0.8, 0.8, 0.8)
	p.Color = rand_color()
	p.CastShadow = false
	p.ReceiveShadow = false
	local x = (math.random() * 2 - 1) * FIELD_HALF
	local z = (math.random() * 2 - 1) * FIELD_HALF
	local y = 30 + math.random() * 25
	p.CFrame = CFrame.new(Vector.new(x, y, z), Vector.new(0, 0, 0))

	local obj = world:Add(p)
	obj.Bounciness    = 0
	obj.Weight        = 1.0
	obj.HitBoxQuality = 5
	obj.CanCollide    = true
	objects[#objects + 1] = obj
	part_count = part_count + 1
end

local function recycle_fallen()
	for i = 1, #objects do
		local obj = objects[i]
		if obj.Alive then
			local cf = obj.CFrame
			if cf.Position.Y < -8 then
				local x = (math.random() * 2 - 1) * FIELD_HALF
				local z = (math.random() * 2 - 1) * FIELD_HALF
				local y = 30 + math.random() * 25
				obj.CFrame = CFrame.new(Vector.new(x, y, z), Vector.new(0, 0, 0))
				obj.Velocity = Vector.new(0, 0, 0)
				obj.AngularVelocity = Vector.new(0, 0, 0)
			end
		end
	end
end

local hud_bg = GUI.Basic.Square()
hud_bg.Color        = Color3.fromRGB(0, 0, 0)
hud_bg.Transparency = 0.4
hud_bg.Position     = Dim.new(12, 12)
hud_bg.Size         = Dim.new(440, 156)
hud_bg.ZIndex       = 10
hud_bg.Visible      = true

local FONT
local hud_lines: { any } = {}
local function make_label(y_offset: number)
	local lbl = GUI.Basic.Font(FONT)
	lbl.TextSize  = 18
	lbl.TextColor = Color3.fromRGB(220, 230, 245)
	lbl.Color     = Color3.fromRGB(0, 0, 0)
	lbl.Transparency = 1.0
	lbl.Position  = Dim.new(24, 22 + y_offset)
	lbl.ZIndex    = 11
	lbl.Visible   = true
	return lbl
end

local label_count
local label_target
local label_fps
local label_ms
local label_status

local ok_font, font_or_err = pcall(function()
	return Asset.GetAsset("Font", "PhysicsTest.font")
end)
if ok_font then
	FONT = font_or_err
	label_count  = make_label(0)
	label_target = make_label(22)
	label_fps    = make_label(44)
	label_ms     = make_label(66)
	label_status = make_label(96)
else
	hud_bg.Visible = false
	print("[PhysicsTest] no font asset bundled — HUD text disabled. Watch the console.")
end

local checkpoint_idx = 1
local target_count   = CHECKPOINTS[checkpoint_idx]
local hold_timer     = 0.0
local frame_dts: { number } = {}
local results: { string } = {}
local benchmark_done = false

local fps_smooth = 0.0
local fps_alpha  = 0.10
local frames_since_hud_text = 0

local function update_hud_text()
	if not FONT then return end
	if label_count then
		label_count.Text = string.format("Parts:    %d", part_count)
	end
	if label_target then
		label_target.Text = string.format("Target:   %d   (checkpoint %d/%d)",
			target_count, math.min(checkpoint_idx, #CHECKPOINTS), #CHECKPOINTS)
	end
	if label_fps then
		label_fps.Text = string.format("FPS:      %.0f", fps_smooth)
	end
	if label_ms then
		local last_dt = frame_dts[#frame_dts] or 0
		label_ms.Text = string.format("Frame:    %.2f ms", last_dt * 1000)
	end
	if label_status then
		if benchmark_done then
			label_status.Text = "Benchmark complete -- holding 10000 parts"
		elseif part_count >= target_count then
			local remaining = math.max(0, HOLD_SECONDS - hold_timer)
			label_status.Text = string.format("Measuring (%.1fs left)", remaining)
		else
			label_status.Text = string.format("Spawning to %d ...", target_count)
		end
	end
end

print(string.format("[PhysicsTest] starting benchmark, %d threads, %d checkpoints",
	PHYSICS_THREADS, #CHECKPOINTS))
print("[PhysicsTest] checkpoint  parts        avg ms     min ms     max ms     avg fps")

Process.Heartbeat:Connect(function(dt)
	if not win:IsOpen() then return end
	if dt > 1/15 then dt = 1/15 end

	if part_count < target_count then
		local need = target_count - part_count
		local per_frame = math.max(1, math.min(SPAWN_BATCH_PER_FRAME_MAX, math.ceil(need / 30)))
		for i = 1, per_frame do
			if part_count >= target_count then break end
			spawn_one()
		end
	end

	recycle_fallen()

	local frame_fps = (dt > 1e-6) and (1 / dt) or 0
	if fps_smooth <= 0 then
		fps_smooth = frame_fps
	else
		fps_smooth = fps_smooth + (frame_fps - fps_smooth) * fps_alpha
	end

	if part_count >= target_count and not benchmark_done then
		hold_timer = hold_timer + dt
		frame_dts[#frame_dts + 1] = dt

		if hold_timer >= HOLD_SECONDS then
			local sum = 0
			local mn = math.huge
			local mx = 0
			for _, d in ipairs(frame_dts) do
				sum = sum + d
				if d < mn then mn = d end
				if d > mx then mx = d end
			end
			local avg = sum / math.max(1, #frame_dts)
			local line = string.format("[PhysicsTest]  #%-2d        %5d        %6.2f     %6.2f     %6.2f     %6.1f",
				checkpoint_idx, part_count, avg * 1000, mn * 1000, mx * 1000, 1 / avg)
			print(line)
			results[#results + 1] = line

			checkpoint_idx = checkpoint_idx + 1
			if CHECKPOINTS[checkpoint_idx] then
				target_count = CHECKPOINTS[checkpoint_idx]
				hold_timer = 0
				frame_dts = {}
			else
				benchmark_done = true
				hold_timer = 0
				frame_dts = {}
				print("[PhysicsTest] ----- benchmark complete -----")
				for _, l in ipairs(results) do print(l) end
				print("[PhysicsTest] press Esc to exit, R to rerun")
			end
		end
	end

	frames_since_hud_text = frames_since_hud_text + 1
	if frames_since_hud_text >= 6 then
		frames_since_hud_text = 0
		update_hud_text()
	end
end)

Keyboard.InputChanged:Connect(function(_id, name, state)
	if state ~= "Begin" then return end
	if name == "Escape" then
		win:Close()
	elseif name == "r" or name == "R" then
		for _, obj in ipairs(objects) do
			if obj.Alive then obj:Destroy() end
		end
		objects = {}
		part_count = 0
		checkpoint_idx = 1
		target_count = CHECKPOINTS[1]
		hold_timer = 0
		frame_dts = {}
		results = {}
		benchmark_done = false
		fps_smooth = 0
		print("[PhysicsTest] ----- restart -----")
	end
end)
