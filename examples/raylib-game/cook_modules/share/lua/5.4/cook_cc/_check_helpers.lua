-- Shared helpers for cc:check:<kind>:<name>:<short-fp> probe bodies.
-- Required by both register-phase (cook_cc.checks) and execute-phase
-- (the probe produce body on a worker VM).

local M = {}

-- Tiny FNV-1a 32-bit hash; returns 8-char lowercase hex.
local function fnv1a(s)
    local h = 0x811C9DC5
    for i = 1, #s do
        h = (h ~ s:byte(i)) & 0xFFFFFFFF
        h = (h * 0x01000193) & 0xFFFFFFFF
    end
    return string.format("%08x", h)
end

local function list_to_str(t)
    if not t then return "" end
    local parts = {}
    for _, v in ipairs(t) do parts[#parts + 1] = tostring(v) end
    return table.concat(parts, ",")
end

function M.fingerprint(opts)
    opts = opts or {}
    local parts = {
        "std=" .. (opts.standard or ""),
        "cf=" .. (opts.extra_cflags or ""),
        "lf=" .. (opts.extra_ldflags or ""),
        "df=" .. list_to_str(opts.defines),
        "in=" .. list_to_str(opts.includes),
        "sl=" .. list_to_str(opts.system_libs),
    }
    return fnv1a(table.concat(parts, "|"))
end

local function include_block(opts)
    if not opts or not opts.includes then return "" end
    local lines = {}
    for _, h in ipairs(opts.includes) do
        lines[#lines + 1] = "#include <" .. h .. ">"
    end
    return table.concat(lines, "\n") .. "\n"
end

function M.probe_c(kind, name, opts)
    opts = opts or {}
    if kind == "has-header" then
        return include_block(opts)
            .. "#include <" .. name .. ">\n"
            .. "int main(void){return 0;}\n"
    elseif kind == "has-function" then
        return include_block(opts)
            .. "extern void (*__probe_ref)(void);\n"
            .. "int main(void){__probe_ref = (void(*)(void))(void*)" .. name .. "; return 0;}\n"
    elseif kind == "has-define" then
        return include_block(opts)
            .. "#ifndef " .. name .. "\n"
            .. "#error \"" .. name .. " not defined\"\n"
            .. "#endif\n"
            .. "int main(void){return 0;}\n"
    elseif kind == "sizeof" then
        return include_block(opts)
            .. "#include <stdio.h>\n"
            .. "int main(void){printf(\"%d\\n\", (int)sizeof(" .. name .. ")); return 0;}\n"
    elseif kind == "endian" then
        return "#include <stdio.h>\n"
            .. "int main(void){\n"
            .. "  const int x = 1;\n"
            .. "  if (*(const char*)&x == 1) printf(\"little\\n\");\n"
            .. "  else printf(\"big\\n\");\n"
            .. "  return 0;\n"
            .. "}\n"
    elseif kind == "has-compile-flag" or kind == "has-link-flag" then
        return "int main(void){return 0;}\n"
    end
    error("[cc.checks] unknown kind '" .. tostring(kind) .. "'")
end

-- kinds that link (need to invoke the linker, not just -c)
local LINK_KINDS = { ["has-function"] = true, ["has-link-flag"] = true, ["endian"] = true, ["sizeof"] = true }

-- kinds whose value is determined by running the compiled binary
local RUN_KINDS = { ["sizeof"] = true, ["endian"] = true }

function M.compile_command(kind, compiler, probe_path, output_path, opts, extra_flag)
    opts = opts or {}
    local cxx = compiler.cxx
    local parts = { cxx }
    if not LINK_KINDS[kind] then
        parts[#parts + 1] = "-c"
    end
    if opts.standard then parts[#parts + 1] = "-std=" .. opts.standard end
    for _, d in ipairs(opts.defines or {}) do parts[#parts + 1] = "-D" .. d end
    if opts.extra_cflags and opts.extra_cflags ~= "" then
        parts[#parts + 1] = opts.extra_cflags
    end
    if kind == "has-compile-flag" then
        parts[#parts + 1] = "-Werror"
        parts[#parts + 1] = extra_flag
    end
    parts[#parts + 1] = probe_path
    parts[#parts + 1] = "-o"
    parts[#parts + 1] = output_path
    if LINK_KINDS[kind] then
        if opts.extra_ldflags and opts.extra_ldflags ~= "" then
            parts[#parts + 1] = opts.extra_ldflags
        end
        if kind == "has-link-flag" then
            parts[#parts + 1] = extra_flag
        end
        for _, lib in ipairs(opts.system_libs or {}) do
            parts[#parts + 1] = "-l" .. lib
        end
    end
    return table.concat(parts, " ") .. " 2>/dev/null"
end

function M.runs(kind) return RUN_KINDS[kind] == true end

function M.evaluate(kind, compile_ok, captured)
    if kind == "has-header" or kind == "has-function" or kind == "has-define"
       or kind == "has-compile-flag" or kind == "has-link-flag" then
        return compile_ok and true or false
    end
    if not compile_ok then return nil end
    if kind == "sizeof" then
        local n = tonumber((captured or ""):match("(%-?%d+)"))
        return n
    end
    if kind == "endian" then
        local v = (captured or ""):match("^%s*(%a+)")
        return v
    end
    error("[cc.checks] unknown kind in evaluate: " .. tostring(kind))
end

return M
