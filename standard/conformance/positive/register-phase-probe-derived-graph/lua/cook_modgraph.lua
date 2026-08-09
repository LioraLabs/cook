-- Standard §{cat.probes.member-source} "Register-phase reads" + §{lua.add-unit-after}
-- (CS-0219): a register-phase body reads a declared probe and registers units
-- whose ORDERING EDGES are a function of the value it read.
--
-- The fixture asserts inside the module rather than against a golden file,
-- because the property under test is the shape of the planned graph, which no
-- parse dump can see. Each assertion names the specific thing that broke.

local M = {}

-- One line per module: `<name> [<imported name> ...]`. Deliberately emitted in
-- dependency order by the probe, because an `after` entry may only point
-- backwards — the ordering the data implies is the ordering the units are
-- registered in, and a module generating units from scanned data is the party
-- that can sort them meaningfully.
local function parse(lines)
    local mods = {}
    for _, line in ipairs(lines) do
        local parts = {}
        for word in line:gmatch("%S+") do
            parts[#parts + 1] = word
        end
        if #parts > 0 then
            local imports = {}
            for i = 2, #parts do
                imports[#imports + 1] = parts[i]
            end
            mods[#mods + 1] = { name = parts[1], imports = imports }
        end
    end
    return mods
end

function M.compile_all(key)
    -- Step 2 of the register-phase lookup: `scan:mods` was never a fan-out
    -- source, so nothing pre-passed it. Before CS-0219 this read `nil`.
    local scanned = cook.probes.get(key)
    if type(scanned) ~= "table" then
        error("register-phase cook.probes.get('" .. key .. "') returned "
              .. type(scanned) .. ", not the probe's value", 0)
    end

    local mods = parse(scanned)
    if #mods ~= 3 then
        error("expected 3 scanned modules, got " .. #mods, 0)
    end

    cook.step_group(function()
        for _, m in ipairs(mods) do
            local after = {}
            for _, imported in ipairs(m.imports) do
                after[#after + 1] = "build/" .. imported .. ".bmi"
            end
            cook.add_unit({
                outputs = { "build/" .. m.name .. ".bmi" },
                command = "printf '' > build/" .. m.name .. ".bmi",
                after   = after,
            })
        end
    end)
end

return M
