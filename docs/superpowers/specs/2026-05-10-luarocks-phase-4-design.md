# SHI-176 Phase 4 — Stub blessed-module rocks, publish CI, retire `cook pull`

Date: 2026-05-10
Status: Design — pending implementation plan
Linear: SHI-176 epic, M4.1–M4.3 sub-tickets (to be filed by the implementation plan)
Parent specs:
- `2026-05-08-luarocks-modules-design.md` — architectural design (what we are building across all phases)
- `2026-05-09-luarocks-modules-decomposition-design.md` — Phase 1–3 decomposition and dispatch protocol
- `2026-05-10-luarocks-phase-3-design.md` — Phase 3 (`cook modules` CLI) operational design
Companion memory:
- `project_cook_module_publishing.md` — three-repo publishing pipeline (cook-modules → cook-rocks-index → rocks.usecook.com)
- `project_shi176_phase_2_done.md` and `project_shi176_phase_1_done.md` — phase ground truth

This document specifies Phase 4 in operational detail: which blessed modules ship as rocks now (and which are deferred), how the publish pipeline becomes a one-tag-push operation on `nas-arm64`, and the clean-burn deletion of the legacy `cook pull` subsystem.

## Context

Phases 1–3 of SHI-176 shipped end-to-end. The `cook modules` CLI installs and resolves real rocks against the live `rocks.usecook.com` index, the runtime extends `package.path` / `package.cpath` correctly, and Standard §7 normatively documents the search-path order. The `cook_smoke` fixture rock proves the publish pipeline works manually for one module.

The original Phase 4 charter (parent spec, §"Phased rollout") read: *"Author and publish `cook-cpp`, `cook-rust`, `cook-pnpm`, `cook-ai` rockspecs against the cook index. Delete `cook pull` and its 2000 LoC. Standard §7 update lands in the same PR."* Two facts have shifted since that charter was written:

1. **Standard §7 already landed in Phase 3** (commit `1d5722b`), co-PR'd with the runtime resolution code per the spec-first rule. Phase 4 carries no Standard impact.
2. **Only `cpp/init.lua` exists today** (973 lines under `~/dev/cook_modules/modules/cpp/`); `rust`, `pnpm`, `ai` were named in the parent spec as aspirational but were never authored. Treating Phase 4 as "author four full modules" would conflate the publish-pipeline work with four independent module-design exercises that each warrant their own spec.

Phase 4 therefore reframes around what it can actually ship without dragging in unrelated module-authoring work: **stub rocks** that reserve the names, exercise the pipeline at four-rock scale, and unblock SHI-133 (the "real" cook_cpp project, currently blocked by SHI-176). Each blessed module's real implementation becomes its own follow-up Linear project, building on the stubs Phase 4 publishes.

## Scope

Phase 4 of SHI-176, decomposed into three slices:

- **M4.1 — Stub rocks.** Author and tag `cook_cpp`, `cook_rust`, `cook_pnpm`, `cook_ai` in the cook-modules monorepo as minimal pure-Lua placeholder rocks. Reserves names on `rocks.usecook.com` and validates the pipeline at realistic rock-count scale.
- **M4.2 — Publish CI.** Wire Gitea Actions on the `nas-arm64` runner to publish on tag push: `luarocks pack` → land artifacts in `cook-rocks-index` → regenerate manifest → mirror to GitHub for Cloudflare Pages pickup. Replaces today's six-step manual ritual.
- **M4.3 — Delete `cook pull`.** Clean-burn removal of `cli/crates/cook-cli/src/pull/` (1953 LoC), the `pull` clap subcommand, all related tests/fixtures, README/CONTRIBUTING references, and supersession of the original `2026-05-07-cook-pull-registry-design.md` design doc.

Out of scope:

- **Real implementations of cook_cpp / cook_rust / cook_pnpm / cook_ai.** Each gets its own Linear project; Phase 4 ships only the stub. SHI-133 already exists as the cook_cpp follow-up.
- **The legacy `~/dev/cook_modules/modules/cpp/init.lua`.** Phase 4 leaves it untouched. SHI-133 will pull it forward as starting material when the real cook_cpp project begins.
- **Phase 5 — Windows packaging.** Re-brainstormed when the Phase 4 publish pipeline is ground truth.
- **Pre-built binary rocks (`.linux-x86_64.rock`, `.darwin-arm64.rock`).** Source rocks suffice — they are platform-agnostic, and the stubs contain no C extensions. Binary rocks return when (and if) the real blessed modules need them; deferred per the parent spec.
- **Migration aids for `cook pull`.** Clean burn: no deprecation alias, no fallback hint at unknown-subcommand time. Clap's standard "unknown subcommand" message is the user-facing handle.

## Topology (no change in this phase)

Phase 4 only adds files into the existing infrastructure; it does not change any seam:

```
~/dev/cook                     ← M4.3 lands here (pull deletion)
~/dev/cook-modules             ← M4.1 stubs + M4.2 workflow live here
~/dev/cook-rocks-index         ← M4.2 publishes into here; mirrored to GitHub
rocks.usecook.com              ← Cloudflare Pages on the GitHub mirror; auto-rebuilds
nas-arm64 Gitea Actions runner ← M4.2 executes here
```

The runner is Linux ARM64, which is sufficient for `.src.rock` publishing — `luarocks pack` produces a portable tarball, no per-platform compilation. Pre-built binary rocks for macOS/Windows are deferred (see "Out of scope").

## M4.1 — Stub rocks

### File layout (in `~/dev/cook-modules`)

One subdirectory per stub, mirroring the existing `cook_smoke/` shape:

```
cook-modules/
├── cook_smoke/        # already exists (Phase 3 fixture)
├── cook_cpp/
│   ├── cook_cpp.lua
│   ├── cook_cpp-0.0.1-1.rockspec
│   └── README.md
├── cook_rust/         # same shape
├── cook_pnpm/         # same shape
└── cook_ai/           # same shape
```

### Versioning

All four stubs ship as `0.0.1-1`. Patch-zero with rockrev-1 is the minimum legal LuaRocks triple, and the all-zeroes-prefix signals "not real yet" to anyone reading `luarocks search`. Real implementations bump to `0.1.0-1` or higher when their respective project tickets land. The stub version is intentionally not a `0.0.0-x` to avoid clashes with luarocks tooling that occasionally treats `0.0.0` as a sentinel.

### Module body (per stub)

Per-stub Lua module follows the canonical shape:

```lua
local M = {}
M.name = "cook_cpp"
function M.placeholder()
    error("[cook_cpp] SHI-176 Phase 4 stub. Real cook_cpp lands in SHI-133.", 2)
end
return M
```

`error(..., 2)` blames the caller's line — the user's `cpp.placeholder()` invocation surfaces in the stack, not the stub file itself. The other three stubs are byte-identical modulo the module name and the cited SHI ticket. The `cook_rust`, `cook_pnpm`, and `cook_ai` real-implementation tickets are filed by the implementation plan **before** the stubs are authored (see "Risks and mitigations" — *Stub error message rot*); the assigned IDs are then substituted into each stub's error message at authoring time. The spec uses `SHI-<TBD-rust>` / `SHI-<TBD-pnpm>` / `SHI-<TBD-ai>` as notational placeholders for those three tickets.

### Rockspec (per stub)

Mirrors `cook_smoke`'s working contract from the publishing-pipeline memory:

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

The `source.url` uses `git+https://` with a `tag` qualifier and **no `source.dir`** — luarocks resolves `source.dir` relative to the temp parent (where the clone lands), and explicitly setting `dir = "cook_cpp"` causes a path-resolution failure documented in `project_cook_module_publishing.md`'s "Bootstrap gotcha" note. Letting luarocks auto-detect `source.dir = "cook-modules"` and putting the full subpath in `build.modules` is the working contract.

### Per-stub README

One paragraph: *"Stub rock for `<module>`. The real implementation is tracked in SHI-NNN. Calling any function on this rock errors with a pointer to that ticket. Replace this directory's contents when the real-implementation ticket ships."*

### Smoke after publish

For each stub, after M4.2's CI publishes:

```sh
luarocks install cook_cpp --tree /tmp/m4-smoke --server https://rocks.usecook.com
ls /tmp/m4-smoke/share/lua/5.4/cook_cpp.lua    # must exist
lua5.4 -e "require('cook_cpp').placeholder()"  # must error with the SHI-133 message
```

## M4.2 — Publish CI on nas-arm64

### Workflow location and trigger

`cook-modules/.gitea/workflows/publish.yml`. Trigger on tag push only:

```yaml
on:
  push:
    tags:
      - "*-*.*.*-*"
```

The pattern matches `<module>-<MAJOR>.<MINOR>.<PATCH>-<rockrev>` (e.g. `cook_cpp-0.0.1-1`). `cook-rocks-index` itself runs no Actions in this phase — it is a passive target.

### Job steps (in order)

1. **Tag-format guard.** Regex assert `^([a-z_]+)-(\d+\.\d+\.\d+-\d+)$` against `${{ gitea.ref_name }}`. Fail fast with an explicit message if the contributor pushed a malformed tag (e.g. `cook_cpp-1.0` missing the rockrev). This protects the index from mispublished artifacts.
2. **Checkout cook-modules** at the tagged commit.
3. **Parse `MODULE` and `VERSION`** from `${{ gitea.ref_name }}` using the captured groups from step 1.
4. **Pack the rock.** `cd "$MODULE" && luarocks pack "$MODULE-$VERSION.rockspec"`. Produces `$MODULE-$VERSION.src.rock` in `$MODULE/`. Pure tarball; no compilation.
5. **Checkout cook-rocks-index** into a sibling working directory using `GITEA_INDEX_TOKEN` (write scope on `LioraLabs/cook-rocks-index`).
6. **Stage artifacts.** Copy `$MODULE/$MODULE-$VERSION.rockspec` and `$MODULE/$MODULE-$VERSION.src.rock` into the index repo root.
7. **Regenerate manifests.** `cd cook-rocks-index && luarocks-admin make-manifest .`.
8. **Commit + push (idempotent).** `git add manifest manifest-5.x *.rockspec *.src.rock && git diff --quiet --staged || git commit -m "publish: $MODULE-$VERSION" && git push origin main`. Re-running with the same tag is a no-op when artifacts are byte-identical.
9. **Mirror to GitHub.** `git remote add github https://x-access-token:${GITHUB_MIRROR_TOKEN}@github.com/lioralabs/cook-rocks-index.git && git push --force github main`. Snapshot-branch mirror model per `project_cook_module_publishing.md`. Cloudflare Pages picks up the push within minutes.

### Tooling on the runner

`apt install luarocks` once at runner-image build time (or as a workflow step on a fresh runner). The publish CI deliberately does **not** depend on `~/.cook/bin/luarocks`. Cook is the *consumer* of these rocks downstream; making the publish pipeline depend on cook's bundled luarocks creates a bootstrap circularity (a fresh runner couldn't publish without first installing a working cook tree).

### Secrets

Two Gitea Actions secrets, scoped at the `LioraLabs` org level:

- `GITEA_INDEX_TOKEN` — write scope on `LioraLabs/cook-rocks-index` (Gitea API token).
- `GITHUB_MIRROR_TOKEN` — fine-grained GitHub PAT, `contents:write` scope on the GitHub mirror repo only. PAT expiry recorded in `project_cook_module_publishing.md`'s "How to apply" section once issued.

### Idempotency contract

Re-running with an already-published tag must be a no-op (no double commit, no force-push churn). The `git diff --quiet --staged` guard in step 8 prevents empty commits. The GitHub mirror push in step 9 is `--force` and benign on no-op (Cloudflare Pages re-deploy on a no-op push is fine).

### Cross-repo seam (only one)

The workflow lives entirely in cook-modules. It writes to cook-rocks-index over the wire (Gitea HTTPS push). cook-rocks-index has no Actions of its own. This means a publish failure between steps 5–9 leaves the index in a clean state (no half-committed manifest); contributors retry by re-tagging or by manually completing the steps that ran.

## M4.3 — Delete `cook pull`

### Files removed entirely

- `cli/crates/cook-cli/src/pull/archive.rs` (326 LoC)
- `cli/crates/cook-cli/src/pull/args.rs` (178 LoC)
- `cli/crates/cook-cli/src/pull/config.rs` (186 LoC)
- `cli/crates/cook-cli/src/pull/errors.rs` (167 LoC)
- `cli/crates/cook-cli/src/pull/fetch.rs` (97 LoC)
- `cli/crates/cook-cli/src/pull/install.rs` (300 LoC)
- `cli/crates/cook-cli/src/pull/mod.rs` (203 LoC)
- `cli/crates/cook-cli/src/pull/prompt.rs` (179 LoC)
- `cli/crates/cook-cli/src/pull/trust.rs` (317 LoC)

Total: 1953 LoC. The whole directory is removed via `git rm -r cli/crates/cook-cli/src/pull/`.

### Files edited

- **`cli/crates/cook-cli/src/cli.rs`** (or wherever the clap subcommand surface lives — verify at implementation time): remove the `Pull(...)` enum variant and the matching dispatch arm. The clap default for an unknown subcommand emits an informative error; no fallback hint is added.
- **`cli/crates/cook-cli/src/lib.rs`**: remove the `mod pull;` declaration.
- **`cli/crates/cook-cli/Cargo.toml`**: audit pull-only dependencies. Candidates likely to drop: `tar`, `flate2`, `sha2` (used for archive extract + integrity), `dialoguer` (the prompt module's interactive consent UI). Verify each candidate is unreferenced after the cut by `cargo build` failing on `cargo check` if removed prematurely; keep any that `cook modules` already reuses.
- **`README.md`** (project root): replace any `cook pull` examples with `cook modules install` examples. Audit by `rg -n 'cook pull' README.md`.
- **`CONTRIBUTING.md`**: same audit, same replacement.
- **`docs/superpowers/specs/2026-05-07-cook-pull-registry-design.md`**: prepend `Status: Superseded by 2026-05-08-luarocks-modules-design.md (SHI-176 Phase 3+4)` to the front-matter. Do not delete — design history is not pruned.

### Tests and fixtures audited

At implementation time, run `rg -l 'cook pull|::pull::|cook_pull' cli/ standard/ docs/ scripts/` and reconcile each match. Expected hits:

- Integration tests under `cli/crates/cook-cli/tests/` invoking `cook pull` — remove.
- Conformance fixtures referencing pull (none expected; pull was never a Standard concept) — verify and remove if found.
- Inline doc comments in non-pull source — rewrite or remove.

After the cut: `cargo build`, `cargo test --workspace`, and `cook check` (the umbrella chore) must all pass green with no `pull/` directory present and no `cook pull` CLI route.

### Standard impact

None. `cook pull` was never a Standard concept; the conformance harness stays green by construction. `feedback_spec_first_no_bypass.md` does not bind this slice (no language-surface paths touched).

## Wave / dependency map

```
Wave 1 (parallel worktrees, branched from main):
  ├── M4.3 — Delete cook pull              [worktree: shi176-m4-3]
  └── M4.1 — Author 4 stub rockspecs+luas  [worktree: shi176-m4-1]
       (no merge dep; lands in cook-modules, separate repo)

Wave 2 (depends on M4.1's tags reaching cook-modules main):
  └── M4.2 — Publish CI                    [worktree: shi176-m4-2]
       Verification = CI publishes cook_cpp-0.0.1-1 end-to-end,
       and the other three follow without rework.
```

M4.3 is fully independent of M4.1 and M4.2 — different repo, no shared file. M4.1 stubs can be hand-published via the documented manual flow (see `project_cook_module_publishing.md`'s "Publish flow (manual v1)") if M4.2 stalls; this preserves the option of slipping M4.2 to a follow-up without blocking the unblock-SHI-133 promise.

The cook-modules and cook (this repo) workstreams have no merge conflicts by construction — they touch disjoint repos.

## M4.0 acceptance gate

A single end-to-end check, run after all three slices have landed:

1. **Tag and publish cook_cpp.** From `~/dev/cook-modules`: `git tag cook_cpp-0.0.1-1 && git push origin cook_cpp-0.0.1-1`. Workflow runs on nas-arm64; artifacts appear in `~/dev/cook-rocks-index` (Gitea), the GitHub mirror, and `rocks.usecook.com` (poll for up to 60s for Cloudflare Pages refresh).
2. **Install from the live index.** `luarocks install cook_cpp --tree /tmp/m4 --server https://rocks.usecook.com` succeeds; `/tmp/m4/share/lua/5.4/cook_cpp.lua` exists.
3. **Cook modules path.** Fixture project with `cook.toml` containing `[modules]` table `cook_cpp = "*"`, then `cook modules install` — succeeds; `cook.lock` lists `cook_cpp 0.0.1-1` with the `rocks.usecook.com` source URL and a populated integrity field.
4. **Cookfile binding.** Fixture Cookfile with `use cook_cpp` (or `use cook_cpp as cpp`); calling `cpp.placeholder()` from a chore raises an error whose message contains the SHI-133 reference, and whose stack frame blames the Cookfile line, not the stub file.
5. **Cook builds clean.** From `~/dev/cook`: `cargo build`, `cargo test --workspace`, and `cook check` all green; `cli/crates/cook-cli/src/pull/` does not exist; `cargo build` does not fail on missing-dependency errors from the dropped `Cargo.toml` deps.
6. **No `cook pull` lingerers.** `rg 'cook pull' cli/ standard/ docs/ scripts/ README.md CONTRIBUTING.md` returns matches **only** in `docs/superpowers/specs/2026-05-07-cook-pull-registry-design.md` (the superseded design), and that file's front-matter contains the supersession marker.
7. **Repeat for the three siblings.** Steps 1–4 for `cook_rust`, `cook_pnpm`, `cook_ai` — all four stubs publishable + installable + their `placeholder()` raises an informative error.

When all seven pass, M4.0 is green and Phase 4 merges to `main` on the cook repo and to `main` on cook-modules.

## Risks and mitigations

- **Cloudflare Pages caching window.** GitHub mirror force-push triggers a Pages rebuild within ~minutes. The acceptance gate's `luarocks install` smoke might race the rebuild. *Mitigation:* poll for the new manifest line up to 60s before declaring failure (the M4.0 step 1 timeout). 60s is roughly 3× the observed Cloudflare Pages deploy time on existing pushes.
- **Tag malformed by contributor.** A push of `cook_cpp-1.0` (missing rockrev) or `cookcpp-0.0.1-1` (missing underscore) would silently mispublish without a guard. *Mitigation:* regex guard at workflow start, fail fast with an explicit message naming the expected format `<module>-<MAJOR>.<MINOR>.<PATCH>-<rockrev>`.
- **`luarocks-admin make-manifest` non-determinism.** The tool occasionally produces zip variants with timestamp churn. *Mitigation:* commit only the unzipped `manifest` and `manifest-5.x` files; gitignore `manifest-*.zip` in cook-rocks-index. The existing index already follows this pattern.
- **PAT expiry on the GitHub mirror push.** Fine-grained GitHub PATs expire (max 1 year). Silent expiry breaks publish without warning. *Mitigation:* record the issuance and expected-renewal dates in `project_cook_module_publishing.md`'s "How to apply" section. A pre-expiry warning is later work; v1 accepts the calendar item.
- **User muscle-memory on `cook pull`.** Clean burn means `cook pull cpp` post-PR fails with clap's "unknown subcommand" message, not a friendly redirect. *Mitigation considered but excluded:* a clap-level fallback that suggests `cook modules install`. Excluded because (a) clap's default is already informative, and (b) the user is the only `cook pull` consumer and acknowledged the gap during brainstorming. Re-evaluate if a third-party adopts cook before the redirect's marginal value rises above the cost of carrying a backwards-compat shim.
- **`Cargo.toml` dependency over-prune.** Removing `tar`/`flate2`/`sha2`/`dialoguer` is correct only if no other module reuses them. *Mitigation:* drive removals via `cargo build` failures — try removing each, build, restore if anything else depends. The build is the source of truth, not grep.
- **Stub error message rot.** The stubs hard-code SHI ticket numbers. If a ticket is renamed or moved, the message lies. *Mitigation:* the implementation plan files the three "real implementation" sub-tickets (cook_rust, cook_pnpm, cook_ai) before the stubs are authored, so the IDs in the error messages are stable from the moment the stubs publish.

## What this enables

- **SHI-133 unblocks.** With `cook_cpp` available as a real (if stub) rock on `rocks.usecook.com`, SHI-133 ("M0 — Promote cpp.lua to a blessed Standard module") can begin the real cook_cpp work — replacing the stub by tagging `cook_cpp-0.1.0-1` and letting M4.2's CI publish the upgrade. The stub's clean shape is the upgrade contract.
- **Publish pipeline at four-rock scale.** Phase 4 forces the publish CI to handle four rocks within days of each other, surfacing latent bugs (tag-format edge cases, manifest churn, PAT auth) that a single-rock pipeline could hide. The fifth rock (when SHI-133 ships) inherits a battle-tested pipeline.
- **Pull deletion frees ~2000 LoC and one CLI subcommand.** The cook binary's surface area shrinks; cook-modules is the singular module-acquisition path; the Doom 3 milestone (SHI-132) inherits a smaller, more coherent cook to extend.
- **Phase 5 (Windows) is the only remaining SHI-176 phase.** Phases 1–4 cover the Unix story end-to-end. Windows packaging (`lua54.dll`, MSI, MSVC docs, curated prebuilt binary rocks for the v1 module set) is the next and final SHI-176 phase. After that, SHI-176 closes.

## References

- Parent architectural spec: `docs/superpowers/specs/2026-05-08-luarocks-modules-design.md`
- Phase 1–3 decomposition: `docs/superpowers/specs/2026-05-09-luarocks-modules-decomposition-design.md`
- Phase 3 operational design: `docs/superpowers/specs/2026-05-10-luarocks-phase-3-design.md`
- Linear epic: SHI-176; downstream blocked epic: SHI-132 (Doom 3); first cook_cpp follow-up: SHI-133
- Publishing pipeline memory: `project_cook_module_publishing.md`
- Spec-first rule (informational; not binding on this phase): `feedback_spec_first_no_bypass.md`
- Linear project routing: `reference_linear_projects.md`
