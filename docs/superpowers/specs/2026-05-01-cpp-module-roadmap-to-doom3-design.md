# cpp Module Roadmap to Doom 3

## Goal

A milestone-by-milestone plan for growing Cook's `cpp` module to the point where Cook can build a Doom 3 fork (likely dhewm3). The near-term waypoint is a **raylib-based C++ project** — first as a consumer of installed raylib, then as a vendored-from-source build. The long-term north star is **full CMake interpretation** (parse and execute `CMakeLists.txt` directly), held as aspirational but not committed.

This spec is a *roadmap*. M0–M6 are detailed at design-spec quality, each ready to spawn its own implementation plan. Doom 3 itself is treated as a **capability checklist + sub-roadmap deferral** — the post-M6 milestones get their own roadmap document, written after M6 lands and informed by what we learned.

## Non-goals

- No commitment to CMake interpretation as part of this roadmap. Held as a long-term direction, not a planned milestone.
- No community-module registry. Separate vision; out of scope here.
- No Windows-specific work in raylib milestones (M0–M6). Windows surfaces in the Doom-3 sub-roadmap if/when needed.
- No commitment to a specific Doom 3 fork yet. Choice deferred to the sub-roadmap.

## Decisions (from brainstorming)

| Decision | Outcome |
|---|---|
| Near-term waypoint | raylib: consume installed → build from source |
| Discovery approach | Cook-native finders + CMake-compat shim; full CMake interpretation = aspirational only |
| Module distribution | Blessed `standard/cook_modules/cpp.lua`; vendoring is a documented escape hatch |
| Roadmap shape | Raylib milestones detailed; Doom 3 destination is a checklist + deferred sub-roadmap |
| Per-platform sources (M4) | User-Lua, no structured-table API |
| Configure-step results (M3) | Returned as Lua values; no hidden global state |
| Build configurations (M5) | Use existing `config NAME ... end` blocks; cpp module exposes setters and partitions outputs |

## Current state (May 2026)

The `cpp.lua` module exists in three vendored copies under `examples/`:

- `examples/cpp-project/cook_modules/cpp.lua` (813 lines, lagging on the QoL surface)
- `examples/lua-build/cook_modules/cpp.lua` (989 lines, current)
- `examples/fzf-picker/cook_modules/cpp.lua` (989 lines, current)

The current surface (post-QoL):
- `cpp.bin` / `cpp.lib` / `cpp.shared` / `cpp.headers` (executables, static libs, shared libs, header-only libs)
- `cpp.compile` / `cpp.archive` / `cpp.link` (lower-level building blocks)
- `cpp.find(name)` — pkg-config-only finder
- `cpp.defaults({...})` — module-wide defaults, additive-merge
- `cpp.toolchain({...})` — compiler / standard / warnings selection
- Transitive propagation of `includes`, `defines`, `system_libs`, `extra_ldflags` via `cook.export` / `cook.import`
- C++20 modules support (Clang only)
- `cpp.compile_commands()` with write-if-changed
- Header dependency tracking via `-MMD` depfile parsing

There is no blessed module. There is no specified surface in the Standard. Each new feature has to be re-vendored across examples; drift has already happened.

## Roadmap

### M0 — Promote `cpp.lua` to a blessed Standard module

**Why now.** Every subsequent milestone is an *addition to* the blessed module. There is no blessed module today — three drifted vendored copies in `examples/`. Calling out the relocation as M0 makes the structural cost visible instead of pretending it is free.

**Scope.**
- Move the QoL-version `cpp.lua` (currently in `examples/lua-build/cook_modules/`) to `standard/cook_modules/cpp.lua`, alongside `checks.lua`.
- Lift the lagging `examples/cpp-project/cook_modules/cpp.lua` (813 lines) onto the QoL surface in passing.
- Spec the module surface in the Standard at RFC-2119 quality (functions, options, semantics, propagation rules).
- Migrate examples (`cpp-project`, `lua-build`, `fzf-picker`) to `use cpp` against the blessed copy; delete the per-example copies.
- Document the *vendoring escape hatch*: a project can ship its own `cook_modules/cpp.lua` and the resolver still picks the local copy first.
- Add conformance fixtures covering the public surface.

**Exit criteria.**
- All in-tree examples build using `use cpp` resolved from the standard module.
- No `cpp.lua` exists outside `standard/cook_modules/`.
- The Standard has a chapter (or appendix) specifying the cpp module's public surface.
- Conformance suite covers the surface.

**Out of scope.** Zero new features. Pure structural prerequisite.

### M1 — Cook-native finders for the well-known set

**Why now.** Unblocks "consume installed raylib" — the first concrete raylib milestone. Without this, the only way to find a system library is via pkg-config; libraries without `.pc` files (OpenAL, GL, X11 directly, frameworks) are unreachable.

**Scope.**
- Generalize `cpp.find` to multi-strategy: project-registered finder → curated package → pkg-config → bare-lib probe. First match wins; result is cached per (package, opts) per recipe.
- Curated finders (Lua) for the Doom-3-relevant short list: `raylib`, `sdl2`, `openal`, `gl`/`opengl`, `threads`, `zlib`, `libcurl`. Each finder knows Linux + macOS at minimum.
- Result record shape grows from `{ cflags, libs }` to `{ cflags, libs, system_libs, include_dirs, lib_dirs, frameworks, found, version }` — superset; existing call sites stay compatible because the new fields are optional.
- Optional version constraint: `cpp.find("raylib", { version = ">=4.0" })` — semver-style comparison against detected version.
- Frameworks support: macOS `-framework OpenAL` etc. emitted into `frameworks` and consumed by `cpp.link`.
- Diagnostics on miss: a clear message naming each strategy that was tried and why it failed (`tried: pkg-config raylib (not found), curated finder (no libraylib.so on default search path), bare probe (failed)`).

**Exit criteria.**
- New `examples/raylib-game/` builds a small raylib demo (e.g., `core_basic_window`) on Linux + macOS using only `cpp.find("raylib")`.
- All curated finders have conformance fixtures.
- Diagnostic output for failures is actionable (names what was tried, why each failed, what to install).

**Out of scope.** Building raylib from source (M3+). CMake-compat (M2). Anything Windows.

### M2 — CMake-compat: consume `XxxConfig.cmake`

**Why now.** A long tail of libraries ship `XxxConfig.cmake` but no `.pc` file (SDL3, glfw, many vendor SDKs). Hand-writing a curated finder for every one is busywork and exactly the position we ruled out — the whole point of the CMake-compat path is that we lean on cmake's own resolver for the long tail.

**Scope.**
- Add a CMake-compat strategy to `cpp.find`'s chain (after curated, before pkg-config; or opt-in via `cmake = true`).
- Implementation: shell out to `cmake --find-package` (subprocess), parse stdout, map output to the M1 result-record shape. We do *not* parse `XxxConfig.cmake` ourselves.
- Cache aggressively (subprocess is slow — avoid re-running across recipes in the same Cookfile).
- Failure modes produce actionable diagnostics: "cmake binary not on PATH", "no Config for X in CMake's search paths", "imported-target chain too complex — see [docs link]".

**Exit criteria.**
- At least one example uses a CMake-discovered package — a library that ships `Config.cmake` but no `.pc`.
- Documentation explains the trust boundary between Cook-native and CMake-compat strategies, and when to prefer each.

**Out of scope.** Parsing `CMakeLists.txt` (north-star territory; not committed). Parsing `XxxConfig.cmake` directly (we shell out to cmake instead). Windows-specific cmake search paths (deferred).

### M3 — Configure step + `config.h` generation

**Why now.** Building raylib *itself* requires generating a config header from header/function probes against the host toolchain. Doom 3 has the same shape (multiple `config.h` files across subsystems).

**Scope.**
- New `cpp.checks` namespace exposing probe functions:
  - `cpp.checks.has_header(name, opts)` → boolean
  - `cpp.checks.has_function(name, opts)` → boolean
  - `cpp.checks.has_define(name)` → boolean
  - `cpp.checks.sizeof(type)` → integer
  - `cpp.checks.endian()` → `"little"` | `"big"`
  - `cpp.checks.has_compile_flag(flag)` → boolean
  - `cpp.checks.has_link_flag(flag)` → boolean
- Each check compiles (and optionally runs) a small probe with the configured toolchain. Result is cached in `cook.cache` keyed by `(check_name, args, compiler, standard, flags)`.
- Probes use the *active* compiler + standard + warnings + extra cflags so checks reflect the real build environment.
- `cpp.config_header(template, output, vars)`: read `template.h.in`, perform `@VAR@` substitution and `#cmakedefine` macro processing, emit `output`. Semantically equivalent to CMake's `configure_file`.
- **No hidden state.** Checks return values; the user composes a `vars` table; that table is passed explicitly to `config_header`. Pattern:
  ```lua
  local vars = {
      HAVE_STDINT_H = cpp.checks.has_header("stdint.h"),
      HAVE_STRDUP   = cpp.checks.has_function("strdup"),
      VERSION       = "1.0",
  }
  cpp.config_header("config.h.in", "config.h", vars)
  ```

**Exit criteria.**
- `examples/raylib-game/` is upgraded to vendor and build raylib from source: configure → generate `raylib_config.h` → compile.
- Probes are deterministic on the same machine and cached (re-running configure does not re-probe).
- Documentation explains semantic equivalence to CMake's `check_*` family and the explicit-vars departure from CMake's hidden-scope model.

**Out of scope.** `try_run`-style runtime probes (defer unless a real case appears). Cross-compile-aware probing (defer; raylib's host-only build doesn't need it).

### M4 — Conditional sources, per-source flags, public/private propagation

**Why now.** Vendored raylib selects different platform sources per OS/backend (X11 vs Wayland vs cocoa). Doom 3's `sys/posix` vs `sys/win32` is the same shape. Without a clean pattern this becomes Lua spaghetti at the call site.

**Scope.**
- **Per-platform sources stay in user-Lua.** Canonical pattern:
  ```lua
  local sources = { "src/main.c", "src/common.c" }
  if cook.platform.os == "linux" then
      table.insert(sources, "src/platform_linux.c")
  elseif cook.platform.os == "macos" then
      table.insert(sources, "src/platform_macos.c")
  end
  cpp.bin("game", { sources = sources })
  ```
  The module ships at most a small helper if a clear pattern emerges, but the canonical answer is to build the list in user-Lua. No structured per-platform-table API.
- **Per-source compile flags** exercised and documented (the building blocks already exist via `cpp.compile`'s `extra_cflags`). Pattern is composing `cpp.compile` calls in user-Lua when per-source flags differ.
- **PRIVATE / PUBLIC / INTERFACE distinction** for transitive propagation. Today everything in `cook.export` is public (consumers see all defines/includes). M4 introduces an `export_*` vs internal split that mirrors CMake's `target_*(... PRIVATE ...)` semantics:
  - `defines = {...}` → private to the target
  - `export_defines = {...}` → public, propagated to consumers
  - same for `includes`, `system_libs`, `extra_ldflags`
  - existing field names retain current (public) semantics for backcompat, with a deprecation note in the Standard
  - decision on backcompat handling refined during M4 implementation planning

**Exit criteria.**
- Vendored raylib build picks the right backend per OS without conditional spaghetti at the `cpp.bin` call site.
- `examples/raylib-game/` exercises both the per-platform source split and at least one PRIVATE-flag case.

**Out of scope.** Build configurations (M5). Install/packaging (deferred to Doom-3 sub-roadmap).

### M5 — Build configurations via existing config blocks

**Why now.** A serious dev workflow ships both debug and release binaries from the same source tree. Cook already has `config NAME ... end` blocks (per `2026-04-05-config-lua-blocks-design.md`) that execute arbitrary Lua per-profile *before* recipes run. M5 is **not** a new top-level concept — it's making `cpp.lua` play well with the existing mechanism.

**Scope.**
- The cpp module relies on the existing setter surface (`cpp.defaults({...})`, `cpp.toolchain({...})`) being callable from inside `config` blocks. Most of this works today — `cpp.defaults` is additive-merge per the QoL spec, which is exactly the layering semantics config blocks need.
- Fill any gaps in setter coverage as they surface during raylib bring-up. Likely small additions:
  - `cpp.defaults({ extra_cflags = "-O3" })` should be a clean way to set per-config optimization (verify it composes correctly when called from both base + named config blocks)
  - `cpp.defaults({ extra_ldflags = "..." })` similarly
- **Output-path awareness.** When a config is active, the cpp module routes outputs to `build/<config>/...` instead of `build/...` so Debug + Release coexist on disk and switching configs doesn't invalidate the other's cache. The active config name is read from `cook.config.name` (or equivalent) at unit-registration time.
- Document the canonical pattern in the Standard:
  ```
  config                                # base — always runs
      cpp.defaults({ defines = {"COMMON"} })
  end

  config dev
      cpp.defaults({ extra_cflags = "-O0 -g", defines = {"DEBUG"} })
  end

  config release
      cpp.defaults({ extra_cflags = "-O3", defines = {"NDEBUG"} })
  end

  recipe game
      cpp.bin("game", { sources = ... })  -- picks up config-block settings
  end
  ```
- **No new top-level cpp concept.** No `cpp.config(name, opts)` function.

**Exit criteria.**
- Raylib game builds in both `cook game --config dev` and `cook game --config release`.
- Binaries land in `build/dev/` and `build/release/` respectively.
- Switching configs does not trigger a full rebuild of the other.

**Cross-cutting dependency.** Confirm `cook.config.name` (or equivalent active-config accessor) is exposed to module code at unit-registration time. If not, that's a small cook-core add — the only non-cpp-module surface change in this milestone. Verify during M5 sub-spec.

### M6 — Precompiled headers

**Why now.** Doom 3 leans hard on `precompiled.h` per major library. Without PCH support, Doom 3 build times become miserable and `idLib` in particular becomes the bottleneck.

**Scope.**
- `cpp.pch(target, header, opts)`: pre-compile `header` once for `target`; all sources in `target` consume it (`-include-pch` on Clang, `.gch` on GCC).
- Compiler-specific dispatch + sanity rails: PCH is brittle (standard mismatches, define mismatches silently produce wrong code). Module verifies build-flag consistency between PCH compile and consumer compiles; mismatch → error.
- PCH file is treated as a normal Cook unit: `inputs = { header } + dep-tracked includes`, `output = build/[<config>/]pch/<target>/<stem>.pch` (incorporates M5's config-aware output path).

**Exit criteria.**
- A measurable speedup demo on a fat-PCH target (raylib game with a synthetic large PCH, or a small synthetic benchmark).
- Both Clang and GCC PCH paths exercised in conformance.

**Out of scope.** C++20 header units / module-as-PCH (separate rabbit hole). MSVC PCH (Windows is post-Doom-3-checkpoint).

## Doom 3 destination — capability checklist

After M6 ships, write a *new* roadmap spec (`YYYY-MM-DD-doom3-roadmap.md`) that commits to a fork (likely dhewm3, possibly RBDOOM-3-BFG), enumerates the remaining gaps with current intel, and lays out post-M6 milestones in detail.

What M0–M6 must collectively unlock for Doom 3:

| Capability | Delivered by |
|---|---|
| Discover SDL2 / OpenAL / libcurl / GL / X11 / ZLIB / JPEG / OGG / Vorbis | M1 + M2 |
| Generate `config.h` from header/function/sizeof probes | M3 |
| Per-platform `sys/` source split | M4 |
| PRIVATE vs PUBLIC compile-flag propagation across `idLib` ↔ `game` ↔ `d3xp` | M4 |
| Debug + Release builds from one Cookfile | M5 |
| PCH for `idLib` and friends | M6 |

Likely additional Doom-3 needs (deferred to sub-roadmap):

- **Multi-target subdirectory composition at scale.** Cross-Cookfile composition is *already* a Cook feature; what's needed is conventions/patterns for a 7+ subsystem layout (idLib, sys, renderer, sound, game, d3xp, cgame, …).
- **Optional Windows / `.rc` resource compilation.** Only if Windows is in the Doom-3 scope.
- **Optional install / packaging rules.** Only if a release artifact is the goal.
- **Windows-finder pass** through M1's curated set, deferred from M1 to keep raylib milestones Linux/macOS-only.

**Why deferred.** Anything written about M7+ today will need to be revised once raylib has actually been built through M0–M6. Premature commitment to milestone details is worse than honest deferral with a clear trigger ("M6 lands → write Doom-3 sub-roadmap").

## Open questions surfaced for sub-specs

- **M5:** confirm `cook.config.name` (or equivalent) is exposed to module code at unit-registration time. If not, add it.
- **M4:** how to handle backcompat when introducing `export_*` vs implicit-public field semantics — deprecate the implicit form, alias them, or hard-cut. Decide during M4 implementation planning.
- **Doom-3 sub-roadmap:** dhewm3 vs RBDOOM-3-BFG. Commit at sub-roadmap time, not now.

## Testing strategy (cross-milestone)

Each milestone adds:
- **Unit-level conformance fixtures** under `standard/conformance/positive/` for the new surface, covering happy paths and at least one edge case per public function.
- **Negative fixtures** for diagnostic surfaces that must produce specific errors (M1 finder misses, M2 cmake-not-found, M3 probe failures, M5 unknown config name, M6 PCH flag mismatch).
- **Example demonstration**: each milestone has a corresponding example under `examples/` exercising the new surface end-to-end. M1–M6 cumulatively grow `examples/raylib-game/` from "consume installed raylib" to "vendor + build raylib + multi-config + PCH."

## Out of scope (explicitly)

- CMake interpretation (parsing and executing `CMakeLists.txt`). North-star direction; not committed.
- Community-module registry. Separate vision.
- Windows support in raylib milestones. Surfaces in Doom-3 sub-roadmap.
- Doom 3 fork commitment. Deferred to sub-roadmap.
- C++20 header units / module-as-PCH. Defer past Doom 3.
- `try_run`-style runtime probes in `cpp.checks`. Defer unless a real case appears.
- Cross-compile-aware probing. Defer.
