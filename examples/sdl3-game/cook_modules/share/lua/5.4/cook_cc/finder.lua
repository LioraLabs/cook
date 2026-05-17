local toolchain = require("cook_cc.toolchain")

local M = {}

-- Per-VM project-registered finder registry.
M._registry = M._registry or {}

-- Per-VM probe-registration tracking: probe-key -> opts-fingerprint
-- (lets find() detect conflicting subsequent calls per spec §3.4).
M._registered = M._registered or {}

local function canonical_opts(opts)
    if not opts or not next(opts) then return "{}" end
    local keys = {}
    for k in pairs(opts) do keys[#keys + 1] = tostring(k) end
    table.sort(keys)
    local parts = {}
    for _, k in ipairs(keys) do
        local v = opts[k]
        if type(v) == "table" then v = table.concat(v, ",") end
        parts[#parts + 1] = k .. "=" .. tostring(v)
    end
    return "{" .. table.concat(parts, ",") .. "}"
end

local function sigil_record(name)
    local fields = { "cflags", "libs", "system_libs", "include_dirs", "lib_dirs",
                     "frameworks", "version", "found", "tried" }
    local backing = {}
    for _, f in ipairs(fields) do
        backing[f] = string.format("$<cc:find:%s.%s>", name, f)
    end
    -- Proxy through an empty table so __newindex fires on every write
    -- (including writes to keys that have a sigil value).
    return setmetatable({}, {
        __index = backing,
        __newindex = function(_, k, _)
            error("[cc.find] cannot mutate find result; field '" .. tostring(k) .. "' is a probe-sigil placeholder", 2)
        end,
        __pairs = function() return pairs(backing) end,
    })
end

local function opts_to_lua_literal(opts)
    if not opts or not next(opts) then return "{}" end
    local parts = {}
    for k, v in pairs(opts) do
        if type(v) == "string" then
            parts[#parts + 1] = k .. "=" .. string.format("%q", v)
        elseif type(v) == "boolean" or type(v) == "number" then
            parts[#parts + 1] = k .. "=" .. tostring(v)
        end
    end
    return "{" .. table.concat(parts, ",") .. "}"
end

local function produce_body(name, opts, raise_on_miss)
    local opts_lua = opts_to_lua_literal(opts)
    local raise_block
    if raise_on_miss then
        raise_block = [[
        local lines = { "could not locate '" .. NAME .. "':" }
        for _, a in ipairs(tried) do
            local line = "  - " .. a.strategy .. ": " .. a.outcome
            if a.reason and a.reason ~= "" then line = line .. " (" .. a.reason .. ")" end
            lines[#lines + 1] = line
        end
        error("[cc.find_or_error] " .. table.concat(lines, "\n"))
]]
    else
        raise_block = "        return h.build_result(nil, tried)"
    end
    return string.format([[
        local h = require("cook_cc._probe_helpers")
        local NAME = %q
        local OPTS = %s
        local tried = {}
        local function try(step) local r = step(); tried[#tried+1] = r; return r end

        -- Project finders are register-VM-only; worker VM passes empty registry.
        local strategies = {
            function() return h.curated_strategy(NAME, OPTS) end,
        }
        if not OPTS.cmake then
            strategies[#strategies+1] = function() return h.pkg_strategy(NAME, OPTS) end
        end
        strategies[#strategies+1] = function() return h.cmake_strategy(NAME, OPTS) end
        strategies[#strategies+1] = function() return h.bare_strategy(NAME, OPTS) end

        for _, step in ipairs(strategies) do
            local r = try(step)
            if r.outcome == "hit" then return h.build_result(r, tried) end
        end
%s
    ]], name, opts_lua, raise_block)
end

local function register_find_probe(name, opts, raise_on_miss)
    -- Ensure dependency-probes are registered so requires-edges resolve.
    toolchain.ensure_probe_registered()
    require("cook_cc.finders.bare_probe")
    require("cook_cc.finders.cmake_compat")

    cook.probe("cc:find:" .. name, {
        inputs = {
            requires = {
                toolchain.get_probe_key(),
                "cc:linker-search-dirs",
                "cc:cmake-driver",
            },
            env = { "PATH", "PKG_CONFIG_PATH", "CMAKE_PREFIX_PATH", "LIBRARY_PATH" },
            tools = { "pkg-config" },
        },
        produce = produce_body(name, opts, raise_on_miss),
    })
end

function M.register(name, finder)
    if type(finder) ~= "function" then
        error("[cc.register_finder] register_finder for '" .. tostring(name)
              .. "' requires a function, got " .. type(finder), 2)
    end
    M._registry[name] = finder
end

function M.find(name, opts)
    opts = opts or {}
    local key = "cc:find:" .. name
    local fp  = canonical_opts(opts)
    if M._registered[key] then
        if M._registered[key] ~= fp then
            error(string.format(
                "[cc.find] duplicate cc.find for '%s' with conflicting opts:\n" ..
                "  first call opts=%s\n  this call opts=%s",
                name, M._registered[key], fp), 2)
        end
        return sigil_record(name)
    end
    register_find_probe(name, opts, false)
    M._registered[key] = fp
    return sigil_record(name)
end

function M.find_or_error(name, opts)
    opts = opts or {}
    local key = "cc:find:" .. name
    local fp  = canonical_opts(opts) .. ":or_error"
    if M._registered[key] then
        if M._registered[key] ~= fp then
            error(string.format(
                "[cc.find_or_error] '%s' previously declared with conflicting opts or non-or_error semantics",
                name), 2)
        end
        return sigil_record(name)
    end
    register_find_probe(name, opts, true)
    M._registered[key] = fp
    return sigil_record(name)
end

return M
