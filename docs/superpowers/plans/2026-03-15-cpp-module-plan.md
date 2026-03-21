# C++ Module Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build `cook_modules/cpp.lua` — a complete C/C++ build module with compiler detection, header dependency tracking, static/shared/interface libraries, executables, C++20 module support, transitive link resolution, and `compile_commands.json` generation.

**Architecture:** Two core changes to Rust (`fs.write`, `fs.mkdir_p`), then everything else is a single Lua file (`cook_modules/cpp.lua`) using the module system APIs. A test project at `examples/cpp-project/` validates the full pipeline with real C++ code.

**Tech Stack:** Lua 5.4 (via mlua), GCC/Clang, the Cook module system APIs (`cook.add_unit`, `cook.step_group`, `cook.export/import`, `cook.cache.*`, `cook.sh`, `cook.platform`, `fs.*`, `path.*`)

**Spec:** `docs/superpowers/specs/2026-03-15-cpp-module-design.md`

---

## File Structure

### New files

| File | Responsibility |
|---|---|
| `examples/cpp-project/cook_modules/cpp.lua` | The C++ module — all build logic |
| `examples/cpp-project/Cookfile` | Example Cookfile using the module |
| `examples/cpp-project/include/mathlib/vec.h` | Vector types header |
| `examples/cpp-project/include/mathlib/util.h` | Utility header |
| `examples/cpp-project/src/math/vec.cpp` | Vector implementation |
| `examples/cpp-project/src/math/util.cpp` | Utility implementation |
| `examples/cpp-project/src/app/main.cpp` | App entry point |
| `examples/cpp-project/tests/test_vec.cpp` | Test executable |

### Modified files

| File | Change |
|---|---|
| `src/runtime/fs_api.rs` | Add `fs.write(path, content)` and `fs.mkdir_p(path)` |

---

## Chunk 1: Core API additions

### Task 1: Add `fs.write` and `fs.mkdir_p` to the runtime

**Files:**
- Modify: `src/runtime/fs_api.rs`

- [ ] **Step 1: Write failing tests**

Add to `src/runtime/fs_api.rs` (create a test module at the bottom):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use mlua::Lua;
    use tempfile::TempDir;

    fn setup(dir: &std::path::Path) -> Lua {
        let lua = Lua::new();
        register_fs_api(&lua, dir).unwrap();
        lua
    }

    #[test]
    fn test_fs_write_creates_file() {
        let dir = TempDir::new().unwrap();
        let lua = setup(dir.path());
        lua.load(r#"fs.write("test.txt", "hello world")"#).exec().unwrap();
        let content = std::fs::read_to_string(dir.path().join("test.txt")).unwrap();
        assert_eq!(content, "hello world");
    }

    #[test]
    fn test_fs_write_overwrites_existing() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("test.txt"), "old").unwrap();
        let lua = setup(dir.path());
        lua.load(r#"fs.write("test.txt", "new")"#).exec().unwrap();
        let content = std::fs::read_to_string(dir.path().join("test.txt")).unwrap();
        assert_eq!(content, "new");
    }

    #[test]
    fn test_fs_mkdir_p_creates_nested_dirs() {
        let dir = TempDir::new().unwrap();
        let lua = setup(dir.path());
        lua.load(r#"fs.mkdir_p("a/b/c")"#).exec().unwrap();
        assert!(dir.path().join("a/b/c").is_dir());
    }

    #[test]
    fn test_fs_mkdir_p_existing_is_ok() {
        let dir = TempDir::new().unwrap();
        std::fs::create_dir_all(dir.path().join("existing")).unwrap();
        let lua = setup(dir.path());
        lua.load(r#"fs.mkdir_p("existing")"#).exec().unwrap();
        assert!(dir.path().join("existing").is_dir());
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --lib -p cook -- runtime::fs_api::tests`
Expected: FAIL — `fs.write` and `fs.mkdir_p` not registered

- [ ] **Step 3: Implement `fs.write` and `fs.mkdir_p`**

In `src/runtime/fs_api.rs`, add these two function registrations before the `lua.globals().set("fs", fs)?;` line:

```rust
    let wd = working_dir.to_path_buf();
    fs.set(
        "write",
        lua.create_function(move |_, (path, content): (String, String)| {
            let full = wd.join(&path);
            if let Some(parent) = full.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| mlua::Error::runtime(format!("fs.write: {e}")))?;
            }
            std::fs::write(&full, content)
                .map_err(|e| mlua::Error::runtime(format!("fs.write: {e}")))?;
            Ok(())
        })?,
    )?;

    let wd = working_dir.to_path_buf();
    fs.set(
        "mkdir_p",
        lua.create_function(move |_, path: String| {
            let full = wd.join(&path);
            std::fs::create_dir_all(&full)
                .map_err(|e| mlua::Error::runtime(format!("fs.mkdir_p: {e}")))?;
            Ok(())
        })?,
    )?;
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --lib -p cook -- runtime::fs_api::tests`
Expected: ALL PASS

- [ ] **Step 5: Run full test suite**

Run: `cargo test`
Expected: ALL PASS (existing tests unaffected)

- [ ] **Step 6: Commit**

```bash
git add src/runtime/fs_api.rs
git commit -m "feat(runtime): add fs.write and fs.mkdir_p APIs"
```

---

## Chunk 2: Module scaffolding and compiler detection

### Task 2: Create cpp.lua skeleton with compiler detection

**Files:**
- Create: `examples/cpp-project/cook_modules/cpp.lua`

- [ ] **Step 1: Create the module directory and file**

Create the file `examples/cpp-project/cook_modules/cpp.lua`:

```lua
-- Cook C++ Module
-- Provides compiler detection, compilation with header dep tracking,
-- static/shared/interface libraries, executables, C++20 modules,
-- transitive link resolution, and compile_commands.json generation.

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

--- Convert absolute path to relative (strips working directory prefix)
local function to_relative(abs_path)
    local wd = cook.sh("pwd"):gsub("%s+$", "") .. "/"
    if abs_path:sub(1, #wd) == wd then
        return abs_path:sub(#wd + 1)
    end
    return abs_path
end

--- Get warning flags for the current preset
local function warning_flags()
    local w = cpp.state.warnings
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
    if std then flags[#flags + 1] = "-std=" .. std end

    for _, inc in ipairs(opts.includes or {}) do
        flags[#flags + 1] = "-I" .. inc
    end
    for _, def in ipairs(opts.defines or {}) do
        flags[#flags + 1] = "-D" .. def
    end

    -- Warning flags
    local wflags = warning_flags()
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
        -- Then recurse into transitive deps
        for _, dep in ipairs(info.links or {}) do
            walk(dep)
        end
    end

    for _, name in ipairs(link_names) do
        walk(name)
    end
    return includes, defines, lib_paths, rpath_dirs
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

    local includes = {}
    for _, inc in ipairs(opts.includes or {}) do includes[#includes + 1] = inc end
    local defines = {}
    for _, def in ipairs(opts.defines or {}) do defines[#defines + 1] = def end
    local standard = opts.standard or cpp.state.default_standard or "c++17"

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
                local wf = warning_flags()
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
                    target_name = name,
                })
            end
        end)
    end

    -- Archive
    cpp.archive(objects, output)

    -- Track target
    local known = cook.cache.get("known_targets") or {}
    known[#known + 1] = name
    cook.cache.set("known_targets", known)

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

    local includes = {}
    for _, inc in ipairs(opts.includes or {}) do includes[#includes + 1] = inc end
    local defines = {}
    for _, def in ipairs(opts.defines or {}) do defines[#defines + 1] = def end
    local standard = opts.standard or cpp.state.default_standard or "c++17"

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

    local known = cook.cache.get("known_targets") or {}
    known[#known + 1] = name
    cook.cache.set("known_targets", known)

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

    local includes = {}
    for _, inc in ipairs(opts.includes or {}) do includes[#includes + 1] = inc end
    local defines = {}
    for _, def in ipairs(opts.defines or {}) do defines[#defines + 1] = def end
    local standard = opts.standard or cpp.state.default_standard or "c++17"

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
                local wf = warning_flags()
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

    local known = cook.cache.get("known_targets") or {}
    known[#known + 1] = name
    cook.cache.set("known_targets", known)

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
    local known = cook.cache.get("known_targets") or {}
    known[#known + 1] = name
    cook.cache.set("known_targets", known)

    cook.export(name, {
        includes = opts.includes or {},
        defines = opts.defines or {},
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
```

- [ ] **Step 2: Verify file was created correctly**

Run: `ls -la examples/cpp-project/cook_modules/cpp.lua`
Expected: file exists

- [ ] **Step 3: Commit**

```bash
git add examples/cpp-project/cook_modules/cpp.lua
git commit -m "feat: add cook_modules/cpp.lua — C++ build module"
```

---

## Chunk 3: Test project — C++ source files

### Task 3: Create the test project source files

**Files:**
- Create: `examples/cpp-project/include/mathlib/util.h`
- Create: `examples/cpp-project/include/mathlib/vec.h`
- Create: `examples/cpp-project/src/math/util.cpp`
- Create: `examples/cpp-project/src/math/vec.cpp`
- Create: `examples/cpp-project/src/app/main.cpp`
- Create: `examples/cpp-project/tests/test_vec.cpp`

- [ ] **Step 1: Create `include/mathlib/util.h`**

```cpp
#ifndef MATHLIB_UTIL_H
#define MATHLIB_UTIL_H

namespace mathlib {

inline float clamp(float val, float lo, float hi) {
    if (val < lo) return lo;
    if (val > hi) return hi;
    return val;
}

inline float abs(float val) {
    return val < 0 ? -val : val;
}

}  // namespace mathlib

#endif  // MATHLIB_UTIL_H
```

- [ ] **Step 2: Create `include/mathlib/vec.h`**

```cpp
#ifndef MATHLIB_VEC_H
#define MATHLIB_VEC_H

#include "util.h"

namespace mathlib {

struct Vec2 {
    float x, y;

    Vec2() : x(0), y(0) {}
    Vec2(float x, float y) : x(x), y(y) {}

    Vec2 operator+(const Vec2& other) const;
    Vec2 operator-(const Vec2& other) const;
    Vec2 operator*(float scalar) const;
    float dot(const Vec2& other) const;
    float length() const;
    Vec2 normalized() const;
};

}  // namespace mathlib

#endif  // MATHLIB_VEC_H
```

- [ ] **Step 3: Create `src/math/util.cpp`**

```cpp
#include "mathlib/util.h"

// util.h is header-only, but we include this translation unit
// to test that the build system handles files with no unique symbols.
// This also validates that the include path resolution works.

namespace mathlib {
namespace detail {
    // Placeholder to ensure this TU is not empty
    static const int util_version = 1;
}
}
```

- [ ] **Step 4: Create `src/math/vec.cpp`**

```cpp
#include "mathlib/vec.h"
#include <cmath>

namespace mathlib {

Vec2 Vec2::operator+(const Vec2& other) const {
    return Vec2(x + other.x, y + other.y);
}

Vec2 Vec2::operator-(const Vec2& other) const {
    return Vec2(x - other.x, y - other.y);
}

Vec2 Vec2::operator*(float scalar) const {
    return Vec2(x * scalar, y * scalar);
}

float Vec2::dot(const Vec2& other) const {
    return x * other.x + y * other.y;
}

float Vec2::length() const {
    return std::sqrt(x * x + y * y);
}

Vec2 Vec2::normalized() const {
    float len = length();
    if (len < 1e-6f) return Vec2(0, 0);
    return Vec2(x / len, y / len);
}

}  // namespace mathlib
```

- [ ] **Step 5: Create `src/app/main.cpp`**

```cpp
#include "mathlib/vec.h"
#include <cstdio>

int main() {
    mathlib::Vec2 a(3.0f, 4.0f);
    mathlib::Vec2 b(1.0f, 2.0f);

    auto sum = a + b;
    auto diff = a - b;
    auto scaled = a * 2.0f;
    float d = a.dot(b);
    auto n = a.normalized();

    std::printf("a + b = (%.1f, %.1f)\n", sum.x, sum.y);
    std::printf("a - b = (%.1f, %.1f)\n", diff.x, diff.y);
    std::printf("a * 2 = (%.1f, %.1f)\n", scaled.x, scaled.y);
    std::printf("a . b = %.1f\n", d);
    std::printf("|a| = %.2f\n", a.length());
    std::printf("norm(a) = (%.2f, %.2f)\n", n.x, n.y);

    return 0;
}
```

- [ ] **Step 6: Create `tests/test_vec.cpp`**

```cpp
#include "mathlib/vec.h"
#include "mathlib/util.h"
#include <cstdio>
#include <cstdlib>

#define ASSERT_FLOAT_EQ(a, b) do { \
    if (mathlib::abs((a) - (b)) > 1e-5f) { \
        std::fprintf(stderr, "FAIL: %s:%d: %s = %f, expected %f\n", \
                     __FILE__, __LINE__, #a, (double)(a), (double)(b)); \
        failures++; \
    } \
} while(0)

int main() {
    int failures = 0;

    // Test addition
    {
        mathlib::Vec2 a(1, 2);
        mathlib::Vec2 b(3, 4);
        auto c = a + b;
        ASSERT_FLOAT_EQ(c.x, 4.0f);
        ASSERT_FLOAT_EQ(c.y, 6.0f);
    }

    // Test subtraction
    {
        mathlib::Vec2 a(5, 3);
        mathlib::Vec2 b(2, 1);
        auto c = a - b;
        ASSERT_FLOAT_EQ(c.x, 3.0f);
        ASSERT_FLOAT_EQ(c.y, 2.0f);
    }

    // Test scalar multiply
    {
        mathlib::Vec2 a(2, 3);
        auto c = a * 3.0f;
        ASSERT_FLOAT_EQ(c.x, 6.0f);
        ASSERT_FLOAT_EQ(c.y, 9.0f);
    }

    // Test dot product
    {
        mathlib::Vec2 a(1, 0);
        mathlib::Vec2 b(0, 1);
        ASSERT_FLOAT_EQ(a.dot(b), 0.0f);
        ASSERT_FLOAT_EQ(a.dot(a), 1.0f);
    }

    // Test length
    {
        mathlib::Vec2 a(3, 4);
        ASSERT_FLOAT_EQ(a.length(), 5.0f);
    }

    // Test normalized
    {
        mathlib::Vec2 a(3, 4);
        auto n = a.normalized();
        ASSERT_FLOAT_EQ(n.length(), 1.0f);
        ASSERT_FLOAT_EQ(n.x, 0.6f);
        ASSERT_FLOAT_EQ(n.y, 0.8f);
    }

    // Test clamp utility
    {
        ASSERT_FLOAT_EQ(mathlib::clamp(5.0f, 0.0f, 1.0f), 1.0f);
        ASSERT_FLOAT_EQ(mathlib::clamp(-1.0f, 0.0f, 1.0f), 0.0f);
        ASSERT_FLOAT_EQ(mathlib::clamp(0.5f, 0.0f, 1.0f), 0.5f);
    }

    if (failures == 0) {
        std::printf("All tests passed!\n");
        return 0;
    } else {
        std::printf("%d test(s) FAILED\n", failures);
        return 1;
    }
}
```

- [ ] **Step 7: Commit source files**

```bash
git add examples/cpp-project/include/ examples/cpp-project/src/ examples/cpp-project/tests/
git commit -m "feat: add example C++ project source files"
```

---

### Task 4: Create the Cookfile and verify the full build

**Files:**
- Create: `examples/cpp-project/Cookfile`

- [ ] **Step 1: Create the Cookfile**

```
use "cpp"

recipe "mathlib"
    >{
        cpp.static_library("mathlib", {
            dir = "src/math/",
            includes = { "include/" },
            export_includes = { "include/" },
            standard = "c++17",
        })
    }
end

recipe "app": "mathlib"
    >{
        cpp.executable("app", {
            sources = { "src/app/main.cpp" },
            links = { "mathlib" },
            standard = "c++17",
        })
    }
end

recipe "tests": "mathlib"
    >{
        cpp.executable("run_tests", {
            dir = "tests/",
            links = { "mathlib" },
            standard = "c++17",
        })
    }
    build/bin/run_tests
end

recipe "compile-commands"
    >{
        cpp.compile_commands()
    }
end

recipe "clean"
    rm -rf build .cook
end
```

- [ ] **Step 2: Build the project manually to verify (from the example dir)**

Run from `examples/cpp-project/`:
```bash
cd examples/cpp-project && ../../target/debug/cook mathlib
```
Expected: Compiles `vec.cpp` and `util.cpp`, archives into `build/lib/libmathlib.a`

- [ ] **Step 3: Build and run the app**

```bash
cd examples/cpp-project && ../../target/debug/cook app
```
Expected: Compiles `main.cpp`, links against `libmathlib.a`, produces `build/bin/app`

```bash
cd examples/cpp-project && ./build/bin/app
```
Expected output:
```
a + b = (4.0, 6.0)
a - b = (2.0, 2.0)
a * 2 = (6.0, 8.0)
a . b = 11.0
|a| = 5.00
norm(a) = (0.60, 0.80)
```

- [ ] **Step 4: Build and run tests**

```bash
cd examples/cpp-project && ../../target/debug/cook tests
```
Expected: Compiles `test_vec.cpp`, links, runs tests, prints "All tests passed!"

- [ ] **Step 5: Verify incremental rebuild (header dep tracking)**

```bash
# Touch a header — only files including it should rebuild
touch examples/cpp-project/include/mathlib/vec.h
cd examples/cpp-project && ../../target/debug/cook mathlib
```
Expected: Only `vec.cpp` recompiles (it includes `vec.h`). `util.cpp` is skipped (cached).

- [ ] **Step 6: Verify compile_commands.json**

```bash
cd examples/cpp-project && ../../target/debug/cook compile-commands && cat compile_commands.json
```
Expected: Valid JSON array with entries for each source file.

- [ ] **Step 7: Verify clean**

```bash
cd examples/cpp-project && ../../target/debug/cook clean
```
Expected: `build/` and `.cook/` directories removed.

- [ ] **Step 8: Commit**

```bash
git add examples/cpp-project/Cookfile
git commit -m "feat: add example Cookfile for C++ project"
```

---

### Task 5: Add Rust integration test for the module system + cpp pipeline

**Files:**
- Modify: `tests/integration.rs`

- [ ] **Step 1: Add integration test**

Add to `tests/integration.rs`:

```rust
#[test]
fn test_cpp_module_builds_static_library() {
    let dir = tempfile::TempDir::new().unwrap();

    // Create module
    let modules_dir = dir.path().join("cook_modules");
    std::fs::create_dir_all(&modules_dir).unwrap();

    // Minimal cpp module that just compiles and archives
    std::fs::write(modules_dir.join("cpp.lua"), r#"
        local cpp = {}
        function cpp.init() end
        function cpp.static_library(name, opts)
            local sources = opts.sources or {}
            local objects = {}
            cook.step_group(function()
                for _, src in ipairs(sources) do
                    local stem = path.stem(src)
                    local obj = "build/obj/" .. stem .. ".o"
                    local compiler = "gcc"
                    local cmd = compiler .. " -c"
                    for _, inc in ipairs(opts.includes or {}) do
                        cmd = cmd .. " -I" .. inc
                    end
                    cmd = cmd .. " " .. src .. " -o " .. obj
                    cook.add_unit({ inputs = { src }, output = obj, command = cmd })
                    objects[#objects + 1] = obj
                end
            end)
            local output = "build/lib/lib" .. name .. ".a"
            local ar_cmd = "ar rcs " .. output
            for _, o in ipairs(objects) do ar_cmd = ar_cmd .. " " .. o end
            cook.add_unit({ inputs = objects, output = output, command = ar_cmd })
            cook.export(name, { lib_path = output, includes = opts.includes or {} })
        end
        return cpp
    "#).unwrap();

    // Create source
    std::fs::create_dir_all(dir.path().join("src")).unwrap();
    std::fs::write(dir.path().join("src/foo.c"), "int foo() { return 42; }").unwrap();

    // Create Cookfile content
    let cookfile = r#"use "cpp"

recipe "mylib"
    >{
        cpp.static_library("mylib", {
            sources = { "src/foo.c" },
            includes = { "." },
        })
    }
end
"#;
    std::fs::write(dir.path().join("Cookfile"), cookfile).unwrap();

    // Parse + codegen + register
    let parsed = cook::parser::parse(cookfile).unwrap();
    let lua_source = cook::codegen::generate(&parsed);
    let env_vars = std::collections::HashMap::new();
    let rt = cook::runtime::Runtime::new(dir.path().to_path_buf(), env_vars);
    let result = rt.register_recipe(&lua_source, "mylib").unwrap();

    // Should have: N compile units + 1 archive
    assert!(result.units.len() >= 2, "expected at least 2 units (compile + archive), got {}", result.units.len());

    // First unit should be a compile (gcc -c ...)
    match &result.units[0].payload {
        cook::contracts::WorkPayload::Shell { cmd, .. } => {
            assert!(cmd.contains("gcc -c"), "expected gcc compile, got: {cmd}");
            assert!(cmd.contains("foo"), "expected foo in command, got: {cmd}");
        }
        other => panic!("expected Shell, got: {:?}", other),
    }

    // Last unit should be archive (ar rcs ...)
    let last = &result.units[result.units.len() - 1];
    match &last.payload {
        cook::contracts::WorkPayload::Shell { cmd, .. } => {
            assert!(cmd.contains("ar rcs"), "expected ar archive, got: {cmd}");
        }
        other => panic!("expected Shell, got: {:?}", other),
    }
}
```

- [ ] **Step 2: Run test**

Run: `cargo test --test integration test_cpp_module_builds_static_library`
Expected: PASS

- [ ] **Step 3: Commit**

```bash
git add tests/integration.rs
git commit -m "test: add integration test for cpp module static library pipeline"
```

---

### Task 6: Final verification

- [ ] **Step 1: Run full Rust test suite**

Run: `cargo test`
Expected: ALL tests pass

- [ ] **Step 2: Run the full example project end-to-end**

```bash
cd examples/cpp-project
rm -rf build .cook
../../target/debug/cook app
./build/bin/app
../../target/debug/cook tests
../../target/debug/cook compile-commands
cat compile_commands.json | head -20
```
Expected: Everything builds, app runs, tests pass, compile_commands.json is valid.

- [ ] **Step 3: Verify incremental rebuild with header change**

```bash
cd examples/cpp-project
touch include/mathlib/util.h
../../target/debug/cook mathlib 2>&1
```
Expected: Files including `util.h` recompile; others are skipped.

- [ ] **Step 4: Verify clean build from scratch**

```bash
cd examples/cpp-project
rm -rf build .cook
../../target/debug/cook app
```
Expected: Full build succeeds from clean state.
