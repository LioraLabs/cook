-- §24.4.3: a scope label containing ':' MUST raise a runtime error.
local m = {}
function m.init()
    cook.probes.scope("a:b")
end
return m
