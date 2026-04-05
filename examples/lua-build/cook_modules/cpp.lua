-- Cook C++ Module
-- Provides compiler detection, compilation with header dep tracking,
-- static/shared/interface libraries, executables, C++20 modules,
-- transitive link resolution, and compile_commands.json generation.
--
-- Module-level defaults via env vars (set in config blocks or via --set):
--   CPP_DEFINES      — whitespace-separated macro names, prepended to each target's defines
--   CPP_INCLUDES     — whitespace-separated include paths, prepended to each target's includes
--   CPP_SYSTEM_LIBS  — whitespace-separated lib names, prepended to each target's system_libs
--   CPP_STANDARD     — scalar (e.g., "c++17"); used when per-target standard is absent
--   CPP_WARNINGS     — scalar ("strict"|"default"|"none" or raw flags); used when per-target warnings is absent

local cpp = {}

-- ---------------------------------------------------------------------------
-- Internal state
-- ---------------------------------------------------------------------------
cpp.state = {
    compiler = nil,           -- { cxx = "g++", cc = "gcc" }
    default_standard = nil,   -- e.g., "c++17"
    warnings = "default",     -- "default" | "strict" | "none" | raw string
}

-- ---------------------------------------------------------------------------
-- Helpers
-- ---------------------------------------------------------------------------

--- Split a whitespace-separated env var into a list. Nil/empty → empty list.
local function env_list(name)
    local v = cook.env[name]
    if v == nil or v == "" then return {} end
    local out = {}
    for word in v:gmatch("%S+") do
        out[#out + 1] = word
    end
    return out
end

--- Read a scalar env var. Nil/empty → nil.
local function env_scalar(name)
    local v = cook.env[name]
    if v == nil or v == "" then return nil end
    return v
end

--- Convert absolute path to relative (strips working directory prefix)
local function to_relative(abs_path)
    local wd = cook.sh("pwd"):gsub("%s+$", "") .. "/"
    if abs_path:sub(1, #wd) == wd then
        return abs_path:sub(#wd + 1)
    end
    return abs_path
end

--- Get warning flags for the given preset (or cpp.state.warnings if nil)
local function warning_flags(w)
    w = w or cpp.state.warnings
    if w == "default" then return "-Wall"
    elseif w == "strict" then return "-Wall -Wextra -Wpedantic"
    elseif w == "none" then return ""
    else return w end  -- raw string
end

--- Check if a file is a C source (not C++)
local function is_c_source(path)
    return path:match("%.[cC]$") ~= nil
end

--- Pick the right compiler for a source file
local function compiler_for(source)
    if is_c_source(source) then
        return cpp.state.compiler.cc
    else
        return cpp.state.compiler.cxx
    end
end

--- Get user flags for the source type
local function user_flags_for(source)
    if is_c_source(source) then
        return (cook.env.CFLAGS or "")
    else
        return (cook.env.CXXFLAGS or "")
    end
end

--- Collect source files from a directory (recursive glob)
local function collect_sources(dir)
    local sources = {}
    local extensions = { "*.cpp", "*.c", "*.cc", "*.cxx" }
    for _, ext in ipairs(extensions) do
        local pattern = dir .. "**/" .. ext
        local matches = fs.glob(pattern)
        for i = 1, #matches do
            sources[#sources + 1] = to_relative(matches[i])
        end
    end
    return sources
end

--- Register a target name in the persistent known_targets list (deduplicated)
local function register_target(name)
    local known = cook.cache.get("known_targets") or {}
    for _, existing in ipairs(known) do
        if existing == name then return end
    end
    known[#known + 1] = name
    cook.cache.set("known_targets", known)
end

-- ---------------------------------------------------------------------------
-- Depfile parsing (header dependency tracking)
-- ---------------------------------------------------------------------------

--- Parse a Make-format .d depfile, returning a list of header paths.
--- Filters out system headers (absolute paths) and the source file itself.
--- Skips headers that no longer exist (stale depfile).
local function parse_depfile(dep_path, source_path)
    if not fs.exists(dep_path) then return {} end
    local content = fs.read(dep_path)
    -- Strip "target: " prefix and join continuation lines
    local deps_str = content:gsub("^.-:%s*", ""):gsub("\\\n", " "):gsub("\\\r\n", " ")
    local deps = {}
    for dep in deps_str:gmatch("%S+") do
        if not dep:match("^/") and dep ~= source_path then
            if fs.exists(dep) then
                deps[#deps + 1] = dep
            end
        end
    end
    return deps
end

-- ---------------------------------------------------------------------------
-- Minimal JSON encoder (for compile_commands.json)
-- ---------------------------------------------------------------------------

local function json_escape(s)
    return s:gsub("\\", "\\\\"):gsub('"', '\\"'):gsub("\n", "\\n"):gsub("\r", "\\r"):gsub("\t", "\\t")
end

local function json_encode_value(val)
    if type(val) == "string" then
        return '"' .. json_escape(val) .. '"'
    elseif type(val) == "number" then
        return tostring(val)
    elseif type(val) == "boolean" then
        return val and "true" or "false"
    elseif type(val) == "table" then
        -- Check if array (sequential integer keys)
        local is_array = true
        local max_i = 0
        for k, _ in pairs(val) do
            if type(k) == "number" and k == math.floor(k) and k > 0 then
                if k > max_i then max_i = k end
            else
                is_array = false
                break
            end
        end
        if is_array and max_i > 0 then
            local parts = {}
            for i = 1, max_i do
                parts[i] = json_encode_value(val[i])
            end
            return "[" .. table.concat(parts, ",") .. "]"
        else
            local parts = {}
            for k, v in pairs(val) do
                parts[#parts + 1] = '"' .. json_escape(tostring(k)) .. '":' .. json_encode_value(v)
            end
            return "{" .. table.concat(parts, ",") .. "}"
        end
    elseif val == nil then
        return "null"
    else
        return '"' .. tostring(val) .. '"'
    end
end

local function json_encode(val)
    return json_encode_value(val)
end

local function json_encode_pretty(val, indent)
    indent = indent or ""
    local next_indent = indent .. "  "
    if type(val) == "table" then
        -- Check if array
        local is_array = true
        local max_i = 0
        for k, _ in pairs(val) do
            if type(k) == "number" and k == math.floor(k) and k > 0 then
                if k > max_i then max_i = k end
            else
                is_array = false
                break
            end
        end
        if is_array and max_i > 0 then
            local parts = {}
            for i = 1, max_i do
                parts[i] = next_indent .. json_encode_pretty(val[i], next_indent)
            end
            return "[\n" .. table.concat(parts, ",\n") .. "\n" .. indent .. "]"
        else
            local parts = {}
            for k, v in pairs(val) do
                parts[#parts + 1] = next_indent .. '"' .. json_escape(tostring(k)) .. '": ' .. json_encode_pretty(v, next_indent)
            end
            return "{\n" .. table.concat(parts, ",\n") .. "\n" .. indent .. "}"
        end
    else
        return json_encode_value(val)
    end
end

-- ---------------------------------------------------------------------------
-- Compiler detection
-- ---------------------------------------------------------------------------

local function detect_compiler()
    local candidates = {
        { cxx = "clang++", cc = "clang" },
        { cxx = "g++", cc = "gcc" },
        { cxx = "c++", cc = "cc" },
    }
    for _, candidate in ipairs(candidates) do
        local ok = pcall(function() cook.sh("command -v " .. candidate.cxx) end)
        if ok then
            return candidate
        end
    end
    error("cpp: no C/C++ compiler found (tried clang++, g++, c++)")
end

function cpp.init()
    local cached = cook.cache.get("compiler")
    if cached then
        local ok = pcall(function() cook.sh("command -v " .. cached.cxx) end)
        if ok then
            cpp.state.compiler = cached
            return
        end
    end
    cpp.state.compiler = detect_compiler()
    cook.cache.set("compiler", cpp.state.compiler)
end

-- ---------------------------------------------------------------------------
-- Toolchain override
-- ---------------------------------------------------------------------------

function cpp.toolchain(opts)
    opts = opts or {}
    if opts.compiler then
        local cxx = opts.compiler
        local cc
        if cxx:match("clang") then cc = "clang"
        elseif cxx:match("g%+%+") then cc = "gcc"
        else cc = "cc" end
        cpp.state.compiler = { cxx = cxx, cc = cc }
    end
    if opts.standard then cpp.state.default_standard = opts.standard end
    if opts.warnings then cpp.state.warnings = opts.warnings end
end

-- ---------------------------------------------------------------------------
-- Core build functions
-- ---------------------------------------------------------------------------

--- Compile a single source file to an object.
--- Returns the output object path (synchronously — compilation is deferred).
function cpp.compile(source, opts)
    opts = opts or {}
    if not fs.exists(source) then
        error("cpp.compile: source file not found: " .. source)
    end
    local target_name = opts.target_name or "default"
    local stem = path.stem(source)
    local obj_dir = "build/obj/" .. target_name
    local obj_out = opts.output or (obj_dir .. "/" .. stem .. ".o")
    local dep_dir = ".cook/deps/" .. target_name
    local dep_file = dep_dir .. "/" .. stem .. ".d"

    -- Ensure output directories exist
    fs.mkdir_p(obj_dir)
    fs.mkdir_p(dep_dir)

    -- Pick compiler
    local compiler = compiler_for(source)

    -- Build flags
    local flags = { "-c" }
    flags[#flags + 1] = "-MMD"
    flags[#flags + 1] = "-MF"
    flags[#flags + 1] = dep_file

    local std = opts.standard or cpp.state.default_standard
    if std and not is_c_source(source) then flags[#flags + 1] = "-std=" .. std end

    for _, inc in ipairs(opts.includes or {}) do
        flags[#flags + 1] = "-I" .. inc
    end
    for _, def in ipairs(opts.defines or {}) do
        flags[#flags + 1] = "-D" .. def
    end

    -- Warning flags (opts.warnings wins over cpp.state.warnings)
    local wflags = warning_flags(opts.warnings)
    if wflags ~= "" then flags[#flags + 1] = wflags end

    -- PIC flag
    if opts.fpic then flags[#flags + 1] = "-fPIC" end

    -- Extra compile flags (e.g., from cpp.find())
    if opts.extra_cflags and opts.extra_cflags ~= "" then
        flags[#flags + 1] = opts.extra_cflags
    end

    -- User flags from config vars
    local uflags = user_flags_for(source)
    if uflags ~= "" then flags[#flags + 1] = uflags end

    local cmd = compiler .. " " .. table.concat(flags, " ") .. " " .. source .. " -o " .. obj_out

    -- Parse existing depfile for header inputs
    local inputs = { source }
    local header_deps = parse_depfile(dep_file, source)
    for _, h in ipairs(header_deps) do
        inputs[#inputs + 1] = h
    end

    cook.add_unit({
        inputs = inputs,
        output = obj_out,
        command = cmd,
    })

    return obj_out
end

--- Create a static library from object files.
function cpp.archive(objects, output)
    fs.mkdir_p(path.dir(output))
    local cmd = "ar rcs " .. output .. " " .. table.concat(objects, " ")
    cook.add_unit({
        inputs = objects,
        output = output,
        command = cmd,
    })
end

--- Link objects into an executable or shared library.
function cpp.link(objects, output, opts)
    opts = opts or {}
    fs.mkdir_p(path.dir(output))
    local compiler = cpp.state.compiler.cxx
    local parts = { compiler }

    -- Add objects
    for _, obj in ipairs(objects) do
        parts[#parts + 1] = obj
    end

    -- Shared library flag
    if opts.shared then
        if cook.platform.os == "macos" then
            parts[#parts + 1] = "-dynamiclib"
        else
            parts[#parts + 1] = "-shared"
        end
    end

    -- Library paths and libraries
    for _, lib in ipairs(opts.libs or {}) do
        parts[#parts + 1] = lib
    end

    -- System libraries
    for _, syslib in ipairs(opts.system_libs or {}) do
        parts[#parts + 1] = "-l" .. syslib
    end

    -- Rpath for shared libraries
    if opts.rpath_dirs then
        for _, rdir in ipairs(opts.rpath_dirs) do
            parts[#parts + 1] = "-Wl,-rpath," .. rdir
        end
    end

    -- Extra link flags
    if opts.extra_ldflags and opts.extra_ldflags ~= "" then
        parts[#parts + 1] = opts.extra_ldflags
    end

    -- User link flags
    local ldflags = cook.env.LDFLAGS or ""
    if ldflags ~= "" then parts[#parts + 1] = ldflags end

    parts[#parts + 1] = "-o"
    parts[#parts + 1] = output

    local all_inputs = {}
    for _, o in ipairs(objects) do all_inputs[#all_inputs + 1] = o end
    for _, l in ipairs(opts.libs or {}) do all_inputs[#all_inputs + 1] = l end

    cook.add_unit({
        inputs = all_inputs,
        output = output,
        command = table.concat(parts, " "),
    })
end

--- Discover a system library via pkg-config.
function cpp.find(name)
    local cached = cook.cache.get("pkg:" .. name)
    if cached then return cached end

    local ok, cflags = pcall(function()
        return cook.sh("pkg-config --cflags " .. name):gsub("%s+$", "")
    end)
    if not ok then
        error("cpp.find: package '" .. name .. "' not found via pkg-config")
    end
    local libs = cook.sh("pkg-config --libs " .. name):gsub("%s+$", "")
    local result = { cflags = cflags, libs = libs }
    cook.cache.set("pkg:" .. name, result)
    return result
end

-- ---------------------------------------------------------------------------
-- Transitive link resolution
-- ---------------------------------------------------------------------------

local function resolve_links(link_names)
    local visited = {}
    local includes = {}
    local defines = {}
    local lib_paths = {}
    local rpath_dirs = {}
    local system_libs = {}
    local extra_ldflags = {}

    local function walk(name)
        if visited[name] then return end
        visited[name] = true
        local info = cook.import(name)
        if not info then
            error("cpp: link target '" .. name .. "' not found (was it exported by a dependency recipe?)")
        end
        -- Add self first (dependent before dependencies — correct link order)
        for _, inc in ipairs(info.includes or {}) do
            includes[#includes + 1] = inc
        end
        for _, def in ipairs(info.defines or {}) do
            defines[#defines + 1] = def
        end
        if info.lib_path then
            lib_paths[#lib_paths + 1] = info.lib_path
            if info.shared then
                rpath_dirs[#rpath_dirs + 1] = path.dir(info.lib_path)
            end
        end
        for _, sl in ipairs(info.system_libs or {}) do
            system_libs[#system_libs + 1] = sl
        end
        for _, lf in ipairs(info.extra_ldflags or {}) do
            extra_ldflags[#extra_ldflags + 1] = lf
        end
        -- Then recurse into transitive deps
        for _, dep in ipairs(info.links or {}) do
            walk(dep)
        end
    end

    for _, name in ipairs(link_names) do
        walk(name)
    end
    return includes, defines, lib_paths, rpath_dirs, system_libs, extra_ldflags
end

-- ---------------------------------------------------------------------------
-- Declarative helpers
-- ---------------------------------------------------------------------------

function cpp.static_library(name, opts)
    opts = opts or {}
    local sources = opts.sources or {}
    if opts.dir then
        sources = collect_sources(opts.dir)
    end
    if #sources == 0 then
        error("cpp.static_library: no sources found for target '" .. name .. "'")
    end

    -- Read env-var defaults: list fields prepended, scalars used when opts.X is absent.
    local env_defines     = env_list("CPP_DEFINES")
    local env_includes    = env_list("CPP_INCLUDES")
    local env_system_libs = env_list("CPP_SYSTEM_LIBS")
    local env_standard    = env_scalar("CPP_STANDARD")
    local env_warnings    = env_scalar("CPP_WARNINGS")

    local includes = {}
    for _, inc in ipairs(env_includes)        do includes[#includes + 1] = inc end
    for _, inc in ipairs(opts.includes or {}) do includes[#includes + 1] = inc end
    local defines = {}
    for _, def in ipairs(env_defines)        do defines[#defines + 1] = def end
    for _, def in ipairs(opts.defines or {}) do defines[#defines + 1] = def end
    local standard = opts.standard or env_standard or cpp.state.default_standard or "c++17"
    local warnings = opts.warnings or env_warnings  -- nil → warning_flags falls back to cpp.state.warnings

    -- Resolve linked targets for includes
    if opts.links then
        local link_incs, link_defs = resolve_links(opts.links)
        for _, inc in ipairs(link_incs) do includes[#includes + 1] = inc end
        for _, def in ipairs(link_defs) do defines[#defines + 1] = def end
    end

    local output = opts.output or ("build/lib/lib" .. name .. ".a")
    local objects = {}

    -- C++20 modules: two step groups
    if opts.modules and #opts.modules > 0 then
        if cpp.state.compiler.cxx:match("g%+%+") then
            error("cpp: C++20 modules require Clang (detected GCC). Set cpp.toolchain({ compiler = 'clang++' })")
        end
        local bmi_dir = "build/bmi/" .. name
        fs.mkdir_p(bmi_dir)

        -- Step group 1: compile module interfaces
        local module_objects = {}
        cook.step_group(function()
            for _, mod_src in ipairs(opts.modules) do
                -- Compile module interface: produce BMI + object
                local mstem = path.stem(mod_src)
                local bmi_path = bmi_dir .. "/" .. mstem .. ".pcm"
                local obj_path = "build/obj/" .. name .. "/" .. mstem .. ".o"
                fs.mkdir_p("build/obj/" .. name)
                fs.mkdir_p(".cook/deps/" .. name)

                local compiler = compiler_for(mod_src)
                local mflags = { "--precompile" }
                mflags[#mflags + 1] = "-std=" .. standard
                for _, inc in ipairs(includes) do mflags[#mflags + 1] = "-I" .. inc end
                for _, def in ipairs(defines) do mflags[#mflags + 1] = "-D" .. def end
                local wf = warning_flags(warnings)
                if wf ~= "" then mflags[#mflags + 1] = wf end
                mflags[#mflags + 1] = "-MMD"
                mflags[#mflags + 1] = "-MF"
                mflags[#mflags + 1] = ".cook/deps/" .. name .. "/" .. mstem .. ".d"

                -- Produce BMI
                local bmi_cmd = compiler .. " " .. table.concat(mflags, " ")
                    .. " " .. mod_src .. " -o " .. bmi_path
                local bmi_inputs = { mod_src }
                local mod_deps = parse_depfile(".cook/deps/" .. name .. "/" .. mstem .. ".d", mod_src)
                for _, h in ipairs(mod_deps) do bmi_inputs[#bmi_inputs + 1] = h end
                cook.add_unit({ inputs = bmi_inputs, output = bmi_path, command = bmi_cmd })

                -- Compile BMI to object
                local obj_cmd = compiler .. " -c " .. bmi_path .. " -o " .. obj_path
                cook.add_unit({ inputs = { bmi_path }, output = obj_path, command = obj_cmd })

                module_objects[#module_objects + 1] = obj_path
            end
        end)

        -- Step group 2: compile regular sources with BMI search path
        cook.step_group(function()
            for _, src in ipairs(sources) do
                local compile_includes = {}
                for _, inc in ipairs(includes) do compile_includes[#compile_includes + 1] = inc end
                objects[#objects + 1] = cpp.compile(src, {
                    includes = compile_includes,
                    defines = defines,
                    standard = standard,
                    warnings = warnings,
                    target_name = name,
                    extra_cflags = "-fprebuilt-module-path=" .. bmi_dir,
                })
            end
        end)

        -- Add module objects to the full list
        for _, mo in ipairs(module_objects) do objects[#objects + 1] = mo end
    else
        -- No modules: single step group
        cook.step_group(function()
            for _, src in ipairs(sources) do
                objects[#objects + 1] = cpp.compile(src, {
                    includes = includes,
                    defines = defines,
                    standard = standard,
                    warnings = warnings,
                    target_name = name,
                })
            end
        end)
    end

    -- Archive
    cpp.archive(objects, output)

    -- Track target
    register_target(name)

    -- Export
    cook.export(name, {
        includes = opts.export_includes or {},
        defines = opts.export_defines or {},
        lib_path = output,
        links = opts.links or {},
        compile_info = {
            sources = sources,
            includes = includes,
            defines = defines,
            standard = standard,
            compiler = cpp.state.compiler.cxx,
        },
    })
end

function cpp.shared_library(name, opts)
    opts = opts or {}
    local sources = opts.sources or {}
    if opts.dir then
        sources = collect_sources(opts.dir)
    end
    if #sources == 0 then
        error("cpp.shared_library: no sources found for target '" .. name .. "'")
    end

    -- Read env-var defaults (see cpp.static_library for semantics).
    local env_defines     = env_list("CPP_DEFINES")
    local env_includes    = env_list("CPP_INCLUDES")
    local env_system_libs = env_list("CPP_SYSTEM_LIBS")
    local env_standard    = env_scalar("CPP_STANDARD")
    local env_warnings    = env_scalar("CPP_WARNINGS")

    local includes = {}
    for _, inc in ipairs(env_includes)        do includes[#includes + 1] = inc end
    for _, inc in ipairs(opts.includes or {}) do includes[#includes + 1] = inc end
    local defines = {}
    for _, def in ipairs(env_defines)        do defines[#defines + 1] = def end
    for _, def in ipairs(opts.defines or {}) do defines[#defines + 1] = def end
    local standard = opts.standard or env_standard or cpp.state.default_standard or "c++17"
    local warnings = opts.warnings or env_warnings

    if opts.links then
        local link_incs, link_defs = resolve_links(opts.links)
        for _, inc in ipairs(link_incs) do includes[#includes + 1] = inc end
        for _, def in ipairs(link_defs) do defines[#defines + 1] = def end
    end

    local ext = cook.platform.os == "macos" and ".dylib" or ".so"
    local output = opts.output or ("build/lib/lib" .. name .. ext)
    local objects = {}

    cook.step_group(function()
        for _, src in ipairs(sources) do
            objects[#objects + 1] = cpp.compile(src, {
                includes = includes,
                defines = defines,
                standard = standard,
                warnings = warnings,
                target_name = name,
                fpic = true,
            })
        end
    end)

    -- Resolve link dependencies
    local link_incs, link_defs, link_libs, rpath_dirs = resolve_links(opts.links or {})
    cpp.link(objects, output, {
        shared = true,
        libs = link_libs,
        system_libs = opts.system_libs,
        rpath_dirs = rpath_dirs,
        extra_ldflags = opts.extra_ldflags,
    })

    register_target(name)

    cook.export(name, {
        includes = opts.export_includes or {},
        defines = opts.export_defines or {},
        lib_path = output,
        shared = true,
        links = opts.links or {},
        compile_info = {
            sources = sources,
            includes = includes,
            defines = defines,
            standard = standard,
            compiler = cpp.state.compiler.cxx,
        },
    })
end

function cpp.executable(name, opts)
    opts = opts or {}
    local sources = opts.sources or {}
    if opts.dir then
        sources = collect_sources(opts.dir)
    end
    if #sources == 0 then
        error("cpp.executable: no sources found for target '" .. name .. "'")
    end

    -- Read env-var defaults (see cpp.static_library for semantics).
    local env_defines     = env_list("CPP_DEFINES")
    local env_includes    = env_list("CPP_INCLUDES")
    local env_system_libs = env_list("CPP_SYSTEM_LIBS")
    local env_standard    = env_scalar("CPP_STANDARD")
    local env_warnings    = env_scalar("CPP_WARNINGS")

    local includes = {}
    for _, inc in ipairs(env_includes)        do includes[#includes + 1] = inc end
    for _, inc in ipairs(opts.includes or {}) do includes[#includes + 1] = inc end
    local defines = {}
    for _, def in ipairs(env_defines)        do defines[#defines + 1] = def end
    for _, def in ipairs(opts.defines or {}) do defines[#defines + 1] = def end
    local standard = opts.standard or env_standard or cpp.state.default_standard or "c++17"
    local warnings = opts.warnings or env_warnings

    -- Resolve linked targets
    local link_incs, link_defs, link_libs, rpath_dirs = resolve_links(opts.links or {})
    for _, inc in ipairs(link_incs) do includes[#includes + 1] = inc end
    for _, def in ipairs(link_defs) do defines[#defines + 1] = def end

    local output = opts.output or ("build/bin/" .. name)
    local objects = {}

    -- C++20 modules support (same pattern as static_library)
    if opts.modules and #opts.modules > 0 then
        if cpp.state.compiler.cxx:match("g%+%+") then
            error("cpp: C++20 modules require Clang (detected GCC). Set cpp.toolchain({ compiler = 'clang++' })")
        end
        local bmi_dir = "build/bmi/" .. name
        fs.mkdir_p(bmi_dir)

        local module_objects = {}
        cook.step_group(function()
            for _, mod_src in ipairs(opts.modules) do
                local mstem = path.stem(mod_src)
                local bmi_path = bmi_dir .. "/" .. mstem .. ".pcm"
                local obj_path = "build/obj/" .. name .. "/" .. mstem .. ".o"
                fs.mkdir_p("build/obj/" .. name)
                fs.mkdir_p(".cook/deps/" .. name)

                local compiler = compiler_for(mod_src)
                local mflags = { "--precompile" }
                mflags[#mflags + 1] = "-std=" .. standard
                for _, inc in ipairs(includes) do mflags[#mflags + 1] = "-I" .. inc end
                for _, def in ipairs(defines) do mflags[#mflags + 1] = "-D" .. def end
                local wf = warning_flags(warnings)
                if wf ~= "" then mflags[#mflags + 1] = wf end
                mflags[#mflags + 1] = "-MMD"
                mflags[#mflags + 1] = "-MF"
                mflags[#mflags + 1] = ".cook/deps/" .. name .. "/" .. mstem .. ".d"

                local bmi_cmd = compiler .. " " .. table.concat(mflags, " ")
                    .. " " .. mod_src .. " -o " .. bmi_path
                local bmi_inputs = { mod_src }
                local mod_deps = parse_depfile(".cook/deps/" .. name .. "/" .. mstem .. ".d", mod_src)
                for _, h in ipairs(mod_deps) do bmi_inputs[#bmi_inputs + 1] = h end
                cook.add_unit({ inputs = bmi_inputs, output = bmi_path, command = bmi_cmd })

                local obj_cmd = compiler .. " -c " .. bmi_path .. " -o " .. obj_path
                cook.add_unit({ inputs = { bmi_path }, output = obj_path, command = obj_cmd })

                module_objects[#module_objects + 1] = obj_path
            end
        end)

        cook.step_group(function()
            for _, src in ipairs(sources) do
                objects[#objects + 1] = cpp.compile(src, {
                    includes = includes,
                    defines = defines,
                    standard = standard,
                    warnings = warnings,
                    target_name = name,
                    extra_cflags = "-fprebuilt-module-path=" .. bmi_dir
                        .. (opts.extra_cflags and (" " .. opts.extra_cflags) or ""),
                })
            end
        end)

        for _, mo in ipairs(module_objects) do objects[#objects + 1] = mo end
    else
        cook.step_group(function()
            for _, src in ipairs(sources) do
                objects[#objects + 1] = cpp.compile(src, {
                    includes = includes,
                    defines = defines,
                    standard = standard,
                    warnings = warnings,
                    target_name = name,
                    extra_cflags = opts.extra_cflags,
                })
            end
        end)
    end

    -- Link
    cpp.link(objects, output, {
        libs = link_libs,
        system_libs = opts.system_libs,
        rpath_dirs = rpath_dirs,
        extra_ldflags = opts.extra_ldflags,
    })

    register_target(name)

    cook.export(name, {
        includes = opts.export_includes or {},
        defines = opts.export_defines or {},
        links = opts.links or {},
        compile_info = {
            sources = sources,
            includes = includes,
            defines = defines,
            standard = standard,
            compiler = cpp.state.compiler.cxx,
        },
    })
end

function cpp.interface_library(name, opts)
    opts = opts or {}
    -- Env-var list defaults (scalars don't apply — no compile step for interface libraries).
    local env_defines     = env_list("CPP_DEFINES")
    local env_includes    = env_list("CPP_INCLUDES")
    local env_system_libs = env_list("CPP_SYSTEM_LIBS")

    local includes = {}
    for _, inc in ipairs(env_includes)        do includes[#includes + 1] = inc end
    for _, inc in ipairs(opts.includes or {}) do includes[#includes + 1] = inc end
    local defines = {}
    for _, def in ipairs(env_defines)        do defines[#defines + 1] = def end
    for _, def in ipairs(opts.defines or {}) do defines[#defines + 1] = def end

    register_target(name)

    cook.export(name, {
        includes = includes,
        defines = defines,
    })
end

-- ---------------------------------------------------------------------------
-- compile_commands.json
-- ---------------------------------------------------------------------------

function cpp.compile_commands()
    local targets = cook.cache.get("known_targets") or {}
    local entries = {}
    local wd = cook.sh("pwd"):gsub("%s+$", "")

    for _, name in ipairs(targets) do
        local info = cook.import(name)
        if info and info.compile_info then
            local ci = info.compile_info
            for _, src in ipairs(ci.sources) do
                local compiler = ci.compiler
                if is_c_source(src) then
                    compiler = cpp.state.compiler.cc
                end
                local flags = { "-c" }
                if ci.standard then flags[#flags + 1] = "-std=" .. ci.standard end
                for _, inc in ipairs(ci.includes or {}) do
                    flags[#flags + 1] = "-I" .. inc
                end
                for _, def in ipairs(ci.defines or {}) do
                    flags[#flags + 1] = "-D" .. def
                end
                local cmd = compiler .. " " .. table.concat(flags, " ")
                    .. " " .. src .. " -o build/obj/" .. name .. "/" .. path.stem(src) .. ".o"
                entries[#entries + 1] = {
                    directory = wd,
                    command = cmd,
                    file = src,
                }
            end
        end
    end

    fs.write("compile_commands.json", json_encode_pretty(entries) .. "\n")
end

return cpp
