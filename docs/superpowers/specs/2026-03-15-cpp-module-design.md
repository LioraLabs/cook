# Cook C++ Module Design

## Summary

A project-local Lua module (`cook_modules/cpp.lua`) that provides full C/C++ build support: compiler detection, compilation with header dependency tracking, static/shared/interface libraries, executables, C++20 module support, transitive link resolution, and `compile_commands.json` generation. Targets GCC and Clang on Linux/macOS.

## Prerequisites — Core API Additions

The following must be added to the core runtime before the module can be implemented:

- **`fs.write(path, content)`** — write a string to a file (needed for `compile_commands.json`)
- **`fs.mkdir_p(path)`** — create directories recursively (needed for output dirs like `build/obj/`, `build/lib/`, `.cook/deps/`)

These are small additions to `src/runtime/fs_api.rs`.

## Goals

- Replace CMake for C/C++ projects using Cook
- Automatic header dependency tracking via compiler-generated `.d` files — changing one header only rebuilds source files that include it
- C++20 module support (explicit declaration)
- Zero configuration for simple projects (auto-detect compiler, sensible defaults)
- `compile_commands.json` for IDE/LSP integration

## Non-Goals (for now)

- MSVC / Windows support
- Cross-compilation / toolchain files
- Install / packaging (`make install` equivalent)
- Precompiled headers
- Automatic C++20 module dependency scanning (P1689)
- Package management beyond pkg-config
- `packages` field for `cpp.find()` integration (manual flag passing for now)

## Cookfile Example

```
use cpp

config debug
    CFLAGS "-g -O0"
    CXXFLAGS "-g -O0"
end

config release
    CFLAGS "-O2 -DNDEBUG"
    CXXFLAGS "-O2 -DNDEBUG"
end

recipe math
    cpp.static_library("math", {
        dir = "src/math/",
        includes = { "include/" },
        export_includes = { "include/math/" },
        standard = "c++17",
    })
end

recipe engine: math
    cpp.static_library("engine", {
        dir = "src/engine/",
        includes = { "include/", "vendor/stb/" },
        export_includes = { "include/engine/" },
        links = { "math" },
        standard = "c++20",
        modules = { "src/engine/renderer.cppm" },
    })
end

recipe game: engine
    cpp.executable("game", {
        dir = "src/game/",
        links = { "engine" },
        system_libs = { "pthread", "dl" },
        standard = "c++20",
    })
end

recipe tests: engine math
    >{
        local gtest = cpp.find("gtest")
        cpp.executable("tests", {
            dir = "tests/",
            links = { "engine", "math" },
            extra_cflags = gtest.cflags,
            extra_ldflags = gtest.libs,
            standard = "c++20",
        })
    }
end

recipe compile-commands
    cpp.compile_commands()
end

recipe clean
    rm -rf build .cook
end
```

## Module Structure

**File:** `cook_modules/cpp.lua` — single file, returns a table.

**Internal state:**
```lua
cpp.state = {
    compiler = nil,           -- { cxx = "g++", cc = "gcc" }
    default_standard = nil,   -- e.g., "c++17"
    warnings = "default",     -- "default" | "strict" | "none" | raw string
    targets = {},             -- list of target names registered this session
}
```

## Compiler Detection

On `init()`, auto-detect the compiler and cache the result.

**Detection order:** clang++, g++, c++ — tries running `<name> --version` via `cook.sh()` wrapped in `pcall`, takes the first that succeeds.

**Compiler table:**
```lua
{ cxx = "clang++", cc = "clang" }
-- or
{ cxx = "g++", cc = "gcc" }
```

**Caching:** Stored via `cook.cache.set("compiler", ...)`. Re-validated on load by trying `cook.sh("command -v " .. cached.cxx)` wrapped in `pcall`. Auto-invalidated if the module source changes (module system handles this).

**Error handling:** If no compiler is found after trying all candidates, `error("cpp: no C/C++ compiler found (tried clang++, g++, c++)")`.

**C vs C++ selection:** Based on file extension. `.c` files use `cc`, `.cpp`/`.cc`/`.cxx`/`.cppm` files use `cxx`.

## Toolchain Override

```lua
cpp.toolchain({
    compiler = "g++",       -- override auto-detected compiler
    standard = "c++20",     -- default standard for all targets
    warnings = "strict",    -- warning preset
})
```

Optional — auto-detection works for the common case. Called inside a recipe body; settings apply to that recipe's VM only (each recipe gets a fresh module load). For global defaults, call `cpp.toolchain()` in each recipe, or set defaults via Cook config vars.

## Warning Presets

| Preset | Flags |
|---|---|
| `"default"` | `-Wall` |
| `"strict"` | `-Wall -Wextra -Wpedantic` |
| `"none"` | (no warning flags) |
| raw string | Passed verbatim, e.g. `"-Wall -Wno-unused"` |

## User Flag Integration

The module builds its own flags, then appends user flags from Cook config vars at the end so the user always wins:

- `cook.env.CFLAGS` — appended to C compilations (`.c` files only)
- `cook.env.CXXFLAGS` — appended to C++ compilations (`.cpp`/`.cc`/`.cxx`/`.cppm` files)
- `cook.env.LDFLAGS` — appended to link commands

If a config var is not set, it is skipped (no nil error). `CFLAGS` never applies to C++ files and `CXXFLAGS` never applies to C files.

## Path Handling

**`fs.glob()` returns absolute paths.** The module strips the working directory prefix to get relative paths for commands and cache inputs. Helper:

```lua
local function to_relative(abs_path)
    local wd = cook.sh("pwd"):gsub("\n", "") .. "/"
    if abs_path:sub(1, #wd) == wd then
        return abs_path:sub(#wd + 1)
    end
    return abs_path
end
```

**Directory creation:** Before compiling, the module calls `fs.mkdir_p()` for output directories (`build/obj/<target>/`, `build/lib/`, `build/bin/`, `build/bmi/<target>/`, `.cook/deps/<target>/`).

## Header Dependency Tracking

### First build (no `.d` files exist)

1. `cpp.compile()` passes `-MMD -MF .cook/deps/<target>/<stem>.d` to the compiler
2. Compiler writes a depfile listing all transitively included headers
3. `cook.add_unit()` is called with `inputs = { source }` only
4. Everything compiles (no cache)

### Second build onwards

1. Before calling `cook.add_unit()`, the module checks for an existing `.d` file
2. Parses it to extract header paths, filtering out system headers (absolute paths) and the source file itself (already in inputs)
3. Calls `cook.add_unit()` with `inputs = { source, header1, header2, ... }`
4. Cook's cache system compares mtimes/hashes of ALL inputs including headers
5. Only source files whose headers changed get recompiled

### Depfile format

Make-compatible:
```
build/obj/math/main.o: src/main.cpp include/math.h include/utils.h
```

### Depfile parsing

```lua
local function parse_depfile(dep_path, source_path)
    if not fs.exists(dep_path) then return {} end
    local content = fs.read(dep_path)
    local deps_str = content:gsub("^.-:%s*", ""):gsub("\\\n", " ")
    local deps = {}
    for dep in deps_str:gmatch("%S+") do
        -- Skip system headers (absolute paths) and the source itself
        if not dep:match("^/") and dep ~= source_path then
            -- Verify header still exists (stale depfile handling)
            if fs.exists(dep) then
                deps[#deps + 1] = dep
            end
        end
    end
    return deps
end
```

If any header in the depfile no longer exists, that entry is skipped. The source still compiles (the `#include` was presumably removed), and the next compile produces a fresh `.d` file.

### Depfile location

`.cook/deps/<target_name>/<stem>.d`

## Core Build Functions

### `cpp.compile(source, opts)`

Compiles one source file to an object. Returns the output path string immediately — the actual compilation is deferred to execution time (capture mode records intent).

```lua
-- opts: { output, includes, defines, standard, flags, target_name, fpic, extra_cflags }
-- Returns the output path
```

1. Pick compiler based on extension (`.c` → cc, else → cxx)
2. Compute output path: `build/obj/<target_name>/<stem>.o`
3. Build flags: `-c`, `-MMD -MF <depfile>`, `-std=`, `-I`, `-D`, warnings, `-fPIC` if opts.fpic, platform flags
4. Append `extra_cflags` if provided
5. Append `cook.env.CXXFLAGS` or `cook.env.CFLAGS` (based on file extension)
6. Ensure output directory exists via `fs.mkdir_p()`
7. Parse existing depfile for header inputs
8. Call `cook.add_unit({ inputs, output, command })`

**Error handling:** If `source` does not exist, `error("cpp.compile: source file not found: " .. source)`.

### `cpp.archive(objects, output)`

Creates a static library via `ar rcs`.

```lua
cook.add_unit({ inputs = objects, output = output, command = "ar rcs " .. output .. " " .. table.concat(objects, " ") })
```

### `cpp.link(objects, output, opts)`

Links an executable or shared library.

```lua
-- opts: { libs, system_libs, shared, extra_ldflags }
```

- `libs` — list of `.a`/`.so` file paths to link
- `system_libs` — list of names for `-l` flags
- `shared` — if true, adds `-shared` (Linux) or `-dynamiclib` (macOS)
- `extra_ldflags` — extra flags appended before `cook.env.LDFLAGS`
- On Linux, when linking against shared libraries, adds `-Wl,-rpath,<lib_dir>` for each shared lib directory

Appends `cook.env.LDFLAGS` last.

### `cpp.find(name)`

Discovers a system library via pkg-config. Returns `{ cflags, libs }`. Does **not** export — the caller passes the returned values manually via `extra_cflags`/`extra_ldflags`.

```lua
function cpp.find(name)
    local cached = cook.cache.get("pkg:" .. name)
    if cached then return cached end

    local ok, cflags = pcall(function() return cook.sh("pkg-config --cflags " .. name):gsub("\n", "") end)
    if not ok then error("cpp.find: package '" .. name .. "' not found via pkg-config") end
    local libs = cook.sh("pkg-config --libs " .. name):gsub("\n", "")
    local result = { cflags = cflags, libs = libs }
    cook.cache.set("pkg:" .. name, result)
    return result
end
```

## Declarative Helpers

### `cpp.static_library(name, opts)`

```lua
-- opts: {
--   sources,           -- explicit file list
--   dir,               -- OR auto-discover .c/.cpp/.cc/.cxx recursively
--   includes,          -- include paths for this target
--   export_includes,   -- include paths exported to consumers
--   export_defines,    -- defines exported to consumers
--   defines,           -- defines for this target
--   standard,          -- C++ standard (default: "c++17")
--   modules,           -- C++20 module interface files (.cppm)
--   links,             -- other Cook targets to link (for include resolution)
-- }
```

1. Resolve sources from `dir` (glob `**/*.cpp`, `**/*.c`, `**/*.cc`, `**/*.cxx`) or `sources`
2. Resolve includes from linked targets (transitive)
3. If `modules` present: compile module interfaces as step group 1, then sources as step group 2
4. Otherwise: compile all sources in one step group (parallel)
5. Archive objects → `build/lib/lib<name>.a`
6. Register target: `cpp.state.targets[#cpp.state.targets+1] = name`
7. Export metadata:

```lua
cook.export(name, {
    includes = opts.export_includes or {},
    defines = opts.export_defines or {},
    lib_path = output,
    links = opts.links or {},
    compile_info = {
        sources = sources,
        includes = all_includes,
        defines = all_defines,
        standard = standard,
        compiler = compiler_used,
    },
})
```

**Error handling:** If no sources found (empty `sources` list and `dir` glob returns nothing), `error("cpp.static_library: no sources found for target '" .. name .. "'")`.

### `cpp.shared_library(name, opts)`

Same API as `static_library`. Differences:
- Objects compiled with `-fPIC`
- Linked with `-shared` (Linux) or `-dynamiclib` (macOS) instead of archived
- Output: `build/lib/lib<name>.so` (Linux), `build/lib/lib<name>.dylib` (macOS)
- Export includes `shared = true` so consumers can add rpath

### `cpp.executable(name, opts)`

```lua
-- opts: {
--   sources, dir, includes, defines, standard, modules,
--   links,             -- Cook targets to link (transitive resolution)
--   system_libs,       -- system libraries (-l flags)
--   extra_cflags,      -- extra compile flags (e.g., from cpp.find())
--   extra_ldflags,     -- extra link flags (e.g., from cpp.find())
-- }
```

1. Resolve transitive link dependencies (see below)
2. Merge includes from linked targets
3. Compile sources (with module step groups if needed)
4. Link → `build/bin/<name>`
5. Register target and export compile_info (no lib_path since it's an executable)

### `cpp.interface_library(name, opts)`

No compilation. Just exports metadata:
```lua
-- opts: { includes, defines }
function cpp.interface_library(name, opts)
    cpp.state.targets[#cpp.state.targets+1] = name
    cook.export(name, {
        includes = opts.includes or {},
        defines = opts.defines or {},
    })
end
```

## Transitive Link Resolution

When resolving `links`, the module performs a depth-first post-order walk with deduplication (first occurrence wins):

```lua
local function resolve_links(link_names)
    local visited = {}
    local includes = {}
    local defines = {}
    local lib_paths = {}  -- ordered: dependents before dependencies

    local function walk(name)
        if visited[name] then return end
        visited[name] = true
        local info = cook.import(name)
        if not info then
            error("cpp: link target '" .. name .. "' not found (was it exported by a dependency recipe?)")
        end
        -- Recurse into transitive deps first
        for _, dep in ipairs(info.links or {}) do
            walk(dep)
        end
        -- Collect this target's exports
        for _, inc in ipairs(info.includes or {}) do includes[#includes+1] = inc end
        for _, def in ipairs(info.defines or {}) do defines[#defines+1] = def end
        if info.lib_path then lib_paths[#lib_paths+1] = info.lib_path end
    end

    for _, name in ipairs(link_names) do walk(name) end
    return includes, defines, lib_paths
end
```

**Link order:** Depth-first post-order means dependencies come after dependents in `lib_paths`. This is correct for static library linking where the linker resolves symbols left-to-right: `libengine.a` must appear before `libmath.a` if engine depends on math.

Wait — post-order puts dependencies *first* (leaf nodes visited last in recursion, but added after the recursive call returns). Let me clarify: the walk recurses into deps first, then adds the current node. So for `engine → math`:
1. `walk("engine")` → recurse into `walk("math")` → add `math` → add `engine`
2. `lib_paths = ["libmath.a", "libengine.a"]`

That's wrong — we need engine before math. Fix: add the current node *before* recursing, or reverse at the end. The simplest fix is to collect in pre-order (add self, then recurse):

```lua
local function walk(name)
    if visited[name] then return end
    visited[name] = true
    local info = cook.import(name)
    if not info then error(...) end
    -- Add self first (dependent before dependencies)
    for _, inc in ipairs(info.includes or {}) do includes[#includes+1] = inc end
    for _, def in ipairs(info.defines or {}) do defines[#defines+1] = def end
    if info.lib_path then lib_paths[#lib_paths+1] = info.lib_path end
    -- Then recurse
    for _, dep in ipairs(info.links or {}) do walk(dep) end
end
```

This produces `lib_paths = ["libengine.a", "libmath.a"]` — correct link order.

## C++20 Module Support

**Explicit declaration:** Users list module interface files in the `modules` field.

**Compilation flow:**
1. Step group 1: Compile `.cppm` files → BMI + object files
2. Step group 2: Compile regular sources with BMI search path → object files
3. Link all objects (both module and regular)

**Compiler flags (Clang):**
- Produce BMI: `clang++ -std=c++20 --precompile src/renderer.cppm -o build/bmi/<target>/renderer.pcm` then `clang++ -c build/bmi/<target>/renderer.pcm -o build/obj/<target>/renderer.o`
- Consumer search: `-fprebuilt-module-path=build/bmi/<target>/`

**GCC support:** Deferred — GCC's module support uses a different mechanism (module mapper, `gcm.cache/`). The module can add GCC-specific branches later using the compiler identity from detection.

**BMI location:** `build/bmi/<target_name>/<stem>.pcm` — namespaced by target to avoid collisions.

**Module name:** Derived from filename stem: `src/renderer.cppm` → `renderer`

**Header tracking:** Module interface units also get `-MMD -MF` flags.

## compile_commands.json

Generated by an explicit recipe:

```
recipe compile-commands
    cpp.compile_commands()
end
```

`cpp.compile_commands()` with no arguments:
1. Iterates `cpp.state.targets` — the internal registry of target names built up by declarative helpers
2. For each target, calls `cook.import(name)` and reads `compile_info`
3. Reconstructs compile commands for each source file from the stored metadata
4. Writes `compile_commands.json` to project root via `fs.write()`

**Note:** Since each recipe gets a fresh VM, `cpp.state.targets` is populated within a single recipe only. For `compile_commands()` to see all targets, it relies on `cook.import()` — the targets were exported by prior recipes. The module iterates a well-known list approach: either the user passes target names, or the module stores the list in `cook.cache`:

```lua
-- In each declarative helper, after cook.export:
local known = cook.cache.get("known_targets") or {}
known[#known+1] = name
cook.cache.set("known_targets", known)

-- In compile_commands():
function cpp.compile_commands()
    local targets = cook.cache.get("known_targets") or {}
    local entries = {}
    local wd = cook.sh("pwd"):gsub("\n", "")
    for _, name in ipairs(targets) do
        local info = cook.import(name)
        if info and info.compile_info then
            local ci = info.compile_info
            for _, src in ipairs(ci.sources) do
                entries[#entries+1] = {
                    directory = wd,
                    command = ci.compiler .. " -c -std=" .. ci.standard
                        .. flags_from(ci.includes, ci.defines) .. " " .. src
                        .. " -o build/obj/" .. name .. "/" .. path.stem(src) .. ".o",
                    file = src,
                }
            end
        end
    end
    -- Write JSON
    fs.write("compile_commands.json", json_encode(entries))
end
```

**JSON encoding:** Lua has no built-in JSON encoder. The module includes a minimal `json_encode()` helper (table → JSON string). This is ~30 lines of Lua for the subset we need (arrays of objects with string values).

**Export `compile_info` field:**
```lua
compile_info = {
    sources = { "src/main.cpp", "src/utils.cpp" },
    includes = { "include/", "vendor/" },
    defines = { "DEBUG" },
    standard = "c++17",
    compiler = "g++",
}
```

## Platform Flags

| Flag | Linux | macOS |
|---|---|---|
| Shared lib PIC | `-fPIC` | `-fPIC` |
| Shared lib link | `-shared` | `-dynamiclib` |
| Shared lib extension | `.so` | `.dylib` |
| Shared lib rpath | `-Wl,-rpath,<dir>` | `-Wl,-rpath,<dir>` |
| Standard library | auto (compiler default) | auto |

Platform detected via `cook.platform.os`.

## Default Output Paths

| Target type | Path |
|---|---|
| Object files | `build/obj/<target_name>/<stem>.o` |
| Static libraries | `build/lib/lib<name>.a` |
| Shared libraries | `build/lib/lib<name>.so` / `.dylib` |
| Executables | `build/bin/<name>` |
| BMI files | `build/bmi/<target_name>/<stem>.pcm` |
| Dep files | `.cook/deps/<target_name>/<stem>.d` |

Object files and BMI files are namespaced by target name to avoid collisions when multiple targets have source files with the same stem.

## Test Project

**Location:** `examples/cpp-project/`

A minimal project exercising all module features:

```
examples/cpp-project/
    Cookfile
    cook_modules/
        cpp.lua
    include/
        mathlib/
            vec.h              (vector types, includes util.h)
            util.h             (helper functions)
        app/
            renderer.h         (includes vec.h — tests transitive header deps)
    src/
        math/
            vec.cpp            (implements vec.h)
            util.cpp           (implements util.h)
        app/
            renderer.cppm      (C++20 module interface)
            main.cpp           (imports renderer module, links mathlib)
    tests/
        test_vec.cpp           (tests math library)
```

**Targets:**
- `mathlib` — static library from `src/math/`, exports `include/mathlib/`
- `app` — executable from `src/app/`, links `mathlib`, uses C++20 module
- `tests` — executable from `tests/`, links `mathlib`

**Cookfile:**
```
use cpp

recipe mathlib
    cpp.static_library("mathlib", {
        dir = "src/math/",
        includes = { "include/" },
        export_includes = { "include/mathlib/" },
        standard = "c++17",
    })
end

recipe app: mathlib
    cpp.executable("app", {
        dir = "src/app/",
        links = { "mathlib" },
        standard = "c++20",
        modules = { "src/app/renderer.cppm" },
    })
end

recipe tests: mathlib
    cpp.executable("tests", {
        dir = "tests/",
        links = { "mathlib" },
        standard = "c++17",
    })
end

recipe compile-commands
    cpp.compile_commands()
end

recipe clean
    rm -rf build .cook
end
```

**Validates:**
- Header dep tracking: change `vec.h` → only `vec.cpp` and files including it rebuild
- Transitive links: `app` links `mathlib` through transitive resolution
- C++20 modules: `renderer.cppm` compiled before `main.cpp`
- Static library + executable pipeline
- `compile_commands.json` generation
- Parallel compilation (step groups)

## What Changes in Core

Small additions to `src/runtime/fs_api.rs`:
- `fs.write(path, content)` — write string to file
- `fs.mkdir_p(path)` — create directories recursively

Everything else is pure Lua using existing APIs:
- `cook.add_unit()`, `cook.step_group()` — capture work units
- `cook.export()`, `cook.import()` — cross-recipe metadata
- `cook.cache.*` — persistent compiler detection and target registry
- `cook.sh()` — compiler detection, pkg-config
- `cook.platform` — OS-specific flag selection
- `fs.*`, `path.*` — file operations
