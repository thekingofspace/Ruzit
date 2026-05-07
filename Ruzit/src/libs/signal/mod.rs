use mlua::{Function, Lua, MultiValue, Table, Value};

pub const SIGNAL_KEY: &str = "ruzit_signal_class";

const SIGNAL_LUA: &str = r#"
local Signal = {}
Signal.__index = Signal

local Connection = {}
Connection.__index = Connection

function Signal.new()
	return setmetatable({
		_conns = {},
		_waiting = {},
		_next_id = 1,
	}, Signal)
end

function Signal:Connect(fn)
	if type(fn) ~= "function" then
		error("Signal:Connect expected a function, got " .. type(fn), 2)
	end
	local id = self._next_id
	self._next_id = id + 1
	local conn = setmetatable({
		_fn = fn,
		_id = id,
		_signal = self,
		Connected = true,
	}, Connection)
	self._conns[id] = conn
	return conn
end

function Signal:Once(fn)
	local conn
	conn = self:Connect(function(...)
		conn:Disconnect()
		fn(...)
	end)
	return conn
end

function Signal:Wait()
	local co = coroutine.running()
	if not co then
		error("Signal:Wait can only be called from inside a coroutine", 2)
	end
	table.insert(self._waiting, co)
	return coroutine.yield()
end

function Signal:Fire(...)
	-- 1. Resume any coroutines that were :Wait()ing.
	local waiting = self._waiting
	self._waiting = {}
	for _, co in ipairs(waiting) do
		local ok, err = coroutine.resume(co, ...)
		if not ok then
			print("[Signal] coroutine error:", err)
		end
	end
	-- 2. Snapshot live connections so a handler that disconnects mid-fire is safe.
	local snapshot = {}
	for _, conn in pairs(self._conns) do
		if conn.Connected then
			table.insert(snapshot, conn)
		end
	end
	-- Each handler runs in its own coroutine so:
	--   * a handler that yields (e.g. Signal:Wait, sleep helpers) doesn't
	--     block subsequent handlers;
	--   * an error in one handler doesn't stop the others — coroutine.resume
	--     returns the failure to us instead of unwinding into Fire's frame.
	for _, conn in ipairs(snapshot) do
		if conn.Connected then
			local co = coroutine.create(conn._fn)
			local ok, err = coroutine.resume(co, ...)
			if not ok then
				print("[Signal] handler error:", err)
			end
		end
	end
end

function Signal:DisconnectAll()
	for _, conn in pairs(self._conns) do
		conn.Connected = false
	end
	self._conns = {}
end

function Connection:Disconnect()
	if self.Connected then
		self.Connected = false
		self._signal._conns[self._id] = nil
	end
end

return Signal
"#;

/// Load the Signal class once and stash it in the named registry so anyone
/// (importers, the engine itself) can grab it cheaply.
pub fn install(lua: &Lua) -> mlua::Result<()> {
    let class: Table = lua.load(SIGNAL_LUA).set_name("@signal").eval()?;
    lua.set_named_registry_value(SIGNAL_KEY, class)?;
    Ok(())
}

/// Returned by `import("Signal")` — the class table itself, with `new()`.
pub fn class(lua: &Lua) -> mlua::Result<Table> {
    lua.named_registry_value(SIGNAL_KEY)
}

/// Construct a fresh Signal instance from Rust.
pub fn new_instance(lua: &Lua) -> mlua::Result<Table> {
    let class: Table = lua.named_registry_value(SIGNAL_KEY)?;
    let new_fn: Function = class.get("new")?;
    new_fn.call::<Table>(())
}

/// Fire `signal:Fire(args...)` from Rust.
pub fn fire(lua: &Lua, signal: &Table, args: MultiValue) -> mlua::Result<()> {
    let fire_fn: Function = signal.get("Fire")?;
    let mut full = MultiValue::new();
    full.push_back(Value::Table(signal.clone()));
    for v in args {
        full.push_back(v);
    }
    let _ = lua; // unused in this branch but kept for API symmetry
    fire_fn.call::<()>(full)?;
    Ok(())
}
