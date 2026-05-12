# SHI-135 — M2: CMake-compat — consume `XxxConfig.cmake` (design)

**Ticket:** [SHI-135](https://linear.app/shiny-guru/issue/SHI-135) — parent [SHI-132](https://linear.app/shiny-guru/issue/SHI-132) (Cook builds Doom 3).
**Date:** 2026-05-12.
**Status:** design — supersedes the M2 section of `2026-05-01-cpp-module-roadmap-to-doom3-design.md` for SHI-135 execution.
**Predecessor:** M1 ([SHI-134](https://linear.app/shiny-guru/issue/SHI-134), Done 2026-05-12). `cook_cc-0.2.0-1` is live on rocks.usecook.com and Cook Standard §9.2 v0.2 specifies the four-stage `cc.find` chain.

## 1. Decisions recap

| # | Question | Decision |
|---|---|---|
| Q0 | `--find-package` legacy mode marked "Do not use" in cmake 4.x — proceed? | Yes. The annotation is years-old with no removal; the cmake project carries it for nudging, not deprecation timeline. Shipping on legacy mode is the lowest-cost path that satisfies M2 exit criteria. Fallback path (temp `CMakeLists.txt` + `message(STATUS …)`) is a swap of `cmake_compat.lua` internals, not a redesign. |
| Q1 | Position in chain? | Always-on, **after pkg-config**, before bare-probe. `FindOpts.cmake = true` lifts cmake-compat to right after curated (bypasses pkg-config). Roadmap doc's "after curated, before pkg-config" framing predates the cmake 4.x finding. |
| Q2 | Cache semantics? | Per-invocation in-memory only (existing `cook.cache`). Disk persistence filed as a generic follow-up that benefits every strategy. |
| Q3 | Output parsing robustness? | Simple case only. Imported-target chains (LINK output containing paths to other `*Config.cmake` / `*Targets.cmake`) produce `outcome="miss"` with a documented reason and `cc.register_finder` hint. No M2.1 work. |
| Q4 | Version constraints? | Punt. `opts.version` set ⇒ cmake-compat returns `outcome="skip"` with `reason="version detection unsupported by legacy cmake --find-package"`. Chain falls through. |
| Q5 | Integration example? | `examples/sdl3-game/` parallel to `examples/raylib-game/`. |
| Q6 | Diagnostics? | New `Attempt.strategy = "cmake-compat"`. Five reason categories with package-specific hints from a small `hints.lua` table; generic fallback otherwise. |
| Q7 | Conformance mode? | Route A — busted spec coverage + parse-only Cookfile fixtures. Execute-mode harness still deferred to [SHI-210](https://linear.app/shiny-guru/issue/SHI-210). |

## 2. Scope

**In scope:**

1. Add a fifth strategy stage `cmake-compat` to `cc.find`, between pkg-config and bare-probe.
2. `FindOpts.cmake = true` opt-in knob that lifts cmake-compat to immediately after curated (and bypasses pkg-config).
3. `cook_cc/finders/cmake_compat.lua` (subprocess wrapper, three-phase probe, LINK-output splitter, hint emission).
4. `cook_cc/finders/cmake_compat/hints.lua` — install-hint catalogue for the known M2 long-tail (SDL3, glfw3, …).
5. New in-tree example `examples/sdl3-game/` exercising `cc.find_or_error("SDL3")` on Linux + macOS.
6. Standard §9.2 v0.2 → v0.3 (additive surface, resolver-structural).

**Out of scope:**

- Building libraries from source via cmake (M3+ for vendoring patterns; cmake as a build executor never).
- Parsing `XxxConfig.cmake` directly (the native-parser alternative; filed as a separate ticket with an evidence-based open bar — see §11).
- Windows-host cmake search paths (post-Doom-3 sub-roadmap, consistent with M0/M1).
- Imported-target chain resolution (M2 detects and rejects; no recovery beyond the `register_finder` hint).
- Disk-persistent `cook.cache` (separate ticket; benefits every strategy, not just cmake-compat).
- Execute-mode conformance harness ([SHI-210](https://linear.app/shiny-guru/issue/SHI-210)).

**Shipped artifacts:**

- `cook_cc 0.3.0-1` on rocks.usecook.com.
- Cook Standard §9.2 v0.3 touching §9.2.3.8, §9.2.5; App. B rationale; App. D `CS-0068`.
- `examples/sdl3-game/`.
- Parse-only conformance fixtures under `standard/conformance/positive/cc-find-cmake-*`.
- Behavioral test coverage in `cook-modules/cook_cc/spec/finders/cmake_compat_spec.lua`.

## 3. Standard surface delta (v0.3)

### 3.1 §9.2.3.8 `cc.find` — v0.3 revision

The §9.2.3.8 numbered chain grows a fifth entry between pkg-config and bare-probe, and `FindOpts` grows a `cmake` boolean. Insertions only — existing rows unchanged.

`FindOpts` (additions):

| Field | Type | Semantics |
|---|---|---|
| `cmake` | boolean | When `true`, the cmake-compat strategy MUST be consulted immediately after curated finders, before pkg-config, and pkg-config MUST be skipped for this call. The default `false` places cmake-compat after pkg-config in the standard order. |

A conforming v0.3 implementation MUST consult strategies in this order, first-match-wins:

1. **Project-registered finders** (per §9.2.3.12).
2. **Curated finders** — as v0.2.
3. **pkg-config** — as v0.1. MUST be skipped when `opts.cmake = true`.
4. **cmake-compat** — described in §9.2.3.8.1. When `opts.cmake = true`, consulted in step 3's slot instead.
5. **Bare-lib probe** — as v0.2.

`Attempt.strategy` catalog grows the entry `"cmake-compat"`. All other `Attempt` fields retain v0.2 semantics.

### 3.2 §9.2.3.8.1 cmake-compat strategy (new sub-section)

A conforming cmake-compat strategy MUST:

1. If `opts.version` is set, return `{ outcome = "skip", reason = "version detection unsupported by legacy cmake --find-package" }` without invoking any subprocess. cmake-compat does not surface version metadata in M2; the chain falls through to bare-probe (itself a no-op on `opts.version`) and finally to `cc.find` returning `found = false`.
2. Locate a `cmake` driver on PATH. If absent, return `{ outcome = "skip", reason = "cmake binary not on PATH", hint = "install cmake: apt: cmake / brew: cmake / dnf: cmake" }`.
3. Probe `EXIST` mode for the queried package using `cmake --find-package -DNAME=<name> -DCOMPILER_ID=GNU -DLANGUAGE=C -DMODE=EXIST -DQUIET=TRUE` (or implementation-equivalent invocation):
   - If `EXIST` reports "not found" (cmake exit ≠ 0), return `{ outcome = "miss", reason = "cmake found no Config or Find module for '<name>'", hint = <package-specific or generic> }`.
4. Probe `COMPILE` mode for cflags; probe `LINK` mode for libs.
5. Parse `COMPILE` output as a shell-token stream per §9.2.3.8.2 (reuses the v0.2 pkg-config token grammar; emits `include_dirs`, `define`-style tokens, etc.).
6. Parse `LINK` output per §9.2.3.8.3 (cmake-compat-specific; LINK mode emits absolute paths interleaved with `-framework`, `-l`, `-L` tokens).
7. If any token in the parsed LINK output is a filesystem path whose final component matches `*Config.cmake` or `*Targets.cmake`, return `{ outcome = "miss", reason = "imported-target chain too complex for legacy --find-package", hint = "register a project finder for '<name>' via cc.register_finder" }`.
8. On a clean hit, return `{ outcome = "hit", strategy = "cmake-compat", payload = { … } }` with `payload.version = nil`.

Names passed to cmake-compat are case-sensitive and forwarded unchanged. `cc.find("SDL3")` probes `SDL3Config.cmake`; `cc.find("sdl3")` probes `sdl3-config.cmake`. The case convention follows cmake's own resolver behaviour.

Caching: same as v0.2 — `canonical_opts` includes `cmake=true` when set, so opt-in and standard-order lookups occupy distinct cache slots and never collide.

### 3.3 §9.2.3.8.2 COMPILE-mode output grammar

`cmake --find-package … -DMODE=COMPILE` emits a shell-token stream containing `-I<dir>`, `-D<macro>`, and (rarely) `-isystem <dir>` tokens, whitespace-separated, with a trailing newline. The implementation MUST reuse the v0.2 token grammar from the pkg-config strategy: `-I` ⇒ `include_dirs`, `-D` ⇒ `define`-class token (collected in `cflags`), other tokens preserved verbatim in `cflags`.

### 3.4 §9.2.3.8.3 LINK-mode output grammar

`cmake --find-package … -DMODE=LINK` emits a shell-token stream whose elements MUST be classified as:

| Token shape | Classification | Field |
|---|---|---|
| `-framework <name>` (two tokens) | macOS framework | `frameworks` |
| `-l<name>` | system library | `system_libs`, also appended to `libs` |
| `-L<dir>` | library search path | `lib_dirs`, also appended to `libs` |
| `*.so`, `*.so.<version>`, `*.dylib`, `*.a`, `*.lib` (any token containing `/` or `\` and ending in one of these suffixes) | absolute path to a library file | `libs` (verbatim) |
| `*Config.cmake`, `*Targets.cmake` | imported-target chain marker | triggers `outcome = "miss"` per §9.2.3.8.1 step 6 |
| any other non-empty token | recorded in `libs` verbatim; not propagated to typed fields | `libs` |

Absolute paths emitted to `libs` MUST be passed to the linker as-is; the linker accepts absolute paths and resolves them without `-L` search.

### 3.5 §9.2.5 — error model catalogue additions

Append to the v0.2 catalogue:

| Function | Condition | Message |
|---|---|---|
| `cc.find` (cmake-compat) | cmake binary not on PATH | (returned as `Attempt`, not raised) `cmake binary not on PATH` |
| `cc.find` (cmake-compat) | EXIST mode reports not found | (returned as `Attempt`) `cmake found no Config or Find module for '<name>'` |
| `cc.find` (cmake-compat) | LINK output contains `*Config.cmake` / `*Targets.cmake` | (returned as `Attempt`) `imported-target chain too complex for legacy --find-package` |

`cc.find` continues to NOT raise; misses surface through `cc.find_or_error`'s aggregate message per §9.2.3.13.

### 3.6 App. B rationale — `## Why cmake-compat ships on legacy --find-package` (new)

Drop-in paragraph explaining: cmake 4.x marks `--find-package` mode "Do not use"; cmake-compat ships on it anyway because (a) the annotation is a nudge, not a removal timeline; (b) reimplementing the same Config-resolution logic natively is the post-M6 "full CMake interpretation" north star and explicitly deferred; (c) the migration path if cmake ever removes legacy mode is to swap `cmake_compat.lua` internals to a temp-project + `message(STATUS …)` approach with no spec impact. The trust boundary is documented: anything found by cmake-compat is found by cmake's own resolver, so accuracy is delegated; the only Cook-implemented part is output parsing.

### 3.7 App. B rationale — `## Why cmake-compat sits after pkg-config in the default chain`

Drop-in paragraph: pkg-config is ~50× faster than the cmake subprocess; pkg-config has not been deprecated by its upstream; pkg-config is the conventional discovery format for Cook's curated short list. Placing cmake-compat after pkg-config means projects whose dependencies all ship `.pc` files never pay the cmake cost. The `opts.cmake = true` opt-in flips the order for callers who know their dependency is Config-only (e.g., SDL3) and want to skip the wasted pkg-config probe.

### 3.8 App. B rationale — `## Why cc.find v0.2 documented a four-stage chain`

Preservation paragraph: the v0.2 wording (released in `cook_cc 0.2.x`) specified a four-stage chain (project → curated → pkg-config → bare-probe). v0.3 (M2, this revision) inserts cmake-compat between pkg-config and bare-probe and adds the `cmake` opt; every v0.2 call site remains a valid v0.3 call site. Bisects against `cook_cc 0.2.x` reference this paragraph.

### 3.9 App. D changelog

`CS-0068: cc cmake-compat strategy (M2)` — captioning the §9.2.3.8 v0.3 revision (chain growth + `FindOpts.cmake`), §9.2.3.8.1 sub-section introduction, §9.2.3.8.2 / §9.2.3.8.3 token-grammar normative text, §9.2.5 catalogue growth.

## 4. Resolver internals

### 4.1 Chain assembly delta

`finder.lua`'s `M.find` is extended to support the `opts.cmake` lift. Diff against M1:

```lua
function M.find(name, opts)
    opts = opts or {}
    local cache_key = "cc.find:" .. name .. ":" .. canonical_opts(opts)
    local cached = cook.cache.get(cache_key)
    if cached then return cached end

    local pkg   = require("cook_cc.finders.pkg_config")
    local bare  = require("cook_cc.finders.bare_probe")
    local cmake = require("cook_cc.finders.cmake_compat")

    local chain
    if opts.cmake then
        chain = {
            function() return project_strategy(name, opts) end,
            function() return curated_strategy(name, opts) end,
            function() return cmake.main_chain(name, opts) end,
            function() return bare.main_chain(name, opts) end,
        }
    else
        chain = {
            function() return project_strategy(name, opts) end,
            function() return curated_strategy(name, opts) end,
            function() return pkg.main_chain(name, opts) end,
            function() return cmake.main_chain(name, opts) end,
            function() return bare.main_chain(name, opts) end,
        }
    end

    local tried, hit = {}, nil
    for _, step in ipairs(chain) do
        local attempt = step()
        tried[#tried + 1] = attempt
        if attempt.outcome == "hit" then hit = attempt; break end
    end

    local result = build_result(hit, tried)
    cook.cache.set(cache_key, result)
    return result
end
```

`canonical_opts` already serializes boolean opts deterministically (`tostring(true)` → `"true"`); no helper changes required.

### 4.2 `cmake_compat.lua` shape

```lua
local M = {}

local function detect_cmake()
    -- cached on first call via cook.cache key "cc.cmake-compat:driver"
    -- value: { ok = true/false, path = "/usr/bin/cmake", legacy_supported = true/false }
end

local function probe_exist(name) end       -- bool + reason
local function probe_compile(name) end     -- string (stdout)
local function probe_link(name) end        -- string (stdout)
local function parse_compile(s) end        -- reuses cook_cc.finders.pkg_config token parser
local function parse_link(s) end           -- new splitter per §9.2.3.8.3

function M.main_chain(name, opts)
    if opts.version then
        return { strategy = "cmake-compat", outcome = "skip",
                 reason = "version detection unsupported by legacy cmake --find-package" }
    end
    local driver = detect_cmake()
    if not driver.ok then
        return { strategy = "cmake-compat", outcome = "skip",
                 reason = "cmake binary not on PATH",
                 hint = "install cmake: apt: cmake / brew: cmake / dnf: cmake" }
    end
    if not driver.legacy_supported then
        return { strategy = "cmake-compat", outcome = "skip",
                 reason = "this cmake build does not support --find-package legacy mode" }
    end

    local found, exist_reason = probe_exist(name)
    if not found then
        local hints = require("cook_cc.finders.cmake_compat.hints")
        return { strategy = "cmake-compat", outcome = "miss",
                 reason = "cmake found no Config or Find module for '" .. name .. "'",
                 hint = hints.for_package(name) }
    end

    local compile_out, compile_err = probe_compile(name)
    local link_out,    link_err    = probe_link(name)
    if compile_err or link_err then
        return { strategy = "cmake-compat", outcome = "miss",
                 reason = "cmake --find-package returned a non-zero exit in COMPILE or LINK mode" }
    end

    local link_tokens = parse_link(link_out)
    for _, tok in ipairs(link_tokens) do
        if tok.kind == "config-file-ref" then
            return { strategy = "cmake-compat", outcome = "miss",
                     reason = "imported-target chain too complex for legacy --find-package",
                     hint = "register a project finder for '" .. name .. "' via cc.register_finder" }
        end
    end

    local payload = build_payload(parse_compile(compile_out), link_tokens, compile_out, link_out)
    return { strategy = "cmake-compat", outcome = "hit", reason = "", payload = payload }
end

return M
```

### 4.3 `cmake` driver detection

Cached in `cook.cache` under `"cc.cmake-compat:driver"`. The cache key intentionally has no version/path component because the answer is "is `cmake` resolvable on this PATH right now"; the first `cc.find(..., {cmake=true})` (or first chain pass past stage 4) per invocation pays the detection cost.

Detection algorithm:

1. `cook.sh("command -v cmake")` (or platform-equivalent); empty/error ⇒ `{ ok = false }`.
2. `cook.sh("cmake --find-package -DNAME=ZLIB -DCOMPILER_ID=GNU -DLANGUAGE=C -DMODE=EXIST -DQUIET=TRUE 2>&1 || true")` (probe of a sentinel name).
   - Reaches "ZLIB found." / "ZLIB not found." ⇒ legacy mode supported. (Either outcome confirms `--find-package` mode itself is honoured; the package name choice does not matter.)
   - Reaches "Unknown option" / "argument" parse errors ⇒ `{ ok = true, legacy_supported = false }`.

A future cmake that hard-removes `--find-package` would trip the `legacy_supported = false` branch and surface a clean "this cmake build does not support --find-package legacy mode" message; the M2.1 swap to a temp-project approach replaces the body of `cmake_compat.lua` without changing the resolver surface.

### 4.4 Subprocess invocation shape

All three probes (`EXIST`, `COMPILE`, `LINK`) share the same flag base:

```
cmake --find-package
    -DNAME=<name>
    -DCOMPILER_ID=GNU              # GNU == clang-compatible; cmake accepts both clang and gcc under GNU
    -DLANGUAGE=<C|CXX>             # selected based on caller; default C
    -DMODE=<EXIST|COMPILE|LINK>
    -DQUIET=TRUE
```

`COMPILER_ID` is fixed at `GNU` for M2; cmake's legacy mode resolves Config files identically regardless. `LANGUAGE` defaults to `C` but is overridable via `opts.language` (future expansion; not in M2 surface — internal only).

stderr is captured but discarded on success; on failure the first non-empty stderr line is folded into the `Attempt.reason` (truncated at 120 chars to keep diagnostic output bounded).

### 4.5 LINK-mode token classifier

```lua
local function classify_link_token(tok)
    if tok == "-framework" then return "framework-marker" end  -- followed by name
    if tok:sub(1, 2) == "-l" then return "syslib" end
    if tok:sub(1, 2) == "-L" then return "libdir" end
    if tok:match("[/\\][^/\\]*Config%.cmake$") then return "config-file-ref" end
    if tok:match("[/\\][^/\\]*Targets%.cmake$") then return "config-file-ref" end
    if tok:match("%.so$") or tok:match("%.so%.%d") then return "abs-lib" end
    if tok:match("%.dylib$") then return "abs-lib" end
    if tok:match("%.a$") or tok:match("%.lib$") then return "abs-lib" end
    return "other"
end
```

The two-token `-framework <name>` pair is reassembled by a state-machine pass over the token sequence. Other parser passes are stateless.

### 4.6 Hints table

`cook_cc/finders/cmake_compat/hints.lua`:

```lua
local HINTS = {
    SDL3   = "apt: libsdl3-dev / brew: sdl3 / dnf: SDL3-devel",
    glfw3  = "apt: libglfw3-dev / brew: glfw / dnf: glfw-devel",
    Vulkan = "apt: libvulkan-dev / brew: vulkan-headers / dnf: vulkan-devel",
    fmt    = "apt: libfmt-dev / brew: fmt / dnf: fmt-devel",
    nlohmann_json = "apt: nlohmann-json3-dev / brew: nlohmann-json / dnf: json-devel",
}

local M = {}
function M.for_package(name)
    return HINTS[name] or ("check 'cmake --find-package -DNAME=" .. name
        .. " -DCOMPILER_ID=GNU -DLANGUAGE=C -DMODE=EXIST'; install the upstream package or register a project finder")
end
return M
```

Catalogue is intentionally short. Adding entries does not require a rock release — projects can override the generic message by registering a project finder that emits its own `Attempt.hint`.

### 4.7 Cache key extension

Reusing v0.2's `canonical_opts`: `cc.find("SDL3", { cmake = true })` canonicalizes to `cmake=true` and produces cache key `cc.find:SDL3:cmake=true`. `cc.find("SDL3")` (default order) keys to `cc.find:SDL3:`. The two never collide.

## 5. `cmake --find-package` output handling (worked examples)

Validated against `cmake 4.3.2` on Linux. Each example annotates how the LINK-mode splitter buckets tokens.

### 5.1 ZLIB

```
$ cmake --find-package -DNAME=ZLIB -DCOMPILER_ID=GNU -DLANGUAGE=C -DMODE=COMPILE
-I/usr/include
$ cmake --find-package -DNAME=ZLIB -DCOMPILER_ID=GNU -DLANGUAGE=C -DMODE=LINK
/usr/lib/libz.so
```

Bucketed payload:

```lua
{
    cflags       = "-I/usr/include",
    libs         = "/usr/lib/libz.so",
    include_dirs = {"/usr/include"},
    lib_dirs     = {},
    system_libs  = {},
    frameworks   = {},
    version      = nil,
}
```

### 5.2 SDL2

```
COMPILE: -I/usr/include -I/usr/include/SDL2
LINK:    /usr/lib/libSDL2main.a /usr/lib/libSDL2-2.0.so.0.3200.68
```

Bucketed:

```lua
{
    cflags       = "-I/usr/include -I/usr/include/SDL2",
    libs         = "/usr/lib/libSDL2main.a /usr/lib/libSDL2-2.0.so.0.3200.68",
    include_dirs = {"/usr/include", "/usr/include/SDL2"},
    -- ...
}
```

### 5.3 SDL3 (M2's canonical target — Debian Trixie / brew sdl3)

```
COMPILE: -I/usr/include/SDL3
LINK:    /usr/lib/x86_64-linux-gnu/libSDL3.so
```

Same bucketing shape as ZLIB.

### 5.4 Imported-target chain rejection (synthetic)

If a Config file's `INTERFACE_LINK_LIBRARIES` resolves through other Config files, LINK mode emits paths to those Config files:

```
LINK: /usr/lib/libFoo.so /usr/lib/cmake/Bar/BarConfig.cmake
```

The `BarConfig.cmake` token trips §9.2.3.8.3's `config-file-ref` classifier, returning the miss-with-hint described in §3.5.

### 5.5 Miss path

```
$ cmake --find-package -DNAME=DoesNotExist -DCOMPILER_ID=GNU -DLANGUAGE=C -DMODE=EXIST -DQUIET=TRUE
DoesNotExist not found.
$ echo $?
1
```

Trapped at step 2 of §9.2.3.8.1; `COMPILE`/`LINK` probes are skipped.

## 6. Frameworks plumbing (delta from M1)

No spec-level change to §9.2.4 transitive propagation. cmake-compat populates the same `payload.frameworks` field M1 introduced; downstream propagation is unchanged.

In practice, the macOS LINK output for Config files that depend on Apple frameworks looks like `-framework Foundation -framework AppKit /opt/homebrew/lib/libFoo.dylib`. The two-token `-framework <name>` state machine in §4.5 emits them into `payload.frameworks` symmetric to the pkg-config strategy's existing behaviour.

## 7. Example: `examples/sdl3-game/`

### 7.1 Layout

```
examples/sdl3-game/
  cook.toml              pins cook_cc 0.3.0-1, indexes=[rocks.usecook.com]
  cook.lock              from `cook modules install`
  cook_modules/          populated by install
  Cookfile               cc.find_or_error("SDL3") + cc.bin
  src/main.c             minimal SDL3 demo (open window, clear, present, quit-on-escape)
  README.md              install hint per OS + Trixie/Bookworm note
```

### 7.2 `Cookfile` (target shape)

```cook
use cook_cc

recipe game
    > local sdl3 = cook_cc.find_or_error("SDL3")
    > cook_cc.bin("game", {
    >     sources       = { "src/main.c" },
    >     includes      = sdl3.include_dirs,
    >     system_libs   = sdl3.system_libs,
    >     extra_ldflags = sdl3.libs,
    >     frameworks    = sdl3.frameworks,
    > })
```

Field mapping for cmake-compat hits:

| FindResult field | BinOpts field | Linker effect |
|---|---|---|
| `include_dirs` (list) | `includes` (list) | `-I<dir>` per entry |
| `system_libs` (list) | `system_libs` (list) | `-l<name>` per entry; usually empty for cmake-compat |
| `libs` (raw string) | `extra_ldflags` (raw string) | appended verbatim — carries absolute `.so`/`.dylib`/`.a` paths from cmake LINK mode |
| `frameworks` (list) | `frameworks` (list) | `-framework <name>` per entry on macOS |

`extra_ldflags` is the documented raw-link-flag channel (§9.2.3.7); absolute library paths emitted by cmake-compat are valid raw link tokens, so no Standard-level surface change is required to plumb them through.

### 7.3 `src/main.c` (sketch — final source under SDL's zlib license, attributed)

~50 lines. `SDL_Init(SDL_INIT_VIDEO)`, create a window via `SDL_CreateWindow`, get a renderer, render-loop with `SDL_PollEvent` until `SDL_QUIT` or escape key, `SDL_Quit`. Identical scope to raylib-game's `core_basic_window`.

### 7.4 Platform availability

| OS | Install | Source |
|---|---|---|
| Debian Trixie (testing) | `apt install libsdl3-dev` | distro packages |
| Debian Bookworm | not packaged | source build, deferred (`README.md` documents the gap) |
| Ubuntu 24.04 LTS | not packaged | source build, deferred |
| Fedora 41 | `dnf install SDL3-devel` | distro packages |
| macOS | `brew install sdl3` | homebrew |
| Arch / CachyOS | `pacman -S sdl3` | distro packages |

Gate-m2 matrix CI pins runners that have SDL3 packaged: `ubuntu-24.04` runner with a `apt install libsdl3-dev` step is the canonical CI target. (Note: as of writing, SDL3 is in Trixie but not Ubuntu 24.04; the runner image step may need a backport repo or a Trixie-based container. Resolved during M2 implementation when CI plumbing lands.)

## 8. Conformance and test plan

### 8.1 Three layers (same as M1)

| Layer | Where | What |
|---|---|---|
| Behavioral | `cook-modules/cook_cc/spec/finders/cmake_compat_spec.lua` | Token classifier + chain wiring + hint emission |
| Surface | `cook/standard/conformance/positive/cc-find-cmake-*` | Public surface parses + binds |
| Integration | `cook/examples/sdl3-game/` | End-to-end build on Linux + macOS |

### 8.2 Busted spec additions

New file `cook-modules/cook_cc/spec/finders/cmake_compat_spec.lua` (~30 tests, ~250 lines):

- Chain position when `opts.cmake = false/nil` (after pkg-config, before bare-probe)
- Chain position when `opts.cmake = true` (after curated, pkg-config skipped)
- `cook.sh` mock for cmake missing → `outcome = "skip"` with install hint
- `cook.sh` mock for `EXIST` miss → `outcome = "miss"` with package-specific hint when in catalogue, generic otherwise
- COMPILE-mode token bucketing (reuses M1 pkg-config grammar tests as templates)
- LINK-mode token classifier exhaustive cases (the seven shape classes from §4.5)
- Imported-target chain detection — both `*Config.cmake` and `*Targets.cmake` suffixes
- `opts.version` set → `outcome = "skip"`
- Cache key separation between `{cmake=true}` and default
- Driver detection caching (only one PATH probe per invocation)

### 8.3 Parse-only Cookfile fixtures

Three new fixtures under `standard/conformance/positive/`:

| Fixture | Surface |
|---|---|
| `cc-find-cmake-compat` | §9.2.3.8 cmake-compat strategy slot |
| `cc-find-cmake-opt-in` | §9.2.3.8 `FindOpts.cmake = true` |
| `cc-find-cmake-version-skip` | §9.2.3.8.1 step 7 — skip on `opts.version` |

Each fixture binds the surface without exercising the subprocess (parse-only Route A). Existing `cc-find-miss-behaviour` from M0 + `cc-find-tried-field` from M1 continue to apply unchanged.

### 8.4 Integration via gate-m2

```yaml
- name: install SDL3 (linux)
  if: matrix.os == 'ubuntu-24.04'
  run: |
    # ... runner-specific provisioning per §7.4 note
    sudo apt install -y libsdl3-dev
- name: install SDL3 (macos)
  if: matrix.os == 'macos-14'
  run: brew install sdl3
- name: build sdl3-game
  run: cd examples/sdl3-game && cook game
```

The runner-image SDL3 availability note in §7.4 is the only operational complication; specifics resolved at CI-plumbing time during implementation. If Trixie-based provisioning slips, the M2 gate accepts macOS-only as a partial signal (sdl3-game still gates Linux as `expected-fail` until provisioning lands) — better than blocking M2 entirely on Ubuntu image lag.

### 8.5 Deferred follow-up

Execute-mode harness ([SHI-210](https://linear.app/shiny-guru/issue/SHI-210)). M2 strengthens the case (subprocess shape leaves even less observable in parse-only fixtures than M1) but the harness work is the whole cc-* fixture corpus, not just M2.

## 9. Release plan

### 9.1 Sequence (mirrors M1)

```
1. PR cook#A  (draft)             §9.2 v0.3 spec text + three parse-only conformance
                                  fixtures + App. B rationale paragraphs + App. D CS-0068.

2. PR cook-modules#B              cmake_compat.lua + hints.lua + chain wiring delta in
                                  finder.lua + new busted suite + rockspec 0.3.0-1.

3. Tag + publish                  cook-modules tag cook_cc-0.3.0-1.
                                  cook-rocks-index PR renders the new rock.
                                  Cloudflare Pages serves within minutes.

4. PR cook#A flips draft → ready  Add commits: pin lua-build, fzf-picker, cpp-project,
                                  raylib-game to 0.3.0-1 (regen cook.lock);
                                  add examples/sdl3-game/.

5. PR cook#A merge                Pre-merge: gate-m2 matrix green on Linux + macOS;
                                  conformance suite green; busted green;
                                  standard.build asserts no new slug regressions.

6. SHI-135 closeout comment       Mirror M0 / M1 closeout shape.
```

### 9.2 Pre-merge gates

| Gate | Verifies |
|---|---|
| `cargo test -p cook-lang --test conformance` | Parse-only cc-find-cmake-* fixtures green |
| `busted` in `cook-modules/cook_cc/` | Behavioral spec (~150 tests total post-M2) |
| `cook game` in `examples/sdl3-game/` | End-to-end on Linux + macOS via gate-m2 |
| `standard.build` (Astro) | No new slug regressions (pre-existing E-pre-v1-checklist warning continues to track as [SHI-212](https://linear.app/shiny-guru/issue/SHI-212)) |

### 9.3 Versioning

`cook_cc 0.2.x → 0.3.0`. Standard §9.2 v0.2 → v0.3. The change is additive at the call-site level (every v0.2 call is a valid v0.3 call) but resolver-structural (chain length changes; new opt). 0.x semver allows the minor bump; consistent with M1's 0.1.x → 0.2.0 pattern.

## 10. Risks

| Risk | Mitigation |
|---|---|
| `cmake --find-package` legacy mode hard-removed in some future cmake | M2.1 internal swap to temp-project + `message(STATUS …)` extraction; no spec change. §4.3 driver detection already surfaces a clean error when the mode is unavailable. |
| Subprocess cost 100–500ms per query | In-VM `cook.cache` memoization (same as M1); first call per `(name, opts)` pays once; later calls free. Disk-persistent cache filed as separate ticket. |
| LINK-mode output contains a Config-file reference more obscure than `*Config.cmake`/`*Targets.cmake` (e.g., direct `*Macros.cmake`, `*Helpers.cmake`) | Step 6 regexes only match the canonical Config/Targets file naming. False negatives (we accept a chain we shouldn't have) surface as link-time errors at the consumer's `cc.link` — actionable. False positives in either direction are filed for follow-up if observed. |
| `cmake --find-package` may emit shell metacharacters or paths with spaces | LPEG tokenizer treats whitespace as a separator with no quote handling; pathological paths with embedded whitespace would be misclassified. Documented as a known limitation; mitigation is to register a project finder. |
| Trixie / Ubuntu 24.04 runner image SDL3 packaging gap | §7.4 acknowledges; macOS-only acceptance on partial-fail is documented; resolution during CI-plumbing implementation. |
| Case-sensitivity gotcha (`cc.find("sdl3")` vs `cc.find("SDL3")`) | Documented in §3.2 and `examples/sdl3-game/README.md`. The miss hint quotes the exact `NAME=` passed to cmake, so the diagnostic is self-correcting. |
| Imported-target chain rejection alienates users on packages we *could* have handled | The "register a project finder" hint is actionable; project finders take ~20 lines of Lua for typical cases. Native parser (option B) filed as a follow-up with explicit evidence bar. |

## 11. Out of scope, tracked elsewhere

| Item | Ticket |
|---|---|
| Native `XxxConfig.cmake` parser (option B) | NEW — file at M2 spec-merge time with the bar "open if cmake-compat subprocess cost or imported-target rejections become a real friction point in observed Cook usage" |
| Disk-persistent `cook.cache` | NEW — generic improvement benefiting every strategy, not just cmake-compat |
| Execute-mode conformance harness | [SHI-210](https://linear.app/shiny-guru/issue/SHI-210) |
| Vendor + build raylib from source | [SHI-136](https://linear.app/shiny-guru/issue/SHI-136) (M3) |
| Windows-host cmake search paths | post-Doom-3 sub-roadmap |
| `E-pre-v1-checklist.mdx` slug warning | [SHI-212](https://linear.app/shiny-guru/issue/SHI-212) |
| Full CMake interpretation | post-M6 north star, not committed |
