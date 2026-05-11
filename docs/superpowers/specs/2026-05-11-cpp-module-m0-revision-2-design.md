# M0 Revision 2 — `cc` module promotion in the post-LuaRocks world

Date: 2026-05-11
Status: Design — pending implementation plan
Linear: SHI-133 (M0; epic SHI-132 "Cook builds Doom 3")
Supersedes:
- `2026-05-01-cpp-module-roadmap-to-doom3-design.md` §M0 (original; `standard/cook_modules/cpp.lua` premise)
- `2026-05-08-cpp-module-m0-revision-design.md` (revision 1; `cook pull` registry premise, deleted in SHI-176 Phase 4)
Standard impact:
- New normative chapter `09-standard-modules.mdx` introducing §9.2 `cc` at RFC-2119 quality.
- New paragraph in `appendix/B-rationale.mdx` (cc-not-cpp; normative-chapter rationale).
- Engine alignment in `cook-register/src/module_loader.rs` to honour Standard §7's resolution order (already specified; the register-side never implemented `share/lua/5.4/`).

## Why this revision exists

Two prior M0 specs are stale:

1. The original (May 1) said *move QoL `cpp.lua` to `standard/cook_modules/cpp.lua`*. That location was never wired into module resolution.
2. Revision 1 (May 8) said *bootstrap via `cook pull` from a git registry*. SHI-176 Phase 4 (merged May 10) **deleted** `cook pull` and its 1953 LoC, replacing it with `cook modules` backed by LuaRocks and a published index at `rocks.usecook.com`.

The world today: `cook_cpp-0.0.1-1` is published as a 12-line **stub** that errors with *"real implementation lands in SHI-133"*. Its source lives at `~/dev/cook-modules/cook_cpp/cook_cpp.lua` (the Phase 4 publish monorepo). The QoL surface (973 lines) is still vendored in three example dirs (`lua-build`, `fzf-picker` byte-identical; `cpp-project` at 806 lines on a pre-QoL API surface). A leftover `~/dev/cook_modules/modules/cpp/init.lua` from the dead `cook pull` registry exists but is unreachable.

Revision 2 reframes M0 to the actual structural step: replace the stub body with a green-field rewrite that **leverages the rocks ecosystem `cook modules` unlocked**, publish `cook_cc-0.1.0-1` (renamed), and crystallise the public surface in a normative Standard chapter.

## Scope summary

- **Rename**: rock `cook_cpp` → `cook_cc`. The module handles C and C++ uniformly; the name `cpp` privileges one of two equal languages. M0 is the cleanest moment to fix this.
- **Green-field rewrite** of the module body. Take the QoL public surface as the spec (with targeted refinements); rebuild internals as a multi-file rock that uses `lua-cjson` (replaces ~80 lines of hand-rolled JSON) and `lpeg` (parses pkg-config output cleanly into the M1-shaped result record).
- **Engine fix as M0.0 prereq**: register-side `cook.load_module` doesn't honour `share/lua/5.4/` — Phase 4 left this gap and `cook_cc` is the first real consumer that trips on it. Standard §7 already specifies the right order; the register-side just hasn't been brought into compliance.
- **Standard chapter** at `09-standard-modules.mdx` with §9.2 `cc` specifying the public surface at RFC-2119 quality. ISO-C style: numbered subsections, formal signatures, rationale annexed to App. B.
- **Examples migration** from vendored `cook_modules/cpp.lua` to `cook.toml` + `cook.lock` + `cook modules install`. `cpp-project`'s Cookfile lifted from the pre-QoL API.
- **Conformance** split across two layers: dense busted unit tests in `~/dev/cook-modules/cook_cc/spec/` (cook-modules CI); ~10 end-to-end fixtures under `standard/conformance/{positive,negative}/cc-*` exercising the public contract.
- **Stub cleanup**: leave `cook_cpp-0.0.1-1` published (luarocks has no proper yank; the stub already errors with a clear pointer). Update SHI-133 title. Note dead `~/dev/cook_modules/` repo in closeout — user-owned cleanup.

Zero new features in the user-visible API. The cleanup of pre-rocks workarounds is the rewrite's prize, but the public surface stays at parity with the QoL contract that examples already exercise.

## State of the world (2026-05-11)

```
~/dev/cook                     ← M0.0 + M0.3 + M0.4 + M0.5 land here
~/dev/cook-modules             ← M0.1 + M0.2 land here (publish monorepo)
~/dev/cook-rocks               ← M0.2 stages into here (rocks-index repo)
rocks.usecook.com              ← Cloudflare Pages on the GitHub mirror
~/dev/cook_modules             ← dead pre-Phase-4 cook pull registry (user-owned cleanup)

examples/cpp-project/cook_modules/cpp.lua   ← 806 lines, pre-QoL API
examples/lua-build/cook_modules/cpp.lua     ← 973 lines, QoL surface
examples/fzf-picker/cook_modules/cpp.lua    ← 973 lines, byte-identical to lua-build

cook_cpp-0.0.1-1 (stub)        ← published; errors with SHI-133 pointer
cook_cc-0.1.0-1 (this M0)      ← target rock
```

There is no CI for cook-modules. Publishing is the chore-driven local flow from `~/dev/cook-modules/Cookfile`:

```sh
# On clean main, rockspec committed:
MODULE=cook_cc VERSION=0.1.0-1 cook publish
```

This tags HEAD, pushes Gitea + GitHub, runs `luarocks pack`, copies the artifacts to `~/dev/cook-rocks/`, regenerates the manifest with `luarocks-admin make-manifest .`, commits there, and recurses into `~/dev/cook-rocks/`'s own `cook publish` chore (which pushes the index and force-pushes the GitHub orphan snapshot that Cloudflare Pages serves).

## 1. Rock identity, layout, and publish flow

**Rock name:** `cook_cc` (renamed from the Phase 4 stub `cook_cpp`).

**Lives in:** `~/dev/cook-modules/cook_cc/` (`git mv` from `cook_cpp/` so history follows).

**Initial real version:** `0.1.0-1` (`0.0.1-1` was the stub; `0.1.0-1` is the minimum semver step that says "released, pre-1.0").

**Source tree:**

```
~/dev/cook-modules/cook_cc/
├── cook_cc-0.1.0-1.rockspec
├── README.md
├── init.lua            — module entry point; returns the cc table; owns init() hook
├── toolchain.lua       — compiler detection + defaults/toolchain setters
├── cc.lua              — compile / archive / link primitives
├── targets.lua         — bin / lib / shared / headers declarative makers
├── finder.lua          — find (pkg-config in M0; multi-strategy in M1)
├── compile_db.lua      — compile_commands (worker-VM-safe, cjson-backed)
├── transitive.lua      — resolve_links + cook.export propagation
└── spec/               — busted unit tests; run by `chore spec` in cook-modules
    ├── compile_db_spec.lua
    ├── transitive_spec.lua
    ├── targets_spec.lua
    ├── finder_spec.lua
    └── toolchain_spec.lua
```

**Rockspec:**

```lua
package = "cook_cc"
version = "0.1.0-1"
source = {
   url = "git+https://github.com/lioralabs/cook-modules.git",
   tag = "cook_cc-0.1.0-1",
}
description = {
   summary  = "Cook C-family (C + C++) native build module",
   detailed = [[
      Blessed Cook module for C and C++ native builds. Provides declarative
      target makers (cc.bin/lib/shared/headers), low-level primitives
      (cc.compile/archive/link), pkg-config discovery (cc.find), and
      compile_commands.json generation. Specified normatively at §9.2 of the
      Cook Standard.
   ]],
   homepage   = "https://github.com/lioralabs/cook-modules",
   license    = "MIT",
   maintainer = "Liora Labs <code@lioralabs.dev>",
}
dependencies = {
   "lua >= 5.4",
   "lua-cjson ~> 2.1",
   "lpeg ~> 1.0",
}
build = {
   type    = "builtin",
   modules = {
     ["cook_cc"]            = "cook_cc/init.lua",
     ["cook_cc.toolchain"]  = "cook_cc/toolchain.lua",
     ["cook_cc.cc"]         = "cook_cc/cc.lua",
     ["cook_cc.targets"]    = "cook_cc/targets.lua",
     ["cook_cc.finder"]     = "cook_cc/finder.lua",
     ["cook_cc.compile_db"] = "cook_cc/compile_db.lua",
     ["cook_cc.transitive"] = "cook_cc/transitive.lua",
   },
}
```

`source.url` uses `git+https://` with a `tag` qualifier and **no `source.dir`** — see `project_cook_module_publishing.md`'s "Bootstrap gotcha". `luafilesystem` is intentionally not in `dependencies`: `cook.fs.*` already covers M0's needs.

## 2. Public API surface (the Standard contract)

`cc` is whatever the user binds via `use cook_cc as cc`. The Cookfile-canonical alias for C++ projects is `as cpp`; for C-only projects, `as cc`. The rock name is namespaced; the binding is the user's choice.

### Target makers (declarative)

```lua
cc.bin(name, opts)        -- executable → build/bin/<name>
cc.lib(name, opts)        -- static lib (.a) → build/lib/lib<name>.a
cc.shared(name, opts)     -- shared lib (.so/.dylib) → build/lib/lib<name>.<ext>
cc.headers(name, opts)    -- header-only library; registers exports, no compile
```

M5 will extend the output paths to gain a `<config>/` segment (`build/<config>/bin/<name>`, etc.) when a named config is active. M0 specifies only the no-config form; M5's spec delta is additive.

`opts` (table; all optional unless noted):

| Field | Type | Required for | Semantics |
|---|---|---|---|
| `sources` | list[string] | `bin`/`lib`/`shared` | Source paths or globs |
| `dir` | string | — | Source root for recursive glob (alternative to `sources`) |
| `includes` | list[string] | — | Include dirs (private to this target) |
| `export_includes` | list[string] | — | Include dirs propagated to consumers |
| `defines` | list[string] | — | Macro names or `NAME=value` |
| `system_libs` | list[string] | — | Names passed to linker as `-l<name>` |
| `extra_cflags` | string | — | Raw flags appended at compile time |
| `extra_ldflags` | string | — | Raw flags appended at link time |
| `links` | list[string] | — | Other cc-registered targets to link against |
| `standard` | string | — | e.g. `"c++17"`, `"c11"` |
| `warnings` | string | — | `"default"` (`-Wall`), `"strict"` (`-Wall -Wextra -Wpedantic`), `"none"`, or raw flags |
| `modules` | list[string] | — | C++20 module deps (Clang-only in M0) |

### Building blocks (lower-level)

```lua
cc.compile(source, opts) -> obj_path     -- compile one source; returns object path
cc.archive(objects, output)              -- ar rcs the static lib
cc.link(objects, output, opts)           -- link executable or shared
```

### Discovery

```lua
cc.find(name, opts) -> result
```

`result` is **pre-shaped for M1** so multi-strategy expansion is additive, not breaking:

```lua
{
   found        = boolean,
   cflags       = string,
   libs         = string,
   system_libs  = {},   -- empty in M0; M1 populates from lpeg parse
   include_dirs = {},   -- empty in M0
   lib_dirs     = {},   -- empty in M0
   frameworks   = {},   -- empty in M0
   version      = nil,  -- nil in M0
}
```

`opts.version` is accepted and ignored in M0; M1's curated finders honour it.

M0 implements pkg-config only. The lpeg grammar in `finder.lua` parses pkg-config's `--cflags`/`--libs` output and populates the structured fields. The M0 record carries the structured form even though only `cflags`/`libs` are non-empty — that's the forward-compat contract.

### Configuration

```lua
cc.toolchain(opts)   -- opts = { compiler=, standard=, warnings= }
cc.defaults(opts)    -- additive-merge module-wide defaults; callable from config blocks
```

`cc.defaults({...})` is the **only** way to set module-wide defaults. The pre-rocks env-var defaults (`CPP_DEFINES` / `CPP_INCLUDES` / `CPP_SYSTEM_LIBS` / `CPP_STANDARD` / `CPP_WARNINGS`) are **removed** in M0 — config-block-driven defaults are the canonical mechanism per the M5 roadmap. The Standard chapter notes the migration.

### Observability

```lua
cc.compile_commands()    -- writes compile_commands.json for currently-registered targets
```

### Removed from the previous QoL surface

| Removed | Reason |
|---|---|
| `cpp.executable` / `cpp.static_library` / `cpp.shared_library` / `cpp.interface_library` | Long-form aliases for the same functions. Canonical = short forms (`cc.bin` / `cc.lib` / `cc.shared` / `cc.headers`). |
| `cpp.init()` (as a user-callable function) | The `init` hook stays — Standard §6.3.4 contract — but is not user-facing. The Standard chapter does not enumerate it. |
| `cpp.state` (as an exposed table) | Module-local in `toolchain.lua`. Not on the returned table. |
| `CPP_DEFINES` / `CPP_INCLUDES` / `CPP_SYSTEM_LIBS` / `CPP_STANDARD` / `CPP_WARNINGS` env reads | Superseded by `cc.defaults(...)` from `config` blocks. |

### Transitive propagation contract (M0 normative)

- `cook.export(target, { includes=, defines=, system_libs=, extra_ldflags=, compile_info= })` records public propagation for `target`.
- `cook.import(target_name)` returns the recorded propagation. The cc target makers call this for each `links` entry, transitively, deduplicating.
- M0 keeps the implicit-public model: all `includes`/`defines`/etc. listed in `opts` are propagated to consumers via `cook.export`. The PRIVATE/PUBLIC distinction (`export_includes` already exists; `export_defines`/`export_system_libs`/`export_extra_ldflags` are M4) is **out of scope for M0**; M4 introduces the full split.

### Error model

- Errors are prefixed with module + function: `error("[cc.bin] no sources found for target '" .. name .. "'", 2)`.
- `level=2` blames the caller's line for user-misuse errors so the diagnostic surfaces at the Cookfile line, not inside cc.
- Internal invariants use plain `error(...)` so bugs surface with a cc stack frame.

## 3. State model and VM boundary

Cook runs Lua in two distinct VM contexts that share zero memory:

1. **Register VM** — one per `cook` invocation. Parses the Cookfile, runs `config` blocks, runs recipe-register bodies (each `cc.bin(...)` call). Emits `WorkItem`s for the DAG.
2. **Worker VMs** — a pool of N threads, each with its own `mlua::Lua`. Pulls `WorkItem`s off a shared queue. Per `cook-luaotp/src/pool.rs:157`: *"Each worker creates its own Lua VM. The VM is `!Send` but never moves between threads."* One worker VM may serve units from multiple Cookfiles (pool.rs:22).

**State sharing across VMs goes through `cook.cache.*`** (disk-backed K/V; survives across invocations). Module-local Lua tables are per-VM and ephemeral.

**The `init()` hook** is the loader-contract entry point (Standard §6.3.4; reference impl `cook-register/src/module_loader.rs:251-254`). The loader calls it once per VM, automatically, the first time the module is loaded in that VM. Not user-callable. cc's `init()` rehydrates the file-local toolchain state from `cook.cache`:

```lua
-- cook_cc/init.lua (sketch)
local M = {}
local toolchain = require("cook_cc.toolchain")
local cc        = require("cook_cc.cc")
local targets   = require("cook_cc.targets")
local finder    = require("cook_cc.finder")
local db        = require("cook_cc.compile_db")

function M.init()                  -- loader auto-calls, once per VM
    toolchain.rehydrate()          -- reads cook.cache.get("compiler");
                                   -- detects-and-caches on miss
end

-- public surface (exposed verbs only)
M.toolchain        = toolchain.set
M.defaults         = toolchain.merge_defaults
M.compile          = cc.compile
M.archive          = cc.archive
M.link             = cc.link
M.bin              = targets.bin
M.lib              = targets.lib
M.shared           = targets.shared
M.headers          = targets.headers
M.find             = finder.find
M.compile_commands = db.write

return M
```

### Where each piece of state lives

| State | Storage | Lifetime | Set by | Read by |
|---|---|---|---|---|
| Detected compiler (`{cxx, cc}`) | `cook.cache.get("compiler")` + VM-local copy | invocations + all VMs | `init()` (detect-and-cache) | every VM that loads cc |
| `cc.toolchain` / `cc.defaults` overrides | file-local in `toolchain.lua` | register VM only | config blocks | `cc.bin/lib/...` at register time |
| Per-target compile info (sources, flags, includes, defines) | `cook.export(name, {compile_info=...})` | register → workers | `targets.bin/lib/...` | `compile_db.write` (worker VM) |
| Registered target list | `cook.cache.get/set("known_targets")` | register → workers | `targets.bin/lib/...` | `compile_db.write` |
| pkg-config results | `cook.cache.get/set("pkg:<name>")` | invocations | `finder.find` first call | `finder.find` subsequent calls |

### Why each function runs where

| Function | Phase | VM | Rationale |
|---|---|---|---|
| `cc.toolchain` | register | register | called from `config` blocks |
| `cc.defaults` | register | register | called from `config` blocks |
| `cc.bin/lib/shared/headers` | register | register | called from recipe-register bodies; bake compile/link commands into shell-string `WorkItem`s |
| `cc.compile/archive/link` | register | register | composed by target makers; baked into `WorkItem`s |
| `cc.find` | register | register | called from recipe-register bodies; pkg-config shellouts at register time |
| `cc.compile_commands` | execute | worker | called from a recipe body (`recipe compile-commands ... cc.compile_commands() end`); reads from `cook.cache` + `cook.import` since the worker VM has no register-VM state |

The split is what makes the rewrite tractable: the only function that needs to survive the VM boundary is `cc.compile_commands`, and it does so via `cook.cache` + `cook.import` — both already established Cook APIs.

## 4. Internal architecture

### Rock-dep use sites (concrete)

**`lua-cjson`** — `compile_db.lua` only:

```lua
-- replaces ~80 lines of hand-rolled JSON
local cjson = require("cjson")
fs.write("compile_commands.json", cjson.encode(entries) .. "\n")
```

`cjson.decode` is available for free; M3's `cc.config_header` (if it needs to read JSON inputs) inherits it.

**`lpeg`** — `finder.lua` only. Parses pkg-config's output:

```
-I/usr/include/foo -DFOO_VERSION=2 -L/usr/lib -lfoo -Wl,-rpath,/x -framework OpenGL
```

into structured tokens populating the M1 result record (`include_dirs={"/usr/include/foo"}`, `defines={"FOO_VERSION=2"}`, `lib_dirs={"/usr/lib"}`, `system_libs={"foo"}`, `extra_ldflags="-Wl,-rpath,/x"`, `frameworks={"OpenGL"}`). M0 populates only the fields pkg-config produces; M1's curated finders + cmake-compat extend the same grammar.

### File responsibilities

- **`init.lua`** — module entry. Returns the cc table. Stitches submodules together. Owns the loader's `init` hook. ~40 LoC.
- **`toolchain.lua`** — file-local state table (compiler, default_standard, warnings, defaults). `cc.toolchain(...)` / `cc.defaults(...)` setters. `rehydrate()` reads from `cook.cache`; `detect()` shells out to find `g++`/`clang++` and caches. Internal accessor functions (`get_compiler()`, `get_defaults()`, etc.) used by other submodules. ~150 LoC.
- **`cc.lua`** — pure compile/archive/link primitives. `cc.compile(source, opts)`, `cc.archive(objects, output)`, `cc.link(objects, output, opts)`. Reads toolchain state via accessor; emits `cook.add_unit(...)` with baked shell strings. ~200 LoC.
- **`targets.lua`** — declarative target makers: `cc.bin/lib/shared/headers`. Composes cc primitives + transitive. Calls `cook.export(name, {compile_info=...})` per target so compile_db can recover it. Registers the target name via `cook.cache` so compile_db knows the full list. ~300 LoC.
- **`transitive.lua`** — pure transitive-link resolution + dedup. Walks `links` via `cook.import` and returns the merged propagation set. No state. ~80 LoC.
- **`finder.lua`** — `cc.find(name, opts)`. M0 implements pkg-config only. Uses lpeg to parse output into the M1-shaped result record. Caches each result in `cook.cache`. ~120 LoC.
- **`compile_db.lua`** — `cc.compile_commands()`. Runs in a worker VM. Reads target list from `cook.cache.get("known_targets")`, per-target info from `cook.import`. Emits JSON via `cjson.encode`. ~80 LoC.
- **`spec/`** — busted unit tests, one file per submodule. Fast (< 5s for the suite). Run by `cd ~/dev/cook-modules && cook spec`.

Total ~1000 LoC across 7 files (vs. the 973-line single file today; the line count is roughly preserved because the JSON encoder savings are offset by per-file headers and the structured finder).

## 5. Standard chapter

**Location:** `standard/src/content/docs/09-standard-modules.mdx` — new top-level numbered chapter. Normative content goes in numbered chapters per the user's ISO-C-style preference; appendices are informative.

**Structure:**

```
§9 Standard Modules
   §9.1 Scope and bootstrap
        — what this chapter governs; cook modules install as bootstrap;
          vendoring escape hatch (project-local hand-vendored override wins
          over share/lua/5.4 per Standard §7's resolution order).

   §9.2 cc — C-family build module
        §9.2.1  Synopsis and conformance scope
        §9.2.2  Identity, resolution, init hook (Standard §6.3.4 contract)
        §9.2.3  Public surface (RFC-2119 MUST / SHOULD per signature)
                9.2.3.1   cc.bin
                9.2.3.2   cc.lib
                9.2.3.3   cc.shared
                9.2.3.4   cc.headers
                9.2.3.5   cc.compile
                9.2.3.6   cc.archive
                9.2.3.7   cc.link
                9.2.3.8   cc.find
                9.2.3.9   cc.defaults
                9.2.3.10  cc.toolchain
                9.2.3.11  cc.compile_commands
        §9.2.4  Transitive propagation
        §9.2.5  Error model
        §9.2.6  Vendoring after install
```

Each public function gets: a formal Lua signature, normative semantics (MUST/SHOULD), the option table schema with per-field semantics, output-path semantics, and at least one positive and one negative example.

**Rationale annex** at `appendix/B-rationale.mdx`: new paragraph explaining (a) why `cc` rather than `cpp` — the module handles both C and C++ uniformly; the toolchain term `cc` is the historically and idiomatically accurate name; and (b) why the Standard Modules chapter is normative — anyone publishing an implementation of `cc` (e.g., a re-implementation in another language layer) must honour the contract, not just track a Lua reference file.

The chapter is grown additively by M1–M6 sub-specs. Each milestone's spec stages its chapter delta in the same commit as the implementation, per `feedback_spec_first_no_bypass.md`.

## 6. Conformance

Two layers, separated by concern.

### Layer 1 — Busted unit tests (cook-modules)

Location: `~/dev/cook-modules/cook_cc/spec/`. Run by `cd ~/dev/cook-modules && cook spec` (new chore added to cook-modules/Cookfile). Fast (< 5s); no C compilation.

Covers internals densely:

- `toolchain_spec.lua`: compiler detection happy path + miss; defaults additive-merge; warnings preset resolution; per-VM rehydrate idempotence.
- `transitive_spec.lua`: link resolution dedup; diamond propagation; cycle detection (if any); export/import roundtrip.
- `targets_spec.lua`: bin/lib/shared/headers happy paths; option-table validation; compile_info shape persisted via `cook.export`.
- `finder_spec.lua`: lpeg grammar against fixture pkg-config outputs (Linux/macOS pkg-config text variants); cache hit; missing-pc behaviour.
- `compile_db_spec.lua`: JSON shape; entry ordering; recovers state when run with a pre-populated mock `cook.cache` + `cook.export` store.

### Layer 2 — End-to-end conformance fixtures (cook-lang)

Location: `standard/conformance/{positive,negative}/cc-*`. Slow (each fixture runs a real compile); ~10 fixtures total exercising the public contract end-to-end.

**Harness setup:** A new setup step in the conformance test harness populates `standard/conformance/_shared/cook_cc/` once at suite start. Source resolution order: `(1)` `COOK_CC_PATH` env var (dev iteration override); `(2)` the published rock fetched via `luarocks install cook_cc --tree _shared --server https://rocks.usecook.com`. The `_shared/cook_cc/share/lua/5.4/cook_cc/…` tree is referenced by each `cc-*` fixture via the existing `share/lua/5.4` resolution path (the same that M0.0's engine fix unlocks register-side).

**Fixtures:**

```
positive/
  cc-bin-c-source         — cc.bin compiles a .c source via cc, produces build/bin/foo
  cc-bin-cpp-source       — cc.bin compiles a .cpp source via cxx, produces build/bin/foo
  cc-lib-and-link         — cc.lib + cc.bin with links propagates includes/system_libs
  cc-headers-only         — cc.headers registers exports; consumer sees include dirs
  cc-find-pkgconfig       — cc.find against a known .pc file returns the M1 record shape
  cc-compile-commands     — recipe body cc.compile_commands writes valid JSON
  cc-defaults-from-config — config block cc.defaults({...}) flows through to cc.bin

negative/
  cc-find-missing         — cc.find("nope") returns found=false with a diagnostic
  cc-bin-missing-source   — cc.bin with a non-existent source errors at the Cookfile line
```

Standard §9.2 spec-first-commits each contract that a fixture proves. Per `feedback_spec_first_no_bypass.md`: the standard text and the fixture land in the same commit.

## 7. Examples migration

Each of `lua-build`, `fzf-picker`, `cpp-project` adopts the same shape:

```
examples/<name>/
├── .gitignore       # adds: cook_modules/, .cook/, build/
├── cook.toml        # [modules] cook_cc = "^0.1"
├── cook.lock        # generated; committed
├── Cookfile         # use cpp  →  use cook_cc as cpp
└── (old cook_modules/cpp.lua removed via git rm)
```

Per-example detail:

| Example | Bind | API-call edits | Notes |
|---|---|---|---|
| `lua-build` | `use cook_cc as cpp` | None — Cookfile already uses `cpp.bin`/`cpp.lib` | smallest migration |
| `fzf-picker` | `use cook_cc as cpp` | None — already on QoL short names | byte-identical Cookfile shape |
| `cpp-project` | `use cook_cc as cpp` | `cpp.executable` → `cpp.bin`; `cpp.static_library` → `cpp.lib`; verify `run-tests` glob `build/bin/test_*` still resolves | the lift; pre-QoL → QoL API |

**Bootstrap from a fresh clone:** `cd examples/<name> && cook modules install && cook <build-target>` — `<build-target>` is each example's primary recipe (`build` for `lua-build`; for `fzf-picker` and `cpp-project`, M0.4 adds a `recipe build: ...` aggregating that example's primary targets so the verification command is uniform).

M0 does **not** introduce auto-install (a `cook` invocation that runs `cook modules install` implicitly when `cook.lock` is present but `cook_modules/` is missing). That ergonomic improvement is a separate discussion.

## 8. Sequenced work

```
M0.0 — Engine fix (cook repo)
       cook-register/src/module_loader.rs: add share/lua/5.4/<name>.lua
       and share/lua/5.4/<name>/init.lua to the load_module path chain,
       mirroring cook-luaotp/src/pool.rs:616's four-path order.
       Add module_loader.rs::tests cases for both paths.
       Upgrade standard/conformance/positive/053-rocks-share-lua-resolution
       from parse-only to runtime-verified.
       Standard impact: §7 already specifies the order; this brings the
       register-side into compliance. Spec-first cite of §7 in the PR.

M0.1 — Rewrite cook_cc (cook-modules repo)
       git mv cook_cpp/ cook_cc/ (preserve history).
       Replace stub body with the multi-file structure of Section 1+4.
       Add cook_cc-0.1.0-1.rockspec with cjson + lpeg deps.
       Add cook_cc/spec/ busted suite.
       Add `chore spec` to cook-modules/Cookfile that runs the busted suite
       locally. Green-bar the suite.

M0.2 — Publish (cook-modules repo)
       From clean main, on the M0.1 commit:
         MODULE=cook_cc VERSION=0.1.0-1 cook publish
       Smoke from a /tmp scratch project:
         cook.toml: [modules] cook_cc = "^0.1"
         cook modules install
         cook_modules/share/lua/5.4/cook_cc/init.lua present
         cook.lock pinned at 0.1.0-1 with sha256 integrity

M0.3 — Standard chapter (cook repo)
       Write standard/src/content/docs/09-standard-modules.mdx.
       Append B-rationale paragraph (cc-not-cpp; normative chapter).
       Lands paired with the first piece of code in M0.4/M0.5 it contracts
       against — no COOK_STANDARD_BYPASS.

M0.4 — Examples migration (cook repo)
       Per-example as Section 7.
       cpp-project: lift cpp.executable→cpp.bin, cpp.static_library→cpp.lib.
       Each example verified via:
         cd examples/<name> && cook modules install && cook build
       Old cook_modules/cpp.lua removed via git rm.

M0.5 — Conformance fixtures (cook repo)
       Harness setup populates standard/conformance/_shared/cook_cc/ at
       suite start (COOK_CC_PATH override for dev; luarocks install for CI).
       Add cc-* fixtures from Section 6 Layer 2.
       Spec text in §9.2 lands in the same commit as each fixture, per
       spec-first.

M0.6 — Closeout
       Update SHI-133 title in Linear:
         "M0 — Promote cc.lua to a blessed Standard module"
       SHI-133 closing comment references this spec; notes dead
       ~/dev/cook_modules/ Gitea repo as user-owned cleanup; flags that
       cook_cpp-0.0.1-1 stays published as an error-on-call stub.
       Optional: publish cook_cpp-0.0.2-1 forwarding stub whose
       placeholder() errors with "renamed to cook_cc; run cook modules
       install cook_cc". Skip if scope creeps.
```

### Independence map

```
M0.0  ── independent ── lands first (engine prereq)
M0.1  ── independent (different repo)
M0.2  ── blocked by M0.1 (publishes M0.1's output)
M0.3  ── independent (spec chapter)
M0.4  ── blocked by M0.0 (register-side needs share/lua) + M0.2 (rock published)
M0.5  ── blocked by M0.0 + M0.2 + M0.3 (chapter contracts; rock installed)
M0.6  ── after M0.7 acceptance gate green
```

M0.0, M0.1, M0.3 can proceed in parallel. M0.2 follows M0.1. M0.4 and M0.5 follow M0.0 + M0.2 + (M0.3 for M0.5).

## 9. M0.7 acceptance gate

All must pass:

1. `cargo test -p cook-register` green incl. new share/lua resolution tests.
2. `cargo test -p cook-lang --test conformance` green incl. upgraded 053 and all new `cc-*` fixtures.
3. `cook publish` from `~/dev/cook-modules` with `MODULE=cook_cc VERSION=0.1.0-1` completes end-to-end; `https://rocks.usecook.com` serves the new manifest line within 60s of the push.
4. From a fresh scratch project: `cook.toml` with `[modules] cook_cc = "^0.1"`, `cook modules install` produces `cook_modules/share/lua/5.4/cook_cc/init.lua` (and siblings) plus a populated `cook.lock`.
5. Each of `examples/{lua-build, fzf-picker, cpp-project}`: `cook modules install && cook build` produces the expected binaries; outputs match pre-migration golden artifacts (size + symbol set spot-check).
6. `cd ~/dev/cook-modules && cook spec` green.
7. `cook check` green at the cook repo root.
8. `cook standard.build && cook standard.lint` green; the rendered §9 chapter is reachable from the table of contents and `lint_keywords` finds no missing RFC-2119 normative coverage.

## Risks and mitigations

- **The engine fix touches `cook.load_module` semantics.** Adding two paths is small but the module loader has cycle detection, source-hash caching, and an init-hook contract. *Mitigation:* `module_loader.rs::tests` already covers the existing paths; the M0.0 patch extends those tests rather than rewriting them. Conformance 053 upgrade is the integration backstop.
- **Lpeg grammar coverage for pkg-config output.** Real-world pkg-config files emit edge-case strings (commas in defines, quoted paths, double-dashed flags). *Mitigation:* M0.1's `finder_spec.lua` includes fixture inputs from at least five real `.pc` files on the dev host (sdl2, openssl, gtk+-3.0, lua5.4, libcurl). M1's curated finders will surface anything the grammar misses on first contact.
- **cjson availability across platforms.** `lua-cjson` is widely used but its C build requires a working toolchain for source rocks. *Mitigation:* on Linux/macOS the dev toolchain is universal; on Windows Phase 5 ships pre-built binary rocks. M0 only needs Linux+macOS for the raylib roadmap.
- **Examples no longer round-trip without `cook modules install`.** A contributor cloning `examples/cpp-project/` and running `cook build` cold will fail. *Mitigation:* README addition (one paragraph) explaining the `cook modules install` step. Auto-install on missing `cook_modules/` is a separate ergonomic ticket.
- **`cpp-project` lift introduces subtle output differences.** The pre-QoL API may have shipped slightly different default flags than the QoL API. *Mitigation:* M0.4 verification spot-checks each example's build outputs (symbol set, dynamic deps) against a pre-migration golden.
- **Standard chapter freezes a surface that M1+ will extend.** RFC-2119 normative text becomes brittle if every milestone has to re-spec what cc.find means. *Mitigation:* the M0 chapter is written with explicit extension points — `cc.find`'s record shape is pre-shaped for M1; `cc.defaults`/`cc.toolchain` are open-ended option tables; the chapter notes which sections M1–M6 extend.
- **The stub `cook_cpp-0.0.1-1` lingers in the registry forever.** Anyone who somehow installs it gets a clear error; the rename is visible from the SHI-133 closing comment and the cook-modules README. *Mitigation considered, not adopted:* a `cook_cpp-0.0.2-1` forwarding stub. Excluded for v1 because the SHI-133 stub already names a follow-up ticket; the user is the only consumer.

## What this enables

- **M1 (cook-native finders) can start.** Once `cc.find` returns the M1 record shape, M1 extends behaviour without breaking the contract.
- **M3 (configure step) inherits `cjson.decode`** for free if any of its probe helpers need to read JSON inputs.
- **The blessed-modules story has its first real entry.** Future modules (rust, pnpm, ai) follow the same recipe: rocks-backed body, multi-file structure where it earns its keep, Standard Modules chapter section, conformance split across busted unit tests + cook-lang end-to-end fixtures. The pattern is set.
- **The register-side / worker-side resolver gap closes.** Standard §7 has been correct since Phase 3; M0.0 brings the implementation into compliance. Future modules (and the user's own hand-vendored `cook_modules/<name>.lua`) inherit a consistent loader.

## References

- Original M0 source: `docs/superpowers/specs/2026-05-01-cpp-module-roadmap-to-doom3-design.md` §M0
- Revision 1 (superseded by this spec): `docs/superpowers/specs/2026-05-08-cpp-module-m0-revision-design.md`
- LuaRocks architectural spec: `docs/superpowers/specs/2026-05-08-luarocks-modules-design.md`
- SHI-176 Phase 3 (CLI): `docs/superpowers/specs/2026-05-10-luarocks-phase-3-design.md`
- SHI-176 Phase 4 (stub rocks + publish flow): `docs/superpowers/specs/2026-05-10-luarocks-phase-4-design.md`
- Repo + module refactor: `docs/superpowers/specs/2026-05-10-cookfile-and-cook-modules-refactor-design.md`
- Publishing pipeline memory: `project_cook_module_publishing.md`
- Module loader (register-side): `cli/crates/cook-register/src/module_loader.rs:181-260`
- Worker pool + per-unit package paths: `cli/crates/cook-luaotp/src/pool.rs:153-186, :565-620`
- `use` codegen: `cli/crates/cook-luagen/src/recipe.rs:397-415`
- Standard §6.3.4 init hook (reference impl): `cli/crates/cook-register/src/module_loader.rs:251-254` + test `test_load_module_init_runs_once_when_memoized`
- Standard §7 resolution order: `standard/src/content/docs/07-cross-cookfile-composition.mdx`
- Spec-first rule: `feedback_spec_first_no_bypass.md`
- Linear: SHI-133 (this M0), SHI-132 (epic), SHI-176 (modules subsystem)
