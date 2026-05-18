-- Vendored standalone renderer for cc.config_header. Implements the
-- @VAR@, ${VAR}, #cmakedefine, #cmakedefine01 substitution rules of
-- CMake's configure_file. Truthiness uses Lua semantics (false/nil
-- are false; everything else — including 0 and "" — is true).

local M = {}

local function substitute_vars(line, vars)
    line = line:gsub("@([%w_]+)@", function(name)
        local v = vars[name]
        if v == nil then return "" end
        return tostring(v)
    end)
    line = line:gsub("%${([%w_]+)}", function(name)
        local v = vars[name]
        if v == nil then return "" end
        return tostring(v)
    end)
    return line
end

local function process_line(line, vars)
    -- #cmakedefine01 SYMBOL
    local sym01 = line:match("^%s*#cmakedefine01%s+([%w_]+)%s*$")
    if sym01 then
        local truthy = vars[sym01] and true or false
        return "#define " .. sym01 .. " " .. (truthy and "1" or "0")
    end
    -- #cmakedefine SYMBOL [VALUE...]
    local sym, rest = line:match("^%s*#cmakedefine%s+([%w_]+)(.*)$")
    if sym then
        local truthy = vars[sym] and true or false
        if not truthy then
            return "/* #undef " .. sym .. " */"
        end
        rest = substitute_vars(rest or "", vars)
        rest = rest:gsub("^%s+", "")
        if rest == "" then
            return "#define " .. sym
        end
        return "#define " .. sym .. " " .. rest
    end
    return substitute_vars(line, vars)
end

function M.render(template_path, output_path, vars)
    vars = vars or {}
    local content = fs.read(template_path)
    if not content then
        error("[cc.config_header] template not found: " .. template_path)
    end
    local out_lines = {}
    -- Preserve trailing newline behaviour by iterating with explicit boundary.
    local pos = 1
    while pos <= #content do
        local nl = content:find("\n", pos, true)
        local line
        if nl then
            line = content:sub(pos, nl - 1)
            pos = nl + 1
        else
            line = content:sub(pos)
            pos = #content + 1
        end
        out_lines[#out_lines + 1] = process_line(line, vars)
        if nl then out_lines[#out_lines + 1] = "\n" end
    end
    fs.mkdir_p((output_path:match("(.+)/[^/]+$")) or ".")
    fs.write(output_path, table.concat(out_lines))
end

-- Standalone-script entry point. When the file is invoked as
-- `lua /path/to/config_header_renderer.lua TEMPLATE OUTPUT VARS_LITERAL`
-- (not require()d), arg[0] is the script path. Run render and exit.
if arg and arg[0] and arg[0]:match("config_header_renderer%.lua$") and arg[1] then
    local template = arg[1]
    local output   = arg[2]
    local vars_lit = arg[3] or "{}"
    local chunk, err = load("return " .. vars_lit, "vars", "t")
    if not chunk then
        error("[cc.config_header] vars-literal parse error: " .. err)
    end
    local vars = chunk()
    -- The standalone path uses raw io operations rather than the
    -- cook.* sandbox APIs because it runs as a subprocess.
    local function read_file(p)
        local f = assert(io.open(p, "r"))
        local s = f:read("*a"); f:close(); return s
    end
    local function write_file(p, c)
        local dir = p:match("(.+)/[^/]+$")
        if dir then os.execute("mkdir -p " .. ("'" .. dir:gsub("'", "'\\''") .. "'")) end
        local f = assert(io.open(p, "w"))
        f:write(c); f:close()
    end
    fs = fs or { read = read_file, write = write_file, mkdir_p = function() end, exists = function() return true end }
    M.render(template, output, vars)
end

return M
