-- Pure-Lua stand-in for a native rock in this register-phase fixture.
-- cargo-test harnesses don't export Lua symbols, so the real .so cannot
-- load. This stub absorbs anything done to it at module-load time —
-- field access, calls, and lpeg's operator-overloaded grammar DSL — by
-- returning itself from every operation. Executing finder/probe BODIES
-- against it is meaningless; this case never does.
local stub = {}
local function self_(...) return stub end
setmetatable(stub, {
  __index = self_, __call = self_, __add = self_, __sub = self_,
  __mul = self_, __div = self_, __mod = self_, __pow = self_,
  __unm = self_, __concat = self_, __len = self_,
})
return stub
