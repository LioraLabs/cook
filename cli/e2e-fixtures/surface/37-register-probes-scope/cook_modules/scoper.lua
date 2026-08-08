-- §24.4.3 register-phase scoped store: init() writes through a scope view;
-- the reader checks the scoped and full-key spellings agree (the scoped
-- operations are DEFINED as the label:key-prefixed ones).
local m = {}
function m.init()
    local sc = cook.probes.scope("toolchain")
    sc.set("cc", "clang-lawful")
end
function m.value()
    local scoped = cook.probes.scope("toolchain").get("cc")
    local full = cook.probes.get("toolchain:cc")
    assert(scoped == full, "scoped and full-key reads must agree")
    return scoped
end
return m
