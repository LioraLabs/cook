local toolchain = require("cook_cc.toolchain")
local helpers   = require("cook_cc._check_helpers")

local M = {}

-- Per-VM probe-registration set: probe-key -> true
M._registered = M._registered or {}

local function opts_to_lua_literal(opts)
    opts = opts or {}
    local parts = {}
    if opts.standard then
        parts[#parts + 1] = "standard=" .. string.format("%q", opts.standard)
    end
    if opts.extra_cflags then
        parts[#parts + 1] = "extra_cflags=" .. string.format("%q", opts.extra_cflags)
    end
    if opts.extra_ldflags then
        parts[#parts + 1] = "extra_ldflags=" .. string.format("%q", opts.extra_ldflags)
    end
    if opts.defines then
        local lst = {}
        for _, d in ipairs(opts.defines) do lst[#lst + 1] = string.format("%q", d) end
        parts[#parts + 1] = "defines={" .. table.concat(lst, ",") .. "}"
    end
    if opts.includes then
        local lst = {}
        for _, d in ipairs(opts.includes) do lst[#lst + 1] = string.format("%q", d) end
        parts[#parts + 1] = "includes={" .. table.concat(lst, ",") .. "}"
    end
    if opts.system_libs then
        local lst = {}
        for _, d in ipairs(opts.system_libs) do lst[#lst + 1] = string.format("%q", d) end
        parts[#parts + 1] = "system_libs={" .. table.concat(lst, ",") .. "}"
    end
    return "{" .. table.concat(parts, ",") .. "}"
end

-- Build the produce body string for a given kind/name/opts.
local function produce_body(kind, name, opts, extra_flag)
    local opts_lua    = opts_to_lua_literal(opts)
    local extra_lit   = extra_flag and string.format("%q", extra_flag) or "nil"
    local cc_key      = toolchain.get_probe_key()
    local cc_key_lit  = string.format("%q", cc_key)
    return string.format([[
        local h = require("cook_cc._check_helpers")
        local KIND  = %q
        local NAME  = %q
        local OPTS  = %s
        local EXTRA = %s
        local cc    = cook.cache.get(%s)
        if not cc then error("[cc.check] compiler probe not resolved: " .. %s) end

        local probe_dir = ".cook/probes/cc-check"
        local fp        = h.fingerprint(OPTS)
        local probe_c   = probe_dir .. "/" .. KIND .. "-" .. fp .. ".c"
        local probe_out = probe_dir .. "/" .. KIND .. "-" .. fp .. ".out"
        fs.mkdir_p(probe_dir)
        fs.write(probe_c, h.probe_c(KIND, NAME, OPTS))

        local cmd = h.compile_command(KIND, cc, probe_c, probe_out, OPTS, EXTRA)
        local compile_ok = pcall(cook.sh, cmd)

        local captured = ""
        if compile_ok and h.runs(KIND) then
            local run_ok, out = pcall(cook.sh, probe_out)
            if run_ok then captured = out or "" else compile_ok = false end
        end
        return h.evaluate(KIND, compile_ok, captured)
    ]], kind, name, opts_lua, extra_lit, cc_key_lit, cc_key_lit)
end

local function register_check(kind, name, opts, extra_flag)
    toolchain.ensure_probe_registered()
    local fp  = helpers.fingerprint(opts)
    local key = "cc:check:" .. kind .. ":" .. name .. ":" .. fp
    if M._registered[key] then
        return key
    end
    cook.probe(key, {
        inputs = {
            requires = { toolchain.get_probe_key() },
            env      = { "PATH" },
        },
        produce = produce_body(kind, name, opts, extra_flag),
    })
    M._registered[key] = true
    return key
end

-- Strict probe-key <name> sanitiser. The sigil resolver in
-- cli/crates/cook-luagen/src/resolver.rs splits "$<key.field>" at the
-- first '.' after the colon section, so '.' MUST NOT appear in the
-- <name> segment of a probe key. Allowed charset: [A-Za-z0-9_+-].
local function sanitise_name(s)
    return (s:gsub("[^%w%+%-_]", "_"))
end

function M.has_header(name, opts)
    local key = register_check("has-header", sanitise_name(name), opts or {})
    return "$<" .. key .. ">"
end

function M.has_function(name, opts)
    local key = register_check("has-function", sanitise_name(name), opts or {})
    return "$<" .. key .. ">"
end

function M.has_define(name, opts)
    local key = register_check("has-define", sanitise_name(name), opts or {})
    return "$<" .. key .. ">"
end

function M.sizeof(type_name, opts)
    local key = register_check("sizeof", sanitise_name(type_name), opts or {})
    return "$<" .. key .. ">"
end

function M.endian()
    local key = register_check("endian", "_", {})
    return "$<" .. key .. ">"
end

function M.has_compile_flag(flag)
    local name = sanitise_name(flag)
    local key  = register_check("has-compile-flag", name, {}, flag)
    return "$<" .. key .. ">"
end

function M.has_link_flag(flag)
    local name = sanitise_name(flag)
    local key  = register_check("has-link-flag", name, {}, flag)
    return "$<" .. key .. ">"
end

return M
