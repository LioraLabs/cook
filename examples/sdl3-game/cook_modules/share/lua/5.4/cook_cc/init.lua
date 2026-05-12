local toolchain = require("cook_cc.toolchain")
local cc        = require("cook_cc.cc")
local targets   = require("cook_cc.targets")
local finder    = require("cook_cc.finder")
local db        = require("cook_cc.compile_db")

local M = {}

function M.init()
    -- Loader-contract hook (Standard §6.3.4). Called once per VM.
    toolchain.rehydrate()
end

-- Public surface (Standard §9.2 contract).
M.toolchain        = toolchain.set
M.defaults         = toolchain.merge_defaults
M.compile          = cc.compile
M.archive          = cc.archive
M.link             = cc.link
M.bin              = targets.bin
M.lib              = targets.lib
M.shared           = targets.shared
M.headers          = targets.headers
M.find             = finder.find
M.register_finder  = finder.register
M.compile_commands = db.write

-- §9.2.3.13 — only function in §9.2 that raises on miss.
function M.find_or_error(name, opts)
    local r = M.find(name, opts)
    if r.found then return r end
    local lines = { "could not locate '" .. name .. "'" }
    if opts and opts.version then
        lines[1] = lines[1] .. " (version " .. opts.version .. ")"
    end
    lines[1] = lines[1] .. ":"
    for _, a in ipairs(r.tried or {}) do
        local line = "  - " .. a.strategy .. ": " .. a.outcome
        if a.reason and a.reason ~= "" then line = line .. " (" .. a.reason .. ")" end
        lines[#lines + 1] = line
        if a.hint then lines[#lines + 1] = "    hint: " .. a.hint end
    end
    error("[cc.find_or_error] " .. table.concat(lines, "\n"), 2)
end

return M
