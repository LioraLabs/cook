# Cook Module System Design

## Summary

A plugin system that allows language-specific Lua modules (e.g., `cook.cpp`, `cook.rust`) to be loaded into Cook via `use name` statements. Modules are project-local Lua files that can manipulate Cook's DAG directly, providing toolchain abstraction and high-level build primitives while preserving Cook's core `cook ... using` identity.

## Goals

- Replace CMake for C++ projects — full toolchain abstraction, cross-platform support, dependency discovery
- Modules are first-class DAG participants, not string-generating helpers
- Both declarative (`cpp.static_library`) and explicit (`cook ... using cpp.compile`) styles are equally valid
- Core provides the minimal runtime API; all language complexity lives in modules
- No package manager — modules are project-local `cook_modules/*.lua` files

## Non-Goals (for now)

- Module registry or package management
- Global module installation
- Module versioning or dependency resolution between modules
- Built-in/bundled modules shipping with the Cook binary
- Module-to-module dependencies (a module cannot `use` another module; if needed later, this would be a separate design)

## Architecture

```
Cookfile (use cpp)
    ↓
Parser → UseStatement AST node
    ↓
Codegen → cook.load_module("cpp") at top of generated Lua
    ↓
Runtime → dofile("cook_modules/cpp.lua"), call init(), bind to `cpp`
    ↓
Recipe body → cpp.static_library(...) calls cook.add_unit(), cook.step_group(), cook.export()
    ↓
CaptureState → same CapturedUnit pipeline as today → DAG → Scheduler
```

Modules integrate at the Lua API level. The scheduler, DAG builder, worker pool, and cache system are completely untouched.

## Module Contract

A module is a Lua file that returns a table. No base class, no registration ceremony.

**File location:** `cook_modules/<name>.lua` or `cook_modules/<name>/init.lua`

**Structure:**

```lua
local cpp = {}

-- Table setup (runs at load time — fast, no I/O)
cpp.state = {}

function cpp.init()
    -- Expensive work (runs once per recipe VM, cached on disk)
    local cached = cook.cache.get("compiler")
    if cached and fs.exists(cached.cxx) then
        cpp.state.compiler = cached
    else
        cpp.state.compiler = cpp.detect_compiler()
        cook.cache.set("compiler", cpp.state.compiler)
    end
end

function cpp.toolchain(opts)
    -- Called by user in Cookfile (after init, before recipe body)
    cpp.state.standard = opts.standard or "c++17"
end

-- Capture-mode functions (called from recipe bodies)
function cpp.static_library(name, opts) ... end
function cpp.executable(name, opts) ... end
function cpp.find(name) ... end

return cpp
```

**Two execution contexts:**

| Context | When | Available APIs |
|---|---|---|
| Module load + init | `use cpp` processed | `cook.env`, `cook.cache`, `cook.sh`, `cook.platform`, `fs.*`, `path.*` |
| Recipe body | Inside recipe function (capture mode) | All of the above + `cook.add_unit`, `cook.step_group`, `cook.export`, `cook.import` |

## Module Cache

Modules get a persistent key-value cache scoped to the module name. Stored at `.cook/cache/<module_name>.json`.

```lua
cook.cache.get(key)            -- returns value or nil
cook.cache.set(key, value)     -- persist (must be serializable)
cook.cache.invalidate(key)     -- remove a key
cook.cache.clear()             -- wipe this module's cache
```

**Lifecycle:**
- Cache is loaded lazily on first `cook.cache.get` or `cook.cache.set` call
- Cache is saved (flushed to disk) when the module's scope ends (after `cook.load_module` returns, or at end of recipe capture)
- Module name scoping is enforced by `cook.load_module`: it sets an internal "current module" name before calling `dofile`, and `cook.cache.*` reads that name to resolve the file path

**Invalidation:**
- User's clean recipe can include `rm -rf .cook/cache/` to clear all module caches
- Module source hash change auto-invalidates: `cook.load_module` computes xxh3 of the module file, compares against a `_source_hash` key in the cache file. If different, the entire cache is wiped and the new hash is stored.
- Modules self-validate (e.g., check `fs.exists(cached.cxx)` before trusting cached compiler path)

## Core Runtime API

### Always available

```lua
cook.env                          -- table of resolved env vars (read/write)
cook.platform.os                  -- "linux" | "macos" | "windows"
cook.platform.arch                -- "x86_64" | "aarch64" | ...
cook.sh(cmd)                      -- execute shell command, return stdout
cook.cache.get/set/invalidate/clear  -- module-scoped persistent cache

fs.exists(path)                   -- existing
fs.size(path)                     -- existing
fs.read(path)                     -- existing
fs.glob(pattern)                  -- existing
fs.mtime(path)                    -- existing

path.stem/name/ext/dir/replace_ext/join  -- existing
```

### Capture mode only (inside recipe bodies)

```lua
cook.add_unit({
    inputs   = { "src/main.cpp" },
    output   = "build/obj/main.o",
    command  = "g++ -c src/main.cpp -o build/obj/main.o",
    cache    = true,
})

cook.step_group(function()
    -- all add_unit() calls in here can run in parallel
end)

cook.export("mylib", {
    includes = { "include/" },
    lib_path = "build/libmylib.a",
})

local info = cook.import("mylib")
-- info.includes, info.lib_path
```

**`cook.add_unit(table)`** is the low-level DAG primitive. The Rust implementation maps the Lua table to existing contracts:

```
Lua table field        → Rust type
─────────────────────────────────────────────────────
inputs                 → CacheMeta.input_paths (Vec<String>)
output                 → CacheMeta.output_path (Option<String>)
                         Also used for WorkPayload.Shell.cmd output
command                → WorkPayload::Shell { cmd, line: 0 }
                         Also hashed → CacheMeta.command_hash (via hash_str)
cache (bool, def true) → Some(CacheMeta) if true, None if false
```

`CacheMeta.recipe_name` and `cache_key` are filled in by the runtime (recipe name from current context, cache key derived from output path + command hash).

`DepKind` is determined by context: if called inside a `cook.step_group()`, the unit gets `DepKind::StepGroup(current_group)`. Otherwise it gets `DepKind::Sequential`.

`cook.add_unit` and the existing `cook.exec`/`cook.layer` both produce `CapturedUnit` and push to the same `CaptureState.units` vec. They coexist — `add_unit` is the clean programmatic API for modules, `exec`/`layer` are the codegen targets for Cookfile DSL syntax. Neither replaces the other.

**`cook.step_group(fn)`** wraps `begin_step()`/`end_step()` in a function call. Internally: opens a new step group, calls `fn`, closes the group. Both `cook.add_unit` and existing `cook.layer` calls inside the function get `DepKind::StepGroup(current_group)`. Cleaner API for modules; existing begin/end markers still work for hand-written Lua.

**`cook.export(name, table)` / `cook.import(name)`** is the cross-recipe communication channel. Stored Rust-side on `Runtime` (`BTreeMap<String, serde_json::Value>`), survives across per-recipe VMs (each VM gets `cook.import` re-registered with a reference to the same Runtime state).

Exports are **declarative metadata** — paths, flags, configuration. Not file contents or data that depends on build outputs existing. When recipe "mylib" calls `cook.export("mylib", { lib_path = "build/libmylib.a" })`, it declares where the library *will be* after execution. Recipe "app" uses `cook.import("mylib")` to read that metadata during its capture phase, before "mylib" has actually been built. This works because capture mode records intent, not results.

## Parser Changes

**New AST node:**

```rust
pub struct UseStatement {
    pub module_name: String,
    pub line: usize,
}
```

**Added to Cookfile AST** (matching existing types — `vars` is `Vec<(String, String)>`, `configs` is `HashMap`):

```rust
pub struct Cookfile {
    pub vars: Vec<(String, String)>,
    pub configs: std::collections::HashMap<String, Vec<(String, String)>>,
    pub recipes: Vec<Recipe>,
    pub uses: Vec<UseStatement>,    // NEW
}
```

**Syntax:** `use name` at top level, before recipes. Parsed as keyword + bare identifier (quoted form `use "name"` also accepted).

**Lexer change:** Add `"use"` to the blocked keywords list in `try_parse_var_decl` (lexer.rs line 53) to prevent `use cpp` from being parsed as a `VarDecl`. Then add a new `Token::UseDecl { name: String }` variant, parsed similarly to `ConfigHeader` — keyword followed by a bare identifier or quoted string.

**Error handling:** If `cook.load_module` cannot find the module file, it raises a Lua error that propagates as `mlua::Error`, terminating recipe registration with a clear message: `"module 'cpp' not found: expected cook_modules/cpp.lua or cook_modules/cpp/init.lua"`.

## Codegen Changes

Each `use` statement emits a `cook.load_module()` call at the top of generated Lua:

```lua
local cpp = cook.load_module("cpp")
local proto = cook.load_module("proto")

-- rest of generated Lua (recipe definitions)
```

**`cook.load_module(name)`** is a Rust-registered function that:
1. Resolves path: `working_dir/cook_modules/name.lua` (or `name/init.lua`)
2. Checks file exists (error if not)
3. Hashes module source, invalidates cache if changed
4. Calls `dofile()` to load the module
5. Calls `init()` if present on returned table
6. Returns the module table

## Cookfile Example

```
use cpp

config debug
    CFLAGS "-g -O0"
end

config release
    CFLAGS "-O2 -DNDEBUG"
end

recipe mylib
    >{
        cpp.toolchain { standard = "c++17", warnings = "strict" }
        cpp.static_library("mylib", {
            sources = { "src/math.cpp", "src/utils.cpp" },
            includes = { "include/", "vendor/json/" },
            export_includes = { "include/" },
        })
    }
end

recipe app: mylib
    cpp.executable("app", {
        sources = { "src/main.cpp" },
        links = { "mylib" },
        system_libs = { "m", "pthread" },
    })
end

recipe test: mylib
    >{
        cpp.find("gtest")
        cpp.test("test_math", {
            sources = { "tests/test_math.cpp" },
            links = { "mylib", "gtest" },
        })
    }
end

recipe clean
    rm -rf build bin
end
```

Module functions and plain Cook steps coexist freely. `recipe clean` is pure shell; `recipe app` uses the module. Both are valid.

## What Core Implements vs What Modules Implement

### Core (Rust)

| New API | Implementation |
|---|---|
| `cook.load_module(name)` | Path resolution, dofile, init, return table |
| `cook.add_unit(table)` | Parse Lua table → CapturedUnit, push to CaptureState |
| `cook.step_group(fn)` | begin_step, call fn, end_step |
| `cook.export(name, table)` | Serialize Lua table, store on Runtime |
| `cook.import(name)` | Lookup from Runtime, deserialize to Lua table |
| `cook.cache.*` | Read/write `.cook/cache/<module>.json`, scoped by module |
| `cook.platform` | Static table with os and arch |
| Parser: `use` statement | New AST node, codegen rule |

### Modules (Lua, no core changes)

- Compiler/toolchain detection and caching
- Platform-specific flag logic
- `cpp.compile()`, `cpp.link()`, `cpp.archive()` — command builders
- `cpp.static_library()`, `cpp.executable()` — declarative helpers
- `cpp.find()` — pkg-config / CMake config file discovery
- Header dependency scanning (gcc `-MD` flag + `.d` file parsing)
- `compile_commands.json` generation for IDE support
- Cross-compilation (target-specific toolchain selection)

## Design Notes

**Per-recipe VM model is preserved.** Each `register_recipe()` call creates a fresh Lua VM. Modules are loaded into each VM via the generated `cook.load_module()` calls. This means module `init()` runs per-recipe, but expensive work is cached on disk via `cook.cache`. The overhead of loading a small Lua file and creating a table per-recipe is negligible.

**`cook.sh()` in module `init()` executes immediately.** This is intentional — module init needs to run real shell commands for compiler detection, pkg-config probes, etc. Same behavior as top-level `cook.sh()` in recipe bodies (outside layers).

**`ingredients` and module sources are independent concerns.** When using modules, `ingredients` in the Cookfile DSL is optional. Modules handle their own input tracking via `cook.add_unit({ inputs = ... })`. The `inputs` field drives cache invalidation for module-created units, same as `ingredients` does for DSL-created units. If both are present, they operate independently — `ingredients` feeds existing DSL cache logic, module inputs feed module-created units.

**`cook.platform` values use Rust's `std::env::consts`:** `cook.platform.os` returns `std::env::consts::OS` ("linux", "macos", "windows") and `cook.platform.arch` returns `std::env::consts::ARCH` ("x86_64", "aarch64"). Modules can map these to toolchain-specific names as needed (e.g., mapping "aarch64" to "arm64" for Apple toolchains).

## What Doesn't Change in Core

- Scheduler, DAG builder, worker pool — untouched
- Cache system — `cook.add_unit` produces the same `CapturedUnit` with `CacheMeta`
- Existing `cook ... using`, `ingredients`, `plate` syntax — all still works
- Codegen (beyond adding `cook.load_module` calls)
- Execute-mode worker VMs (one per thread, OTP-style)
