-- A module written for register phase: its top level registers a recipe, which
-- is register-only API. Loading it on a worker VM raises. That is what makes it
-- the probe for CS-0205's reference gate — a body that merely has a FIELD named
-- `regonly` must not cause this module to be loaded at execute time.
local m = {}

cook.recipe("regonly_registered", { requires = {} }, function() end)

function m.value()
  return "REGISTER-ONLY"
end

return m
