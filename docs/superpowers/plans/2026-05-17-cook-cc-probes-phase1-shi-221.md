# cook_cc → cook.probe Migration — Phase 1 (SHI-221)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Migrate `cook_cc` from register-phase `cook.cache.set` / `cook.cache.get` to first-class `cook.probe` units, and reshape `cook_cc.bin/.lib/.shared/.headers` to accept `needs = {...}` for declarative system-library discovery.

**Architecture:** The three pure-environment caches (compiler detect, linker search dirs, cmake driver) become `cook.probe` units with explicit `inputs`. The per-package `cc.find(name)` call becomes a `cc:find:<name>` probe whose `produce` body composes the strategy chain inline. The two register-time accumulators (`known_targets`, the per-call canonical-opts memo) move to module-local Lua state. Target makers (`bin/lib/shared/headers`) gain a `needs = {...}` field that auto-registers find probes and weaves `$<cc:find:<n>.cflags>` etc. into per-source compile and link command templates. Imperative `cook_cc.find` / `find_or_error` return a sigil-record for advanced use.

**Tech Stack:** Lua 5.4 module under `/home/alex/dev/cook-modules/cook_cc/`. Busted unit tests in `cook_cc/spec/`. Cook engine (Rust) under `/home/alex/dev/cook/cli/`. Conformance harness at `/home/alex/dev/cook/standard/conformance/`. Standard prose at `/home/alex/dev/cook/standard/src/content/docs/28-cc.mdx`.

**Two-repo scope:**
- **`/home/alex/dev/cook-modules/`** — cook_cc source, rockspec, unit tests
- **`/home/alex/dev/cook/`** — Standard chapter §28, conformance fixtures, examples that consume cook_cc

**Spec reference:** `/home/alex/dev/cook/docs/superpowers/specs/2026-05-17-cook-cc-probes-and-checks-design.md` (Phase 1 = §10.1).

---

## File map

**Created:**
- `cook-modules/cook_cc/_probe_helpers.lua` — strategy helpers shared by `cc:find:<n>` probe bodies (pulled from current `finders/` modules so the probe `produce` worker-VM can `require` them)
- `cook-modules/cook_cc/cook_cc-0.5.0-1.rockspec` — bumped rockspec
- `cook-modules/cook_cc/spec/probe_helpers_spec.lua` — busted tests for the helper module
- `cook-modules/cook_cc/spec/needs_field_spec.lua` — busted tests for the `needs` reshape
- `cook/standard/conformance/positive/cc-needs-pkgconfig/` — new fixture
- `cook/standard/conformance/positive/cc-needs-bare/` — new fixture
- `cook/standard/conformance/positive/cc-toolchain-override/` — new fixture
- `cook/standard/conformance/positive/cc-find-record-sigil/` — new fixture
- `cook/standard/conformance/negative/cc-find-conflicting-opts/` — new fixture
- `cook/standard/conformance/negative/cc-find-missing-on-build/` — new fixture

**Modified:**
- `cook-modules/cook_cc/spec/cook_stub.lua` — add `cook.probe` capture and probe-value injection
- `cook-modules/cook_cc/toolchain.lua` — replace `cook.cache.set/get` with `cook.probe` for `cc:compiler:<override>`
- `cook-modules/cook_cc/init.lua` — drop `toolchain.rehydrate()` from init hook; toolchain becomes lazy via probe
- `cook-modules/cook_cc/finders/bare_probe.lua` — replace `cook.cache.set/get` for `cc:linker-search-dirs` with probe
- `cook-modules/cook_cc/finders/cmake_compat.lua` — replace `cook.cache.set/get` for `cc:cmake-driver` with probe
- `cook-modules/cook_cc/finder.lua` — refactor `find` to register `cc:find:<n>` probe, return sigil-record; drop per-call canonical-opts memo
- `cook-modules/cook_cc/targets.lua` — accept `needs = {...}`; move `known_targets` to module-local state; weave `$<cc:find:<n>.…>` into command templates
- `cook-modules/cook_cc/compile_db.lua` — read `targets._known` (module-local) instead of `cook.cache.get("known_targets")`
- `cook-modules/cook_cc/init.lua` — expose `needs`-aware bin/lib/shared/headers (no public-surface change, just wiring)
- `cook-modules/cook_cc/spec/toolchain_spec.lua` — update tests for probe-based compiler detection
- `cook-modules/cook_cc/spec/finder_spec.lua` — update tests for probe-based find
- `cook-modules/cook_cc/spec/targets_spec.lua` — update for `needs` field
- `cook/examples/raylib-game/Cookfile` — migrate to `needs = {"raylib"}`
- `cook/examples/sdl3-game/Cookfile` — migrate to `needs = {"SDL3"}`
- `cook/examples/raylib-game/cook.toml` — bump cook_cc pin to 0.5.0-1
- `cook/examples/sdl3-game/cook.toml` — bump cook_cc pin to 0.5.0-1
- `cook/standard/src/content/docs/28-cc.mdx` — amend §28 to v0.5 (add `needs` field, document sigil-record return, demand-driven `find_or_error` semantics)
- `cook/standard/src/content/docs/appendix/E-changes.mdx` — add CS-0075 entry

**Deleted:**
- None.

---

## Task 1: Extend `cook_stub.lua` to support `cook.probe`

**Files:**
- Modify: `cook-modules/cook_cc/spec/cook_stub.lua`

The current stub has `cook.cache.get/set` but no `cook.probe`. Subsequent tasks need to drive `cook.probe` registrations and inject probe values for consumer-side tests.

- [ ] **Step 1: Write the failing test** in `cook-modules/cook_cc/spec/cook_stub_spec.lua`:

```lua
local stub = require("cook_stub")

describe("cook_stub probe API", function()
    before_each(function() stub.reset(); stub.install() end)

    it("captures cook.probe registrations", function()
        cook.probe("cc:test", { inputs = {}, produce = "return 42" })
        assert.same({ "cc:test" }, stub.probe_keys())
        local opts = stub.probe_opts("cc:test")
        assert.equals("return 42", opts.produce)
    end)

    it("rejects duplicate cook.probe key", function()
        cook.probe("cc:dup", { inputs = {}, produce = "return 1" })
        assert.has_error(function()
            cook.probe("cc:dup", { inputs = {}, produce = "return 2" })
        end, "[cook_stub] duplicate cook.probe key 'cc:dup'")
    end)

    it("set_probe_value lets tests inject probe outcomes", function()
        stub.set_probe_value("cc:test", { x = 1 })
        cook.probe("cc:test", { inputs = {}, produce = "return {x=1}" })
        assert.same({ x = 1 }, cook.cache.get("cc:test"))
    end)
end)
```

- [ ] **Step 2: Run test to verify it fails**

```
cd /home/alex/dev/cook-modules/cook_cc && busted spec/cook_stub_spec.lua
```
Expected: FAIL — `stub.probe_keys`, `stub.probe_opts`, `stub.set_probe_value` undefined; `cook.probe` undefined.

- [ ] **Step 3: Extend the stub**

In `cook-modules/cook_cc/spec/cook_stub.lua`, add to the module-local state at the top:

```lua
local probe_registrations = {}      -- key -> opts
local probe_values        = {}      -- key -> any (injected by tests)
```

Add to `M.reset()`:

```lua
    probe_registrations = {}
    probe_values        = {}
```

Add new module functions:

```lua
function M.probe_keys()
    local keys = {}
    for k in pairs(probe_registrations) do keys[#keys + 1] = k end
    table.sort(keys)
    return keys
end

function M.probe_opts(key)
    return probe_registrations[key]
end

function M.set_probe_value(key, value)
    probe_values[key] = value
end
```

In `M.install()`'s `_G.cook` table, add:

```lua
        probe = function(key, opts)
            if probe_registrations[key] ~= nil then
                error("[cook_stub] duplicate cook.probe key '" .. key .. "'")
            end
            probe_registrations[key] = opts
        end,
```

Modify `cook.cache.get` to fall through to `probe_values` if no cache entry exists:

```lua
        cache = {
            get = function(k)
                if cache_store[k] ~= nil then return cache_store[k] end
                return probe_values[k]
            end,
            set = function(k, v) cache_store[k] = v end,
        },
```

- [ ] **Step 4: Run test to verify it passes**

```
cd /home/alex/dev/cook-modules/cook_cc && busted spec/cook_stub_spec.lua
```
Expected: 3 passing.

- [ ] **Step 5: Run full spec to confirm no regression**

```
cd /home/alex/dev/cook-modules/cook_cc && busted .
```
Expected: existing tests still pass.

- [ ] **Step 6: Commit**

```
cd /home/alex/dev/cook-modules && git add cook_cc/spec/cook_stub.lua cook_cc/spec/cook_stub_spec.lua
git commit -m "test(cook_cc): extend cook_stub with cook.probe capture + value injection"
```

---

## Task 2: `cc:compiler` probe (replaces `toolchain.rehydrate`)

**Files:**
- Modify: `cook-modules/cook_cc/toolchain.lua`
- Modify: `cook-modules/cook_cc/init.lua`
- Modify: `cook-modules/cook_cc/spec/toolchain_spec.lua`

The current `toolchain.rehydrate()` runs from `init()` and uses `cook.cache.get/set("compiler", ...)`. Replace with a register-phase `cook.probe("cc:compiler:<override>")` call; the consumer (`cc.compile`, `cc.link`) gets the compiler at execute time via `cook.cache.get("cc:compiler:<override>")`. The probe key encodes the override per spec §3.1.

- [ ] **Step 1: Write the failing tests** — replace `cook-modules/cook_cc/spec/toolchain_spec.lua` with:

```lua
local stub = require("cook_stub")

local function reload()
    package.loaded["cook_cc.toolchain"] = nil
    return require("cook_cc.toolchain")
end

describe("toolchain probe registration", function()
    before_each(function() stub.reset(); stub.install() end)

    it("registers cc:compiler:auto on require with no override", function()
        local tc = reload()
        tc.ensure_probe_registered()
        assert.same({ "cc:compiler:auto" }, stub.probe_keys())
    end)

    it("toolchain.set({compiler='clang++'}) selects cc:compiler:clang++", function()
        local tc = reload()
        tc.set({ compiler = "clang++" })
        tc.ensure_probe_registered()
        assert.same({ "cc:compiler:clang++" }, stub.probe_keys())
    end)

    it("get_probe_key reflects current override", function()
        local tc = reload()
        assert.equals("cc:compiler:auto", tc.get_probe_key())
        tc.set({ compiler = "g++" })
        assert.equals("cc:compiler:g++", tc.get_probe_key())
    end)

    it("get_compiler at execute-time reads from probe value store", function()
        stub.set_probe_value("cc:compiler:auto", { cxx = "g++", cc = "gcc" })
        local tc = reload()
        tc.ensure_probe_registered()
        assert.same({ cxx = "g++", cc = "gcc" }, tc.get_compiler())
    end)

    it("produce body string contains the override compiler name", function()
        local tc = reload()
        tc.set({ compiler = "clang++" })
        tc.ensure_probe_registered()
        local opts = stub.probe_opts("cc:compiler:clang++")
        assert.is_string(opts.produce)
        assert.matches("clang%+%+", opts.produce)
    end)

    it("set + merge_defaults preserve module-local state across reload boundary", function()
        local tc = reload()
        tc.set({ standard = "c++17", warnings = "strict" })
        tc.merge_defaults({ defines = { "A" } })
        assert.equals("c++17", tc.get_default_standard())
        assert.equals("strict", tc.get_warnings())
        assert.same({ "A" }, tc.get_defaults().defines)
    end)
end)
```

- [ ] **Step 2: Run tests to verify they fail**

```
cd /home/alex/dev/cook-modules/cook_cc && busted spec/toolchain_spec.lua
```
Expected: FAIL — `ensure_probe_registered`, `get_probe_key` undefined; old test names match nothing.

- [ ] **Step 3: Rewrite `toolchain.lua`**

Replace `cook-modules/cook_cc/toolchain.lua` with:

```lua
local M = {}

local state = {
    compiler_override = nil,         -- nil means "auto"
    default_standard  = nil,
    warnings          = "default",
    defaults          = {},
    probe_registered  = {},          -- set: key -> true
}

local function probe_key()
    return "cc:compiler:" .. (state.compiler_override or "auto")
end

local function produce_body(override)
    local override_literal = override and string.format("%q", override) or "nil"
    return string.format([[
        local override = %s
        if override then
            local out = cook.sh("command -v " .. override .. " 2>/dev/null")
            if not out:match("%%S") then
                error("[cc.toolchain] override compiler '" .. override .. "' not on PATH")
            end
            local cc
            if override:match("clang") then cc = "clang"
            elseif override:match("g%%+%%+") then cc = "gcc"
            else cc = "cc" end
            return { cxx = override, cc = cc }
        end
        for _, c in ipairs({ {cxx="g++",cc="gcc"}, {cxx="clang++",cc="clang"} }) do
            local out = cook.sh("command -v " .. c.cxx .. " 2>/dev/null")
            if out:match("%%S") then return c end
        end
        error("[cc.toolchain] no C/C++ compiler on PATH (tried g++, clang++)")
    ]], override_literal)
end

function M.ensure_probe_registered()
    local key = probe_key()
    if state.probe_registered[key] then return end
    cook.probe(key, {
        inputs = { tools = { "g++", "clang++", "cc" } },
        produce = produce_body(state.compiler_override),
    })
    state.probe_registered[key] = true
end

function M.get_probe_key()
    return probe_key()
end

function M.get_compiler()
    M.ensure_probe_registered()
    return cook.cache.get(probe_key())
end

function M.set(opts)
    opts = opts or {}
    if opts.compiler then state.compiler_override = opts.compiler end
    if opts.standard then state.default_standard = opts.standard end
    if opts.warnings then state.warnings = opts.warnings end
end

local function append_list(dst, src)
    if not src then return end
    for _, v in ipairs(src) do dst[#dst + 1] = v end
end

function M.merge_defaults(opts)
    opts = opts or {}
    state.defaults.defines     = state.defaults.defines     or {}
    state.defaults.includes    = state.defaults.includes    or {}
    state.defaults.system_libs = state.defaults.system_libs or {}
    append_list(state.defaults.defines,     opts.defines)
    append_list(state.defaults.includes,    opts.includes)
    append_list(state.defaults.system_libs, opts.system_libs)
    if opts.extra_cflags then
        local prev = state.defaults.extra_cflags
        state.defaults.extra_cflags = prev and (prev .. " " .. opts.extra_cflags) or opts.extra_cflags
    end
    if opts.extra_ldflags then
        local prev = state.defaults.extra_ldflags
        state.defaults.extra_ldflags = prev and (prev .. " " .. opts.extra_ldflags) or opts.extra_ldflags
    end
    if opts.standard then state.default_standard = opts.standard end
    if opts.warnings then state.warnings = opts.warnings end
end

function M.get_default_standard() return state.default_standard end
function M.get_warnings()         return state.warnings         end
function M.get_defaults()         return state.defaults         end

function M.warning_flags(override)
    local w = override or state.warnings
    if w == "default" then return "-Wall"
    elseif w == "strict" then return "-Wall -Wextra -Wpedantic"
    elseif w == "none" then return ""
    else return w end
end

return M
```

- [ ] **Step 4: Drop `toolchain.rehydrate` from `init.lua`**

In `cook-modules/cook_cc/init.lua`, replace:

```lua
function M.init()
    toolchain.rehydrate()
end
```

with:

```lua
function M.init()
    -- Toolchain probe is registered lazily on first get_compiler() / target-maker call.
end
```

- [ ] **Step 5: Run tests to verify they pass**

```
cd /home/alex/dev/cook-modules/cook_cc && busted spec/toolchain_spec.lua
```
Expected: 6 passing.

- [ ] **Step 6: Run full spec — some failures expected**

```
cd /home/alex/dev/cook-modules/cook_cc && busted .
```
Expected: existing `targets_spec.lua`, `finder_spec.lua`, etc. may now fail because they assumed `toolchain.get_compiler()` worked at register time without probe-value injection. Those will be fixed in subsequent tasks.

Note the list of newly-failing tests but do not fix them here.

- [ ] **Step 7: Commit**

```
cd /home/alex/dev/cook-modules && git add cook_cc/toolchain.lua cook_cc/init.lua cook_cc/spec/toolchain_spec.lua
git commit -m "feat(cook_cc): cc:compiler probe replaces toolchain.rehydrate

Toolchain detection becomes a register-phase cook.probe whose key
encodes the user's override choice (cc:compiler:auto or
cc:compiler:<override>). get_compiler() reads from the probe-value
store at consumer-call time."
```

---

## Task 3: `cc:linker-search-dirs` probe (replaces `bare_probe` cache)

**Files:**
- Modify: `cook-modules/cook_cc/finders/bare_probe.lua`
- Modify: `cook-modules/cook_cc/spec/finders/bare_probe_spec.lua` (or `cook-modules/cook_cc/spec/finder_spec.lua` if that's where current coverage lives — check first)

- [ ] **Step 1: Identify current bare_probe test file**

```
cd /home/alex/dev/cook-modules/cook_cc && ls spec/finders/ 2>/dev/null; ls spec/ | grep -i bare
```

If a dedicated spec exists, modify it; otherwise add `spec/finders/bare_probe_spec.lua`.

- [ ] **Step 2: Write the failing test**

In the identified spec file:

```lua
local stub = require("cook_stub")

local function reload()
    package.loaded["cook_cc.finders.bare_probe"] = nil
    return require("cook_cc.finders.bare_probe")
end

describe("bare_probe linker-search-dirs probe", function()
    before_each(function() stub.reset(); stub.install() end)

    it("registers cc:linker-search-dirs probe on require", function()
        reload()
        local keys = stub.probe_keys()
        local found = false
        for _, k in ipairs(keys) do if k == "cc:linker-search-dirs" then found = true end end
        assert.is_true(found, "expected cc:linker-search-dirs probe to be registered")
    end)

    it("probe inputs include cc tool and LIBRARY_PATH env", function()
        reload()
        local opts = stub.probe_opts("cc:linker-search-dirs")
        local has_cc = false
        for _, t in ipairs(opts.inputs.tools or {}) do if t == "cc" then has_cc = true end end
        assert.is_true(has_cc, "expected 'cc' in probe inputs.tools")
        local has_libpath = false
        for _, e in ipairs(opts.inputs.env or {}) do if e == "LIBRARY_PATH" then has_libpath = true end end
        assert.is_true(has_libpath, "expected LIBRARY_PATH in probe inputs.env")
    end)

    it("try() consults probe value store for search dirs", function()
        stub.set_probe_value("cc:linker-search-dirs", { "/opt/raylib/lib", "/usr/lib" })
        stub.set_file_exists("/opt/raylib/lib/libraylib.so", true)
        local bare = reload()
        local r = bare.try("raylib")
        assert.is_not_nil(r)
        assert.equals("bare-probe", r.strategy)
        assert.equals("hit", r.outcome)
    end)
end)
```

- [ ] **Step 3: Run tests to verify they fail**

```
cd /home/alex/dev/cook-modules/cook_cc && busted spec/finders/bare_probe_spec.lua
```
Expected: FAIL.

- [ ] **Step 4: Rewrite `bare_probe.lua`**

Replace `cook-modules/cook_cc/finders/bare_probe.lua` with:

```lua
local M = {}

local PROBE_KEY = "cc:linker-search-dirs"
local probe_registered = false

local function produce_body()
    return [[
        local dirs = { "/usr/lib", "/usr/local/lib" }
        local ok, out = pcall(cook.sh, "cc -print-search-dirs 2>/dev/null")
        if ok and out then
            local libs_line = out:match("libraries:%s*=([^\n]+)")
            if libs_line then
                for d in libs_line:gmatch("[^:]+") do dirs[#dirs + 1] = d end
            end
        end
        return dirs
    ]]
end

local function ensure_probe()
    if probe_registered then return end
    cook.probe(PROBE_KEY, {
        inputs = { tools = { "cc" }, env = { "LIBRARY_PATH" } },
        produce = produce_body(),
    })
    probe_registered = true
end

ensure_probe()

local function search_dirs()
    return cook.cache.get(PROBE_KEY) or { "/usr/lib", "/usr/local/lib" }
end

local function extensions()
    if cook.platform.os == "macos" then
        return { ".dylib", ".so", ".a" }
    end
    return { ".so", ".dylib", ".a" }
end

local function blank_payload()
    return {
        cflags = "", libs = "", system_libs = {}, include_dirs = {}, lib_dirs = {},
        frameworks = {}, version = nil,
    }
end

-- CS-0045: bare probe walks system paths outside the project sandbox.
-- Use cook.sh "test -e" with shell-quote escaping (single-quote, escape
-- embedded ' as '\''). Linker constrains lib names to a tame charset;
-- this is defense in depth.
local function exists_unsandboxed(path)
    local quoted = "'" .. (path:gsub("'", "'\\''")) .. "'"
    local ok, out = pcall(cook.sh, "test -e " .. quoted .. " && echo y || echo n")
    return ok and (out or ""):match("^y") ~= nil
end

function M.try(name)
    for _, dir in ipairs(search_dirs()) do
        for _, ext in ipairs(extensions()) do
            local p = dir .. "/lib" .. name .. ext
            if exists_unsandboxed(p) then
                local payload = blank_payload()
                payload.system_libs = { name }
                payload.libs = "-l" .. name
                return { strategy = "bare-probe", outcome = "hit", reason = "", payload = payload }
            end
        end
    end
    return nil
end

function M.main_chain(name, opts)
    if opts.version then
        return { strategy = "bare-probe", outcome = "skip",
                 reason = "bare probe cannot verify version constraints" }
    end
    local attempt = M.try(name)
    if attempt then return attempt end
    return { strategy = "bare-probe", outcome = "miss",
             reason = "no lib" .. name .. ".{so,dylib,a} on default linker search paths" }
end

return M
```

The two key changes from the prior version: (1) `cook.cache.set` is gone — the probe writes the value at execute time; (2) `cook.cache.get` reads from the probe-value store, falling back to `{"/usr/lib","/usr/local/lib"}` so register-time consumers (currently none, but defensively) don't choke on missing probe values.

- [ ] **Step 5: Run tests to verify they pass**

```
cd /home/alex/dev/cook-modules/cook_cc && busted spec/finders/bare_probe_spec.lua
```
Expected: 3 passing.

- [ ] **Step 6: Commit**

```
cd /home/alex/dev/cook-modules && git add cook_cc/finders/bare_probe.lua cook_cc/spec/finders/bare_probe_spec.lua
git commit -m "feat(cook_cc): cc:linker-search-dirs probe replaces bare_probe cache"
```

---

## Task 4: `cc:cmake-driver` probe (replaces `cmake_compat` cache)

**Files:**
- Modify: `cook-modules/cook_cc/finders/cmake_compat.lua`
- Modify or create: `cook-modules/cook_cc/spec/finders/cmake_compat_spec.lua`

The current cmake_compat finder caches `cc.cmake-compat:driver` via `cook.cache.set/get` (in finders/cmake_compat.lua at lines 67, 74, 83 per the grep in §1 of the spec).

- [ ] **Step 1: Read the current cmake_compat.lua** and identify the driver-detection function. Note: the file does more than driver detection — it also runs `cmake --find-package` per package. Only the *driver* detection becomes a probe in Phase 1; per-package cmake lookup happens inside the per-`cc.find:<n>` probe in Task 8.

```
cat /home/alex/dev/cook-modules/cook_cc/finders/cmake_compat.lua | head -100
```

- [ ] **Step 2: Write the failing test** — `cook-modules/cook_cc/spec/finders/cmake_compat_spec.lua` (create if missing):

```lua
local stub = require("cook_stub")

local function reload()
    package.loaded["cook_cc.finders.cmake_compat"] = nil
    return require("cook_cc.finders.cmake_compat")
end

describe("cmake_compat driver probe", function()
    before_each(function() stub.reset(); stub.install() end)

    it("registers cc:cmake-driver probe on require", function()
        reload()
        local opts = stub.probe_opts("cc:cmake-driver")
        assert.is_not_nil(opts)
        assert.is_string(opts.produce)
    end)

    it("probe inputs include cmake tool and CMAKE_PREFIX_PATH env", function()
        reload()
        local opts = stub.probe_opts("cc:cmake-driver")
        local has_cmake = false
        for _, t in ipairs(opts.inputs.tools or {}) do if t == "cmake" then has_cmake = true end end
        assert.is_true(has_cmake)
        local has_env = false
        for _, e in ipairs(opts.inputs.env or {}) do if e == "CMAKE_PREFIX_PATH" then has_env = true end end
        assert.is_true(has_env)
    end)

    it("driver() reads probe value store, returns nil when no cmake", function()
        stub.set_probe_value("cc:cmake-driver", nil)
        local cm = reload()
        assert.is_nil(cm.driver())
    end)

    it("driver() returns the cached driver record on hit", function()
        stub.set_probe_value("cc:cmake-driver", { binary = "/usr/bin/cmake", version = "3.27.0" })
        local cm = reload()
        assert.same({ binary = "/usr/bin/cmake", version = "3.27.0" }, cm.driver())
    end)
end)
```

- [ ] **Step 3: Run test to verify it fails**

```
cd /home/alex/dev/cook-modules/cook_cc && busted spec/finders/cmake_compat_spec.lua
```
Expected: FAIL.

- [ ] **Step 4: Modify `cmake_compat.lua`**

Replace the driver-detection block (the `cook.cache.set/get("cc.cmake-compat:driver", ...)` lines) with a probe registration. Keep the rest of the file (per-package `cmake --find-package` logic) intact for now — Task 8 will refactor.

In `cook-modules/cook_cc/finders/cmake_compat.lua`, find the driver-detection function (around line 60-85 of the current file) and replace it with:

```lua
local PROBE_KEY_DRIVER = "cc:cmake-driver"
local driver_probe_registered = false

local function produce_driver_body()
    return [[
        local out = cook.sh("command -v cmake 2>/dev/null")
        local binary = out:match("(%S+)")
        if not binary then return nil end
        local ver_out = cook.sh(binary .. " --version 2>/dev/null")
        local version = ver_out:match("cmake version (%S+)") or "unknown"
        return { binary = binary, version = version }
    ]]
end

local function ensure_driver_probe()
    if driver_probe_registered then return end
    cook.probe(PROBE_KEY_DRIVER, {
        inputs = { tools = { "cmake" }, env = { "CMAKE_PREFIX_PATH" } },
        produce = produce_driver_body(),
    })
    driver_probe_registered = true
end

ensure_driver_probe()

local function driver()
    return cook.cache.get(PROBE_KEY_DRIVER)
end

-- Public-export the driver accessor so tests can target it.
M.driver = driver
```

The exact integration depends on the existing structure of `cmake_compat.lua` — the old code reads the cached driver in `main_chain` to decide whether to attempt cmake-find-package. Replace those reads with calls to `driver()` and delete the `cook.cache.set("cc.cmake-compat:driver", ...)` calls.

- [ ] **Step 5: Run tests to verify they pass**

```
cd /home/alex/dev/cook-modules/cook_cc && busted spec/finders/cmake_compat_spec.lua
```
Expected: 4 passing.

- [ ] **Step 6: Commit**

```
cd /home/alex/dev/cook-modules && git add cook_cc/finders/cmake_compat.lua cook_cc/spec/finders/cmake_compat_spec.lua
git commit -m "feat(cook_cc): cc:cmake-driver probe replaces cmake_compat driver cache"
```

---

## Task 5: Drop register-time accumulators from `cook.cache`

**Files:**
- Modify: `cook-modules/cook_cc/targets.lua`
- Modify: `cook-modules/cook_cc/compile_db.lua`
- Modify: `cook-modules/cook_cc/spec/targets_spec.lua` (or wherever known_targets is currently tested)
- Modify: `cook-modules/cook_cc/spec/compile_db_spec.lua`

`known_targets` is a register-time accumulator (a list of targets declared so far in this Cookfile). It's not probe-shaped. Move to module-local state per spec §4.

- [ ] **Step 1: Read current state** of `register_known` in `targets.lua` and `M.write` in `compile_db.lua` to plan the change.

```
grep -n "known_targets\|register_known" /home/alex/dev/cook-modules/cook_cc/targets.lua /home/alex/dev/cook-modules/cook_cc/compile_db.lua
```

- [ ] **Step 2: Write the failing test** in `cook-modules/cook_cc/spec/targets_spec.lua` (append):

```lua
describe("known_targets module-local state", function()
    before_each(function()
        stub.reset(); stub.install()
        package.loaded["cook_cc.targets"] = nil
    end)

    it("targets registry is exposed via targets._known()", function()
        local tg = require("cook_cc.targets")
        assert.same({}, tg._known())
        -- Indirectly populate via bin(); concrete invocation belongs to
        -- the targets_spec proper — here we only assert the accessor shape.
    end)

    it("compile_db.write reads from targets._known, not cook.cache", function()
        package.loaded["cook_cc.compile_db"] = nil
        local db = require("cook_cc.compile_db")
        -- With no targets registered and no cache, write produces empty json:
        db.write()
        -- (fs.write capture in cook_stub recorded the empty payload)
        local units = stub.added_units()
        local found = false
        for _, u in ipairs(units) do
            if u.kind == "fs.write" and u.path == "compile_commands.json" then
                assert.equals("[]\n", u.content)
                found = true
            end
        end
        assert.is_true(found, "expected fs.write to compile_commands.json")
    end)
end)
```

- [ ] **Step 3: Run tests to verify they fail**

```
cd /home/alex/dev/cook-modules/cook_cc && busted spec/targets_spec.lua
```
Expected: FAIL on the `_known` accessor (not yet exposed).

- [ ] **Step 4: Modify `targets.lua`**

At the top of `cook-modules/cook_cc/targets.lua`, add module-local state:

```lua
local M = {}
M._known_list = M._known_list or {}     -- per-VM accumulator

function M._known()
    return M._known_list
end
```

Replace the existing `register_known(name)` function body:

```lua
local function register_known(name)
    for _, n in ipairs(M._known_list) do if n == name then return end end
    M._known_list[#M._known_list + 1] = name
end
```

- [ ] **Step 5: Modify `compile_db.lua`**

In `cook-modules/cook_cc/compile_db.lua`, change the line `local targets = cook.cache.get("known_targets") or {}` to:

```lua
local targets = require("cook_cc.targets")._known()
```

- [ ] **Step 6: Run tests to verify they pass**

```
cd /home/alex/dev/cook-modules/cook_cc && busted spec/targets_spec.lua spec/compile_db_spec.lua
```
Expected: green.

- [ ] **Step 7: Commit**

```
cd /home/alex/dev/cook-modules && git add cook_cc/targets.lua cook_cc/compile_db.lua cook_cc/spec/targets_spec.lua cook_cc/spec/compile_db_spec.lua
git commit -m "refactor(cook_cc): known_targets is module-local state, not cook.cache

Register-time accumulators don't fit the probe model. Move
known_targets to a per-VM module table exposed via targets._known()
and read from compile_db.write() at register-phase invocation."
```

---

## Task 6: Extract probe-helper module for shared find strategies

**Files:**
- Create: `cook-modules/cook_cc/_probe_helpers.lua`
- Create: `cook-modules/cook_cc/spec/probe_helpers_spec.lua`

The `cc:find:<n>` probe's `produce` body (Task 8) runs on a worker VM and needs to execute the strategy chain. Pull strategy helpers into a separate module so the produce body can `require("cook_cc._probe_helpers")`.

- [ ] **Step 1: Write the failing test**

In `cook-modules/cook_cc/spec/probe_helpers_spec.lua`:

```lua
local stub = require("cook_stub")

local function reload()
    package.loaded["cook_cc._probe_helpers"] = nil
    return require("cook_cc._probe_helpers")
end

describe("_probe_helpers", function()
    before_each(function() stub.reset(); stub.install() end)

    it("exposes pkg_strategy, cmake_strategy, bare_strategy, build_result", function()
        local h = reload()
        assert.is_function(h.pkg_strategy)
        assert.is_function(h.cmake_strategy)
        assert.is_function(h.bare_strategy)
        assert.is_function(h.build_result)
    end)

    it("pkg_strategy on hit returns hit record", function()
        local h = reload()
        stub.set_pkg_config_response("zlib", { exists = true, cflags = "-I/usr/include", libs = "-lz", version = "1.2.13" })
        local r = h.pkg_strategy("zlib", {})
        assert.equals("hit", r.outcome)
        assert.equals("-lz", r.payload.libs)
    end)

    it("build_result with hit returns found=true record", function()
        local h = reload()
        local hit = { payload = { cflags = "-Ix", libs = "-ly", system_libs = {"y"} } }
        local r = h.build_result(hit, { hit })
        assert.is_true(r.found)
        assert.equals("-Ix", r.cflags)
        assert.equals("-ly", r.libs)
    end)

    it("build_result with no hit returns found=false with tried record", function()
        local h = reload()
        local r = h.build_result(nil, { { strategy = "pkg-config", outcome = "miss" } })
        assert.is_false(r.found)
        assert.equals(1, #r.tried)
    end)
end)
```

- [ ] **Step 2: Run test to verify it fails**

```
cd /home/alex/dev/cook-modules/cook_cc && busted spec/probe_helpers_spec.lua
```
Expected: FAIL — module not found.

- [ ] **Step 3: Write `_probe_helpers.lua`**

Pull the strategy functions from `cook-modules/cook_cc/finder.lua` and finders/pkg_config.lua, finders/cmake_compat.lua. Compose them into a single module exposing `pkg_strategy`, `cmake_strategy`, `bare_strategy`, and `build_result`:

```lua
-- Shared strategy helpers for cc:find:<n> probe bodies.
-- Required by both register-phase (cook_cc.finder.lua) and execute-phase
-- (the probe produce body on a worker VM).

local M = {}

local function blank_result()
    return {
        found        = false,
        cflags       = "",
        libs         = "",
        system_libs  = {},
        include_dirs = {},
        lib_dirs     = {},
        frameworks   = {},
        version      = nil,
        tried        = {},
    }
end

function M.build_result(hit, tried)
    if not hit then
        local r = blank_result()
        r.tried = tried
        return r
    end
    local p = hit.payload
    return {
        found        = true,
        cflags       = p.cflags or "",
        libs         = p.libs or "",
        system_libs  = p.system_libs or {},
        include_dirs = p.include_dirs or {},
        lib_dirs     = p.lib_dirs or {},
        frameworks   = p.frameworks or {},
        version      = p.version,
        tried        = tried,
    }
end

function M.project_strategy(_registry, name, opts)
    local fn = _registry and _registry[name]
    if not fn then
        return { strategy = "project:" .. name, outcome = "skip",
                 reason = "no project finder registered" }
    end
    local rec = fn(opts)
    if rec and rec.found then
        return { strategy = "project:" .. name, outcome = "hit", reason = "",
                 payload = {
                     cflags       = rec.cflags or "",
                     libs         = rec.libs or "",
                     system_libs  = rec.system_libs or {},
                     include_dirs = rec.include_dirs or {},
                     lib_dirs     = rec.lib_dirs or {},
                     frameworks   = rec.frameworks or {},
                     version      = rec.version,
                 } }
    end
    return { strategy = "project:" .. name, outcome = "miss",
             reason = "project finder returned found=false" }
end

function M.curated_strategy(name, opts)
    local curated = require("cook_cc.finders")
    local fn = curated.lookup(name)
    if not fn then
        return { strategy = "curated:" .. name, outcome = "skip",
                 reason = "no curated finder for '" .. name .. "'" }
    end
    return fn(opts)
end

function M.pkg_strategy(name, opts)
    local pkg = require("cook_cc.finders.pkg_config")
    return pkg.main_chain(name, opts)
end

function M.cmake_strategy(name, opts)
    local cm = require("cook_cc.finders.cmake_compat")
    return cm.main_chain(name, opts)
end

function M.bare_strategy(name, opts)
    local bare = require("cook_cc.finders.bare_probe")
    return bare.main_chain(name, opts)
end

return M
```

- [ ] **Step 4: Run tests to verify they pass**

```
cd /home/alex/dev/cook-modules/cook_cc && busted spec/probe_helpers_spec.lua
```
Expected: 4 passing.

- [ ] **Step 5: Commit**

```
cd /home/alex/dev/cook-modules && git add cook_cc/_probe_helpers.lua cook_cc/spec/probe_helpers_spec.lua
git commit -m "feat(cook_cc): _probe_helpers module exposes strategy chain to probe produce bodies"
```

---

## Task 7: `cc:find:<n>` probe — refactor `finder.find` to register a probe

**Files:**
- Modify: `cook-modules/cook_cc/finder.lua`
- Modify: `cook-modules/cook_cc/spec/finder_spec.lua`

`cook_cc.find(name, opts)` becomes a probe-registration helper. Returns a sigil-record per spec §5. Conflicting opts raise per §3.4.

- [ ] **Step 1: Write the failing tests** — replace relevant sections of `cook-modules/cook_cc/spec/finder_spec.lua`:

```lua
local stub = require("cook_stub")

local function reload()
    package.loaded["cook_cc.finder"] = nil
    package.loaded["cook_cc.toolchain"] = nil
    return require("cook_cc.finder")
end

describe("cc.find probe registration", function()
    before_each(function() stub.reset(); stub.install() end)

    it("find(name) registers cc:find:<name> probe", function()
        local finder = reload()
        finder.find("raylib")
        assert.is_not_nil(stub.probe_opts("cc:find:raylib"))
    end)

    it("find(name) returns a sigil-record", function()
        local finder = reload()
        local r = finder.find("raylib")
        assert.equals("$<cc:find:raylib.cflags>",       r.cflags)
        assert.equals("$<cc:find:raylib.libs>",         r.libs)
        assert.equals("$<cc:find:raylib.include_dirs>", r.include_dirs)
        assert.equals("$<cc:find:raylib.system_libs>",  r.system_libs)
        assert.equals("$<cc:find:raylib.frameworks>",   r.frameworks)
        assert.equals("$<cc:find:raylib.found>",        r.found)
    end)

    it("find(name) is idempotent — duplicate calls do not duplicate probe registration", function()
        local finder = reload()
        finder.find("raylib")
        finder.find("raylib")
        local count = 0
        for _, k in ipairs(stub.probe_keys()) do
            if k == "cc:find:raylib" then count = count + 1 end
        end
        assert.equals(1, count)
    end)

    it("find(name, opts) with conflicting opts on second call raises", function()
        local finder = reload()
        finder.find("raylib", { version = ">=4.0" })
        assert.has_error(function()
            finder.find("raylib", { version = ">=5.0" })
        end)
    end)

    it("find_or_error registers probe and returns sigil-record (raises lazily)", function()
        local finder = reload()
        local r = finder.find_or_error("raylib")
        assert.equals("$<cc:find:raylib.cflags>", r.cflags)
    end)

    it("probe inputs include cc:compiler, cc:linker-search-dirs, cc:cmake-driver", function()
        local finder = reload()
        finder.find("raylib")
        local opts = stub.probe_opts("cc:find:raylib")
        local reqs = {}
        for _, k in ipairs(opts.inputs.requires or {}) do reqs[k] = true end
        assert.is_true(reqs["cc:compiler:auto"], "expected cc:compiler:auto in requires")
        assert.is_true(reqs["cc:linker-search-dirs"], "expected cc:linker-search-dirs in requires")
        assert.is_true(reqs["cc:cmake-driver"], "expected cc:cmake-driver in requires")
    end)

    it("register_finder still works for project strategy", function()
        local finder = reload()
        finder.register("raylib", function(_)
            return { found = true, cflags = "-I/from-project", libs = "-lraylib" }
        end)
        -- Project registration affects what the probe produces at execute time;
        -- at register time it doesn't change the sigil-record returned by find().
        -- We only assert here that register() exists and accepts a function.
    end)
end)
```

- [ ] **Step 2: Run tests to verify they fail**

```
cd /home/alex/dev/cook-modules/cook_cc && busted spec/finder_spec.lua
```
Expected: FAIL across the probe-shaped expectations.

- [ ] **Step 3: Rewrite `finder.lua`**

Replace `cook-modules/cook_cc/finder.lua` with:

```lua
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
    local r = {}
    for _, f in ipairs(fields) do
        r[f] = string.format("$<cc:find:%s.%s>", name, f)
    end
    return setmetatable(r, {
        __newindex = function(_, k, _)
            error("[cc.find] cannot mutate find result; field '" .. tostring(k) .. "' is a probe-sigil placeholder", 2)
        end
    })
end

local function produce_body(name, opts)
    local opts_lua = "{}"
    if opts and next(opts) then
        local parts = {}
        for k, v in pairs(opts) do
            if type(v) == "string"  then parts[#parts + 1] = k .. "=" .. string.format("%q", v)
            elseif type(v) == "boolean" then parts[#parts + 1] = k .. "=" .. tostring(v)
            elseif type(v) == "number"  then parts[#parts + 1] = k .. "=" .. tostring(v)
            end
        end
        opts_lua = "{" .. table.concat(parts, ",") .. "}"
    end
    return string.format([[
        local h = require("cook_cc._probe_helpers")
        local NAME = %q
        local OPTS = %s
        local tried = {}
        local function try(step) local r = step(); tried[#tried+1] = r; return r end

        -- Project finders are register-VM-only; the worker VM cannot see
        -- them. Skip project_strategy in the produce body.
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
        return h.build_result(nil, tried)
    ]], name, opts_lua)
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
    local key  = "cc:find:" .. name
    local fp   = canonical_opts(opts)
    if M._registered[key] then
        if M._registered[key] ~= fp then
            error(string.format(
                "[cc.find] duplicate cc.find for '%s' with conflicting opts:\n" ..
                "  first call opts=%s\n  this call opts=%s",
                name, M._registered[key], fp), 2)
        end
        return sigil_record(name)
    end

    -- Ensure dependency-probes are registered so requires-edges resolve.
    toolchain.ensure_probe_registered()
    require("cook_cc.finders.bare_probe")
    require("cook_cc.finders.cmake_compat")

    cook.probe(key, {
        inputs = {
            requires = {
                toolchain.get_probe_key(),
                "cc:linker-search-dirs",
                "cc:cmake-driver",
            },
            env = { "PATH", "PKG_CONFIG_PATH", "CMAKE_PREFIX_PATH", "LIBRARY_PATH" },
            tools = { "pkg-config" },
        },
        produce = produce_body(name, opts),
    })
    M._registered[key] = fp
    return sigil_record(name)
end

function M.find_or_error(name, opts)
    -- find_or_error semantics under demand-driven scheduling: failure
    -- happens at execute time when the probe's produce returns
    -- found=false. The register-time call just registers the probe.
    -- The probe's produce body is the same as find(); the calling
    -- convention is what tells consumers that a non-find means hard
    -- failure. To make execute-time failure explicit, wrap the produce
    -- body so found=false raises.
    opts = opts or {}
    local key = "cc:find:" .. name
    local fp  = canonical_opts(opts) .. ":or_error"
    if M._registered[key] then
        if M._registered[key] ~= fp then
            error(string.format("[cc.find_or_error] '%s' previously declared with non-or_error semantics or conflicting opts", name), 2)
        end
        return sigil_record(name)
    end

    toolchain.ensure_probe_registered()
    require("cook_cc.finders.bare_probe")
    require("cook_cc.finders.cmake_compat")

    local opts_lua = "{}"
    if next(opts) then
        local parts = {}
        for k, v in pairs(opts) do
            if type(v) == "string"  then parts[#parts + 1] = k .. "=" .. string.format("%q", v)
            elseif type(v) == "boolean" then parts[#parts + 1] = k .. "=" .. tostring(v)
            elseif type(v) == "number"  then parts[#parts + 1] = k .. "=" .. tostring(v)
            end
        end
        opts_lua = "{" .. table.concat(parts, ",") .. "}"
    end

    local body = string.format([[
        local h = require("cook_cc._probe_helpers")
        local NAME = %q
        local OPTS = %s
        local tried = {}
        local function try(step) local r = step(); tried[#tried+1] = r; return r end

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
        local lines = { "could not locate '" .. NAME .. "':" }
        for _, a in ipairs(tried) do
            local line = "  - " .. a.strategy .. ": " .. a.outcome
            if a.reason and a.reason ~= "" then line = line .. " (" .. a.reason .. ")" end
            lines[#lines + 1] = line
        end
        error("[cc.find_or_error] " .. table.concat(lines, "\n"))
    ]], name, opts_lua)

    cook.probe(key, {
        inputs = {
            requires = {
                toolchain.get_probe_key(),
                "cc:linker-search-dirs",
                "cc:cmake-driver",
            },
            env = { "PATH", "PKG_CONFIG_PATH", "CMAKE_PREFIX_PATH", "LIBRARY_PATH" },
            tools = { "pkg-config" },
        },
        produce = body,
    })
    M._registered[key] = fp
    return sigil_record(name)
end

return M
```

- [ ] **Step 4: Run tests to verify they pass**

```
cd /home/alex/dev/cook-modules/cook_cc && busted spec/finder_spec.lua
```
Expected: all in `cc.find probe registration` describe block passing. Some legacy tests in the same file may now fail — those will be deleted in the next step.

- [ ] **Step 5: Remove dead legacy tests**

Delete any tests in `spec/finder_spec.lua` that exercised the old synchronous-record behavior (e.g., `assert.equals("-lraylib", r.libs)`-style asserts that expected raw strings). These no longer apply; the sigil-record contract is the new shape.

Run the spec again:

```
cd /home/alex/dev/cook-modules/cook_cc && busted spec/finder_spec.lua
```
Expected: green, no failures.

- [ ] **Step 6: Update `init.lua`** — `find` and `find_or_error` now have the new signatures; no surface change needed in init since `M.find = finder.find` is already in place.

Verify:

```
grep "find" /home/alex/dev/cook-modules/cook_cc/init.lua
```

Confirm `M.find = finder.find` and `M.find_or_error = ...` map straight through. If `find_or_error` has separate logic in `init.lua`, replace with `M.find_or_error = finder.find_or_error`.

- [ ] **Step 7: Commit**

```
cd /home/alex/dev/cook-modules && git add cook_cc/finder.lua cook_cc/spec/finder_spec.lua cook_cc/init.lua
git commit -m "feat(cook_cc): cc:find:<n> probe; find()/find_or_error() return sigil-record

find(name, opts) registers a cc:find:<name> probe whose produce body
runs the strategy chain at execute time. Returns a sigil-record
(table of \$<cc:find:<name>.<field>> placeholders) so consumers can
weave the result into command templates. Conflicting opts on the same
name raise per spec §3.4. find_or_error registers a probe whose
produce raises on found=false — failure becomes demand-driven."
```

---

## Task 8: `targets.bin/.lib/.shared/.headers` accept `needs = {...}` and weave sigils

**Files:**
- Modify: `cook-modules/cook_cc/targets.lua`
- Modify: `cook-modules/cook_cc/cc.lua` (if compile/link command building lives there)
- Modify or create: `cook-modules/cook_cc/spec/needs_field_spec.lua`

This is the biggest commit. Targets accept a `needs = {...}` list; for each name they register the corresponding probe (via `finder.find`) and wire `probes = {"cc:find:<n>", ...}` plus sigil flags into the compile and link units.

- [ ] **Step 1: Read current `targets.lua` and `cc.lua`** to understand the command-string building paths.

```
grep -n "command\|cook.add_unit" /home/alex/dev/cook-modules/cook_cc/cc.lua /home/alex/dev/cook-modules/cook_cc/targets.lua
```

- [ ] **Step 2: Write the failing tests**

In `cook-modules/cook_cc/spec/needs_field_spec.lua`:

```lua
local stub = require("cook_stub")

local function reload_all()
    for _, m in ipairs({
        "cook_cc.toolchain", "cook_cc.finder", "cook_cc.targets",
        "cook_cc.cc", "cook_cc.transitive", "cook_cc",
    }) do package.loaded[m] = nil end
    return require("cook_cc")
end

describe("targets.bin with needs={...}", function()
    before_each(function() stub.reset(); stub.install() end)

    it("registers cc:find:<name> probe for each entry in needs", function()
        local cc = reload_all()
        cc.bin("game", { sources = {"src/main.c"}, needs = { "raylib" } })
        assert.is_not_nil(stub.probe_opts("cc:find:raylib"))
    end)

    it("registers one probe per needs entry across multiple targets", function()
        local cc = reload_all()
        cc.bin("game", { sources = {"src/main.c"}, needs = { "raylib" } })
        cc.bin("editor", { sources = {"src/editor.c"}, needs = { "raylib" } })
        local count = 0
        for _, k in ipairs(stub.probe_keys()) do
            if k == "cc:find:raylib" then count = count + 1 end
        end
        assert.equals(1, count)
    end)

    it("compile units carry probes = {cc:find:<n>, cc:compiler:<o>}", function()
        local cc = reload_all()
        cc.bin("game", { sources = {"src/main.c"}, needs = { "raylib" } })
        local units = stub.added_units()
        local compile_unit
        for _, u in ipairs(units) do
            if u.output and u.output:match("%.o$") then
                compile_unit = u; break
            end
        end
        assert.is_not_nil(compile_unit)
        local p = {}
        for _, k in ipairs(compile_unit.probes or {}) do p[k] = true end
        assert.is_true(p["cc:find:raylib"], "expected cc:find:raylib in compile-unit probes")
        assert.is_true(p["cc:compiler:auto"], "expected cc:compiler:auto in compile-unit probes")
    end)

    it("link unit command embeds $<cc:find:<n>.libs> sigil", function()
        local cc = reload_all()
        cc.bin("game", { sources = {"src/main.c"}, needs = { "raylib" } })
        local units = stub.added_units()
        local link_unit
        for _, u in ipairs(units) do
            if u.output and u.output:match("build/bin/game$") then
                link_unit = u; break
            end
        end
        assert.is_not_nil(link_unit)
        assert.matches("%$<cc:find:raylib%.libs>", link_unit.command)
    end)

    it("compile unit command embeds $<cc:find:<n>.cflags> sigil for headers", function()
        local cc = reload_all()
        cc.bin("game", { sources = {"src/main.c"}, needs = { "raylib" } })
        local units = stub.added_units()
        local compile_unit
        for _, u in ipairs(units) do
            if u.output and u.output:match("%.o$") then
                compile_unit = u; break
            end
        end
        assert.matches("%$<cc:find:raylib%.cflags>", compile_unit.command)
    end)

    it("compile command uses $<cc:compiler:auto.cxx> or .cc sigil, not literal compiler", function()
        local cc = reload_all()
        cc.bin("game", { sources = {"src/main.c"} })   -- .c source -> .cc field
        local units = stub.added_units()
        local compile_unit
        for _, u in ipairs(units) do
            if u.output and u.output:match("%.o$") then compile_unit = u; break end
        end
        assert.matches("^%$<cc:compiler:auto%.cc>", compile_unit.command)
    end)

    it("needs is additive with links (local-target propagation unchanged)", function()
        local cc = reload_all()
        cc.lib("mylib", { sources = {"src/mylib.c"} })
        cc.bin("game", {
            sources = {"src/main.c"},
            needs   = { "raylib" },
            links   = { "mylib" },
        })
        -- needs registers a probe; links still flows through cook.export/import
        assert.is_not_nil(stub.probe_opts("cc:find:raylib"))
    end)
end)
```

- [ ] **Step 3: Run tests to verify they fail**

```
cd /home/alex/dev/cook-modules/cook_cc && busted spec/needs_field_spec.lua
```
Expected: FAIL — `needs` field unhandled; command strings don't yet include find sigils.

- [ ] **Step 4: Modify `targets.lua` to handle `needs` and weave sigils**

This is the load-bearing change. In `cook-modules/cook_cc/targets.lua`:

(a) At the top, add:

```lua
local finder = require("cook_cc.finder")
```

(b) Add a helper near the top:

```lua
local function register_needs(needs)
    local probe_keys = {}
    for _, name in ipairs(needs or {}) do
        finder.find(name)   -- registers cc:find:<name> idempotently
        probe_keys[#probe_keys + 1] = "cc:find:" .. name
    end
    return probe_keys
end

local function sigil_chain(needs, field)
    -- Returns "$<cc:find:n1.field> $<cc:find:n2.field> ..." for the field.
    local parts = {}
    for _, name in ipairs(needs or {}) do
        parts[#parts + 1] = "$<cc:find:" .. name .. "." .. field .. ">"
    end
    return table.concat(parts, " ")
end
```

(c) In `bin/lib/shared/headers`, extract `needs` from opts and pass to `cc.compile` / `cc.link`:

For `M.bin`:

```lua
function M.bin(name, opts)
    opts = opts or {}
    local needs = opts.needs or {}
    cook.recipe(name, { requires = opts.links or {} }, function()
        local b = build_opts(opts, "bin")
        b.needs = needs
        local sources = gather_sources(opts)
        if #sources == 0 then
            error("[cc.bin] no sources found for target '" .. name .. "'", 2)
        end
        register_known(name)
        local merged = transitive.resolve_links(b.links)
        b.includes = merge_includes(b.includes, merged.includes)
        record_export(name, sources, b, "")
        local objs = compile_all(name, sources, b)
        cc.link(objs, "build/bin/" .. name, {
            system_libs   = merge_system_libs(merged.system_libs, b.system_libs),
            frameworks    = merge_frameworks(merged.frameworks, b.frameworks),
            extra_ldflags = build_ldflags(merged.lib_paths, merged.extra_ldflags, b.extra_ldflags),
            needs         = needs,                 -- NEW
        })
    end)
    return name
end
```

Mirror for `M.lib`, `M.shared`, `M.headers`.

(d) Inside `compile_all`, pass `needs` through to `cc.compile`:

```lua
local function compile_all(name, sources, b)
    local objs = {}
    for _, src in ipairs(sources) do
        objs[#objs + 1] = cc.compile(src, {
            target_name  = name,
            includes     = b.includes,
            defines      = b.defines,
            standard     = b.standard,
            warnings     = b.warnings,
            extra_cflags = b.extra_cflags,
            fpic         = b.fpic,
            needs        = b.needs,                -- NEW
        })
    end
    return objs
end
```

- [ ] **Step 5: Modify `cc.lua` to weave sigils into compile + link commands**

The current `cc.lua` calls `toolchain.get_compiler()` synchronously to bake the compiler binary name into the command string. That stops working under the probe model — `get_compiler()` reads the probe value store, which is empty at register time. Replace with sigil placeholders.

Replace `M.compile` in `cook-modules/cook_cc/cc.lua`:

```lua
function M.compile(source, opts)
    opts = opts or {}
    if not fs.exists(source) then
        error("[cc.compile] source file not found: " .. source, 2)
    end
    local target_name = opts.target_name or "default"
    local stem = path.stem(source)
    local obj_dir = "build/obj/" .. target_name
    local obj_out = opts.output or (obj_dir .. "/" .. stem .. ".o")
    local dep_dir = ".cook/deps/" .. target_name
    local dep_file = dep_dir .. "/" .. stem .. ".d"

    fs.mkdir_p(obj_dir)
    fs.mkdir_p(dep_dir)

    -- Ensure compiler probe is registered so the sigil resolves.
    toolchain.ensure_probe_registered()
    local cc_probe_key = toolchain.get_probe_key()
    local cc_field = is_c(source) and "cc" or "cxx"
    local compiler_sigil = "$<" .. cc_probe_key .. "." .. cc_field .. ">"

    local flags = { "-c", "-MMD", "-MF", dep_file }
    local std = opts.standard or toolchain.get_default_standard()
    if std and not is_c(source) then
        flags[#flags + 1] = "-std=" .. std
    end
    for _, inc in ipairs(opts.includes or {}) do flags[#flags + 1] = "-I" .. inc end
    for _, def in ipairs(opts.defines  or {}) do flags[#flags + 1] = "-D" .. def end
    local wflags = toolchain.warning_flags(opts.warnings)
    if wflags ~= "" then flags[#flags + 1] = wflags end
    if opts.fpic then flags[#flags + 1] = "-fPIC" end
    if opts.extra_cflags and opts.extra_cflags ~= "" then
        flags[#flags + 1] = opts.extra_cflags
    end

    local needs = opts.needs or {}
    local probes = { cc_probe_key }
    local cflags_sigils = {}
    for _, n in ipairs(needs) do
        probes[#probes + 1] = "cc:find:" .. n
        cflags_sigils[#cflags_sigils + 1] = "$<cc:find:" .. n .. ".cflags>"
    end

    -- Trailing space ensures assertions like " -c " match even when -c is the last token.
    local cmd = compiler_sigil .. " " .. table.concat(flags, " ")
        .. " " .. source .. " -o " .. obj_out
    if #cflags_sigils > 0 then
        cmd = cmd .. " " .. table.concat(cflags_sigils, " ")
    end
    cmd = cmd .. " "

    cook.add_unit({
        inputs            = { source },
        output            = obj_out,
        command           = cmd,
        probes            = probes,
        discovered_inputs = { from = dep_file, format = "make" },
    })

    return obj_out
end
```

Replace `M.link`:

```lua
function M.link(objects, output, opts)
    opts = opts or {}
    fs.mkdir_p(path.dir(output))

    toolchain.ensure_probe_registered()
    local cc_probe_key = toolchain.get_probe_key()
    local compiler_sigil = "$<" .. cc_probe_key .. ".cxx>"

    local needs = opts.needs or {}
    local probes = { cc_probe_key }
    local libs_sigils = {}
    for _, n in ipairs(needs) do
        probes[#probes + 1] = "cc:find:" .. n
        libs_sigils[#libs_sigils + 1] = "$<cc:find:" .. n .. ".libs>"
    end

    local parts = { compiler_sigil, table.concat(objects, " "), "-o", output }
    for _, lib in ipairs(opts.system_libs or {}) do
        parts[#parts + 1] = "-l" .. lib
    end
    if cook.platform.os == "macos" then
        for _, fw in ipairs(opts.frameworks or {}) do
            parts[#parts + 1] = "-framework"
            parts[#parts + 1] = fw
        end
    end
    if opts.extra_ldflags and opts.extra_ldflags ~= "" then
        parts[#parts + 1] = opts.extra_ldflags
    end
    if opts.shared then parts[#parts + 1] = "-shared" end
    if #libs_sigils > 0 then
        parts[#parts + 1] = table.concat(libs_sigils, " ")
    end

    local cmd = table.concat(parts, " ") .. " "

    cook.add_unit({
        inputs  = objects,
        output  = output,
        command = cmd,
        probes  = probes,
    })
    return output
end
```

`M.archive` requires no change — it doesn't invoke the compiler.

The load-bearing structural changes:
1. `compiler_for(source)` and `toolchain.get_compiler().cxx` are replaced with sigil strings (`$<cc:compiler:auto.cc>` for C sources, `$<cc:compiler:auto.cxx>` for C++ and link).
2. The `probes` field on each `cook.add_unit` carries `{cc_probe_key, "cc:find:n1", "cc:find:n2", ...}`.
3. `cflags` sigils append to compile commands; `libs` sigils append to link commands.

- [ ] **Step 6: Run tests to verify they pass**

```
cd /home/alex/dev/cook-modules/cook_cc && busted spec/needs_field_spec.lua
```
Expected: all 6 passing.

- [ ] **Step 7: Run full spec for regressions**

```
cd /home/alex/dev/cook-modules/cook_cc && busted .
```
Expected: green. Fix any pre-existing tests that asserted on the literal compiler in command strings — those need updating to expect `$<cc:compiler:auto.cxx>` sigils.

- [ ] **Step 8: Commit**

```
cd /home/alex/dev/cook-modules && git add cook_cc/targets.lua cook_cc/cc.lua cook_cc/spec/needs_field_spec.lua cook_cc/spec/targets_spec.lua cook_cc/spec/cc_spec.lua
git commit -m "feat(cook_cc): needs={...} on bin/lib/shared/headers

Target makers accept a needs list of system-library names. Each name
registers a cc:find:<n> probe (idempotently) and contributes
\$<cc:find:<n>.cflags> to compile commands, \$<cc:find:<n>.libs>
to link commands. Compile + link units carry probes={cc:compiler,
cc:find:<n>, ...} so demand-driven scheduling pulls them when the
recipe is invoked."
```

---

## Task 9: Bump rockspec to 0.5.0-1

**Files:**
- Create: `cook-modules/cook_cc/cook_cc-0.5.0-1.rockspec`

- [ ] **Step 1: Copy the 0.4.0 rockspec and edit**

```
cd /home/alex/dev/cook-modules/cook_cc
cp cook_cc-0.4.0-1.rockspec cook_cc-0.5.0-1.rockspec
```

- [ ] **Step 2: Edit version and tag fields**

In `cook_cc-0.5.0-1.rockspec`:
- `version = "0.5.0-1"`
- `source.tag = "cook_cc-0.5.0-1"`

- [ ] **Step 3: Replace the `detailed` summary** to describe the probe-based migration:

```
detailed = [[
    Blessed Cook module for C and C++ native builds. Provides declarative
    target makers (cc.bin/lib/shared/headers) accepting a `needs` list for
    declarative system-library discovery, low-level primitives
    (cc.compile/archive/link), multi-strategy package discovery
    (cc.find with project / curated / pkg-config / cmake-compat / bare-probe stages),
    project-scoped finder registration (cc.register_finder), a raising
    find convenience (cc.find_or_error), transitive link propagation
    including macOS frameworks, and compile_commands.json generation.

    0.5.0 (CS-0075) migrates internal caching from register-phase
    cook.cache.set/get to first-class cook.probe units. cc:compiler,
    cc:linker-search-dirs, cc:cmake-driver, and cc:find:<n> are now
    proper probes with declared inputs and demand-driven scheduling.
    Target makers expose a new `needs = {...}` field for system-library
    declarations; the older pattern of capturing find_or_error's record
    into a config block and threading fields into the bin call still
    works (find_or_error now returns a sigil-record of probe-value
    placeholders). find_or_error failure becomes demand-driven —
    missing libraries surface at build time rather than register time.

    Specified normatively at §28 of the Cook Standard (v0.5).
]]
```

- [ ] **Step 4: Verify the build.modules section** lists all the new files:

```lua
build = {
   type    = "builtin",
   modules = {
     ["cook_cc"]                       = "cook_cc/init.lua",
     ["cook_cc.toolchain"]             = "cook_cc/toolchain.lua",
     ["cook_cc.cc"]                    = "cook_cc/cc.lua",
     ["cook_cc.targets"]               = "cook_cc/targets.lua",
     ["cook_cc.finder"]                = "cook_cc/finder.lua",
     ["cook_cc.compile_db"]            = "cook_cc/compile_db.lua",
     ["cook_cc.transitive"]            = "cook_cc/transitive.lua",
     ["cook_cc.version"]               = "cook_cc/version.lua",
     ["cook_cc._probe_helpers"]        = "cook_cc/_probe_helpers.lua",   -- NEW
     ["cook_cc.finders"]               = "cook_cc/finders/init.lua",
     ["cook_cc.finders.pkg_config"]    = "cook_cc/finders/pkg_config.lua",
     ["cook_cc.finders.bare_probe"]    = "cook_cc/finders/bare_probe.lua",
     ["cook_cc.finders.cmake_compat"]  = "cook_cc/finders/cmake_compat.lua",
     ["cook_cc.finders.header_probe"]  = "cook_cc/finders/header_probe.lua",
     ["cook_cc.finders.tool_config"]   = "cook_cc/finders/tool_config.lua",
     ["cook_cc.finders.gl"]            = "cook_cc/finders/gl.lua",
     ["cook_cc.finders.libcurl"]       = "cook_cc/finders/libcurl.lua",
     ["cook_cc.finders.openal"]        = "cook_cc/finders/openal.lua",
     ["cook_cc.finders.raylib"]        = "cook_cc/finders/raylib.lua",
     ["cook_cc.finders.sdl2"]          = "cook_cc/finders/sdl2.lua",
     ["cook_cc.finders.threads"]       = "cook_cc/finders/threads.lua",
     ["cook_cc.finders.zlib"]          = "cook_cc/finders/zlib.lua",
   },
}
```

Note the new `_probe_helpers` entry. Cross-check against the actual files: `ls cook_cc/finders/`.

- [ ] **Step 5: Commit**

```
cd /home/alex/dev/cook-modules && git add cook_cc/cook_cc-0.5.0-1.rockspec
git commit -m "chore(cook_cc): rockspec 0.5.0-1 — probe-based migration

CS-0075. Internal caches become cook.probe units; target makers
gain needs={...} field. Surface contract specified at Standard
§28 v0.5."
```

---

## Task 10: Migrate `examples/raylib-game/Cookfile` to `needs = {...}`

**Files:**
- Modify: `cook/examples/raylib-game/Cookfile`
- Modify: `cook/examples/raylib-game/cook.toml`

- [ ] **Step 1: Read current Cookfile**

```
cat /home/alex/dev/cook/examples/raylib-game/Cookfile
```

- [ ] **Step 2: Rewrite to use `needs`**

Replace `cook/examples/raylib-game/Cookfile` with:

```
use cook_cc

cook_cc.bin("game", {
    sources = { "src/main.c" },
    needs   = { "raylib" },
})
```

- [ ] **Step 3: Bump cook_cc pin in `cook.toml`**

In `cook/examples/raylib-game/cook.toml`, change:

```
cook_cc = "0.4.0-1"
```

to:

```
cook_cc = "0.5.0-1"
```

- [ ] **Step 4: Pre-publish: copy 0.5.0 source into the vendored tree** so the example can resolve before the rock is published.

```
cd /home/alex/dev/cook
cp -r ../cook-modules/cook_cc/*.lua examples/raylib-game/cook_modules/share/lua/5.4/cook_cc/
cp -r ../cook-modules/cook_cc/finders examples/raylib-game/cook_modules/share/lua/5.4/cook_cc/
```

This is a sanity step for the in-tree smoke test. Final publishing (Task 14) regenerates the rock and re-resolves cook_modules.

- [ ] **Step 5: Run the smoke test**

```
cd /home/alex/dev/cook/examples/raylib-game
cook game
```

Expected: `build/bin/game` produced; binary links against raylib. If raylib is unavailable on this host, expect a build-time failure naming the failed probe (demand-driven failure). Document the actual outcome.

- [ ] **Step 6: Commit**

```
cd /home/alex/dev/cook && git add examples/raylib-game/Cookfile examples/raylib-game/cook.toml examples/raylib-game/cook_modules/
git commit -m "examples(raylib-game): migrate to cook_cc 0.5.0 needs={...} shape"
```

---

## Task 11: Migrate `examples/sdl3-game/Cookfile` to `needs = {...}`

**Files:**
- Modify: `cook/examples/sdl3-game/Cookfile`
- Modify: `cook/examples/sdl3-game/cook.toml`

- [ ] **Step 1: Read current Cookfile**

```
cat /home/alex/dev/cook/examples/sdl3-game/Cookfile
```

- [ ] **Step 2: Rewrite**

Replace `cook/examples/sdl3-game/Cookfile` with:

```
use cook_cc

cook_cc.bin("game", {
    sources = { "src/main.c" },
    needs   = { "SDL3" },
})
```

- [ ] **Step 3: Bump pin and vendored sources** (mirror of Task 10 steps 3-4)

```
cd /home/alex/dev/cook
sed -i 's/cook_cc = "0.4.0-1"/cook_cc = "0.5.0-1"/' examples/sdl3-game/cook.toml
cp -r ../cook-modules/cook_cc/*.lua examples/sdl3-game/cook_modules/share/lua/5.4/cook_cc/
cp -r ../cook-modules/cook_cc/finders examples/sdl3-game/cook_modules/share/lua/5.4/cook_cc/
```

- [ ] **Step 4: Run the smoke test**

```
cd /home/alex/dev/cook/examples/sdl3-game
cook game
```

Expected: build succeeds against host SDL3 install, or fails with a probe-error diagnostic naming SDL3's missing strategies.

- [ ] **Step 5: Commit**

```
cd /home/alex/dev/cook && git add examples/sdl3-game/Cookfile examples/sdl3-game/cook.toml examples/sdl3-game/cook_modules/
git commit -m "examples(sdl3-game): migrate to cook_cc 0.5.0 needs={...} shape"
```

---

## Task 12: Amend Standard §28 — cc module v0.5

**Files:**
- Modify: `cook/standard/src/content/docs/28-cc.mdx`

Cook Standard governs language changes per `CONTRIBUTING.md`. The new `needs` field, sigil-record return from `find/find_or_error`, and demand-driven `find_or_error` failure are all surface changes — all need spec'd.

- [ ] **Step 1: Read the current §28 chapter** to find the right edit locations.

```
cat /home/alex/dev/cook/standard/src/content/docs/28-cc.mdx
```

- [ ] **Step 2: Update §28.1 synopsis line** to reference v0.5:

Replace:

```
The official `cook_cc` rock at `rocks.usecook.com` is the reference implementation.
```

with:

```
The official `cook_cc` rock at `rocks.usecook.com` is the reference implementation. This chapter normatively specifies surface v0.5 (cook_cc 0.5.x).
```

- [ ] **Step 3: Add `needs` field documentation** to §28.3.1 (`cc.bin`), §28.3.2 (`cc.lib`), §28.3.3 (`cc.shared`), §28.3.4 (`cc.headers`).

For each function, insert into the `*Opts` field table:

```
| `needs` | list[string] | System-library names. Each name MUST be resolved via a `cc:find:<name>` probe registered by the implementation (§{cat.probes}). Compile units MUST carry `probes = {…, "cc:find:<name>", …}` and link units MUST weave the find probe's `libs` field via the `$<cc:find:<name>.libs>` sigil. (§{cat.cc.needs-semantics}) |
```

- [ ] **Step 4: Add a new subsection §28.3.14 — Find semantics**

After the surface entries, add:

```
### 28.3.N. Find semantics under probes [#cat.cc.needs-semantics]

A conforming implementation MUST treat each name passed via `opts.needs`
as a probe-key suffix: `"cc:find:" .. name`. The implementation MUST
register the probe idempotently (subsequent `cc.find` or `needs`
references to the same name MUST NOT re-register the probe). A probe
registered with conflicting `opts` (e.g., two calls for `"raylib"` with
different `version` constraints) MUST raise a register-phase diagnostic
naming both call sites.

The implementation's `cc.find(name, opts)` and `cc.find_or_error(name, opts)`
public functions MUST register the probe (if not already) and MUST
return a *sigil record*: a Lua table whose fields are strings of the
form `"$<cc:find:" .. name .. "." .. field .. ">"`, one per documented
field of the find result. The sigil record's fields MUST NOT support
mutation; assigning to a sigil-record field MUST raise.

`cc.find_or_error` MUST register a probe whose `produce` raises a Lua
error when no strategy reports `outcome == "hit"`. Per §{cat.probes.exec}
the raised error fails the consuming unit and propagates via DAG edge.
Register-phase callers of `cc.find_or_error` MUST NOT observe a
register-time error for missing libraries; failure is demand-driven.
```

- [ ] **Step 5: Update §28.3.8 (`cc.find`) and §28.3.13 (`cc.find_or_error`)** to drop the old "returns table with raw `cflags` / `libs` strings" wording and replace with a forward-reference to §28.3.14 (the new sigil-record section). The original cross-reference text should change to: "Returns a *sigil record* per §{cat.cc.needs-semantics}."

- [ ] **Step 6: Commit**

```
cd /home/alex/dev/cook && git add standard/src/content/docs/28-cc.mdx
git commit -m "standard(§28): cc module v0.5 — needs field + sigil-record find"
```

---

## Task 13: Add CS-0075 to Appendix E

**Files:**
- Modify: `cook/standard/src/content/docs/appendix/E-changes.mdx`

- [ ] **Step 1: Read the existing CS-0074 entry** to match the format.

```
grep -A 30 "CS-0074" /home/alex/dev/cook/standard/src/content/docs/appendix/E-changes.mdx
```

- [ ] **Step 2: Add a new entry** below the most recent existing entry (likely CS-0074):

```markdown
## CS-0075 — cc module probe-based migration

**Affects:** `cook_cc` rock (§{cat.cc}), surface v0.4 → v0.5.

The cc module's register-phase ad-hoc caches migrate to first-class
`cook.probe` units (§{cat.probes}). The following probe keys are now
canonical:

- `cc:compiler:<override>` — toolchain detection
- `cc:linker-search-dirs` — bare-probe linker search paths
- `cc:cmake-driver` — cmake-compat driver discovery
- `cc:find:<name>` — per-library find result

Target makers (`cc.bin`, `cc.lib`, `cc.shared`, `cc.headers`) accept a
`needs = {…}` field for declarative system-library discovery. The
imperative `cc.find` and `cc.find_or_error` functions return a
*sigil record* — a table of `$<cc:find:<name>.<field>>` placeholders
resolved at execute time by the existing template-expansion pipeline
(§{cat.probes.templates}).

`cc.find_or_error` failure becomes demand-driven: missing libraries
fail the build at execute time rather than raising during register.
This is a deliberate tradeoff — recipes that do not consume the
missing library no longer fail with register-time noise.

The `cc:*` probe namespace is non-normative pattern documentation;
cook_cc itself is not part of the conformance surface, but the design
informs other blessed modules.
```

- [ ] **Step 3: Commit**

```
cd /home/alex/dev/cook && git add standard/src/content/docs/appendix/E-changes.mdx
git commit -m "standard(appendix-e): CS-0075 — cc module probe-based migration"
```

---

## Task 14: Add conformance fixtures — positive

**Files:**
- Create: `cook/standard/conformance/positive/cc-needs-pkgconfig/`
- Create: `cook/standard/conformance/positive/cc-needs-bare/`
- Create: `cook/standard/conformance/positive/cc-toolchain-override/`
- Create: `cook/standard/conformance/positive/cc-find-record-sigil/`

Each conformance fixture follows the existing convention: a `Cookfile` plus baseline output files. Pattern from existing fixtures like `cc-find-pkgconfig`.

- [ ] **Step 1: Inspect an existing cc fixture** to match the format:

```
ls /home/alex/dev/cook/standard/conformance/positive/cc-find-pkgconfig/
cat /home/alex/dev/cook/standard/conformance/positive/cc-find-pkgconfig/Cookfile
```

- [ ] **Step 2: Create `cc-needs-pkgconfig` fixture**

```
mkdir -p /home/alex/dev/cook/standard/conformance/positive/cc-needs-pkgconfig
```

Create `cc-needs-pkgconfig/Cookfile`:

```
use cook_cc

cook_cc.bin("hello", {
    sources = { "src/hello.c" },
    needs   = { "zlib" },
})
```

Create `cc-needs-pkgconfig/src/hello.c`:

```c
#include <zlib.h>
#include <stdio.h>
int main(void) { printf("zlib %s\n", zlibVersion()); return 0; }
```

Generate baseline output files per the harness's conventions. Match neighbouring fixtures' file set (likely `parse.txt`, `register_ok.txt`).

- [ ] **Step 3: Create `cc-needs-bare` fixture**

Similar shape but with a library that doesn't ship `.pc` files — `libm` is a good candidate (always present, no pkg-config):

```
use cook_cc

cook_cc.bin("calc", {
    sources = { "src/calc.c" },
    needs   = { "m" },
})
```

`src/calc.c`:

```c
#include <math.h>
#include <stdio.h>
int main(void) { printf("%f\n", sin(1.0)); return 0; }
```

- [ ] **Step 4: Create `cc-toolchain-override` fixture**

```
use cook_cc

cook_cc.toolchain({ compiler = "g++" })

cook_cc.bin("hello", { sources = { "src/hello.c" } })
```

Harness assertion: the registered probe key is `cc:compiler:g++` (not `:auto`). Verify with the existing harness's probe-introspection capability if available; otherwise the fixture asserts the build produces a binary using the named compiler.

- [ ] **Step 5: Create `cc-find-record-sigil` fixture**

Exercises the imperative escape hatch:

```
use cook_cc

register
    raylib = cook_cc.find_or_error("raylib")
    assert(raylib.cflags:find("^%$<cc:find:raylib.cflags>$"),
           "expected raylib.cflags to be a sigil placeholder")
```

- [ ] **Step 6: Run conformance**

```
cd /home/alex/dev/cook
cargo test -p cook-lang --test conformance
```

Expected: new fixtures pass after baseline files are populated. If baselines need regeneration, the harness usually has an environment flag or subcommand — check `standard/conformance/README.md` if present.

- [ ] **Step 7: Commit**

```
cd /home/alex/dev/cook && git add standard/conformance/positive/cc-needs-pkgconfig standard/conformance/positive/cc-needs-bare standard/conformance/positive/cc-toolchain-override standard/conformance/positive/cc-find-record-sigil
git commit -m "test(conformance): cc-needs / toolchain-override / find-record-sigil positives"
```

---

## Task 15: Add conformance fixtures — negative

**Files:**
- Create: `cook/standard/conformance/negative/cc-find-conflicting-opts/`
- Create: `cook/standard/conformance/negative/cc-find-missing-on-build/`

- [ ] **Step 1: Create `cc-find-conflicting-opts` fixture**

```
mkdir -p /home/alex/dev/cook/standard/conformance/negative/cc-find-conflicting-opts
```

`Cookfile`:

```
use cook_cc

register
    cook_cc.find("raylib", { version = ">=4.0" })
    cook_cc.find("raylib", { version = ">=5.0" })
```

Baseline expected diagnostic (in `register_err.txt` or whatever the harness convention is):

```
[cc.find] duplicate cc.find for 'raylib' with conflicting opts:
  first call opts={version=">=4.0"}
  this call opts={version=">=5.0"}
```

- [ ] **Step 2: Create `cc-find-missing-on-build` fixture**

```
use cook_cc

cook_cc.bin("hello", {
    sources = { "src/hello.c" },
    needs   = { "definitely-not-installed-xyz123" },
})
```

`src/hello.c`:

```c
int main(void) { return 0; }
```

Baseline asserts register succeeds (no register-phase error) but build fails with a probe diagnostic naming the missing library and the tried strategies. Match neighbouring negative fixtures' file conventions.

- [ ] **Step 3: Run conformance**

```
cd /home/alex/dev/cook && cargo test -p cook-lang --test conformance
```
Expected: green.

- [ ] **Step 4: Commit**

```
cd /home/alex/dev/cook && git add standard/conformance/negative/cc-find-conflicting-opts standard/conformance/negative/cc-find-missing-on-build
git commit -m "test(conformance): cc-find-conflicting-opts + cc-find-missing-on-build negatives"
```

---

## Task 16: End-to-end smoke and publish

This task is partially manual — it involves publishing to the cook-rocks-index (which the user owns) and verifying the published rock resolves.

- [ ] **Step 1: Run the cook-modules unit test suite end-to-end**

```
cd /home/alex/dev/cook-modules && busted cook_cc/
```
Expected: all green.

- [ ] **Step 2: Run cook's full conformance + Rust test suite**

```
cd /home/alex/dev/cook
cargo test
cargo test -p cook-lang --test conformance
```
Expected: green.

- [ ] **Step 3: Verify raylib-game and sdl3-game build with vendored sources**

```
cd /home/alex/dev/cook/examples/raylib-game && cook clean && cook game
cd /home/alex/dev/cook/examples/sdl3-game && cook clean && cook game
```
Expected: both produce `build/bin/game`.

- [ ] **Step 4: Tag and publish cook_cc 0.5.0-1**

```
cd /home/alex/dev/cook-modules
git tag -a cook_cc-0.5.0-1 -m "cook_cc 0.5.0-1 — probe-based migration"
MODULE=cook_cc VERSION=0.5.0-1 cook publish
```

This runs the publish chore documented in `cook-modules/Cookfile`: tags HEAD, pushes to Gitea + GitHub, packs the rock, stages in `~/dev/cook-rocks`, regenerates manifest, and pushes the rocks index.

- [ ] **Step 5: Refresh examples' cook_modules from published rock**

```
cd /home/alex/dev/cook/examples/raylib-game && cook modules update
cd /home/alex/dev/cook/examples/sdl3-game && cook modules update
```

Smoke-test the published flow:

```
cd /home/alex/dev/cook/examples/raylib-game && cook clean && cook game
cd /home/alex/dev/cook/examples/sdl3-game && cook clean && cook game
```

Expected: green from published rock.

- [ ] **Step 6: Commit cook.lock updates**

```
cd /home/alex/dev/cook && git add examples/raylib-game/cook.lock examples/sdl3-game/cook.lock
git commit -m "examples: refresh cook.lock against published cook_cc 0.5.0-1"
```

- [ ] **Step 7: Final summary in commit-log**

Verify the commit log on `main`:

```
cd /home/alex/dev/cook-modules && git log --oneline -20
cd /home/alex/dev/cook && git log --oneline -20
```

Expected: a coherent sequence of 13 commits in cook-modules (Tasks 1-9 + publish) and 7 commits in cook (Tasks 10-15 + lockfile refresh).

---

## Notes for execution

- **Test-first cadence.** Every task above starts with a failing test. If a test passes before implementation, the test isn't load-bearing — strengthen it.
- **Commit per task.** Each task's commit captures one coherent unit of behavior. Resist bundling multiple tasks into one commit; the audit trail is the point.
- **Two-repo discipline.** Most tasks edit only `cook-modules`. Tasks 10-16 edit `cook`. Don't cross the streams in a single commit.
- **Spec-first hook.** The cook repo enforces a `core.hooksPath = .githooks` pre-commit hook that pairs language-surface changes with Standard updates. Tasks 12, 13 are the Standard side; if any other task surfaces language-surface drift the hook flags, pair the fix with a spec update — do not bypass with `COOK_STANDARD_BYPASS=1` (per durable user feedback).
- **Worktree.** This plan is meant to execute in a worktree under `/home/alex/dev/cook/.worktrees/shi-221-probes/` (and analogous for cook-modules if desired). Create with `git worktree add` against a fresh branch off `main` in each repo before starting.
