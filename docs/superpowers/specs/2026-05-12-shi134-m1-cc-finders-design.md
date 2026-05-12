# SHI-134 — M1: Cook-native finders for the well-known set (design)

**Ticket:** [SHI-134](https://linear.app/shiny-guru/issue/SHI-134) — parent [SHI-132](https://linear.app/shiny-guru/issue/SHI-132) (Cook builds Doom 3).
**Date:** 2026-05-12.
**Status:** design — supersedes the M1 section of `2026-05-01-cpp-module-roadmap-to-doom3-design.md` for SHI-134 execution.
**Predecessor:** M0 ([SHI-133](https://linear.app/shiny-guru/issue/SHI-133), Done 2026-05-11). `cook_cc-0.1.2-1` is live on rocks.usecook.com and Cook Standard §9.2 specifies the v0.1 cc surface.

## 1. Decisions recap

| # | Question | Decision |
|---|---|---|
| Q1 | Where do curated finders live? | Hybrid: inline in `cook_cc` as `cook_cc.finders.<name>` submodules + public `cc.register_finder(name, fn)` for project / third-party extension |
| Q2 | Release cadence for the curated set? | One rock release: `cook_cc 0.2.0-1` ships resolver + seven curated finders + register API + version constraint |
| Q3 | Frameworks support in M1 or split? | In M1. `LinkOpts.frameworks` (§9.2.3.7), transitive propagation (§9.2.4), no-op on non-macOS hosts |
| Q4 | Version constraint semantics? | Strategy-specific check; failure-to-satisfy = strategy miss; chain continues. Semver-style grammar with comma-AND |
| Q5 | Diagnostics shape? | Structured `FindResult.tried` list of attempts; `cc.find_or_error` convenience wrapper raises with formatted message |
| Q6 | Conformance mode? | Route A — busted spec coverage + parse-only Cookfile fixtures. Execute-mode harness deferred to [SHI-210](https://linear.app/shiny-guru/issue/SHI-210) |

## 2. Scope

**In scope:**

1. Generalize `cc.find` from single-strategy (pkg-config) to a four-stage chain: project-registered → curated → pkg-config → bare-lib probe.
2. Seven inline curated finders for `raylib`, `sdl2`, `openal`, `gl`/`opengl`, `threads`, `zlib`, `libcurl` (Linux + macOS).
3. `cc.register_finder(name, fn)` as a public Standard surface function.
4. Honour version constraints (Standard §9.2.3.8 v0.1 → v0.2 transition).
5. `frameworks` field on `LinkOpts` (§9.2.3.7) and on transitive propagation (§9.2.4).
6. `tried` field on `FindResult`; `cc.find_or_error(name, opts)` convenience.
7. New in-tree example `examples/raylib-game/` exercising `cc.find_or_error("raylib")` on Linux + macOS.

**Out of scope:**

- Building raylib from source (M3+).
- CMake-compat strategy (M2 / SHI-135 — `cmake --find-package` shim).
- Windows-host finders (post-Doom-3 sub-roadmap).
- Compile/link-based bare-probe (file-existence only; see §3).
- Execute-mode conformance harness (filed as [SHI-210](https://linear.app/shiny-guru/issue/SHI-210)).

**Shipped artifacts:**

- `cook_cc 0.2.0-1` on rocks.usecook.com.
- Cook Standard §9.2 v0.2 touching §9.2.3.7, §9.2.3.8, §9.2.3.12 (new), §9.2.3.13 (new), §9.2.4, §9.2.5.
- `examples/raylib-game/`.
- Parse-only conformance fixtures under `standard/conformance/positive/cc-find-*` and `cc-frameworks-*`.
- Behavioral test coverage extended in `cook-modules/cook_cc/spec/`.

## 3. Standard surface delta (v0.2)

### 3.1 §9.2.3.7 `cc.link` — `LinkOpts.frameworks`

Append to LinkOpts:

| Field | Type | Semantics |
|---|---|---|
| `frameworks` | list[string] | Each entry MUST emit `-framework <name>` at link time on macOS hosts. On non-macOS hosts the implementation MUST treat the field as a no-op. |

### 3.2 §9.2.3.8 `cc.find` — v0.2 normative rewrite

```
cc.find(name: string, opts: FindOpts?) -> FindResult
```

`FindOpts`:

| Field | Type | Semantics |
|---|---|---|
| `version` | string | A semver-style constraint (e.g., `">=4.0"`, `"=4.0.1"`, `">=2.0,<3.0"`). Honoured by every strategy that can determine the discovered package's version. Pre-release tags are excluded (`"4.0.0-rc1"` does NOT satisfy `">=4.0.0"`). |

A conforming v0.2 implementation MUST consult strategies in this order, first-match-wins:

1. **Project-registered finders** (§9.2.3.12). Tried first to preserve the project-level escape hatch.
2. **Curated finders** — implementation-provided. The reference implementation ships: `raylib`, `sdl2`, `openal`, `gl`/`opengl`, `threads`, `zlib`, `libcurl`. The Standard does NOT require any specific curated finder — implementations MAY ship a different set — but each curated finder MUST conform to the FindResult contract.
3. **pkg-config** — as v0.1.
4. **Bare-lib probe** — file-existence check for `lib<name>.{so,dylib,a}` on the host's default linker search paths. MUST be skipped if `opts.version` is set.

A strategy MAY report `outcome = "skip"` (the strategy could not be consulted) or `outcome = "miss"` (the strategy was consulted and rejected). Only `outcome = "hit"` populates `found = true` and the result fields.

`FindResult`:

| Field | Type | Semantics |
|---|---|---|
| `found` | boolean | `true` iff some strategy returned `outcome = "hit"`. |
| `cflags` | string | Compile flags from the winning strategy (empty on miss). |
| `libs` | string | Link flags from the winning strategy (empty on miss). |
| `system_libs` | list[string] | `-l<name>` libraries from the winning strategy. |
| `include_dirs` | list[string] | `-I<dir>` paths from the winning strategy. |
| `lib_dirs` | list[string] | `-L<dir>` paths from the winning strategy. |
| `frameworks` | list[string] | `-framework <name>` entries (macOS) from the winning strategy. |
| `version` | string \| nil | Version detected by the winning strategy. |
| `tried` | list[Attempt] | Ordered list of every strategy consulted, including the winner on hits. |

`Attempt`:

| Field | Type | Semantics |
|---|---|---|
| `strategy` | string | One of `"project:<name>"`, `"curated:<name>"`, `"pkg-config"`, `"bare-probe"`, or a third-party-namespaced form (e.g., `"rocks.cook_cc_vulkan:vulkan"`). |
| `outcome` | string | `"hit"`, `"miss"`, or `"skip"`. |
| `reason` | string | Human-readable explanation. MAY be empty on `"hit"`. |
| `hint` | string \| nil | Install hint. Curated finders SHOULD emit on miss; others SHOULD NOT. |

Caching: results MUST be cached in `cook.cache` keyed by `"cc.find:<name>:<canonical-opts>"`. Repeat calls within an invocation MUST NOT re-consult the chain.

### 3.3 §9.2.3.12 `cc.register_finder` (new)

```
cc.register_finder(name: string, finder: function(opts) -> FindResult)
```

Registers a project-scoped finder for `name`. Subsequent `cc.find(name, ...)` calls MUST consult `finder` before the curated/pkg-config/bare-probe stages. The function MUST return a FindResult; the implementation MAY discard the function's `tried` field. Re-registration replaces (no warning) — enables config-block overrides.

### 3.4 §9.2.3.13 `cc.find_or_error` (new)

```
cc.find_or_error(name: string, opts: FindOpts?) -> FindResult
```

Calls `cc.find(name, opts)`. If `result.found` is `true`, returns. Otherwise raises at level 2 prefixed `[cc.find_or_error]`. The error message MUST list every Attempt in `result.tried` and MUST include any `hint` fields. The only function in §9.2 that raises on a missing package.

### 3.5 §9.2.4 Transitive propagation

Extend the propagated info record:

- `info.frameworks` — `-framework <name>` entries the consumer adds at link time (macOS).

Closure walk MUST merge `frameworks` analogously to `system_libs` (dedup-preserving order).

### 3.6 §9.2.5 Error model — diagnostic catalogue additions

| Function | Condition | Message |
|---|---|---|
| `cc.find_or_error` | `result.found == false` | `could not locate '<name>'<version-suffix>:\n  - <strategy>: <outcome> (<reason>)...\n<hints>` |
| `cc.register_finder` | `finder` is not a function | `register_finder for '<name>' requires a function, got <type>` |

`cc.find` continues to NOT raise on miss.

### 3.7 App. D changelog

`CS-0067: cc finder multi-strategy chain (M1)` — captioning the §9.2.3.7 LinkOpts addition, §9.2.3.8 v0.2 rewrite, §9.2.3.12 + §9.2.3.13 introduction, §9.2.4 frameworks addition, §9.2.5 catalog growth.

## 4. Resolver internals

### 4.1 Chain dispatch

```lua
function M.find(name, opts)
    opts = opts or {}
    local cache_key = "cc.find:" .. name .. ":" .. canonical_opts(opts)
    local cached = cook.cache.get(cache_key)
    if cached then return cached end

    local tried = {}
    local hit = nil

    for _, strategy in ipairs(resolver.chain(name, opts)) do
        local attempt = strategy(name, opts)
        tried[#tried + 1] = attempt
        if attempt.outcome == "hit" then hit = attempt; break end
    end

    local result = build_result(hit, tried)
    cook.cache.set(cache_key, result)
    return result
end
```

`resolver.chain(name, opts)` resolves the strategy list at call time, so a project-registered finder registered after import but before the recipe is consulted.

### 4.2 Cache key canonicalization

`canonical_opts(opts)` serializes opts deterministically: keys sorted lexicographically, values stringified, list-valued opts joined with `,`. `{}` and `nil` produce the same canonical form (empty string), so `cc.find("raylib")` and `cc.find("raylib", {})` hit the same cache entry.

In M1 only `version` is recognized; the canonicalization scheme is forward-stable for future opts like `prefer` ("static"/"shared").

### 4.3 Strategy internal contract

Each strategy is `(name, opts) -> Attempt`. Hits carry a `payload` table the resolver consumes to build the FindResult; misses/skips have no payload.

```lua
{
    strategy = "curated:raylib",
    outcome  = "hit",
    reason   = "",
    hint     = nil,
    payload  = {
        cflags       = "-I/usr/include/raylib",
        libs         = "-lraylib -lm -ldl -lpthread",
        system_libs  = { "raylib", "m", "dl", "pthread" },
        include_dirs = { "/usr/include/raylib" },
        lib_dirs     = {},
        frameworks   = {},
        version      = "4.5.0",
    },
}
```

### 4.4 Bare-probe specifics

Last-stage fallback. Algorithm:

1. Build the search-path list once per invocation: `/usr/lib`, `/usr/local/lib`, plus the parsed `libraries:` line from `cc -print-search-dirs`. Cached in `cook.cache` under `"cc.linker-search-dirs"`, key includes the compiler driver path so toolchain changes invalidate.
2. Check whether any of `lib<name>.so`, `lib<name>.dylib`, `lib<name>.a` exists on those paths.

| Condition | Attempt |
|---|---|
| `opts.version` set | `{outcome="skip", reason="bare probe cannot verify version constraints"}` |
| File found | `{outcome="hit", payload={system_libs={name}, libs="-l"..name, ...}}` |
| Nothing found | `{outcome="miss", reason="no lib"..name..".{so,dylib,a} on default linker search paths"}` |

### 4.5 Module file layout

```
cook_cc/
  init.lua             facade + M.register_finder = resolver.register
  finder.lua           resolver shell + dispatch (~80 lines)
  finders/
    init.lua           curated registry + alias table
    pkg_config.lua     shared helper + main-chain strategy
    bare_probe.lua     shared helper + main-chain strategy
    raylib.lua
    sdl2.lua
    openal.lua
    gl.lua             includes alias "opengl"
    threads.lua
    zlib.lua
    libcurl.lua
  version.lua          semver parser + constraint checker
```

`finder.lua` is the resolver shell; curated finders ~30–60 lines each. All curated finders `require("cook_cc.version")` for constraint checks.

### 4.6 `register_finder` storage

Per-name table of project-registered finders inside the worker VM, stored at `package.loaded["cook_cc.finder"]._registry`. Deliberately not in `cook.cache` because it's a Lua function value, not a serializable payload. Standard §6's one-VM-per-recipe-execution model means the registry's lifetime matches one recipe's scope — exactly the override semantics intended.

## 5. Curated finder catalog

**Layering.** Curated finders own the per-library knowledge of *which sub-strategy is right per host*. A curated finder for `zlib` on Linux internally calls `pkg_config.try("zlib")`; the curated `openal` finder on macOS skips pkg-config entirely and returns `frameworks={"OpenAL"}` directly. The main-chain pkg-config strategy is the fallback for libs that have *no* curated finder.

**Shared helpers** (under `cook_cc/finders/`):

- `pkg_config.try(name) -> Attempt?`
- `bare_probe.try(name) -> Attempt?`
- `header_probe.parse_define(path, macro) -> string?`
- `tool_config.try(tool, args) -> string?`

**The seven:**

| Finder | Linux | macOS | Version | Install hint |
|---|---|---|---|---|
| **raylib** | `pkg_config.try` → fallback `bare_probe.try` + parse `raylib.h` `RAYLIB_VERSION` | `pkg_config.try`; post-process to add `frameworks={"OpenGL","Cocoa","IOKit","CoreVideo","CoreAudio"}` when missing from `Libs.private` | `--modversion` or header macro | `apt: libraylib-dev / brew: raylib` |
| **sdl2** | `tool_config.try("sdl2-config", ...)` → `pkg_config.try("sdl2")` | Same; `sdl2-config --libs` emits `-framework SDL2` on macOS Framework installs and `-lSDL2` on dylib installs | `sdl2-config --version` | `apt: libsdl2-dev / brew: sdl2` |
| **openal** | `pkg_config.try("openal")` (openal-soft) → `bare_probe.try("openal")` | Return `{frameworks={"OpenAL"}}` directly; no probe (system framework) | `pkg-config --modversion` on Linux; nil on macOS | `apt: libopenal-dev / macOS system framework / brew: openal-soft` |
| **gl / opengl** | `pkg_config.try("gl")` → `bare_probe.try("GL")` (mesa ships `libGL.so`) | Return `{frameworks={"OpenGL"}}` directly | nil on both | `apt: libgl-dev / macOS system framework` |
| **threads** | Return `{cflags="-pthread", libs="-pthread"}` | Return `{found=true}` with empty fields | N/A; `opts.version` set → `skip` | N/A (system primitive) |
| **zlib** | `pkg_config.try("zlib")` → `bare_probe.try("z")` + parse `zlib.h` `ZLIB_VERSION` | `pkg_config.try("zlib")` → `bare_probe.try("z")` | `--modversion` or header macro | `apt: zlib1g-dev / macOS system / brew: zlib` |
| **libcurl** | `tool_config.try("curl-config", ...)` → `pkg_config.try("libcurl")` | Same; `curl-config` in Xcode CLT / homebrew | `curl-config --version` | `apt: libcurl4-openssl-dev / macOS system / brew: curl` |

**Aliases.** `cc.find("opengl")` routes to the `gl` curated finder. Attempt's `strategy` field reports the canonical name (`curated:gl`).

**Skip semantics.** A curated finder MAY return `skip` (not `miss`) when version-undetectable (threads, gl) or platform-structurally-unsupported (e.g., a Linux-only finder called on Windows). Both let the chain continue; the distinction surfaces in `tried` and is visible in `cc.find_or_error`'s error message.

## 6. Version constraint mechanics

### 6.1 Grammar

```
constraint   ::= clause (',' clause)*
clause       ::= operator? version
operator     ::= '>=' | '>' | '<=' | '<' | '='
version      ::= digit+ ('.' digit+)? ('.' digit+)?
```

Missing operator defaults to `=`. Comma is AND. No OR. No LuaRocks `~>`.

### 6.2 Parser (`cook_cc/version.lua`)

`M.parse(s) -> { major, minor, patch, prerelease } | nil`. Missing minor/patch default to 0. Pre-release tags after `-` captured; build metadata after `+` dropped. Non-numeric major → `nil` (caller treats as "version detection failed").

### 6.3 `M.satisfies(detected, constraint) -> boolean`

1. Parse `constraint` into clauses. Unparseable → error at level 2 (`[cc.find]`).
2. Parse `detected`. Unparseable → return `false`.
3. Per-clause: `=` exact (missing fields = wildcard), comparison operators zero-fill missing fields.
4. Pre-release exclusion: detected pre-release fails `>=`/`>`/`<=`/`<` against a non-pre-release constraint.
5. AND across clauses.

### 6.4 "Cannot determine version" interactions

| Situation | Resolver behavior |
|---|---|
| `payload.version = nil` AND `opts.version = nil` | Hit; `result.version = nil` |
| `payload.version = nil` AND `opts.version` set | Strategy returns `miss` (transient failure) or `skip` (structural — threads, gl); reason mentions version-undetectable |
| `payload.version = "X"` AND constraint unsatisfied | `miss` with `reason = "detected version X does not satisfy <constraint>"` |

`miss` vs `skip` distinction: `skip` = wrong tool for this query; `miss` = right tool, failed. Both let the chain continue; both visible in `tried`.

### 6.5 Edge cases

- `version = ""` → empty clause list → vacuously satisfied (equivalent to no constraint).
- `version = ">=4"` → zero-fills to `>=4.0.0`.
- `version = "<4"` → strict; matches nothing in 4.x.
- Whitespace inside the string tolerated.

## 7. Frameworks plumbing

### 7.1 `cc.link` emission

Insertion point: after `system_libs`, before `extra_ldflags` (so users can still override via `-weak_framework` in `extra_ldflags`).

```lua
if cook.platform.os == "macos" then
    for _, fw in ipairs(opts.frameworks or {}) do
        parts[#parts + 1] = "-framework"
        parts[#parts + 1] = fw
    end
end
```

Each framework is two CLI tokens; today's `cc.link` joins parts with single spaces, producing the correct `-framework <name>` form. Non-macOS hosts silently ignore.

### 7.2 Target makers

`cc.bin / cc.lib / cc.shared` accept `frameworks` symmetric to `system_libs`; flows through to the `LinkOpts` table assembled for `cc.link`. `cc.headers` does not emit a link unit; `frameworks` there only affects transitive propagation.

### 7.3 Transitive propagation

`cook.export(name, info)` info record grows `frameworks`. Closure walk in `transitive.collect(links)` adds `frameworks` to the dedup-preserving-first-seen-order field list (uniform with `system_libs`).

### 7.4 Observed chain

```cook
recipe game
    > local raylib = cook_cc.find_or_error("raylib")
    > cook_cc.bin("game", {
    >     sources     = { "src/main.c" },
    >     system_libs = raylib.system_libs,
    >     frameworks  = raylib.frameworks,
    > })
end
```

On macOS `raylib.frameworks = {"OpenGL","Cocoa","IOKit","CoreVideo","CoreAudio"}`; on Linux `raylib.frameworks = {}` and `raylib.system_libs` carries the load. Same target_def works everywhere.

### 7.5 Non-goal

`-F /path/to/frameworks` (framework search path) is not in M1. Curated finders return system frameworks; default search paths cover `/System/Library/Frameworks` and `/Library/Frameworks`. Deferred until a real case appears.

## 8. Conformance and test plan

### 8.1 Three layers

| Layer | Where | What |
|---|---|---|
| Behavioral | `cook-modules/cook_cc/spec/` (busted) | Chain semantics + per-curated finder |
| Surface | `cook/standard/conformance/positive/cc-find-*` | Public surface parses + binds |
| Integration | `cook/examples/raylib-game/` | End-to-end build on Linux + macOS |

### 8.2 Busted spec additions

New files under `cook-modules/cook_cc/spec/`:

```
spec/
  finder_spec.lua           (existing — reduced to integration)
  finders/
    resolver_spec.lua       chain order, caching, attempt schema
    version_spec.lua        semver parser + satisfies()
    pkg_config_spec.lua     (extracted from finder_spec)
    bare_probe_spec.lua     file-existence + skip-on-version
    raylib_spec.lua         one per curated finder...
    sdl2_spec.lua
    openal_spec.lua
    gl_spec.lua             includes alias test for "opengl"
    threads_spec.lua        includes skip-on-version
    zlib_spec.lua
    libcurl_spec.lua
```

Mocking via the existing `spec/cook_stub.lua`, extended with `set_pkg_config_response`, `set_tool_config_response`, `set_file_exists`, and platform-OS override. Roughly 80 new tests on top of the existing 41. Runs in < 2 seconds.

### 8.3 Parse-only Cookfile fixtures

Six new fixtures under `standard/conformance/positive/`, each locking one Standard surface element:

| Fixture | Surface |
|---|---|
| `cc-find-version-constraint` | §9.2.3.8 FindOpts.version (v0.2 honoured) |
| `cc-find-or-error` | §9.2.3.13 cc.find_or_error |
| `cc-find-tried-field` | §9.2.3.8 FindResult.tried |
| `cc-register-finder` | §9.2.3.12 cc.register_finder |
| `cc-frameworks-on-link` | §9.2.3.7 LinkOpts.frameworks |
| `cc-frameworks-transitive` | §9.2.4 frameworks in propagation |

Existing `cc-find-miss-behaviour` from M0 is reused as-is for the no-args path.

### 8.4 Integration example

```
examples/raylib-game/
  cook.toml              pins cook_cc 0.2.0-1, indexes=[rocks.usecook.com]
  cook.lock              from `cook modules install`
  cook_modules/          populated by install
  Cookfile               cc.find_or_error + cc.bin
  src/main.c             raylib's core_basic_window.c (BSD, attributed)
  README.md              install hint per OS
```

Gate-m2 matrix gains a step that pre-installs raylib (`apt install -y libraylib-dev` on Linux; `brew install raylib` on macOS) and runs `cook game`.

### 8.5 Deferred follow-up

Execute-mode harness filed as [SHI-210](https://linear.app/shiny-guru/issue/SHI-210). Scope: execute-fixture file layout, harness implementation, output normalization, retroactive upgrade of M0 + M1 cc-* fixtures. Parent SHI-132. Strong forcing function from M2 and M3.

## 9. Release plan

### 9.1 Sequence

```
1. PR cook#A  (draft)             §9.2 v0.2 spec text + new parse-only conformance
                                  fixtures only.

2. PR cook-modules#B              Resolver + 7 curated finders + register API
                                  + version.lua + extended busted suite
                                  + rockspec 0.2.0-1.

3. Tag + publish                  cook-modules tag cook_cc-0.2.0-1.
                                  cook-rocks-index PR renders the new rock.
                                  Cloudflare Pages serves within minutes.

4. PR cook#A flips draft → ready  Add commits: pin lua-build, fzf-picker,
                                  cpp-project to 0.2.0-1 (regen cook.lock);
                                  add examples/raylib-game/.

5. PR cook#A merge                Pre-merge: gate-m2 matrix green on Linux + macOS;
                                  standard.build asserts no new slug regressions;
                                  conformance suite green; busted green.

6. SHI-134 closeout comment       Mirror M0 closeout shape.
```

### 9.2 Why split-then-merge

- Reviewers read the Standard delta in isolation, before the implementation exists.
- Pin + example commits depend on a publishable rock, which depends on PR cook-modules#B; holding PR cook#A in draft prevents a merged Standard that lies about deployable behavior.
- One reviewable squash-merge captures "Standard §9.2 v0.2 + cook_cc 0.2.0 + examples" — clean for `git bisect`.

### 9.3 Pre-merge gates

| Gate | Verifies |
|---|---|
| `cargo test -p cook-lang --test conformance` | Parse-only cc-* fixtures green |
| `busted` in `cook-modules/cook_cc/` | Behavioral spec (~120 tests total) |
| `cook game` in `examples/raylib-game/` | End-to-end on Linux + macOS via gate-m2 |
| `standard.build` (Astro) | No new slug regressions (pre-existing E-pre-v1-checklist warning tracked as [SHI-212](https://linear.app/shiny-guru/issue/SHI-212)) |

### 9.4 Versioning of the Standard

§9.2.3.8 wording transitions from "A conforming v0.1 implementation MUST use pkg-config..." to "A conforming v0.2 implementation MUST consult strategies in this order...". The v0.1 wording is retained as a historical paragraph inside App. B rationale so anyone bisecting against cook_cc-0.1.2 still has a referenceable spec.

## 10. Risks

| Risk | Mitigation |
|---|---|
| `pkg-config --modversion` format varies (prerelease tags, dates) | Treat unparseable detected version as `nil`; curated finders use their own version path when canonical is known better |
| macOS `OpenAL.framework` deprecation warning at link time | Expected; not actionable in M1. raylib-game README mentions it |
| `bare_probe` paths from `cc -print-search-dirs` are compiler-specific | Cache key includes compiler driver path; invalidates on toolchain change |
| `sdl2-config` not on PATH on macOS without homebrew | Curated sdl2 falls back to pkg-config; install hint reads `brew install sdl2` |
| Cookfile authors expect `cc.find("openssl")` to work without curated finder | Documented: pkg-config handles uncurated libs uniformly; curated finders fill specific gaps |

## 11. Out of scope, tracked elsewhere

| Item | Ticket |
|---|---|
| Execute-mode conformance harness | [SHI-210](https://linear.app/shiny-guru/issue/SHI-210) |
| Bundled luarocks 3.11 dual-server bug | [SHI-211](https://linear.app/shiny-guru/issue/SHI-211) |
| `E-pre-v1-checklist.mdx` `§{exec.execute-phase}` slug warning | [SHI-212](https://linear.app/shiny-guru/issue/SHI-212) |
| CMake-compat `find_package` strategy | [SHI-135](https://linear.app/shiny-guru/issue/SHI-135) (M2) |
| Vendor + build raylib from source | [SHI-136](https://linear.app/shiny-guru/issue/SHI-136) (M3) |
| Persistent linker dispatch — `cc.link` driver invocation | [SHI-204](https://linear.app/shiny-guru/issue/SHI-204), [SHI-205](https://linear.app/shiny-guru/issue/SHI-205); independent of M1 |
