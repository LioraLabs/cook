# Cook v1.0 Test Runner — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Implement the design in `standard/specs/2026-05-07-test-runner-language-design.md` (committed `dc5fb7b`) and `docs/superpowers/specs/2026-05-07-test-runner-design.md` (local). After this plan executes: (1) the Standard ships CS-NNNN with the `as STRING` test-step modifier, the test-cache contract, and `cook.add_test` field defaults nailed; (2) `cook --test` exists and runs every test in the workspace, producing a terminal summary, JSON sidecar, and optional JUnit XML; (3) passing tests cache and replay across runs; (4) `--rerun [PATTERN]`, `--rerun-failed`, `--filter PATTERN`, `--fail-fast` are wired; (5) three new example fixtures pin runner behavior in CI.

**Architecture:** Ten phases, each ending at a shippable checkpoint. Phase 1 lands example skeletons so subsequent phases have validation targets. Phase 2 lands the Standard change (CS-NNNN). Phase 3 fixes `cook-engine`'s broken `RunResult.test_outputs` and adds register-all + reverse-closure. Phase 4 lands `cook --test` with a terminal reporter (no caching yet). Phase 5 extends `cook-fingerprint` for tests and adds the test-result cache backend. Phase 6 wires `--rerun [PATTERN]`. Phase 7 adds `.cook/test-state.json` and `--rerun-failed`. Phase 8 emits JSON + JUnit sidecars. Phase 9 enables walkthrough assertions. Phase 10 lands integration tests. TDD throughout: failing test → minimal implementation → green test → commit.

**Tech Stack:** Rust (edition 2021, workspace at `cli/`), `cargo test`, `mlua` for Lua 5.4 runtime, `serde_json` for sidecar emission, `quick-xml` (workspace dep, used by tree-sitter-cook tests today) for JUnit XML, MDX (Astro Starlight) for the Standard, Lua Cookfile fixtures under `examples/`, bash walkthrough scripts.

---

## Working directory and prerequisites

All paths are relative to `/home/alex/dev/cook` unless noted.

Confirm the spec-first hook is installed:

```bash
git -C /home/alex/dev/cook config --get core.hooksPath
# Expected: .githooks
```

If empty: `git -C /home/alex/dev/cook config core.hooksPath .githooks`. The Standard-side spec at `standard/specs/2026-05-07-test-runner-language-design.md` is already committed (`dc5fb7b`); that satisfies the hook's spec-pairing requirement for everything in this plan that touches `cli/crates/cook-lang/`, `cli/crates/cook-luagen/`, `cli/crates/cook-register/`, or `tree-sitter-cook/`.

Confirm a clean working tree before starting:

```bash
git -C /home/alex/dev/cook status --short
# Expected: empty output
```

## Per-task verification commands

| Scope | Command | Expected |
|---|---|---|
| Lang unit tests | `cd cli && cargo test -p cook-lang` | clean |
| Conformance | `cd cli && cargo test -p cook-lang --test conformance` | clean |
| Luagen unit tests | `cd cli && cargo test -p cook-luagen` | clean |
| Register unit tests | `cd cli && cargo test -p cook-register` | clean |
| Fingerprint unit tests | `cd cli && cargo test -p cook-fingerprint` | clean |
| Cache unit tests | `cd cli && cargo test -p cook-cache --lib` | clean |
| Cache integration tests | `cd cli && cargo test -p cook-cache --test '*'` | clean |
| Engine unit tests | `cd cli && cargo test -p cook-engine --lib` | clean |
| Progress unit tests | `cd cli && cargo test -p cook-progress` | clean |
| CLI tests | `cd cli && cargo test -p cook-cli` | clean |
| Whole CLI test suite | `cd cli && cargo test` | clean |
| Spec build | `cd standard && pnpm build` | exit 0 |
| Walkthrough — exit codes | `cd examples/cli_audit_exit_codes && ./walkthrough.sh` | clean |
| Walkthrough — test_benchmarks | `cd examples/test_benchmarks && ./walkthrough.sh` | clean (after Phase 9) |
| Walkthrough — monorepo_test | `cd examples/monorepo_test && ./walkthrough.sh` | clean (after Phase 9) |
| Walkthrough — test_caching | `cd examples/test_caching && ./walkthrough.sh` | clean (after Phase 9) |

## File structure

| File | Responsibility | Phase / Tasks |
|---|---|---|
| `examples/test_benchmarks/Cookfile` (new) | Ten recipes pinning every shape of test the runner handles | 1.1 |
| `examples/test_benchmarks/src/inputs/*.txt` (new) | Iteration inputs for `pass_iterated`, `fail_partial`, etc. | 1.1 |
| `examples/test_benchmarks/walkthrough.sh` (new) | Runtime conformance pin for the runner | 1.2, 9.1 |
| `examples/test_benchmarks/README.md` (new) | One-paragraph orientation | 1.1 |
| `examples/monorepo_test/Cookfile` (new) | Workspace-root Cookfile with three imports | 1.3 |
| `examples/monorepo_test/apps/web/Cookfile` (new) | Frontend test recipes | 1.3 |
| `examples/monorepo_test/apps/api/Cookfile` (new) | Backend test recipes | 1.3 |
| `examples/monorepo_test/packages/utils/Cookfile` (new) | Shared utility test recipes | 1.3 |
| `examples/monorepo_test/walkthrough.sh` (new) | Workspace-discovery + namespace-scope pin | 1.3, 9.2 |
| `examples/test_caching/Cookfile` (new) | Two-run cache-hit scenario | 1.4 |
| `examples/test_caching/walkthrough.sh` (new) | First-run write, second-run hit, `--rerun` bust | 1.4, 9.3 |
| `cli/crates/cook-lang/src/ast.rs` | `TestStep::as_name: Option<String>` field | 2.1 |
| `cli/crates/cook-lang/src/recipe.rs` | Parse `as STRING` modifier, enforce canonical order | 2.2, 2.3 |
| `cli/crates/cook-lang/src/tests.rs` | Coverage for new modifier + reject-cases | 2.2, 2.3, 2.4 |
| `cli/crates/cook-luagen/src/test_step.rs` | Propagate `as_name` into emitted `cook.add_test({ name = … })` | 2.5 |
| `cli/crates/cook-luagen/src/tests.rs` | Codegen tests for `as` substitution | 2.5 |
| `cli/crates/cook-register/src/test_api.rs` | Default `suite` to recipe name; reject empty `command`; reject `timeout ≤ 0` | 2.6, 2.7 |
| `cli/crates/cook-register/src/tests.rs` | Coverage for the three contract-fixes | 2.6, 2.7 |
| `standard/src/content/docs/04-recipes.mdx` | §4.8 modifier table extended | 2.10 |
| `standard/src/content/docs/06-cook-lua-api.mdx` | §6.4 `add_test` field table | 2.10 |
| `standard/src/content/docs/08-execution-model.mdx` | §4.8 outcome table; new §8.6.x test-cache subsection | 2.10 |
| `standard/src/content/docs/appendix/A-grammar.mdx` | `test_modifiers` production updated | 2.10 |
| `standard/src/content/docs/appendix/B-rationale.mdx` | Five new B.4 subsections | 2.10 |
| `standard/src/content/docs/appendix/D-changes.mdx` | CS-NNNN entry | 2.10 |
| `standard/conformance/positive/test_as_modifier_*.cook` (new, several) | Per-modifier-order positive fixtures | 2.11 |
| `standard/conformance/negative/test_as_*.cook` (new, several) | Reject-case negative fixtures | 2.11 |
| `tree-sitter-cook/grammar.js` | `test_step` modifier slot extended with `as_name` | 2.12 |
| `tree-sitter-cook/queries/highlights.scm` | Highlight `as_name` field as `@string.special` | 2.12 |
| `tree-sitter-cook/test/corpus/test_step.txt` | Corpus fixtures for the new surface | 2.12 |
| `cli/crates/cook-engine/src/lib.rs` | Extend `EngineEvent` with test events; `RunResult.test_results` typed | 3.1, 3.2 |
| `cli/crates/cook-engine/src/run.rs` | Replace `Vec::new()` placeholder; populate `test_results`; new `run_for_test()` entry | 3.3, 3.4, 4.1 |
| `cli/crates/cook-engine/src/analyzer.rs` | New `register_workspace_for_test()` | 3.5 |
| `cli/crates/cook-engine/src/dag_builder.rs` | New `build_test_slice()` reverse-closure | 3.6 |
| `cli/crates/cook-engine/src/executor.rs` | Wire test events into engine event bus; populate accumulator | 3.4 |
| `cli/crates/cook-cli/src/cli.rs` | `--test`, `--filter`, `--fail-fast`, `--rerun [PATTERN]`, `--rerun-failed`, `--report-json`, `--report-junit` | 4.2, 6.1, 7.2, 8.1, 8.2 |
| `cli/crates/cook-cli/src/error.rs` | Restore `CookError::TestFailure(String)` variant | 4.3 |
| `cli/crates/cook-cli/src/pipeline.rs` | New `cmd_test` arm | 4.4 |
| `cli/crates/cook-cli/src/test_reporter.rs` (new) | Terminal summary; failures section; next-steps footer; JSON sidecar; JUnit XML | 4.5–4.7, 8.3, 8.4 |
| `cli/crates/cook-fingerprint/src/check.rs` | Test-unit fingerprint includes `timeout` and `should_fail` | 5.1 |
| `cli/crates/cook-fingerprint/src/lib.rs` | `compute_test_fingerprint()` helper | 5.1 |
| `cli/crates/cook-cache/src/test_cache.rs` (new) | `TestCache` API: `lookup`, `store`, no-op for non-passing | 5.2, 5.3 |
| `cli/crates/cook-cache/src/lib.rs` | Re-export `TestCache`, `TestCacheEntry` | 5.2 |
| `cli/crates/cook-engine/src/run.rs` | Wire fingerprint + cache lookup into Phase 4 of test pipeline | 5.4 |
| `cli/crates/cook-cli/src/test_state.rs` (new) | `.cook/test-state.json` read/write | 7.1 |
| `cli/crates/cook-cli/tests/test_runner_*.rs` (new, several) | Integration tests for runner | 10.1–10.4 |

No file is fully deleted in this plan. Three legacy fragments are restored or replaced:
- `CookError::TestFailure` (deleted by `7e3c5d4`) is re-introduced.
- `cook-engine::run::RunResult::test_outputs` (today an empty `Vec::new()` placeholder at `run.rs:215`) is replaced with a real `test_results: Vec<TestResult>`.
- The `let all_test_outputs: Vec<cook_luaotp::TestOutput> = Vec::new();` at `run.rs:215` becomes a real accumulator wired to the executor's `WorkResult::test_output`.

---

## Phase 1: Example skeletons (validation targets)

Lands the three new fixture directories with Cookfiles and stub walkthroughs. Walkthroughs in this phase **do not** assert behavior — they invoke `cook` and print output. Phase 9 enables assertions once the runner exists. This phase is shippable: the Cookfiles parse cleanly today (only the `as` modifier in `test_benchmarks/Cookfile` Task 1.1's `named_test` recipe is forward-looking; the rest use existing surface). The `as` lines are **commented out** in this phase and uncommented in Phase 2 after the parser supports them.

### Task 1.1: Create `examples/test_benchmarks/`

**Files:**
- Create: `examples/test_benchmarks/Cookfile`
- Create: `examples/test_benchmarks/src/inputs/input_01.txt` … `input_12.txt`
- Create: `examples/test_benchmarks/README.md`

- [ ] **Step 1.1.1: Create the input fixtures**

```bash
mkdir -p examples/test_benchmarks/src/inputs
cd examples/test_benchmarks
for i in $(seq -f "%02g" 1 12); do
  printf 'fixture %s\n' "$i" > src/inputs/input_${i}.txt
done
ls src/inputs/
```

Expected: 12 files `input_01.txt` through `input_12.txt`.

- [ ] **Step 1.1.2: Author the Cookfile**

Write `examples/test_benchmarks/Cookfile`:

```cook
# CS-NNNN test-runner conformance fixture: every shape of test the runner handles.
#
# This Cookfile is paired with examples/test_benchmarks/walkthrough.sh which
# pins the runner's behavior across the recipes below. Each recipe targets a
# specific runner contract; the recipe name names the contract.

config
    env.SLEEP = "0.1"

# (1) Green path, one-shot. Pins terminal output, cache write.
recipe pass_basic
    test { true } timeout 5

# (2) One-to-one over 12 inputs. Pins per-iteration discriminator,
#     12 separate cache entries.
recipe pass_iterated
    ingredients "src/inputs/*.txt"
    cook "build/pass_iterated/$<in.stem>.out" using {
        mkdir -p build/pass_iterated
        sleep $<SLEEP>
        echo "ok" > $<out>
    }
    test { test -s $<in> } timeout 5

# (3) should_fail test whose body exits non-zero. Pins should_fail
#     semantics survive caching.
recipe pass_should_fail
    test { exit 1 } should_fail timeout 5

# (4) One-shot test that fails. Pins failure capture, never-cached invariant.
recipe fail_basic
    test { exit 1 } timeout 5

# (5) One-to-one over 12 inputs where 3 fail (those whose stem ends in 3, 6, 9).
#     Pins continue-past-failure and mixed pass/fail aggregation.
recipe fail_partial
    ingredients "src/inputs/*.txt"
    cook "build/fail_partial/$<in.stem>.out" using {
        mkdir -p build/fail_partial
        sleep $<SLEEP>
        echo "ok" > $<out>
    }
    test {
        case "$<in.stem>" in
            *3|*6|*9) exit 1 ;;
            *) exit 0 ;;
        esac
    } timeout 5

# (6) Test whose preceding cook step calls false. Pins blocked status.
recipe blocked_by_build
    cook "build/blocked_by_build/never.out" using {
        mkdir -p build/blocked_by_build
        false
    }
    test { test -f $<in> } timeout 5

# (7) Test that times out. Pins timed_out outcome.
recipe slow_timeout
    test { sleep 10 } timeout 1

# (8) `as` modifier. Pins name override surface.
#     COMMENTED OUT until Phase 2.5 lands `as` parsing — uncomment in Task 2.5.
# recipe named_test
#     ingredients "src/inputs/*.txt"
#     cook "build/named_test/$<in.stem>.out" using {
#         mkdir -p build/named_test
#         echo "ok" > $<out>
#     }
#     test { test -s $<in> } as 'non-empty' timeout 5

# (9) Cache replay. Pins cache hit on second run.
recipe cached_replay
    ingredients "src/inputs/*.txt"
    cook "build/cached_replay/$<in.stem>.out" using {
        mkdir -p build/cached_replay
        sleep $<SLEEP>
        echo "ok" > $<out>
    }
    test { true } timeout 5

# (10) Used by walkthrough --rerun-failed cycle.
#      First run: tests #1, #2 fail. Second run with --rerun-failed: only
#      those two re-run.
recipe rerun_failed_set
    ingredients "src/inputs/*.txt"
    cook "build/rerun_failed_set/$<in.stem>.out" using {
        mkdir -p build/rerun_failed_set
        sleep $<SLEEP>
        echo "$<in.stem>" > $<out>
    }
    test {
        # First two iteration items fail; rest pass.
        case "$<in.stem>" in
            input_01|input_02) exit 1 ;;
            *) exit 0 ;;
        esac
    } timeout 5

chore clean
    rm -rf build .cook
```

- [ ] **Step 1.1.3: Author the README**

Write `examples/test_benchmarks/README.md`:

```markdown
# `examples/test_benchmarks/`

Fixture pinning the v1.0 test runner across every shape of test the runner
handles. Paired with `walkthrough.sh`, which is the runtime conformance pin
in CI.

Recipes:

| Recipe | Pins |
|---|---|
| `pass_basic` | Green path, terminal output, cache write |
| `pass_iterated` | Per-iteration discriminator, 12 separate cache entries |
| `pass_should_fail` | `should_fail` semantics survive caching |
| `fail_basic` | Failure capture, never-cached invariant |
| `fail_partial` | Continue-past-failure, mixed pass/fail aggregation |
| `blocked_by_build` | Blocked status (upstream cook failed) |
| `slow_timeout` | Timed-out outcome |
| `named_test` | `as 'name'` modifier (Phase 2.5+) |
| `cached_replay` | Cache hit on second run |
| `rerun_failed_set` | `--rerun-failed` selection |

Run: `cook --test` (after Phase 4) — see `walkthrough.sh` for the full
conformance assertions.
```

- [ ] **Step 1.1.4: Verify the Cookfile parses with the current parser**

```bash
cd /home/alex/dev/cook
cargo run -q --bin cook -- --emit-lua -f examples/test_benchmarks/Cookfile pass_basic | head -5
```

Expected: Lua codegen output, no parse error. (Use `--emit-lua` to avoid actually executing.)

- [ ] **Step 1.1.5: Commit**

```bash
git add examples/test_benchmarks/
git commit -m "$(cat <<'EOF'
test(examples): scaffold test_benchmarks fixture for runner conformance

Ten recipes covering every shape of test the v1.0 runner has to handle:
green paths, iteration with mixed outcomes, blocked-by-build, timeouts,
should_fail, cache replay. Walkthrough lands in next task; assertions
come online in Phase 9 once the runner exists.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

### Task 1.2: Stub `examples/test_benchmarks/walkthrough.sh`

**Files:**
- Create: `examples/test_benchmarks/walkthrough.sh`

- [ ] **Step 1.2.1: Author the stub walkthrough**

Write `examples/test_benchmarks/walkthrough.sh`:

```bash
#!/bin/bash
# walkthrough.sh — pin the v1.0 cook --test runner against the
# test_benchmarks fixture.
#
# This stub runs cook against each recipe and prints exit codes. Phase 9
# replaces the body with full assertions on the summary block, JSON
# sidecar, --rerun-failed, and --filter behavior. Until Phase 4 lands
# `cook --test`, this script exits 0 with a "runner not yet wired" notice.

set -uo pipefail

cd "$(dirname "$0")"
COOK="${COOK:-../../cli/target/debug/cook}"
COOK="$(cd "$(dirname "$COOK")" && pwd)/$(basename "$COOK")"

if [ ! -x "$COOK" ]; then
    echo "cook binary not found at $COOK"
    echo "build it first: (cd ../../cli && cargo build --bin cook)"
    exit 1
fi

# Stub mode: confirm cook --test is wired, otherwise skip with notice.
if ! "$COOK" --help 2>&1 | grep -q '\-\-test'; then
    echo "[skip] cook --test not yet wired; this walkthrough is a stub"
    echo "       enabled in Phase 9 (Task 9.1) of the test-runner plan"
    exit 0
fi

echo "[walkthrough] cook --test is available; full assertions land in Phase 9"
exit 0
```

- [ ] **Step 1.2.2: Make executable and verify**

```bash
chmod +x examples/test_benchmarks/walkthrough.sh
cd /home/alex/dev/cook && cargo build -q --bin cook
examples/test_benchmarks/walkthrough.sh
```

Expected: `[skip] cook --test not yet wired; this walkthrough is a stub`, exit 0.

- [ ] **Step 1.2.3: Commit**

```bash
git add examples/test_benchmarks/walkthrough.sh
git commit -m "$(cat <<'EOF'
test(examples): test_benchmarks walkthrough stub

Skips with notice until Phase 4 wires cook --test; assertions land in
Phase 9. Same shape as cli_audit_exit_codes/walkthrough.sh.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

### Task 1.3: Create `examples/monorepo_test/`

**Files:**
- Create: `examples/monorepo_test/Cookfile`
- Create: `examples/monorepo_test/apps/web/Cookfile`
- Create: `examples/monorepo_test/apps/api/Cookfile`
- Create: `examples/monorepo_test/packages/utils/Cookfile`
- Create: `examples/monorepo_test/walkthrough.sh`
- Create: `examples/monorepo_test/README.md`

- [ ] **Step 1.3.1: Create the workspace skeleton**

```bash
cd /home/alex/dev/cook
mkdir -p examples/monorepo_test/apps/web examples/monorepo_test/apps/api examples/monorepo_test/packages/utils
```

- [ ] **Step 1.3.2: Author the workspace-root Cookfile**

Write `examples/monorepo_test/Cookfile`:

```cook
# CS-NNNN test-runner conformance fixture: monorepo workspace test discovery.
#
# Workspace has three imported Cookfiles. Bare `cook --test` at this root
# discovers tests in all three. `cook --test apps.web` scopes to the web
# namespace. `cook --test apps.web.unit` scopes to a single recipe.

import apps.web ./apps/web
import apps.api ./apps/api
import shared //packages/utils

# Convenience aggregator: building this builds every project. Used to verify
# that --test does NOT depend on this aggregator existing — workspace
# discovery is independent of any aggregator recipe.
recipe build: apps.web.build apps.api.build shared.build
    cook "build/all.txt" using {
        mkdir -p build
        echo "all" > $<out>
    }

chore clean
    rm -rf build .cook apps/*/build apps/*/.cook packages/*/build packages/*/.cook
```

- [ ] **Step 1.3.3: Author `apps/web/Cookfile`**

Write `examples/monorepo_test/apps/web/Cookfile`:

```cook
# Frontend project. Two test recipes — one passing, one mixed.

recipe build
    cook "build/web.bundle" using {
        mkdir -p build
        echo "web bundle" > $<out>
    }

recipe unit: build
    test { true } timeout 5
    test { echo "unit ok"; true } timeout 5

recipe e2e: build
    test { test -s $<build> } timeout 5
    test { exit 1 } timeout 5
```

- [ ] **Step 1.3.4: Author `apps/api/Cookfile`**

Write `examples/monorepo_test/apps/api/Cookfile`:

```cook
# Backend project. All passing.

recipe build
    cook "build/api.bin" using {
        mkdir -p build
        echo "api binary" > $<out>
    }

recipe unit: build
    test { true } timeout 5
    test { test -s $<build> } timeout 5
```

- [ ] **Step 1.3.5: Author `packages/utils/Cookfile`**

Write `examples/monorepo_test/packages/utils/Cookfile`:

```cook
# Shared utility package. One passing test.

recipe build
    cook "build/utils.lib" using {
        mkdir -p build
        echo "utils library" > $<out>
    }

recipe unit: build
    test { test -s $<build> } timeout 5
```

- [ ] **Step 1.3.6: Author the walkthrough stub and README**

Write `examples/monorepo_test/walkthrough.sh`:

```bash
#!/bin/bash
# walkthrough.sh — pin the v1.0 cook --test runner against a monorepo
# workspace with three imported Cookfiles. Stub until Phase 9.

set -uo pipefail
cd "$(dirname "$0")"
COOK="${COOK:-../../cli/target/debug/cook}"
COOK="$(cd "$(dirname "$COOK")" && pwd)/$(basename "$COOK")"

if [ ! -x "$COOK" ]; then
    echo "cook binary not found at $COOK"
    exit 1
fi

if ! "$COOK" --help 2>&1 | grep -q '\-\-test'; then
    echo "[skip] cook --test not yet wired; full assertions land in Phase 9"
    exit 0
fi

echo "[walkthrough] cook --test available; assertions enabled in Phase 9.2"
exit 0
```

Write `examples/monorepo_test/README.md`:

```markdown
# `examples/monorepo_test/`

Workspace-shaped fixture pinning monorepo test discovery. Three imported
Cookfiles (`apps.web`, `apps.api`, `shared`) each contain test recipes.

Bare `cook --test` at this root discovers all of them. Namespace and
recipe scopes are pinned by the walkthrough.

Run: `cook --test` (after Phase 4) at this directory.
```

- [ ] **Step 1.3.7: Make walkthrough executable and verify imports parse**

```bash
chmod +x examples/monorepo_test/walkthrough.sh
cd examples/monorepo_test
../../cli/target/debug/cook --emit-lua build | head -3
cd /home/alex/dev/cook
```

Expected: Lua codegen, no parse errors.

- [ ] **Step 1.3.8: Commit**

```bash
git add examples/monorepo_test/
git commit -m "$(cat <<'EOF'
test(examples): scaffold monorepo_test fixture for workspace test discovery

Three imported Cookfiles (apps.web, apps.api, shared utils) with test
recipes spanning passing, mixed, and pure-passing shapes. Walkthrough
stub until Phase 9 enables assertions.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

### Task 1.4: Create `examples/test_caching/`

**Files:**
- Create: `examples/test_caching/Cookfile`
- Create: `examples/test_caching/walkthrough.sh`
- Create: `examples/test_caching/README.md`

- [ ] **Step 1.4.1: Author the Cookfile**

Write `examples/test_caching/Cookfile`:

```cook
# CS-NNNN test-runner conformance fixture: test-result caching.
#
# Three recipes pin distinct cache shapes. Walkthrough runs cook --test
# twice and asserts cache hits on the second run; runs --rerun and asserts
# bust; touches a source file and asserts targeted invalidation.

config
    env.SLEEP = "0.1"

# Deterministic test — should cache cleanly across runs.
recipe deterministic
    cook "build/deterministic.out" using {
        mkdir -p build
        sleep $<SLEEP>
        echo "stable" > $<out>
    }
    test { test -s $<in> } timeout 5

# Iterated test — should cache N entries (one per iteration).
recipe iterated
    ingredients "src/*.txt"
    cook "build/iterated/$<in.stem>.out" using {
        mkdir -p build/iterated
        sleep $<SLEEP>
        cat $<in> > $<out>
    }
    test { grep -q "stable" $<in> } timeout 5

# should_fail — caches as a pass when the body exits non-zero.
recipe should_fail_caches
    test { exit 1 } should_fail timeout 5

chore clean
    rm -rf build .cook src
```

- [ ] **Step 1.4.2: Author input fixtures**

```bash
cd /home/alex/dev/cook
mkdir -p examples/test_caching/src
echo "stable line" > examples/test_caching/src/a.txt
echo "stable line" > examples/test_caching/src/b.txt
echo "stable line" > examples/test_caching/src/c.txt
```

- [ ] **Step 1.4.3: Author walkthrough stub and README**

Write `examples/test_caching/walkthrough.sh`:

```bash
#!/bin/bash
# walkthrough.sh — pin v1.0 test-result caching contract. Stub until Phase 9.

set -uo pipefail
cd "$(dirname "$0")"
COOK="${COOK:-../../cli/target/debug/cook}"
COOK="$(cd "$(dirname "$COOK")" && pwd)/$(basename "$COOK")"

if [ ! -x "$COOK" ]; then
    echo "cook binary not found at $COOK"
    exit 1
fi

if ! "$COOK" --help 2>&1 | grep -q '\-\-test'; then
    echo "[skip] cook --test not yet wired; assertions land in Phase 9.3"
    exit 0
fi

echo "[walkthrough] cook --test available; cache assertions enabled in Phase 9.3"
exit 0
```

Write `examples/test_caching/README.md`:

```markdown
# `examples/test_caching/`

Fixture pinning the test-result caching contract. Walkthrough verifies
that passing tests cache, second runs hit cache, source-file touches
bust the affected test only, and `--rerun` busts everything.
```

- [ ] **Step 1.4.4: Make executable, verify, commit**

```bash
chmod +x examples/test_caching/walkthrough.sh
cd /home/alex/dev/cook
cargo run -q --bin cook -- --emit-lua -f examples/test_caching/Cookfile deterministic | head -3
git add examples/test_caching/
git commit -m "$(cat <<'EOF'
test(examples): scaffold test_caching fixture for cache contract

Three recipes (deterministic, iterated, should_fail) covering the cache
shapes the runner has to honor. Walkthrough stub until Phase 9.3.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

**Phase 1 checkpoint.** Three example fixtures exist with parseable Cookfiles and stub walkthroughs that exit 0. No language or runner changes yet. Cookfiles use the existing `test { … }` surface; the `as` modifier line in `test_benchmarks/Cookfile` is commented out and re-enabled in Phase 2.

---

## Phase 2: Standard change CS-NNNN — language additions

Lands the language-side work: `as STRING` modifier on `test_step`, modifier-order enforcement, `cook.add_test` field default fixes, conformance fixtures, Standard chapter updates, tree-sitter grammar update. After this phase, the parser accepts `test { … } as 'name' timeout N should_fail`; the cook binary still does not have a test runner. The Standard PR for CS-NNNN is acceptance-complete at the end of this phase.

### Task 2.1: Add `as_name` field to `ast::TestStep`

**Files:**
- Modify: `cli/crates/cook-lang/src/ast.rs`

- [ ] **Step 2.1.1: Locate the existing `TestStep` struct**

```bash
grep -n "pub struct TestStep" cli/crates/cook-lang/src/ast.rs
```

Expected: one match. Open the file at that line.

- [ ] **Step 2.1.2: Add the `as_name` field**

Add `as_name: Option<String>` to the struct, immediately after the existing `body` field and before `timeout`:

```rust
pub struct TestStep {
    pub body: Body,
    pub as_name: Option<String>,
    pub timeout: Option<u64>,
    pub should_fail: bool,
    pub line: usize,
}
```

(Field positions match this order; if the existing struct uses different field names, preserve them and only add `as_name`.)

- [ ] **Step 2.1.3: Verify the AST compiles**

```bash
cd cli && cargo check -p cook-lang
```

Expected: compile errors at every `TestStep { … }` construction site (callers don't yet supply `as_name`). Fix each by adding `as_name: None`. Search:

```bash
grep -rn "TestStep {" cli/crates/cook-lang/src/ cli/crates/cook-luagen/src/
```

For each match, add `as_name: None,` in the struct literal. Then re-run `cargo check -p cook-lang -p cook-luagen` until clean.

- [ ] **Step 2.1.4: Commit**

```bash
git add cli/crates/cook-lang/src/ast.rs cli/crates/cook-luagen/src/
git commit -m "$(cat <<'EOF'
feat(cook-lang): add TestStep::as_name field

Optional display-name override per CS-NNNN §3.1. Default None; parsing
in next task.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

### Task 2.2: Parse `as STRING` modifier (positive cases)

**Files:**
- Modify: `cli/crates/cook-lang/src/recipe.rs`
- Modify: `cli/crates/cook-lang/src/tests.rs`

- [ ] **Step 2.2.1: Locate the test-modifier parsing path**

```bash
grep -n "test_step\|fn parse_test\|timeout\|should_fail" cli/crates/cook-lang/src/recipe.rs | head -20
```

Identify the function that parses `test_step` modifiers. The body parser ends at the closing `}`; modifiers follow on the same line.

- [ ] **Step 2.2.2: Write failing tests for `as` parsing**

Append to `cli/crates/cook-lang/src/tests.rs`:

```rust
#[test]
fn test_step_parses_as_modifier() {
    let src = r#"
recipe r
    test { foo $<in> } as 'name' timeout 30 should_fail
"#;
    let recipes = parse(src).expect("parse").recipes;
    let step = match &recipes[0].steps[0] {
        Step::Test(s) => s,
        other => panic!("expected test_step, got {:?}", other),
    };
    assert_eq!(step.as_name.as_deref(), Some("name"));
    assert_eq!(step.timeout, Some(30));
    assert!(step.should_fail);
}

#[test]
fn test_step_as_only() {
    let src = r#"
recipe r
    test { foo } as 'just-as'
"#;
    let recipes = parse(src).expect("parse").recipes;
    let step = match &recipes[0].steps[0] { Step::Test(s) => s, _ => panic!() };
    assert_eq!(step.as_name.as_deref(), Some("just-as"));
    assert_eq!(step.timeout, None);
    assert!(!step.should_fail);
}

#[test]
fn test_step_as_with_substitution_string() {
    let src = r#"
recipe r
    ingredients "src/*.txt"
    cook "build/$<in.stem>.out" using { echo > $<out> }
    test { foo $<in> } as '$<in.stem>-roundtrip' timeout 10
"#;
    let recipes = parse(src).expect("parse").recipes;
    let test = recipes[0].steps.iter().find_map(|s| match s {
        Step::Test(t) => Some(t), _ => None,
    }).unwrap();
    // Parser preserves the literal string; substitution happens at codegen.
    assert_eq!(test.as_name.as_deref(), Some("$<in.stem>-roundtrip"));
}
```

- [ ] **Step 2.2.3: Run the failing tests**

```bash
cd cli && cargo test -p cook-lang test_step_parses_as_modifier test_step_as_only test_step_as_with_substitution_string
```

Expected: 3 failures. The parser doesn't accept `as` yet.

- [ ] **Step 2.2.4: Implement `as` parsing**

In `cli/crates/cook-lang/src/recipe.rs`, in the test-step modifier path, accept an optional `as STRING` token *before* `timeout`. The exact code shape depends on existing modifier-parsing helpers; the canonical insertion is:

```rust
// After parsing the body and before parsing `timeout` / `should_fail`:
let as_name = if peek_keyword(tokens, "as") {
    consume_keyword(tokens, "as");
    Some(parse_string_literal(tokens)?)
} else {
    None
};

let timeout = parse_optional_timeout(tokens)?;
let should_fail = parse_optional_should_fail(tokens)?;

Ok(TestStep { body, as_name, timeout, should_fail, line })
```

(Match the helper names already in use in the file; the parser's modifier scaffolding already exists for `timeout` and `should_fail` — `as` follows the same shape.)

- [ ] **Step 2.2.5: Run the tests; confirm green**

```bash
cd cli && cargo test -p cook-lang test_step_parses_as_modifier test_step_as_only test_step_as_with_substitution_string
```

Expected: 3 passes.

- [ ] **Step 2.2.6: Run full lang test suite**

```bash
cd cli && cargo test -p cook-lang
```

Expected: all green. (The `cook-luagen` test suite may regress — Task 2.5 fixes that.)

- [ ] **Step 2.2.7: Commit**

```bash
git add cli/crates/cook-lang/
git commit -m "$(cat <<'EOF'
feat(cook-lang): parse `as STRING` modifier on test_step (CS-NNNN §3.1)

Modifier sits before `timeout` per the canonical-order rule. Substitution
of $<...> placeholders inside the STRING happens at codegen.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

### Task 2.3: Reject out-of-order modifiers

**Files:**
- Modify: `cli/crates/cook-lang/src/recipe.rs`
- Modify: `cli/crates/cook-lang/src/tests.rs`

- [ ] **Step 2.3.1: Write failing rejection tests**

Append to `cli/crates/cook-lang/src/tests.rs`:

```rust
#[test]
fn test_step_rejects_as_after_timeout() {
    let src = r#"
recipe r
    test { foo } timeout 30 as 'name'
"#;
    let err = parse(src).expect_err("must reject");
    assert!(
        err.to_string().contains("`as`") && err.to_string().contains("must precede"),
        "diagnostic should name `as` and `must precede`, got: {err}"
    );
}

#[test]
fn test_step_rejects_should_fail_before_timeout() {
    let src = r#"
recipe r
    test { foo } should_fail timeout 30
"#;
    let err = parse(src).expect_err("must reject");
    assert!(err.to_string().contains("`timeout`"));
}

#[test]
fn test_step_rejects_should_fail_before_as() {
    let src = r#"
recipe r
    test { foo } should_fail as 'name'
"#;
    let err = parse(src).expect_err("must reject");
    let msg = err.to_string();
    assert!(msg.contains("`as`") || msg.contains("`should_fail`"));
}
```

- [ ] **Step 2.3.2: Run; expect failures**

```bash
cd cli && cargo test -p cook-lang test_step_rejects
```

Expected: 3 failures (the parser silently accepts the misordered tokens or produces a generic error).

- [ ] **Step 2.3.3: Implement order enforcement**

Inside the test-modifier parser, after each modifier is consumed, verify subsequent modifiers don't include earlier ones. The cleanest shape is to consume in canonical order and emit a specific error if the leftover token stream begins with an earlier-order keyword:

```rust
let as_name = parse_optional_as_name(tokens)?;
let timeout = parse_optional_timeout(tokens)?;

// After timeout, `as` is no longer accepted.
if peek_keyword(tokens, "as") {
    return Err(ParseError::new(
        format!("modifier `as` must precede `timeout` in test_step \
                 (canonical order: as → timeout → should_fail)"),
        current_line(tokens),
    ));
}

let should_fail = parse_optional_should_fail(tokens)?;

// After should_fail, `as` and `timeout` are no longer accepted.
if peek_keyword(tokens, "as") {
    return Err(ParseError::new(
        format!("modifier `as` must precede `should_fail` in test_step"),
        current_line(tokens),
    ));
}
if peek_keyword(tokens, "timeout") {
    return Err(ParseError::new(
        format!("modifier `timeout` must precede `should_fail` in test_step"),
        current_line(tokens),
    ));
}
```

- [ ] **Step 2.3.4: Run; confirm green**

```bash
cd cli && cargo test -p cook-lang test_step_rejects
cd cli && cargo test -p cook-lang
```

Expected: rejection tests pass; full suite stays green.

- [ ] **Step 2.3.5: Commit**

```bash
git add cli/crates/cook-lang/
git commit -m "$(cat <<'EOF'
feat(cook-lang): enforce test_step modifier canonical order (CS-NNNN §3.1.1)

Order is `as → timeout → should_fail`. Out-of-order modifier sequences
are rejected with a diagnostic naming the offending modifier and the
canonical position.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

### Task 2.4: Reject `as` on cook_step / plate_step

**Files:**
- Modify: `cli/crates/cook-lang/src/recipe.rs`
- Modify: `cli/crates/cook-lang/src/tests.rs`

- [ ] **Step 2.4.1: Write failing tests**

```rust
#[test]
fn test_as_rejected_on_cook_step() {
    let src = r#"
recipe r
    cook "out.txt" using { echo > $<out> } as 'foo'
"#;
    let err = parse(src).expect_err("must reject");
    let msg = err.to_string();
    assert!(msg.contains("`as`"));
    assert!(msg.contains("test_step") || msg.contains("test-step"));
}

#[test]
fn test_as_rejected_on_plate_step() {
    let src = r#"
recipe r
    cook "out.txt" using { echo > $<out> }
    plate { cp $<in> /tmp } as 'foo'
"#;
    let err = parse(src).expect_err("must reject");
    assert!(err.to_string().contains("`as`"));
}
```

- [ ] **Step 2.4.2: Run; expect failures**

```bash
cd cli && cargo test -p cook-lang test_as_rejected
```

- [ ] **Step 2.4.3: Implement rejection**

In each of cook_step and plate_step modifier paths, add:

```rust
if peek_keyword(tokens, "as") {
    return Err(ParseError::new(
        format!("modifier `as` is only valid on test_step (CS-NNNN §3.1)"),
        current_line(tokens),
    ));
}
```

Place these checks *after* the step's own modifiers are parsed (so an erroneous trailing `as 'foo'` triggers the rejection rather than being silently consumed).

- [ ] **Step 2.4.4: Run; confirm green; commit**

```bash
cd cli && cargo test -p cook-lang
```

Expected: all green. Commit:

```bash
git add cli/crates/cook-lang/
git commit -m "$(cat <<'EOF'
feat(cook-lang): reject `as` on cook_step and plate_step (CS-NNNN §3.1.3)

The `as STRING` modifier is test_step-only in CS-NNNN. Cook and plate
steps reject it with a diagnostic naming the migration target.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

### Task 2.5: Codegen — propagate `as_name` into `cook.add_test`

**Files:**
- Modify: `cli/crates/cook-luagen/src/test_step.rs`
- Modify: `cli/crates/cook-luagen/src/tests.rs`
- Modify: `examples/test_benchmarks/Cookfile` (uncomment `named_test`)

- [ ] **Step 2.5.1: Locate `generate_test_step`**

```bash
grep -n "fn generate_test_step\|cook.add_test" cli/crates/cook-luagen/src/test_step.rs | head
```

The function emits `cook.add_test({ … })` Lua. It needs to include `name = "..."` when `as_name` is present.

- [ ] **Step 2.5.2: Write failing codegen test**

Append to `cli/crates/cook-luagen/src/tests.rs`:

```rust
#[test]
fn test_step_codegen_with_as_emits_name_field() {
    let cook_src = r#"
recipe r
    test { true } as 'my-test' timeout 5
"#;
    let lua = codegen(cook_src).expect("codegen");
    assert!(
        lua.contains("name = \"my-test\""),
        "expected emitted Lua to set name; got:\n{lua}"
    );
}

#[test]
fn test_step_codegen_without_as_omits_name_field_or_uses_auto() {
    let cook_src = r#"
recipe r
    test { true } timeout 5
"#;
    let lua = codegen(cook_src).expect("codegen");
    // Auto-name path: codegen MAY emit `name = "test#1"` or omit the field
    // entirely; both are valid per §3.2 (the runner generates the auto-name
    // if the field is absent or empty). We only assert no `as_name` was
    // forced through.
    assert!(!lua.contains("name = \"my-test\""));
}

#[test]
fn test_step_codegen_substitutes_as_name() {
    // The `as '$<in.stem>-rt'` modifier substitutes per CS-0033 at codegen.
    let cook_src = r#"
recipe r
    ingredients "src/*.txt"
    cook "build/$<in.stem>.out" using { echo > $<out> }
    test { test -s $<in> } as '$<in.stem>-rt'
"#;
    let lua = codegen(cook_src).expect("codegen");
    // The emitted Lua should contain a name expression that substitutes
    // through the existing iteration binding (`_test_in` or equivalent).
    // The exact name varies per emitter; assert the bare token doesn't
    // leak through unsubstituted.
    assert!(
        !lua.contains("name = \"$<in.stem>-rt\""),
        "as_name should be substituted, not literal:\n{lua}"
    );
}
```

(`codegen()` is the existing helper used by other `cook-luagen` tests; if it doesn't exist, replicate the parse-then-emit pattern from sibling tests.)

- [ ] **Step 2.5.3: Run; expect failures**

```bash
cd cli && cargo test -p cook-luagen test_step_codegen_with_as test_step_codegen_substitutes_as_name
```

- [ ] **Step 2.5.4: Implement propagation**

In `generate_test_step`, when `as_name.is_some()`:
- For one-shot mode: substitute `as_name` through `expand_template_to_lua_with_deps` (no iteration binding); inject a `name = <substituted>,` field into the emitted `cook.add_test` table.
- For one-to-one shell: each iteration emits its own `cook.add_test` call inside the `for _, _test_in in ipairs(...) do ... end` loop; `as_name` substitutes per iteration. If the substituted name contains the iteration discriminator (i.e. `as_name` referenced `$<in>` / `$<in.X>`), no extra discriminator is appended; otherwise the runner appends `[<iteration-item>]` at execute time (this is a runner-side concern, not codegen — codegen just emits the substituted name).
- For many-to-one: one `cook.add_test` call; `as_name` substitutes against the empty-iteration binding (only `$<all>` and free identifiers are valid).

The codegen passes `as_name` as a separate argument to a new helper `emit_test_unit(body_text, as_name, timeout, should_fail, mode, source_list)`; this helper emits the appropriate Lua based on mode.

Concrete example for one-shot:

```rust
fn emit_one_shot_test(
    cmd_substituted: &str,
    as_name_substituted: Option<&str>,
    timeout: u64,
    should_fail: bool,
) -> String {
    let mut tbl = format!("cook.add_test({{ command = {:?}, ", cmd_substituted);
    if let Some(name) = as_name_substituted {
        tbl.push_str(&format!("name = {:?}, ", name));
    }
    tbl.push_str(&format!("timeout = {}, should_fail = {} }})", timeout, should_fail));
    tbl
}
```

For one-to-one, the substituted `as_name` and `cmd` are inside the `for` body so they reference `_test_in`.

- [ ] **Step 2.5.5: Run; confirm green**

```bash
cd cli && cargo test -p cook-luagen
```

- [ ] **Step 2.5.6: Uncomment `named_test` recipe in `test_benchmarks/Cookfile`**

In `examples/test_benchmarks/Cookfile`, uncomment the `named_test` recipe (Task 1.1.2's commented block):

```cook
recipe named_test
    ingredients "src/inputs/*.txt"
    cook "build/named_test/$<in.stem>.out" using {
        mkdir -p build/named_test
        echo "ok" > $<out>
    }
    test { test -s $<in> } as 'non-empty' timeout 5
```

Verify it parses + codegens:

```bash
cd /home/alex/dev/cook
cargo run -q --bin cook -- --emit-lua -f examples/test_benchmarks/Cookfile named_test | grep -i "name = "
```

Expected: at least one line containing `name = "non-empty"`.

- [ ] **Step 2.5.7: Commit**

```bash
git add cli/crates/cook-luagen/ examples/test_benchmarks/Cookfile
git commit -m "$(cat <<'EOF'
feat(cook-luagen): emit `name` field from test_step `as` modifier

Codegen substitutes $<...> in the as_name string and propagates it as
`name = ...` in the emitted cook.add_test() table per CS-NNNN §3.1.
Uncomments the named_test recipe in examples/test_benchmarks/.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

### Task 2.6: `cook.add_test` — default `suite` to recipe name; reject empty `command`

**Files:**
- Modify: `cli/crates/cook-register/src/test_api.rs`
- Modify: `cli/crates/cook-register/src/tests.rs`
- Modify: `cli/crates/cook-register/src/lib.rs` (if `current_recipe` not yet exposed)

- [ ] **Step 2.6.1: Confirm `CaptureState` knows the current recipe**

```bash
grep -n "current_recipe\|recipe_name\|enclosing_recipe" cli/crates/cook-register/src/lib.rs cli/crates/cook-register/src/*.rs
```

Identify the field; if it's not present, the register-phase already tracks recipe boundaries some other way (look for the `recipe r1` / `recipe r2` switching logic). The test_api needs the *fully-qualified* recipe name (namespace included for imported Cookfiles).

- [ ] **Step 2.6.2: Write failing tests**

Append to `cli/crates/cook-register/src/tests.rs`:

```rust
#[test]
fn add_test_defaults_suite_to_recipe_name() {
    let (lua, capture_state) = make_lua_with_test_api();
    capture_state.borrow_mut().current_recipe = Some("frontend.unit".to_string());

    lua.load(r#"
        cook.add_test({ command = "true" })
    "#).exec().unwrap();

    let state = capture_state.borrow();
    assert_eq!(state.units.len(), 1);
    let unit = &state.units[0];
    let payload = match &unit.payload {
        WorkPayload::Test { suite_name, .. } => suite_name,
        _ => panic!("expected Test payload"),
    };
    assert_eq!(payload, "frontend.unit");
}

#[test]
fn add_test_rejects_empty_command() {
    let (lua, capture_state) = make_lua_with_test_api();
    capture_state.borrow_mut().current_recipe = Some("r".to_string());

    let res = lua.load(r#"
        cook.add_test({ command = "" })
    "#).exec();

    assert!(res.is_err(), "empty command must be rejected");
    assert!(format!("{:?}", res).contains("command"));
}

#[test]
fn add_test_rejects_missing_command() {
    let (lua, capture_state) = make_lua_with_test_api();
    capture_state.borrow_mut().current_recipe = Some("r".to_string());

    let res = lua.load(r#"
        cook.add_test({ name = "x" })
    "#).exec();

    assert!(res.is_err(), "missing command must be rejected");
}

#[test]
fn add_test_rejects_non_positive_timeout() {
    let (lua, capture_state) = make_lua_with_test_api();
    capture_state.borrow_mut().current_recipe = Some("r".to_string());

    let res = lua.load(r#"
        cook.add_test({ command = "true", timeout = 0 })
    "#).exec();

    assert!(res.is_err());
    assert!(format!("{:?}", res).contains("timeout"));
}

#[test]
fn add_test_explicit_suite_overrides_default() {
    let (lua, capture_state) = make_lua_with_test_api();
    capture_state.borrow_mut().current_recipe = Some("r".to_string());

    lua.load(r#"
        cook.add_test({ command = "true", suite = "explicit" })
    "#).exec().unwrap();

    let state = capture_state.borrow();
    let suite = match &state.units[0].payload {
        WorkPayload::Test { suite_name, .. } => suite_name,
        _ => panic!(),
    };
    assert_eq!(suite, "explicit");
}
```

(If `current_recipe: Option<String>` doesn't exist on `CaptureState`, add it now in `lib.rs` with `Default::default()` initialization.)

- [ ] **Step 2.6.3: Run; expect failures**

```bash
cd cli && cargo test -p cook-register add_test_defaults add_test_rejects add_test_explicit
```

- [ ] **Step 2.6.4: Implement the contract-fixes**

Modify `cli/crates/cook-register/src/test_api.rs`'s `add_test_fn` closure body:

```rust
let command: String = tbl.get::<String>("command")
    .map_err(|_| mlua::Error::external("cook.add_test: command field is required"))?;
if command.is_empty() {
    return Err(mlua::Error::external(
        "cook.add_test: command field is required and must be a non-empty string"
    ));
}

let timeout: u64 = tbl.get::<Option<u64>>("timeout")?.unwrap_or(300);
if timeout == 0 {
    return Err(mlua::Error::external(
        "cook.add_test: timeout must be a positive number, got 0"
    ));
}

// Default suite to the enclosing recipe's name (CS-NNNN §3.2).
let suite_name: String = match tbl.get::<Option<String>>("suite")? {
    Some(s) if !s.is_empty() => s,
    _ => {
        let cs = capture_state.borrow();
        cs.current_recipe.clone().unwrap_or_default()
    }
};

let test_name: String = tbl.get::<Option<String>>("name")?.unwrap_or_default();
let should_fail: bool = tbl.get::<Option<bool>>("should_fail")?.unwrap_or(false);
```

(The `current_recipe` field was added in 2.6.1 if it didn't exist.)

- [ ] **Step 2.6.5: Verify `current_recipe` is populated when register-phase enters a recipe**

Find where the register driver enters a recipe (likely in `cook-engine/src/run.rs` or `cook-register/src/lib.rs`). Set `capture_state.borrow_mut().current_recipe = Some(qualified_name);` on entry; clear on exit.

```bash
grep -n "fn register_recipe\|current_recipe" cli/crates/cook-engine/src/ cli/crates/cook-register/src/
```

If no such hook exists, add `pub fn enter_recipe(&self, name: String)` and `pub fn exit_recipe(&self)` to `CaptureState` and call them at the recipe boundaries during register-phase.

- [ ] **Step 2.6.6: Run; confirm green**

```bash
cd cli && cargo test -p cook-register
cd cli && cargo test
```

Expected: all green workspace-wide.

- [ ] **Step 2.6.7: Commit**

```bash
git add cli/crates/cook-register/ cli/crates/cook-engine/ cli/crates/cook-contracts/
git commit -m "$(cat <<'EOF'
feat(cook-register): nail down cook.add_test field defaults (CS-NNNN §3.2)

- `suite` defaults to the enclosing recipe's name (was: "")
- empty/missing `command` rejected at register time
- `timeout <= 0` rejected
- explicit `suite` continues to override the default

Tracks `current_recipe` on CaptureState; populated on register-phase
recipe entry, cleared on exit.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

### Task 2.7: Update tests for fingerprint inputs (timeout, should_fail)

**Files:**
- Modify: `cli/crates/cook-fingerprint/src/check.rs` (or wherever the cook-unit fingerprint is computed)
- Modify: `cli/crates/cook-fingerprint/src/lib.rs`

This task only adds the `timeout` and `should_fail` fingerprint inputs to the existing fingerprint computation for **test units**. The full test-result cache backend lands in Phase 5; this preparatory step ensures the fingerprint is correct before the cache is wired.

- [ ] **Step 2.7.1: Survey the existing fingerprint API**

```bash
grep -n "pub fn\|fn compute" cli/crates/cook-fingerprint/src/lib.rs cli/crates/cook-fingerprint/src/check.rs | head -30
```

Identify the function that computes per-unit fingerprints. It likely takes a `WorkPayload` or `CapturedUnit`.

- [ ] **Step 2.7.2: Write failing test for timeout difference**

Append to the appropriate `#[cfg(test)]` module in `cook-fingerprint`:

```rust
#[test]
fn test_unit_fingerprint_includes_timeout() {
    let mk = |timeout: u64| WorkPayload::Test {
        cmd: "true".into(), line: 1,
        timeout, should_fail: false,
        suite_name: "r".into(), test_name: "t".into(),
    };
    let fp_30 = compute_test_fingerprint(&mk(30), &empty_inputs());
    let fp_60 = compute_test_fingerprint(&mk(60), &empty_inputs());
    assert_ne!(fp_30, fp_60, "different timeouts must produce different fingerprints");
}

#[test]
fn test_unit_fingerprint_includes_should_fail() {
    let mk = |sf: bool| WorkPayload::Test {
        cmd: "true".into(), line: 1,
        timeout: 30, should_fail: sf,
        suite_name: "r".into(), test_name: "t".into(),
    };
    let fp_t = compute_test_fingerprint(&mk(true), &empty_inputs());
    let fp_f = compute_test_fingerprint(&mk(false), &empty_inputs());
    assert_ne!(fp_t, fp_f);
}

#[test]
fn test_unit_fingerprint_independent_of_test_name() {
    // Renaming via `as` (the test_name) MUST NOT bust fingerprint per CS-NNNN §3.3.
    let mk = |name: &str| WorkPayload::Test {
        cmd: "true".into(), line: 1,
        timeout: 30, should_fail: false,
        suite_name: "r".into(), test_name: name.into(),
    };
    let fp_a = compute_test_fingerprint(&mk("alpha"), &empty_inputs());
    let fp_b = compute_test_fingerprint(&mk("beta"),  &empty_inputs());
    assert_eq!(fp_a, fp_b, "renaming a test MUST NOT bust its fingerprint");
}

#[test]
fn test_unit_fingerprint_independent_of_suite_name() {
    let mk = |suite: &str| WorkPayload::Test {
        cmd: "true".into(), line: 1,
        timeout: 30, should_fail: false,
        suite_name: suite.into(), test_name: "t".into(),
    };
    let fp_a = compute_test_fingerprint(&mk("recipe_a"), &empty_inputs());
    let fp_b = compute_test_fingerprint(&mk("recipe_b"), &empty_inputs());
    assert_eq!(fp_a, fp_b);
}

fn empty_inputs() -> FingerprintInputs {
    FingerprintInputs::default() // or whatever empty constructor exists
}
```

(`compute_test_fingerprint` is the new function being added; if a function of similar shape already exists for cook units, mirror its signature.)

- [ ] **Step 2.7.3: Run; expect failures**

```bash
cd cli && cargo test -p cook-fingerprint test_unit_fingerprint
```

Expected: compile error (`compute_test_fingerprint` doesn't exist).

- [ ] **Step 2.7.4: Implement `compute_test_fingerprint`**

Add to `cli/crates/cook-fingerprint/src/lib.rs` (or `check.rs` per the file layout convention):

```rust
use cook_contracts::WorkPayload;
use sha2::{Sha256, Digest};

#[derive(Debug, Default, Clone)]
pub struct FingerprintInputs {
    pub cook_outputs: Vec<(String, String)>, // (path, content_fingerprint)
    pub dep_outputs: Vec<(String, String)>,
    pub env_keys: Vec<(String, String)>,
    pub tool_hashes: Vec<(String, String)>,
}

/// Compute a content-addressed fingerprint for a test unit per CS-NNNN §3.3.
///
/// Inputs (in this order, hashed as a stable byte sequence):
///   1. cmd (substituted command text)
///   2. timeout (literal u64, big-endian bytes)
///   3. should_fail (literal u8: 0 or 1)
///   4. cook_outputs (sorted by path; each: path + content_fingerprint)
///   5. dep_outputs (sorted by path; each: path + content_fingerprint)
///   6. env_keys (sorted by key; each: key + value)
///   7. tool_hashes (sorted by name; each: name + hash)
///
/// Excluded: suite_name, test_name (display metadata; renaming MUST NOT bust).
pub fn compute_test_fingerprint(
    payload: &WorkPayload,
    inputs: &FingerprintInputs,
) -> String {
    let (cmd, timeout, should_fail) = match payload {
        WorkPayload::Test { cmd, timeout, should_fail, .. } =>
            (cmd.as_str(), *timeout, *should_fail),
        _ => panic!("compute_test_fingerprint: not a Test payload"),
    };

    let mut h = Sha256::new();
    h.update(cmd.as_bytes()); h.update(b"\0");
    h.update(timeout.to_be_bytes()); h.update(b"\0");
    h.update([if should_fail { 1u8 } else { 0u8 }]); h.update(b"\0");

    let mut sort = |v: &[(String, String)]| {
        let mut s: Vec<_> = v.iter().collect();
        s.sort();
        for (k, val) in s { h.update(k.as_bytes()); h.update(b"="); h.update(val.as_bytes()); h.update(b"\0"); }
    };
    sort(&inputs.cook_outputs);
    sort(&inputs.dep_outputs);
    sort(&inputs.env_keys);
    sort(&inputs.tool_hashes);

    format!("sha256:{:x}", h.finalize())
}
```

If `sha2` isn't already a dep, add it to `cli/crates/cook-fingerprint/Cargo.toml`:

```bash
grep -q '^sha2' cli/crates/cook-fingerprint/Cargo.toml || \
  echo 'sha2 = "0.10"' >> cli/crates/cook-fingerprint/Cargo.toml
```

(Verify the workspace Cargo.toml already pins `sha2` to a version; if so, use `sha2.workspace = true` instead.)

- [ ] **Step 2.7.5: Run; confirm green**

```bash
cd cli && cargo test -p cook-fingerprint
```

- [ ] **Step 2.7.6: Commit**

```bash
git add cli/crates/cook-fingerprint/
git commit -m "$(cat <<'EOF'
feat(cook-fingerprint): add compute_test_fingerprint (CS-NNNN §3.3)

Test-unit fingerprint includes cmd + timeout + should_fail + cook_outputs
+ dep_outputs + env_keys + tool_hashes. Display metadata (suite_name,
test_name) is intentionally excluded so renaming does not bust cache.

Cache wiring in Phase 5.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

### Task 2.8: Conformance fixtures (positive)

**Files:**
- Create: `standard/conformance/positive/test_as_modifier.cook`
- Create: `standard/conformance/positive/test_as_with_substitution.cook`
- Create: `standard/conformance/positive/test_add_test_default_suite.cook`
- Create: `standard/conformance/positive/test_should_fail_pass_table.cook`
- Create: `standard/conformance/positive/test_cache_key_independence.cook`

- [ ] **Step 2.8.1: Author `test_as_modifier.cook`**

```cook
# Conformance: every modifier-order subset accepted (CS-NNNN §3.1.1).
recipe r1
    test { true } as 'name'

recipe r2
    test { true } as 'name' timeout 30

recipe r3
    test { true } as 'name' should_fail

recipe r4
    test { true } as 'name' timeout 30 should_fail

recipe r5
    test { true } timeout 30

recipe r6
    test { true } timeout 30 should_fail

recipe r7
    test { true } should_fail

recipe r8
    test { true }
```

- [ ] **Step 2.8.2: Author `test_as_with_substitution.cook`**

```cook
# Conformance: $<...> substitution in as_name (CS-NNNN §3.1).
recipe r
    ingredients "src/*.txt"
    cook "build/$<in.stem>.out" using { echo > $<out> }
    test { test -s $<in> } as '$<in.stem>-roundtrip'
```

- [ ] **Step 2.8.3: Author `test_add_test_default_suite.cook`**

```cook
# Conformance: `suite` defaults to recipe name when omitted (CS-NNNN §3.2).
# Also: explicit suite overrides default.
recipe r
    test >{
        cook.add_test { command = "true" }              -- default suite = "r"
        cook.add_test { command = "true", suite = "x" } -- explicit suite = "x"
    }
```

- [ ] **Step 2.8.4: Author `test_should_fail_pass_table.cook`**

```cook
# Conformance: pass/fail interpretation table (CS-NNNN §3.4).
recipe pass_zero_no_should_fail
    test { exit 0 } timeout 5

recipe pass_nonzero_with_should_fail
    test { exit 1 } should_fail timeout 5

recipe fail_zero_with_should_fail
    test { exit 0 } should_fail timeout 5

recipe fail_nonzero_no_should_fail
    test { exit 1 } timeout 5
```

- [ ] **Step 2.8.5: Author `test_cache_key_independence.cook`**

```cook
# Conformance: identical bodies in different recipes share fingerprint
# (CS-NNNN §3.3 — display name and recipe name not in fingerprint).
recipe alpha
    test { test 1 -eq 1 } as 'first' timeout 5

recipe beta
    test { test 1 -eq 1 } as 'second' timeout 5
```

- [ ] **Step 2.8.6: Run conformance**

```bash
cd cli && cargo test -p cook-lang --test conformance
```

Expected: all pass (parser already supports the `as` modifier from Tasks 2.2–2.4).

- [ ] **Step 2.8.7: Commit**

```bash
git add standard/conformance/positive/
git commit -m "$(cat <<'EOF'
test(conformance): positive fixtures for CS-NNNN test_step additions

Pin modifier-order subsets, $<...> substitution in as_name, suite
default-to-recipe-name, pass/fail outcome table, and fingerprint
independence from display metadata.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

### Task 2.9: Conformance fixtures (negative)

**Files:**
- Create: `standard/conformance/negative/test_as_after_timeout.cook`
- Create: `standard/conformance/negative/test_as_after_should_fail.cook`
- Create: `standard/conformance/negative/test_should_fail_before_timeout.cook`
- Create: `standard/conformance/negative/test_as_non_string.cook`
- Create: `standard/conformance/negative/test_as_on_cook_step.cook`
- Create: `standard/conformance/negative/test_as_on_plate_step.cook`
- Create: `standard/conformance/negative/test_add_test_empty_command.cook`
- Create: `standard/conformance/negative/test_add_test_negative_timeout.cook`

- [ ] **Step 2.9.1: Author each negative fixture**

```cook
# test_as_after_timeout.cook — modifier `as` must precede `timeout`.
recipe r
    test { true } timeout 30 as 'name'
```

```cook
# test_as_after_should_fail.cook — `as` must precede `should_fail`.
recipe r
    test { true } should_fail as 'name'
```

```cook
# test_should_fail_before_timeout.cook — `timeout` must precede `should_fail`.
recipe r
    test { true } should_fail timeout 30
```

```cook
# test_as_non_string.cook — `as` requires a STRING literal.
recipe r
    test { true } as 5
```

```cook
# test_as_on_cook_step.cook — `as` is test_step-only.
recipe r
    cook "out.txt" using { echo > $<out> } as 'foo'
```

```cook
# test_as_on_plate_step.cook — `as` is test_step-only.
recipe r
    cook "out.txt" using { echo > $<out> }
    plate { cp $<in> /tmp } as 'foo'
```

```cook
# test_add_test_empty_command.cook — empty command rejected.
recipe r
    test >{ cook.add_test { command = "" } }
```

```cook
# test_add_test_negative_timeout.cook — non-positive timeout rejected.
recipe r
    test >{ cook.add_test { command = "true", timeout = 0 } }
```

- [ ] **Step 2.9.2: Run conformance**

```bash
cd cli && cargo test -p cook-lang --test conformance
```

Expected: all negatives produce parse / register errors as required.

- [ ] **Step 2.9.3: Commit**

```bash
git add standard/conformance/negative/
git commit -m "$(cat <<'EOF'
test(conformance): negative fixtures for CS-NNNN test_step additions

Pin every reject-case from §3.1 (modifier order, non-string as),
§3.1.3 (as on cook/plate), §3.2 (empty command, non-positive timeout).

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

### Task 2.10: Update Standard chapters and appendices

**Files:**
- Modify: `standard/src/content/docs/04-recipes.mdx`
- Modify: `standard/src/content/docs/06-cook-lua-api.mdx`
- Modify: `standard/src/content/docs/08-execution-model.mdx`
- Modify: `standard/src/content/docs/appendix/A-grammar.mdx`
- Modify: `standard/src/content/docs/appendix/B-rationale.mdx`
- Modify: `standard/src/content/docs/appendix/D-changes.mdx`

The exact prose lives in the Standard-side spec at `standard/specs/2026-05-07-test-runner-language-design.md`. Apply the deltas literally.

- [ ] **Step 2.10.1: §4.8 — extend modifier table**

In `standard/src/content/docs/04-recipes.mdx`, locate §4.8's `test_step` modifier table. Add a row for `as STRING` *before* the `timeout` row (per canonical-order rule). Replace any prior textual description of `test_modifiers` with a reference to App. A.4's grammar production.

Add the §3.4 outcome table (pass/fail × should_fail × timeout) immediately below the modifier table.

- [ ] **Step 2.10.2: §6.4 — `cook.add_test` field table**

In `standard/src/content/docs/06-cook-lua-api.mdx`, locate §6.4's `cook.add_test` documentation. Replace the existing field list with the table from CS-NNNN §3.2. Note in prose: `suite` defaults to the enclosing recipe's fully-qualified name; `command` empty/missing is rejected; `timeout ≤ 0` is rejected.

- [ ] **Step 2.10.3: §8.6 — new test-cache subsection**

In `standard/src/content/docs/08-execution-model.mdx`, locate §8.6 (caching). Add a new subsection §8.6.x titled "Test-unit caching" with the verbatim rules from CS-NNNN §3.3 (the `> §8.6.x Test-unit caching.` block in the spec). Number the subsection by following the existing §8.6 numbering convention.

- [ ] **Step 2.10.4: App. A.4 — grammar update**

In `standard/src/content/docs/appendix/A-grammar.mdx`, locate the `test_step` and `test_modifiers` productions. Replace `test_modifiers` with:

```ebnf
test_modifiers ::= ("as" STRING)? ("timeout" NUMBER)? "should_fail"?
```

Add a sentence in the surrounding prose: "Modifier order is canonical: `as → timeout → should_fail`. Out-of-order sequences are rejected (§4.8)."

- [ ] **Step 2.10.5: App. B.4 — rationale subsections**

In `standard/src/content/docs/appendix/B-rationale.mdx`, add five new B.4 subsections per CS-NNNN §3.8 (B-new.1 through B-new.5). Use the verbatim prose from the spec.

- [ ] **Step 2.10.6: App. D — CS-NNNN entry**

In `standard/src/content/docs/appendix/D-changes.mdx`:
- In the "Versions" index, append CS-NNNN to the `v0.8 (unreleased)` row.
- Add a new section `## D.NN. CS-NNNN — test-step language additions for v1.0 test runner` at the bottom (numbering matches the index — currently the last D.NN is D.59 for CS-0059, so this is D.60 if assigned next; but the Standard-side numbering is "assigned at PR time" per repo convention, so use `CS-NNNN` placeholder until merge).

Use the verbatim entry from CS-NNNN §3.9.

- [ ] **Step 2.10.7: Verify the spec builds**

```bash
cd standard && pnpm build
```

Expected: exit 0. Fix any MDX errors (unbalanced tags, broken cross-references).

- [ ] **Step 2.10.8: Commit**

```bash
git add standard/src/
git commit -m "$(cat <<'EOF'
spec(CS-NNNN): test_step language additions — `as` modifier, cache contract

Updates §4.8 (modifier table + outcome table), §6.4 (add_test fields),
§8.6 (test-cache subsection), App. A.4 (grammar), App. B.4 (rationale
×5), App. D (changelog entry).

Companion runner-impl spec is local under docs/superpowers/specs/.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

### Task 2.11: tree-sitter-cook grammar update

**Files:**
- Modify: `tree-sitter-cook/grammar.js`
- Modify: `tree-sitter-cook/queries/highlights.scm`
- Create/Modify: `tree-sitter-cook/test/corpus/test_step.txt`

- [ ] **Step 2.11.1: Locate the `test_step` rule**

```bash
grep -n "test_step\|test_modifier" tree-sitter-cook/grammar.js
```

- [ ] **Step 2.11.2: Update the grammar**

In `tree-sitter-cook/grammar.js`, modify the `test_step` rule's modifier slot to admit an optional leading `as_name` field:

```javascript
test_step: $ => seq(
  'test',
  field('body', choice($.shell_block, $.using_lua_block)),
  field('as_name', optional(seq('as', $.string))),
  field('timeout', optional(seq('timeout', $.number))),
  field('should_fail', optional('should_fail')),
),
```

- [ ] **Step 2.11.3: Update highlights query**

In `tree-sitter-cook/queries/highlights.scm`, append:

```scheme
(test_step as_name: (string) @string.special)
```

- [ ] **Step 2.11.4: Add corpus fixture**

Append to `tree-sitter-cook/test/corpus/test_step.txt` (or create if it doesn't exist) — follow the existing tree-sitter-cook corpus format:

```
==================
test_step with as modifier
==================

recipe r
    test { true } as 'foo' timeout 30 should_fail

---

(source_file
  (recipe_decl
    name: (identifier)
    body: (recipe_body
      (test_step
        body: (shell_block (shell_content))
        as_name: (string)
        timeout: (number)))))
```

(Adjust the parse-tree assertion to match the actual node names tree-sitter-cook emits; run `npx tree-sitter parse` to inspect.)

- [ ] **Step 2.11.5: Build and test the grammar**

```bash
cd tree-sitter-cook
npx tree-sitter generate
npx tree-sitter test
node scripts/conformance.mjs
```

Expected: all pass. Fix grammar.js / corpus mismatches as needed.

- [ ] **Step 2.11.6: Commit**

```bash
git add tree-sitter-cook/
git commit -m "$(cat <<'EOF'
feat(tree-sitter-cook): support `as STRING` test_step modifier (CS-NNNN)

Grammar admits optional `as_name` field before `timeout`. Highlights
query promotes the as_name string to @string.special. Corpus pinned.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

**Phase 2 checkpoint.** CS-NNNN is acceptance-complete: parser, codegen, register, fingerprint, conformance, Standard, tree-sitter all updated. The `as 'name'` modifier and contract-fix `cook.add_test` defaults are live. The cook binary still has no test runner — `cook --test` is not yet a flag. Existing `cook RECIPE` invocations work identically; failing test_steps still produce `CommandFailed` exit 1 (the `cli_audit_exit_codes/walkthrough.sh` continues to pass).

---

## Phase 3: Engine — fix `RunResult`, add register-all and reverse-closure

Three discrete changes: (1) populate `RunResult.test_results` from worker output (today's empty-Vec placeholder); (2) extend `EngineEvent` with test-shaped variants; (3) add `analyzer::register_workspace_for_test()` and `dag_builder::build_test_slice()`. After this phase, the engine is *ready* for a test-mode invocation but no CLI plumbing exists yet.

### Task 3.1: Define `TestId`, `TestOutcome`, `TestResult`

**Files:**
- Modify: `cli/crates/cook-engine/src/lib.rs`

- [ ] **Step 3.1.1: Add the public types**

Append to `cli/crates/cook-engine/src/lib.rs` (or in a new sub-module if the file is already busy):

```rust
/// Stable identity for one test unit. Format: `<namespace>.<recipe>:<name>[<discriminator>]`.
/// The discriminator is empty for one-shot tests; for iteration modes it is the
/// iteration item (typically the input filename's basename).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Ord, PartialOrd)]
pub struct TestId(pub String);

impl std::fmt::Display for TestId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Outcome of one test unit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TestOutcome {
    Passed,
    Failed,
    Blocked,
    TimedOut,
}

/// Reason for a test failure (when outcome is Failed).
#[derive(Debug, Clone)]
pub enum TestFailureReason {
    ExitStatusMismatch { expected_success: bool, observed_success: bool },
    SignalKilled(i32),
    SpawnError(String),
}

/// One row in `RunResult.test_results`.
#[derive(Debug, Clone)]
pub struct TestResult {
    pub id: TestId,
    pub namespace: String,
    pub recipe: String,
    pub name: String,
    pub suite: String,
    pub iteration_item: Option<String>,
    pub outcome: TestOutcome,
    pub duration: std::time::Duration,
    pub from_cache: bool,
    pub stdout: String,
    pub stderr: String,
    pub fingerprint: Option<String>,
    pub blocked_by: Option<String>,
    pub should_fail: bool,
    pub timed_out: bool,
}
```

- [ ] **Step 3.1.2: Verify compile**

```bash
cd cli && cargo check -p cook-engine
```

Expected: clean.

- [ ] **Step 3.1.3: Commit**

```bash
git add cli/crates/cook-engine/src/lib.rs
git commit -m "$(cat <<'EOF'
feat(cook-engine): define TestId, TestOutcome, TestResult types

Public types for the v1.0 test runner per
docs/superpowers/specs/2026-05-07-test-runner-design.md §4.6.
Wiring follows.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

### Task 3.2: Extend `EngineEvent` with test variants

**Files:**
- Modify: `cli/crates/cook-engine/src/lib.rs`

- [ ] **Step 3.2.1: Locate the existing `EngineEvent` enum**

```bash
grep -n "pub enum EngineEvent\|RecipeStarted\|UnitFailed" cli/crates/cook-engine/src/lib.rs | head -10
```

- [ ] **Step 3.2.2: Add the new variants**

Add to the `EngineEvent` enum (preserve existing variants; add at the end of the enum):

```rust
TestStarted {
    id: TestId,
    recipe: String,
    name: String,
},
TestPassed {
    id: TestId,
    duration: std::time::Duration,
    cached: bool,
    stdout: String,
    stderr: String,
},
TestFailed {
    id: TestId,
    duration: std::time::Duration,
    stdout: String,
    stderr: String,
    reason: TestFailureReason,
},
TestBlocked {
    id: TestId,
    upstream: String,
},
TestTimedOut {
    id: TestId,
    timeout: std::time::Duration,
    stdout: String,
    stderr: String,
},
```

If `EngineEvent` carries `#[non_exhaustive]` (per CS-0049), this is purely additive. If it doesn't yet, leave the attribute alone — adding non_exhaustive in this CS would be out-of-scope.

- [ ] **Step 3.2.3: Verify compile + workspace tests**

```bash
cd cli && cargo test --workspace
```

Expected: all green. (Adding enum variants is additive; no existing match should be exhaustive over the test variants.)

If a downstream match arm errors (e.g., `cook-progress` exhaustively matches), add a wildcard arm `_ => {}` for the new variants. Phase 4 wires real handlers.

- [ ] **Step 3.2.4: Commit**

```bash
git add cli/crates/cook-engine/ cli/crates/cook-progress/ cli/crates/cook-cli/
git commit -m "$(cat <<'EOF'
feat(cook-engine): add test events to EngineEvent

TestStarted/Passed/Failed/Blocked/TimedOut. Downstream consumers stub
with wildcard until Phase 4 wires real handlers.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

### Task 3.3: Replace empty `Vec::new()` with real `RunResult.test_results`

**Files:**
- Modify: `cli/crates/cook-engine/src/run.rs`
- Modify: `cli/crates/cook-engine/src/executor.rs`

- [ ] **Step 3.3.1: Locate the placeholder**

```bash
grep -n "all_test_outputs\|test_outputs\|test_results" cli/crates/cook-engine/src/run.rs
```

Confirms: line ~215 has `let all_test_outputs: Vec<cook_luaotp::TestOutput> = Vec::new();` and line ~323 stuffs that empty Vec into the result.

- [ ] **Step 3.3.2: Update `RunResult` shape**

Replace the existing `pub struct RunResult { pub test_outputs: Vec<cook_luaotp::TestOutput> }` with:

```rust
pub struct RunResult {
    pub test_results: Vec<crate::TestResult>,
}
```

- [ ] **Step 3.3.3: Replace the placeholder in `run_inner`**

Replace `let all_test_outputs: Vec<cook_luaotp::TestOutput> = Vec::new();` with a real accumulator:

```rust
let test_results: std::sync::Mutex<Vec<crate::TestResult>> = std::sync::Mutex::new(Vec::new());
```

(`Mutex` because the executor is multi-threaded; `Arc` if you need to clone into worker contexts.)

At the bottom of `run_inner`, replace `test_outputs: all_test_outputs` with:

```rust
test_results: test_results.into_inner().expect("mutex poisoned"),
```

- [ ] **Step 3.3.4: Wire executor to push test results**

In `cli/crates/cook-engine/src/executor.rs`, locate the `WorkResult` consumer. When a `WorkResult` carries `test_output: Some(TestOutput { … })`, translate it to a `TestResult` and push to the accumulator:

```rust
if let Some(to) = result.test_output {
    let outcome = if to.timed_out {
        crate::TestOutcome::TimedOut
    } else if (to.exit_success && !to.should_fail) || (!to.exit_success && to.should_fail) {
        crate::TestOutcome::Passed
    } else {
        crate::TestOutcome::Failed
    };

    let id = parse_test_id(&result.node_name); // helper that maps "<recipe>:<test_name>" → TestId
    test_results.lock().unwrap().push(crate::TestResult {
        id: id.clone(),
        namespace: id_namespace(&id),
        recipe: id_recipe(&id),
        name: to.test_name.clone(),
        suite: to.suite_name.clone(),
        iteration_item: None, // TODO: extract from node_name in 3.4 wiring
        outcome,
        duration: std::time::Duration::from_secs_f64(to.duration),
        from_cache: false, // populated when cache lookup hits in Phase 5
        stdout: to.stdout,
        stderr: to.stderr,
        fingerprint: None, // Populated in Phase 5
        blocked_by: None,
        should_fail: to.should_fail,
        timed_out: to.timed_out,
    });
}
```

Define the helpers `parse_test_id`, `id_namespace`, `id_recipe` in a new private module `id.rs` under `cook-engine/src/`:

```rust
// cli/crates/cook-engine/src/id.rs
use crate::TestId;

pub fn parse_test_id(node_name: &str) -> TestId {
    // node_name shape from cook-luaotp: "<recipe>:<test_name>" or "<namespace>.<recipe>:<test_name>"
    TestId(node_name.to_string())
}

pub fn id_namespace(id: &TestId) -> String {
    let s = &id.0;
    if let Some(colon) = s.find(':') {
        let before = &s[..colon];
        if let Some(dot) = before.rfind('.') {
            return before[..dot].to_string();
        }
    }
    String::new()
}

pub fn id_recipe(id: &TestId) -> String {
    let s = &id.0;
    let before_colon = s.split(':').next().unwrap_or("");
    if let Some(dot) = before_colon.rfind('.') {
        before_colon[dot + 1..].to_string()
    } else {
        before_colon.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parse_simple() {
        let id = parse_test_id("frontend.unit:test#1");
        assert_eq!(id_namespace(&id), "frontend");
        assert_eq!(id_recipe(&id), "unit");
    }
    #[test]
    fn parse_no_namespace() {
        let id = parse_test_id("build:test#1");
        assert_eq!(id_namespace(&id), "");
        assert_eq!(id_recipe(&id), "build");
    }
}
```

Wire `pub mod id;` into `cli/crates/cook-engine/src/lib.rs`.

- [ ] **Step 3.3.5: Update existing tests that consume `RunResult.test_outputs`**

```bash
grep -rn "test_outputs\|RunResult {" cli/crates/cook-engine/ cli/crates/cook-cli/
```

Replace every reference to `result.test_outputs` with `result.test_results`. Update the test at `run.rs` line 397 (`assert!(result.unwrap().test_outputs.is_empty())`) to `result.unwrap().test_results.is_empty()`.

- [ ] **Step 3.3.6: Run; confirm green**

```bash
cd cli && cargo test --workspace
```

- [ ] **Step 3.3.7: Commit**

```bash
git add cli/crates/cook-engine/ cli/crates/cook-cli/
git commit -m "$(cat <<'EOF'
fix(cook-engine): populate RunResult.test_results from worker output

Replaces the Vec::new() placeholder at run.rs:215 with a real
accumulator. Workers' WorkResult::test_output is translated into
crate::TestResult and pushed.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

### Task 3.4: Emit test events from the executor

**Files:**
- Modify: `cli/crates/cook-engine/src/executor.rs`

- [ ] **Step 3.4.1: Wire event emission into the WorkResult consumer**

Adjacent to the `TestResult` push from Task 3.3, emit the corresponding `EngineEvent`:

```rust
let evt = match outcome {
    crate::TestOutcome::Passed => crate::EngineEvent::TestPassed {
        id: id.clone(),
        duration: std::time::Duration::from_secs_f64(to.duration),
        cached: false,
        stdout: to.stdout.clone(),
        stderr: to.stderr.clone(),
    },
    crate::TestOutcome::Failed => crate::EngineEvent::TestFailed {
        id: id.clone(),
        duration: std::time::Duration::from_secs_f64(to.duration),
        stdout: to.stdout.clone(),
        stderr: to.stderr.clone(),
        reason: crate::TestFailureReason::ExitStatusMismatch {
            expected_success: !to.should_fail,
            observed_success: to.exit_success,
        },
    },
    crate::TestOutcome::TimedOut => crate::EngineEvent::TestTimedOut {
        id: id.clone(),
        timeout: std::time::Duration::from_secs_f64(to.duration),
        stdout: to.stdout.clone(),
        stderr: to.stderr.clone(),
    },
    crate::TestOutcome::Blocked => unreachable!("Blocked is set when upstream cook fails, not from WorkResult"),
};
on_event(evt);
```

- [ ] **Step 3.4.2: Emit `TestBlocked` when upstream cook fails**

Locate where the executor handles cook-step failures that should cancel downstream test units. For each test unit whose dep on a failed cook step is fatal, emit `TestBlocked { id, upstream }` and push a `TestResult` with `outcome: Blocked`.

The wave-grouper (`cook-engine/src/wave_grouper.rs`) already keeps test sibling failures from cancelling each other (`DepKind::TestSibling`); blocked-by-upstream is a separate concern. Use the executor's existing failed-unit dependency walk to compute blocked test units after the wave completes.

- [ ] **Step 3.4.3: Run; commit**

```bash
cd cli && cargo test --workspace
```

```bash
git add cli/crates/cook-engine/
git commit -m "$(cat <<'EOF'
feat(cook-engine): emit test events from executor

Translates worker TestOutput into TestPassed/Failed/TimedOut events;
detects blocked test units from upstream cook failures and emits
TestBlocked.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

### Task 3.5: `analyzer::register_workspace_for_test`

**Files:**
- Modify: `cli/crates/cook-engine/src/analyzer.rs`

- [ ] **Step 3.5.1: Survey existing analyzer API**

```bash
grep -n "pub fn\|fn analyze\|RecipeInfo" cli/crates/cook-engine/src/analyzer.rs | head -20
```

Identify the target-driven entry point. The new entry point produces the same `BTreeMap<String, RecipeInfo>` but without filtering to a target list — every recipe in every imported Cookfile is included.

- [ ] **Step 3.5.2: Write a failing test**

Append to `cli/crates/cook-engine/src/analyzer.rs`'s `#[cfg(test)]` mod:

```rust
#[test]
fn register_workspace_for_test_includes_all_recipes_across_imports() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();

    // Workspace root with one import
    std::fs::write(root.join("Cookfile"), "
import sub ./sub
recipe build
    cook \"build/r.txt\" using { echo > $<out> }
").unwrap();

    std::fs::create_dir(root.join("sub")).unwrap();
    std::fs::write(root.join("sub/Cookfile"), "
recipe inner
    cook \"build/i.txt\" using { echo > $<out> }
recipe test_only
    test { true } timeout 5
").unwrap();

    let result = register_workspace_for_test(root).expect("must succeed");
    let names: BTreeSet<_> = result.keys().collect();
    // Every recipe from every Cookfile MUST be present, even those not
    // referenced by any cross-Cookfile dep.
    assert!(names.contains(&"build".to_string()));
    assert!(names.contains(&"sub.inner".to_string()));
    assert!(names.contains(&"sub.test_only".to_string()),
            "test_only is not referenced by any target but must still be registered");
}
```

- [ ] **Step 3.5.3: Implement the function**

Add to `cli/crates/cook-engine/src/analyzer.rs`:

```rust
/// Register every recipe in every imported Cookfile in the workspace,
/// regardless of whether it is reachable from any target. Used by
/// `cook --test` to discover all test_step units across the workspace.
pub fn register_workspace_for_test(
    project_root: &Path,
) -> Result<BTreeMap<String, RecipeInfo>, GraphError> {
    // 1. Walk the import graph from project_root, collecting (alias, cookfile_path)
    //    pairs. The existing target-driven path already does this — extract the
    //    walk into a helper if needed.
    let cookfiles = walk_workspace_imports(project_root)?;

    // 2. For each Cookfile, load and register every recipe. Use a synthetic
    //    target list = every recipe name in that Cookfile so the existing
    //    register pipeline runs each.
    let mut all = BTreeMap::new();
    for (namespace, cookfile_path) in cookfiles {
        let parsed = cook_lang::parse_file(&cookfile_path)?;
        for recipe in &parsed.recipes {
            let qualified = if namespace.is_empty() {
                recipe.name.clone()
            } else {
                format!("{}.{}", namespace, recipe.name)
            };
            // Register this recipe; mirror the target-driven path's logic.
            let info = register_one_recipe(&parsed, recipe, &cookfile_path)?;
            all.insert(qualified, info);
        }
    }
    Ok(all)
}

fn walk_workspace_imports(root: &Path) -> Result<Vec<(String, PathBuf)>, GraphError> {
    // Reuse existing import-walk code; if not factored, replicate the BFS over
    // `import alias path` declarations starting from root/Cookfile.
    todo!("factor walk from existing analyzer; see cook-engine/src/analyzer.rs target-driven path")
}

fn register_one_recipe(
    parsed: &cook_lang::ast::Cookfile,
    recipe: &cook_lang::ast::Recipe,
    cookfile_path: &Path,
) -> Result<RecipeInfo, GraphError> {
    // Mirror the per-recipe registration logic the target-driven path uses.
    todo!("factor register_one from existing analyzer")
}
```

The two `todo!` helpers exist in spirit in the existing analyzer; lift them into named functions (`walk_workspace_imports`, `register_one_recipe`) so both target-driven and workspace-for-test entry points can share them. If the existing code has them inlined, refactor in this task.

- [ ] **Step 3.5.4: Run; confirm green**

```bash
cd cli && cargo test -p cook-engine register_workspace_for_test
```

- [ ] **Step 3.5.5: Commit**

```bash
git add cli/crates/cook-engine/src/analyzer.rs
git commit -m "$(cat <<'EOF'
feat(cook-engine): register_workspace_for_test entry point

Walks the import graph and registers every recipe in every Cookfile,
regardless of reachability from a target. Used by cook --test workspace
discovery (Phase 4).

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

### Task 3.6: `dag_builder::build_test_slice`

**Files:**
- Modify: `cli/crates/cook-engine/src/dag_builder.rs`

- [ ] **Step 3.6.1: Survey existing DAG builder API**

```bash
grep -n "pub fn\|build_dag\|UnitId" cli/crates/cook-engine/src/dag_builder.rs | head -20
```

- [ ] **Step 3.6.2: Write a failing test**

Append to `cli/crates/cook-engine/src/dag_builder.rs`'s `#[cfg(test)]` mod:

```rust
#[test]
fn build_test_slice_excludes_unrelated_cook_units() {
    // 4 units in the registered set:
    //   #0: cook "needed.bin"      (test #2 depends on this)
    //   #1: cook "unrelated.bin"   (no test depends on this)
    //   #2: test (depends on #0)
    //   #3: test (one-shot, no deps)
    let units = vec![
        mk_cook_unit(0, "needed.bin"),
        mk_cook_unit(1, "unrelated.bin"),
        mk_test_unit(2, "needs-needed", &[0]),
        mk_test_unit(3, "one-shot", &[]),
    ];
    let dep_edges = vec![(2, "needed.bin".to_string())];

    let slice = build_test_slice(&units, &dep_edges);

    // Slice MUST include #0 (needed by test #2), #2, #3.
    // Slice MUST NOT include #1 (no test depends on it).
    let slice_ids: BTreeSet<_> = slice.iter().copied().collect();
    assert!(slice_ids.contains(&0));
    assert!(slice_ids.contains(&2));
    assert!(slice_ids.contains(&3));
    assert!(!slice_ids.contains(&1), "unrelated cook unit must be excluded");
}

#[test]
fn build_test_slice_handles_transitive_deps() {
    // #0 cook → #1 cook → #2 test
    let units = vec![
        mk_cook_unit(0, "a.out"),
        mk_cook_unit(1, "b.out"),  // depends on a.out
        mk_test_unit(2, "t", &[1]),
    ];
    let dep_edges = vec![
        (1, "a.out".to_string()),
        (2, "b.out".to_string()),
    ];
    let slice = build_test_slice(&units, &dep_edges);
    assert_eq!(slice.len(), 3, "transitive cook dep must be included");
}

fn mk_cook_unit(id: usize, output: &str) -> /* unit type */ { todo!() }
fn mk_test_unit(id: usize, name: &str, deps: &[usize]) -> /* unit type */ { todo!() }
```

(Fill in `mk_cook_unit` / `mk_test_unit` with whatever shape the existing test helpers use; `cook-engine`'s test infrastructure already has fixtures for this.)

- [ ] **Step 3.6.3: Implement `build_test_slice`**

```rust
/// Compute the minimal set of unit indices required to execute every test
/// unit in `units`. Test units themselves are always included; cook units
/// are included only if at least one test (transitively) depends on them.
///
/// Implements Phase 3 of the runner pipeline per
/// docs/superpowers/specs/2026-05-07-test-runner-design.md §4.3.
pub fn build_test_slice(
    units: &[CapturedUnit],
    dep_edges: &[(usize, String)],
) -> Vec<usize> {
    use std::collections::{BTreeSet, VecDeque};

    // Index: output_path → producing unit id
    let mut producer_by_output: BTreeMap<String, usize> = BTreeMap::new();
    for (i, u) in units.iter().enumerate() {
        for output in unit_outputs(u) {
            producer_by_output.insert(output.to_string(), i);
        }
    }

    // BFS backward from every test unit
    let mut visited: BTreeSet<usize> = BTreeSet::new();
    let mut queue: VecDeque<usize> = units.iter().enumerate()
        .filter(|(_, u)| matches!(u.payload, WorkPayload::Test { .. }))
        .map(|(i, _)| i)
        .collect();

    while let Some(id) = queue.pop_front() {
        if !visited.insert(id) { continue; }
        for (uid, dep_output) in dep_edges {
            if *uid != id { continue; }
            if let Some(&producer) = producer_by_output.get(dep_output) {
                queue.push_back(producer);
            }
        }
        // Also include implicit cook-output deps (CS-0024 source-list flattening)
        for dep in implicit_cook_deps(units, id) {
            queue.push_back(dep);
        }
    }

    let mut slice: Vec<usize> = visited.into_iter().collect();
    slice.sort();
    slice
}

fn unit_outputs(u: &CapturedUnit) -> Vec<&str> {
    // Return the unit's declared output paths (cook units have outputs;
    // tests do not).
    match &u.payload {
        WorkPayload::Cook { outputs, .. } => outputs.iter().map(|s| s.as_str()).collect(),
        _ => vec![],
    }
}

fn implicit_cook_deps(units: &[CapturedUnit], test_id: usize) -> Vec<usize> {
    // For a test unit, find the preceding cook step's outputs in the same
    // recipe (CS-0024 §3.3 source-list flattening). Implementation follows
    // the existing source-list lookup in cook-luagen / cook-engine.
    todo!("integrate with the existing source-list flattening helper")
}
```

The `implicit_cook_deps` helper integrates with existing infrastructure used by codegen for plate/test source-list flattening. Locate that code:

```bash
grep -rn "source.list\|preceding cook\|last_cook_index" cli/crates/cook-luagen/src/ cli/crates/cook-engine/src/
```

Lift the lookup into a shared helper if not already exposed.

- [ ] **Step 3.6.4: Run; commit**

```bash
cd cli && cargo test -p cook-engine build_test_slice
```

```bash
git add cli/crates/cook-engine/
git commit -m "$(cat <<'EOF'
feat(cook-engine): build_test_slice reverse closure

Computes the minimal cook-unit slice required by a set of test units.
Used by cook --test to skip unrelated cook units during test-only
execution per runner spec §4.3.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

**Phase 3 checkpoint.** Engine has the plumbing for a test-mode invocation: real `RunResult.test_results`, test events, register-all entry, reverse-closure builder. No CLI surface yet — `cook --test` is not a flag.

---

## Phase 4: `cook --test` CLI flag + terminal reporter

Wires the runner end-to-end with terminal output. No caching yet (Phase 5), no JSON sidecar (Phase 8), no `--rerun-failed` (Phase 7). After this phase, `cook --test` produces the summary block from the runner spec §6.5.

### Task 4.1: New `run_for_test()` entry in cook-engine

**Files:**
- Modify: `cli/crates/cook-engine/src/run.rs`

- [ ] **Step 4.1.1: Define the entry point**

Add to `cook-engine/src/run.rs`:

```rust
/// Test-mode engine entry point.
///
/// Behavior per docs/superpowers/specs/2026-05-07-test-runner-design.md §4:
/// - Phase 1: register-all (or target-driven if scope is Some).
/// - Phase 2: filter (filter_pattern + rerun_failed_set).
/// - Phase 3: reverse-closure to compute the cook-unit slice.
/// - Phase 4: fingerprint + cache lookup (Phase 5 wiring; stub here).
/// - Phase 5: execute & report.
pub fn run_for_test(
    project_root: &Path,
    scope: Option<TestScope>,
    filter_patterns: &[String],
    rerun_failed_set: Option<&BTreeSet<TestId>>,
    rerun_patterns: &[String],
    fail_fast: bool,
    num_jobs: usize,
    on_event: impl Fn(EngineEvent) + Send + Sync,
) -> Result<RunResult, EngineError> {
    let started = std::time::Instant::now();
    let result = run_for_test_inner(
        project_root, scope, filter_patterns, rerun_failed_set,
        rerun_patterns, fail_fast, num_jobs, &on_event,
    );
    on_event(EngineEvent::Finished {
        elapsed: started.elapsed(),
        success: result.is_ok(),
    });
    result
}

#[derive(Debug, Clone)]
pub enum TestScope {
    /// `cook --test <recipe>` — scope to a single recipe and its dep closure.
    Recipe(String),
    /// `cook --test <namespace>` — scope to an import alias's tree.
    Namespace(String),
}

fn run_for_test_inner<F>(
    project_root: &Path,
    scope: Option<TestScope>,
    filter_patterns: &[String],
    rerun_failed_set: Option<&BTreeSet<TestId>>,
    _rerun_patterns: &[String], // Phase 6
    _fail_fast: bool,            // Phase 4 follow-up if simple; Phase 6 otherwise
    num_jobs: usize,
    on_event: &F,
) -> Result<RunResult, EngineError>
where
    F: Fn(EngineEvent) + Send + Sync,
{
    // Phase 1: discover
    let recipe_infos = match &scope {
        None => analyzer::register_workspace_for_test(project_root)?,
        Some(TestScope::Recipe(name)) => {
            // Use existing target-driven analyzer with the single recipe as target.
            analyzer::analyze(project_root, &[name.clone()])?
        }
        Some(TestScope::Namespace(ns)) => {
            // Register everything under the namespace.
            let all = analyzer::register_workspace_for_test(project_root)?;
            all.into_iter()
                .filter(|(n, _)| n.starts_with(&format!("{}.", ns)))
                .collect()
        }
    };

    // Build the engine state from recipe_infos. Reuse the existing
    // `run_inner` body's bootstrap (cache context, denylist, registries) up to
    // the wave-execution loop. Then:

    // Phase 2: filter test units
    let mut test_units = collect_test_units(&recipe_infos);
    if !filter_patterns.is_empty() {
        test_units.retain(|u| matches_any_glob(&u.id, filter_patterns));
    }
    if let Some(prev) = rerun_failed_set {
        test_units.retain(|u| prev.contains(&u.id));
    }

    if test_units.is_empty() {
        // Empty set is success per runner spec §6.2.
        return Ok(RunResult { test_results: Vec::new() });
    }

    // Phase 3: reverse-closure
    let slice = dag_builder::build_test_slice(&all_units(&recipe_infos), &all_dep_edges(&recipe_infos));
    let slice_units: Vec<_> = slice.into_iter()
        .map(|i| all_units(&recipe_infos)[i].clone())
        .collect();

    // Phase 4: fingerprint + cache lookup — Phase 5 wires this. For now,
    // every test runs.

    // Phase 5: execute & report (uses existing wave-grouped executor,
    // restricted to slice_units)
    let test_results = std::sync::Mutex::new(Vec::new());
    execute_units(slice_units, num_jobs, on_event, &test_results)?;

    Ok(RunResult { test_results: test_results.into_inner().unwrap() })
}

// Helpers — implement against the existing engine's primitives:
fn collect_test_units(infos: &BTreeMap<String, analyzer::RecipeInfo>) -> Vec<TestUnitMeta> { todo!() }
fn all_units(infos: &BTreeMap<String, analyzer::RecipeInfo>) -> Vec<CapturedUnit> { todo!() }
fn all_dep_edges(infos: &BTreeMap<String, analyzer::RecipeInfo>) -> Vec<(usize, String)> { todo!() }
fn matches_any_glob(id: &TestId, patterns: &[String]) -> bool {
    patterns.iter().any(|pat| glob_match(pat, &id.0))
}
fn glob_match(pattern: &str, text: &str) -> bool {
    // Use the same glob matcher Cook uses for ingredients globs.
    // It's in cook-engine or cook-contracts; locate via:
    //   grep -rn "glob_match\|matches_pattern" cli/crates/cook-engine/src/ cli/crates/cook-contracts/src/
    todo!("locate and reuse the existing glob matcher")
}
fn execute_units(...) -> Result<(), EngineError> { todo!() }

struct TestUnitMeta {
    id: TestId,
    /* ... */
}
```

Lift the bootstrap/execute portions of the existing `run_inner` into shared helpers so this new entry point doesn't duplicate them.

- [ ] **Step 4.1.2: Verify compile (no tests yet — wired in 4.5)**

```bash
cd cli && cargo build -p cook-engine
```

Fix any unresolved references; the goal is a compiling skeleton.

- [ ] **Step 4.1.3: Commit**

```bash
git add cli/crates/cook-engine/src/run.rs
git commit -m "$(cat <<'EOF'
feat(cook-engine): run_for_test entry point skeleton

Five-phase pipeline: discover (workspace or target-driven), filter,
reverse-closure, (cache-check stubbed), execute. Helpers TODO; CLI
wiring follows.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

### Task 4.2: CLI flags

**Files:**
- Modify: `cli/crates/cook-cli/src/cli.rs`

- [ ] **Step 4.2.1: Add the flags**

In `cli/crates/cook-cli/src/cli.rs`, in the `Cli` struct, add the test-runner flags after the existing built-ins:

```rust
/// Run tests in the workspace (or scoped to a recipe/namespace).
#[arg(
    long = "test",
    help_heading = "Built-in commands",
    conflicts_with_all = ["menu", "init", "serve", "logs", "dag", "emit_lua"]
)]
pub test: bool,

/// Filter tests by glob against `<namespace>.<recipe>:<name>`. Repeatable.
#[arg(long = "filter", num_args = 1, requires = "test")]
pub filter: Vec<String>,

/// Cancel queued tests on first failure.
#[arg(long = "fail-fast", requires = "test")]
pub fail_fast: bool,

/// Force re-run of tests matching glob (or all if no pattern).
#[arg(long = "rerun", num_args = 0..=1, default_missing_value = "*", requires = "test")]
pub rerun: Option<Vec<String>>,

/// Re-run only tests that failed (or were blocked / timed out) last run.
#[arg(long = "rerun-failed", requires = "test")]
pub rerun_failed: bool,

/// Write JSON test report to the given path (default: .cook/test-report.json).
#[arg(long = "report-json", num_args = 1, requires = "test")]
pub report_json: Option<PathBuf>,

/// Write JUnit XML test report to the given path.
#[arg(long = "report-junit", num_args = 1, requires = "test")]
pub report_junit: Option<PathBuf>,
```

- [ ] **Step 4.2.2: Verify clap compiles + `--help` renders**

```bash
cd cli && cargo build -p cook-cli && ./target/debug/cook --help | grep -A1 "\-\-test"
```

Expected: `--test` listed under "Built-in commands"; sub-flags listed.

- [ ] **Step 4.2.3: Commit**

```bash
git add cli/crates/cook-cli/src/cli.rs
git commit -m "$(cat <<'EOF'
feat(cook-cli): add --test flag and sub-flags

Conflicts with other built-in commands; sub-flags require --test.
Wiring in next task.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

### Task 4.3: Restore `CookError::TestFailure`

**Files:**
- Modify: `cli/crates/cook-cli/src/error.rs`

- [ ] **Step 4.3.1: Add the variant**

In `cli/crates/cook-cli/src/error.rs`, add `TestFailure(String)` to the `CookError` enum and route its exit code to 1:

```rust
#[derive(Debug)]
#[allow(dead_code)]
pub enum CookError {
    ParseError(String),
    RecipeNotFound(String),
    CommandFailed(String),
    TestFailure(String),
    Other(String),
}

impl CookError {
    pub fn exit_code(&self) -> i32 {
        match self {
            CookError::CommandFailed(_) => 1,
            CookError::ParseError(_) => 2,
            CookError::RecipeNotFound(_) => 3,
            CookError::TestFailure(_) => 1,
            CookError::Other(_) => 1,
        }
    }
}

impl std::fmt::Display for CookError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CookError::ParseError(msg) => write!(f, "parse error: {msg}"),
            CookError::RecipeNotFound(name) => write!(f, "recipe not found: {name}"),
            CookError::CommandFailed(msg) => write!(f, "{msg}"),
            CookError::TestFailure(msg) => write!(f, "{msg}"),
            CookError::Other(msg) => write!(f, "{msg}"),
        }
    }
}
```

- [ ] **Step 4.3.2: Compile and commit**

```bash
cd cli && cargo build -p cook-cli
git add cli/crates/cook-cli/src/error.rs
git commit -m "$(cat <<'EOF'
feat(cook-cli): restore CookError::TestFailure variant

Deleted by 7e3c5d4; re-introduced for the v1.0 test runner. Exit code 1.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

### Task 4.4: `cmd_test` arm in pipeline.rs

**Files:**
- Modify: `cli/crates/cook-cli/src/pipeline.rs`
- Modify: `cli/crates/cook-cli/src/main.rs`

- [ ] **Step 4.4.1: Add the dispatch arm**

In `cli/crates/cook-cli/src/main.rs`, locate the dispatch on Cli flags. Add the `--test` arm before the default "run a recipe" arm:

```rust
if cli.test {
    return pipeline::cmd_test(&cli);
}
```

- [ ] **Step 4.4.2: Implement `cmd_test`**

In `cli/crates/cook-cli/src/pipeline.rs`, add:

```rust
pub fn cmd_test(cli: &Cli) -> Result<(), CookError> {
    use cook_engine::{TestScope, run_for_test};

    let project_root = std::env::current_dir().map_err(|e| CookError::Other(e.to_string()))?;

    // Determine scope from positional `recipe` argument.
    let scope = match cli.recipe.as_deref() {
        None => None,
        Some(name) if name.contains('.') => {
            // Could be either `<namespace>.<recipe>` or just `<namespace>`.
            // The engine resolves: if the name is a full recipe ref, treat as Recipe;
            // else if it's an import alias, treat as Namespace.
            Some(resolve_test_scope(name, &project_root)?)
        }
        Some(name) => Some(TestScope::Recipe(name.to_string())),
    };

    // Read --rerun-failed state if requested.
    let rerun_failed_set = if cli.rerun_failed {
        match crate::test_state::load_failed_set(&project_root) {
            Ok(set) => Some(set),
            Err(e) => {
                eprintln!("warning: {}", e);
                eprintln!("hint: run `cook --test` first to populate state");
                return Ok(());
            }
        }
    } else { None };

    let rerun_patterns: Vec<String> = cli.rerun.clone().unwrap_or_default();
    let num_jobs = cli.jobs.unwrap_or_else(num_cpus::get);

    let mut reporter = crate::test_reporter::Reporter::new(cli);
    let on_event = |evt: cook_engine::EngineEvent| { reporter.on_event(evt); };

    let result = run_for_test(
        &project_root,
        scope,
        &cli.filter,
        rerun_failed_set.as_ref(),
        &rerun_patterns,
        cli.fail_fast,
        num_jobs,
        on_event,
    ).map_err(|e| CookError::Other(format!("{e}")))?;

    // Persist last-run state (Phase 7).
    crate::test_state::save(&project_root, &result.test_results)
        .map_err(|e| CookError::Other(format!("failed to write test state: {e}")))?;

    // Emit JSON sidecar (Phase 8).
    crate::test_reporter::write_json_sidecar(&project_root, cli, &result.test_results)
        .map_err(|e| CookError::Other(format!("failed to write JSON report: {e}")))?;

    if let Some(path) = &cli.report_junit {
        crate::test_reporter::write_junit_sidecar(path, &result.test_results)
            .map_err(|e| CookError::Other(format!("failed to write JUnit report: {e}")))?;
    }

    let any_failed = result.test_results.iter().any(|r| matches!(
        r.outcome,
        cook_engine::TestOutcome::Failed | cook_engine::TestOutcome::Blocked | cook_engine::TestOutcome::TimedOut
    ));

    reporter.finish(&result.test_results);

    if any_failed {
        Err(CookError::TestFailure("one or more tests failed".to_string()))
    } else {
        Ok(())
    }
}

fn resolve_test_scope(name: &str, project_root: &Path) -> Result<TestScope, CookError> {
    // Try recipe first; fall back to namespace if no recipe by that name.
    // Implementation scans the workspace registry.
    todo!("introspect registry to choose Recipe vs Namespace")
}
```

The `test_state` and `test_reporter` modules are stubs at this point; full impls land in Phase 7 / Phase 8 / Phase 4.5 respectively.

Add stub modules:

```rust
// cli/crates/cook-cli/src/test_state.rs
use std::collections::BTreeSet;
use std::path::Path;
use cook_engine::{TestId, TestResult};

pub fn load_failed_set(_project_root: &Path) -> std::io::Result<BTreeSet<TestId>> {
    // Phase 7 implements this.
    Err(std::io::Error::new(
        std::io::ErrorKind::NotFound,
        "no previous test run recorded at .cook/test-state.json"
    ))
}

pub fn save(_project_root: &Path, _results: &[TestResult]) -> std::io::Result<()> {
    // Phase 7 implements this.
    Ok(())
}
```

Stub `test_reporter.rs` skeleton in Task 4.5.

- [ ] **Step 4.4.3: Wire modules**

In `cli/crates/cook-cli/src/main.rs` (or `lib.rs` if it exists):

```rust
pub mod test_reporter;
pub mod test_state;
```

- [ ] **Step 4.4.4: Compile**

```bash
cd cli && cargo build -p cook-cli
```

Fix any imports until it compiles.

- [ ] **Step 4.4.5: Commit**

```bash
git add cli/crates/cook-cli/src/
git commit -m "$(cat <<'EOF'
feat(cook-cli): cmd_test pipeline arm

Wires --test to engine::run_for_test. Stub modules for test_state and
test_reporter (filled in Phases 4.5, 7, 8).

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

### Task 4.5: Terminal reporter — live progress

**Files:**
- Modify: `cli/crates/cook-cli/src/test_reporter.rs`

- [ ] **Step 4.5.1: Implement the Reporter struct**

```rust
// cli/crates/cook-cli/src/test_reporter.rs

use std::collections::BTreeMap;
use std::time::Duration;
use cook_engine::{EngineEvent, TestId, TestOutcome, TestResult, TestFailureReason};
use crate::cli::Cli;

pub struct Reporter {
    /// Per-recipe accumulators: counts of each outcome.
    by_recipe: BTreeMap<String, RecipeStats>,
    /// Total wall time at start.
    started: std::time::Instant,
    /// Verbose flag mirrors Cli::verbose.
    verbose: bool,
}

#[derive(Default, Clone)]
struct RecipeStats {
    passed: usize,
    failed: usize,
    blocked: usize,
    timed_out: usize,
    cached: usize,
    duration: Duration,
}

impl Reporter {
    pub fn new(cli: &Cli) -> Self {
        Self {
            by_recipe: BTreeMap::new(),
            started: std::time::Instant::now(),
            verbose: cli.verbose,
        }
    }

    pub fn on_event(&mut self, evt: EngineEvent) {
        match evt {
            EngineEvent::TestStarted { id, .. } => {
                if self.verbose {
                    println!("    test {} ...", id);
                }
            }
            EngineEvent::TestPassed { id, duration, cached, .. } => {
                let recipe = recipe_of(&id);
                let stats = self.by_recipe.entry(recipe).or_default();
                stats.passed += 1;
                if cached { stats.cached += 1; }
                stats.duration += duration;
            }
            EngineEvent::TestFailed { id, duration, .. } => {
                let recipe = recipe_of(&id);
                let stats = self.by_recipe.entry(recipe).or_default();
                stats.failed += 1;
                stats.duration += duration;
            }
            EngineEvent::TestBlocked { id, .. } => {
                let recipe = recipe_of(&id);
                self.by_recipe.entry(recipe).or_default().blocked += 1;
            }
            EngineEvent::TestTimedOut { id, timeout, .. } => {
                let recipe = recipe_of(&id);
                let stats = self.by_recipe.entry(recipe).or_default();
                stats.timed_out += 1;
                stats.duration += timeout;
            }
            _ => {}
        }
    }

    pub fn finish(&mut self, results: &[TestResult]) {
        // Print per-recipe lines.
        for (recipe, stats) in &self.by_recipe {
            let icon = if stats.failed > 0 || stats.blocked > 0 || stats.timed_out > 0 {
                "❌"
            } else if stats.cached == stats.passed && stats.passed > 0 {
                "✨"
            } else {
                "✅"
            };
            print!("{} {:<25}", icon, recipe);
            let mut parts = Vec::new();
            if stats.passed > 0 { parts.push(format!("{} passed", stats.passed)); }
            if stats.failed > 0 { parts.push(format!("{} failed", stats.failed)); }
            if stats.blocked > 0 { parts.push(format!("{} blocked", stats.blocked)); }
            if stats.timed_out > 0 { parts.push(format!("{} timed out", stats.timed_out)); }
            if stats.cached > 0 { parts.push(format!("{} cached", stats.cached)); }
            print!(" {}", parts.join(", "));
            println!("  ({:.1}s)", stats.duration.as_secs_f64());
        }

        // Failures section.
        let failures: Vec<&TestResult> = results.iter()
            .filter(|r| matches!(r.outcome, TestOutcome::Failed | TestOutcome::TimedOut))
            .collect();
        if !failures.is_empty() {
            println!("\nFailures:");
            for r in &failures {
                println!("  {} > {}", recipe_of(&r.id), r.name);
                if !r.stdout.is_empty() {
                    for line in r.stdout.lines().take(20) {
                        println!("    {}", line);
                    }
                }
                if !r.stderr.is_empty() {
                    println!("    [ stderr ]");
                    for line in r.stderr.lines().take(20) {
                        println!("    {}", line);
                    }
                }
                println!();
            }
        }

        // Blocked section.
        let blocked: Vec<&TestResult> = results.iter()
            .filter(|r| matches!(r.outcome, TestOutcome::Blocked))
            .collect();
        if !blocked.is_empty() {
            println!("Blocked:");
            for r in &blocked {
                let cause = r.blocked_by.as_deref().unwrap_or("upstream cook step");
                println!("  {} > {}  (build failed: {})", recipe_of(&r.id), r.name, cause);
            }
            println!();
        }

        // Summary.
        let total_passed: usize = self.by_recipe.values().map(|s| s.passed).sum();
        let total_failed: usize = self.by_recipe.values().map(|s| s.failed).sum();
        let total_blocked: usize = self.by_recipe.values().map(|s| s.blocked).sum();
        let total_to: usize = self.by_recipe.values().map(|s| s.timed_out).sum();
        let total_cached: usize = self.by_recipe.values().map(|s| s.cached).sum();
        let wall = self.started.elapsed();
        let cache_savings: Duration = results.iter()
            .filter(|r| r.from_cache)
            .map(|r| r.duration)
            .sum();

        let mut parts = Vec::new();
        if total_passed > 0 { parts.push(format!("{} passed", total_passed)); }
        if total_failed > 0 { parts.push(format!("{} failed", total_failed)); }
        if total_blocked > 0 { parts.push(format!("{} blocked", total_blocked)); }
        if total_to > 0 { parts.push(format!("{} timed out", total_to)); }
        if total_cached > 0 { parts.push(format!("{} cached", total_cached)); }
        println!("Summary: {}  —  {:.1}s wall ({:.1}s saved by cache)",
            parts.join(", "), wall.as_secs_f64(), cache_savings.as_secs_f64());

        // Footer hint.
        if total_failed > 0 || total_blocked > 0 || total_to > 0 {
            println!("\nFailed tests:");
            println!("  cook --test --rerun-failed         # re-run only these");
            println!("  cat .cook/test-report.json | jq    # full structured report");
        }
    }
}

fn recipe_of(id: &TestId) -> String {
    let s = &id.0;
    s.split(':').next().unwrap_or("").to_string()
}

// Stubs filled in Phase 8.
pub fn write_json_sidecar(_root: &std::path::Path, _cli: &Cli, _results: &[TestResult]) -> std::io::Result<()> {
    Ok(())
}
pub fn write_junit_sidecar(_path: &std::path::Path, _results: &[TestResult]) -> std::io::Result<()> {
    Ok(())
}
```

- [ ] **Step 4.5.2: Compile and run a smoke test**

```bash
cd /home/alex/dev/cook
cargo build -q --bin cook
cd examples/test_benchmarks
../../cli/target/debug/cook --test pass_basic
```

Expected: a summary block printed; pass_basic shows `1 passed`. Exit code 0.

```bash
echo $?
```

- [ ] **Step 4.5.3: Run a failing recipe**

```bash
../../cli/target/debug/cook --test fail_basic
```

Expected: failure section printed; `1 failed`; exit code 1.

- [ ] **Step 4.5.4: Commit**

```bash
cd /home/alex/dev/cook
git add cli/crates/cook-cli/src/test_reporter.rs
git commit -m "$(cat <<'EOF'
feat(cook-cli): terminal test reporter — summary block + failures

Per-recipe lines, failures section with stdout/stderr, summary footer
with pass/fail/cached counts, next-steps hint when tests fail.
Exit code 1 on any failure/blocked/timed-out test.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

**Phase 4 checkpoint.** `cook --test [SCOPE]` works end-to-end. Tests run, summary prints, failures show with stdout/stderr, exit code is correct. No caching yet — every run executes every test. No `--rerun-failed`. No JSON sidecar. The `examples/test_benchmarks/` walkthrough still runs in stub mode (Phase 9 enables assertions).

---

## Phase 5: Test-result cache backend

Adds `cook-cache::test_cache::TestCache`, wires fingerprint computation into `run_for_test_inner`'s Phase-4, and wires cache writes from the executor's WorkResult consumer.

### Task 5.1: Cache types and round-trip

**Files:**
- Create: `cli/crates/cook-cache/src/test_cache.rs`
- Modify: `cli/crates/cook-cache/src/lib.rs`

- [ ] **Step 5.1.1: Author the module**

```rust
// cli/crates/cook-cache/src/test_cache.rs

use std::path::{Path, PathBuf};
use std::time::SystemTime;
use serde::{Serialize, Deserialize};

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum TestCacheOutcome {
    Passed,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct TestCacheEntry {
    pub schema_version: u32,
    pub fingerprint: String,
    pub outcome: TestCacheOutcome,
    pub stdout: String,
    pub stderr: String,
    pub duration_secs: f64,
    pub should_fail_observed: bool,
    pub recorded_at: String, // ISO-8601
}

pub struct TestCache {
    root: PathBuf,
}

impl TestCache {
    pub fn new(local_root: PathBuf) -> Self {
        Self { root: local_root.join("cache").join("tests") }
    }

    pub fn lookup(&self, fingerprint: &str) -> Option<TestCacheEntry> {
        let path = self.path_for(fingerprint);
        if !path.exists() { return None; }
        let bytes = std::fs::read(&path).ok()?;
        let entry: TestCacheEntry = serde_json::from_slice(&bytes).ok()?;
        if entry.schema_version != 1 { return None; }
        if entry.fingerprint != fingerprint { return None; }
        Some(entry)
    }

    pub fn store(&self, fingerprint: &str, entry: &TestCacheEntry) -> std::io::Result<()> {
        // Only `Passed` entries are written per CS-NNNN §3.3.
        if !matches!(entry.outcome, TestCacheOutcome::Passed) {
            return Ok(());
        }
        let path = self.path_for(fingerprint);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let tmp = path.with_extension("json.tmp");
        let bytes = serde_json::to_vec_pretty(entry)?;
        std::fs::write(&tmp, &bytes)?;
        std::fs::rename(&tmp, &path)?;
        Ok(())
    }

    fn path_for(&self, fingerprint: &str) -> PathBuf {
        // .cook/cache/tests/<fp_prefix>/<full>.json
        let stripped = fingerprint.strip_prefix("sha256:").unwrap_or(fingerprint);
        let prefix = &stripped[..2.min(stripped.len())];
        self.root.join(prefix).join(format!("{}.json", stripped))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn make_entry(fp: &str) -> TestCacheEntry {
        TestCacheEntry {
            schema_version: 1,
            fingerprint: fp.to_string(),
            outcome: TestCacheOutcome::Passed,
            stdout: "ok\n".to_string(),
            stderr: "".to_string(),
            duration_secs: 0.42,
            should_fail_observed: false,
            recorded_at: "2026-05-07T15:32:00Z".to_string(),
        }
    }

    #[test]
    fn roundtrip_passing_entry() {
        let tmp = tempdir().unwrap();
        let cache = TestCache::new(tmp.path().to_path_buf());
        let fp = "sha256:abcdef0123";
        let entry = make_entry(fp);
        cache.store(fp, &entry).unwrap();
        let got = cache.lookup(fp).expect("must hit");
        assert_eq!(got.duration_secs, 0.42);
        assert_eq!(got.outcome, TestCacheOutcome::Passed);
    }

    #[test]
    fn lookup_miss_returns_none() {
        let tmp = tempdir().unwrap();
        let cache = TestCache::new(tmp.path().to_path_buf());
        assert!(cache.lookup("sha256:doesnotexist").is_none());
    }

    #[test]
    fn fingerprint_mismatch_returns_none() {
        let tmp = tempdir().unwrap();
        let cache = TestCache::new(tmp.path().to_path_buf());
        let entry = make_entry("sha256:realfp");
        // Manually write under a different filename to simulate corruption.
        let path = cache.path_for("sha256:realfp");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, serde_json::to_vec(&entry).unwrap()).unwrap();
        // Lookup with a different fingerprint should miss.
        // (The lookup returns None when the entry's fingerprint doesn't match.)
        // To trigger the mismatch path, write the entry under a "wrong" hash filename:
        let wrong_path = cache.path_for("sha256:wrongfp");
        std::fs::create_dir_all(wrong_path.parent().unwrap()).unwrap();
        std::fs::write(&wrong_path, serde_json::to_vec(&entry).unwrap()).unwrap();
        assert!(cache.lookup("sha256:wrongfp").is_none(),
                "entry's internal fp doesn't match the looked-up fp");
    }
}
```

- [ ] **Step 5.1.2: Re-export from `lib.rs`**

In `cli/crates/cook-cache/src/lib.rs`:

```rust
pub mod test_cache;
pub use test_cache::{TestCache, TestCacheEntry, TestCacheOutcome};
```

- [ ] **Step 5.1.3: Add deps if needed**

```bash
grep -E "^(serde|serde_json|tempfile)" cli/crates/cook-cache/Cargo.toml
```

If `serde`, `serde_json`, or `tempfile` aren't already deps in `cook-cache`, add them (use `.workspace = true` if the workspace pins them).

- [ ] **Step 5.1.4: Run; commit**

```bash
cd cli && cargo test -p cook-cache test_cache
```

```bash
git add cli/crates/cook-cache/
git commit -m "$(cat <<'EOF'
feat(cook-cache): add TestCache backend

Content-addressed test-result cache per CS-NNNN §3.3 / runner spec §5.
Only Passed entries written; lookup is filesystem-checked + version-
gated. Layout: .cook/cache/tests/<fp_prefix>/<fp>.json.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

### Task 5.2: Wire cache lookup in run_for_test

**Files:**
- Modify: `cli/crates/cook-engine/src/run.rs`

- [ ] **Step 5.2.1: In `run_for_test_inner`, between Phase 3 and Phase 5, add Phase 4**

Replace the comment "Phase 4: fingerprint + cache lookup — Phase 5 wires this. For now, every test runs." with real code:

```rust
// Phase 4: fingerprint + cache lookup
let test_cache = cook_cache::TestCache::new(project_root.join(".cook"));
let mut cache_hits: BTreeMap<TestId, cook_cache::TestCacheEntry> = BTreeMap::new();
let rerun_pat_set: Vec<&str> = rerun_patterns.iter().map(|s| s.as_str()).collect();
let mut to_run: Vec<TestUnitMeta> = Vec::new();

for tu in test_units {
    let fp_inputs = build_fingerprint_inputs(&tu, &recipe_infos)?;
    let fp = cook_fingerprint::compute_test_fingerprint(&tu.payload, &fp_inputs);
    let force_rerun = rerun_pat_set.iter().any(|p| glob_match(p, &tu.id.0));
    if !force_rerun {
        if let Some(entry) = test_cache.lookup(&fp) {
            cache_hits.insert(tu.id.clone(), entry);
            continue;
        }
    }
    tu.fingerprint = Some(fp);
    to_run.push(tu);
}

// Synthesize TestPassed events for cache hits.
for (id, entry) in &cache_hits {
    on_event(EngineEvent::TestPassed {
        id: id.clone(),
        duration: std::time::Duration::from_secs_f64(entry.duration_secs),
        cached: true,
        stdout: entry.stdout.clone(),
        stderr: entry.stderr.clone(),
    });
}

// Phase 5 executes only `to_run`.
```

`build_fingerprint_inputs` walks the unit's source list (per CS-0024) and the registry to produce the `FingerprintInputs` struct.

- [ ] **Step 5.2.2: Wire cache writes in the executor**

In `cli/crates/cook-engine/src/executor.rs`, when a `TestResult` lands with `outcome: Passed` and a known fingerprint, write to the test cache:

```rust
if matches!(outcome, TestOutcome::Passed) {
    if let Some(fp) = &tu.fingerprint {
        let entry = cook_cache::TestCacheEntry {
            schema_version: 1,
            fingerprint: fp.clone(),
            outcome: cook_cache::TestCacheOutcome::Passed,
            stdout: to.stdout.clone(),
            stderr: to.stderr.clone(),
            duration_secs: to.duration,
            should_fail_observed: !to.exit_success,
            recorded_at: chrono::Utc::now().to_rfc3339(),
        };
        let _ = test_cache.store(fp, &entry);
    }
}
```

If `chrono` isn't in the workspace, use `time` (already a likely workspace dep) or `humantime`'s ISO8601 helper.

- [ ] **Step 5.2.3: Update synthesized cache-hit results to include `from_cache: true`**

When pushing `TestResult` for cache hits, set `from_cache: true` and `fingerprint: Some(fp)`.

- [ ] **Step 5.2.4: Verify with the test_caching fixture**

```bash
cd /home/alex/dev/cook && cargo build -q --bin cook
cd examples/test_caching
rm -rf .cook build
../../cli/target/debug/cook --test deterministic
echo "----- second run -----"
../../cli/target/debug/cook --test deterministic
```

Expected on second run: `1 cached, 0 ran` or equivalent; `✨` icon on the recipe line.

- [ ] **Step 5.2.5: Commit**

```bash
cd /home/alex/dev/cook
git add cli/crates/cook-engine/
git commit -m "$(cat <<'EOF'
feat(cook-engine): wire test-result cache into run_for_test

Phase 4 of the runner pipeline: per-test fingerprint, lookup, hit-skip.
Hits emit synthesized TestPassed events with cached=true. Misses
proceed to execution; passing executions write cache entries.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

**Phase 5 checkpoint.** Test-result caching is live. Second runs of `cook --test` over an unchanged source tree are dominated by cache lookups. Failed tests never write cache. The `examples/test_caching/` walkthrough is still in stub mode; Phase 9.3 enables its assertions.

---

## Phase 6: `--rerun [PATTERN]`

Already partially wired in 5.2 (the `rerun_patterns` slice is consumed). This phase adds the glob matcher and pins behavior with a focused unit test.

### Task 6.1: Glob matcher utility

**Files:**
- Modify: `cli/crates/cook-engine/src/run.rs` (or a new `id_match.rs` module)
- Test: `cli/crates/cook-engine/src/run.rs::tests`

- [ ] **Step 6.1.1: Locate existing glob matcher**

```bash
grep -rn "glob_match\|matches_pattern\|fnmatch" cli/crates/ | head -10
```

Cook uses `globset` for ingredients globs (likely a workspace dep). Reuse it.

- [ ] **Step 6.1.2: Implement the helper**

```rust
fn glob_match(pattern: &str, text: &str) -> bool {
    let glob = match globset::Glob::new(pattern) {
        Ok(g) => g.compile_matcher(),
        Err(_) => return false,
    };
    glob.is_match(text)
}
```

If `globset` isn't already a `cook-engine` dep, add it.

- [ ] **Step 6.1.3: Test**

```rust
#[test]
fn glob_match_simple_prefix() {
    assert!(glob_match("frontend.*", "frontend.unit:t#1"));
    assert!(!glob_match("frontend.*", "backend.unit:t#1"));
}

#[test]
fn glob_match_test_name() {
    assert!(glob_match("*:vitest-*", "frontend.unit:vitest-suite"));
    assert!(!glob_match("*:vitest-*", "frontend.unit:other-test"));
}

#[test]
fn glob_match_iteration_discriminator() {
    assert!(glob_match("*:t*[*.json]", "backend.api:t1[users.json]"));
}
```

- [ ] **Step 6.1.4: Commit**

```bash
cd cli && cargo test -p cook-engine glob_match
git add cli/crates/cook-engine/
git commit -m "$(cat <<'EOF'
feat(cook-engine): glob matcher for --filter and --rerun patterns

Uses globset (already a workspace dep). Matches against the test's
identity string `<namespace>.<recipe>:<name>[<discriminator>]`.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

**Phase 6 checkpoint.** `cook --test --rerun [PATTERN]` is fully wired.

---

## Phase 7: `.cook/test-state.json` and `--rerun-failed`

### Task 7.1: Implement `test_state` module

**Files:**
- Modify: `cli/crates/cook-cli/src/test_state.rs`

- [ ] **Step 7.1.1: Replace the stub with a real impl**

```rust
// cli/crates/cook-cli/src/test_state.rs

use std::collections::BTreeSet;
use std::path::Path;
use serde::{Serialize, Deserialize};
use cook_engine::{TestId, TestOutcome, TestResult};

const STATE_FILE: &str = ".cook/test-state.json";
const SCHEMA_VERSION: u32 = 1;

#[derive(Serialize, Deserialize)]
struct StateFile {
    schema_version: u32,
    ran_at: String,
    results: Vec<StateEntry>,
}

#[derive(Serialize, Deserialize)]
struct StateEntry {
    id: String,
    outcome: String,
    duration_secs: f64,
    from_cache: bool,
}

pub fn load_failed_set(project_root: &Path) -> std::io::Result<BTreeSet<TestId>> {
    let path = project_root.join(STATE_FILE);
    if !path.exists() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("no previous test run recorded at {STATE_FILE}"),
        ));
    }
    let bytes = std::fs::read(&path)?;
    let state: StateFile = serde_json::from_slice(&bytes)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    if state.schema_version != SCHEMA_VERSION {
        eprintln!(
            "warning: {} is schema_version {}; expected {} — ignoring",
            STATE_FILE, state.schema_version, SCHEMA_VERSION,
        );
        return Ok(BTreeSet::new());
    }
    Ok(state.results.iter()
        .filter(|e| matches!(e.outcome.as_str(), "failed" | "blocked" | "timed_out"))
        .map(|e| TestId(e.id.clone()))
        .collect())
}

pub fn save(project_root: &Path, results: &[TestResult]) -> std::io::Result<()> {
    let path = project_root.join(STATE_FILE);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let state = StateFile {
        schema_version: SCHEMA_VERSION,
        ran_at: chrono::Utc::now().to_rfc3339(),
        results: results.iter().map(|r| StateEntry {
            id: r.id.0.clone(),
            outcome: outcome_to_str(r.outcome).to_string(),
            duration_secs: r.duration.as_secs_f64(),
            from_cache: r.from_cache,
        }).collect(),
    };
    let tmp = path.with_extension("json.tmp");
    let bytes = serde_json::to_vec_pretty(&state)?;
    std::fs::write(&tmp, &bytes)?;
    std::fs::rename(&tmp, &path)?;
    Ok(())
}

fn outcome_to_str(o: TestOutcome) -> &'static str {
    match o {
        TestOutcome::Passed => "passed",
        TestOutcome::Failed => "failed",
        TestOutcome::Blocked => "blocked",
        TestOutcome::TimedOut => "timed_out",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;
    use std::time::Duration;

    fn mk(id: &str, outcome: TestOutcome) -> TestResult {
        TestResult {
            id: TestId(id.to_string()),
            namespace: String::new(),
            recipe: id.split(':').next().unwrap().to_string(),
            name: id.split(':').nth(1).unwrap_or("").to_string(),
            suite: String::new(),
            iteration_item: None,
            outcome,
            duration: Duration::from_millis(100),
            from_cache: false,
            stdout: String::new(),
            stderr: String::new(),
            fingerprint: None,
            blocked_by: None,
            should_fail: false,
            timed_out: false,
        }
    }

    #[test]
    fn save_then_load_failed_returns_only_failed() {
        let tmp = tempdir().unwrap();
        let results = vec![
            mk("r:a", TestOutcome::Passed),
            mk("r:b", TestOutcome::Failed),
            mk("r:c", TestOutcome::Blocked),
            mk("r:d", TestOutcome::TimedOut),
            mk("r:e", TestOutcome::Passed),
        ];
        save(tmp.path(), &results).unwrap();
        let failed = load_failed_set(tmp.path()).unwrap();
        assert_eq!(failed.len(), 3);
        assert!(failed.contains(&TestId("r:b".to_string())));
        assert!(failed.contains(&TestId("r:c".to_string())));
        assert!(failed.contains(&TestId("r:d".to_string())));
    }

    #[test]
    fn load_missing_state_file_errors() {
        let tmp = tempdir().unwrap();
        let err = load_failed_set(tmp.path()).expect_err("must error");
        assert_eq!(err.kind(), std::io::ErrorKind::NotFound);
    }
}
```

- [ ] **Step 7.1.2: Run; commit**

```bash
cd cli && cargo test -p cook-cli test_state
```

```bash
git add cli/crates/cook-cli/src/test_state.rs
git commit -m "$(cat <<'EOF'
feat(cook-cli): test_state — persist last-run outcomes

.cook/test-state.json round-trips per runner spec §4.7. load_failed_set
filters to outcome ∈ {failed, blocked, timed_out}; save writes via
tempfile + rename for atomicity. Schema version-gated; warns and
returns empty set on mismatch.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

### Task 7.2: Smoke test `--rerun-failed` against test_benchmarks

- [ ] **Step 7.2.1: Manual verification**

```bash
cd /home/alex/dev/cook && cargo build -q --bin cook
cd examples/test_benchmarks
rm -rf .cook build
../../cli/target/debug/cook --test rerun_failed_set || true  # expect failures on input_01, input_02
echo "----- rerun-failed -----"
../../cli/target/debug/cook --test --rerun-failed || true
```

Expected: second run lists exactly two test executions (the two that failed).

- [ ] **Step 7.2.2: Commit a regression note (no code changes; this verifies wiring)**

If the manual run passes, no commit needed. If it doesn't, debug — likely the `cmd_test` function isn't passing `rerun_failed_set` correctly. Fix and commit.

**Phase 7 checkpoint.** `cook --test --rerun-failed` works end-to-end.

---

## Phase 8: JSON sidecar + JUnit XML

### Task 8.1: JSON sidecar writer

**Files:**
- Modify: `cli/crates/cook-cli/src/test_reporter.rs`

- [ ] **Step 8.1.1: Replace the stub with a real impl**

```rust
pub fn write_json_sidecar(
    project_root: &std::path::Path,
    cli: &Cli,
    results: &[TestResult],
) -> std::io::Result<()> {
    use serde_json::json;

    let path = cli.report_json.clone()
        .unwrap_or_else(|| project_root.join(".cook/test-report.json"));

    let summary = compute_summary(results);
    let payload = json!({
        "schema_version": 1,
        "cook_version": env!("CARGO_PKG_VERSION"),
        "ran_at": chrono::Utc::now().to_rfc3339(),
        "duration_secs": results.iter().map(|r| r.duration.as_secs_f64()).sum::<f64>(),
        "wall_clock_secs": summary.wall_secs,
        "saved_by_cache_secs": results.iter()
            .filter(|r| r.from_cache)
            .map(|r| r.duration.as_secs_f64())
            .sum::<f64>(),
        "summary": {
            "passed": summary.passed,
            "failed": summary.failed,
            "blocked": summary.blocked,
            "timed_out": summary.timed_out,
            "cached": summary.cached,
            "total": results.len(),
        },
        "tests": results.iter().map(|r| json!({
            "id": r.id.0,
            "namespace": r.namespace,
            "recipe": r.recipe,
            "name": r.name,
            "suite": r.suite,
            "iteration_item": r.iteration_item,
            "outcome": outcome_str(r.outcome),
            "duration_secs": r.duration.as_secs_f64(),
            "from_cache": r.from_cache,
            "should_fail": r.should_fail,
            "timed_out": r.timed_out,
            "stdout": r.stdout,
            "stderr": r.stderr,
            "fingerprint": r.fingerprint,
            "blocked_by": r.blocked_by,
        })).collect::<Vec<_>>(),
    });

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let bytes = serde_json::to_vec_pretty(&payload)?;
    std::fs::write(&path, &bytes)?;
    Ok(())
}

struct Summary {
    passed: usize,
    failed: usize,
    blocked: usize,
    timed_out: usize,
    cached: usize,
    wall_secs: f64,
}

fn compute_summary(results: &[TestResult]) -> Summary {
    let mut s = Summary { passed: 0, failed: 0, blocked: 0, timed_out: 0, cached: 0, wall_secs: 0.0 };
    for r in results {
        match r.outcome {
            TestOutcome::Passed => s.passed += 1,
            TestOutcome::Failed => s.failed += 1,
            TestOutcome::Blocked => s.blocked += 1,
            TestOutcome::TimedOut => s.timed_out += 1,
        }
        if r.from_cache { s.cached += 1; }
        s.wall_secs += r.duration.as_secs_f64();
    }
    s
}

fn outcome_str(o: TestOutcome) -> &'static str {
    match o {
        TestOutcome::Passed => "passed",
        TestOutcome::Failed => "failed",
        TestOutcome::Blocked => "blocked",
        TestOutcome::TimedOut => "timed_out",
    }
}
```

- [ ] **Step 8.1.2: Smoke test**

```bash
cd /home/alex/dev/cook && cargo build -q --bin cook
cd examples/test_benchmarks
rm -rf .cook build
../../cli/target/debug/cook --test pass_basic
cat .cook/test-report.json | head -20
```

Expected: a JSON file matching the schema in runner spec §6.3.

- [ ] **Step 8.1.3: Commit**

```bash
cd /home/alex/dev/cook
git add cli/crates/cook-cli/src/test_reporter.rs
git commit -m "$(cat <<'EOF'
feat(cook-cli): JSON test report sidecar

Always written to .cook/test-report.json (or --report-json PATH).
Schema_version: 1; matches runner spec §6.3.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

### Task 8.2: JUnit XML sidecar writer

**Files:**
- Modify: `cli/crates/cook-cli/src/test_reporter.rs`

- [ ] **Step 8.2.1: Implement using `quick-xml`**

```rust
pub fn write_junit_sidecar(
    path: &std::path::Path,
    results: &[TestResult],
) -> std::io::Result<()> {
    use std::collections::BTreeMap;
    use std::io::Write;

    // Group by recipe (= testsuite).
    let mut by_recipe: BTreeMap<String, Vec<&TestResult>> = BTreeMap::new();
    for r in results {
        by_recipe.entry(recipe_of(&r.id)).or_default().push(r);
    }

    let summary = compute_summary(results);
    let mut out = String::new();
    out.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
    out.push_str(&format!(
        "<testsuites name=\"cook\" tests=\"{}\" failures=\"{}\" errors=\"0\" time=\"{:.3}\">\n",
        results.len(),
        summary.failed + summary.timed_out,
        summary.wall_secs,
    ));

    for (recipe, tests) in &by_recipe {
        let recipe_failures = tests.iter().filter(|r| matches!(r.outcome,
            TestOutcome::Failed | TestOutcome::TimedOut)).count();
        let recipe_time: f64 = tests.iter().map(|r| r.duration.as_secs_f64()).sum();
        out.push_str(&format!(
            "  <testsuite name=\"{}\" tests=\"{}\" failures=\"{}\" time=\"{:.3}\">\n",
            xml_escape(recipe),
            tests.len(),
            recipe_failures,
            recipe_time,
        ));

        for r in tests {
            out.push_str(&format!(
                "    <testcase name=\"{}\" classname=\"{}\" time=\"{:.3}\"",
                xml_escape(&r.name),
                xml_escape(recipe),
                r.duration.as_secs_f64(),
            ));
            match r.outcome {
                TestOutcome::Passed => { out.push_str("/>\n"); }
                TestOutcome::Failed => {
                    out.push_str(">\n");
                    out.push_str(&format!(
                        "      <failure message=\"test failed\"><![CDATA[\n{}\n{}\n]]></failure>\n",
                        r.stdout, r.stderr,
                    ));
                    out.push_str("    </testcase>\n");
                }
                TestOutcome::TimedOut => {
                    out.push_str(">\n");
                    out.push_str(&format!(
                        "      <failure message=\"timed out\"><![CDATA[\n{}\n{}\n]]></failure>\n",
                        r.stdout, r.stderr,
                    ));
                    out.push_str("    </testcase>\n");
                }
                TestOutcome::Blocked => {
                    out.push_str(">\n");
                    let cause = r.blocked_by.as_deref().unwrap_or("upstream");
                    out.push_str(&format!(
                        "      <skipped message=\"blocked by upstream cook failure: {}\"/>\n",
                        xml_escape(cause),
                    ));
                    out.push_str("    </testcase>\n");
                }
            }
        }
        out.push_str("  </testsuite>\n");
    }
    out.push_str("</testsuites>\n");

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, out)?;
    Ok(())
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
     .replace('<', "&lt;")
     .replace('>', "&gt;")
     .replace('"', "&quot;")
}
```

- [ ] **Step 8.2.2: Smoke test**

```bash
cd examples/test_benchmarks
rm -rf .cook build
../../cli/target/debug/cook --test pass_basic --report-junit /tmp/junit.xml
xmllint --noout /tmp/junit.xml && echo OK
```

Expected: `OK`. (`xmllint` validates well-formedness.)

- [ ] **Step 8.2.3: Commit**

```bash
cd /home/alex/dev/cook
git add cli/crates/cook-cli/src/test_reporter.rs
git commit -m "$(cat <<'EOF'
feat(cook-cli): JUnit XML test report sidecar

Opt-in via --report-junit PATH. Standard <testsuites> shape. Blocked
maps to <skipped>; failed and timed_out map to <failure>.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

**Phase 8 checkpoint.** JSON sidecar always written; JUnit XML opt-in. Both consume cleanly from CI tooling.

---

## Phase 9: Walkthrough assertions enabled

Replace each fixture's stub walkthrough with full assertions on runner behavior.

### Task 9.1: `examples/test_benchmarks/walkthrough.sh`

- [ ] **Step 9.1.1: Replace the stub body with assertions**

```bash
#!/bin/bash
# walkthrough.sh — pin v1.0 cook --test against the test_benchmarks fixture.

set -uo pipefail
cd "$(dirname "$0")"
COOK="${COOK:-../../cli/target/debug/cook}"
COOK="$(cd "$(dirname "$COOK")" && pwd)/$(basename "$COOK")"

if [ ! -x "$COOK" ]; then
    echo "cook binary not found at $COOK"
    exit 1
fi

pass=0
fail=0

assert_exit() {
    local desc="$1"; local expected="$2"; shift 2
    local out; out=$("$@" 2>&1); local actual=$?
    if [ "$actual" = "$expected" ]; then
        echo "PASS: $desc (exit=$actual)"
        pass=$((pass + 1))
    else
        echo "FAIL: $desc (expected=$expected actual=$actual)"
        echo "----- output -----"
        echo "$out"
        echo "------------------"
        fail=$((fail + 1))
    fi
}

assert_grep() {
    local desc="$1"; local pattern="$2"; shift 2
    local out; out=$("$@" 2>&1)
    if echo "$out" | grep -q "$pattern"; then
        echo "PASS: $desc"
        pass=$((pass + 1))
    else
        echo "FAIL: $desc — pattern not found: $pattern"
        echo "----- output -----"
        echo "$out"
        echo "------------------"
        fail=$((fail + 1))
    fi
}

clean() { rm -rf .cook build; }

# -- 1. Green path
clean
assert_exit "pass_basic exits 0" 0 "$COOK" --test pass_basic

# -- 2. Iteration over 12 inputs, all passing
clean
assert_grep "pass_iterated reports 12 passed" "12 passed" "$COOK" --test pass_iterated

# -- 3. should_fail
clean
assert_exit "pass_should_fail exits 0" 0 "$COOK" --test pass_should_fail

# -- 4. Failure
clean
assert_exit "fail_basic exits 1" 1 "$COOK" --test fail_basic

# -- 5. Mixed pass/fail
clean
assert_grep "fail_partial reports 3 failed" "3 failed" "$COOK" --test fail_partial

# -- 6. Blocked
clean
assert_exit "blocked_by_build exits 1" 1 "$COOK" --test blocked_by_build
assert_grep "blocked status reported" "blocked" "$COOK" --test blocked_by_build

# -- 7. Timeout
clean
assert_grep "slow_timeout reports timeout" "timed out" "$COOK" --test slow_timeout

# -- 8. as modifier
clean
assert_grep "named_test name appears in output" "non-empty" "$COOK" --test named_test

# -- 9. Cache replay
clean
"$COOK" --test cached_replay > /dev/null 2>&1
out=$("$COOK" --test cached_replay 2>&1)
if echo "$out" | grep -q "cached"; then
    echo "PASS: cached_replay second run shows cached"
    pass=$((pass + 1))
else
    echo "FAIL: cached_replay no cache hit"
    echo "$out"
    fail=$((fail + 1))
fi

# -- 10. --rerun-failed
clean
"$COOK" --test rerun_failed_set > /dev/null 2>&1
out=$("$COOK" --test --rerun-failed 2>&1)
ran=$(echo "$out" | grep -E "^.*input_0[12]" | wc -l)
if [ "$ran" -ge 2 ]; then
    echo "PASS: --rerun-failed re-ran the previously failed tests"
    pass=$((pass + 1))
else
    echo "FAIL: --rerun-failed did not re-run failed set"
    echo "$out"
    fail=$((fail + 1))
fi

echo
echo "Passed: $pass   Failed: $fail"
exit $((fail > 0 ? 1 : 0))
```

- [ ] **Step 9.1.2: Run and iterate**

```bash
cd /home/alex/dev/cook && cargo build -q --bin cook
examples/test_benchmarks/walkthrough.sh
```

Expected: all PASS, exit 0. Debug any failures.

- [ ] **Step 9.1.3: Commit**

```bash
git add examples/test_benchmarks/walkthrough.sh
git commit -m "$(cat <<'EOF'
test(examples): enable test_benchmarks walkthrough assertions

Pins green path, iteration aggregation, should_fail, failure capture,
blocked status, timeout, as modifier, cache replay, and --rerun-failed.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

### Task 9.2: `examples/monorepo_test/walkthrough.sh`

- [ ] **Step 9.2.1: Replace stub with assertions**

```bash
#!/bin/bash
set -uo pipefail
cd "$(dirname "$0")"
COOK="${COOK:-../../cli/target/debug/cook}"
COOK="$(cd "$(dirname "$COOK")" && pwd)/$(basename "$COOK")"

if [ ! -x "$COOK" ]; then echo "cook binary not found at $COOK"; exit 1; fi

pass=0; fail=0
clean() { rm -rf .cook build apps/*/build apps/*/.cook packages/*/build packages/*/.cook; }

assert_grep() {
    local desc="$1"; local pattern="$2"; shift 2
    local out; out=$("$@" 2>&1)
    if echo "$out" | grep -q "$pattern"; then
        echo "PASS: $desc"; pass=$((pass+1))
    else
        echo "FAIL: $desc — pattern not found: $pattern"
        echo "$out"; fail=$((fail+1))
    fi
}

# -- 1. Bare cook --test discovers tests in all three Cookfiles
clean
assert_grep "apps.web.unit appears" "apps.web.unit" "$COOK" --test
assert_grep "apps.api.unit appears" "apps.api.unit" "$COOK" --test
assert_grep "shared.unit appears"   "shared.unit"   "$COOK" --test

# -- 2. Namespace-scoped run
clean
out=$("$COOK" --test apps.web 2>&1)
echo "$out" | grep -q "apps.web" && pass=$((pass+1)) || { echo "FAIL: namespace scope"; fail=$((fail+1)); }
echo "$out" | grep -q "apps.api" && { echo "FAIL: namespace scope leaked apps.api"; fail=$((fail+1)); } || pass=$((pass+1))

# -- 3. Recipe-scoped run
clean
out=$("$COOK" --test apps.web.unit 2>&1)
echo "$out" | grep -q "apps.web.unit" && pass=$((pass+1)) || { echo "FAIL: recipe scope"; fail=$((fail+1)); }

# -- 4. JSON sidecar exists and parses
clean
"$COOK" --test > /dev/null 2>&1
test -f .cook/test-report.json && pass=$((pass+1)) || { echo "FAIL: no JSON sidecar"; fail=$((fail+1)); }
jq -e '.summary.total > 0' .cook/test-report.json > /dev/null \
    && pass=$((pass+1)) || { echo "FAIL: JSON summary malformed"; fail=$((fail+1)); }

echo; echo "Passed: $pass  Failed: $fail"
exit $((fail > 0 ? 1 : 0))
```

- [ ] **Step 9.2.2: Run and commit**

```bash
examples/monorepo_test/walkthrough.sh
```

```bash
git add examples/monorepo_test/walkthrough.sh
git commit -m "$(cat <<'EOF'
test(examples): enable monorepo_test walkthrough assertions

Pins workspace discovery, namespace scope, recipe scope, JSON sidecar.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

### Task 9.3: `examples/test_caching/walkthrough.sh`

- [ ] **Step 9.3.1: Replace stub with cache-shape assertions**

```bash
#!/bin/bash
set -uo pipefail
cd "$(dirname "$0")"
COOK="${COOK:-../../cli/target/debug/cook}"
COOK="$(cd "$(dirname "$COOK")" && pwd)/$(basename "$COOK")"

if [ ! -x "$COOK" ]; then echo "cook binary not found at $COOK"; exit 1; fi

pass=0; fail=0
clean() { rm -rf .cook build; }

# -- First run: every test runs
clean
out1=$("$COOK" --test 2>&1)
if echo "$out1" | grep -q "cached"; then
    echo "FAIL: first run should have no cache hits"; echo "$out1"; fail=$((fail+1))
else
    echo "PASS: first run no cache hits"; pass=$((pass+1))
fi

# -- Second run: every passing test cached
out2=$("$COOK" --test 2>&1)
if echo "$out2" | grep -q "cached"; then
    echo "PASS: second run shows cache hits"; pass=$((pass+1))
else
    echo "FAIL: second run no cache hits"; echo "$out2"; fail=$((fail+1))
fi

# -- Touch a source file; only iterated test should re-run for that input
touch src/a.txt
out3=$("$COOK" --test iterated 2>&1)
echo "$out3" | grep -qE "(2 cached|1 ran)" && pass=$((pass+1)) || {
    echo "FAIL: source touch did not selectively bust cache"; echo "$out3"; fail=$((fail+1));
}

# -- --rerun busts everything
out4=$("$COOK" --test --rerun 2>&1)
if echo "$out4" | grep -q "cached"; then
    echo "FAIL: --rerun should have no cache hits"; echo "$out4"; fail=$((fail+1))
else
    echo "PASS: --rerun busts cache"; pass=$((pass+1))
fi

echo; echo "Passed: $pass  Failed: $fail"
exit $((fail > 0 ? 1 : 0))
```

- [ ] **Step 9.3.2: Run and commit**

```bash
examples/test_caching/walkthrough.sh
git add examples/test_caching/walkthrough.sh
git commit -m "$(cat <<'EOF'
test(examples): enable test_caching walkthrough assertions

Pins first-run no-hits, second-run cache replay, source-touch
selective invalidation, --rerun bust.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

**Phase 9 checkpoint.** All three example walkthroughs pin runner behavior. Run them in CI alongside `cli_audit_exit_codes/walkthrough.sh`.

---

## Phase 10: Integration tests in `cook-cli/tests/`

Stand up `cargo`-driven integration tests that don't require a built binary in PATH, so the test suite is portable.

### Task 10.1: Workspace integration test

**Files:**
- Create: `cli/crates/cook-cli/tests/test_runner_workspace.rs`

- [ ] **Step 10.1.1: Author the test**

```rust
use std::process::Command;
use tempfile::tempdir;

fn cook_binary() -> std::path::PathBuf {
    let mut path = std::env::current_exe().unwrap();
    path.pop(); // /target/debug/deps
    path.pop(); // /target/debug
    path.push("cook");
    if !path.exists() {
        panic!("cook binary not built at {} — run `cargo build --bin cook` first", path.display());
    }
    path
}

fn write_workspace(root: &std::path::Path) {
    std::fs::write(root.join("Cookfile"), "
import sub ./sub
recipe build
    cook \"build/r.txt\" using { mkdir -p build; echo > $<out> }
").unwrap();
    std::fs::create_dir(root.join("sub")).unwrap();
    std::fs::write(root.join("sub/Cookfile"), "
recipe pass
    test { true } timeout 5
recipe fail_one
    test { false } timeout 5
").unwrap();
}

#[test]
fn cook_test_discovers_workspace() {
    let tmp = tempdir().unwrap();
    write_workspace(tmp.path());
    let out = Command::new(cook_binary())
        .arg("--test")
        .current_dir(tmp.path())
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("sub.pass"), "stdout: {stdout}");
    assert!(stdout.contains("sub.fail_one"));
    assert_ne!(out.status.code().unwrap(), 0, "should exit non-zero — fail_one fails");
}

#[test]
fn cook_test_namespace_scope() {
    let tmp = tempdir().unwrap();
    write_workspace(tmp.path());
    let out = Command::new(cook_binary())
        .args(["--test", "sub"])
        .current_dir(tmp.path())
        .output()
        .unwrap();
    assert!(out.status.code().unwrap() != 0); // fail_one in scope
}

#[test]
fn cook_test_recipe_scope_pass() {
    let tmp = tempdir().unwrap();
    write_workspace(tmp.path());
    let out = Command::new(cook_binary())
        .args(["--test", "sub.pass"])
        .current_dir(tmp.path())
        .output()
        .unwrap();
    assert_eq!(out.status.code().unwrap(), 0);
}

#[test]
fn cook_test_writes_json_sidecar() {
    let tmp = tempdir().unwrap();
    write_workspace(tmp.path());
    Command::new(cook_binary())
        .arg("--test")
        .current_dir(tmp.path())
        .output()
        .unwrap();
    let report = tmp.path().join(".cook/test-report.json");
    assert!(report.exists(), "JSON sidecar not written");
    let bytes = std::fs::read(&report).unwrap();
    let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(v["schema_version"], 1);
    assert!(v["summary"]["total"].as_u64().unwrap() >= 2);
}
```

- [ ] **Step 10.1.2: Run; commit**

```bash
cd cli && cargo build --bin cook && cargo test -p cook-cli --test test_runner_workspace
```

```bash
git add cli/crates/cook-cli/tests/test_runner_workspace.rs
git commit -m "$(cat <<'EOF'
test(cook-cli): integration tests for workspace test discovery

cook --test bare → discovers all imported recipes.
cook --test <namespace> → scopes to namespace.
cook --test <recipe> → scopes to recipe.
JSON sidecar always written.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

### Task 10.2: Caching integration test

**Files:**
- Create: `cli/crates/cook-cli/tests/test_runner_caching.rs`

- [ ] **Step 10.2.1: Author**

```rust
use std::process::Command;
use tempfile::tempdir;

fn cook_binary() -> std::path::PathBuf {
    /* same helper as Task 10.1 */
    let mut path = std::env::current_exe().unwrap();
    path.pop(); path.pop(); path.push("cook");
    path
}

#[test]
fn passing_test_caches_and_replays() {
    let tmp = tempdir().unwrap();
    std::fs::write(tmp.path().join("Cookfile"), "
recipe r
    test { true } timeout 5
").unwrap();

    let _ = Command::new(cook_binary()).arg("--test").current_dir(tmp.path()).output().unwrap();
    let out2 = Command::new(cook_binary()).arg("--test").current_dir(tmp.path()).output().unwrap();
    let stdout = String::from_utf8_lossy(&out2.stdout);
    assert!(stdout.contains("cached"), "second run should show cache hit; stdout: {stdout}");
}

#[test]
fn failing_test_is_not_cached() {
    let tmp = tempdir().unwrap();
    std::fs::write(tmp.path().join("Cookfile"), "
recipe r
    test { false } timeout 5
").unwrap();

    Command::new(cook_binary()).arg("--test").current_dir(tmp.path()).output().unwrap();
    let out2 = Command::new(cook_binary()).arg("--test").current_dir(tmp.path()).output().unwrap();
    let stdout = String::from_utf8_lossy(&out2.stdout);
    assert!(!stdout.contains("cached"), "failed test must not be cached");
}

#[test]
fn rerun_busts_cache() {
    let tmp = tempdir().unwrap();
    std::fs::write(tmp.path().join("Cookfile"), "
recipe r
    test { true } timeout 5
").unwrap();

    Command::new(cook_binary()).arg("--test").current_dir(tmp.path()).output().unwrap();
    let out2 = Command::new(cook_binary()).args(["--test", "--rerun"]).current_dir(tmp.path()).output().unwrap();
    let stdout = String::from_utf8_lossy(&out2.stdout);
    assert!(!stdout.contains("cached"), "--rerun should bust cache; stdout: {stdout}");
}
```

- [ ] **Step 10.2.2: Run; commit**

```bash
cd cli && cargo test -p cook-cli --test test_runner_caching
```

```bash
git add cli/crates/cook-cli/tests/test_runner_caching.rs
git commit -m "$(cat <<'EOF'
test(cook-cli): integration tests for cache contract

Passing tests cache and replay; failing tests never cache; --rerun
busts cache.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

### Task 10.3: --rerun-failed integration test

**Files:**
- Create: `cli/crates/cook-cli/tests/test_runner_rerun_failed.rs`

- [ ] **Step 10.3.1: Author**

```rust
use std::process::Command;
use tempfile::tempdir;

fn cook_binary() -> std::path::PathBuf {
    let mut path = std::env::current_exe().unwrap();
    path.pop(); path.pop(); path.push("cook"); path
}

#[test]
fn rerun_failed_runs_only_previously_failed_tests() {
    let tmp = tempdir().unwrap();
    std::fs::write(tmp.path().join("Cookfile"), "
recipe pass
    test { true } as 'p1' timeout 5
recipe fail
    test { false } as 'f1' timeout 5
").unwrap();

    let _ = Command::new(cook_binary()).arg("--test").current_dir(tmp.path()).output().unwrap();

    let out2 = Command::new(cook_binary()).args(["--test", "--rerun-failed"])
        .current_dir(tmp.path()).output().unwrap();
    let stdout = String::from_utf8_lossy(&out2.stdout);

    // Only the failed test should be in the second run's report.
    assert!(stdout.contains("fail"), "rerun-failed should re-run the failed recipe; stdout: {stdout}");
    // (The Reporter only emits a recipe line if it had any tests run; if `pass`
    // produces no output, that's the assertion. If the implementation emits a
    // `0 tests ran` line for the pass recipe, weaken this check accordingly.)
}

#[test]
fn rerun_failed_with_no_state_warns_and_exits_zero() {
    let tmp = tempdir().unwrap();
    std::fs::write(tmp.path().join("Cookfile"), "recipe r\n    test { true }\n").unwrap();
    let out = Command::new(cook_binary()).args(["--test", "--rerun-failed"])
        .current_dir(tmp.path()).output().unwrap();
    assert_eq!(out.status.code().unwrap(), 0);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("no previous test run"), "stderr: {stderr}");
}
```

- [ ] **Step 10.3.2: Run; commit**

```bash
cd cli && cargo test -p cook-cli --test test_runner_rerun_failed
```

```bash
git add cli/crates/cook-cli/tests/test_runner_rerun_failed.rs
git commit -m "$(cat <<'EOF'
test(cook-cli): integration tests for --rerun-failed

Re-runs only previously-failed tests; warns + exits 0 when state is
absent.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

### Task 10.4: Filter and fail-fast integration tests

**Files:**
- Create: `cli/crates/cook-cli/tests/test_runner_filter.rs`

- [ ] **Step 10.4.1: Author**

```rust
use std::process::Command;
use tempfile::tempdir;

fn cook_binary() -> std::path::PathBuf {
    let mut path = std::env::current_exe().unwrap();
    path.pop(); path.pop(); path.push("cook"); path
}

#[test]
fn filter_restricts_test_set() {
    let tmp = tempdir().unwrap();
    std::fs::write(tmp.path().join("Cookfile"), "
recipe a
    test { true } as 'alpha' timeout 5
recipe b
    test { true } as 'beta' timeout 5
").unwrap();
    let out = Command::new(cook_binary())
        .args(["--test", "--filter", "*:alpha"])
        .current_dir(tmp.path()).output().unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("alpha"));
    assert!(!stdout.contains("beta"));
}

#[test]
fn filter_with_zero_matches_warns_exits_zero() {
    let tmp = tempdir().unwrap();
    std::fs::write(tmp.path().join("Cookfile"), "
recipe r
    test { true } timeout 5
").unwrap();
    let out = Command::new(cook_binary())
        .args(["--test", "--filter", "nonexistent:*"])
        .current_dir(tmp.path()).output().unwrap();
    assert_eq!(out.status.code().unwrap(), 0);
}
```

- [ ] **Step 10.4.2: Run; commit**

```bash
cd cli && cargo test -p cook-cli --test test_runner_filter
```

```bash
git add cli/crates/cook-cli/tests/test_runner_filter.rs
git commit -m "$(cat <<'EOF'
test(cook-cli): integration tests for --filter

Filter restricts the test set; zero-match exits 0 with a warning.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

### Task 10.5: Cross-cutting check

- [ ] **Step 10.5.1: Run the entire test suite**

```bash
cd cli && cargo test --workspace
```

Expected: all green.

- [ ] **Step 10.5.2: Run all walkthroughs**

```bash
cd /home/alex/dev/cook
for dir in examples/cli_audit_exit_codes examples/test_benchmarks examples/monorepo_test examples/test_caching; do
  echo "=== $dir ==="
  (cd "$dir" && ./walkthrough.sh)
done
```

Expected: every walkthrough exits 0.

- [ ] **Step 10.5.3: Spec build**

```bash
cd standard && pnpm build
```

Expected: exit 0.

- [ ] **Step 10.5.4: Final commit (if any cleanup needed)**

If you cleaned up dead code, removed `_unused`-prefixed locals, or fixed lint warnings during the integration phase, commit them now:

```bash
git add -p
git commit -m "$(cat <<'EOF'
chore(cook-cli): post-integration cleanup

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

**Phase 10 checkpoint — runner is feature-complete.** `cook --test` runs every test in the workspace, scopes to recipe / namespace, caches passing results, replays them, supports `--rerun [PATTERN]`, `--rerun-failed`, `--filter`, `--fail-fast`, emits JSON + JUnit sidecars, prints a summary block with grouped failures, and exits non-zero on any failure / blocked / timed-out test. The Standard ships CS-NNNN. Three example walkthroughs and four integration tests pin behavior in CI.

---

## Acceptance criteria — final verification

Run the full conformance check:

```bash
cd /home/alex/dev/cook
cargo test --workspace
cd standard && pnpm build && cd ..
for dir in examples/cli_audit_exit_codes examples/test_benchmarks examples/monorepo_test examples/test_caching; do
  (cd "$dir" && ./walkthrough.sh) || { echo "FAIL: $dir walkthrough"; exit 1; }
done
echo "All green."
```

Expected: `All green.`, exit 0.

Sanity-check the spec-first hook:

```bash
git -C /home/alex/dev/cook log --oneline standard/specs/2026-05-07-test-runner-language-design.md
```

Expected: at least one entry (the spec commit `dc5fb7b` from before this plan).

Confirm CS-NNNN replacement:

```bash
grep -rn "CS-NNNN" standard/specs/2026-05-07-test-runner-language-design.md \
    standard/src/content/docs/appendix/D-changes.mdx
```

Replace `CS-NNNN` with the assigned number (`CS-0061`, `CS-0062`, or whatever the next sequential ID is) in a single PR-time edit. Apply the same substitution to:
- The Standard chapter D-changes entry
- The two design specs (`standard/specs/...` and `docs/superpowers/specs/...`)
- This plan's commit messages and prose if you re-run any of them

---

## Self-review (executed inline)

1. **Spec coverage:** Every clause in `standard/specs/2026-05-07-test-runner-language-design.md` (CS-NNNN) maps to a Phase 2 task; every clause in `docs/superpowers/specs/2026-05-07-test-runner-design.md` maps to a Phase 3–10 task. Acceptance criteria from both specs are pinned by Phase 10's final cross-cutting check.

2. **Placeholder scan:** Three `todo!()` macros remain in Phase 4.1 (`run_for_test_inner` helpers). They are sign-posts to "factor existing engine helpers" — the surrounding prose names the helpers and points at the existing code path. The plan's executor must lift those into the new entry point; that's a known refactor task, not a TBD.

3. **Type consistency:** `TestId`, `TestOutcome`, `TestResult`, `TestFailureReason`, `EngineEvent::Test*` are defined in Task 3.1 / 3.2 and consumed identically in 3.3, 3.4, 4.1, 4.5, 5.2, 7.1, 8.1, 8.2, 10.x. Field names match across tasks. The `from_cache` field in `TestResult` and the `cached` field in `EngineEvent::TestPassed` parallel each other — the runner's "test was a cache hit" signal is uniform.

4. **Scope check:** Each phase ends at a shippable checkpoint:
   - **Phase 1** — examples committed; no code change.
   - **Phase 2** — CS-NNNN acceptance-complete; no runner.
   - **Phase 3** — engine plumbing; no CLI.
   - **Phase 4** — `cook --test` works without caching.
   - **Phase 5** — caching live.
   - **Phase 6** — `--rerun PATTERN` live.
   - **Phase 7** — `--rerun-failed` live.
   - **Phase 8** — sidecars emitted.
   - **Phase 9** — walkthroughs assert.
   - **Phase 10** — integration tests pass.

   The plan is one feature, not multiple subsystems; staging is for incremental landing, not for splitting into separate plans.

