# SHI-134 — M1 cc Finders Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Generalize `cc.find` to a four-stage strategy chain with seven inline curated finders, add `cc.register_finder` + `cc.find_or_error`, plumb `frameworks` through link + transitive propagation, and graduate Cook Standard §9.2 from v0.1 to v0.2. Reference implementation ships as `cook_cc 0.2.0-1`.

**Architecture:** Two repos coordinate lockstep. The cook repo holds the Standard prose, six new parse-only conformance fixtures, the new `examples/raylib-game/` integration example, and version pins for existing examples. The cook-modules repo holds the cook_cc Lua implementation (resolver shell, seven curated finders, shared helpers, version checker, busted spec). One coordinated release ties them together: cook_cc 0.2.0-1 publishes to rocks.usecook.com between PR cook#A (draft) and PR cook#A (ready-for-merge with example pins).

**Tech Stack:** Lua 5.4 + lpeg (cook_cc), busted (specs), MDX + Astro (Standard), Cook's parse-only conformance harness (`cargo test -p cook-lang --test conformance`), LuaRocks (rocks.usecook.com), Cloudflare Pages (rocks index hosting).

**Spec reference:** `docs/superpowers/specs/2026-05-12-shi134-m1-cc-finders-design.md`.

---

## File Structure

### `cook-modules/cook_cc/` (cook-modules repo)

| File | Status | Responsibility |
|---|---|---|
| `version.lua` | NEW | Semver parser + `M.satisfies(detected, constraint)` |
| `finder.lua` | REWRITE | Resolver shell, chain dispatch, `M.register` for project finders |
| `finders/init.lua` | NEW | Curated registry + alias table + chain assembly |
| `finders/pkg_config.lua` | NEW | Shared `try(name) → Attempt?` helper + main-chain strategy |
| `finders/bare_probe.lua` | NEW | Shared `try(name) → Attempt?` helper + main-chain strategy |
| `finders/header_probe.lua` | NEW | `parse_define(path, macro) → string?` helper |
| `finders/tool_config.lua` | NEW | `try(tool, args) → string?` helper |
| `finders/raylib.lua` | NEW | Curated raylib finder |
| `finders/sdl2.lua` | NEW | Curated sdl2 finder |
| `finders/openal.lua` | NEW | Curated openal finder |
| `finders/gl.lua` | NEW | Curated gl/opengl finder |
| `finders/threads.lua` | NEW | Curated threads finder |
| `finders/zlib.lua` | NEW | Curated zlib finder |
| `finders/libcurl.lua` | NEW | Curated libcurl finder |
| `init.lua` | MODIFY | Add `M.register_finder`, `M.find_or_error` |
| `cc.lua` | MODIFY | Emit `-framework <name>` in `M.link` on macOS |
| `targets.lua` | MODIFY | Accept + propagate `frameworks` in `bin/lib/shared` |
| `transitive.lua` | MODIFY | Propagate `frameworks` through `resolve_links` |
| `spec/cook_stub.lua` | MODIFY | Add platform.os override, pkg-config / tool-config / file-exists mocks |
| `spec/finders/resolver_spec.lua` | NEW | Chain order, caching, attempt schema |
| `spec/finders/version_spec.lua` | NEW | Parser + satisfies() |
| `spec/finders/pkg_config_spec.lua` | NEW | Helper unit tests |
| `spec/finders/bare_probe_spec.lua` | NEW | Helper unit tests |
| `spec/finders/raylib_spec.lua` | NEW | Per-curated finder spec |
| `spec/finders/sdl2_spec.lua` | NEW | Per-curated finder spec |
| `spec/finders/openal_spec.lua` | NEW | Per-curated finder spec |
| `spec/finders/gl_spec.lua` | NEW | Per-curated finder spec + alias test |
| `spec/finders/threads_spec.lua` | NEW | Per-curated finder spec + skip-on-version |
| `spec/finders/zlib_spec.lua` | NEW | Per-curated finder spec |
| `spec/finders/libcurl_spec.lua` | NEW | Per-curated finder spec |
| `spec/finder_spec.lua` | REDUCE | Public-API integration tests only |
| `spec/cc_spec.lua` | MODIFY | Add frameworks emission test for macOS |
| `spec/targets_spec.lua` | MODIFY | Add frameworks pass-through test |
| `spec/transitive_spec.lua` | MODIFY | Add frameworks propagation test |
| `cook_cc-0.2.0-1.rockspec` | NEW | Rockspec for the new version |

### `cook/` (main repo)

| File | Status | Responsibility |
|---|---|---|
| `standard/src/content/docs/09-standard-modules.mdx` | MODIFY | §9.2.3.7 LinkOpts.frameworks; §9.2.3.8 v0.2 rewrite; §9.2.3.12 + §9.2.3.13 new; §9.2.4 + §9.2.5 additions |
| `standard/src/content/docs/appendix/D-changes.mdx` | MODIFY | CS-0067 changelog entry |
| `standard/src/content/docs/appendix/B-rationale.mdx` | MODIFY | One paragraph retaining v0.1 of §9.2.3.8 |
| `standard/conformance/positive/cc-find-version-constraint/` | NEW | Cookfile + parse.txt + notes.md |
| `standard/conformance/positive/cc-find-or-error/` | NEW | Cookfile + parse.txt + notes.md |
| `standard/conformance/positive/cc-find-tried-field/` | NEW | Cookfile + parse.txt + notes.md |
| `standard/conformance/positive/cc-register-finder/` | NEW | Cookfile + parse.txt + notes.md |
| `standard/conformance/positive/cc-frameworks-on-link/` | NEW | Cookfile + parse.txt + notes.md |
| `standard/conformance/positive/cc-frameworks-transitive/` | NEW | Cookfile + parse.txt + notes.md |
| `examples/raylib-game/cook.toml` | NEW | Pin cook_cc 0.2.0-1 |
| `examples/raylib-game/Cookfile` | NEW | `cc.find_or_error("raylib")` + `cc.bin` |
| `examples/raylib-game/src/main.c` | NEW | raylib's `core_basic_window.c` (BSD-licensed, attributed) |
| `examples/raylib-game/README.md` | NEW | Install hint per OS |
| `examples/lua-build/cook.toml` | MODIFY | Pin to 0.2.0-1 |
| `examples/fzf-picker/cook.toml` | MODIFY | Pin to 0.2.0-1 |
| `examples/cpp-project/cook.toml` | MODIFY | Pin to 0.2.0-1 |

### Working assumptions

- `~/dev/cook` is the cook repo working tree (this plan's CWD unless otherwise stated).
- `~/dev/cook-modules` is the cook-modules repo working tree.
- The git remote for cook-modules is the Gitea NAS (`gilberthouse.story-pike.ts.net`).
- `cook` binary on PATH is from the SHI-176 Phase 1+ install; `busted` on PATH.
- All `cd` markers below are explicit; otherwise commands run from `~/dev/cook`.

---

## Stage A — Standard prose + conformance fixtures (cook repo, PR cook#A draft)

### Task A1: Add §9.2.3.7 LinkOpts.frameworks

**Files:**
- Modify: `standard/src/content/docs/09-standard-modules.mdx:131-138`

- [ ] **Step 1: Locate the current §9.2.3.7 cc.link prose**

Run: `grep -n "9.2.3.7\|cc.link" standard/src/content/docs/09-standard-modules.mdx | head`
Expected output includes line 131 (heading) and line 137 (the "Links objects to `output`..." paragraph).

- [ ] **Step 2: Append the frameworks field to the LinkOpts contract**

The §9.2.3.7 section today reads as a single descriptive paragraph. Append, immediately after the existing paragraph at line 137:

```mdx
`LinkOpts.frameworks` (list[string]): each entry MUST emit `-framework <name>` at link time on macOS hosts. On non-macOS hosts the implementation MUST treat the field as a no-op, allowing cross-platform call sites where a curated finder returns `system_libs={"openal"}` on Linux and `frameworks={"OpenAL"}` on macOS to use the same `cc.link` invocation.
```

- [ ] **Step 3: Verify standard.build still parses**

Run: `cd standard && npm run build 2>&1 | tail -20`
Expected: build succeeds; the pre-existing `E-pre-v1-checklist.mdx` slug warning ([SHI-212](https://linear.app/shiny-guru/issue/SHI-212)) is the only warning.

- [ ] **Step 4: Commit**

```bash
git -C ~/dev/cook add standard/src/content/docs/09-standard-modules.mdx
git -C ~/dev/cook commit -m "docs(standard): §9.2.3.7 LinkOpts.frameworks (M1)"
```

### Task A2: §9.2.3.8 cc.find — v0.2 normative rewrite

**Files:**
- Modify: `standard/src/content/docs/09-standard-modules.mdx:139-168`

- [ ] **Step 1: Replace the §9.2.3.8 body**

Replace the content between the `#### 9.2.3.8. cc.find` heading and the `#### 9.2.3.9. cc.defaults` heading with:

````mdx
#### 9.2.3.8. cc.find [#stdmods.cc.find]

```
cc.find(name: string, opts: FindOpts?) -> FindResult
```

Discovers a package by name and returns a structured result record.

`FindOpts` (all optional):

| Field | Type | Semantics |
|---|---|---|
| `version` | string | A semver-style constraint (e.g., `">=4.0"`, `"=4.0.1"`, `">=2.0,<3.0"`). Honoured by every strategy that can determine the discovered package's version. Pre-release tags are excluded (`"4.0.0-rc1"` does NOT satisfy `">=4.0.0"`). |

A conforming v0.2 implementation MUST consult strategies in this order, first-match-wins:

1. **Project-registered finders** (per §9.2.3.12). MUST be tried first so projects can override curated behaviour.
2. **Curated finders** — implementation-provided finders for a documented short list. The reference implementation ships finders for `raylib`, `sdl2`, `openal`, `gl`/`opengl`, `threads`, `zlib`, `libcurl`. Implementations MAY ship a different set; each curated finder MUST conform to the FindResult contract.
3. **pkg-config** — as v0.1.
4. **Bare-lib probe** — file-existence check for `lib<name>.{so,dylib,a}` on the host's default linker search paths (`/usr/lib`, `/usr/local/lib`, and paths reported by `cc -print-search-dirs`). MUST be skipped when `opts.version` is set; bare probe cannot verify version.

A strategy MAY report `outcome = "skip"` (the strategy structurally could not be consulted) or `outcome = "miss"` (the strategy was consulted and rejected the package). Only `outcome = "hit"` populates `found = true` and the result fields. `skip` and `miss` both allow the chain to continue; the distinction is informational.

`FindResult` (returned by every `cc.find` call):

| Field | Type | Semantics |
|---|---|---|
| `found` | boolean | `true` iff some strategy returned `outcome = "hit"`. |
| `cflags` | string | Compile flags from the winning strategy (empty on miss). |
| `libs` | string | Link flags from the winning strategy (empty on miss). |
| `system_libs` | list[string] | Parsed `-l<name>` libraries. |
| `include_dirs` | list[string] | Parsed `-I<dir>` paths. |
| `lib_dirs` | list[string] | Parsed `-L<dir>` paths. |
| `frameworks` | list[string] | Parsed `-framework <name>` entries (macOS). |
| `version` | string \| nil | Version detected by the winning strategy (nil if the winning strategy reports none). |
| `tried` | list[Attempt] | Ordered list of every strategy consulted, including the winning entry on hits. |

`Attempt`:

| Field | Type | Semantics |
|---|---|---|
| `strategy` | string | `"project:<name>"`, `"curated:<name>"`, `"pkg-config"`, `"bare-probe"`, or for third-party rocks `"rocks.<rockname>:<finder>"`. |
| `outcome` | string | `"hit"`, `"miss"`, or `"skip"`. |
| `reason` | string | Human-readable explanation. MAY be empty on `"hit"`. |
| `hint` | string \| nil | Install hint (e.g., `"apt: libraylib-dev / brew: raylib"`). Curated finders SHOULD emit on miss; other strategies SHOULD NOT. |

Caching: results MUST be cached in `cook.cache` keyed by `"cc.find:<name>:<canonical-opts>"`, where `canonical-opts` is the deterministic serialisation of `opts` (keys sorted, values stringified, list-valued opts joined with `,`). Repeat calls within one invocation MUST NOT re-consult the chain.
````

- [ ] **Step 2: Verify standard.build still parses**

Run: `cd standard && npm run build 2>&1 | tail -20`
Expected: build succeeds.

- [ ] **Step 3: Commit**

```bash
git -C ~/dev/cook add standard/src/content/docs/09-standard-modules.mdx
git -C ~/dev/cook commit -m "docs(standard): §9.2.3.8 cc.find v0.2 — multi-strategy chain (M1)"
```

### Task A3: §9.2.3.12 cc.register_finder (new)

**Files:**
- Modify: `standard/src/content/docs/09-standard-modules.mdx` (insert after §9.2.3.11)

- [ ] **Step 1: Locate insertion point**

Run: `grep -n "9.2.3.11\|9.2.4" standard/src/content/docs/09-standard-modules.mdx | head`
Expected: §9.2.3.11 heading near line 194, §9.2.4 heading near line 202.

- [ ] **Step 2: Insert §9.2.3.12 immediately before §9.2.4**

Insert before the `### 9.2.4. Transitive propagation` heading:

````mdx
#### 9.2.3.12. cc.register_finder [#stdmods.cc.register-finder]

```
cc.register_finder(name: string, finder: function(opts) -> FindResult)
```

Registers a project-scoped finder for `name`. Subsequent `cc.find(name, ...)` calls MUST consult `finder` before the curated/pkg-config/bare-probe stages, recording its Attempt with `strategy = "project:<name>"`. The function MUST return a FindResult; the implementation MAY discard the function's own `tried` field (the resolver synthesises the chain-level `tried` list externally). Re-registration replaces the prior finder without warning; this enables `config` blocks to override defaults per build profile.

Raising in the finder is permitted; the error surfaces at the `cc.find` call site at level 2 (per §9.2.5).
````

- [ ] **Step 3: Insert §9.2.3.13 cc.find_or_error immediately after §9.2.3.12**

Append immediately after the §9.2.3.12 block, still before §9.2.4:

````mdx
#### 9.2.3.13. cc.find_or_error [#stdmods.cc.find-or-error]

```
cc.find_or_error(name: string, opts: FindOpts?) -> FindResult
```

Calls `cc.find(name, opts)`. If `result.found` is `true`, returns `result` unchanged. Otherwise raises an error at level 2 prefixed `[cc.find_or_error]`. The error message MUST list every Attempt in `result.tried` (strategy / outcome / reason) and MUST include any `hint` fields. This is the only function in §9.2 that raises on a missing package.
````

- [ ] **Step 4: Verify standard.build still parses**

Run: `cd standard && npm run build 2>&1 | tail -20`
Expected: build succeeds.

- [ ] **Step 5: Commit**

```bash
git -C ~/dev/cook add standard/src/content/docs/09-standard-modules.mdx
git -C ~/dev/cook commit -m "docs(standard): §9.2.3.12 cc.register_finder + §9.2.3.13 cc.find_or_error (M1)"
```

### Task A4: §9.2.4 frameworks in propagation + §9.2.5 error catalog

**Files:**
- Modify: `standard/src/content/docs/09-standard-modules.mdx:202-228`

- [ ] **Step 1: Extend §9.2.4 info-record field list**

In the bullet list under §9.2.4 (currently at lines 206-211 — `info.includes`, `info.defines`, `info.system_libs`, `info.extra_ldflags`, `info.links`, `info.compile_info`), insert a new bullet between `info.system_libs` and `info.extra_ldflags`:

```mdx
- `info.frameworks` — macOS framework entries the consumer adds at link time as `-framework <name>`
```

- [ ] **Step 2: Extend the closure-walk paragraph at the end of §9.2.4**

Append to the last paragraph of §9.2.4 (the "When `opts.links = ...` is provided..." paragraph):

```mdx
The closure's `frameworks` MUST be merged into the new target's link command analogously to `system_libs` (dedup-preserving first-seen order); a non-macOS host implementation MAY merge but MUST NOT emit (§9.2.3.7).
```

- [ ] **Step 3: Extend §9.2.5 diagnostic catalogue**

Add two rows to the §9.2.5 error catalog table:

```mdx
| `cc.find_or_error` | `result.found == false` | `could not locate '<name>'<version-suffix>:\n  - <strategy>: <outcome> (<reason>)...\n<hints>` |
| `cc.register_finder` | `finder` is not a function | `register_finder for '<name>' requires a function, got <type>` |
```

- [ ] **Step 4: Verify standard.build still parses**

Run: `cd standard && npm run build 2>&1 | tail -20`
Expected: build succeeds.

- [ ] **Step 5: Commit**

```bash
git -C ~/dev/cook add standard/src/content/docs/09-standard-modules.mdx
git -C ~/dev/cook commit -m "docs(standard): §9.2.4 frameworks propagation + §9.2.5 catalogue (M1)"
```

### Task A5: App. D CS-0067 changelog + App. B rationale

**Files:**
- Modify: `standard/src/content/docs/appendix/D-changes.mdx`
- Modify: `standard/src/content/docs/appendix/B-rationale.mdx`

- [ ] **Step 1: Read the current tail of App. D**

Run: `grep -n "^## CS-" standard/src/content/docs/appendix/D-changes.mdx | tail -5`
Expected: identifies the last few CS-0NN entries (most recent CS-0066 from M0 close).

- [ ] **Step 2: Append CS-0067 to App. D**

At the end of `standard/src/content/docs/appendix/D-changes.mdx`, append:

```mdx
## CS-0067 — cc finder multi-strategy chain (M1) [#cs-0067]

**What changed.** Cook Standard §9.2 graduates from v0.1 to v0.2:

- §9.2.3.7 `cc.link`: `LinkOpts` grows a `frameworks` field. Each entry emits `-framework <name>` at link time on macOS; no-op on other hosts.
- §9.2.3.8 `cc.find`: rewritten from pkg-config-only to a four-stage chain (project-registered → curated → pkg-config → bare-probe). `FindResult` grows a `tried` field listing every strategy consulted. `FindOpts.version` is now honoured.
- §9.2.3.12 `cc.register_finder`: new — project-scoped extension seam for the chain.
- §9.2.3.13 `cc.find_or_error`: new — raising convenience wrapper.
- §9.2.4 transitive propagation: `frameworks` joins the propagated field set.
- §9.2.5 error catalogue: two new entries for the new raising surfaces.

**Provenance.** [SHI-134](https://linear.app/shiny-guru/issue/SHI-134), parent [SHI-132](https://linear.app/shiny-guru/issue/SHI-132) (Cook builds Doom 3). Reference implementation `cook_cc 0.2.0-1`.

**Migration.** Existing call sites of `cc.find(name)` continue to work unchanged; new fields on the result are additive. Existing target makers (`cc.bin`/`cc.lib`/`cc.shared`) gain an optional `frameworks` option; pre-v0.2 Cookfiles that do not set it observe identical behaviour.
```

- [ ] **Step 3: Append v0.1 rationale paragraph to App. B**

Locate the §B.7 or §B.8 area where M0's CS-0062 rationale lives (`grep -n "9.2.3.8\|find" standard/src/content/docs/appendix/B-rationale.mdx | head`). Append a paragraph capturing the v0.1 wording:

```mdx
### B.8.x — Why `cc.find` v0.1 specified pkg-config-only

The v0.1 wording (released in `cook_cc 0.1.x`) specified pkg-config as the sole `cc.find` strategy. This was deliberate: M0 (the chapter introduction) optimised for a tight normative surface that real Cookfiles could verify against, while leaving room to extend. v0.2 (M1, this revision) generalises to a multi-strategy chain; the v0.1 wording is preserved here so anyone bisecting against `cook_cc 0.1.x` has a referenceable spec. There is no permanent break — every v0.1 call site (`cc.find("zlib")` etc.) remains a valid v0.2 call site, and pkg-config remains a chain stage.
```

- [ ] **Step 4: Verify standard.build still parses**

Run: `cd standard && npm run build 2>&1 | tail -20`
Expected: build succeeds.

- [ ] **Step 5: Commit**

```bash
git -C ~/dev/cook add standard/src/content/docs/appendix/D-changes.mdx standard/src/content/docs/appendix/B-rationale.mdx
git -C ~/dev/cook commit -m "docs(standard): App. D CS-0067 + App. B v0.1 rationale (M1)"
```

### Task A6: Conformance fixture — cc-find-version-constraint

**Files:**
- Create: `standard/conformance/positive/cc-find-version-constraint/Cookfile`
- Create: `standard/conformance/positive/cc-find-version-constraint/parse.txt`
- Create: `standard/conformance/positive/cc-find-version-constraint/notes.md`

- [ ] **Step 1: Create the fixture directory and Cookfile**

```bash
mkdir -p standard/conformance/positive/cc-find-version-constraint
```

Write `standard/conformance/positive/cc-find-version-constraint/Cookfile`:

```cook
use cook_cc

recipe probe
    > local r = cook_cc.find("zlib", { version = ">=1.2,<2.0" })
    > if not r.found then error("expected zlib hit") end
```

- [ ] **Step 2: Generate parse.txt**

Run: `cook --parse-only standard/conformance/positive/cc-find-version-constraint/Cookfile > standard/conformance/positive/cc-find-version-constraint/parse.txt`

Expected `parse.txt` content (verify by running `cat`):

```
Cookfile
  uses: [UseStatement module_name="cook_cc" line=1]
  imports: []
  config_blocks: []
  recipes:
    Recipe name="probe" line=3
      deps: []
      ingredients: []
      excludes: []
      steps:
        Lua code="local r = cook_cc.find(\"zlib\", { version = \">=1.2,<2.0\" })"
        Lua code="if not r.found then error(\"expected zlib hit\") end"
  chores:
```

- [ ] **Step 3: Write notes.md**

Write `standard/conformance/positive/cc-find-version-constraint/notes.md`:

```markdown
# cc-find-version-constraint

Locks §9.2.3.8 `FindOpts.version` syntax: comma-separated AND, mixed operators.
Parse-only: this fixture does not verify behaviour, only that the Cookfile's
Lua step parses cleanly with the new opts form.
Runtime conformance for this surface is filed under SHI-210.
```

- [ ] **Step 4: Run the conformance harness**

Run: `cargo test -p cook-lang --test conformance cc_find_version_constraint`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git -C ~/dev/cook add standard/conformance/positive/cc-find-version-constraint/
git -C ~/dev/cook commit -m "test(conformance): cc-find-version-constraint parse fixture (M1)"
```

### Task A7: Conformance fixture — cc-find-or-error

**Files:**
- Create: `standard/conformance/positive/cc-find-or-error/{Cookfile,parse.txt,notes.md}`

- [ ] **Step 1: Create directory + Cookfile**

```bash
mkdir -p standard/conformance/positive/cc-find-or-error
```

Write `Cookfile`:

```cook
use cook_cc

recipe probe
    > local r = cook_cc.find_or_error("zlib")
    > if not r.found then error("unreachable") end
```

- [ ] **Step 2: Generate parse.txt**

Run: `cook --parse-only standard/conformance/positive/cc-find-or-error/Cookfile > standard/conformance/positive/cc-find-or-error/parse.txt`

Expected:

```
Cookfile
  uses: [UseStatement module_name="cook_cc" line=1]
  imports: []
  config_blocks: []
  recipes:
    Recipe name="probe" line=3
      deps: []
      ingredients: []
      excludes: []
      steps:
        Lua code="local r = cook_cc.find_or_error(\"zlib\")"
        Lua code="if not r.found then error(\"unreachable\") end"
  chores:
```

- [ ] **Step 3: Write notes.md**

```markdown
# cc-find-or-error

Locks §9.2.3.13 `cc.find_or_error` surface — parses identically to `cc.find`
at the Cookfile level. Runtime miss-raises behaviour is exercised by busted
in cook-modules/cook_cc/spec/ and would be re-locked here once SHI-210 lands.
```

- [ ] **Step 4: Run harness**

Run: `cargo test -p cook-lang --test conformance cc_find_or_error`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git -C ~/dev/cook add standard/conformance/positive/cc-find-or-error/
git -C ~/dev/cook commit -m "test(conformance): cc-find-or-error parse fixture (M1)"
```

### Task A8: Conformance fixture — cc-find-tried-field

**Files:**
- Create: `standard/conformance/positive/cc-find-tried-field/{Cookfile,parse.txt,notes.md}`

- [ ] **Step 1: Create directory + Cookfile**

```bash
mkdir -p standard/conformance/positive/cc-find-tried-field
```

Write `Cookfile`:

```cook
use cook_cc

recipe probe
    > local r = cook_cc.find("definitely_no_such_package_xyz_42")
    > if r.found then error("expected miss") end
    > if type(r.tried) ~= "table" then error("expected tried list") end
```

- [ ] **Step 2: Generate parse.txt**

Run: `cook --parse-only standard/conformance/positive/cc-find-tried-field/Cookfile > standard/conformance/positive/cc-find-tried-field/parse.txt`

Expected:

```
Cookfile
  uses: [UseStatement module_name="cook_cc" line=1]
  imports: []
  config_blocks: []
  recipes:
    Recipe name="probe" line=3
      deps: []
      ingredients: []
      excludes: []
      steps:
        Lua code="local r = cook_cc.find(\"definitely_no_such_package_xyz_42\")"
        Lua code="if r.found then error(\"expected miss\") end"
        Lua code="if type(r.tried) ~= \"table\" then error(\"expected tried list\") end"
  chores:
```

- [ ] **Step 3: Write notes.md**

```markdown
# cc-find-tried-field

Locks §9.2.3.8 `FindResult.tried` shape at the parse level. The runtime
assertion is `type(r.tried) == "table"` — adequate for parse-fixture surface
checks. Full Attempt-record shape is verified by the busted suite (each
finder spec) and will be re-locked here once SHI-210 lands.
```

- [ ] **Step 4: Run harness**

Run: `cargo test -p cook-lang --test conformance cc_find_tried_field`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git -C ~/dev/cook add standard/conformance/positive/cc-find-tried-field/
git -C ~/dev/cook commit -m "test(conformance): cc-find-tried-field parse fixture (M1)"
```

### Task A9: Conformance fixture — cc-register-finder

**Files:**
- Create: `standard/conformance/positive/cc-register-finder/{Cookfile,parse.txt,notes.md}`

- [ ] **Step 1: Create directory + Cookfile**

```bash
mkdir -p standard/conformance/positive/cc-register-finder
```

Write `Cookfile`:

```cook
use cook_cc

recipe probe
    > cook_cc.register_finder("mylib", function(_opts)
    >     return { found = true, cflags = "", libs = "-lmylib", system_libs = {"mylib"}, include_dirs = {}, lib_dirs = {}, frameworks = {}, version = nil, tried = {} }
    > end)
    > local r = cook_cc.find("mylib")
    > if not r.found then error("expected project-registered hit") end
```

- [ ] **Step 2: Generate parse.txt**

Run: `cook --parse-only standard/conformance/positive/cc-register-finder/Cookfile > standard/conformance/positive/cc-register-finder/parse.txt`

Expected (multi-line Lua step preserved verbatim):

```
Cookfile
  uses: [UseStatement module_name="cook_cc" line=1]
  imports: []
  config_blocks: []
  recipes:
    Recipe name="probe" line=3
      deps: []
      ingredients: []
      excludes: []
      steps:
        Lua code="cook_cc.register_finder(\"mylib\", function(_opts)"
        Lua code="    return { found = true, cflags = \"\", libs = \"-lmylib\", system_libs = {\"mylib\"}, include_dirs = {}, lib_dirs = {}, frameworks = {}, version = nil, tried = {} }"
        Lua code="end)"
        Lua code="local r = cook_cc.find(\"mylib\")"
        Lua code="if not r.found then error(\"expected project-registered hit\") end"
  chores:
```

(If the parse harness coalesces multi-line `> ... > end)` into a single Lua step, accept whatever the harness produces — write parse.txt with the actual output. The fixture's purpose is surface-stability, not assertion of exact line splitting.)

- [ ] **Step 3: Write notes.md**

```markdown
# cc-register-finder

Locks §9.2.3.12 `cc.register_finder` parse surface (function-value second
argument, multi-line Lua block at the recipe level). Runtime override
priority is exercised by busted/finders/resolver_spec.lua.
```

- [ ] **Step 4: Run harness**

Run: `cargo test -p cook-lang --test conformance cc_register_finder`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git -C ~/dev/cook add standard/conformance/positive/cc-register-finder/
git -C ~/dev/cook commit -m "test(conformance): cc-register-finder parse fixture (M1)"
```

### Task A10: Conformance fixture — cc-frameworks-on-link

**Files:**
- Create: `standard/conformance/positive/cc-frameworks-on-link/{Cookfile,parse.txt,notes.md,src/main.c}`

- [ ] **Step 1: Create directory + source + Cookfile**

```bash
mkdir -p standard/conformance/positive/cc-frameworks-on-link/src
```

Write `src/main.c`:

```c
int main(void) { return 0; }
```

Write `Cookfile`:

```cook
use cook_cc

recipe app
    > cook_cc.bin("app", {
    >     sources    = { "src/main.c" },
    >     frameworks = { "OpenGL", "Cocoa" },
    > })
```

- [ ] **Step 2: Generate parse.txt**

Run: `cook --parse-only standard/conformance/positive/cc-frameworks-on-link/Cookfile > standard/conformance/positive/cc-frameworks-on-link/parse.txt`

Expected (record the actual harness output verbatim):

```
Cookfile
  uses: [UseStatement module_name="cook_cc" line=1]
  imports: []
  config_blocks: []
  recipes:
    Recipe name="app" line=3
      deps: []
      ingredients: []
      excludes: []
      steps:
        Lua code="cook_cc.bin(\"app\", {"
        Lua code="    sources    = { \"src/main.c\" },"
        Lua code="    frameworks = { \"OpenGL\", \"Cocoa\" },"
        Lua code="})"
  chores:
```

- [ ] **Step 3: Write notes.md**

```markdown
# cc-frameworks-on-link

Locks §9.2.3.7 `LinkOpts.frameworks` Cookfile surface — recognised as an
option key on `cc.bin`/`cc.lib`/`cc.shared`. Runtime emission of
`-framework <name>` is verified by cook_cc/spec/cc_spec.lua.
```

- [ ] **Step 4: Run harness**

Run: `cargo test -p cook-lang --test conformance cc_frameworks_on_link`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git -C ~/dev/cook add standard/conformance/positive/cc-frameworks-on-link/
git -C ~/dev/cook commit -m "test(conformance): cc-frameworks-on-link parse fixture (M1)"
```

### Task A11: Conformance fixture — cc-frameworks-transitive

**Files:**
- Create: `standard/conformance/positive/cc-frameworks-transitive/{Cookfile,parse.txt,notes.md,src/lib.c,src/main.c}`

- [ ] **Step 1: Create directory + sources + Cookfile**

```bash
mkdir -p standard/conformance/positive/cc-frameworks-transitive/src
```

Write `src/lib.c`:

```c
int gfx_init(void) { return 0; }
```

Write `src/main.c`:

```c
int gfx_init(void);
int main(void) { return gfx_init(); }
```

Write `Cookfile`:

```cook
use cook_cc

recipe app
    > cook_cc.lib("gfx", {
    >     sources    = { "src/lib.c" },
    >     frameworks = { "OpenGL" },
    > })
    > cook_cc.bin("app", {
    >     sources = { "src/main.c" },
    >     links   = { "gfx" },
    > })
```

- [ ] **Step 2: Generate parse.txt**

Run: `cook --parse-only standard/conformance/positive/cc-frameworks-transitive/Cookfile > standard/conformance/positive/cc-frameworks-transitive/parse.txt`

(Capture the actual harness output. Expected shape: two Lua chunks, one for the `cc.lib` block, one for the `cc.bin` block, with `frameworks = { "OpenGL" }` preserved verbatim.)

- [ ] **Step 3: Write notes.md**

```markdown
# cc-frameworks-transitive

Locks §9.2.4 propagation of `frameworks` from a `cc.lib` target into a
downstream `cc.bin` via the `links` chain. Parse-only: this fixture
confirms the Cookfile shape parses; the resolve-and-emit behaviour is
verified by cook_cc/spec/transitive_spec.lua and targets_spec.lua.
```

- [ ] **Step 4: Run harness**

Run: `cargo test -p cook-lang --test conformance cc_frameworks_transitive`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git -C ~/dev/cook add standard/conformance/positive/cc-frameworks-transitive/
git -C ~/dev/cook commit -m "test(conformance): cc-frameworks-transitive parse fixture (M1)"
```

### Task A12: Open PR cook#A as draft

- [ ] **Step 1: Push the branch**

```bash
git -C ~/dev/cook checkout -b shi-134-m1-cc-finders
git -C ~/dev/cook push -u origin shi-134-m1-cc-finders
```

- [ ] **Step 2: Open draft PR**

```bash
gh pr create --draft --title "SHI-134: M1 — cc finder multi-strategy chain (Standard v0.2)" --body "$(cat <<'EOF'
Spec-first half of M1 (SHI-134). Standard §9.2 graduates from v0.1 to v0.2;
parse-only conformance fixtures lock the new surface. Implementation lands
in cook-modules#B; this PR moves to ready-for-merge once cook_cc 0.2.0-1 is
published to rocks.usecook.com.

## Standard changes

- §9.2.3.7 `LinkOpts.frameworks` (new field)
- §9.2.3.8 `cc.find` v0.2 rewrite (multi-strategy chain, `FindResult.tried`)
- §9.2.3.12 `cc.register_finder` (new)
- §9.2.3.13 `cc.find_or_error` (new)
- §9.2.4 transitive propagation grows `frameworks`
- §9.2.5 catalogue grows two entries

## Conformance fixtures (parse-only)

- `cc-find-version-constraint`
- `cc-find-or-error`
- `cc-find-tried-field`
- `cc-register-finder`
- `cc-frameworks-on-link`
- `cc-frameworks-transitive`

## Out of scope

- Execute-mode conformance: SHI-210
- luarocks dual-server bug: SHI-211
- E-pre-v1-checklist slug warning: SHI-212

🤖 Generated with [Claude Code](https://claude.com/claude-code)
EOF
)"
```

Expected: PR URL returned. Mark PR cook#A as draft.

---

## Stage B — cook_cc resolver implementation (cook-modules repo, PR cook-modules#B)

All Stage B tasks run inside `~/dev/cook-modules`. Branch:

```bash
git -C ~/dev/cook-modules checkout -b shi-134-m1-cc-finders
```

### Task B1: Extend cook_stub.lua with new mocks

**Files:**
- Modify: `cook_cc/spec/cook_stub.lua`

- [ ] **Step 1: Add platform-OS override + helpers**

Replace the body of `cook_cc/spec/cook_stub.lua` with:

```lua
-- Minimal stand-ins for the cook-engine-provided globals so busted can
-- exercise cook_cc submodules in isolation. Each spec resets the state.

local M = {}

local cache_store = {}
local export_store = {}
local added_units = {}
local sh_handlers = {}      -- map[string] -> function(cmd) -> stdout
local pkg_responses = {}    -- name -> { cflags, libs, version, exists }
local tool_responses = {}   -- "tool args" -> output
local file_exists_set = {}  -- path -> bool
local platform_os = "linux"

function M.reset()
    cache_store = {}
    export_store = {}
    added_units = {}
    sh_handlers = {}
    pkg_responses = {}
    tool_responses = {}
    file_exists_set = {}
    platform_os = "linux"
end

function M.set_platform_os(os)
    platform_os = os
end

function M.set_sh_handler(prefix, handler)
    sh_handlers[prefix] = handler
end

function M.set_pkg_config_response(name, response)
    -- response = { exists = bool, cflags = string?, libs = string?, version = string? }
    pkg_responses[name] = response
end

function M.set_tool_config_response(cmd, output)
    tool_responses[cmd] = output
end

function M.set_file_exists(path, exists)
    file_exists_set[path] = exists and true or false
end

function M.added_units()
    return added_units
end

local function pkg_dispatch(cmd)
    -- "pkg-config --exists NAME" / "--cflags NAME" / "--libs NAME" / "--modversion NAME"
    local op, name = cmd:match("pkg%-config %-%-(%S+) (.+)$")
    if not op then return nil end
    local r = pkg_responses[name]
    if not r then return nil end
    if op == "exists" then
        if r.exists then return "" end
        error("[cook_stub] pkg-config exists " .. name .. " failed")
    elseif op == "cflags"      then return (r.cflags or "") .. "\n"
    elseif op == "libs"        then return (r.libs   or "") .. "\n"
    elseif op == "modversion"  then return (r.version or "") .. "\n"
    end
    return nil
end

function M.install()
    _G.cook = {
        env = setmetatable({}, { __index = function() return nil end }),
        platform = setmetatable({}, { __index = function(_, k)
            if k == "os" then return platform_os end
        end }),
        cache = {
            get = function(k) return cache_store[k] end,
            set = function(k, v) cache_store[k] = v end,
        },
        export = function(name, info) export_store[name] = info end,
        import = function(name) return export_store[name] end,
        add_unit = function(u) added_units[#added_units + 1] = u end,
        sh = function(cmd)
            local pkg = pkg_dispatch(cmd)
            if pkg ~= nil then return pkg end
            local tool = tool_responses[cmd]
            if tool ~= nil then return tool end
            for prefix, fn in pairs(sh_handlers) do
                if cmd:sub(1, #prefix) == prefix then return fn(cmd) end
            end
            error("[cook_stub] unhandled sh: " .. cmd)
        end,
    }

    _G.fs = {
        exists = function(p)
            if file_exists_set[p] ~= nil then return file_exists_set[p] end
            local h = sh_handlers["__exists"]
            return h and h(p) or true
        end,
        read = function(p)
            local h = sh_handlers["__read"]
            return h and h(p) or ""
        end,
        write   = function(p, content)
            added_units[#added_units + 1] = { kind = "fs.write", path = p, content = content }
        end,
        mkdir_p = function() end,
        glob    = function(_) return {} end,
    }

    _G.path = {
        stem = function(p) return p:match("([^/]+)%.[^.]+$") end,
        dir  = function(p) return p:match("(.+)/[^/]+$") or "." end,
    }
end

return M
```

- [ ] **Step 2: Confirm existing specs still pass**

```bash
cd ~/dev/cook-modules/cook_cc && busted
```

Expected: existing 41 tests pass.

- [ ] **Step 3: Commit**

```bash
git -C ~/dev/cook-modules add cook_cc/spec/cook_stub.lua
git -C ~/dev/cook-modules commit -m "test(cook_cc): extend cook_stub with platform.os + pkg-config / tool / file mocks"
```

### Task B2: version.lua — parser

**Files:**
- Create: `cook_cc/version.lua`
- Create: `cook_cc/spec/finders/version_spec.lua`

- [ ] **Step 1: Write the failing parser test**

Create `cook_cc/spec/finders/version_spec.lua`:

```lua
local stub = require("cook_stub")

describe("version.parse", function()
    before_each(function()
        stub.reset(); stub.install()
        package.loaded["cook_cc.version"] = nil
    end)

    it("parses major.minor.patch", function()
        local v = require("cook_cc.version")
        assert.same({ major = 4, minor = 5, patch = 0 }, v.parse("4.5.0"))
    end)

    it("zero-fills missing fields", function()
        local v = require("cook_cc.version")
        assert.same({ major = 4, minor = 0, patch = 0 }, v.parse("4"))
        assert.same({ major = 4, minor = 5, patch = 0 }, v.parse("4.5"))
    end)

    it("captures prerelease tags", function()
        local v = require("cook_cc.version")
        local parsed = v.parse("4.0.0-rc1")
        assert.equals("rc1", parsed.prerelease)
    end)

    it("drops build metadata after +", function()
        local v = require("cook_cc.version")
        assert.same({ major = 1, minor = 2, patch = 3 }, v.parse("1.2.3+sha.abc"))
    end)

    it("returns nil on non-numeric major", function()
        local v = require("cook_cc.version")
        assert.is_nil(v.parse("garbage"))
        assert.is_nil(v.parse(""))
    end)
end)
```

- [ ] **Step 2: Run test to verify it fails**

```bash
cd ~/dev/cook-modules/cook_cc && busted spec/finders/version_spec.lua
```

Expected: FAIL — `module 'cook_cc.version' not found`.

- [ ] **Step 3: Write minimal parser**

Create `cook_cc/version.lua`:

```lua
local M = {}

local function parse_int(s) return tonumber(s, 10) end

function M.parse(s)
    if type(s) ~= "string" or s == "" then return nil end
    -- Strip build metadata first.
    local trunc = s:match("^([^+]+)") or s
    -- Split off prerelease.
    local core, prerelease = trunc:match("^([^-]+)%-?(.*)$")
    if not core then return nil end
    if prerelease == "" then prerelease = nil end
    local major, minor, patch = core:match("^(%d+)%.?(%d*)%.?(%d*)$")
    if not major then return nil end
    local M_, m_, p_ = parse_int(major), parse_int(minor or "") or 0, parse_int(patch or "") or 0
    if not M_ then return nil end
    return { major = M_, minor = m_, patch = p_, prerelease = prerelease }
end

return M
```

- [ ] **Step 4: Run test to verify pass**

```bash
busted spec/finders/version_spec.lua
```

Expected: 5 passes.

- [ ] **Step 5: Commit**

```bash
git -C ~/dev/cook-modules add cook_cc/version.lua cook_cc/spec/finders/version_spec.lua
git -C ~/dev/cook-modules commit -m "feat(cook_cc): version.lua parser — semver core + prerelease"
```

### Task B3: version.lua — satisfies()

**Files:**
- Modify: `cook_cc/version.lua`
- Modify: `cook_cc/spec/finders/version_spec.lua`

- [ ] **Step 1: Write failing satisfies tests**

Append to `cook_cc/spec/finders/version_spec.lua`:

```lua
describe("version.satisfies", function()
    before_each(function()
        package.loaded["cook_cc.version"] = nil
    end)

    it("honours a single >= clause", function()
        local v = require("cook_cc.version")
        assert.is_true(v.satisfies("4.5.0", ">=4.0"))
        assert.is_false(v.satisfies("3.9.0", ">=4.0"))
    end)

    it("treats missing operator as =", function()
        local v = require("cook_cc.version")
        assert.is_true(v.satisfies("4.0.0", "4.0"))
        assert.is_false(v.satisfies("4.0.1", "=4.0.0"))
    end)

    it("supports comma-AND clauses", function()
        local v = require("cook_cc.version")
        assert.is_true(v.satisfies("4.5.0", ">=4.0,<5.0"))
        assert.is_false(v.satisfies("5.0.0", ">=4.0,<5.0"))
    end)

    it("excludes prerelease against a non-prerelease constraint", function()
        local v = require("cook_cc.version")
        assert.is_false(v.satisfies("4.0.0-rc1", ">=4.0.0"))
        assert.is_true(v.satisfies("4.0.0-rc1", ">=4.0.0-rc1"))
    end)

    it("returns false when detected is unparseable", function()
        local v = require("cook_cc.version")
        assert.is_false(v.satisfies("garbage", ">=4.0"))
    end)

    it("vacuously passes empty constraint", function()
        local v = require("cook_cc.version")
        assert.is_true(v.satisfies("1.0.0", ""))
    end)

    it("tolerates whitespace inside the constraint", function()
        local v = require("cook_cc.version")
        assert.is_true(v.satisfies("4.5.0", " >= 4.0 , < 5 "))
    end)

    it("zero-fills <X strictly", function()
        local v = require("cook_cc.version")
        assert.is_false(v.satisfies("4.0.0", "<4"))
    end)

    it("raises on unparseable constraint clause", function()
        local v = require("cook_cc.version")
        assert.has_error(function() v.satisfies("4.0.0", ">=abc") end)
    end)
end)
```

- [ ] **Step 2: Run, verify fail**

```bash
busted spec/finders/version_spec.lua
```

Expected: FAIL — `attempt to call a nil value (field 'satisfies')`.

- [ ] **Step 3: Implement satisfies()**

Append to `cook_cc/version.lua` (before the `return M`):

```lua
local OPERATORS = { ">=", "<=", ">", "<", "=" }

local function strip_whitespace(s) return (s or ""):gsub("%s+", "") end

local function split_clauses(constraint)
    if constraint == "" then return {} end
    local clauses = {}
    for piece in constraint:gmatch("[^,]+") do
        clauses[#clauses + 1] = piece
    end
    return clauses
end

local function parse_clause(piece)
    for _, op in ipairs(OPERATORS) do
        if piece:sub(1, #op) == op then
            local v = M.parse(piece:sub(#op + 1))
            if not v then error("[cc.find] unparseable version in clause: " .. piece) end
            return op, v
        end
    end
    -- No operator → =
    local v = M.parse(piece)
    if not v then error("[cc.find] unparseable version in clause: " .. piece) end
    return "=", v
end

local function cmp_core(a, b)
    if a.major ~= b.major then return a.major - b.major end
    if a.minor ~= b.minor then return a.minor - b.minor end
    return a.patch - b.patch
end

-- For `=`, missing fields in the constraint act as wildcards.
local function matches_eq(detected, constraint_str)
    local maj, min, pat = constraint_str:match("^(%d+)%.?(%d*)%.?(%d*)$")
    if not maj then return false end
    if tonumber(maj) ~= detected.major then return false end
    if min ~= "" and tonumber(min) ~= detected.minor then return false end
    if pat ~= "" and tonumber(pat) ~= detected.patch then return false end
    return true
end

local function check_clause(detected, op, constraint_v, raw_v_string)
    local c = cmp_core(detected, constraint_v)
    -- Prerelease exclusion: detected has prerelease, constraint does not, same core → reject for >=/>/<=/</=.
    if detected.prerelease and not constraint_v.prerelease and c == 0 then return false end
    if op == ">=" then return c >= 0
    elseif op == ">"  then return c >  0
    elseif op == "<=" then return c <= 0
    elseif op == "<"  then return c <  0
    elseif op == "="  then return matches_eq(detected, raw_v_string)
    end
    return false
end

function M.satisfies(detected_str, constraint_str)
    constraint_str = strip_whitespace(constraint_str)
    local clauses = split_clauses(constraint_str)
    if #clauses == 0 then return true end
    local detected = M.parse(detected_str)
    if not detected then return false end
    for _, piece in ipairs(clauses) do
        local op, v = parse_clause(piece)
        -- For `=`, recover the raw version string from the clause (op-stripped).
        local raw
        for _, candidate in ipairs(OPERATORS) do
            if piece:sub(1, #candidate) == candidate then raw = piece:sub(#candidate + 1); break end
        end
        raw = raw or piece
        if not check_clause(detected, op, v, raw) then return false end
    end
    return true
end
```

- [ ] **Step 4: Run, verify pass**

```bash
busted spec/finders/version_spec.lua
```

Expected: 14 passes (5 parser + 9 satisfies).

- [ ] **Step 5: Commit**

```bash
git -C ~/dev/cook-modules add cook_cc/version.lua cook_cc/spec/finders/version_spec.lua
git -C ~/dev/cook-modules commit -m "feat(cook_cc): version.satisfies — semver constraint check"
```

### Task B4: pkg_config helper

**Files:**
- Create: `cook_cc/finders/pkg_config.lua`
- Create: `cook_cc/spec/finders/pkg_config_spec.lua`

- [ ] **Step 1: Write failing test**

Create `cook_cc/spec/finders/pkg_config_spec.lua`:

```lua
local stub = require("cook_stub")

describe("finders.pkg_config.try", function()
    before_each(function()
        stub.reset(); stub.install()
        package.loaded["cook_cc.finders.pkg_config"] = nil
    end)

    it("returns hit Attempt with payload on success", function()
        stub.set_pkg_config_response("zlib", {
            exists = true,
            cflags = "-I/usr/include",
            libs   = "-L/usr/lib -lz",
            version = "1.2.13",
        })
        local pkg = require("cook_cc.finders.pkg_config")
        local a = pkg.try("zlib")
        assert.equals("hit", a.outcome)
        assert.equals("pkg-config", a.strategy)
        assert.equals("1.2.13", a.payload.version)
        assert.same({ "z" }, a.payload.system_libs)
        assert.same({ "/usr/include" }, a.payload.include_dirs)
        assert.same({ "/usr/lib" }, a.payload.lib_dirs)
        assert.same({}, a.payload.frameworks)
    end)

    it("returns nil on miss", function()
        local pkg = require("cook_cc.finders.pkg_config")
        local a = pkg.try("nonesuch")
        assert.is_nil(a)
    end)

    it("parses framework tokens", function()
        stub.set_pkg_config_response("gl-macos", {
            exists = true, cflags = "", libs = "-framework OpenGL", version = "1.0",
        })
        local pkg = require("cook_cc.finders.pkg_config")
        local a = pkg.try("gl-macos")
        assert.same({ "OpenGL" }, a.payload.frameworks)
    end)
end)
```

- [ ] **Step 2: Run, verify fail**

```bash
busted spec/finders/pkg_config_spec.lua
```

Expected: FAIL — module not found.

- [ ] **Step 3: Implement helper**

Create `cook_cc/finders/pkg_config.lua`:

```lua
local lpeg = require("lpeg")
local P, S, C, Ct = lpeg.P, lpeg.S, lpeg.C, lpeg.Ct

local M = {}

local space     = S(" \t")^0
local non_space = (P(1) - S(" \t"))^1
local include   = P("-I") * C(non_space)
local libdir    = P("-L") * C(non_space)
local syslib    = P("-l") * C(non_space)
local define    = P("-D") * C(non_space)
local framework = P("-framework") * S(" \t")^1 * C(non_space)
local other     = C(non_space)

local token = framework / function(v) return { kind = "framework", value = v } end
            + include   / function(v) return { kind = "include",   value = v } end
            + libdir    / function(v) return { kind = "libdir",    value = v } end
            + syslib    / function(v) return { kind = "syslib",    value = v } end
            + define    / function(v) return { kind = "define",    value = v } end
            + other     / function(v) return { kind = "other",     value = v } end

local line_pattern = Ct((space * token)^0 * space)
local function parse_tokens(s) return line_pattern:match(s or "") or {} end

local function shell_chomp(s) return (s or ""):gsub("%s+$", "") end

local function try_sh(cmd)
    local ok, out = pcall(cook.sh, cmd)
    return ok, ok and shell_chomp(out) or nil
end

function M.try(name)
    local ok = try_sh("pkg-config --exists " .. name)
    if not ok then return nil end
    local _, cflags  = try_sh("pkg-config --cflags " .. name)
    local _, libs    = try_sh("pkg-config --libs "   .. name)
    local _, version = try_sh("pkg-config --modversion " .. name)
    if version == "" then version = nil end

    local payload = {
        cflags = cflags or "",
        libs   = libs or "",
        system_libs = {}, include_dirs = {}, lib_dirs = {}, frameworks = {},
        version = version,
    }
    local function bucket(toks)
        for _, t in ipairs(toks) do
            if     t.kind == "include"   then payload.include_dirs[#payload.include_dirs + 1] = t.value
            elseif t.kind == "libdir"    then payload.lib_dirs[#payload.lib_dirs + 1] = t.value
            elseif t.kind == "syslib"    then payload.system_libs[#payload.system_libs + 1] = t.value
            elseif t.kind == "framework" then payload.frameworks[#payload.frameworks + 1] = t.value
            end
        end
    end
    bucket(parse_tokens(payload.cflags))
    bucket(parse_tokens(payload.libs))

    return { strategy = "pkg-config", outcome = "hit", reason = "", payload = payload }
end

function M.main_chain(name, opts)
    local attempt = M.try(name)
    if attempt then
        if opts.version and attempt.payload.version then
            local ver = require("cook_cc.version")
            if not ver.satisfies(attempt.payload.version, opts.version) then
                return { strategy = "pkg-config", outcome = "miss",
                         reason = "detected version " .. attempt.payload.version
                                  .. " does not satisfy " .. opts.version }
            end
        elseif opts.version and not attempt.payload.version then
            return { strategy = "pkg-config", outcome = "miss",
                     reason = "could not determine version; constraint " .. opts.version
                              .. " cannot be honoured" }
        end
        return attempt
    end
    return { strategy = "pkg-config", outcome = "miss",
             reason = "package '" .. name .. "' not found by pkg-config" }
end

return M
```

- [ ] **Step 4: Run, verify pass**

```bash
busted spec/finders/pkg_config_spec.lua
```

Expected: 3 passes.

- [ ] **Step 5: Commit**

```bash
git -C ~/dev/cook-modules add cook_cc/finders/pkg_config.lua cook_cc/spec/finders/pkg_config_spec.lua
git -C ~/dev/cook-modules commit -m "feat(cook_cc): finders/pkg_config helper + main-chain strategy"
```

### Task B5: bare_probe helper

**Files:**
- Create: `cook_cc/finders/bare_probe.lua`
- Create: `cook_cc/spec/finders/bare_probe_spec.lua`

- [ ] **Step 1: Write failing test**

Create `cook_cc/spec/finders/bare_probe_spec.lua`:

```lua
local stub = require("cook_stub")

describe("finders.bare_probe.try", function()
    before_each(function()
        stub.reset(); stub.install()
        package.loaded["cook_cc.finders.bare_probe"] = nil
        -- Mock toolchain so cc.print-search-dirs is callable.
        stub.set_sh_handler("cc -print-search-dirs", function()
            return "libraries: =/usr/lib:/usr/local/lib\n"
        end)
    end)

    it("hits when libNAME.so exists on default linker path", function()
        stub.set_file_exists("/usr/lib/libz.so", true)
        local bare = require("cook_cc.finders.bare_probe")
        local a = bare.try("z")
        assert.equals("hit", a.outcome)
        assert.same({ "z" }, a.payload.system_libs)
        assert.equals("-lz", a.payload.libs)
    end)

    it("returns nil when nothing exists", function()
        local bare = require("cook_cc.finders.bare_probe")
        local a = bare.try("nonesuch")
        assert.is_nil(a)
    end)

    it("prefers .dylib first on macOS", function()
        stub.set_platform_os("macos")
        stub.set_file_exists("/usr/lib/libz.dylib", true)
        local bare = require("cook_cc.finders.bare_probe")
        local a = bare.try("z")
        assert.equals("hit", a.outcome)
    end)

    it("main_chain returns skip when version constraint set", function()
        local bare = require("cook_cc.finders.bare_probe")
        local a = bare.main_chain("z", { version = ">=1.0" })
        assert.equals("skip", a.outcome)
        assert.matches("version", a.reason)
    end)

    it("main_chain returns miss when not found and no version", function()
        local bare = require("cook_cc.finders.bare_probe")
        local a = bare.main_chain("nonesuch", {})
        assert.equals("miss", a.outcome)
    end)
end)
```

- [ ] **Step 2: Run, verify fail**

```bash
busted spec/finders/bare_probe_spec.lua
```

Expected: FAIL — module not found.

- [ ] **Step 3: Implement helper**

Create `cook_cc/finders/bare_probe.lua`:

```lua
local M = {}

local function search_dirs()
    local cached = cook.cache.get("cc.linker-search-dirs")
    if cached then return cached end
    local dirs = { "/usr/lib", "/usr/local/lib" }
    local ok, out = pcall(cook.sh, "cc -print-search-dirs")
    if ok and out then
        local libs_line = out:match("libraries:%s*=([^\n]+)")
        if libs_line then
            for d in libs_line:gmatch("[^:]+") do dirs[#dirs + 1] = d end
        end
    end
    cook.cache.set("cc.linker-search-dirs", dirs)
    return dirs
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

function M.try(name)
    for _, dir in ipairs(search_dirs()) do
        for _, ext in ipairs(extensions()) do
            local p = dir .. "/lib" .. name .. ext
            if fs.exists(p) then
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

- [ ] **Step 4: Run, verify pass**

```bash
busted spec/finders/bare_probe_spec.lua
```

Expected: 5 passes.

- [ ] **Step 5: Commit**

```bash
git -C ~/dev/cook-modules add cook_cc/finders/bare_probe.lua cook_cc/spec/finders/bare_probe_spec.lua
git -C ~/dev/cook-modules commit -m "feat(cook_cc): finders/bare_probe — file-existence fallback strategy"
```

### Task B6: header_probe + tool_config helpers

**Files:**
- Create: `cook_cc/finders/header_probe.lua`
- Create: `cook_cc/finders/tool_config.lua`

- [ ] **Step 1: Implement header_probe**

Create `cook_cc/finders/header_probe.lua`:

```lua
local M = {}

-- Parses `#define MACRO "X.Y.Z"` or `#define MACRO X.Y.Z` from a header file.
-- Returns the captured value as a string, or nil if not found / unreadable.
function M.parse_define(path, macro)
    if not fs.exists(path) then return nil end
    local content = fs.read(path) or ""
    -- Quoted form first.
    local v = content:match("#define%s+" .. macro .. "%s+\"([^\"]+)\"")
    if v then return v end
    -- Bare form: capture digits / dots / dashes / alphanumerics until newline.
    v = content:match("#define%s+" .. macro .. "%s+([%w%.%-]+)")
    return v
end

return M
```

- [ ] **Step 2: Implement tool_config**

Create `cook_cc/finders/tool_config.lua`:

```lua
local M = {}

-- Runs `<tool> <args>` via cook.sh; returns trimmed stdout on success, nil on error.
function M.try(tool, args)
    local cmd = tool .. " " .. args
    local ok, out = pcall(cook.sh, cmd)
    if not ok then return nil end
    return (out or ""):gsub("%s+$", "")
end

return M
```

- [ ] **Step 3: Commit (no dedicated spec — exercised indirectly through curated finder specs)**

```bash
git -C ~/dev/cook-modules add cook_cc/finders/header_probe.lua cook_cc/finders/tool_config.lua
git -C ~/dev/cook-modules commit -m "feat(cook_cc): finders helpers — header_probe + tool_config"
```

### Task B7: Resolver shell + register API

**Files:**
- Modify: `cook_cc/finder.lua` (rewrite)
- Create: `cook_cc/spec/finders/resolver_spec.lua`

- [ ] **Step 1: Write failing resolver tests**

Create `cook_cc/spec/finders/resolver_spec.lua`:

```lua
local stub = require("cook_stub")

local function reset_all()
    stub.reset(); stub.install()
    package.loaded["cook_cc.finder"] = nil
    package.loaded["cook_cc.finders.pkg_config"] = nil
    package.loaded["cook_cc.finders.bare_probe"] = nil
    package.loaded["cook_cc.finders"] = nil
    -- Default: no pkg / no bare matches.
    stub.set_sh_handler("cc -print-search-dirs",
        function() return "libraries: =/usr/lib\n" end)
end

describe("finder resolver", function()
    before_each(reset_all)

    it("returns blank result + tried list on full miss", function()
        local f = require("cook_cc.finder")
        local r = f.find("nonesuch")
        assert.is_false(r.found)
        assert.is_table(r.tried)
        -- pkg-config + bare-probe at minimum.
        assert.is_true(#r.tried >= 2)
        assert.same({}, r.system_libs)
    end)

    it("caches result by name+opts", function()
        local f = require("cook_cc.finder")
        local r1 = f.find("nonesuch")
        local r2 = f.find("nonesuch")
        assert.equals(r1, r2)
    end)

    it("project-registered finder runs first and wins", function()
        local f = require("cook_cc.finder")
        f.register("zzz", function(_opts)
            return { found = true, cflags = "", libs = "-lzzz",
                     system_libs = {"zzz"}, include_dirs = {}, lib_dirs = {},
                     frameworks = {}, version = "9.9.9", tried = {} }
        end)
        local r = f.find("zzz")
        assert.is_true(r.found)
        assert.same({ "zzz" }, r.system_libs)
        assert.equals("project:zzz", r.tried[1].strategy)
        assert.equals("hit", r.tried[1].outcome)
    end)

    it("re-registration replaces silently", function()
        local f = require("cook_cc.finder")
        local hits = 0
        f.register("zzz", function(_) hits = hits + 1
            return { found = true, cflags = "", libs = "", system_libs = {},
                     include_dirs = {}, lib_dirs = {}, frameworks = {}, tried = {} } end)
        f.register("zzz", function(_) hits = hits + 100
            return { found = true, cflags = "", libs = "", system_libs = {},
                     include_dirs = {}, lib_dirs = {}, frameworks = {}, tried = {} } end)
        f.find("zzz")
        assert.equals(100, hits)
    end)

    it("falls through to pkg-config when curated/project all miss", function()
        stub.set_pkg_config_response("zlib", {
            exists = true, cflags = "", libs = "-lz", version = "1.2.13",
        })
        local f = require("cook_cc.finder")
        local r = f.find("zlib")
        assert.is_true(r.found)
        assert.equals("1.2.13", r.version)
    end)

    it("cache key distinguishes opts", function()
        local f = require("cook_cc.finder")
        stub.set_pkg_config_response("zlib", {
            exists = true, cflags = "", libs = "-lz", version = "1.2.13",
        })
        local r1 = f.find("zlib")
        local r2 = f.find("zlib", { version = ">=99.0" })
        assert.is_true(r1.found)
        assert.is_false(r2.found)
    end)
end)
```

- [ ] **Step 2: Run, verify fail**

```bash
busted spec/finders/resolver_spec.lua
```

Expected: FAIL — current `finder.lua` exposes only `M.find`, not `M.register`.

- [ ] **Step 3: Rewrite `cook_cc/finder.lua`**

Replace contents:

```lua
local M = {}

-- Per-VM project-registered finder registry.
M._registry = M._registry or {}

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

local function canonical_opts(opts)
    if not opts then return "" end
    local keys = {}
    for k in pairs(opts) do keys[#keys + 1] = tostring(k) end
    table.sort(keys)
    local parts = {}
    for _, k in ipairs(keys) do
        local v = opts[k]
        if type(v) == "table" then v = table.concat(v, ",") end
        parts[#parts + 1] = k .. "=" .. tostring(v)
    end
    return table.concat(parts, ",")
end

local function build_result(hit, tried)
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

function M.register(name, finder)
    if type(finder) ~= "function" then
        error("[cc.register_finder] register_finder for '" .. tostring(name)
              .. "' requires a function, got " .. type(finder), 2)
    end
    M._registry[name] = finder
end

local function project_strategy(name, opts)
    local fn = M._registry[name]
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

local function curated_strategy(name, opts)
    local curated = require("cook_cc.finders")
    local fn = curated.lookup(name)
    if not fn then
        return { strategy = "curated:" .. name, outcome = "skip",
                 reason = "no curated finder for '" .. name .. "'" }
    end
    return fn(opts)
end

function M.find(name, opts)
    opts = opts or {}
    local cache_key = "cc.find:" .. name .. ":" .. canonical_opts(opts)
    local cached = cook.cache.get(cache_key)
    if cached then return cached end

    local pkg  = require("cook_cc.finders.pkg_config")
    local bare = require("cook_cc.finders.bare_probe")

    local chain = {
        function() return project_strategy(name, opts) end,
        function() return curated_strategy(name, opts) end,
        function() return pkg.main_chain(name, opts) end,
        function() return bare.main_chain(name, opts) end,
    }

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

return M
```

- [ ] **Step 4: Stub the curated registry (will be replaced by Task B8)**

Until Task B8 lands the real registry, the resolver tests need a stub that returns `nil` for every lookup. Create temporary `cook_cc/finders/init.lua`:

```lua
local M = {}
function M.lookup(_) return nil end
return M
```

- [ ] **Step 5: Run, verify pass**

```bash
busted spec/finders/resolver_spec.lua
```

Expected: 6 passes.

- [ ] **Step 6: Commit**

```bash
git -C ~/dev/cook-modules add cook_cc/finder.lua cook_cc/finders/init.lua cook_cc/spec/finders/resolver_spec.lua
git -C ~/dev/cook-modules commit -m "feat(cook_cc): resolver shell + register API (project-stage strategy)"
```

### Task B8: Curated registry + aliases

**Files:**
- Modify: `cook_cc/finders/init.lua` (replace stub)

- [ ] **Step 1: Implement the real registry**

Replace `cook_cc/finders/init.lua`:

```lua
local M = {}

local CURATED = {
    -- name → module path. Aliases below.
    raylib  = "cook_cc.finders.raylib",
    sdl2    = "cook_cc.finders.sdl2",
    openal  = "cook_cc.finders.openal",
    gl      = "cook_cc.finders.gl",
    threads = "cook_cc.finders.threads",
    zlib    = "cook_cc.finders.zlib",
    libcurl = "cook_cc.finders.libcurl",
}

local ALIASES = {
    opengl = "gl",
}

function M.lookup(name)
    local canonical = ALIASES[name] or name
    local mod_path = CURATED[canonical]
    if not mod_path then return nil end
    local mod = require(mod_path)
    return function(opts)
        local attempt = mod.find(opts)
        attempt.strategy = "curated:" .. canonical
        return attempt
    end
end

return M
```

- [ ] **Step 2: Verify resolver tests still pass (curated finders not yet implemented; lookup returns nil for unknown names; existing tests use `nonesuch` and `zzz` and `zlib`)**

```bash
busted spec/finders/resolver_spec.lua
```

Expected: 6 passes. (Curated stubs are not yet required by the existing resolver tests because each test uses an unregistered name; the registry returns nil for them.)

- [ ] **Step 3: Commit**

```bash
git -C ~/dev/cook-modules add cook_cc/finders/init.lua
git -C ~/dev/cook-modules commit -m "feat(cook_cc): curated finder registry + opengl→gl alias"
```

### Task B9: Curated finder — zlib (smallest first)

**Files:**
- Create: `cook_cc/finders/zlib.lua`
- Create: `cook_cc/spec/finders/zlib_spec.lua`

- [ ] **Step 1: Write failing test**

Create `cook_cc/spec/finders/zlib_spec.lua`:

```lua
local stub = require("cook_stub")

describe("finders.zlib", function()
    before_each(function()
        stub.reset(); stub.install()
        package.loaded["cook_cc.finders.zlib"] = nil
        package.loaded["cook_cc.finders.pkg_config"] = nil
        package.loaded["cook_cc.finders.bare_probe"] = nil
        stub.set_sh_handler("cc -print-search-dirs",
            function() return "libraries: =/usr/lib\n" end)
    end)

    it("Linux happy path via pkg-config", function()
        stub.set_pkg_config_response("zlib", {
            exists = true, cflags = "", libs = "-lz", version = "1.2.13",
        })
        local f = require("cook_cc.finders.zlib")
        local a = f.find({})
        assert.equals("hit", a.outcome)
        assert.same({ "z" }, a.payload.system_libs)
        assert.equals("1.2.13", a.payload.version)
    end)

    it("falls back to bare probe when pkg-config misses", function()
        stub.set_file_exists("/usr/lib/libz.so", true)
        local f = require("cook_cc.finders.zlib")
        local a = f.find({})
        assert.equals("hit", a.outcome)
        assert.same({ "z" }, a.payload.system_libs)
    end)

    it("miss emits install hint", function()
        local f = require("cook_cc.finders.zlib")
        local a = f.find({})
        assert.equals("miss", a.outcome)
        assert.matches("zlib1g%-dev", a.hint)
    end)

    it("version-undetectable + constraint → miss", function()
        stub.set_file_exists("/usr/lib/libz.so", true)
        local f = require("cook_cc.finders.zlib")
        local a = f.find({ version = ">=1.0" })
        -- No header, bare probe doesn't supply version.
        assert.equals("miss", a.outcome)
    end)
end)
```

- [ ] **Step 2: Run, verify fail**

```bash
busted spec/finders/zlib_spec.lua
```

Expected: FAIL — module not found.

- [ ] **Step 3: Implement zlib finder**

Create `cook_cc/finders/zlib.lua`:

```lua
local M = {}

local INSTALL_HINT = "apt: zlib1g-dev / macOS system / brew: zlib"

local function check_version(payload, constraint)
    if not constraint then return true end
    if not payload.version then return false end
    local ver = require("cook_cc.version")
    return ver.satisfies(payload.version, constraint)
end

function M.find(opts)
    opts = opts or {}
    -- 1. pkg-config
    local pkg = require("cook_cc.finders.pkg_config")
    local a = pkg.try("zlib")
    if a and check_version(a.payload, opts.version) then return a end
    if a and not check_version(a.payload, opts.version) then
        return { strategy = "curated:zlib", outcome = "miss",
                 reason = "pkg-config version " .. (a.payload.version or "(unknown)")
                          .. " does not satisfy " .. opts.version,
                 hint = INSTALL_HINT }
    end

    -- 2. bare probe `-lz`
    local bare = require("cook_cc.finders.bare_probe")
    local b = bare.try("z")
    if b then
        -- Attempt to recover version from header if present.
        local header = require("cook_cc.finders.header_probe")
        local v = header.parse_define("/usr/include/zlib.h", "ZLIB_VERSION")
        if v then b.payload.version = v end
        if check_version(b.payload, opts.version) then return b end
        return { strategy = "curated:zlib", outcome = "miss",
                 reason = "bare-probe version " .. (b.payload.version or "(undetectable)")
                          .. " does not satisfy " .. opts.version,
                 hint = INSTALL_HINT }
    end

    return { strategy = "curated:zlib", outcome = "miss",
             reason = "neither pkg-config nor bare probe located zlib",
             hint = INSTALL_HINT }
end

return M
```

- [ ] **Step 4: Run, verify pass**

```bash
busted spec/finders/zlib_spec.lua
```

Expected: 4 passes.

- [ ] **Step 5: Commit**

```bash
git -C ~/dev/cook-modules add cook_cc/finders/zlib.lua cook_cc/spec/finders/zlib_spec.lua
git -C ~/dev/cook-modules commit -m "feat(cook_cc): curated finder — zlib"
```

### Task B10: Curated finder — threads

**Files:**
- Create: `cook_cc/finders/threads.lua`
- Create: `cook_cc/spec/finders/threads_spec.lua`

- [ ] **Step 1: Write failing test**

Create `cook_cc/spec/finders/threads_spec.lua`:

```lua
local stub = require("cook_stub")

describe("finders.threads", function()
    before_each(function()
        stub.reset(); stub.install()
        package.loaded["cook_cc.finders.threads"] = nil
    end)

    it("Linux returns -pthread on both cflags and libs", function()
        local f = require("cook_cc.finders.threads")
        local a = f.find({})
        assert.equals("hit", a.outcome)
        assert.equals("-pthread", a.payload.cflags)
        assert.equals("-pthread", a.payload.libs)
    end)

    it("macOS returns found with empty fields", function()
        stub.set_platform_os("macos")
        local f = require("cook_cc.finders.threads")
        local a = f.find({})
        assert.equals("hit", a.outcome)
        assert.equals("", a.payload.cflags)
        assert.equals("", a.payload.libs)
    end)

    it("opts.version returns skip", function()
        local f = require("cook_cc.finders.threads")
        local a = f.find({ version = ">=1.0" })
        assert.equals("skip", a.outcome)
        assert.matches("no detectable version", a.reason)
    end)
end)
```

- [ ] **Step 2: Run, verify fail**

```bash
busted spec/finders/threads_spec.lua
```

Expected: FAIL — module not found.

- [ ] **Step 3: Implement threads finder**

Create `cook_cc/finders/threads.lua`:

```lua
local M = {}

local function blank_payload()
    return {
        cflags = "", libs = "", system_libs = {}, include_dirs = {}, lib_dirs = {},
        frameworks = {}, version = nil,
    }
end

function M.find(opts)
    opts = opts or {}
    if opts.version then
        return { strategy = "curated:threads", outcome = "skip",
                 reason = "threads has no detectable version; constraint cannot be honoured" }
    end
    local payload = blank_payload()
    if cook.platform.os ~= "macos" then
        payload.cflags = "-pthread"
        payload.libs   = "-pthread"
    end
    return { strategy = "curated:threads", outcome = "hit", reason = "", payload = payload }
end

return M
```

- [ ] **Step 4: Run, verify pass**

```bash
busted spec/finders/threads_spec.lua
```

Expected: 3 passes.

- [ ] **Step 5: Commit**

```bash
git -C ~/dev/cook-modules add cook_cc/finders/threads.lua cook_cc/spec/finders/threads_spec.lua
git -C ~/dev/cook-modules commit -m "feat(cook_cc): curated finder — threads (-pthread + macOS no-op)"
```

### Task B11: Curated finder — gl (with opengl alias)

**Files:**
- Create: `cook_cc/finders/gl.lua`
- Create: `cook_cc/spec/finders/gl_spec.lua`

- [ ] **Step 1: Write failing test**

Create `cook_cc/spec/finders/gl_spec.lua`:

```lua
local stub = require("cook_stub")

describe("finders.gl", function()
    before_each(function()
        stub.reset(); stub.install()
        package.loaded["cook_cc.finders.gl"] = nil
        package.loaded["cook_cc.finders.pkg_config"] = nil
        package.loaded["cook_cc.finders.bare_probe"] = nil
        stub.set_sh_handler("cc -print-search-dirs",
            function() return "libraries: =/usr/lib\n" end)
    end)

    it("Linux: pkg-config gl wins", function()
        stub.set_pkg_config_response("gl", {
            exists = true, cflags = "", libs = "-lGL", version = "1.0",
        })
        local f = require("cook_cc.finders.gl")
        local a = f.find({})
        assert.equals("hit", a.outcome)
        assert.same({ "GL" }, a.payload.system_libs)
    end)

    it("Linux: bare probe libGL.so fallback", function()
        stub.set_file_exists("/usr/lib/libGL.so", true)
        local f = require("cook_cc.finders.gl")
        local a = f.find({})
        assert.equals("hit", a.outcome)
    end)

    it("macOS returns frameworks={OpenGL}", function()
        stub.set_platform_os("macos")
        local f = require("cook_cc.finders.gl")
        local a = f.find({})
        assert.equals("hit", a.outcome)
        assert.same({ "OpenGL" }, a.payload.frameworks)
    end)

    it("opengl alias resolves via curated registry to gl", function()
        stub.set_pkg_config_response("gl", {
            exists = true, cflags = "", libs = "-lGL", version = "1.0",
        })
        package.loaded["cook_cc.finders"] = nil
        local curated = require("cook_cc.finders")
        local fn = curated.lookup("opengl")
        assert.is_function(fn)
        local a = fn({})
        assert.equals("curated:gl", a.strategy)
    end)
end)
```

- [ ] **Step 2: Run, verify fail**

```bash
busted spec/finders/gl_spec.lua
```

Expected: FAIL — module not found.

- [ ] **Step 3: Implement gl finder**

Create `cook_cc/finders/gl.lua`:

```lua
local M = {}

local INSTALL_HINT = "apt: libgl-dev / macOS system framework"

local function blank_payload()
    return {
        cflags = "", libs = "", system_libs = {}, include_dirs = {}, lib_dirs = {},
        frameworks = {}, version = nil,
    }
end

function M.find(opts)
    opts = opts or {}
    if cook.platform.os == "macos" then
        if opts.version then
            return { strategy = "curated:gl", outcome = "skip",
                     reason = "macOS OpenGL framework has no detectable version" }
        end
        local payload = blank_payload()
        payload.frameworks = { "OpenGL" }
        return { strategy = "curated:gl", outcome = "hit", reason = "", payload = payload }
    end

    local pkg = require("cook_cc.finders.pkg_config")
    local a = pkg.try("gl")
    if a then return a end

    local bare = require("cook_cc.finders.bare_probe")
    local b = bare.try("GL")
    if b then return b end

    return { strategy = "curated:gl", outcome = "miss",
             reason = "neither pkg-config 'gl' nor libGL on default linker paths",
             hint = INSTALL_HINT }
end

return M
```

- [ ] **Step 4: Run, verify pass**

```bash
busted spec/finders/gl_spec.lua
```

Expected: 4 passes.

- [ ] **Step 5: Commit**

```bash
git -C ~/dev/cook-modules add cook_cc/finders/gl.lua cook_cc/spec/finders/gl_spec.lua
git -C ~/dev/cook-modules commit -m "feat(cook_cc): curated finder — gl + opengl alias"
```

### Task B12: Curated finder — openal

**Files:**
- Create: `cook_cc/finders/openal.lua`
- Create: `cook_cc/spec/finders/openal_spec.lua`

- [ ] **Step 1: Write failing test**

Create `cook_cc/spec/finders/openal_spec.lua`:

```lua
local stub = require("cook_stub")

describe("finders.openal", function()
    before_each(function()
        stub.reset(); stub.install()
        package.loaded["cook_cc.finders.openal"] = nil
        package.loaded["cook_cc.finders.pkg_config"] = nil
        package.loaded["cook_cc.finders.bare_probe"] = nil
        stub.set_sh_handler("cc -print-search-dirs",
            function() return "libraries: =/usr/lib\n" end)
    end)

    it("Linux: pkg-config openal hit", function()
        stub.set_pkg_config_response("openal", {
            exists = true, cflags = "", libs = "-lopenal", version = "1.21",
        })
        local f = require("cook_cc.finders.openal")
        local a = f.find({})
        assert.equals("hit", a.outcome)
        assert.same({ "openal" }, a.payload.system_libs)
    end)

    it("Linux: bare probe libopenal.so fallback", function()
        stub.set_file_exists("/usr/lib/libopenal.so", true)
        local f = require("cook_cc.finders.openal")
        local a = f.find({})
        assert.equals("hit", a.outcome)
    end)

    it("macOS returns frameworks={OpenAL}", function()
        stub.set_platform_os("macos")
        local f = require("cook_cc.finders.openal")
        local a = f.find({})
        assert.equals("hit", a.outcome)
        assert.same({ "OpenAL" }, a.payload.frameworks)
    end)

    it("Linux miss carries install hint", function()
        local f = require("cook_cc.finders.openal")
        local a = f.find({})
        assert.equals("miss", a.outcome)
        assert.matches("libopenal%-dev", a.hint)
    end)
end)
```

- [ ] **Step 2: Run, verify fail**

```bash
busted spec/finders/openal_spec.lua
```

Expected: FAIL — module not found.

- [ ] **Step 3: Implement openal finder**

Create `cook_cc/finders/openal.lua`:

```lua
local M = {}

local INSTALL_HINT = "apt: libopenal-dev / macOS system framework / brew: openal-soft"

local function blank_payload()
    return {
        cflags = "", libs = "", system_libs = {}, include_dirs = {}, lib_dirs = {},
        frameworks = {}, version = nil,
    }
end

function M.find(opts)
    opts = opts or {}
    if cook.platform.os == "macos" then
        if opts.version then
            return { strategy = "curated:openal", outcome = "skip",
                     reason = "macOS OpenAL framework has no detectable version" }
        end
        local payload = blank_payload()
        payload.frameworks = { "OpenAL" }
        return { strategy = "curated:openal", outcome = "hit", reason = "", payload = payload }
    end

    local pkg = require("cook_cc.finders.pkg_config")
    local a = pkg.try("openal")
    if a then return a end

    local bare = require("cook_cc.finders.bare_probe")
    local b = bare.try("openal")
    if b then return b end

    return { strategy = "curated:openal", outcome = "miss",
             reason = "neither pkg-config 'openal' nor libopenal on default linker paths",
             hint = INSTALL_HINT }
end

return M
```

- [ ] **Step 4: Run, verify pass**

```bash
busted spec/finders/openal_spec.lua
```

Expected: 4 passes.

- [ ] **Step 5: Commit**

```bash
git -C ~/dev/cook-modules add cook_cc/finders/openal.lua cook_cc/spec/finders/openal_spec.lua
git -C ~/dev/cook-modules commit -m "feat(cook_cc): curated finder — openal (Linux pkg / macOS framework)"
```

### Task B13: Curated finder — libcurl

**Files:**
- Create: `cook_cc/finders/libcurl.lua`
- Create: `cook_cc/spec/finders/libcurl_spec.lua`

- [ ] **Step 1: Write failing test**

Create `cook_cc/spec/finders/libcurl_spec.lua`:

```lua
local stub = require("cook_stub")

describe("finders.libcurl", function()
    before_each(function()
        stub.reset(); stub.install()
        package.loaded["cook_cc.finders.libcurl"] = nil
        package.loaded["cook_cc.finders.pkg_config"] = nil
        stub.set_sh_handler("cc -print-search-dirs",
            function() return "libraries: =/usr/lib\n" end)
    end)

    it("uses curl-config when available", function()
        stub.set_tool_config_response("curl-config --cflags --libs",
            "-I/usr/include/curl -L/usr/lib -lcurl")
        stub.set_tool_config_response("curl-config --version", "libcurl 7.85.0")
        local f = require("cook_cc.finders.libcurl")
        local a = f.find({})
        assert.equals("hit", a.outcome)
        assert.same({ "curl" }, a.payload.system_libs)
        assert.equals("7.85.0", a.payload.version)
    end)

    it("falls back to pkg-config when curl-config absent", function()
        stub.set_pkg_config_response("libcurl", {
            exists = true, cflags = "", libs = "-lcurl", version = "7.85.0",
        })
        local f = require("cook_cc.finders.libcurl")
        local a = f.find({})
        assert.equals("hit", a.outcome)
    end)

    it("miss carries hint", function()
        local f = require("cook_cc.finders.libcurl")
        local a = f.find({})
        assert.equals("miss", a.outcome)
        assert.matches("libcurl4%-openssl%-dev", a.hint)
    end)
end)
```

- [ ] **Step 2: Run, verify fail**

```bash
busted spec/finders/libcurl_spec.lua
```

Expected: FAIL.

- [ ] **Step 3: Implement libcurl finder**

Create `cook_cc/finders/libcurl.lua`:

```lua
local M = {}

local INSTALL_HINT = "apt: libcurl4-openssl-dev / macOS system / brew: curl"

local function parse_tool_output(out)
    local pkg = require("cook_cc.finders.pkg_config")
    -- Reuse pkg_config's token-bucketing by feeding the raw output as libs.
    local fake_attempt = pkg.try("__no_such_pkg__")  -- forces nil
    -- Fallback: hand-parse here using the same lpeg setup (kept private to libcurl).
    local lpeg = require("lpeg")
    local P, S, C, Ct = lpeg.P, lpeg.S, lpeg.C, lpeg.Ct
    local space     = S(" \t")^0
    local non_space = (P(1) - S(" \t"))^1
    local include   = P("-I") * C(non_space) / function(v) return { kind = "I", value = v } end
    local libdir    = P("-L") * C(non_space) / function(v) return { kind = "L", value = v } end
    local syslib    = P("-l") * C(non_space) / function(v) return { kind = "l", value = v } end
    local other     = C(non_space)             / function(v) return { kind = "o", value = v } end
    local token     = include + libdir + syslib + other
    local line      = Ct((space * token)^0 * space)
    local toks      = line:match(out or "") or {}
    local payload = {
        cflags = "", libs = out or "", system_libs = {}, include_dirs = {}, lib_dirs = {},
        frameworks = {}, version = nil,
    }
    for _, t in ipairs(toks) do
        if     t.kind == "I" then payload.include_dirs[#payload.include_dirs + 1] = t.value
        elseif t.kind == "L" then payload.lib_dirs[#payload.lib_dirs + 1] = t.value
        elseif t.kind == "l" then payload.system_libs[#payload.system_libs + 1] = t.value
        end
    end
    return payload
end

function M.find(opts)
    opts = opts or {}
    local tool = require("cook_cc.finders.tool_config")
    local out = tool.try("curl-config", "--cflags --libs")
    if out then
        local payload = parse_tool_output(out)
        local ver_out = tool.try("curl-config", "--version")
        if ver_out then
            payload.version = ver_out:match("libcurl%s+([%d%.]+)") or ver_out
        end
        if opts.version then
            local ver = require("cook_cc.version")
            if not ver.satisfies(payload.version or "", opts.version) then
                return { strategy = "curated:libcurl", outcome = "miss",
                         reason = "curl-config version " .. (payload.version or "(undetectable)")
                                  .. " does not satisfy " .. opts.version,
                         hint = INSTALL_HINT }
            end
        end
        return { strategy = "curated:libcurl", outcome = "hit", reason = "", payload = payload }
    end

    local pkg = require("cook_cc.finders.pkg_config")
    local a = pkg.try("libcurl")
    if a then
        if opts.version then
            local ver = require("cook_cc.version")
            if not (a.payload.version and ver.satisfies(a.payload.version, opts.version)) then
                return { strategy = "curated:libcurl", outcome = "miss",
                         reason = "pkg-config version " .. (a.payload.version or "(undetectable)")
                                  .. " does not satisfy " .. opts.version,
                         hint = INSTALL_HINT }
            end
        end
        return a
    end

    return { strategy = "curated:libcurl", outcome = "miss",
             reason = "neither curl-config nor pkg-config 'libcurl' located libcurl",
             hint = INSTALL_HINT }
end

return M
```

- [ ] **Step 4: Run, verify pass**

```bash
busted spec/finders/libcurl_spec.lua
```

Expected: 3 passes.

- [ ] **Step 5: Commit**

```bash
git -C ~/dev/cook-modules add cook_cc/finders/libcurl.lua cook_cc/spec/finders/libcurl_spec.lua
git -C ~/dev/cook-modules commit -m "feat(cook_cc): curated finder — libcurl (curl-config + pkg-config)"
```

### Task B14: Curated finder — sdl2

**Files:**
- Create: `cook_cc/finders/sdl2.lua`
- Create: `cook_cc/spec/finders/sdl2_spec.lua`

- [ ] **Step 1: Write failing test**

Create `cook_cc/spec/finders/sdl2_spec.lua`:

```lua
local stub = require("cook_stub")

describe("finders.sdl2", function()
    before_each(function()
        stub.reset(); stub.install()
        package.loaded["cook_cc.finders.sdl2"] = nil
        package.loaded["cook_cc.finders.pkg_config"] = nil
    end)

    it("sdl2-config preferred", function()
        stub.set_tool_config_response("sdl2-config --cflags --libs",
            "-I/usr/include/SDL2 -L/usr/lib -lSDL2")
        stub.set_tool_config_response("sdl2-config --version", "2.30.1")
        local f = require("cook_cc.finders.sdl2")
        local a = f.find({})
        assert.equals("hit", a.outcome)
        assert.same({ "SDL2" }, a.payload.system_libs)
        assert.equals("2.30.1", a.payload.version)
    end)

    it("falls back to pkg-config sdl2", function()
        stub.set_pkg_config_response("sdl2", {
            exists = true, cflags = "", libs = "-lSDL2", version = "2.30.1",
        })
        local f = require("cook_cc.finders.sdl2")
        local a = f.find({})
        assert.equals("hit", a.outcome)
    end)

    it("miss carries hint", function()
        local f = require("cook_cc.finders.sdl2")
        local a = f.find({})
        assert.equals("miss", a.outcome)
        assert.matches("libsdl2%-dev", a.hint)
    end)
end)
```

- [ ] **Step 2: Run, verify fail**

```bash
busted spec/finders/sdl2_spec.lua
```

Expected: FAIL.

- [ ] **Step 3: Implement sdl2 finder**

Create `cook_cc/finders/sdl2.lua`:

```lua
local M = {}

local INSTALL_HINT = "apt: libsdl2-dev / brew: sdl2"

-- Reuse libcurl's tool-output parser shape (duplicated rather than coupled).
local function parse_tool_output(out)
    local lpeg = require("lpeg")
    local P, S, C, Ct = lpeg.P, lpeg.S, lpeg.C, lpeg.Ct
    local space     = S(" \t")^0
    local non_space = (P(1) - S(" \t"))^1
    local framework = P("-framework") * S(" \t")^1 * C(non_space)
                      / function(v) return { kind = "F", value = v } end
    local include   = P("-I") * C(non_space) / function(v) return { kind = "I", value = v } end
    local libdir    = P("-L") * C(non_space) / function(v) return { kind = "L", value = v } end
    local syslib    = P("-l") * C(non_space) / function(v) return { kind = "l", value = v } end
    local other     = C(non_space)             / function(v) return { kind = "o", value = v } end
    local token     = framework + include + libdir + syslib + other
    local line      = Ct((space * token)^0 * space)
    local toks      = line:match(out or "") or {}
    local payload = {
        cflags = "", libs = out or "", system_libs = {}, include_dirs = {}, lib_dirs = {},
        frameworks = {}, version = nil,
    }
    for _, t in ipairs(toks) do
        if     t.kind == "I" then payload.include_dirs[#payload.include_dirs + 1] = t.value
        elseif t.kind == "L" then payload.lib_dirs[#payload.lib_dirs + 1] = t.value
        elseif t.kind == "l" then payload.system_libs[#payload.system_libs + 1] = t.value
        elseif t.kind == "F" then payload.frameworks[#payload.frameworks + 1] = t.value
        end
    end
    return payload
end

function M.find(opts)
    opts = opts or {}
    local tool = require("cook_cc.finders.tool_config")
    local out = tool.try("sdl2-config", "--cflags --libs")
    if out then
        local payload = parse_tool_output(out)
        payload.version = tool.try("sdl2-config", "--version")
        if opts.version then
            local ver = require("cook_cc.version")
            if not (payload.version and ver.satisfies(payload.version, opts.version)) then
                return { strategy = "curated:sdl2", outcome = "miss",
                         reason = "sdl2-config version " .. (payload.version or "(undetectable)")
                                  .. " does not satisfy " .. opts.version,
                         hint = INSTALL_HINT }
            end
        end
        return { strategy = "curated:sdl2", outcome = "hit", reason = "", payload = payload }
    end

    local pkg = require("cook_cc.finders.pkg_config")
    local a = pkg.try("sdl2")
    if a then
        if opts.version then
            local ver = require("cook_cc.version")
            if not (a.payload.version and ver.satisfies(a.payload.version, opts.version)) then
                return { strategy = "curated:sdl2", outcome = "miss",
                         reason = "pkg-config version " .. (a.payload.version or "(undetectable)")
                                  .. " does not satisfy " .. opts.version,
                         hint = INSTALL_HINT }
            end
        end
        return a
    end

    return { strategy = "curated:sdl2", outcome = "miss",
             reason = "neither sdl2-config nor pkg-config 'sdl2' located SDL2",
             hint = INSTALL_HINT }
end

return M
```

- [ ] **Step 4: Run, verify pass**

```bash
busted spec/finders/sdl2_spec.lua
```

Expected: 3 passes.

- [ ] **Step 5: Commit**

```bash
git -C ~/dev/cook-modules add cook_cc/finders/sdl2.lua cook_cc/spec/finders/sdl2_spec.lua
git -C ~/dev/cook-modules commit -m "feat(cook_cc): curated finder — sdl2 (sdl2-config + pkg-config)"
```

### Task B15: Curated finder — raylib

**Files:**
- Create: `cook_cc/finders/raylib.lua`
- Create: `cook_cc/spec/finders/raylib_spec.lua`

- [ ] **Step 1: Write failing test**

Create `cook_cc/spec/finders/raylib_spec.lua`:

```lua
local stub = require("cook_stub")

describe("finders.raylib", function()
    before_each(function()
        stub.reset(); stub.install()
        package.loaded["cook_cc.finders.raylib"] = nil
        package.loaded["cook_cc.finders.pkg_config"] = nil
        package.loaded["cook_cc.finders.bare_probe"] = nil
        stub.set_sh_handler("cc -print-search-dirs",
            function() return "libraries: =/usr/lib\n" end)
    end)

    it("Linux: pkg-config raylib hit", function()
        stub.set_pkg_config_response("raylib", {
            exists = true, cflags = "-I/usr/include",
            libs = "-lraylib -lm -ldl -lpthread", version = "4.5.0",
        })
        local f = require("cook_cc.finders.raylib")
        local a = f.find({})
        assert.equals("hit", a.outcome)
        assert.equals("4.5.0", a.payload.version)
    end)

    it("macOS: post-processes frameworks when missing", function()
        stub.set_platform_os("macos")
        stub.set_pkg_config_response("raylib", {
            exists = true, cflags = "", libs = "-lraylib", version = "4.5.0",
        })
        local f = require("cook_cc.finders.raylib")
        local a = f.find({})
        assert.equals("hit", a.outcome)
        local has_opengl = false
        for _, fw in ipairs(a.payload.frameworks) do
            if fw == "OpenGL" then has_opengl = true end
        end
        assert.is_true(has_opengl)
    end)

    it("version constraint enforced", function()
        stub.set_pkg_config_response("raylib", {
            exists = true, cflags = "", libs = "-lraylib", version = "3.0.0",
        })
        local f = require("cook_cc.finders.raylib")
        local a = f.find({ version = ">=4.0" })
        assert.equals("miss", a.outcome)
        assert.matches("3%.0%.0", a.reason)
    end)

    it("miss carries hint", function()
        local f = require("cook_cc.finders.raylib")
        local a = f.find({})
        assert.equals("miss", a.outcome)
        assert.matches("libraylib%-dev", a.hint)
    end)
end)
```

- [ ] **Step 2: Run, verify fail**

```bash
busted spec/finders/raylib_spec.lua
```

Expected: FAIL.

- [ ] **Step 3: Implement raylib finder**

Create `cook_cc/finders/raylib.lua`:

```lua
local M = {}

local INSTALL_HINT = "apt: libraylib-dev / brew: raylib"

local MAC_FRAMEWORKS = { "OpenGL", "Cocoa", "IOKit", "CoreVideo", "CoreAudio" }

local function ensure_mac_frameworks(payload)
    if cook.platform.os ~= "macos" then return end
    local present = {}
    for _, fw in ipairs(payload.frameworks) do present[fw] = true end
    for _, fw in ipairs(MAC_FRAMEWORKS) do
        if not present[fw] then payload.frameworks[#payload.frameworks + 1] = fw end
    end
end

function M.find(opts)
    opts = opts or {}
    local pkg = require("cook_cc.finders.pkg_config")
    local a = pkg.try("raylib")
    if a then
        ensure_mac_frameworks(a.payload)
        if opts.version then
            local ver = require("cook_cc.version")
            if not (a.payload.version and ver.satisfies(a.payload.version, opts.version)) then
                return { strategy = "curated:raylib", outcome = "miss",
                         reason = "pkg-config version " .. (a.payload.version or "(undetectable)")
                                  .. " does not satisfy " .. opts.version,
                         hint = INSTALL_HINT }
            end
        end
        return a
    end

    local bare = require("cook_cc.finders.bare_probe")
    local b = bare.try("raylib")
    if b then
        ensure_mac_frameworks(b.payload)
        local header = require("cook_cc.finders.header_probe")
        local v = header.parse_define("/usr/include/raylib.h", "RAYLIB_VERSION")
        if v then b.payload.version = v end
        if opts.version then
            local ver = require("cook_cc.version")
            if not (b.payload.version and ver.satisfies(b.payload.version, opts.version)) then
                return { strategy = "curated:raylib", outcome = "miss",
                         reason = "bare-probe version " .. (b.payload.version or "(undetectable)")
                                  .. " does not satisfy " .. opts.version,
                         hint = INSTALL_HINT }
            end
        end
        return b
    end

    return { strategy = "curated:raylib", outcome = "miss",
             reason = "neither pkg-config 'raylib' nor libraylib on default linker paths",
             hint = INSTALL_HINT }
end

return M
```

- [ ] **Step 4: Run, verify pass**

```bash
busted spec/finders/raylib_spec.lua
```

Expected: 4 passes.

- [ ] **Step 5: Commit**

```bash
git -C ~/dev/cook-modules add cook_cc/finders/raylib.lua cook_cc/spec/finders/raylib_spec.lua
git -C ~/dev/cook-modules commit -m "feat(cook_cc): curated finder — raylib (pkg-config + macOS framework injection)"
```

### Task B16: cc.lua — frameworks emission in link

**Files:**
- Modify: `cook_cc/cc.lua:71-92`
- Modify: `cook_cc/spec/cc_spec.lua` (add frameworks emission tests)

- [ ] **Step 1: Add failing test**

Append to `cook_cc/spec/cc_spec.lua`. Reuse the file's existing `with_toolchain()` helper (defined at the top of `cc_spec.lua`) which mocks `command -v g++` and runs `toolchain.rehydrate()` — this is the established toolchain setup pattern; do not invent a cache-key shortcut:

```lua
describe("cc.link frameworks", function()
    before_each(function()
        stub.reset(); stub.install()
        package.loaded["cook_cc.cc"] = nil
        package.loaded["cook_cc.toolchain"] = nil
        with_toolchain()
    end)

    it("emits -framework <name> on macOS", function()
        stub.set_platform_os("macos")
        local cc = require("cook_cc.cc")
        cc.link({ "a.o" }, "build/bin/x", { frameworks = { "OpenGL", "Cocoa" } })
        local cmd = stub.added_units()[1].command
        assert.matches("%-framework OpenGL", cmd)
        assert.matches("%-framework Cocoa", cmd)
    end)

    it("ignores frameworks on Linux", function()
        local cc = require("cook_cc.cc")
        cc.link({ "a.o" }, "build/bin/x", { frameworks = { "OpenGL" } })
        local cmd = stub.added_units()[1].command
        assert.is_nil(cmd:find("%-framework"))
    end)
end)
```

- [ ] **Step 2: Run, verify fail**

```bash
busted spec/cc_spec.lua
```

Expected: FAIL — no `-framework` token emitted.

- [ ] **Step 3: Modify `cc.lua:71-92`**

Replace the `M.link` body (current lines 71-92) with:

```lua
function M.link(objects, output, opts)
    opts = opts or {}
    fs.mkdir_p(path.dir(output))
    local cxx = toolchain.get_compiler().cxx
    local parts = { cxx, table.concat(objects, " "), "-o", output }
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
    -- Trailing space ensures assertions like " -lpthread " match the last token.
    local cmd = table.concat(parts, " ") .. " "

    cook.add_unit({
        inputs = objects,
        output = output,
        command = cmd,
    })
    return output
end
```

- [ ] **Step 4: Run, verify pass**

```bash
busted spec/cc_spec.lua
```

Expected: previous tests + 2 new tests pass.

- [ ] **Step 5: Commit**

```bash
git -C ~/dev/cook-modules add cook_cc/cc.lua cook_cc/spec/cc_spec.lua
git -C ~/dev/cook-modules commit -m "feat(cook_cc): cc.link emits -framework on macOS, no-op elsewhere"
```

### Task B17: targets.lua — frameworks pass-through

**Files:**
- Modify: `cook_cc/targets.lua`
- Modify: `cook_cc/spec/targets_spec.lua`

- [ ] **Step 1: Add failing test**

Append to `cook_cc/spec/targets_spec.lua`. Use the file's existing toolchain-setup helper (mirror the existing `targets_spec.lua` `before_each` pattern — if the file uses a local `with_toolchain()` helper, reuse it; otherwise copy the helper from `cc_spec.lua` into a shared form):

```lua
describe("targets frameworks", function()
    before_each(function()
        stub.reset(); stub.install()
        package.loaded["cook_cc.targets"] = nil
        package.loaded["cook_cc.cc"] = nil
        package.loaded["cook_cc.toolchain"] = nil
        stub.set_sh_handler("command -v g++", function() return "/usr/bin/g++\n" end)
        local toolchain = require("cook_cc.toolchain")
        toolchain.rehydrate()
        stub.set_platform_os("macos")
    end)

    it("cc.bin passes frameworks through to link command", function()
        local t = require("cook_cc.targets")
        t.bin("app", { sources = { "src/main.c" }, frameworks = { "OpenGL" } })
        local link_unit = stub.added_units()[#stub.added_units()]
        assert.matches("%-framework OpenGL", link_unit.command)
    end)

    it("cc.lib exports frameworks via cook.export", function()
        local t = require("cook_cc.targets")
        t.lib("gfx", { sources = { "src/lib.c" }, frameworks = { "OpenGL" } })
        local info = cook.import("gfx")
        assert.same({ "OpenGL" }, info.frameworks)
    end)
end)
```

- [ ] **Step 2: Run, verify fail**

```bash
busted spec/targets_spec.lua
```

Expected: FAIL — frameworks not threaded through `build_opts` / `record_export` / link call.

- [ ] **Step 3: Thread `frameworks` through `build_opts`**

Modify `cook_cc/targets.lua:26-50` — add `merged_frameworks` and `frameworks` to `build_opts`'s return:

```lua
local function build_opts(opts, kind)
    opts = opts or {}
    local d = toolchain.get_defaults()
    local merged_includes = {}
    local merged_defines  = {}
    local merged_libs     = {}
    local merged_fw       = {}
    for _, v in ipairs(d.includes    or {}) do merged_includes[#merged_includes + 1] = v end
    for _, v in ipairs(opts.includes or {}) do merged_includes[#merged_includes + 1] = v end
    for _, v in ipairs(d.defines     or {}) do merged_defines [#merged_defines  + 1] = v end
    for _, v in ipairs(opts.defines  or {}) do merged_defines [#merged_defines  + 1] = v end
    for _, v in ipairs(d.system_libs    or {}) do merged_libs[#merged_libs + 1] = v end
    for _, v in ipairs(opts.system_libs or {}) do merged_libs[#merged_libs + 1] = v end
    for _, v in ipairs(d.frameworks    or {}) do merged_fw[#merged_fw + 1] = v end
    for _, v in ipairs(opts.frameworks or {}) do merged_fw[#merged_fw + 1] = v end
    return {
        includes      = merged_includes,
        defines       = merged_defines,
        system_libs   = merged_libs,
        frameworks    = merged_fw,
        standard      = opts.standard,
        warnings      = opts.warnings,
        extra_cflags  = opts.extra_cflags,
        extra_ldflags = opts.extra_ldflags,
        export_includes = opts.export_includes,
        links         = opts.links or {},
        fpic          = (kind == "shared"),
    }
end
```

- [ ] **Step 4: Add frameworks to `record_export`**

Modify `cook_cc/targets.lua:52-68` — add `frameworks = b.frameworks` to the `cook.export` info table:

```lua
local function record_export(name, sources, b, lib_path)
    cook.export(name, {
        includes      = b.export_includes or b.includes,
        defines       = b.defines,
        system_libs   = b.system_libs,
        frameworks    = b.frameworks,
        extra_ldflags = b.extra_ldflags or "",
        links         = b.links,
        lib_path      = lib_path or "",
        compile_info  = {
            sources  = sources,
            includes = b.includes,
            defines  = b.defines,
            standard = b.standard,
            compiler = toolchain.get_compiler() and toolchain.get_compiler().cxx,
        },
    })
end
```

- [ ] **Step 5: Add a frameworks merger + pass to link**

Add a helper near the existing `merge_system_libs`:

```lua
-- Merge frameworks: transitive first, then local (dedup, first occurrence wins).
local function merge_frameworks(merged_transitive, local_fw)
    local seen = {}
    local result = {}
    for _, v in ipairs(merged_transitive or {}) do
        if not seen[v] then seen[v] = true; result[#result + 1] = v end
    end
    for _, v in ipairs(local_fw or {}) do
        if not seen[v] then seen[v] = true; result[#result + 1] = v end
    end
    return result
end
```

Modify the `cc.link` call inside `M.bin` (and the equivalent in `M.shared`) — add `frameworks = merge_frameworks(merged.frameworks, b.frameworks)`:

```lua
function M.bin(name, opts)
    local b = build_opts(opts, "bin")
    local sources = gather_sources(opts or {})
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
    })
    return name
end
```

And `M.shared`:

```lua
function M.shared(name, opts)
    local b = build_opts(opts, "shared")
    local sources = gather_sources(opts or {})
    if #sources == 0 then
        error("[cc.shared] no sources found for target '" .. name .. "'", 2)
    end
    local so_path = "build/lib/lib" .. name .. ".so"
    register_known(name)
    local merged = transitive.resolve_links(b.links)
    b.includes = merge_includes(b.includes, merged.includes)
    record_export(name, sources, b, so_path)
    local objs = compile_all(name, sources, b)
    cc.link(objs, so_path, {
        system_libs   = merge_system_libs(merged.system_libs, b.system_libs),
        frameworks    = merge_frameworks(merged.frameworks, b.frameworks),
        extra_ldflags = build_ldflags(merged.lib_paths, merged.extra_ldflags, b.extra_ldflags),
        shared        = true,
    })
    return name
end
```

- [ ] **Step 6: Run, verify pass**

```bash
busted spec/targets_spec.lua
```

Expected: 2 new tests pass; pre-existing tests pass.

- [ ] **Step 7: Commit**

```bash
git -C ~/dev/cook-modules add cook_cc/targets.lua cook_cc/spec/targets_spec.lua
git -C ~/dev/cook-modules commit -m "feat(cook_cc): targets.bin/lib/shared accept + propagate frameworks"
```

### Task B18: transitive.lua — propagate frameworks

**Files:**
- Modify: `cook_cc/transitive.lua`
- Modify: `cook_cc/spec/transitive_spec.lua`

- [ ] **Step 1: Add failing test**

Append to `cook_cc/spec/transitive_spec.lua`:

```lua
    it("propagates frameworks from a linked target", function()
        cook.export("gfx", {
            includes = {}, system_libs = {}, frameworks = { "OpenGL" },
            extra_ldflags = "", links = {}, lib_path = "",
        })
        local t = require("cook_cc.transitive")
        local merged = t.resolve_links({ "gfx" })
        assert.same({ "OpenGL" }, merged.frameworks)
    end)
```

- [ ] **Step 2: Run, verify fail**

```bash
busted spec/transitive_spec.lua
```

Expected: FAIL — `merged.frameworks` is nil.

- [ ] **Step 3: Modify `cook_cc/transitive.lua`**

Replace `M.resolve_links` body:

```lua
function M.resolve_links(links)
    local merged = {
        includes      = {},
        defines       = {},
        system_libs   = {},
        frameworks    = {},
        lib_paths     = {},
        extra_ldflags = "",
    }
    local seen_inc, seen_def, seen_lib, seen_fw, seen_path = {}, {}, {}, {}, {}
    local visited = {}

    local function walk(name)
        if visited[name] then return end
        visited[name] = true
        local info = cook.import(name)
        if not info then return end
        add_unique(merged.includes,    info.includes,    seen_inc)
        add_unique(merged.defines,     info.defines,     seen_def)
        add_unique(merged.system_libs, info.system_libs, seen_lib)
        add_unique(merged.frameworks,  info.frameworks,  seen_fw)
        if info.lib_path and info.lib_path ~= "" then
            add_unique(merged.lib_paths, { info.lib_path }, seen_path)
        end
        if info.extra_ldflags and info.extra_ldflags ~= "" then
            if merged.extra_ldflags ~= "" then
                merged.extra_ldflags = merged.extra_ldflags .. " " .. info.extra_ldflags
            else
                merged.extra_ldflags = info.extra_ldflags
            end
        end
        for _, child in ipairs(info.links or {}) do walk(child) end
    end

    for _, name in ipairs(links or {}) do walk(name) end
    return merged
end
```

- [ ] **Step 4: Run, verify pass**

```bash
busted spec/transitive_spec.lua
```

Expected: new test plus existing tests pass.

- [ ] **Step 5: Commit**

```bash
git -C ~/dev/cook-modules add cook_cc/transitive.lua cook_cc/spec/transitive_spec.lua
git -C ~/dev/cook-modules commit -m "feat(cook_cc): transitive.resolve_links propagates frameworks"
```

### Task B19: init.lua — register_finder + find_or_error

**Files:**
- Modify: `cook_cc/init.lua`
- Modify: `cook_cc/spec/finder_spec.lua` (reduce to integration tests + add or_error)

- [ ] **Step 1: Add failing tests for find_or_error**

Replace `cook_cc/spec/finder_spec.lua` body (the existing M0 tests get reframed as integration tests with the new contract):

```lua
local stub = require("cook_stub")

describe("cc.find integration", function()
    before_each(function()
        stub.reset(); stub.install()
        package.loaded["cook_cc"] = nil
        package.loaded["cook_cc.finder"] = nil
        package.loaded["cook_cc.finders"] = nil
        package.loaded["cook_cc.finders.pkg_config"] = nil
        package.loaded["cook_cc.finders.bare_probe"] = nil
        stub.set_sh_handler("cc -print-search-dirs",
            function() return "libraries: =/usr/lib\n" end)
    end)

    it("M0 v0.1 contract: pkg-config hit populates legacy fields", function()
        stub.set_pkg_config_response("foo", {
            exists = true, cflags = "-I/usr/include/foo -DFOO=1",
            libs = "-L/usr/lib -lfoo -lpthread", version = "1.0",
        })
        local cc = require("cook_cc")
        local r = cc.find("foo")
        assert.is_true(r.found)
        assert.equals("-I/usr/include/foo -DFOO=1", r.cflags)
        assert.same({ "foo", "pthread" }, r.system_libs)
        assert.is_table(r.tried)
    end)

    it("M0 v0.1 contract: miss returns blank result with tried list", function()
        local cc = require("cook_cc")
        local r = cc.find("definitely_no_such_package_xyz_42")
        assert.is_false(r.found)
        assert.same({}, r.system_libs)
        assert.is_table(r.tried)
    end)
end)

describe("cc.find_or_error", function()
    before_each(function()
        stub.reset(); stub.install()
        package.loaded["cook_cc"] = nil
        package.loaded["cook_cc.finder"] = nil
        package.loaded["cook_cc.finders"] = nil
        package.loaded["cook_cc.finders.pkg_config"] = nil
        package.loaded["cook_cc.finders.bare_probe"] = nil
        stub.set_sh_handler("cc -print-search-dirs",
            function() return "libraries: =/usr/lib\n" end)
    end)

    it("returns the result on hit", function()
        stub.set_pkg_config_response("zlib", {
            exists = true, cflags = "", libs = "-lz", version = "1.2.13",
        })
        local cc = require("cook_cc")
        local r = cc.find_or_error("zlib")
        assert.is_true(r.found)
    end)

    it("raises on miss with formatted tried list", function()
        local cc = require("cook_cc")
        assert.has_error(function() cc.find_or_error("nonesuch") end,
            function(err)
                return type(err) == "string"
                       and err:find("%[cc.find_or_error%]")
                       and err:find("nonesuch")
            end)
    end)
end)

describe("cc.register_finder", function()
    before_each(function()
        stub.reset(); stub.install()
        package.loaded["cook_cc"] = nil
        package.loaded["cook_cc.finder"] = nil
    end)

    it("raises when finder is not a function", function()
        local cc = require("cook_cc")
        assert.has_error(function() cc.register_finder("bad", "not a fn") end)
    end)
end)
```

- [ ] **Step 2: Run, verify fail**

```bash
busted spec/finder_spec.lua
```

Expected: FAIL — `cc.find_or_error` does not exist; `cc.register_finder` does not exist on the top-level table.

- [ ] **Step 3: Modify `cook_cc/init.lua`**

Replace contents:

```lua
local toolchain = require("cook_cc.toolchain")
local cc        = require("cook_cc.cc")
local targets   = require("cook_cc.targets")
local finder    = require("cook_cc.finder")
local db        = require("cook_cc.compile_db")

local M = {}

function M.init()
    toolchain.rehydrate()
end

-- Public surface (Standard §9.2 contract).
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
M.register_finder  = finder.register
M.compile_commands = db.write

-- §9.2.3.13 — only function in §9.2 that raises on miss.
function M.find_or_error(name, opts)
    local r = M.find(name, opts)
    if r.found then return r end
    local lines = { "could not locate '" .. name .. "'" }
    if opts and opts.version then
        lines[1] = lines[1] .. " (version " .. opts.version .. ")"
    end
    lines[1] = lines[1] .. ":"
    for _, a in ipairs(r.tried or {}) do
        local line = "  - " .. a.strategy .. ": " .. a.outcome
        if a.reason and a.reason ~= "" then line = line .. " (" .. a.reason .. ")" end
        lines[#lines + 1] = line
        if a.hint then lines[#lines + 1] = "    hint: " .. a.hint end
    end
    error("[cc.find_or_error] " .. table.concat(lines, "\n"), 2)
end

return M
```

- [ ] **Step 4: Run, verify pass**

```bash
busted spec/finder_spec.lua
```

Expected: all new tests pass.

- [ ] **Step 5: Run the full suite — verify nothing else broke**

```bash
busted
```

Expected: all specs green; total count around 120+ tests (41 existing + ~80 added).

- [ ] **Step 6: Commit**

```bash
git -C ~/dev/cook-modules add cook_cc/init.lua cook_cc/spec/finder_spec.lua
git -C ~/dev/cook-modules commit -m "feat(cook_cc): expose register_finder + find_or_error on cook_cc surface"
```

### Task B20: Bump rockspec to 0.2.0-1

**Files:**
- Create: `cook_cc/cook_cc-0.2.0-1.rockspec`

- [ ] **Step 1: Write the new rockspec**

Create `cook_cc/cook_cc-0.2.0-1.rockspec`:

```lua
package = "cook_cc"
version = "0.2.0-1"
source = {
   url = "git+https://github.com/lioralabs/cook-modules.git",
   tag = "cook_cc-0.2.0-1",
}
description = {
   summary  = "Cook C-family (C + C++) native build module",
   detailed = [[
      Blessed Cook module for C and C++ native builds. Provides declarative
      target makers (cc.bin/lib/shared/headers), low-level primitives
      (cc.compile/archive/link), multi-strategy package discovery
      (cc.find with project / curated / pkg-config / bare-probe stages),
      project-scoped finder registration (cc.register_finder), a raising
      find convenience (cc.find_or_error), transitive link propagation
      including macOS frameworks, and compile_commands.json generation.
      Specified normatively at §9.2 of the Cook Standard (v0.2).
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
     ["cook_cc"]                       = "cook_cc/init.lua",
     ["cook_cc.toolchain"]             = "cook_cc/toolchain.lua",
     ["cook_cc.cc"]                    = "cook_cc/cc.lua",
     ["cook_cc.targets"]               = "cook_cc/targets.lua",
     ["cook_cc.finder"]                = "cook_cc/finder.lua",
     ["cook_cc.compile_db"]            = "cook_cc/compile_db.lua",
     ["cook_cc.transitive"]            = "cook_cc/transitive.lua",
     ["cook_cc.version"]               = "cook_cc/version.lua",
     ["cook_cc.finders"]               = "cook_cc/finders/init.lua",
     ["cook_cc.finders.pkg_config"]    = "cook_cc/finders/pkg_config.lua",
     ["cook_cc.finders.bare_probe"]    = "cook_cc/finders/bare_probe.lua",
     ["cook_cc.finders.header_probe"]  = "cook_cc/finders/header_probe.lua",
     ["cook_cc.finders.tool_config"]   = "cook_cc/finders/tool_config.lua",
     ["cook_cc.finders.raylib"]        = "cook_cc/finders/raylib.lua",
     ["cook_cc.finders.sdl2"]          = "cook_cc/finders/sdl2.lua",
     ["cook_cc.finders.openal"]        = "cook_cc/finders/openal.lua",
     ["cook_cc.finders.gl"]            = "cook_cc/finders/gl.lua",
     ["cook_cc.finders.threads"]       = "cook_cc/finders/threads.lua",
     ["cook_cc.finders.zlib"]          = "cook_cc/finders/zlib.lua",
     ["cook_cc.finders.libcurl"]       = "cook_cc/finders/libcurl.lua",
   },
}
```

- [ ] **Step 2: Lint the rockspec**

```bash
cd ~/dev/cook-modules/cook_cc && luarocks lint cook_cc-0.2.0-1.rockspec
```

Expected: `cook_cc-0.2.0-1.rockspec is OK`.

- [ ] **Step 3: Commit**

```bash
git -C ~/dev/cook-modules add cook_cc/cook_cc-0.2.0-1.rockspec
git -C ~/dev/cook-modules commit -m "chore(cook_cc): rockspec 0.2.0-1 — M1 surface (SHI-134)"
```

### Task B21: Push branch and open PR cook-modules#B

- [ ] **Step 1: Push the branch**

```bash
git -C ~/dev/cook-modules push -u origin shi-134-m1-cc-finders
```

- [ ] **Step 2: Open PR**

Use the project's PR-creation flow. PR title: `SHI-134: M1 — cc finder multi-strategy chain (cook_cc 0.2.0)`. Body mirrors the design spec scope. Mark ready-for-review (not draft) — this PR is independent of cook#A and can be reviewed in isolation.

---

## Stage C — Release coordination

### Task C1: Tag cook_cc-0.2.0-1 and verify rocks.usecook.com

- [ ] **Step 1: After cook-modules#B merges, tag the release**

```bash
git -C ~/dev/cook-modules checkout main
git -C ~/dev/cook-modules pull
git -C ~/dev/cook-modules tag cook_cc-0.2.0-1
git -C ~/dev/cook-modules push origin cook_cc-0.2.0-1
```

- [ ] **Step 2: Trigger / verify the rocks-index render**

Follow the `cook-rocks-index` repo's publish workflow (per the M0 closeout, this involves a PR or auto-render on push). Wait for Cloudflare Pages to propagate (typically minutes).

- [ ] **Step 3: Verify the rock is fetchable**

```bash
cd /tmp && luarocks --server=https://rocks.usecook.com search cook_cc
```

Expected: `cook_cc 0.2.0-1` appears in the listing.

Also verify a fresh install works:

```bash
cd /tmp/empty-test-dir && cat > cook.toml <<'EOF'
[registry]
indexes = ["https://rocks.usecook.com"]

[modules]
cook_cc = "0.2.0-1"
EOF
cook modules install
ls cook_modules/share/lua/5.4/cook_cc/finders/raylib.lua
```

Expected: file present.

---

## Stage D — Examples + verification + closeout (back in cook#A)

### Task D1: examples/raylib-game/ scaffolding + source

**Files:**
- Create: `examples/raylib-game/cook.toml`
- Create: `examples/raylib-game/Cookfile`
- Create: `examples/raylib-game/src/main.c`
- Create: `examples/raylib-game/README.md`

- [ ] **Step 1: Create directories**

```bash
mkdir -p examples/raylib-game/src
```

- [ ] **Step 2: Write `examples/raylib-game/cook.toml`**

```toml
[registry]
indexes = ["https://rocks.usecook.com"]

[modules]
cook_cc = "0.2.0-1"
```

- [ ] **Step 3: Write `examples/raylib-game/src/main.c`**

```c
/*
 * Adapted from raylib's examples/core/core_basic_window.c
 *      https://github.com/raysan5/raylib (BSD-equivalent zlib license)
 *
 * Copyright (c) 2013-2024 Ramon Santamaria (@raysan5)
 */
#include "raylib.h"

int main(void)
{
    InitWindow(800, 450, "raylib via Cook");

    SetTargetFPS(60);

    while (!WindowShouldClose())
    {
        BeginDrawing();
        ClearBackground(RAYWHITE);
        DrawText("Congrats! You made your first raylib window via Cook!", 80, 200, 20, LIGHTGRAY);
        EndDrawing();
    }

    CloseWindow();
    return 0;
}
```

- [ ] **Step 4: Write `examples/raylib-game/Cookfile`**

```cook
use cook_cc

recipe game
    > local raylib = cook_cc.find_or_error("raylib")
    > cook_cc.bin("game", {
    >     sources     = { "src/main.c" },
    >     includes    = raylib.include_dirs,
    >     system_libs = raylib.system_libs,
    >     frameworks  = raylib.frameworks,
    > })
```

- [ ] **Step 5: Write `examples/raylib-game/README.md`**

```markdown
# raylib-game example

A minimal raylib demo built via Cook + `cook_cc`. Exercises `cc.find_or_error`,
the curated raylib finder, and macOS framework propagation through `cc.bin`.

## Install raylib

- **Debian/Ubuntu:** `sudo apt install libraylib-dev`
- **macOS (Homebrew):** `brew install raylib`

`cc.find_or_error` will raise with the install hint if raylib is not present.

## Build

```bash
cook game
```

The resulting binary is at `build/bin/game`.

## Notes

- macOS will warn about deprecated `OpenGL.framework` at link time — expected,
  not actionable in M1. raylib still links and runs correctly.
- The `cook.toml [registry] indexes` single-entry line is a workaround for
  the bundled-luarocks dual-server bug (SHI-211).
```

- [ ] **Step 6: Commit (lock file follows in next task)**

```bash
git -C ~/dev/cook add examples/raylib-game/cook.toml examples/raylib-game/Cookfile examples/raylib-game/src/main.c examples/raylib-game/README.md
git -C ~/dev/cook commit -m "feat(examples): raylib-game — M1 integration example"
```

### Task D2: examples/raylib-game/ install + lock

- [ ] **Step 1: Install cook_cc 0.2.0-1 into the example**

```bash
cd ~/dev/cook/examples/raylib-game && cook modules install
```

Expected output: `cook_cc-0.2.0-1` installed under `cook_modules/share/lua/5.4/`.

- [ ] **Step 2: Verify the cook.lock**

```bash
cat cook.lock
```

Expected: pins `cook_cc` to `0.2.0-1` and lists dependency tree.

- [ ] **Step 3: Commit lock + cook_modules tree**

```bash
git -C ~/dev/cook add examples/raylib-game/cook.lock examples/raylib-game/cook_modules
git -C ~/dev/cook commit -m "chore(examples): raylib-game — cook.lock + cook_modules install"
```

### Task D3: Pin existing examples to 0.2.0-1

**Files:**
- Modify: `examples/lua-build/cook.toml`
- Modify: `examples/fzf-picker/cook.toml`
- Modify: `examples/cpp-project/cook.toml`

- [ ] **Step 1: Pin lua-build**

Edit `examples/lua-build/cook.toml` — change `cook_cc = "0.1.2-1"` to `cook_cc = "0.2.0-1"`.

```bash
cd ~/dev/cook/examples/lua-build && cook modules install && cook  # smoke test
```

Expected: build succeeds; existing behavior unchanged.

- [ ] **Step 2: Pin fzf-picker**

Edit `examples/fzf-picker/cook.toml` — same change.

```bash
cd ~/dev/cook/examples/fzf-picker && cook modules install && cook  # smoke test
```

- [ ] **Step 3: Pin cpp-project**

Edit `examples/cpp-project/cook.toml` — same change.

```bash
cd ~/dev/cook/examples/cpp-project && cook modules install && cook  # smoke test
```

- [ ] **Step 4: Commit all three pins together**

```bash
git -C ~/dev/cook add examples/lua-build examples/fzf-picker examples/cpp-project
git -C ~/dev/cook commit -m "chore(examples): pin cook_cc 0.2.0-1 across lua-build, fzf-picker, cpp-project"
```

### Task D4: Run cargo conformance + verify cook-side green

- [ ] **Step 1: Run the full conformance suite**

```bash
cd ~/dev/cook && cargo test -p cook-lang --test conformance
```

Expected: all parse-only fixtures green; cc-* fixtures from M0 + the six new M1 fixtures all pass.

- [ ] **Step 2: Run the Astro build to confirm no new slug regressions**

```bash
cd ~/dev/cook/standard && npm run build 2>&1 | tail -20
```

Expected: build succeeds; the only warning is the pre-existing E-pre-v1-checklist slug ([SHI-212](https://linear.app/shiny-guru/issue/SHI-212)).

- [ ] **Step 3: If gate-m2 runs locally, exercise it**

If the gate-m2 matrix is invocable locally:

```bash
# Linux: install raylib then run the example
sudo apt install -y libraylib-dev
cd ~/dev/cook/examples/raylib-game && cook game
ls build/bin/game
```

Expected: `build/bin/game` exists.

- [ ] **Step 4: Commit nothing (verification only)**

No new commit — this step records the green signal in the PR description / Linear comment.

### Task D5: Mark PR cook#A ready-for-merge

- [ ] **Step 1: Push the example pins + raylib-game commits**

```bash
git -C ~/dev/cook push origin shi-134-m1-cc-finders
```

- [ ] **Step 2: Flip PR cook#A from draft to ready**

```bash
gh pr ready
```

- [ ] **Step 3: Confirm CI gate-m2 matrix green on Linux + macOS**

Wait for the CI run; resolve any platform-specific failures (most likely candidates: raylib install differences across runners, sdl2-config presence on macOS runner).

### Task D6: Squash-merge PR cook#A

- [ ] **Step 1: After approval, squash-merge**

Use the project's standard squash-merge flow. Squash commit title: `SHI-134: M1 — cc finder multi-strategy chain (Standard v0.2 + cook_cc 0.2.0)`.

### Task D7: Close out SHI-134

- [ ] **Step 1: Post the closeout comment on SHI-134**

Mirror the M0 closeout shape. Include:
- Rock version published (cook_cc 0.2.0-1)
- Standard sections touched (§9.2.3.7, §9.2.3.8, §9.2.3.12, §9.2.3.13, §9.2.4, §9.2.5, App. D CS-0067, App. B v0.1 rationale)
- Engine alignment: none expected (the resolver is pure Lua; no Rust changes)
- Examples migrated (lua-build, fzf-picker, cpp-project pinned; raylib-game added)
- Conformance fixtures added (six)
- Deferred follow-ups: SHI-210 (execute-mode harness), SHI-211 (luarocks dual-server), SHI-212 (slug warning)
- Unblocks: M2 (SHI-135 CMake-compat) and M3 (SHI-136 configure step)

- [ ] **Step 2: Mark SHI-134 Done**

Move the Linear ticket from Backlog → Done.

---

## Self-Review Notes for the Implementer

- **Spec coverage:** every numbered requirement in the design spec maps to one or more Stage A or Stage B tasks. The §9.2.3.7-13 surface changes land in A1-A4. The seven curated finders land in B9-B15. The resolver shape lands in B7. Frameworks plumbing lands in B16-B18. Diagnostics shape (FindResult.tried + find_or_error) lands in B7 + B19. Version constraints land in B2-B3 with per-finder usage in B9-B15.

- **TDD discipline:** every B-stage task that adds Lua starts with a failing busted test. Run busted between every step; never commit red.

- **Spec-first lockstep:** Stage A (Standard prose) commits land first in their PR. Stage B (cook_cc impl) commits land independently in their PR. The two PRs converge at Stage C (rock publish) and Stage D (example pinning, ready-for-merge).

- **If a curated finder spec mismatches its implementation:** prefer changing the implementation, not weakening the test. The spec's per-finder behaviour table (Section 5 of the design doc) is authoritative; the busted test encodes it; the Lua implementation must match.

- **Out-of-scope drift to avoid:** do not implement CMake-compat (M2 / SHI-135). Do not implement `framework_dirs` (deferred). Do not implement `prefer = "static"` even though the cache canonicalization supports the key. Do not implement execute-mode conformance (SHI-210 is the home for that).

- **Bundled-luarocks workaround:** every `cook.toml` in this plan uses `indexes = ["https://rocks.usecook.com"]` as a single-entry list. This is the SHI-211 workaround and is the right answer in every example until SHI-211 resolves.
