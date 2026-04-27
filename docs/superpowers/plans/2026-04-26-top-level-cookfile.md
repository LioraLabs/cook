# Top-level Cookfile Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a top-level `Cookfile` at `/home/alex/dev/cook/Cookfile` that captures the developer workflows from the design at `docs/superpowers/specs/2026-04-26-top-level-cookfile-design.md`. Eleven recipes covering verification, mechanical rituals, self-install, and an umbrella check.

**Architecture:** Two commits. The first normalizes the cli/crates/cook-lang/README.md claim phrasing from `claims to implement **...**` to `claims **...**` so the `bump-claim` recipe's sed pattern works uniformly across all four claim mirrors, then creates the Cookfile, then smoke-tests every non-destructive recipe. The second adds the bootstrap line to the project root README.md.

**Tech Stack:** Cookfile language (recipe headers, indented shell bodies via `@`-prefix interactive shell, recipe-name dep lists, `--set KEY=VALUE` overrides surfacing as env vars). Underlying tools: cargo, pnpm, sed, git, the existing `standard/scripts/check-conformance-against-tag.sh`.

---

## Working directory and prerequisites

This plan executes on `main` of `/home/alex/dev/cook` (no worktree — the changes are small and reversible).

The smoke tests use the workspace-built `cook` binary at `cli/target/debug/cook` so the plan does not require `cook` to be on PATH. Bootstrap-via-`cargo install` is documented in Task 2's README addition for end-users; the plan itself does not exercise install.

---

## File structure

| File | Status | Responsibility | Tasks |
|------|--------|---------------|-------|
| `cli/crates/cook-lang/README.md` | Modify | Normalize the claim line phrasing so `bump-claim`'s single sed pattern matches all four claim mirrors. | 1 |
| `Cookfile` (repo root) | Create | Top-level Cookfile with 11 recipes: build, test, conformance, version, standard-build, standard-lint, against-tag, install, bump-claim, retag, check. | 1 |
| `README.md` (repo root) | Modify | Add a bootstrap line below the existing v0.2 claim, documenting `cargo install --locked --path cli/crates/cook-cli` for first-time setup and `cook check` as the smoke test. | 2 |

No deletions. No new tests beyond manual smoke tests.

---

## Task 1: Normalize claim phrasing, create Cookfile, smoke-test

**Files:**
- Modify: `cli/crates/cook-lang/README.md`
- Create: `Cookfile` (at repo root)

This task normalizes one line of `cli/crates/cook-lang/README.md`, creates the Cookfile, and smoke-tests every recipe except `install` (system side effect — overwrites `~/.cargo/bin/cook`) and `retag` (destructive — would force-move `cs-standard/v0.2` away from its current correct position at `eb06cee`).

- [ ] **Step 1.1: Normalize cli/crates/cook-lang/README.md claim phrasing**

The current file contains:

```markdown
This crate claims to implement **Cook Standard v0.2**.
```

Change to:

```markdown
This crate claims **Cook Standard v0.2**.
```

(Drop the words "to implement". The new phrasing matches the project root README.md and CONFORMANCE.md, all of which now use `claims **Cook Standard vX.Y**` so a single sed pattern works on all four files.)

The change is exactly one line, around line 7 of `cli/crates/cook-lang/README.md`. Verify with:

```bash
grep -n "claims" /home/alex/dev/cook/cli/crates/cook-lang/README.md
```

Expected after edit: one line reading `This crate claims **Cook Standard v0.2**.`

- [ ] **Step 1.2: Create the top-level Cookfile**

Create `/home/alex/dev/cook/Cookfile` with this exact content:

```
# Cookfile for the Cook project itself.
#
# Common invocations:
#   cook check        — pre-commit smoke test (build + test + conformance + standard build/lint).
#   cook bump-claim   — after a Standard cut, mirror the version into cook-lang's claim sites.
#   cook retag        — force-move cs-standard/v$VERSION to HEAD (when the parser dump format changed).
#   cook against-tag  — verify the parser still satisfies a previously-cut Standard version.
#   cook install      — cargo install the binary in place.
#
# Recipes that take a version (bump-claim, retag, against-tag) read VERSION from
#   cook <recipe> --set VERSION=X.Y
# and default to the current contents of standard/VERSION.
#
# First-time install of `cook` is documented in README.md.

recipe build
    @cargo build --manifest-path cli/Cargo.toml
end

recipe test
    @cargo test --manifest-path cli/Cargo.toml
end

recipe conformance
    @cargo test --manifest-path cli/Cargo.toml -p cook-lang --test conformance
end

recipe version
    @cargo build --manifest-path cli/Cargo.toml --bin cook
    @cli/target/debug/cook --version
end

recipe standard-build
    @cd standard && pnpm build
end

recipe standard-lint
    @cd standard && pnpm lint:keywords
end

recipe against-tag
    @V="${VERSION:-$(cat standard/VERSION)}" && standard/scripts/check-conformance-against-tag.sh "v$V"
end

recipe install
    @cargo install --locked --path cli/crates/cook-cli
end

recipe bump-claim
    @V="${VERSION:-$(cat standard/VERSION)}" && sed -i "s|pub const COOK_STANDARD_VERSION: &str = \"[^\"]*\"|pub const COOK_STANDARD_VERSION: \&str = \"$V\"|" cli/crates/cook-lang/src/lib.rs && for f in cli/crates/cook-lang/README.md cli/crates/cook-lang/CONFORMANCE.md README.md; do sed -i "s|claims \*\*Cook Standard v[0-9.]*\*\*|claims **Cook Standard v$V**|" "$f"; done
end

recipe retag
    @V="${VERSION:-$(cat standard/VERSION)}" && git tag --force "cs-standard/v$V" HEAD
end

recipe check: build test conformance standard-build standard-lint
end
```

Notes on the file:
- Every recipe body line is `@`-prefixed for streaming output (cargo, pnpm, shell scripts produce useful progress output that cook's default capture would buffer).
- The `bump-claim` line is long because each cook recipe-body line is one shell invocation; multi-line scripts have to be chained on one line with `&&` or split into separate lines that re-resolve `VERSION`. One-line-with-loop is the cleanest of those options.
- The `check` recipe has an empty body; the deps list does the work.
- The `install` recipe's `--locked` flag pins to `cli/Cargo.lock` for reproducibility.

- [ ] **Step 1.3: Build the cook binary used for smoke tests**

```bash
cd /home/alex/dev/cook && cargo build --manifest-path cli/Cargo.toml --bin cook
```

Expected: clean build (only the preexisting `expand_template_to_lua`, `ColorMode`, `resolve` warnings).

- [ ] **Step 1.4: Smoke-test the Cookfile parses**

Invoke `cook menu` (lists recipes) and confirm all 11 recipe names appear:

```bash
cd /home/alex/dev/cook && ./cli/target/debug/cook menu 2>&1 | tee /tmp/cook-menu.txt
```

Expected: output lists `build`, `test`, `conformance`, `version`, `standard-build`, `standard-lint`, `against-tag`, `install`, `bump-claim`, `retag`, `check` (in some order). If the parser rejects any recipe, the output will include an error — fix the offending recipe before continuing.

- [ ] **Step 1.5: Smoke-test `cook version`**

```bash
cd /home/alex/dev/cook && ./cli/target/debug/cook version 2>&1 | tail -5
```

Expected: ends with `cook 0.1.0 (Cook Standard v0.2)`.

- [ ] **Step 1.6: Smoke-test `cook conformance`**

```bash
cd /home/alex/dev/cook && ./cli/target/debug/cook conformance 2>&1 | tail -10
```

Expected: ends with `test result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out`.

- [ ] **Step 1.7: Smoke-test `cook against-tag` (defaults to v0.2)**

```bash
cd /home/alex/dev/cook && ./cli/target/debug/cook against-tag 2>&1 | tail -10
```

Expected: ends with `test result: ok. 3 passed; 0 failed`.

- [ ] **Step 1.8: Smoke-test `cook bump-claim` is idempotent at current VERSION**

The current VERSION in `standard/VERSION` is `0.2`. All four mirror files already contain `**Cook Standard v0.2**`. Running `cook bump-claim` should produce no diff.

```bash
cd /home/alex/dev/cook && ./cli/target/debug/cook bump-claim 2>&1 | tail -5
git -C /home/alex/dev/cook diff --quiet && echo "IDEMPOTENT: no diff" || git -C /home/alex/dev/cook diff
```

Expected: `IDEMPOTENT: no diff`. If a diff appears, the sed pattern is matching/replacing something unintended — inspect the diff and adjust the sed expressions in the Cookfile before proceeding.

- [ ] **Step 1.9: Smoke-test `cook check`**

```bash
cd /home/alex/dev/cook && ./cli/target/debug/cook check 2>&1 | tail -10
```

This runs all 5 deps (build, test, conformance, standard-build, standard-lint). It will take 30–60 seconds. Expected: completes successfully. The exact output format depends on cook's UI; what matters is the exit code is 0. Confirm:

```bash
echo "exit=$?"
```

Expected: `exit=0`.

If `pnpm` is not on PATH, `standard-build` and `standard-lint` will fail. In that case skip those recipes for smoke purposes by running only the cargo deps:

```bash
cd /home/alex/dev/cook && ./cli/target/debug/cook build && ./cli/target/debug/cook test && ./cli/target/debug/cook conformance
```

…and note the partial smoke in the commit message.

- [ ] **Step 1.10: Skip-list — recipes NOT smoke-tested**

These recipes are not exercised by this plan:

- `install` — has system side effects (overwrites `~/.cargo/bin/cook`). Smoke-test by inspection: read the recipe body, confirm it shells `cargo install --locked --path cli/crates/cook-cli`. Run manually after the Cookfile lands if desired.
- `retag` — destructive: would force-move `cs-standard/v0.2` away from its current correct position at `eb06cee`. Smoke-test by inspection: read the recipe body, confirm it shells `git tag --force "cs-standard/v$V" HEAD`. Will be exercised on the next real cut.

- [ ] **Step 1.11: Commit**

The commit touches `Cookfile` (new file, project root) and `cli/crates/cook-lang/README.md` (claim phrasing normalization). Neither path is "language surface" by the pre-commit hook's allowlist (`cli/crates/cook-lang/*` is, but README.md within it is documentation). The hook may still warn for the cli/crates/cook-lang/README.md edit; use `COOK_STANDARD_BYPASS=1` if needed:

```bash
git -C /home/alex/dev/cook add Cookfile cli/crates/cook-lang/README.md
COOK_STANDARD_BYPASS=1 git -C /home/alex/dev/cook commit -m "$(cat <<'EOF'
feat: top-level Cookfile capturing project workflows

Eleven recipes:
- Verification: build, test, conformance, version, standard-build,
  standard-lint, against-tag.
- Rituals: bump-claim (mirror version into cook-lang's 4 claim sites),
  retag (force-move cs-standard/vX.Y to HEAD).
- Self-install: install (cargo install --locked).
- Umbrella: check (deps-only, runs the 5 verification recipes).

Recipes taking a version read VERSION from --set, defaulting to
standard/VERSION. cli/crates/cook-lang/README.md normalized to use
the same "claims **Cook Standard vX.Y**" phrasing as the other three
mirrors, so bump-claim's single sed pattern works on all four files.
EOF
)"
```

Expected: commit lands. Verify with:

```bash
git -C /home/alex/dev/cook log --oneline -2
```

## Task 2: README bootstrap line

**Files:**
- Modify: `README.md` (repo root)

The project root `README.md` currently has:

```markdown
The reference implementation in [`cli/crates/cook-lang/`](cli/crates/cook-lang/) claims **Cook Standard v0.2**.
```

Add one line documenting first-time bootstrap and the smoke-test invocation.

- [ ] **Step 2.1: Add the bootstrap line**

Use the Edit tool to insert immediately after the existing claim line. The new content is one paragraph:

```markdown

First-time setup: `cargo install --locked --path cli/crates/cook-cli`. After that, `cook install` updates in place; `cook check` runs the full verification suite.
```

(Note the leading blank line — this paragraph is separated from the preceding claim line by an empty line so it renders as its own paragraph in markdown.)

The full root README.md after this edit should be approximately:

```markdown
# Cook

A modern build system with Lua. The Cookfile language is defined by the [Cook Standard](standard/); see [`CONTRIBUTING.md`](CONTRIBUTING.md) for development guidelines.

The reference implementation in [`cli/crates/cook-lang/`](cli/crates/cook-lang/) claims **Cook Standard v0.2**.

First-time setup: `cargo install --locked --path cli/crates/cook-cli`. After that, `cook install` updates in place; `cook check` runs the full verification suite.
```

- [ ] **Step 2.2: Commit**

```bash
git -C /home/alex/dev/cook add README.md
git -C /home/alex/dev/cook commit -m "docs(readme): document first-time bootstrap and cook check"
git -C /home/alex/dev/cook log --oneline -3
```

The pre-commit hook does not flag root `README.md` (not in the language-surface allowlist; not `D-changes.mdx`). No bypass needed.

---

## Final verification

After both tasks complete:

```bash
cd /home/alex/dev/cook && ./cli/target/debug/cook check 2>&1 | tail -5
cd /home/alex/dev/cook && ./cli/target/debug/cook version 2>&1 | tail -1
cd /home/alex/dev/cook && ./cli/target/debug/cook against-tag 2>&1 | tail -3
cd /home/alex/dev/cook && git diff main..HEAD --stat
```

Expected:
- `cook check` exits 0 (all 5 verification recipes pass).
- `cook version` prints `cook 0.1.0 (Cook Standard v0.2)`.
- `cook against-tag` runs the conformance harness against `cs-standard/v0.2` and reports 3/3 pass.
- `git diff` shows three files touched across two commits: `Cookfile` (new), `cli/crates/cook-lang/README.md`, `README.md`.

## Self-review notes

- **Spec coverage.** Spec §3 (recipe inventory) maps to Steps 1.2 (Cookfile creation) and 1.4–1.9 (smoke tests of each recipe). Spec §4 (argument convention) is captured in the recipe bodies for `against-tag`, `bump-claim`, `retag`. Spec §5 (style choices) is captured in the Cookfile content (flat, no config blocks, no `use` modules, `@`-prefix freely, no `cook "<out>" using "<cmd>"` build steps, top-of-file comment block). Spec §6 (README update) is Task 2. Spec §7 (verification) is the Final verification block above.
- **No placeholders.** Every step contains the exact command or file content needed.
- **Type/name consistency.** Recipe names appear identically in the Cookfile, the smoke tests, the dep list of `check`, and the spec inventory. The `VERSION` env var is consistent across `against-tag`, `bump-claim`, `retag`.
- **Out of scope (per spec §2 and §8).** No `cut` recipe. No Windows portability. No CI integration. The phrasing normalization in Step 1.1 is in scope because `bump-claim`'s sed needs uniformity to be a single pattern; it is the minimum coupling change.
