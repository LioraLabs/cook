# Probe-based `cook_cc`: find, toolchain, and checks

**Date:** 2026-05-17
**Status:** Approved (brainstorm)
**Scope:** SHI-221 (migrate cook_cc to `cook.probe`) and SHI-136 (M3 — `cpp.checks` + `cpp.config_header`) as one design, two implementation plans
**Standard target:** 0.12
**Module target:** `cook_cc` 0.4.0

## 1. Motivation

CS-0074 (probe units + demand-driven scheduling) shipped on 2026-05-16. The probe machinery is proven by three runnable demos and a conformance corpus. But `cook_cc` — the most-used blessed module — still caches via `cook.cache.set` / `cook.cache.get` from register-phase. Six call sites across `bare_probe.lua`, `toolchain.lua`, `finder.lua`, `compile_db.lua`, `targets.lua`, and `cmake_compat.lua` use the legacy pattern.

Migrating these ad-hoc caches to first-class probes is foundational for two reasons:

1. **Demand-driven invalidation.** Today the linker-search-dirs probe and the compiler-detection probe run unconditionally on every register pass via `toolchain.rehydrate()`. Under probes, they run only when reached from a scheduled consumer, and their fingerprints fold into consumers' fingerprints.
2. **M3 needs the same substrate.** `cpp.checks` (per SHI-136) is literally a probe pattern — compile a small probe.c, return a value, cache by `(check_kind, name, compiler-fp)`. Designing M3 without first establishing the probe pattern in cook_cc would mean inventing the pattern twice.

This design covers both tickets as one architecture; implementation lands as two sequenced plans.

## 2. The reshape (Option C — names-only API)

The blessed module pattern becomes declarative. Users specify dependencies by name; the module orchestrates probe registration and unit wiring transparently.

```lua
use cook_cc

cook_cc.toolchain({ compiler = "clang++", standard = "c++17" })   -- optional
cook_cc.defaults({ extra_cflags = "-O2" })                         -- optional

cook_cc.bin("game", {
    sources = { "src/main.c" },
    needs   = { "raylib" },        -- system libs (resolved via cc.find probes)
    links   = { "mygame_lib" },    -- local targets (resolved via cook.import)
})
```

Two distinct dependency-kind fields:

- **`needs`** — list of system-library names. Each name is resolved at execute time via a `cc:find:<name>` probe whose `produce` runs the strategy chain (project → curated → pkg-config → cmake-compat → bare-probe) and returns a record `{ cflags, libs, system_libs, include_dirs, lib_dirs, frameworks, version }`. Idempotent across multiple calls — the same `needs = {"raylib"}` from several targets registers one shared probe.
- **`links`** — list of local target names. Resolved at register time via `cook.import` (unchanged from today). Local-target transitive propagation is unchanged.

This split is the new public contract. Today's `system_libs = raylib.system_libs` pattern — where users manually compose flag lists — disappears from the canonical surface. The imperative `cook_cc.find` / `cook_cc.find_or_error` functions remain as a documented escape hatch (see §5).

## 3. Probe registry

Five probe-shaped facilities emerge. Each entry below specifies the canonical key, declared inputs, and produce-time return value.

### 3.1. `cc:compiler`

| Field | Value |
|---|---|
| Key | `cc:compiler:<override-or-auto>` |
| Inputs | `tools = {"g++", "clang++", "cc"}` |
| Produces | `{ cxx: string, cc: string }` |

The `<override-or-auto>` suffix encodes the user's toolchain choice. `cook_cc.toolchain({compiler="clang++"})` selects the key `cc:compiler:clang++`; no explicit override selects `cc:compiler:auto`. Different config blocks naturally produce different probe keys; no engine env-contribution mechanism is required.

The `produce` body:

```lua
produce = [[
    local override = "<OVERRIDE>"   -- baked at probe-registration time
    if override ~= "auto" then
        if not cook.sh("command -v " .. override .. " 2>/dev/null"):match("%S") then
            error("[cc.toolchain] override compiler '" .. override .. "' not on PATH")
        end
        local cc = override:match("clang") and "clang" or (override:match("g%+%+") and "gcc" or "cc")
        return { cxx = override, cc = cc }
    end
    for _, c in ipairs({{cxx="g++",cc="gcc"}, {cxx="clang++",cc="clang"}}) do
        if cook.sh("command -v " .. c.cxx .. " 2>/dev/null"):match("%S") then return c end
    end
    error("[cc.toolchain] no C/C++ compiler on PATH (tried g++, clang++)")
]]
```

### 3.2. `cc:linker-search-dirs`

| Field | Value |
|---|---|
| Key | `cc:linker-search-dirs` |
| Inputs | `requires = {"cc:compiler:<override-or-auto>"}`, `env = {"LIBRARY_PATH"}` |
| Produces | `string[]` |

Resolves `cc -print-search-dirs` against the active compiler; falls back to `/usr/lib`, `/usr/local/lib` on platforms where the directive is unsupported.

### 3.3. `cc:cmake-driver`

| Field | Value |
|---|---|
| Key | `cc:cmake-driver` |
| Inputs | `tools = {"cmake"}`, `env = {"CMAKE_PREFIX_PATH"}` |
| Produces | `{ binary: string, version: string }` or `nil` |

Detects the cmake driver used by the cmake-compat finder. Returns `nil` if cmake is not on PATH; consumers (the cmake-compat strategy) interpret `nil` as "this strategy is unavailable, skip."

### 3.4. `cc:find:<name>`

| Field | Value |
|---|---|
| Key | `cc:find:<name>` (no opts in the key — see below) |
| Inputs | `requires = {"cc:compiler:<o>", "cc:linker-search-dirs", "cc:cmake-driver"}`, `env = {"PATH", "PKG_CONFIG_PATH", "CMAKE_PREFIX_PATH", "LIBRARY_PATH"}`, `tools = {"pkg-config"}` |
| Produces | `{ found: bool, cflags, libs, system_libs, include_dirs, lib_dirs, frameworks, version, tried }` |

`name` is the system-library name (`raylib`, `openal`, `SDL3`, etc.). `cook_cc.bin({needs = {"raylib"}})` registers (or reuses) the `cc:find:raylib` probe — repeated `needs` across targets dedup naturally.

**Opts handling.** Today `cc.find` accepts `opts = {version=..., cmake=true, ...}`. Embedding opts in the probe key (`cc:find:raylib:v>=4.0`) is technically clean but explodes the key namespace. The design choice: **opts are baked into the probe's `produce` body at registration time**, not the key. If the same name is used with different opts in the same register pass, the *first* registration wins; cook_cc raises a register-phase diagnostic if a later call passes incompatible opts.

```
duplicate cc.find for 'raylib' with conflicting opts:
  first call at Cookfile:12 opts={version=">=4.0"}
  this call at Cookfile:25 opts={version=">=5.0"}
```

Rationale: a single Cookfile asking for raylib `>=4.0` in one target and `>=5.0` in another is almost certainly a bug. If it isn't, the user can disambiguate by aliasing — register a project finder under a different name.

**The `produce` body** composes the strategy chain inline:

```lua
produce = [[
    local NAME = "<NAME>"
    local OPTS = <OPTS_LITERAL>   -- emitted as Lua table literal at registration time
    local tried = {}
    local function try(step) local r = step(); tried[#tried+1] = r; return r end

    local strategies = {
        function() return project_strategy(NAME, OPTS) end,
        function() return curated_strategy(NAME, OPTS) end,
    }
    if not OPTS.cmake then
        strategies[#strategies+1] = function() return pkg_strategy(NAME, OPTS) end
    end
    strategies[#strategies+1] = function() return cmake_strategy(NAME, OPTS) end
    strategies[#strategies+1] = function() return bare_strategy(NAME, OPTS) end

    for _, step in ipairs(strategies) do
        local r = try(step)
        if r.outcome == "hit" then return build_result(r, tried) end
    end
    return build_result(nil, tried)   -- {found=false, tried=tried}
]]
```

The strategy helpers (`project_strategy`, etc.) are vendored alongside the probe `produce` as a shared Lua chunk — the registration glue concatenates the helper definitions with the per-probe `produce` body. The plan picks the exact concatenation mechanism (heredoc string + name/opts interpolation, or a single shared Lua module the worker VM requires).

**`find_or_error` semantics.** Under the new model, `cook_cc.find_or_error(name, opts)` registers the probe and, if `r.found == false` at execute time, raises a Lua error from the probe `produce`. That fails the consuming unit and propagates via DAG edge. The register-time-early-warning behavior of today's `find_or_error` is lost — missing-lib failure is demand-driven. This is a deliberate tradeoff: you don't get told a library is missing until you actually try to build something that needs it.

### 3.5. `cc:check:<kind>:<name>:<short-fp>`

| Field | Value |
|---|---|
| Key | `cc:check:<kind>:<name>:<short-fp>` |
| Inputs | `requires = {"cc:compiler:<o>"}`, `env = {"PATH"}`, `tools = {<resolved-compiler>}` |
| Produces | `boolean` / `integer` / `string` depending on `<kind>` |

`<kind>` ∈ `{has-header, has-function, has-define, sizeof, endian, has-compile-flag, has-link-flag}`. `<short-fp>` is a short (8-hex-char) hash of `(standard, extra_cflags, extra_ldflags, defines, includes)` — the subset of compiler invocation that meaningfully changes check outcomes. Keys stay readable; collisions cap at the cache-implementation level (a fingerprint collision invalidates correctly because the full inputs are still hashed).

`produce` writes a tiny probe.c, compiles (and for some kinds, links) with the active compiler, returns the boolean/integer/endian value. See §6 for per-kind probe.c shapes.

## 4. Module-local state (not probes)

Two of SHI-221's six call sites aren't actually probe-shaped:

- **`targets.lua` — `known_targets`.** A register-time list of "targets declared so far in this Cookfile." Belongs in a module-local Lua table (`local M._known = M._known or {}`). It's accumulator state, not an environment fact.
- **`compile_db.lua` — reads `known_targets`** at the same register pass. Becomes a read of the module-local table. Verified by spot-check: existing examples (`examples/lua-build/Cookfile:42`, `examples/fzf-picker/Cookfile:32`) call `cook_cc.compile_commands()` via `>>` (inline-lua, register-phase), so the read happens before the register pass ends.
- **`finder.lua` — per-call canonical-opts memoization.** Mooted once `cc.find` IS a probe; the probe key + fingerprint provide cross-invocation caching. Delete the in-process memo.

The migration drops `cook.cache.set`/`cook.cache.get` from cook_cc entirely.

## 5. The imperative escape hatch

`cook_cc.find(name, opts)` and `cook_cc.find_or_error(name, opts)` remain in the public surface as a documented advanced-use API:

- They register the `cc:find:<name>` probe (idempotent — no-op if already registered with matching opts; raises on conflict per §3.4).
- They return a **sigil record**: a Lua table whose fields are sigil strings.
  ```lua
  local r = cook_cc.find("raylib")
  -- r.cflags       == "$<cc:find:raylib.cflags>"
  -- r.libs         == "$<cc:find:raylib.libs>"
  -- r.system_libs  == "$<cc:find:raylib.system_libs>"
  -- r.include_dirs == "$<cc:find:raylib.include_dirs>"
  -- r.found        == "$<cc:find:raylib.found>"          (resolves to "true"/"false" at execute)
  ```
- The user can pass these sigil strings into any field that accepts a command-template-style string. Direct mutation of the record raises (`__newindex` errors).
- Register-time conditional logic on probe values (`if r.found then ...`) is forbidden by construction — every field is a non-nil sigil string. Documented as a constraint with the recommended pattern (use `needs = {...}` instead).

This escape hatch keeps two-line scripts and advanced patterns viable without forcing every user through the `needs` plumbing.

## 6. M3: `cpp.checks` and `cpp.config_header`

### 6.1. The check functions

Each function in `cook_cc.checks.*` is a thin wrapper that:

1. Hashes the relevant opts into a short fingerprint.
2. Registers (or reuses) probe `cc:check:<kind>:<name>:<short-fp>` per §3.5.
3. Returns the sigil string `"$<cc:check:<kind>:<name>:<short-fp>>"`.

Function signatures:

| Function | `<kind>` | Probe returns | Typical opts |
|---|---|---|---|
| `cook_cc.checks.has_header(name, opts?)` | `has-header` | `boolean` | `includes`, `defines` |
| `cook_cc.checks.has_function(name, opts?)` | `has-function` | `boolean` | `includes`, `system_libs` |
| `cook_cc.checks.has_define(name, opts?)` | `has-define` | `boolean` | `includes` |
| `cook_cc.checks.sizeof(type, opts?)` | `sizeof` | `integer` | `includes` |
| `cook_cc.checks.endian()` | `endian` | `"little"` \| `"big"` | none |
| `cook_cc.checks.has_compile_flag(flag)` | `has-compile-flag` | `boolean` | none |
| `cook_cc.checks.has_link_flag(flag)` | `has-link-flag` | `boolean` | none |

Each probe's `produce` body composes a small probe.c, invokes the compiler (and, for has-function / has-link-flag, the linker), and decides the result from exit code or compiled-output inspection.

For `sizeof`, the standard trick — compile with a static_assert that picks the right size — produces a determinate integer return. For `endian`, the probe compiles a tiny C program that inspects byte order at compile time via a constant expression (no `try_run`, per SHI-136's out-of-scope).

### 6.2. `cpp.config_header`

```lua
cook_cc.config_header("config.h.in", "include/config.h", {
    HAVE_STDINT_H = cook_cc.checks.has_header("stdint.h"),
    HAVE_STRDUP   = cook_cc.checks.has_function("strdup"),
    VERSION       = "1.0",
})
```

`config_header(template, output, vars)` is a register-phase function that:

1. Scans `vars` values for sigil placeholders (`"$<cc:check:..."`); collects the set as the probe keys this unit depends on.
2. Registers a `cook.add_unit`:
   - `name`: `<template-basename>.gen`
   - `inputs`: `{template}`
   - `outputs`: `{output}`
   - `probes`: the collected probe-key set
   - `command`: invokes a vendored renderer script (mechanism per §12).
3. The vendored `cook_cc_config_header.lua` script reads its args, deserializes the vars table, performs `@VAR@` substitution and `#cmakedefine`/`#cmakedefine01` macro processing per the CMake `configure_file` contract, and writes `output`. The serialized-vars payload contains the resolved probe values (the sigil expansion happened before the command was invoked) plus literal string values.

The vendored Lua script lives at `standard/cook_modules/cook_cc/cook_cc_config_header.lua`. The plan picks the script-locator mechanism: an existing `$<COOK_MODULE_DIR.cook_cc>` sigil if Cook already exposes one, a register-time absolute-path interpolation, or a thin shell wrapper that knows where cook_cc's module dir lives.

**Implementation hook for sigil expansion of table-valued args.** The `'<serialized-vars>'` argument is a Lua table literal embedded in the command string; sigil placeholders inside that literal need to expand to their resolved values when the command runs. The existing sigil pipeline (`cook-luagen/src/template.rs`) expands `$<key.field>` placeholders that appear in command strings as plain text — so a literal like `"{ HAVE_X = $<cc:check:has-header:x.h:abc>, VERSION = '1.0' }"` becomes `"{ HAVE_X = true, VERSION = '1.0' }"` at execute time. The plan verifies this works for table-shaped serializations and falls back to a side-file (write the resolved vars to `.cook/probe-vars-<hash>.lua` at expansion time, read from the renderer script) if not.

### 6.3. `@VAR@` and `#cmakedefine` semantics

The renderer implements CMake's `configure_file` substitution rules:

- `@VAR@` → value of `vars[VAR]`, stringified. Missing key → empty string with a diagnostic.
- `${VAR}` → identical to `@VAR@`.
- `#cmakedefine SYMBOL VALUE` → if `vars[SYMBOL]` is truthy: `#define SYMBOL VALUE` (with `${VAR}` substitution in VALUE); else: `/* #undef SYMBOL */`.
- `#cmakedefine01 SYMBOL` → `#define SYMBOL 1` if truthy else `#define SYMBOL 0`.

Lua boolean semantics determine truthiness: `false` and `nil` are false; everything else (including `0` and `""`) is true. This matches CMake's quirks for `#cmakedefine` but is documented explicitly.

## 7. Standard amendments

### 7.1. §9.2 — `cook_cc` public surface, v0.3 → v0.4

- Add `needs` field to `bin/lib/shared` options. Document the semantic split between `needs` (system libs, resolved via probes) and `links` (local targets, resolved via `cook.import`).
- Add `cook_cc.checks.*` namespace with the seven functions above; specify return value as a probe-sigil string and the probe-key shape.
- Add `cook_cc.config_header(template, output, vars)` signature and semantics; document the `@VAR@` / `#cmakedefine` rules.
- Update `cook_cc.find` / `cook_cc.find_or_error` documentation to specify they return a sigil record under v0.4. Note the constraint on register-time conditional logic.
- Note that `find_or_error` failure is now demand-driven (raises during execute, not register).

### 7.2. §22.5.x — probe convention note

Add a non-normative paragraph documenting the pattern: "a register-phase API that returns a string of the form `$<key.field>` is a probe handle; consumers use the sigil verbatim in command templates." This codifies the convention so subsequent modules adopt the same pattern.

### 7.3. Appendix E — CS-0075

Mint CS-0075 covering the cook_cc API shape changes. Body summarises:

- `cook_cc` 0.4: new `needs` field on target makers, `cook_cc.checks.*` namespace, `cook_cc.config_header`, sigil-record return from `find` / `find_or_error`.
- The `cc:*` probe namespace is non-normative pattern documentation; cook_cc itself is not part of the conformance surface, but the design pattern informs other blessed modules.

## 8. Examples

### 8.1. Migrated examples (Phase 1)

`examples/raylib-game/Cookfile` and `examples/sdl3-game/Cookfile`:

```lua
use cook_cc

cook_cc.bin("game", {
    sources = { "src/main.c" },
    needs   = { "raylib" },           -- was: system_libs = raylib.system_libs
})
```

Two-line `Cookfile`. The previous `config` block with `raylib = cook_cc.find_or_error("raylib")` is gone — the find probe is auto-registered by `needs`.

### 8.2. New example (Phase 2 — M3)

`examples/raylib-game/` upgraded to vendor + build raylib from source. Cookfile sketch:

```lua
use cook_cc

cook_cc.toolchain({ standard = "c99" })

local raylib_config = cook_cc.config_header(
    "raylib-src/src/raylib.h.in",
    "build/raylib-src/raylib.h",
    {
        HAVE_STDINT_H = cook_cc.checks.has_header("stdint.h"),
        HAVE_STRDUP   = cook_cc.checks.has_function("strdup"),
        VERSION       = "5.0",
        BUILD_LIBTYPE_STATIC = true,
    })

cook_cc.lib("raylib", {
    sources  = { "raylib-src/src/rcore.c", "raylib-src/src/rshapes.c", ... },
    includes = { "build/raylib-src", "raylib-src/src" },
    -- depends on raylib_config existing; needs no probes itself
})

cook_cc.bin("game", {
    sources = { "src/main.c" },
    links   = { "raylib" },           -- local target
})
```

## 9. Conformance corpus

Under `standard/conformance/`.

### 9.1. Phase 1 positive fixtures

- `cook-cc-needs-pkgconfig` — `cook_cc.bin({needs={"x"}})` where `x` is resolvable via pkg-config; assert probe registration, command-template expansion, build success.
- `cook-cc-needs-bare` — same but resolved via bare-probe strategy.
- `cook-cc-toolchain-override` — `cook_cc.toolchain({compiler="g++"})` selects `cc:compiler:g++` key; default invocation selects `cc:compiler:auto`; assert distinct cache entries.
- `cook-cc-find-record-sigil` — `cook_cc.find("x")` returns a record whose fields stringify to `$<cc:find:x.…>` sigils.

### 9.2. Phase 1 negative fixtures

- `cook-cc-find-conflicting-opts` — two `cook_cc.find` calls for the same name with conflicting opts raise the §3.4 diagnostic.
- `cook-cc-find-missing-on-build` — `find_or_error` for a missing library doesn't fail register, but fails the build with a probe error referencing all tried strategies.

### 9.3. Phase 2 positive fixtures

- `cook-cc-check-has-header` — `cook_cc.checks.has_header("stdint.h")` returns true; key shape and fingerprint stability verified.
- `cook-cc-check-sizeof` — `cook_cc.checks.sizeof("long")` returns the correct integer for the host.
- `cook-cc-check-endian` — `cook_cc.checks.endian()` returns "little" or "big" matching the host.
- `cook-cc-config-header-basic` — `@VAR@` substitution and `#cmakedefine` processing produce the expected output file.
- `cook-cc-config-header-cmakedefine01` — `#cmakedefine01` produces `#define X 1` or `#define X 0`.

### 9.4. Phase 2 negative fixtures

- `cook-cc-check-bad-flag` — `has_compile_flag` on a syntactically invalid flag returns false (compiler rejects); diagnostic is informative.
- `cook-cc-config-header-missing-var` — referencing an undefined var emits a diagnostic and substitutes empty.

## 10. Implementation phases

### 10.1. Phase 1 — SHI-221 (migration)

Sequenced steps, each its own commit:

1. **Add probe-helper Lua module** (`cook_cc/_probe_helpers.lua` or similar) containing the strategy-chain helpers (`project_strategy`, `curated_strategy`, `pkg_strategy`, `cmake_strategy`, `bare_strategy`, `build_result`) extracted from today's finder modules. These will be `require`d by probe `produce` bodies on the worker VM.
2. **Migrate system probes** — replace `cook.cache.set/get` in `toolchain.lua`, `finders/bare_probe.lua`, `finders/cmake_compat.lua` with `cook.probe` registrations at module init. Add `cc:compiler:<override>`, `cc:linker-search-dirs`, `cc:cmake-driver`.
3. **Drop register-time accumulators from cache** — `known_targets` moves to module-local state (`targets.lua`); `compile_db.lua` reads the module-local table. Delete the per-call memoization in `finder.lua`.
4. **Refactor `cc.find` to a probe** — `cook_cc.find(name, opts)` becomes a registration helper that registers `cc:find:<name>` (idempotent + conflict-checked) and returns the sigil record.
5. **Reshape `bin/lib/shared/headers`** — accept `needs = {...}`, register the find probes idempotently, wire `probes = {"cc:find:<n>", ...}` into all per-source compile units and the link unit, weave `$<cc:find:<n>.cflags>` etc. into command templates.
6. **Update transitive merge logic** — `merge_includes` etc. in `targets.lua` retain their role for `cook.import` data; the find-probe sigil data appends to command-template strings without dedup. Document the no-dedup-across-finds tradeoff.
7. **Migrate examples** — `examples/raylib-game/Cookfile` and `examples/sdl3-game/Cookfile` to the `needs = {...}` shape.
8. **Standard amendments** — §9.2 v0.3 → v0.4 entries, CS-0075 in Appendix E.
9. **Conformance fixtures** — positive and negative per §9.1, §9.2.
10. **Bump `cook_cc` to 0.5.0** and publish to `cook-rocks-index` (0.4.x appears already taken by intermediate builds in some vendored examples; 0.5.0 leaves headroom).

### 10.2. Phase 2 — SHI-136 (M3)

Builds on Phase 1's substrate:

1. **Add `cook_cc.checks.*` namespace** — seven functions, each registering a `cc:check:<kind>:<name>:<short-fp>` probe.
2. **Vendor `cook_cc_config_header.lua` renderer** under `standard/cook_modules/cook_cc/`.
3. **Add `cook_cc.config_header(template, output, vars)`** registering a `cook.add_unit` with auto-detected `probes` and a command invoking the renderer.
4. **Upgrade `examples/raylib-game/`** to vendor and build raylib from source — configure → generate `raylib.h` → compile.
5. **Standard amendments** — `checks.*` and `config_header` documented in §9.2.
6. **Conformance fixtures** — per §9.3, §9.4.
7. **Bump `cook_cc` to 0.6.0** and publish.

## 11. What does not change

- CS-0074 probe machinery: API surface, fingerprint algorithm, msgpack walker, sigil pipeline, demand-driven scheduling.
- `cook.export` / `cook.import` mechanism for local-target transitive propagation. The `links` field still flows through this; `needs` is additive.
- `cook_cc.compile`, `cook_cc.archive`, `cook_cc.link` low-level API surface — internals updated to weave sigils into command templates, but external signatures unchanged.
- Recipe and unit shapes per Standard §22 / §6.
- The transitive merge logic (`merge_includes`, `merge_system_libs`, `merge_frameworks`, `build_ldflags`) for `cook.import` data — unchanged. The find-probe sigil data is concatenated into command strings; no register-time dedup across finds, but the cost is duplicate flags in the link line which the linker tolerates.

## 12. Open questions deferred to implementation plans

- **Probe-helper module location.** Vendored under `standard/cook_modules/cook_cc/` so the worker VM can `require` it during probe execution. Confirm the worker VM's `package.path` includes the right prefix; if not, an engine-side adjustment is needed.
- **Sigil expansion into table-shaped command args.** The `config_header` design assumes `$<cc:check:...>` sigils inside a Lua-table-literal command argument expand correctly. If the existing pipeline only expands sigils in flat string contexts, the fallback is: emit the vars table as a sequence of `KEY=$<cc:check:...:...>` shell-arg assignments (one arg per var), and the renderer script reconstructs the table from `arg[]`. Or write resolved vars to a side-file at expansion time. Plan picks.

- **Renderer-script locator.** The renderer script's invocation path must be resolvable at unit-execute time. Per §6.2, the plan picks between `$<COOK_MODULE_DIR.cook_cc>`-style sigils (if Cook exposes one), register-time absolute-path interpolation, or a shell wrapper.
- **Endian probe mechanism.** A compile-time-only endian detection without `try_run` requires a careful probe.c — typical pattern uses a multi-character `int` literal or a union with an init expression. Plan validates portability across gcc/clang/clang-cl-equivalent compilers.

## 13. Out of scope

- Cross-compile-aware probing (deferred from SHI-136).
- `try_run`-style runtime probes (deferred from SHI-136).
- Windows-specific paths in `cc:find` strategies (Windows is post-Doom-3-checkpoint).
- M4/M5/M6 cpp features (per-platform sources, build configurations, PCH). Separate milestones.
- Migrating `cpp.lua` (the older module) to probes. The newer `cook_cc` is what gets the treatment; `cpp.lua` is either retired or migrated separately.
