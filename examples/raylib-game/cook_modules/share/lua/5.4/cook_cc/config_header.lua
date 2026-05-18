local M = {}

local function renderer_path()
    local p = package.searchpath("cook_cc.config_header_renderer", package.path)
    if not p then
        error("[cc.config_header] cannot locate cook_cc.config_header_renderer on package.path", 2)
    end
    return p
end

local function shell_quote(s)
    -- Single-quote and escape any embedded single quotes.
    return "'" .. (s:gsub("'", "'\\''")) .. "'"
end

local function value_to_lua_literal(v)
    local t = type(v)
    if t == "string" then
        local probe = v:match("^%$<(cc:check:.+)>$")
        if probe then
            -- Sigil — emit verbatim so the sigil pipeline expands it.
            return v, probe
        end
        return string.format("%q", v), nil
    elseif t == "boolean" or t == "number" then
        return tostring(v), nil
    elseif t == "nil" then
        return "nil", nil
    end
    error("[cc.config_header] unsupported var type: " .. t)
end

local function build_vars_literal(vars)
    local entries = {}
    local probes  = {}
    -- Stable ordering for fingerprint determinism.
    local keys = {}
    for k in pairs(vars) do keys[#keys + 1] = k end
    table.sort(keys)
    for _, k in ipairs(keys) do
        local lit, probe_key = value_to_lua_literal(vars[k])
        entries[#entries + 1] = k .. " = " .. lit
        if probe_key then probes[#probes + 1] = probe_key end
    end
    return "{ " .. table.concat(entries, ", ") .. " }", probes
end

local function recipe_name_for(output)
    -- Synthesize a stable recipe name from the output path so consumers can
    -- declare a `requires` against it. Sanitize separators / dots into _.
    local sanitized = output:gsub("[/.]", "_")
    return "__cc_config_header__" .. sanitized
end

function M.config_header(template, output, vars)
    vars = vars or {}
    local vars_literal, probes = build_vars_literal(vars)
    local cmd = "lua " .. renderer_path()
        .. " " .. template
        .. " " .. output
        .. " " .. shell_quote(vars_literal)
    local recipe = recipe_name_for(output)
    cook.recipe(recipe, { requires = {} }, function()
        cook.add_unit({
            inputs  = { template },
            output  = output,
            command = cmd,
            probes  = probes,
        })
    end)
    return recipe
end

setmetatable(M, { __call = function(_, ...) return M.config_header(...) end })

return M
