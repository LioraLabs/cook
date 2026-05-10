# SHI-176 Phase 3 — `cook modules` CLI

Date: 2026-05-10
Status: Design — pending implementation plan
Linear: SHI-176 epic, M3.1–M3.8 sub-tickets (to be filed by the implementation plan)
Parent specs:
- `2026-05-08-luarocks-modules-design.md` — architectural design (what we are building)
- `2026-05-09-luarocks-modules-decomposition-design.md` — Phase 1–3 decomposition and dispatch protocol
- `2026-05-09-luarocks-phase-2-design.md` — Phase 2 (bundle Lua + LuaRocks) operational design
Companion explainer: `docs/architecture/dynamic-linking-and-rocks.md` (runtime mechanics primer)

This document specifies Phase 3 in operational detail: how `cook modules` is wired into the CLI, how the manifest and lockfile interact with the bundled LuaRocks, where runtime `package.path` / `package.cpath` extension lives, and what publishes to `rocks.usecook.com` to validate the pipeline end-to-end.

## Context

Phase 2 (M2) shipped 2026-05-10: bundled Lua 5.4.7 sources, vendored luarocks 3.11.0, `-rdynamic` / `-Wl,-export_dynamic` linker flags, the `gate-m2` chore proving lua-cjson dlopens against cook's flat-namespace exports on linux-x86_64 and darwin-arm64, and the `cook-xtask` retirement (replaced by `chore "package"`). Phase 2 also surfaced SHI-188 (cook.exec error opacity), now fixed (commit `7fbbbb2`); chore bodies and Rust subprocess callers can rely on captured-stream inlining at failure time.

Phase 3 (M3) ships the user-facing `cook modules` subsystem on top of Phase 2's bundled LuaRocks: manifest parsing, lockfile generation, the subprocess driver wrapping `~/.cook/bin/luarocks`, the clap CLI surface, runtime `package.path` / `package.cpath` extension, and the Standard §7 amendment that normatively documents the search-path order.

### Two amendments to the parent design

The parent architectural spec is the architectural reference; two decisions made during this brainstorm refine its `[modules]` table treatment:

- **Single rock-name namespace; no bare-name shorthand.** The parent spec proposed two namespaces in `[modules]` — bare short names routed through Cook's index, full rock names addressing luarocks.org directly. Phase 3 collapses this to a single namespace: rock names. Cookfile authors write `use cook_cpp` directly (the rock provides a Lua module named `cook_cpp`). Aliasing is opt-in via `use cook_cpp as cpp`.
- **Underscore separators for blessed modules.** Cook's blessed module rocks use `cook_smoke`, `cook_cpp`, `cook_rust`, etc. — never `cook-smoke`. The underscore form is simultaneously a valid Lua identifier, a valid bare TOML key (no quoting required), and a valid LuaRocks rock name. Hyphenated luarocks.org rocks (`lua-cjson`, `lua-resty-http`) install fine but require `use lua-cjson as cjson` for the binding.

Both amendments reduce the surface area of Standard §6 (`use`-env): no auto-mangling rules, no two-namespace resolution. These decisions are documented here and supersede the parent spec's `[modules]` table example.

## Scope

Phase 3 ships these slices, each one Linear sub-ticket under SHI-176:

- **M3.1** — `cook.toml [modules]` table parsing + `[registry].indexes` array.
- **M3.2** — `cook.lock` format (full-closure, including transitive rocks) + read/write + integrity verification + closure introspection over `cook_modules/lib/luarocks/rocks-5.4/`.
- **M3.3** — LuaRocks subprocess driver (`RocksDriver`) wrapping `~/.cook/bin/luarocks`; bare `Command` invocations; passthrough error surfacing inheriting SHI-188's stream capture.
- **M3.4** — `cook modules install/remove/update/list/search` clap subcommand surface; integrates M3.1, M3.2, M3.3.
- **M3.5** — Runtime `package.path` / `package.cpath` extension in `cli/crates/cook-luaotp/src/pool.rs`; renames `refresh_package_path` → `refresh_package_search_paths`; retires Phase 2's manual cpath prepend in the gate-m2 fixture.
- **M3.6** — Standard §7 search-path-order clause + App. B rationale paragraph + App. D CS entry; **co-lands in the same PR as M3.5** per the spec-first rule.
- **M3.7** — `rocks.usecook.com` resolver wiring (default `[registry].indexes` constants).
- **M3.8** — `cook_smoke` throwaway rockspec + Lua source + publish to rocks.usecook.com; M3 acceptance fixture.

Out of scope (deferred to Phase 4):

- **`cook pull` deletion.** The legacy pull subsystem (`cli/crates/cook-cli/src/pull/`, ~2000 LoC) and the legacy `[registry].url` field stay intact through Phase 3. Phase 3 introduces the new `[registry].indexes` array next to the legacy `url`. Phase 4 deletes pull and the legacy field atomically.
- **Blessed module rock migration.** Authoring `cook_cpp`, `cook_rust`, `cook_pnpm`, `cook_ai` rockspecs from existing modules and publishing them to rocks.usecook.com is Phase 4 work. Phase 3's `cook_smoke` is the validation fixture; it does not subsume the migration.
- **Phase 5 Windows packaging.** `lua54.dll` packaging, mlua external-Lua mode on Windows, MSI, MSVC documentation. Phase 3 is Linux + macOS only.
- **Authentication and private registries.** v1 ships against public indexes only; token/SSH support is later work.

## Approach: Rust-side machinery, Cookfile-validated

Phase 2 was Cookfile-driven (every Phase 2 build step is a chore in the repo-root `Cookfile`). Phase 3 is the opposite: every Phase 3 deliverable is a Rust-side change to `cli/crates/cook-cli/` (manifest, lockfile, driver, clap surface) plus a per-unit Lua-state setup change in `cli/crates/cook-luaotp/`. The validation surface is Cookfile-side — the M3 acceptance fixture is a Cookfile project that exercises the whole pipeline.

This is intentional: `cook modules` is a CLI subsystem, not a build pipeline. It needs to live in the same crate as the rest of the cook subcommands (clap dispatch, error surfacing, output formatting). The Phase 2 model — chores composing chores — does not fit a CLI subsystem with persistent state on disk.

### Module structure for the new subsystem

Per `CLAUDE.md`'s module-structure rules:

```
cli/crates/cook-cli/src/modules/
├── mod.rs              # facade: pub mod declarations + re-exports
├── manifest.rs         # ManifestModules, ManifestRegistry; cook.toml parsing
├── lockfile.rs         # Lockfile, LockedModule; cook.lock read/write/introspect
├── driver.rs           # RocksDriver; ~/.cook/bin/luarocks subprocess wrapper
└── cli.rs              # clap subcommand wiring (also edits parent cli.rs)
```

`mod.rs` is a facade. Cross-module imports in the rest of the cook codebase go through `cli::modules::*` re-exports — never reach into `modules::driver::internal_thing`. Shared types between Phase 3 and the rest of the engine (e.g., `LockedModule` consumed by future install-time hooks) live in `cli/crates/cook-contracts/` if and when that need arises; v1 keeps everything in `cook-cli`.

## M3.1 — Manifest parsing

### Schema additions to `cook.toml`

```toml
[registry]
url = "..."                                    # legacy (Phase 1) — Phase 3 leaves untouched
indexes = [                                    # NEW Phase 3
  "https://rocks.usecook.com",
  "https://luarocks.org",
]

[modules]                                      # NEW Phase 3
cook_smoke  = "*"
"lua-cjson" = "2.1.*"
argparse    = ">=0.7"
```

`[modules]` is a flat TOML table; each entry is `<rock-name> = "<luarocks-version-constraint>"`. Rock names are whatever luarocks accepts (alphanumeric + `_` + `-` + `.`). Quoted keys are required only when the name contains characters TOML reserves (e.g., `-` in `"lua-cjson"`). Constraints pass through verbatim to luarocks; Cook does not invent grammar and does not validate beyond non-empty string.

### Index precedence

`[registry].indexes` is a new array distinct from legacy `[registry].url`. Left-to-right precedence: the first index that has a rock matching the requested name wins. The `--registry <url>` CLI flag (M3.4) prepends to the configured `indexes` for one invocation.

Default `[registry].indexes` (when `cook.toml` omits the field) is:

```rust
["https://rocks.usecook.com", "https://luarocks.org"]
```

baked into M3.7's `ManifestRegistry::default()` rather than written into the user's `cook.toml`. Empty `indexes = []` falls through to the same default.

### Public types

```rust
// cli/crates/cook-cli/src/modules/manifest.rs

pub struct ManifestModules {
    pub modules: BTreeMap<String, String>,  // rock name -> constraint
}

pub struct ManifestRegistry {
    pub indexes: Vec<String>,
}

impl ManifestRegistry {
    pub fn default() -> Self { /* M3.7 wires the constants */ }
}

pub fn parse_cook_toml(path: &Path) -> Result<(ManifestModules, ManifestRegistry)>;
```

`BTreeMap` not `HashMap` per the project's deterministic-output rule.

### Slice boundary

M3.1 owns:
- `cli/crates/cook-cli/src/modules/manifest.rs` (new file)
- `cli/crates/cook-cli/src/modules/mod.rs` (new file; declares the manifest module)
- Tests at `cli/crates/cook-cli/src/modules/manifest_tests.rs` (or inline `#[cfg(test)] mod tests`)

M3.1 does *not* touch `cli/crates/cook-cli/src/pull/config.rs` (pull keeps its own copy of registry parsing through Phase 3) and does *not* import from `cli::modules::driver` (M3.3) or `cli::modules::cli` (M3.4).

### Acceptance for M3.1

- Positive parsing: mixed-name `[modules]` table; both `[registry]` forms (legacy `url` + new `indexes`) present; `[registry]` omitted entirely; `[modules]` empty; `[modules]` absent.
- Negative parsing: malformed TOML; non-string constraint values; `[registry].indexes` containing non-URL strings (relaxed: pass through to driver, let driver fail with a clear error).
- Constraint round-trip: every value the user wrote is preserved byte-for-byte in `ManifestModules.modules`.
- `ManifestRegistry::default()` returns the documented defaults when `cook.toml` omits the field.

## M3.2 — Lockfile format

### File shape

```toml
# cook.lock — generated, commit this.
schema = 1

[[module]]
name      = "cook_smoke"
version   = "0.1.0-1"
source    = "https://rocks.usecook.com/cook_smoke-0.1.0-1.src.rock"
integrity = "sha256-1f3d…"
direct    = true

[[module]]
name      = "lua-cjson"
version   = "2.1.0.10-1"
source    = "https://luarocks.org/manifests/openresty/lua-cjson-2.1.0.10-1.src.rock"
integrity = "sha256-9b2c…"
direct    = true

[[module]]
name      = "luafilesystem"
version   = "1.8.0-1"
source    = "https://luarocks.org/manifests/hisham/luafilesystem-1.8.0-1.src.rock"
integrity = "sha256-7a44…"
direct    = false
```

`direct = true` for rocks named in `[modules]`; `direct = false` for transitives picked up by luarocks's own dep resolver. The boolean lets `cook modules remove` know which transitive deps become orphans when a top-level rock is removed.

### Closure depth

Full closure: top-level deps + every transitive rock luarocks installed. Reproducibility means another machine running `cook modules install` with the same `cook.lock` fetches the *exact* same set of rocks at the *exact* same versions, regardless of upstream movement on luarocks.org.

This matches Cargo / pnpm / Bundler. The cost is a bigger lockfile; the win is determinism across days, machines, and luarocks-side updates.

### Generation

After every state-changing `cook modules` invocation (`install <name>`, `install` with no lockfile, `update`, `update <name>`, `remove`), the orchestrator (M3.4):

1. Invokes the driver (M3.3) to perform the install/remove/update operation.
2. Calls `RocksDriver::list_installed()` (M3.3 primitive) to enumerate every rock present under `cook_modules/lib/luarocks/rocks-5.4/`.
3. For each enumerated rock, reads the rock's manifest at `cook_modules/lib/luarocks/rocks-5.4/<name>/<version>/rock_manifest` and the rockspec at `<name>-<version>.rockspec` to extract `source.url`.
4. Computes the SHA-256 of the cached source rock at `cook_modules/lib/luarocks/cache/<name>-<version>.src.rock`.
5. Marks `direct = true` for rocks whose names appear in `ManifestModules.modules`, else `direct = false`.
6. Writes the resulting `Lockfile` value via M3.2's serializer.

### Consumption

Plain `cook modules install` (no args) reads `cook.lock` if present, validates each entry's integrity against the on-disk source-rock cache, and feeds the closure to the driver via `RocksDriver::install_locked(&LockedModule)` per entry. Luarocks's own dep resolver is bypassed in this path — the closure is already pinned. Network is consulted only to fetch rocks whose source-rock cache is missing.

If the lockfile is inconsistent with `cook.toml` (a rock named in `[modules]` is missing from the lockfile, or vice versa), `cook modules install` errors with a clear diagnostic and a suggested next step (`cook modules install <name>` or `cook modules update`).

State-changing invocations (any `install` with explicit args, `update`, `update <name>`, `remove`) regenerate the lockfile.

### Schema versioning

`schema = 1` is required at the top. Mismatches surface a clear error: `cook.lock schema version N is newer than this cook supports; upgrade cook`. No auto-migration in v1; future schema bumps are opt-in via a `cook modules migrate-lock` (or similar) and are out of Phase 3 scope.

### Public types

```rust
// cli/crates/cook-cli/src/modules/lockfile.rs

pub struct Lockfile {
    pub schema: u32,
    pub modules: Vec<LockedModule>,
}

pub struct LockedModule {
    pub name: String,
    pub version: String,
    pub source: String,
    pub integrity: String,  // "sha256-<base64>" form
    pub direct: bool,
}

pub fn read(path: &Path) -> Result<Lockfile>;
pub fn write(path: &Path, lock: &Lockfile) -> Result<()>;
pub fn verify_integrity(locked: &LockedModule, cache_dir: &Path) -> Result<()>;
pub fn introspect_closure(modules_dir: &Path, manifest: &ManifestModules) -> Result<Lockfile>;
```

`introspect_closure` is the post-install pass — given a populated `cook_modules/` and the manifest (to determine `direct` flags), it produces a `Lockfile`. M3.4 calls it after every state-changing driver invocation.

### Slice boundary

M3.2 owns `cli/crates/cook-cli/src/modules/lockfile.rs` plus tests. Does *not* touch `manifest.rs` (M3.1) or `driver.rs` (M3.3). The closure introspection logic *consumes* the rock-tree state produced by M3.3's driver and is *called* from M3.4's orchestration loop.

### Acceptance for M3.2

- Round-trip serialization: a `Lockfile` value written then read produces a byte-identical `Lockfile`.
- Integrity verification: correct hash → ok; wrong hash → error naming the rock and both expected/actual hashes.
- Schema mismatch: reading a lockfile with `schema = 99` errors clearly.
- `direct: bool` is preserved across round-trip and correctly inferred by `introspect_closure` against a fixture manifest.
- `introspect_closure` against a fixture rock tree (committed under `cli/crates/cook-cli/tests/fixtures/`) produces the expected closure — including transitives marked `direct = false`.

## M3.3 — LuaRocks subprocess driver

### Public surface

```rust
// cli/crates/cook-cli/src/modules/driver.rs

pub struct RocksDriver {
    prefix: PathBuf,        // ~/.cook (or override for tests)
    indexes: Vec<String>,   // resolved with --registry override applied
    project_dir: PathBuf,   // the Cookfile project root; cook_modules/ lives here
}

pub struct InstalledRock {
    pub name: String,
    pub version: String,
    pub rockspec_path: PathBuf,
    pub cached_source_rock: Option<PathBuf>,
}

pub struct SearchHit {
    pub name: String,
    pub version: String,
    pub index: String,  // which index produced this hit
}

impl RocksDriver {
    pub fn new(prefix: PathBuf, indexes: Vec<String>, project_dir: PathBuf) -> Self;
    pub fn install(&self, name: &str, constraint: &str) -> Result<()>;
    pub fn install_locked(&self, locked: &LockedModule) -> Result<()>;
    pub fn remove(&self, name: &str) -> Result<()>;
    pub fn update(&self, name: Option<&str>, manifest: &ManifestModules) -> Result<()>;
    pub fn search(&self, query: &str) -> Result<Vec<SearchHit>>;
    pub fn list_installed(&self) -> Result<Vec<InstalledRock>>;
}
```

### Invocation pattern

Every method that talks to luarocks builds a `std::process::Command` of the form:

```
~/.cook/bin/luarocks --tree <project_dir>/cook_modules <subcmd> [--server <url>]... [args]
```

`--tree` ensures rocks land in the project's `cook_modules/`, not in a user-global luarocks tree. `--server <url>` is repeated once per index in `self.indexes`, in order; luarocks tries them left-to-right.

For `install_locked` (lockfile-replay path), the driver passes the pinned source URL directly via `luarocks install <source-url>` rather than letting luarocks re-resolve. This bypasses luarocks's own resolver and guarantees the lockfile's pinned closure is what lands on disk.

### Error surfacing

Bare `Command::output()`. On non-zero exit, the driver returns an `anyhow::Error` whose Display includes:

- The full argv as a quoted string.
- The captured stdout (truncated at 8KB with a `... (N more bytes)` marker, matching `cook.exec`'s SHI-188 cap).
- The captured stderr (same 8KB cap).
- The exit status.

No structured parsing of luarocks output; no typed error variants beyond `LuarocksFailed { argv, stdout, stderr, status }`. The user sees luarocks's own diagnostic verbatim. This matches the SHI-188 model on the chore-body side (`cook.exec` failures inline captured streams) and avoids the brittleness of pattern-matching luarocks output.

### Lockfile coordination

The driver does *not* read or write `cook.lock`. Lockfile management is M3.4's orchestration responsibility. The driver exposes `list_installed()` as the introspection primitive M3.4 calls after each state-changing operation.

### Slice boundary

M3.3 owns:
- `cli/crates/cook-cli/src/modules/driver.rs`
- `cli/crates/cook-cli/tests/fixtures/luarocks-fixture/` (a vendored fixture rock tree for offline driver tests)

Does *not* touch `manifest.rs` (M3.1) or `lockfile.rs` (M3.2) — the driver consumes `LockedModule` and `ManifestModules` only as types from public re-exports.

### Acceptance for M3.3

- Golden tests against the fixture luarocks tree: assert argv shape for `install`, `remove`, `update`, `search`, `list_installed`. Mock the binary by pointing `prefix` at a script that records argv.
- Online integration test (gated on `cook chore gate-m2` having run): install a real rock (cook_smoke from rocks.usecook.com or argparse from luarocks.org) end-to-end; assert files land at expected paths; assert `list_installed` enumerates them correctly.
- Error-surfacing test: induce a luarocks failure (invalid rock name); assert the error Display contains argv + non-empty stderr.

## M3.4 — `cook modules` clap surface

### Subcommand layout

```
cook modules install                     read cook.lock; install pinned closure
cook modules install <name>...           add to [modules]; install; regenerate cook.lock
cook modules install <name>@<ver>        add with explicit version pin
cook modules remove <name>...            drop from [modules]; remove from tree; regenerate
cook modules update                      bump every dep within manifest constraints
cook modules update <name>               bump one dep within its manifest constraint
cook modules list                        read cook.lock; print installed rocks
cook modules search <query>              driver.search(query) against configured indexes
```

Cross-subcommand flags:

- `--registry <url>` — one-shot prefix to `[registry].indexes` for this invocation.
- `--non-interactive` — error on any prompt instead of asking.
- `--accept-trust` — non-interactive TOFU consent for CI.

### Orchestration loop (install path)

```
1. Read cook.toml -> (ManifestModules, ManifestRegistry).
2. Resolve indexes = (--registry value, if any) ++ ManifestRegistry.indexes.
3. driver = RocksDriver::new(prefix, indexes, project_dir)
4. If args is empty:
     a. If cook.lock exists:
          - Read lockfile.
          - Validate consistency with ManifestModules (every direct=true rock in lockfile is in manifest; every manifest entry is in lockfile).
          - For each LockedModule: driver.install_locked(&module).
        Else:
          - For each (name, constraint) in ManifestModules:
              driver.install(name, constraint)
          - Regenerate cook.lock via lockfile::introspect_closure().
   Else:
     - For each user-named rock:
         Update ManifestModules (write cook.toml).
         driver.install(name, constraint).
     - Regenerate cook.lock.
```

`remove`, `update`, and `update <name>` follow analogous patterns with the appropriate driver call and a regenerated lockfile.

### Wiring

Wired into the existing subcommand-dispatch pattern in `cli/crates/cook-cli/src/cli.rs`. Reference: commit `da7a4aa` (Phase 1's `cook release` chore wiring) for the same dispatch shape.

### Slice boundary

M3.4 owns:
- Edits to `cli/crates/cook-cli/src/cli.rs` (clap dispatch; new `Modules` subcommand variant).
- New `cli/crates/cook-cli/src/modules/cli.rs` (the modules-specific subcommand wiring).
- New integration tests at `cli/crates/cook-cli/tests/modules_integration.rs`.

Does not edit `manifest.rs` / `lockfile.rs` / `driver.rs` — only consumes their public types.

### Tests

- *Offline tests*: driver mocked (the test stubs `RocksDriver` via a trait or a process-level mock). Exercise the clap → manifest → lockfile flow with deterministic synthetic rock metadata. Cover all 8 subcommand paths.
- *Online integration test*: gated on `cook chore gate-m2` having been run (so `~/.cook/bin/luarocks` exists). A fixture project at `cli/crates/cook-cli/tests/fixtures/phase3-online/` declares `cook_smoke = "*"`; `cook modules install` succeeds; the lockfile names cook_smoke at the published version; a Cookfile recipe that does `local s = require("cook_smoke"); print(s.value())` prints `42`. This is the M3 acceptance gate; M3.8 ships the rock that makes it real.

## M3.5 + M3.6 — Runtime resolution + Standard §7 (single PR)

### M3.5: code change

`cli/crates/cook-luaotp/src/pool.rs:498` defines `refresh_package_path`, called per-execute-unit from the worker loop with the current Cookfile's working directory. M3.5:

1. Renames the function to `refresh_package_search_paths` (the broader scope is now `package.path` + `package.cpath`).
2. Grows it to set both:

```
package.path:
  <cwd>/cook_modules/?.lua
  <cwd>/cook_modules/?/init.lua
  <cwd>/cook_modules/share/lua/5.4/?.lua
  <cwd>/cook_modules/share/lua/5.4/?/init.lua
  <original>

package.cpath:
  <cwd>/cook_modules/?.so                       (Linux/macOS; .dll on Windows in Phase 5)
  <cwd>/cook_modules/lib/lua/5.4/?.so
  <original>
```

3. Generalises the `_cook_original_path` stash pattern to cover both: `_cook_original_path` and `_cook_original_cpath`. Per-unit refresh stays idempotent (re-invocation does not grow the path strings unboundedly).
4. Updates all call sites of `refresh_package_path` to the new name.

### M3.5 cleanup of Phase 2 fixtures

Phase 2's `chore "gate-m2"` has two Cookfiles (Part A: `Cookfile-a.lua`, Part B: `Cookfile-b.lua`) that each prepend `package.cpath` manually as a Phase-2-only workaround. M3.5 deletes those prepends — once the runtime configures cpath, the fixtures become "drop the rock at the right place, write `require('cook_hello')` (or `require('cjson')`), done."

The fixture relocation noted in the parent decomposition spec also executes at M3.5 merge time:
- `tests/fixtures/c-ext-hello/` → `cli/crates/cook-engine/tests/fixtures/c-rock/`

### M3.6: Standard §7 amendment

`standard/src/content/docs/07-cross-cookfile-composition.mdx` grows a normative clause after the existing locality rule (currently at line 68). Drafted text:

> A conforming implementation MUST configure `package.path` and `package.cpath` for an execute-phase work unit such that the calling Cookfile's `cook_modules/` is searched in the following order: (1) `cook_modules/?.lua` and `cook_modules/?/init.lua` (hand-vendored, top level); (2) `cook_modules/share/lua/<lua-version>/?.lua` and `cook_modules/share/lua/<lua-version>/?/init.lua` (LuaRocks-installed pure Lua); for native modules: (a) `cook_modules/?.<so-ext>` (hand-vendored, top level); (b) `cook_modules/lib/lua/<lua-version>/?.<so-ext>` (LuaRocks-installed). `<lua-version>` is the embedded Lua's MAJOR.MINOR (currently `5.4`). `<so-ext>` is the platform's loadable-module extension (`.so` on Linux/macOS, `.dll` on Windows).

App. B-rationale grows a paragraph explaining why the LuaRocks tree layout is adopted as the on-disk shape (interop with luarocks.org rockspecs without forking layout policy; hand-vendored top level retains highest priority so authors can override rock-installed versions by dropping a file).

App. D gets a new CS entry (next available number) referencing the new §7 clause.

### Co-PR rule

M3.5 and M3.6 land in the **same PR**. The pre-commit hook at `.githooks/pre-commit` enforces the spec-first rule: any change to `cli/crates/cook-luaotp/src/pool.rs` against the search-path setup paths requires a paired `standard/src/content/docs/07-*.mdx` change. **`COOK_STANDARD_BYPASS=1` is never used.**

### Slice boundary

The combined-slice agent owns:
- `cli/crates/cook-luaotp/src/pool.rs` (the `refresh_package_search_paths` function and call sites).
- `standard/src/content/docs/07-cross-cookfile-composition.mdx` (the §7 amendment).
- `standard/src/content/docs/appendix/B-rationale.mdx` (or wherever App. B lives) — the rationale paragraph.
- `standard/src/content/docs/appendix/D-conformance-summary.mdx` (or analogous) — the new CS entry.
- The fixture relocation: deletes `tests/fixtures/c-ext-hello/`, creates `cli/crates/cook-engine/tests/fixtures/c-rock/` with the same C source and rockspec.
- Edits to the gate-m2 chore Cookfiles to remove the manual cpath prepend.
- A new positive conformance case under `standard/conformance/positive/` exercising `cook_modules/share/lua/5.4/foo.lua` resolution.

### Acceptance for M3.5+M3.6

- Unit tests in `cook-luaotp` for `refresh_package_search_paths` (cpath shape per platform; idempotent across calls; original suffixes preserved).
- The relocated C-rock fixture loads via plain `require('cook_hello')` (no manual cpath in the test Cookfile).
- A pure-Lua rock fixture loads via plain `require()`.
- `cook chore gate-m2` still passes after the Phase 2 fixture cpath workarounds are removed.
- Conformance harness green for the §7 amendment; the new positive case loads a `cook_modules/share/lua/5.4/foo.lua` and asserts the loaded module's exported value.
- `.githooks/pre-commit` is happy with the paired Rust + Standard changes.

## M3.7 + M3.8 — Resolver wiring + `cook_smoke` publish (single agent)

### M3.7: resolver wiring

Small Rust diff (~30 LoC):

- `ManifestRegistry::default()` returns `["https://rocks.usecook.com", "https://luarocks.org"]`.
- M3.4's clap surface threads the resolved index list (with `--registry` prefix applied) into `RocksDriver::new`.
- Any project-init template that scaffolds a `cook.toml` writes the explicit `[registry].indexes` so users see the chain in their committed config.

No new files; edits to M3.1's `manifest.rs` (the agent coordinates with M3.1's slice boundary by specifically owning the `default()` impl, not the parser).

### M3.8: `cook_smoke` rockspec + publish

A throwaway rock that exists to validate the pipeline end-to-end. Files (all new, in this slice's worktree):

```
rocks/cook_smoke/
├── cook_smoke-0.1.0-1.rockspec      # the rockspec
├── cook_smoke.lua                   # the Lua source
└── README.md                        # one-paragraph "Phase 3 acceptance fixture"
```

Rockspec:

```lua
package = "cook_smoke"
version = "0.1.0-1"
source = {
   url = "https://rocks.usecook.com/cook_smoke-0.1.0.tar.gz",
}
description = {
   summary = "Phase 3 acceptance fixture for cook modules pipeline",
   license = "MIT",
}
dependencies = {
   "lua >= 5.4",
}
build = {
   type = "builtin",
   modules = {
      cook_smoke = "cook_smoke.lua",
   },
}
```

Lua source:

```lua
local M = {}
function M.value() return 42 end
return M
```

### Publish flow

The agent reads SHI-180's notes for the rocks.usecook.com upload mechanism (rsync-to-static-host, `gh release upload`, or Gitea Pages — depending on what hosting model SHI-180 settled on). The publish steps (in the slice brief, not committed code):

1. `~/.cook/bin/luarocks pack rocks/cook_smoke/cook_smoke-0.1.0-1.rockspec` produces `cook_smoke-0.1.0-1.src.rock`.
2. Generate or update the rocks.usecook.com manifest to include the new rock.
3. Upload the `.src.rock` and the manifest per SHI-180's procedure.
4. Verify from a fresh project: `[modules] cook_smoke = "*"` + `cook modules install` succeeds.

### Slice boundary

M3.7+M3.8 owns:
- `rocks/cook_smoke/` (new directory at repo root).
- The `ManifestRegistry::default()` constants in `cli/crates/cook-cli/src/modules/manifest.rs` (coordinated with M3.1's slice — M3.1 leaves the `default()` impl as a stub that M3.7 fills in).
- Any `cook init`-style scaffolding template that writes a default `cook.toml`.

Does *not* touch `manifest.rs` parsing logic (M3.1) or `driver.rs` (M3.3). Does *not* delete the legacy `[registry].url` (Phase 4 territory). Does *not* author rockspecs for blessed modules other than cook_smoke (Phase 4).

### Acceptance for M3.7+M3.8

- Unit test for `ManifestRegistry::default()` returning the documented index list.
- Smoke test: from a fresh fixture project, `cook modules install cook_smoke` succeeds; the lockfile names cook_smoke at the published version; a recipe doing `local s = require("cook_smoke"); print(s.value())` prints `42`.
- The published rock is accessible via `curl -fsSL https://rocks.usecook.com/manifest` (or whatever the index URL convention is).

## Dependency / sequencing map

```
Phase 2 gate (M2.4) ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━┓
                                                                                       ▼
Phase 3 Wave A (parallel, max in-flight = 5):                                          │
  ┌─────────┐  ┌─────────┐  ┌─────────┐  ┌──────────────────┐  ┌────────────────┐    │
  │  M3.1   │  │  M3.2   │  │  M3.3   │  │   M3.5 + M3.6    │  │  M3.7 + M3.8   │    │
  │manifest │  │lockfile │  │ driver  │  │ runtime cpath +  │  │ defaults +     │    │
  │parser   │  │ format  │  │subprocs │  │ Standard §7 (PR) │  │ cook_smoke pub │    │
  └────┬────┘  └────┬────┘  └────┬────┘  └────────┬─────────┘  └────────┬───────┘    │
       │            │            │                │                     │             │
       └────────────┴────────────┘                ▼                     ▼             │
                    │              Standard §7 lands;               cook_smoke        │
                    ▼              Phase 2 cpath workaround          available on     │
                                   retired                           rocks.usecook    │
                                                                                       │
Phase 3 Wave B (single slice):                                                        │
  ┌──────────────────────────────────────────┐                                        │
  │             M3.4                         │                                        │
  │ cook modules clap subcommand surface     │                                        │
  │ (consumes M3.1 + M3.2 + M3.3 outputs)    │                                        │
  └──────────────────┬───────────────────────┘                                        │
                     │                                                                │
                     ▼                                                                │
        Phase 3 acceptance gate runs                                                  │
        on linux-x86_64 + darwin-arm64                                                │
                     │                                                                │
                     ▼ unblocks Phase 4 (out of plan)                                 │
                                                                                      ━┛
```

Wave A's 5 slices touch disjoint files (per the slice-boundary clauses) and produce disjoint artifacts; they dispatch in parallel from `feature/luarocks-modules` worktrees. Wave B is a single slice (M3.4) that dispatches once Wave A is fully merged to the integration branch.

Maximum in-flight subagent count: **5** (Wave A). Wave B dispatches one slice. The integration burden on the orchestrator stays bounded.

## Linear sub-ticket structure

Filed under the SHI-176 epic by the implementation plan (writing-plans output), not by this design:

```
SHI-176 (epic)
└── M3 — cook modules CLI (Phase 3)
    ├── M3.1 — cook.toml [modules] + [registry].indexes parsing       blockedBy: M2.4
    ├── M3.2 — cook.lock format + read/write + closure introspection  blockedBy: M2.4
    ├── M3.3 — LuaRocks subprocess driver                             blockedBy: M2.4
    ├── M3.4 — cook modules clap subcommand surface                   blockedBy: M3.1, M3.2, M3.3
    ├── M3.5 — Runtime package.path / package.cpath extension         blockedBy: M2.4 ; co-PR with M3.6
    ├── M3.6 — Standard §7 search-path order clause + App. B/D        co-lands with M3.5
    ├── M3.7 — rocks.usecook.com resolver wiring                      blockedBy: M2.4
    └── M3.8 — cook_smoke rockspec + publish                          blockedBy: M2.4 ; co-owned with M3.7
```

## Dispatch protocol updates

The decomposition spec's clauses 1–8 (and Phase 2 spec's three operational notes) still apply. Phase 3 adds:

- **Underscore-only naming convention.** Every Phase 3 brief includes the line: "Cook's blessed module rocks use underscore separators (`cook_smoke`, never `cook-smoke`). The `cook_*` prefix is reserved for Cook-authored modules; the underscore form keeps rock names valid as Lua identifiers and bare TOML keys without quoting. Hyphenated luarocks.org rocks (`lua-cjson`) install fine and require `use lua-cjson as cjson` aliasing for the binding."
- **`cook pull` is out of scope.** Every Phase 3 brief includes the line: "`cli/crates/cook-cli/src/pull/` is Phase 4 territory. Do not edit, refactor, or delete pull during Phase 3. The legacy `[registry].url` field stays untouched; Phase 3 introduces the new `[registry].indexes` array next to it."
- **Spec-first co-PR for M3.5+M3.6.** The combined-slice brief names both files (`cli/crates/cook-luaotp/src/pool.rs` and `standard/src/content/docs/07-cross-cookfile-composition.mdx`) plus the App. B/D edits and the new positive conformance case. The pre-commit hook is the backstop, not the primary control. **`COOK_STANDARD_BYPASS=1` is never used.**
- **CS-0045 chore-body compliance** for M3.8's rock-publish flow (which is a procedural/manual step, not a chore — but any helper Cookfile chore added in this slice follows the rule): `cook.platform.os` not `io.popen`, `cook.sh` / `cook.exec` for command stdout, project-rooted paths only.
- **Worktree paths.** `<worktree-root>/luarocks-p3-s<N>` branched from `feature/luarocks-modules` at Wave A start point.

### File-conflict preemption (Phase 3)

Three known cross-slice file collisions, mitigated before dispatch:

- *M3.1 vs M3.4* — both touch `cli/crates/cook-cli/src/`. **Mitigation**: M3.1 owns `modules/manifest.rs` + `modules/mod.rs` + the public types. M3.4 owns clap dispatch in `cli.rs` + `modules/cli.rs` + integration tests; consumes M3.1's types via public re-exports. Brief for M3.1 includes "do not import from `cli::modules::driver` (M3.3 territory) and do not edit `cli.rs` (M3.4 territory)."
- *M3.1 vs M3.7* — both touch `manifest.rs`. **Mitigation**: M3.1 leaves `ManifestRegistry::default()` as a stub returning `Vec::new()`. M3.7 fills in the constants. The brief for each names this seam.
- *M3.5 fixture relocation* — Phase 2's `tests/fixtures/c-ext-hello/` moves to `cli/crates/cook-engine/tests/fixtures/c-rock/` in M3.5+M3.6. Brief includes the move command and the gate-m2 Cookfile edits that delete the manual cpath prepend.

## Acceptance gates

### Per-slice gates

- **M3.1** — Positive and negative `cook.toml` parsing tests pass; `ManifestRegistry::default()` returns the stub (M3.7 fills it in).
- **M3.2** — Round-trip serialization green; integrity verification catches wrong hashes; schema-mismatch detection works; `introspect_closure` against fixture rock tree produces expected `direct` flags.
- **M3.3** — Golden argv tests pass; online integration test (gated on M2.4) installs a real rock.
- **M3.4** — Offline tests cover all 8 subcommand paths; online integration test passes against cook_smoke (depends on M3.8).
- **M3.5+M3.6** — Unit tests for `refresh_package_search_paths` pass; relocated C-rock fixture loads without manual cpath; new positive conformance case green; `.githooks/pre-commit` accepts the paired change.
- **M3.7+M3.8** — `ManifestRegistry::default()` returns expected pair; `cook_smoke` is fetchable from rocks.usecook.com; smoke test from a fresh project succeeds.

### M3 cumulative gate (phase gate)

Phase 3 has no single gate slice. The phase merges to `main` when all 8 M3.x slices are green and the cumulative end-to-end check below passes on **linux-x86_64** (locally) AND **darwin-arm64** (manual on `mini` until SHI-187 lands the runner).

A fresh fixture project at `cli/crates/cook-cli/tests/fixtures/phase3-acceptance/` with:

```toml
# cook.toml
[registry]
indexes = ["https://rocks.usecook.com", "https://luarocks.org"]

[modules]
cook_smoke  = "*"
"lua-cjson" = ">=2.1"
argparse    = "*"
```

```cook
# Cookfile
use cook_smoke
use lua-cjson as cjson
use argparse

recipe smoke
    > local s = require("cook_smoke")
    > local out = cjson.encode({hello = "world", value = s.value()})
    > print(out)
    > local p = argparse("smoke", "test")
    > print("argparse loaded:", p ~= nil)
end
```

Acceptance procedure:

1. From a clean checkout: `cook modules install`.
2. `cook_modules/share/lua/5.4/cook_smoke.lua`, `cook_modules/lib/lua/5.4/cjson.so`, and `cook_modules/share/lua/5.4/argparse.lua` all exist on disk.
3. `cook.lock` lists all three top-level deps with `direct = true` plus any transitives with `direct = false`.
4. `cook smoke` prints a JSON object containing `"value":42` and `"hello":"world"` plus the argparse line.
5. Re-running `cook modules install` is a no-op (lockfile-replay path; no new network calls beyond integrity-verifiable cache hits).
6. `cook modules remove cook_smoke` removes the entry from `[modules]`, removes the rock files, regenerates `cook.lock` without cook_smoke and without its transitive-only deps.
7. Conformance harness green for the Standard §7 amendment.

Both platforms via the same procedure as Phase 2's gate: linux-x86_64 locally; darwin-arm64 manually on `mini`. SHI-187 (Gitea Actions runner on mini) will eventually automate steps 2–7 on macOS; until then, capture stdout/stderr to a file and post to the M3 cumulative-gate ticket as a comment.

## Risks

- **Online vs offline test split.** M3.4's online integration test and M3.8's smoke install need network egress + a live rocks.usecook.com. CI without egress can't run them — same trade-off as Phase 2's gate-m2 Part B. Mitigation: tag online tests with a feature flag; document that Phase 3's gate requires network.
- **rocks.usecook.com hosting model.** SHI-180 settled the hosting; M3.8's brief reads SHI-180 for the upload mechanism (rsync-to-static-host vs. `gh release upload` vs. Gitea Pages). Confirm before dispatching M3.7+M3.8.
- **macOS C-rock regression catcher (lua-cjson).** Phase 2's gate-m2 already validates this; Phase 3's acceptance gate re-validates it via the cjson dep in the fixture project. If the gate fails on macOS specifically, suspect symbol-resolution regressions and check `nm -gU cook` for the `lua_*` exports first. The companion explainer (`docs/architecture/dynamic-linking-and-rocks.md`) walks through why this is the canonical regression catcher.
- **Luarocks transitive resolution drift.** A `lua-cjson` (or other rock) update on luarocks.org could change which transitives land. Mitigation: `cook.lock` pins the closure; `cook modules install` (no args) uses the locked closure, so the gate is reproducible from a committed lockfile.
- **Phase 2 cleanup ordering.** M3.5 deletes Phase 2's manual cpath prepends in the gate-m2 chore. If M3.5 lands before M3.4, the gate-m2 fixture briefly relies on the new runtime resolution before any `cook modules` user-facing surface exists. That's fine — gate-m2's correctness is independent of the CLI surface — but the brief notes the ordering explicitly.
- **Lockfile divergence between developers.** Two developers running `cook modules update` against the same `[modules]` constraints may produce different lockfiles if luarocks.org has resolved a new patch version between runs. Mitigation: typical of every package manager (Cargo, pnpm); the resolution is "the lockfile from main wins; rebase on conflicts." Document in the user guide that ships with M3 release notes.
- **Driver mocking complexity in M3.4 offline tests.** Trait-based mocking of `RocksDriver` is the cleanest approach; process-level mocking (script-on-PATH) adds shell-script test infrastructure. M3.4's brief should specify the mocking approach (recommendation: trait-based) so the agent doesn't reinvent it.
- **`cook pull` and `cook modules` user confusion.** Both subsystems exist through Phase 3. Mitigation: the user guide for M3 release notes documents that `cook modules` is the new surface and `cook pull` is being phased out in Phase 4. No deprecation warning yet (per the brainstorm decision; warnings without a working alternative would be user-hostile).

## Future work

- **Phase 4** — Migrate blessed modules (cook_cpp, cook_rust, cook_pnpm, cook_ai) to rocks; publish to rocks.usecook.com; delete `cli/crates/cook-cli/src/pull/`; delete legacy `[registry].url` from cook.toml schema. Re-brainstormed when Phase 3 is ground truth on `main`.
- **Phase 5** — Windows packaging (`lua54.dll`, mlua external-Lua mode, MSI, MSVC documentation, curated prebuilt binary rocks for cook_*). Re-brainstormed when Phase 3 is stable on Unix. The runtime mechanics primer (`docs/architecture/dynamic-linking-and-rocks.md`) explains why this is a separate phase.
- **`cook modules publish`** — A Cook-side publish flow for blessed modules would simplify Phase 4's procedural upload step. Out of scope for v1; non-goal per parent design (`luarocks upload` is the current path).
- **Authentication and private registries.** Token / SSH support for private rocks indexes. v1 is public-only.
- **Workspace-level deduplication.** A multi-Cookfile workspace currently installs per-Cookfile. Sharing a single `cook_modules/` across siblings is later work.
- **Sandboxed install.** `cook modules install <random-rock>` runs arbitrary build steps from the rockspec. v1 inherits the npm/cargo trust model. A future `--sandbox` flag is plausible but out of scope.
- **Schema-bump migration tooling.** `cook modules migrate-lock` for future `schema = 2` lockfiles.

## References

- Parent architectural spec: `docs/superpowers/specs/2026-05-08-luarocks-modules-design.md`
- Parent decomposition spec: `docs/superpowers/specs/2026-05-09-luarocks-modules-decomposition-design.md`
- Phase 2 design: `docs/superpowers/specs/2026-05-09-luarocks-phase-2-design.md`
- Runtime mechanics primer: `docs/architecture/dynamic-linking-and-rocks.md`
- Linear epic: SHI-176
- Linear ops project: "Cook distribution infra — rocks index + installer hosting" (SHI-178..183)
- macOS build slave ticket: SHI-187
- SHI-188 (cook.exec failure stream inlining): commit `7fbbbb2` (shipped 2026-05-10)
- Phase 2 shipped commit: `ec32ad9`
- Phase 1 shipped notice: `project_shi176_phase_1_done.md` (auto-memory)
- Phase 2 shipped notice: `project_shi176_phase_2_done.md` (auto-memory)
- Spec-first rule: `feedback_spec_first_no_bypass.md` (auto-memory)
- Pre-commit hook: `.githooks/pre-commit`
- CONTRIBUTING.md: spec-first rule, conformance harness, language-surface paths list
- Module structure rules: `CLAUDE.md` ("Module Structure Rules")
