# SHI-176 Phase 4 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship four stub blessed-module rocks (`cook_cpp`, `cook_rust`, `cook_pnpm`, `cook_ai`) to `rocks.usecook.com`, automate publishing via Gitea Actions on `nas-arm64`, and clean-burn-delete `cook pull` from the cook repository.

**Architecture:** Three independent slices in three repos. Slice M4.3 (delete `cook pull`) lands in `~/dev/cook` and is fully independent. Slice M4.1 (author stub rocks) lands in `~/dev/cook-modules` and depends only on three Linear ticket IDs filed up-front. Slice M4.2 (publish CI) lands in `~/dev/cook-modules` and depends on M4.1's stubs being committed (it consumes their tags). The acceptance gate (M4.0) is a cross-repo end-to-end verification.

**Tech Stack:** Rust (cook CLI), Lua 5.4 (rock modules), LuaRocks 3.11.0 (`luarocks pack` + `luarocks-admin make-manifest`), Gitea Actions (CI), Git (publish substrate via `git+https`), Cloudflare Pages (CDN), GitHub (mirror for Pages).

**Spec:** `docs/superpowers/specs/2026-05-10-luarocks-phase-4-design.md`

**Working repos referenced:**
- `~/dev/cook` — this repo (M4.3 lives here)
- `~/dev/cook-modules` — sibling source-of-truth monorepo (M4.1, M4.2 live here)
- `~/dev/cook-rocks` — sibling rendered-index repo (M4.2 publishes into here; no edits in this plan beyond CI's automated commits)

---

## Section A — Pre-flight: file follow-up Linear tickets

The stub Lua modules embed SHI ticket IDs in their `error()` messages so users hitting `cook_cpp.placeholder()` know where the real implementation is being tracked. Those IDs must exist before the stubs are authored — otherwise the messages would lie or churn.

`cook_cpp` already has its follow-up ticket: **SHI-133** ("M0 — Promote cpp.lua to a blessed Standard module"). The other three need filing.

### Task A.1: File real-implementation tickets for cook_rust, cook_pnpm, cook_ai under SHI-176

**Files:**
- Modify: `docs/superpowers/plans/2026-05-10-luarocks-phase-4.md` (this plan — record the assigned IDs in the table below)

- [ ] **Step 1: File three new Linear issues**

For each of the three modules, create a Linear issue under the **Shiny-guru** team, in the **Cook modules — LuaRocks integration** project (the same project that holds SHI-176), with `Blocks: SHI-176` (and let SHI-176 hold the parent epic relation).

Use the Linear MCP `save_issue` tool (or the Linear web UI) with these three drafts:

```
Title: M0 — cook_rust blessed module: real implementation
Description:
  Replace the SHI-176 Phase 4 stub `cook_rust` rock (currently
  `0.0.1-1`, `placeholder()` errors with a pointer at this ticket)
  with a real cargo/rustc-driving Lua module. The stub reserves the
  name on rocks.usecook.com and proves the publish pipeline; this
  ticket designs and ships the actual module.

  Bump the rock version to 0.1.0-1 (or higher) when shipping;
  M4.2's CI handles the publish on tag push.

Project: Cook modules — LuaRocks integration
Team: Shiny-guru
Priority: Medium (raise when scheduled)
Blocked by: SHI-176
```

```
Title: M0 — cook_pnpm blessed module: real implementation
Description:
  Replace the SHI-176 Phase 4 stub `cook_pnpm` rock with a real
  pnpm-monorepo-driving Lua module. The stub reserves the name on
  rocks.usecook.com and proves the publish pipeline; this ticket
  designs and ships the actual module (likely consuming `lyaml`
  to read pnpm-lock.yaml and emit one Cook recipe per workspace
  package — see parent spec §"What this enables").

  Bump the rock version to 0.1.0-1 (or higher) when shipping;
  M4.2's CI handles the publish on tag push.

Project: Cook modules — LuaRocks integration
Team: Shiny-guru
Priority: Medium (raise when scheduled)
Blocked by: SHI-176
```

```
Title: M0 — cook_ai blessed module: real implementation
Description:
  Replace the SHI-176 Phase 4 stub `cook_ai` rock with a real
  LLM-streaming-completions Lua module. The stub reserves the name
  on rocks.usecook.com and proves the publish pipeline; this ticket
  designs and ships the actual module (likely a ~200-line module
  using lua-resty-http + lua-cjson, cache keyed on input hash +
  prompt version — see parent spec §"What this enables").

  Bump the rock version to 0.1.0-1 (or higher) when shipping;
  M4.2's CI handles the publish on tag push.

Project: Cook modules — LuaRocks integration
Team: Shiny-guru
Priority: Medium (raise when scheduled)
Blocked by: SHI-176
```

Expected: three tickets created and assigned IDs by Linear (e.g. `SHI-201`, `SHI-202`, `SHI-203` — actual numbers may differ).

- [ ] **Step 2: Record the assigned IDs in this plan**

Edit this plan's ticket-ID table below, replacing each `<TBD>` with the assigned Linear ID. The table is the source of truth that Section C tasks reference when authoring each stub's `error()` message.

| Module | Real-implementation ticket ID |
|---|---|
| cook_cpp | SHI-133 |
| cook_rust | `<TBD-rust>` |
| cook_pnpm | `<TBD-pnpm>` |
| cook_ai | `<TBD-ai>` |

- [ ] **Step 3: Commit the plan update**

```bash
cd ~/dev/cook
git add -f docs/superpowers/plans/2026-05-10-luarocks-phase-4.md
git commit -m "docs(phase4): record cook_rust/pnpm/ai follow-up ticket IDs in plan"
```

Expected: clean commit; the plan now has stable IDs for Section C to substitute into stub bodies.

---

## Section B — M4.3: Delete `cook pull`

Pure-subtraction slice. Order: capture pre-state → remove code → verify everything still builds → audit deps → finalize.

### Task B.1: Capture pre-deletion baseline

This task verifies the `cook pull` surface is what we expect to remove, so the deletion's diff is observable.

**Files:**
- Read-only: capture output to a transient note (not committed)

- [ ] **Step 1: Confirm `cook pull` is currently a working subcommand**

```bash
cd ~/dev/cook
cargo run -p cook-cli -- pull --help 2>&1 | head -20
```

Expected: clap help text describing `cook pull`, including `--list`, `--registry`, and a positional names argument.

- [ ] **Step 2: Confirm the pull integration test currently passes**

```bash
cd ~/dev/cook
cargo test -p cook-cli --test pull_integration 2>&1 | tail -10
```

Expected: tests pass (some number of tests OK, 0 failed).

- [ ] **Step 3: Inventory the touchpoints we'll edit**

```bash
cd ~/dev/cook
rg -l 'cook pull|::pull::|crate::pull|mod pull|cook_cli::pull' \
  cli/ README.md CONTRIBUTING.md docs/superpowers/specs/ 2>/dev/null
```

Expected: a file list matching the design spec's "Files removed" + "Files edited" sections. If new touchpoints appear, the plan needs updating before proceeding.

This task does not commit anything.

### Task B.2: Remove the `pull/` source directory

**Files:**
- Delete: `cli/crates/cook-cli/src/pull/` (entire directory, 9 files, 1953 LoC)

- [ ] **Step 1: Remove the directory**

```bash
cd ~/dev/cook
git rm -r cli/crates/cook-cli/src/pull/
```

Expected: 9 files removed; git records the deletion.

- [ ] **Step 2: Confirm `cargo build` now fails**

```bash
cd ~/dev/cook
cargo build -p cook-cli 2>&1 | tail -20
```

Expected: build fails with errors pointing at `cli/crates/cook-cli/src/lib.rs` (`pub mod pull;` references missing module) and `cli/crates/cook-cli/src/cli.rs` (`use crate::pull::PullArgs;` references missing module). This is correct — the next tasks fix those references.

This task does not commit yet (the tree won't build until Task B.3 finishes).

### Task B.3: Remove the `pull` clap subcommand and dispatch

**Files:**
- Modify: `cli/crates/cook-cli/src/lib.rs` (remove `pub mod pull;` declaration)
- Modify: `cli/crates/cook-cli/src/main.rs` (remove `use cook_cli::pull;` import and the `Cmd::Pull` dispatch arm)
- Modify: `cli/crates/cook-cli/src/cli.rs` (remove `use crate::pull::PullArgs;`, the `Pull(PullArgs)` enum variant, and the `pull_subcommand_with_names` test)

- [ ] **Step 1: Remove `pub mod pull;` from `lib.rs`**

Remove the `pub mod pull;` line at line 6 of `cli/crates/cook-cli/src/lib.rs`. Leave the surrounding lines untouched. Also drop the lib.rs doc comment fragment that mentions `pull::run_from_argv` (currently lines 2-5) — replace with a generic note or delete entirely if the rest of the doc is still meaningful.

Concrete edit: read `cli/crates/cook-cli/src/lib.rs` first to see the exact current contents, then remove only the pull-related lines (the `mod pull;` declaration and any `//!` doc lines mentioning pull).

- [ ] **Step 2: Remove pull import and dispatch from `main.rs`**

In `cli/crates/cook-cli/src/main.rs`:

- Remove line 15: `use cook_cli::pull;`
- Remove line 50 (the dispatch arm): `Some(Cmd::Pull(args)) => std::process::exit(pull::run(args)),`

- [ ] **Step 3: Remove pull from the clap surface in `cli.rs`**

In `cli/crates/cook-cli/src/cli.rs`:

- Remove line 17: `use crate::pull::PullArgs;`
- Remove lines 78-79 (the Pull enum variant + its doc):
  ```rust
  /// Pull cook_modules from a configured HTTP(S) registry.
  Pull(PullArgs),
  ```
- Remove the test `pull_subcommand_with_names` (lines 292-300 ish — read the file to find the exact span). The test's body references `Cmd::Pull(args)` which no longer exists.

- [ ] **Step 4: Verify the build now succeeds**

```bash
cd ~/dev/cook
cargo build -p cook-cli 2>&1 | tail -10
```

Expected: clean build. If unresolved references remain, grep `cli/crates/cook-cli/src/` for any remaining `pull` mention and fix.

- [ ] **Step 5: Confirm `cook pull --help` is now an unknown subcommand**

```bash
cd ~/dev/cook
cargo run -p cook-cli -- pull --help 2>&1 | head -5
```

Expected: clap reports an unknown subcommand and lists the available ones (`modules`, etc.). The "unknown subcommand" message is the clean-burn behaviour from the design spec.

- [ ] **Step 6: Commit**

```bash
cd ~/dev/cook
git add cli/crates/cook-cli/src/lib.rs cli/crates/cook-cli/src/main.rs cli/crates/cook-cli/src/cli.rs cli/crates/cook-cli/src/pull/
git commit -m "$(cat <<'EOF'
feat(phase4): remove cook pull subsystem (1953 LoC)

cook pull is superseded by cook modules (Phase 3). Delete the pull
source tree, the Pull clap variant, and the dispatch arm. Tests and
docs updated in follow-on commits.

Refs: SHI-176 M4.3
EOF
)"
```

Expected: a single commit removing 9 files in `pull/` plus 3 edited entry-point files.

### Task B.4: Remove the pull integration test

**Files:**
- Delete: `cli/crates/cook-cli/tests/pull_integration.rs` (190 LoC)

- [ ] **Step 1: Remove the test file**

```bash
cd ~/dev/cook
git rm cli/crates/cook-cli/tests/pull_integration.rs
```

- [ ] **Step 2: Verify `cargo test` still completes (with no pull tests)**

```bash
cd ~/dev/cook
cargo test -p cook-cli 2>&1 | tail -10
```

Expected: all remaining tests pass; `pull_integration` no longer in the test harness output.

- [ ] **Step 3: Commit**

```bash
cd ~/dev/cook
git commit -m "test(phase4): drop pull_integration test (cook pull removed)

Refs: SHI-176 M4.3"
```

### Task B.5: Update `README.md` and `CONTRIBUTING.md`

**Files:**
- Modify: `README.md` (replace `cook pull` examples with `cook modules install`)
- Modify: `CONTRIBUTING.md` (audit and replace any pull references)

- [ ] **Step 1: Read the current README pull section**

Read `README.md` lines 1-30 (or wherever the install/modules section sits — the current pull mentions are around lines 14-21).

- [ ] **Step 2: Replace the pull-block with a `cook modules` block**

In `README.md`, replace any block resembling:

```text
cook pull --list           # see what's available
cook pull cpp              # pull the cpp module into ./cook_modules/cpp
cook pull cpp rust         # pull multiple
```

with:

```text
cook modules install                # realise cook.toml + cook.lock into ./cook_modules
cook modules install cook_cpp        # add a single module dependency
cook modules install cook_cpp cook_rust   # add multiple
```

Also replace the surrounding prose. The current paragraph at line 19 starts with "The first time you pull from a given registry, cook prints a one-time disclaimer..." — remove that whole paragraph; `cook modules` does not have the same TOFU consent flow (it operates against pinned-source rocks via luarocks).

The "To use a different registry" line at 21 (`cook pull --registry https://my.registry/r ...`) should be replaced with:

```text
To use a different rocks index, edit `cook.toml`'s `[registry].indexes` list — see Standard §7 for resolution order.
```

- [ ] **Step 3: Audit `CONTRIBUTING.md`**

```bash
cd ~/dev/cook
rg -n 'cook pull|pull_registry|pull::' CONTRIBUTING.md
```

Expected: zero matches (current state). If matches appear, edit them to reference `cook modules` instead.

- [ ] **Step 4: Verify no stray pull mentions remain in user-facing docs**

```bash
cd ~/dev/cook
rg -n 'cook pull' README.md CONTRIBUTING.md
```

Expected: zero matches.

- [ ] **Step 5: Commit**

```bash
cd ~/dev/cook
git add README.md CONTRIBUTING.md
git commit -m "docs(phase4): replace cook pull examples with cook modules

Refs: SHI-176 M4.3"
```

### Task B.6: Update `modules/cli.rs` and `modules/manifest.rs` doc comments + remove the `[registry].url` field

The cook modules code currently documents itself in relation to the now-defunct `cook pull` (mentioned in three doc-comment locations). It also still parses `[registry].url` — a Phase 1 legacy field that only `cook pull` consumed. Both clean up here.

**Files:**
- Modify: `cli/crates/cook-cli/src/modules/cli.rs` (lines 4-5 doc comments)
- Modify: `cli/crates/cook-cli/src/modules/manifest.rs` (doc + `url` field + tests)

- [ ] **Step 1: Edit `modules/cli.rs` doc comment**

In `cli/crates/cook-cli/src/modules/cli.rs` lines 4-5, the doc comment says:

```rust
//! binary's subcommand dispatch (mirroring `cook pull`'s shape — see
//! cli/crates/cook-cli/src/main.rs and cli/crates/cook-cli/src/pull/).
```

Replace with:

```rust
//! binary's subcommand dispatch (one variant per `cook modules` subcommand).
```

(The "mirroring cook pull" historical anchor is gone; the comment can stand on its own.)

- [ ] **Step 2: Read `modules/manifest.rs` to find the `url` field and its tests**

Read `cli/crates/cook-cli/src/modules/manifest.rs` and locate:
- Line ~11: doc comment "Phase 1 `[registry].url` (which `cook pull` still consumes)"
- Line ~55: doc comment about `url` deserialization
- Line ~155: doc comment "`cook modules` ignores `url` (only `cook pull` consumes it)"
- The `RegistryConfig` struct's `url: Option<String>` field
- Any tests asserting `url` parses

- [ ] **Step 3: Remove the `url` field and its supporting code**

In `cli/crates/cook-cli/src/modules/manifest.rs`:

- Remove the three doc-comment fragments mentioning `cook pull`. Replace with neutral phrasing where the surrounding doc still makes sense, or delete the whole comment if it was solely about `url`.
- Remove the `url: Option<String>` field from `RegistryConfig` (or whichever struct holds it).
- Remove any `#[serde(default)]` / `#[serde(rename = "url")]` annotation on that field.
- Remove or update any test that asserts `url` parses — pull-only behaviour, so remove. Keep tests that exercise `indexes` parsing.

- [ ] **Step 4: Verify `cargo build` and `cargo test -p cook-cli` are green**

```bash
cd ~/dev/cook
cargo build -p cook-cli 2>&1 | tail -5 && cargo test -p cook-cli 2>&1 | tail -10
```

Expected: build clean; tests pass.

- [ ] **Step 5: Verify no `cook pull` mentions remain in active source**

```bash
cd ~/dev/cook
rg 'cook pull|crate::pull|::pull::' cli/ standard/
```

Expected: zero matches in `cli/` and `standard/` (any matches in `docs/superpowers/specs/` are dealt with in Task B.7).

- [ ] **Step 6: Commit**

```bash
cd ~/dev/cook
git add cli/crates/cook-cli/src/modules/
git commit -m "$(cat <<'EOF'
refactor(modules): drop [registry].url legacy field + pull doc refs

cook pull was the only consumer of [registry].url; with pull
deleted, the field is dead. Also clean stale 'mirroring cook pull'
doc comments in modules/cli.rs.

Refs: SHI-176 M4.3
EOF
)"
```

### Task B.7: Mark the original cook pull design spec superseded

**Files:**
- Modify: `docs/superpowers/specs/2026-05-07-cook-pull-registry-design.md` (prepend supersession marker)

- [ ] **Step 1: Read the spec's current header**

```bash
cd ~/dev/cook
sed -n '1,10p' docs/superpowers/specs/2026-05-07-cook-pull-registry-design.md
```

Expected: typical YAML/Markdown front-matter or a Markdown title.

- [ ] **Step 2: Prepend the supersession line**

Edit `docs/superpowers/specs/2026-05-07-cook-pull-registry-design.md` to add this line immediately after the title heading (before any other content):

```markdown
> **Status: Superseded by `2026-05-08-luarocks-modules-design.md` (SHI-176 Phase 3+4). The `cook pull` subsystem this spec describes was removed in Phase 4 (M4.3).**
```

If the file already has a `Status:` line, replace it. Do not delete the spec — design history is preserved.

- [ ] **Step 3: Commit**

```bash
cd ~/dev/cook
git add -f docs/superpowers/specs/2026-05-07-cook-pull-registry-design.md
git commit -m "docs(phase4): mark cook pull design spec superseded

Refs: SHI-176 M4.3"
```

(`-f` because `docs/superpowers/` is gitignored — the prior phase specs are committed via the same forced-add precedent.)

### Task B.8: Audit and clean `cook-cli` Cargo.toml

The `pull/` module pulled in some deps that nothing else uses. The cleanest verification is iterative: comment out a candidate, rebuild, restore if anything else needs it.

**Files:**
- Modify: `cli/crates/cook-cli/Cargo.toml`

- [ ] **Step 1: Identify candidate deps to remove**

Strong candidates (used in `pull/` but no longer referenced — confirmed via grep at plan-write time):

- `flate2` — used by `pull/archive.rs` only
- `tar` — used by `pull/archive.rs` only

Possible candidates (confirm with `cargo build` after each removal):

- `mockito` (dev-dep) — likely only `pull_integration.rs` used it; verify against remaining tests
- `serial_test` (dev-dep) — same; verify

Do **not** remove (still in use by `cook modules`):

- `sha2` — used by `modules/lockfile.rs`
- `base64` — used by `modules/lockfile.rs`
- `ureq` — used by other code paths (verify with grep)
- `dirs`, `humantime`, `toml`, `serde`, `serde_json`, `anyhow`, `clap`, `notify`, `glob` — all in use elsewhere

- [ ] **Step 2: Remove `flate2` and `tar` from `[dependencies]`**

Edit `cli/crates/cook-cli/Cargo.toml`. Remove these two lines from the `[dependencies]` block:

```toml
flate2 = "1"
tar = "0.4"
```

- [ ] **Step 3: Verify the build is still green**

```bash
cd ~/dev/cook
cargo build -p cook-cli 2>&1 | tail -10
```

Expected: clean build. If it fails on missing `flate2`/`tar`, restore the deps and grep for the consumer (`rg 'use flate2|use tar' cli/`).

- [ ] **Step 4: Probe `mockito` and `serial_test`**

For each dev-dep, comment out and rebuild tests:

```bash
cd ~/dev/cook
# Comment out mockito in [dev-dependencies]
cargo test -p cook-cli 2>&1 | tail -10
# If green, leave commented; if a test fails on missing mockito, restore.
```

Repeat for `serial_test`.

If a dev-dep is unused after Task B.4 (test removal), remove it; if any remaining test still uses it, restore.

- [ ] **Step 5: Final clean build + test**

```bash
cd ~/dev/cook
cargo build -p cook-cli 2>&1 | tail -5 && cargo test -p cook-cli 2>&1 | tail -10
```

Expected: both green.

- [ ] **Step 6: Commit**

```bash
cd ~/dev/cook
git add cli/crates/cook-cli/Cargo.toml
git commit -m "chore(phase4): drop pull-only Cargo deps (flate2, tar, ...)

Refs: SHI-176 M4.3"
```

(The commit message mentions the deps actually removed; if `mockito`/`serial_test` survived Step 4, drop them from the message.)

### Task B.9: Final verification (cook check, conformance, rg sweep)

**Files:**
- None (verification only)

- [ ] **Step 1: Run the umbrella check**

```bash
cd ~/dev/cook
cook check 2>&1 | tail -30
```

Expected: green across `cli.build`, `cli.test`, `cli.conformance`, `ts.test`, `ts.conformance`, `standard.build`, `standard.lint`. If any sub-check fails, fix the underlying issue (do not bypass).

- [ ] **Step 2: Run the conformance harness explicitly**

```bash
cd ~/dev/cook
cargo test -p cook-lang --test conformance 2>&1 | tail -10
```

Expected: green.

- [ ] **Step 3: Final `rg` sweep — no `cook pull` outside the superseded spec**

```bash
cd ~/dev/cook
rg 'cook pull|pull_registry|cook_pull' cli/ standard/ docs/ scripts/ README.md CONTRIBUTING.md 2>/dev/null
```

Expected: matches **only** in `docs/superpowers/specs/2026-05-07-cook-pull-registry-design.md`, and only on the supersession line. No matches in source, tests, fixtures, or other docs.

- [ ] **Step 4: Confirm `pull/` directory is gone**

```bash
cd ~/dev/cook
test ! -d cli/crates/cook-cli/src/pull/ && echo "pull/ removed" || echo "ERROR: pull/ still exists"
test ! -f cli/crates/cook-cli/tests/pull_integration.rs && echo "pull_integration.rs removed" || echo "ERROR: pull_integration.rs still exists"
```

Expected: both lines print "removed".

- [ ] **Step 5: Push the M4.3 branch (if working in a worktree)**

If this slice was developed in a worktree branched from `main`, push it now and open a PR. If working directly on `main`, this step is skipped.

This completes Section B (M4.3).

---

## Section C — M4.1: Author stub rocks in `~/dev/cook-modules`

Four parallel-shaped tasks. Each authors one stub directory in `~/dev/cook-modules/` with three files (`<name>.lua`, `<name>-0.0.1-1.rockspec`, `README.md`). The `error()` message in each stub embeds the corresponding follow-up Linear ticket ID from the table in Task A.1.

The stubs are **not tagged** in this section — tagging happens in Section D, after the publish CI workflow exists and can act on the tag push.

### Task C.1: Smoke-baseline (current state of cook-modules)

**Files:** none (read-only)

- [ ] **Step 1: Verify cook-modules is clean and on main**

```bash
cd ~/dev/cook-modules
git status
git branch --show-current
```

Expected: clean working tree; on `main` (or whatever the default branch is).

- [ ] **Step 2: Confirm cook_smoke is the only published module**

```bash
cd ~/dev/cook-modules
ls -d */
```

Expected: only `cook_smoke/` (and possibly nothing else). If other module directories exist, they predate this plan and should be left alone.

- [ ] **Step 3: Read cook_smoke for reference**

```bash
cd ~/dev/cook-modules
cat cook_smoke/cook_smoke.lua cook_smoke/cook_smoke-0.1.0-1.rockspec
```

Expected: the working contract from `project_cook_module_publishing.md`. The stubs in C.2–C.5 mirror this exact shape.

This task does not commit.

### Task C.2: Author `cook_cpp/`

**Files:**
- Create: `~/dev/cook-modules/cook_cpp/cook_cpp.lua`
- Create: `~/dev/cook-modules/cook_cpp/cook_cpp-0.0.1-1.rockspec`
- Create: `~/dev/cook-modules/cook_cpp/README.md`

- [ ] **Step 1: Create the directory**

```bash
mkdir -p ~/dev/cook-modules/cook_cpp
```

- [ ] **Step 2: Write `cook_cpp.lua`**

Create `~/dev/cook-modules/cook_cpp/cook_cpp.lua` with this exact content:

```lua
-- cook_cpp — SHI-176 Phase 4 stub.
-- The real implementation lands in SHI-133.

local M = {}
M.name = "cook_cpp"

function M.placeholder()
    error("[cook_cpp] SHI-176 Phase 4 stub. Real cook_cpp lands in SHI-133.", 2)
end

return M
```

- [ ] **Step 3: Write `cook_cpp-0.0.1-1.rockspec`**

Create `~/dev/cook-modules/cook_cpp/cook_cpp-0.0.1-1.rockspec` with this exact content:

```lua
package = "cook_cpp"
version = "0.0.1-1"
source = {
   url = "git+https://github.com/lioralabs/cook-modules.git",
   tag = "cook_cpp-0.0.1-1",
}
description = {
   summary = "Stub for the cook C/C++ build module — real implementation tracked in SHI-133",
   detailed = [[
      Stub rock published by SHI-176 Phase 4 to reserve the cook_cpp name on
      rocks.usecook.com and exercise the publish pipeline at realistic
      multi-rock scale. Calling cook_cpp.placeholder() errors with a pointer
      at the real-implementation ticket. Replace this rock's contents when
      SHI-133 lands.
   ]],
   homepage = "https://github.com/lioralabs/cook-modules",
   license = "MIT",
   maintainer = "Liora Labs <code@lioralabs.dev>",
}
dependencies = { "lua >= 5.4" }
build = {
   type = "builtin",
   modules = { cook_cpp = "cook_cpp/cook_cpp.lua" },
}
```

- [ ] **Step 4: Write `README.md`**

Create `~/dev/cook-modules/cook_cpp/README.md` with this exact content:

```markdown
# cook_cpp

**Stub rock for the cook C/C++ build module.** Real implementation tracked in [SHI-133](https://linear.app/shiny-guru/issue/SHI-133).

This rock currently exposes only `cook_cpp.placeholder()`, which raises an error pointing at the real-implementation ticket. It exists to reserve the `cook_cpp` name on `rocks.usecook.com` and to exercise the publish pipeline at multi-rock scale (SHI-176 Phase 4).

When SHI-133 ships, replace this directory's contents with the real implementation and bump the rock version to `0.1.0-1` (or higher). The Gitea Actions publish CI on tag push handles the rest.
```

- [ ] **Step 5: Local rockspec lint**

```bash
cd ~/dev/cook-modules/cook_cpp
luarocks lint cook_cpp-0.0.1-1.rockspec 2>&1
```

Expected: `cook_cpp-0.0.1-1.rockspec is OK` (or no errors).

- [ ] **Step 6: Commit (do NOT tag yet)**

```bash
cd ~/dev/cook-modules
git add cook_cpp/
git commit -m "feat(stub): cook_cpp 0.0.1-1 — Phase 4 stub rock (SHI-133 follow-up)"
```

Expected: clean commit. Section D handles tagging + publish.

### Task C.3: Author `cook_rust/`

**Files:**
- Create: `~/dev/cook-modules/cook_rust/cook_rust.lua`
- Create: `~/dev/cook-modules/cook_rust/cook_rust-0.0.1-1.rockspec`
- Create: `~/dev/cook-modules/cook_rust/README.md`

**Prerequisite:** the `cook_rust` real-implementation ticket ID is filled in Task A.1's table. Substitute it for `<TBD-rust>` in steps 2 and 4 below.

- [ ] **Step 1: Create the directory**

```bash
mkdir -p ~/dev/cook-modules/cook_rust
```

- [ ] **Step 2: Write `cook_rust.lua`**

Create `~/dev/cook-modules/cook_rust/cook_rust.lua` with this exact content (substitute `<TBD-rust>` with the assigned ID, e.g. `SHI-201`):

```lua
-- cook_rust — SHI-176 Phase 4 stub.
-- The real implementation lands in <TBD-rust>.

local M = {}
M.name = "cook_rust"

function M.placeholder()
    error("[cook_rust] SHI-176 Phase 4 stub. Real cook_rust lands in <TBD-rust>.", 2)
end

return M
```

- [ ] **Step 3: Write `cook_rust-0.0.1-1.rockspec`**

Create `~/dev/cook-modules/cook_rust/cook_rust-0.0.1-1.rockspec` with this exact content (substitute `<TBD-rust>`):

```lua
package = "cook_rust"
version = "0.0.1-1"
source = {
   url = "git+https://github.com/lioralabs/cook-modules.git",
   tag = "cook_rust-0.0.1-1",
}
description = {
   summary = "Stub for the cook Rust build module — real implementation tracked in <TBD-rust>",
   detailed = [[
      Stub rock published by SHI-176 Phase 4 to reserve the cook_rust name on
      rocks.usecook.com and exercise the publish pipeline at realistic
      multi-rock scale. Calling cook_rust.placeholder() errors with a pointer
      at the real-implementation ticket. Replace this rock's contents when
      <TBD-rust> lands.
   ]],
   homepage = "https://github.com/lioralabs/cook-modules",
   license = "MIT",
   maintainer = "Liora Labs <code@lioralabs.dev>",
}
dependencies = { "lua >= 5.4" }
build = {
   type = "builtin",
   modules = { cook_rust = "cook_rust/cook_rust.lua" },
}
```

- [ ] **Step 4: Write `README.md`**

Create `~/dev/cook-modules/cook_rust/README.md` with this exact content (substitute `<TBD-rust>`):

```markdown
# cook_rust

**Stub rock for the cook Rust build module.** Real implementation tracked in [<TBD-rust>](https://linear.app/shiny-guru/issue/<TBD-rust>).

This rock currently exposes only `cook_rust.placeholder()`, which raises an error pointing at the real-implementation ticket. It exists to reserve the `cook_rust` name on `rocks.usecook.com` and to exercise the publish pipeline at multi-rock scale (SHI-176 Phase 4).

When <TBD-rust> ships, replace this directory's contents with the real implementation and bump the rock version to `0.1.0-1` (or higher). The Gitea Actions publish CI on tag push handles the rest.
```

- [ ] **Step 5: Local rockspec lint**

```bash
cd ~/dev/cook-modules/cook_rust
luarocks lint cook_rust-0.0.1-1.rockspec 2>&1
```

Expected: `cook_rust-0.0.1-1.rockspec is OK`.

- [ ] **Step 6: Commit**

```bash
cd ~/dev/cook-modules
git add cook_rust/
git commit -m "feat(stub): cook_rust 0.0.1-1 — Phase 4 stub rock (<TBD-rust> follow-up)"
```

(Substitute `<TBD-rust>` in the commit message.)

### Task C.4: Author `cook_pnpm/`

**Files:**
- Create: `~/dev/cook-modules/cook_pnpm/cook_pnpm.lua`
- Create: `~/dev/cook-modules/cook_pnpm/cook_pnpm-0.0.1-1.rockspec`
- Create: `~/dev/cook-modules/cook_pnpm/README.md`

**Prerequisite:** the `cook_pnpm` real-implementation ticket ID is filled in Task A.1's table. Substitute it for `<TBD-pnpm>` below.

- [ ] **Step 1: Create the directory**

```bash
mkdir -p ~/dev/cook-modules/cook_pnpm
```

- [ ] **Step 2: Write `cook_pnpm.lua`**

Create `~/dev/cook-modules/cook_pnpm/cook_pnpm.lua` with this exact content (substitute `<TBD-pnpm>`):

```lua
-- cook_pnpm — SHI-176 Phase 4 stub.
-- The real implementation lands in <TBD-pnpm>.

local M = {}
M.name = "cook_pnpm"

function M.placeholder()
    error("[cook_pnpm] SHI-176 Phase 4 stub. Real cook_pnpm lands in <TBD-pnpm>.", 2)
end

return M
```

- [ ] **Step 3: Write `cook_pnpm-0.0.1-1.rockspec`**

Create `~/dev/cook-modules/cook_pnpm/cook_pnpm-0.0.1-1.rockspec` with this exact content (substitute `<TBD-pnpm>`):

```lua
package = "cook_pnpm"
version = "0.0.1-1"
source = {
   url = "git+https://github.com/lioralabs/cook-modules.git",
   tag = "cook_pnpm-0.0.1-1",
}
description = {
   summary = "Stub for the cook pnpm-monorepo module — real implementation tracked in <TBD-pnpm>",
   detailed = [[
      Stub rock published by SHI-176 Phase 4 to reserve the cook_pnpm name on
      rocks.usecook.com and exercise the publish pipeline at realistic
      multi-rock scale. Calling cook_pnpm.placeholder() errors with a pointer
      at the real-implementation ticket. Replace this rock's contents when
      <TBD-pnpm> lands.
   ]],
   homepage = "https://github.com/lioralabs/cook-modules",
   license = "MIT",
   maintainer = "Liora Labs <code@lioralabs.dev>",
}
dependencies = { "lua >= 5.4" }
build = {
   type = "builtin",
   modules = { cook_pnpm = "cook_pnpm/cook_pnpm.lua" },
}
```

- [ ] **Step 4: Write `README.md`**

Create `~/dev/cook-modules/cook_pnpm/README.md` with this exact content (substitute `<TBD-pnpm>`):

```markdown
# cook_pnpm

**Stub rock for the cook pnpm-monorepo module.** Real implementation tracked in [<TBD-pnpm>](https://linear.app/shiny-guru/issue/<TBD-pnpm>).

This rock currently exposes only `cook_pnpm.placeholder()`, which raises an error pointing at the real-implementation ticket. It exists to reserve the `cook_pnpm` name on `rocks.usecook.com` and to exercise the publish pipeline at multi-rock scale (SHI-176 Phase 4).

When <TBD-pnpm> ships, replace this directory's contents with the real implementation and bump the rock version to `0.1.0-1` (or higher). The Gitea Actions publish CI on tag push handles the rest.
```

- [ ] **Step 5: Local rockspec lint**

```bash
cd ~/dev/cook-modules/cook_pnpm
luarocks lint cook_pnpm-0.0.1-1.rockspec 2>&1
```

Expected: `cook_pnpm-0.0.1-1.rockspec is OK`.

- [ ] **Step 6: Commit**

```bash
cd ~/dev/cook-modules
git add cook_pnpm/
git commit -m "feat(stub): cook_pnpm 0.0.1-1 — Phase 4 stub rock (<TBD-pnpm> follow-up)"
```

### Task C.5: Author `cook_ai/`

**Files:**
- Create: `~/dev/cook-modules/cook_ai/cook_ai.lua`
- Create: `~/dev/cook-modules/cook_ai/cook_ai-0.0.1-1.rockspec`
- Create: `~/dev/cook-modules/cook_ai/README.md`

**Prerequisite:** the `cook_ai` real-implementation ticket ID is filled in Task A.1's table. Substitute it for `<TBD-ai>` below.

- [ ] **Step 1: Create the directory**

```bash
mkdir -p ~/dev/cook-modules/cook_ai
```

- [ ] **Step 2: Write `cook_ai.lua`**

Create `~/dev/cook-modules/cook_ai/cook_ai.lua` with this exact content (substitute `<TBD-ai>`):

```lua
-- cook_ai — SHI-176 Phase 4 stub.
-- The real implementation lands in <TBD-ai>.

local M = {}
M.name = "cook_ai"

function M.placeholder()
    error("[cook_ai] SHI-176 Phase 4 stub. Real cook_ai lands in <TBD-ai>.", 2)
end

return M
```

- [ ] **Step 3: Write `cook_ai-0.0.1-1.rockspec`**

Create `~/dev/cook-modules/cook_ai/cook_ai-0.0.1-1.rockspec` with this exact content (substitute `<TBD-ai>`):

```lua
package = "cook_ai"
version = "0.0.1-1"
source = {
   url = "git+https://github.com/lioralabs/cook-modules.git",
   tag = "cook_ai-0.0.1-1",
}
description = {
   summary = "Stub for the cook AI/LLM module — real implementation tracked in <TBD-ai>",
   detailed = [[
      Stub rock published by SHI-176 Phase 4 to reserve the cook_ai name on
      rocks.usecook.com and exercise the publish pipeline at realistic
      multi-rock scale. Calling cook_ai.placeholder() errors with a pointer
      at the real-implementation ticket. Replace this rock's contents when
      <TBD-ai> lands.
   ]],
   homepage = "https://github.com/lioralabs/cook-modules",
   license = "MIT",
   maintainer = "Liora Labs <code@lioralabs.dev>",
}
dependencies = { "lua >= 5.4" }
build = {
   type = "builtin",
   modules = { cook_ai = "cook_ai/cook_ai.lua" },
}
```

- [ ] **Step 4: Write `README.md`**

Create `~/dev/cook-modules/cook_ai/README.md` with this exact content (substitute `<TBD-ai>`):

```markdown
# cook_ai

**Stub rock for the cook AI/LLM module.** Real implementation tracked in [<TBD-ai>](https://linear.app/shiny-guru/issue/<TBD-ai>).

This rock currently exposes only `cook_ai.placeholder()`, which raises an error pointing at the real-implementation ticket. It exists to reserve the `cook_ai` name on `rocks.usecook.com` and to exercise the publish pipeline at multi-rock scale (SHI-176 Phase 4).

When <TBD-ai> ships, replace this directory's contents with the real implementation and bump the rock version to `0.1.0-1` (or higher). The Gitea Actions publish CI on tag push handles the rest.
```

- [ ] **Step 5: Local rockspec lint**

```bash
cd ~/dev/cook-modules/cook_ai
luarocks lint cook_ai-0.0.1-1.rockspec 2>&1
```

Expected: `cook_ai-0.0.1-1.rockspec is OK`.

- [ ] **Step 6: Commit + push all four stubs**

```bash
cd ~/dev/cook-modules
git add cook_ai/
git commit -m "feat(stub): cook_ai 0.0.1-1 — Phase 4 stub rock (<TBD-ai> follow-up)"
git push origin main
git push github main
```

(`origin` → Gitea, `github` → GitHub. Confirm with `git remote -v` if uncertain. Both remotes need the source commits because each rockspec's `source.url` points at GitHub — luarocks pack will clone from there at the tagged commit. The publish CI in Section D triggers on tag push to either remote.)

This completes Section C. Four stub directories committed; no tags pushed yet.

---

## Section D — M4.2: Publish CI on `nas-arm64`

The CI workflow lives in cook-modules. It triggers on tag push, packs the corresponding rock, lands artifacts in cook-rocks-index (Gitea), and force-pushes to the GitHub mirror. Cloudflare Pages deploys from there.

### Task D.1: Author the publish workflow

**Files:**
- Create: `~/dev/cook-modules/.gitea/workflows/publish.yml`

- [ ] **Step 1: Create the workflows directory**

```bash
mkdir -p ~/dev/cook-modules/.gitea/workflows
```

- [ ] **Step 2: Write the workflow file**

Create `~/dev/cook-modules/.gitea/workflows/publish.yml` with this exact content:

```yaml
name: publish

on:
  push:
    tags:
      - "*-*.*.*-*"

jobs:
  publish:
    runs-on: nas-arm64
    steps:
      - name: Tag-format guard
        id: parse
        run: |
          set -euo pipefail
          TAG="${GITHUB_REF_NAME:-}"
          if [[ -z "$TAG" ]]; then
            echo "ERROR: no GITHUB_REF_NAME present (expected on tag push)" >&2
            exit 1
          fi
          if ! [[ "$TAG" =~ ^([a-z_]+)-([0-9]+\.[0-9]+\.[0-9]+-[0-9]+)$ ]]; then
            echo "ERROR: tag '$TAG' does not match <module>-<MAJOR>.<MINOR>.<PATCH>-<rockrev>" >&2
            exit 1
          fi
          MODULE="${BASH_REMATCH[1]}"
          VERSION="${BASH_REMATCH[2]}"
          echo "module=$MODULE" >> "$GITHUB_OUTPUT"
          echo "version=$VERSION" >> "$GITHUB_OUTPUT"
          echo "Parsed: module=$MODULE version=$VERSION"

      - name: Checkout cook-modules at the tag
        uses: actions/checkout@v4
        with:
          ref: ${{ github.ref_name }}
          path: cook-modules

      - name: Pack the rock
        working-directory: cook-modules/${{ steps.parse.outputs.module }}
        run: |
          set -euo pipefail
          MODULE="${{ steps.parse.outputs.module }}"
          VERSION="${{ steps.parse.outputs.version }}"
          luarocks pack "${MODULE}-${VERSION}.rockspec"
          ls -la "${MODULE}-${VERSION}.src.rock"

      - name: Checkout cook-rocks-index
        uses: actions/checkout@v4
        with:
          repository: LioraLabs/cook-rocks-index
          token: ${{ secrets.GITEA_INDEX_TOKEN }}
          path: cook-rocks-index

      - name: Stage artifacts + regenerate manifest
        working-directory: cook-rocks-index
        run: |
          set -euo pipefail
          MODULE="${{ steps.parse.outputs.module }}"
          VERSION="${{ steps.parse.outputs.version }}"
          cp "../cook-modules/${MODULE}/${MODULE}-${VERSION}.rockspec" .
          cp "../cook-modules/${MODULE}/${MODULE}-${VERSION}.src.rock" .
          luarocks-admin make-manifest .
          ls -la "${MODULE}-${VERSION}".*

      - name: Commit and push to Gitea (idempotent)
        working-directory: cook-rocks-index
        run: |
          set -euo pipefail
          MODULE="${{ steps.parse.outputs.module }}"
          VERSION="${{ steps.parse.outputs.version }}"
          git config user.email "ci@lioralabs.dev"
          git config user.name "cook-modules CI"
          git add manifest manifest-5.* "${MODULE}-${VERSION}".rockspec "${MODULE}-${VERSION}".src.rock
          if git diff --quiet --staged; then
            echo "No changes to commit (idempotent re-run)"
          else
            git commit -m "publish: ${MODULE}-${VERSION}"
            git push origin main
          fi

      - name: Force-push to GitHub mirror (Cloudflare Pages source)
        working-directory: cook-rocks-index
        run: |
          set -euo pipefail
          git remote add github "https://x-access-token:${{ secrets.GITHUB_MIRROR_TOKEN }}@github.com/lioralabs/cook-rocks-index.git" || true
          git push --force github main
```

Notes on the workflow:
- `gitea.ref_name` is Gitea Actions' equivalent of `github.ref_name`; the env-var fallback in the parse step handles either runner type.
- `actions/checkout@v4` works on Gitea Actions (it's compatible with the v4 action surface).
- The mirror push uses `--force` per `project_cook_module_publishing.md`'s "snapshot-branch model" for cook-rocks-index on GitHub.
- The `git remote add` is wrapped with `|| true` so re-running the job (e.g. from a re-tag) doesn't fail on "remote exists."

- [ ] **Step 3: Lint the YAML**

```bash
cd ~/dev/cook-modules
# If yamllint is available; otherwise use any YAML linter you have
yamllint .gitea/workflows/publish.yml 2>&1 || python3 -c "import yaml; yaml.safe_load(open('.gitea/workflows/publish.yml'))"
```

Expected: no errors.

- [ ] **Step 4: Commit + push**

```bash
cd ~/dev/cook-modules
git add .gitea/workflows/publish.yml
git commit -m "ci(phase4): publish workflow for tag-push to rocks index

Triggers on tags matching <module>-<MAJOR>.<MINOR>.<PATCH>-<rockrev>.
Packs the rockspec, lands artifacts in cook-rocks-index, regenerates
the manifest, and force-pushes the GitHub mirror.

Refs: SHI-176 M4.2"
git push origin main
```

### Task D.2: Configure Gitea Actions secrets and runner tooling

This task is operationally manual — it touches Gitea infrastructure, not code.

**Files:** none in code (Gitea web UI / runner host)

- [ ] **Step 1: Create / locate `GITEA_INDEX_TOKEN`**

In Gitea, navigate to: `LioraLabs` org settings → Applications → Generate new token, scoped to `write:repository` on `LioraLabs/cook-rocks-index`. Copy the token.

In Gitea Actions: org-level secrets → Add secret `GITEA_INDEX_TOKEN` → paste the token.

- [ ] **Step 2: Create / locate `GITHUB_MIRROR_TOKEN`**

In GitHub: Settings → Developer settings → Personal access tokens → Fine-grained tokens → Generate new token. Resource owner: `lioralabs`. Repository access: only `cook-rocks-index`. Permissions: Contents = read/write. Expiry: 1 year (or your standard rotation cadence).

In Gitea Actions: org-level secrets → Add secret `GITHUB_MIRROR_TOKEN` → paste the token.

- [ ] **Step 3: Confirm `nas-arm64` runner has `luarocks` available**

SSH to the runner host (or use the Gitea runner's exec shell):

```bash
luarocks --version
```

Expected: `luarocks 3.11.0` (or whatever is current). If `luarocks` is missing:

```bash
sudo apt update && sudo apt install -y luarocks
luarocks --version
```

- [ ] **Step 4: Update memory with secret rotation date**

Edit `~/.claude/projects/-home-alex-dev-cook/memory/project_cook_module_publishing.md` and add to the "How to apply" section:

```markdown
- **Secret rotation:** `GITHUB_MIRROR_TOKEN` (Gitea Actions org-level secret) is a fine-grained GitHub PAT, contents:write on cook-rocks-index. Issued: <YYYY-MM-DD>. Renew by: <YYYY-MM-DD + 11mo>. Stale token = silent CI failure on the mirror push step.
```

(Substitute today's date and the renewal date.)

- [ ] **Step 5: Commit the memory update**

The memory file is in a different repo (`~/.claude/projects/-home-alex-dev-cook/memory/`); it auto-syncs through the alex-memory mechanism. No git commit in cook here. If your environment uses a different memory persistence model, follow that.

### Task D.3: First-tag canary — publish `cook_cpp-0.0.1-1` end-to-end

**Files:** none (this is a tag push + observation)

- [ ] **Step 1: Push the cook_cpp tag**

```bash
cd ~/dev/cook-modules
git tag cook_cpp-0.0.1-1
git push origin cook_cpp-0.0.1-1
```

Push the tag to GitHub as well — the rockspec's `source.url` points at GitHub, so `luarocks pack` resolves the tag there:

```bash
git push github cook_cpp-0.0.1-1
```

- [ ] **Step 2: Observe the workflow run**

Open Gitea → cook-modules → Actions tab. Find the `publish` workflow run for tag `cook_cpp-0.0.1-1`. Watch each step:

- Tag-format guard → must pass; output should show `Parsed: module=cook_cpp version=0.0.1-1`.
- Pack the rock → must produce `cook_cpp-0.0.1-1.src.rock`.
- Stage artifacts + regenerate manifest → `manifest`, `manifest-5.4`, etc. updated.
- Commit and push to Gitea → either a publish commit lands or "no changes" (first run = commit).
- Force-push to GitHub mirror → succeeds.

Expected: all steps green. If any step fails, fix the underlying issue (do **not** force the workflow green by skipping checks).

- [ ] **Step 3: Verify the index reflects the new rock**

```bash
# Wait up to 60s for Cloudflare Pages to redeploy.
for i in 1 2 3 4 5 6; do
  if curl -fsSL https://rocks.usecook.com/manifest-5.4 | grep -q "cook_cpp"; then
    echo "cook_cpp present in manifest-5.4 after ${i}0s"
    break
  fi
  sleep 10
done
curl -fsSL https://rocks.usecook.com/cook_cpp-0.0.1-1.src.rock --output /tmp/check.src.rock
ls -la /tmp/check.src.rock
```

Expected: `manifest-5.4` contains `cook_cpp`; the `.src.rock` downloads successfully (non-zero size).

- [ ] **Step 4: Install the rock via luarocks against the live index**

```bash
rm -rf /tmp/m4-canary
luarocks install cook_cpp --tree /tmp/m4-canary --server https://rocks.usecook.com
ls /tmp/m4-canary/share/lua/5.4/cook_cpp.lua
```

Expected: install succeeds; `cook_cpp.lua` exists in the tree.

- [ ] **Step 5: Verify the placeholder error fires correctly**

```bash
lua5.4 -e "package.path = '/tmp/m4-canary/share/lua/5.4/?.lua;' .. package.path; require('cook_cpp').placeholder()"
```

Expected: error printed: `[cook_cpp] SHI-176 Phase 4 stub. Real cook_cpp lands in SHI-133.`

If any of steps 3–5 fail: troubleshoot the publish workflow (most likely Gitea token scope, Cloudflare Pages cache window, or rockspec dir resolution). Do not retag until the underlying issue is fixed; rectify on the workflow side and either rerun the existing tag's workflow from the Gitea Actions UI or push a new tag.

### Task D.4: Tag the remaining three stubs

**Files:** none (tag pushes only)

- [ ] **Step 1: Push all three tags**

```bash
cd ~/dev/cook-modules
git tag cook_rust-0.0.1-1
git tag cook_pnpm-0.0.1-1
git tag cook_ai-0.0.1-1
git push origin cook_rust-0.0.1-1 cook_pnpm-0.0.1-1 cook_ai-0.0.1-1
# Push to GitHub remote as well if applicable.
git push github cook_rust-0.0.1-1 cook_pnpm-0.0.1-1 cook_ai-0.0.1-1
```

- [ ] **Step 2: Watch all three workflow runs to green**

Open Gitea Actions and confirm three new `publish` runs land green. Each one independently lands its rock in cook-rocks-index and pushes the GitHub mirror.

- [ ] **Step 3: Verify all four stubs are now installable**

```bash
for MOD in cook_cpp cook_rust cook_pnpm cook_ai; do
  rm -rf /tmp/m4-batch
  luarocks install "$MOD" --tree /tmp/m4-batch --server https://rocks.usecook.com
  test -f "/tmp/m4-batch/share/lua/5.4/${MOD}.lua" && echo "$MOD: OK" || echo "$MOD: FAIL"
done
```

Expected: all four print "OK".

This completes Section D.

---

## Section E — M4.0 Acceptance Gate

A single end-to-end run that validates the full Phase 4 outcome: published stubs reachable through `cook modules`, error messages fire correctly inside a Cookfile, cook builds clean without `pull/`, and no `cook pull` mentions remain in active source.

### Task E.1: Run the end-to-end acceptance gate

**Files:**
- Create (transient): `/tmp/m4-fixture/cook.toml`
- Create (transient): `/tmp/m4-fixture/Cookfile`

- [ ] **Step 1: Cook builds clean from M4.3**

```bash
cd ~/dev/cook
cargo build --workspace 2>&1 | tail -5
cargo test --workspace 2>&1 | tail -10
cook check 2>&1 | tail -20
```

Expected: all three green. If `cook check` fails on `standard.lint` or any sub-check, fix root cause (do not bypass).

- [ ] **Step 2: No `cook pull` mentions in active source**

```bash
cd ~/dev/cook
rg 'cook pull|pull_registry|cook_pull' cli/ standard/ docs/ scripts/ README.md CONTRIBUTING.md 2>/dev/null
```

Expected: matches **only** in `docs/superpowers/specs/2026-05-07-cook-pull-registry-design.md`, on the supersession line. No matches elsewhere.

- [ ] **Step 3: Build a Cookfile fixture using a stub**

Create the directory and write the fixture files:

```bash
mkdir -p /tmp/m4-fixture
cd /tmp/m4-fixture
```

Write `/tmp/m4-fixture/cook.toml`:

```toml
[registry]
indexes = ["https://rocks.usecook.com"]

[modules]
cook_cpp = "*"
```

Write `/tmp/m4-fixture/Cookfile`:

```
use cook_cpp as cpp

chore "smoke"
    > cpp.placeholder()
```

(The exact Cookfile syntax above should match the language Standard. If `chore` requires different framing in current cook syntax, adjust to invoke `cpp.placeholder()` from a runnable chore body.)

- [ ] **Step 4: Run `cook modules install`**

```bash
cd /tmp/m4-fixture
~/dev/cook/cli/target/debug/cook modules install 2>&1 | tail -20
```

(Or use the installed `cook` if `~/.cook/bin/cook` is on PATH.)

Expected: clean install; `/tmp/m4-fixture/cook.lock` is created with a `cook_cpp 0.0.1-1` entry pointing at `https://rocks.usecook.com/cook_cpp-0.0.1-1.src.rock` and a populated `integrity` field.

- [ ] **Step 5: Run the chore — placeholder() must error informatively**

```bash
cd /tmp/m4-fixture
~/dev/cook/cli/target/debug/cook smoke 2>&1 | tail -20
```

Expected: chore fails with the error message `[cook_cpp] SHI-176 Phase 4 stub. Real cook_cpp lands in SHI-133.` The stack frame should blame the Cookfile line that called `cpp.placeholder()`, not the stub Lua file.

- [ ] **Step 6: Verify all four stubs install cleanly via cook modules**

Update `/tmp/m4-fixture/cook.toml`'s `[modules]` table:

```toml
[modules]
cook_cpp = "*"
cook_rust = "*"
cook_pnpm = "*"
cook_ai = "*"
```

Re-run install:

```bash
cd /tmp/m4-fixture
rm -rf cook_modules cook.lock
~/dev/cook/cli/target/debug/cook modules install 2>&1 | tail -30
```

Expected: clean install of all four stubs; `cook.lock` lists all four with their `0.0.1-1` versions and `rocks.usecook.com` source URLs.

- [ ] **Step 7: Smoke each stub's placeholder error**

For each module, write a one-line Cookfile chore that calls `<module>.placeholder()` and verify the error message is correct:

```bash
cd /tmp/m4-fixture
for MOD in cook_cpp cook_rust cook_pnpm cook_ai; do
  echo "=== $MOD ==="
  lua5.4 -e "package.path = './cook_modules/share/lua/5.4/?.lua;' .. package.path; require('${MOD}').placeholder()" 2>&1 | head -3
done
```

Expected: each prints its own `[cook_<mod>] SHI-176 Phase 4 stub. Real cook_<mod> lands in <ticket-id>.` message.

- [ ] **Step 8: Acceptance gate passes**

When steps 1–7 are all green, M4.0 is satisfied. Phase 4 is shippable.

This completes Section E.

---

## Section F — Final integration

### Task F.1: Update memory and close out

**Files:**
- Modify: `~/.claude/projects/-home-alex-dev-cook/memory/MEMORY.md`
- Create: `~/.claude/projects/-home-alex-dev-cook/memory/project_shi176_phase_4_done.md`

- [ ] **Step 1: Write the Phase 4 completion memory**

Create `~/.claude/projects/-home-alex-dev-cook/memory/project_shi176_phase_4_done.md`:

```markdown
---
name: SHI-176 Phase 4 shipped
description: Phase 4 of SHI-176 (stub blessed-module rocks + publish CI + cook pull deletion) shipped on <YYYY-MM-DD>. Phases 1–4 cover the Unix story end-to-end; Phase 5 (Windows) is the only remaining SHI-176 phase.
type: project
---

Phase 4 of the SHI-176 (Cook modules — LuaRocks integration) epic shipped on <YYYY-MM-DD>:

- **Four stub blessed-module rocks** published to `rocks.usecook.com`: `cook_cpp` (0.0.1-1, real impl in SHI-133), `cook_rust` (0.0.1-1, real impl in <TBD-rust>), `cook_pnpm` (0.0.1-1, real impl in <TBD-pnpm>), `cook_ai` (0.0.1-1, real impl in <TBD-ai>). Each errors with an SHI-pointing message when called.
- **Publish CI on nas-arm64**: tag-push of `<module>-<MAJOR>.<MINOR>.<PATCH>-<rockrev>` triggers Gitea Actions to pack the rock, land artifacts in cook-rocks-index, force-push the GitHub mirror; Cloudflare Pages picks it up.
- **`cook pull` clean-burn deletion**: 1953 LoC under `cli/crates/cook-cli/src/pull/` removed, the `pull` clap variant gone, README/CONTRIBUTING updated, original `2026-05-07-cook-pull-registry-design.md` marked Status: Superseded.

**Why:** Phase 4's reframed goal was to prove the publish pipeline at four-rock scale and unblock SHI-133 (the real cook_cpp project) — not to author four full modules at once. Each blessed module's real implementation is its own follow-up Linear project.

**How to apply:**
- New blessed module → file a real-implementation ticket, then ship a stub rock first to reserve the name; CI handles the publish on tag push.
- `cook modules install <name>` is the canonical module-acquisition path. There is no `cook pull` and no migration alias.
- Phase 5 (Windows packaging) is the only remaining SHI-176 phase.
```

- [ ] **Step 2: Add a one-line entry to MEMORY.md**

Edit `~/.claude/projects/-home-alex-dev-cook/memory/MEMORY.md` and add this line (alphabetically near the other SHI-176 phase entries):

```markdown
- [SHI-176 Phase 4 shipped](project_shi176_phase_4_done.md) — stub rocks (cpp/rust/pnpm/ai) + nas-arm64 publish CI + cook pull deletion (-1953 LoC); Phase 5 (Windows) is the only SHI-176 phase remaining
```

- [ ] **Step 3: Push everything**

```bash
cd ~/dev/cook
git push origin main

cd ~/dev/cook-modules
git push origin main --tags
git push github main --tags
```

Expected: all three remotes (cook, cook-modules Gitea, cook-modules GitHub) up to date.

- [ ] **Step 4: Mark Linear epic Phase 4 complete**

In Linear: SHI-176 → add a comment summarizing Phase 4 completion (link to the spec, the plan, and the acceptance gate output). If Phase 4 had its own milestone or sub-epic, mark it Done. Do **not** close SHI-176 — Phase 5 (Windows) is still pending.

This completes the implementation plan.

---

## Summary of files touched

**`~/dev/cook` (M4.3):**
- Deleted: `cli/crates/cook-cli/src/pull/` (9 files, 1953 LoC)
- Deleted: `cli/crates/cook-cli/tests/pull_integration.rs` (190 LoC)
- Modified: `cli/crates/cook-cli/src/lib.rs` (drop `pub mod pull;` and pull doc comments)
- Modified: `cli/crates/cook-cli/src/main.rs` (drop `use cook_cli::pull;` and `Cmd::Pull` arm)
- Modified: `cli/crates/cook-cli/src/cli.rs` (drop PullArgs import, Pull variant, pull subcommand test)
- Modified: `cli/crates/cook-cli/src/modules/cli.rs` (doc comment update)
- Modified: `cli/crates/cook-cli/src/modules/manifest.rs` (drop `[registry].url` field + pull doc refs)
- Modified: `cli/crates/cook-cli/Cargo.toml` (drop `flate2`, `tar`, possibly `mockito`/`serial_test`)
- Modified: `README.md`
- Modified: `CONTRIBUTING.md` (audited; likely no edits needed)
- Modified: `docs/superpowers/specs/2026-05-07-cook-pull-registry-design.md` (Status: Superseded marker)
- Modified: `docs/superpowers/plans/2026-05-10-luarocks-phase-4.md` (this plan — record ticket IDs)

**`~/dev/cook-modules` (M4.1 + M4.2):**
- Created: `cook_cpp/{cook_cpp.lua, cook_cpp-0.0.1-1.rockspec, README.md}`
- Created: `cook_rust/{cook_rust.lua, cook_rust-0.0.1-1.rockspec, README.md}`
- Created: `cook_pnpm/{cook_pnpm.lua, cook_pnpm-0.0.1-1.rockspec, README.md}`
- Created: `cook_ai/{cook_ai.lua, cook_ai-0.0.1-1.rockspec, README.md}`
- Created: `.gitea/workflows/publish.yml`
- Tagged: `cook_cpp-0.0.1-1`, `cook_rust-0.0.1-1`, `cook_pnpm-0.0.1-1`, `cook_ai-0.0.1-1`

**`~/dev/cook-rocks` (M4.2 — automated by CI, not direct edits):**
- Added: `cook_cpp-0.0.1-1.{rockspec, src.rock}` (and 3 siblings)
- Modified: `manifest`, `manifest-5.4` (and other manifest-5.x variants)
- Mirror force-pushed to GitHub for Cloudflare Pages

**Memory:**
- Created: `~/.claude/projects/-home-alex-dev-cook/memory/project_shi176_phase_4_done.md`
- Modified: `~/.claude/projects/-home-alex-dev-cook/memory/MEMORY.md` (one-line entry)
- Modified: `~/.claude/projects/-home-alex-dev-cook/memory/project_cook_module_publishing.md` (PAT rotation date)

**Linear:**
- Created: 3 follow-up tickets (`<TBD-rust>`, `<TBD-pnpm>`, `<TBD-ai>`) under SHI-176 epic
- Updated: SHI-176 with Phase 4 completion comment
