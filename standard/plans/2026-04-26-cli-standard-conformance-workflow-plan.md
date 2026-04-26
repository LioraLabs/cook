# CLI/Standard Conformance Sync Workflow — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Apply the design in `standard/specs/2026-04-26-cli-standard-conformance-workflow-design.md`. Stand up the workflow that keeps `cook-lang` honestly tracking the Cook Standard, then close the existing v0.2 gap caused by CS-0011 (VarDecl removal). After this plan executes, the parser claims `Cook Standard v0.2`, the conformance harness gates against the corpus at HEAD, the corpus is impl-agnostic, and a backwards-conformance script verifies prior cuts via `cs-standard/vX.Y` git tags.

**Architecture:** Two phases. **Phase 1** lands the workflow infrastructure while the parser still claims `v0.1` honestly — the harness, version constant, version surfacing in `cook --version`, README claims, `CONFORMANCE.md`, the env-overridable corpus path, the backwards-conformance script, the pre-commit hook second arm, and CONTRIBUTING.md updates. **Phase 2** closes the v0.2 gap: adds the CS-0011 negative case (which fails CI on land and proves the workflow works), removes `Token::VarDecl` from the lexer/parser/AST, updates all consumers (env layering, codegen tests, conformance harness format), updates the 14 existing positive `parse.txt` files to drop the `vars: []` line, adds the CS-0011 positive case, and bumps the claim to `v0.2`. The corpus carries no per-case version metadata; git tags are the version axis.

**Tech Stack:** Rust 2021 (cargo workspace at `cli/`), clap 4 (derive + builder API for runtime version override), bash (pre-commit hook, backwards-conformance script), Astro/Starlight MDX (CONTRIBUTING.md is a top-level repo doc, not Starlight). Tests run via `cargo test` (unit + integration + conformance harness). Standard build runs via `pnpm build` from `standard/`.

---

## Working directory and prerequisites

All paths are relative to `/home/alex/dev/cook` unless noted. Run from the repo root.

Confirm the spec-first hook is installed (it should already be):

```bash
git -C /home/alex/dev/cook config --get core.hooksPath
# Expected: .githooks
```

If empty, run `git -C /home/alex/dev/cook config core.hooksPath .githooks`.

The Rust workspace root for cargo commands is `cli/`. Run cargo commands as:

```bash
cd /home/alex/dev/cook/cli && cargo test -p cook-lang --test conformance
```

…or chain `cd cli &&` in front of any cargo invocation.

## Per-task verification

Phase 1 tasks (workflow plumbing) verify with:

```bash
cd /home/alex/dev/cook/cli && cargo build -q && cargo test -q
```

Phase 2 tasks involving Standard prose (CONTRIBUTING.md, scripts under `standard/scripts/`) additionally verify with:

```bash
cd /home/alex/dev/cook/standard && pnpm build
```

Conformance harness alone:

```bash
cd /home/alex/dev/cook/cli && cargo test -p cook-lang --test conformance
```

Expected output once the workflow is in place includes a one-line summary printed via `eprintln!` (added in Task 5). Until then, the harness only prints the standard `cargo test` summary.

---

## File structure

| File | Status | Responsibility | Tasks |
|---|---|---|---|
| `cli/crates/cook-lang/src/lib.rs` | Modify | Add `COOK_STANDARD_VERSION` const; remove VarDecl arm and `vars` plumbing in `parse()`. | 1, 11 |
| `cli/crates/cook-lang/src/lexer.rs` | Modify | Remove `Token::VarDecl` variant, `try_parse_var_decl`, three call sites in `tokenize`, two VarDecl unit tests, and one dead `test_use_decl_not_var`. | 11 |
| `cli/crates/cook-lang/src/recipe.rs` | Modify | Remove the `Token::VarDecl` arm in `parse_recipe`. | 11 |
| `cli/crates/cook-lang/src/ast.rs` | Modify | Remove `vars: Vec<(String, String)>` from `Cookfile` and from two test constructions. | 11 |
| `cli/crates/cook-lang/src/cook_line.rs` | Modify | Remove dead `parse_quoted_strings_parser` (only existed to support VarDecl-related splitting). | 11 |
| `cli/crates/cook-lang/src/tests.rs` | Modify | Delete `test_bare_vars_parsed`, `test_vars_and_configs_together`, `test_var_after_recipe_is_shell_command` (rewrite as `test_indented_quoted_pair_is_shell_command`); update `test_parse_use_with_vars_and_configs` to drop the vars assertion. | 11 |
| `cli/crates/cook-lang/tests/conformance.rs` | Modify | (a) Honor `COOK_CONFORMANCE_CORPUS` env override for corpus root; (b) drop `format_var` and the `vars: …` line from `format_cookfile`; (c) add a final `eprintln!` summary line with the claimed version. | 5, 11 |
| `cli/crates/cook-lang/CONFORMANCE.md` | Create | Short impl-side doc — claim, harness, backwards-script, pending list. | 4 |
| `cli/crates/cook-lang/README.md` | Create | State the claim. | 3, 12 |
| `cli/crates/cook-cli/src/main.rs` | Modify | Use clap builder API to inject the version string built from `cook_lang::COOK_STANDARD_VERSION`. | 2 |
| `cli/crates/cook-cli/src/cli.rs` | Modify | Remove the `version = …` derive attribute (we'll set version dynamically in `main.rs`). | 2 |
| `cli/crates/cook-cli/src/env.rs` | Modify | Drop the "Cookfile bare vars" layer (Layer 2) and update the docstring. | 11 |
| `cli/crates/cook-luagen/src/dep_ref.rs` | Modify | Remove `vars: vec![]` from one `Cookfile` construction. | 11 |
| `cli/crates/cook-luagen/src/tests.rs` | Modify | Remove `vars: vec![]` from seven `Cookfile` constructions. | 11 |
| `standard/conformance/positive/001-empty-recipe/parse.txt` | Modify | Drop the `  vars: []` line. | 11 |
| `standard/conformance/positive/002-shell-step/parse.txt` | Modify | Drop the `  vars: []` line. | 11 |
| `standard/conformance/positive/003-interactive-shell/parse.txt` | Modify | Drop the `  vars: []` line. | 11 |
| `standard/conformance/positive/004-ingredients-with-exclude/parse.txt` | Modify | Drop the `  vars: []` line. | 11 |
| `standard/conformance/positive/005-cook-single-output-with-shell-using/parse.txt` | Modify | Drop the `  vars: []` line. | 11 |
| `standard/conformance/positive/006-cook-multi-output-with-shell-block/parse.txt` | Modify | Drop the `  vars: []` line. | 11 |
| `standard/conformance/positive/007-cook-multi-output-with-lua-block/parse.txt` | Modify | Drop the `  vars: []` line. | 11 |
| `standard/conformance/positive/008-lua-line-and-block/parse.txt` | Modify | Drop the `  vars: []` line. | 11 |
| `standard/conformance/positive/009-test-step/parse.txt` | Modify | Drop the `  vars: []` line. | 11 |
| `standard/conformance/positive/010-use-and-module-call/parse.txt` | Modify | Drop the `  vars: []` line. | 11 |
| `standard/conformance/positive/011-cross-recipe-bare-reference/parse.txt` | Modify | Drop the `  vars: []` line. | 11 |
| `standard/conformance/positive/012-cross-recipe-accessor-iteration/parse.txt` | Modify | Drop the `  vars: []` line. | 11 |
| `standard/conformance/positive/013-cross-recipe-ingredients-only/parse.txt` | Modify | Drop the `  vars: []` line. | 11 |
| `standard/conformance/positive/014-path-match-is-opaque/parse.txt` | Modify | Drop the `  vars: []` line. | 11 |
| `standard/conformance/negative/007-bare-vardecl-rejected/` | Create | New negative case for top-level `NAME "value"` rejection (Cookfile, error.txt, notes.md). | 10 |
| `standard/conformance/positive/015-config-block-only-vars/` | Create | New positive case showing config-block-only variable surface (Cookfile, parse.txt, notes.md). | 11 |
| `standard/scripts/check-conformance-against-tag.sh` | Create | Backwards-conformance script — materializes a tag's corpus and runs the harness. | 6 |
| `.githooks/pre-commit` | Modify | Add second arm: warn when `D-changes.mdx` adds a CS without growing `standard/conformance/`. | 7 |
| `CONTRIBUTING.md` | Modify | Document the workflow: claim mechanism, version-bumping ritual, default vs backwards-conformance harness modes. | 8 |
| `README.md` | Modify | State the claimed Standard version in the project description. | 3, 12 |

No deletions of whole files. No new crates.

---

## Phase 1 — Workflow infrastructure (parser still claims v0.1)

Phase 1 lands the workflow plumbing while the parser is still pre-CS-0011-removal. The claim is honestly `v0.1` until Phase 2 closes the gap. Each task in this phase ends in its own commit.

### Task 1: Add `COOK_STANDARD_VERSION` constant claiming v0.1

**Files:**
- Modify: `cli/crates/cook-lang/src/lib.rs`

- [ ] **Step 1.1: Add the constant near the top of `lib.rs`**

In `cli/crates/cook-lang/src/lib.rs`, immediately after the `pub mod` declarations and before the `use` statements (i.e., after line 6), insert:

```rust

/// The Cook Standard version this crate claims to fully implement.
///
/// "Fully implement" means every case under `standard/conformance/` (relative
/// to the workspace root, or under `$COOK_CONFORMANCE_CORPUS` if set) passes
/// the conformance harness in `tests/conformance.rs`.
///
/// Move this constant in lockstep with `standard/VERSION` when the parser
/// catches up to a new cut. See `cli/crates/cook-lang/CONFORMANCE.md`.
pub const COOK_STANDARD_VERSION: &str = "0.1";

```

- [ ] **Step 1.2: Build the workspace**

Run: `cd /home/alex/dev/cook/cli && cargo build -q`
Expected: clean build (the existing dead-code warning for `parse_quoted_strings_parser` is preexisting; ignore for this task).

- [ ] **Step 1.3: Commit**

```bash
git -C /home/alex/dev/cook add cli/crates/cook-lang/src/lib.rs
git -C /home/alex/dev/cook commit -m "feat(cook-lang): add COOK_STANDARD_VERSION constant claiming v0.1"
```

### Task 2: Surface the claim in `cook --version`

**Files:**
- Modify: `cli/crates/cook-cli/src/cli.rs`
- Modify: `cli/crates/cook-cli/src/main.rs`

clap 4's derive macro `#[command(version, ...)]` only accepts compile-time literals, so we drop the derive attribute and set the version string at runtime via the builder API. This keeps `cook_lang::COOK_STANDARD_VERSION` as the single source of truth.

- [ ] **Step 2.1: Ensure no `version` attribute is set on the derive in `cli.rs`**

Open `cli/crates/cook-cli/src/cli.rs`. The current `#[command(...)]` block (lines 7–12) does not set `version`. Confirm by reading it:

```rust
#[command(
    name = "cook",
    about = "A modern build system with Lua",
    override_usage = "cook [OPTIONS] [RECIPE] [CONFIG]",
    after_help = "Run `cook <recipe>` to execute a recipe (defaults to 'build')"
)]
```

No edit needed if `version` is not present. If a `version` attribute is added in the future, remove it before continuing.

- [ ] **Step 2.2: Add a `cook_lang` import to `main.rs`**

In `cli/crates/cook-cli/src/main.rs`, near the existing imports (after the `use clap::Parser;` line and the `use cli::{Cli, Command};` lines), the workspace already exposes `cook_lang` via `cook-cli`'s Cargo.toml dependency. No new import line is strictly needed because we access via the fully qualified path below; for readability add:

```rust
use clap::CommandFactory;
```

immediately after `use clap::Parser;`.

- [ ] **Step 2.3: Build the version string and inject it into the parsed command**

Replace the line `let cli = Cli::parse();` near the top of `fn main()` (currently line 21 of `main.rs`) with:

```rust
    let version_string = format!(
        "{} (Cook Standard v{})",
        env!("CARGO_PKG_VERSION"),
        cook_lang::COOK_STANDARD_VERSION,
    );
    let cli_command = <Cli as CommandFactory>::command().version(version_string);
    let matches = cli_command.get_matches();
    let cli = <Cli as clap::FromArgMatches>::from_arg_matches(&matches)
        .expect("clap derive guarantees this conversion");
```

- [ ] **Step 2.4: Build and run `cook --version` to verify**

```bash
cd /home/alex/dev/cook/cli && cargo build -q --bin cook && ./target/debug/cook --version
```

Expected: `cook 0.1.0 (Cook Standard v0.1)` (the package version is `0.1.0` per `cook-cli/Cargo.toml`).

- [ ] **Step 2.5: Commit**

```bash
git -C /home/alex/dev/cook add cli/crates/cook-cli/src/main.rs cli/crates/cook-cli/src/cli.rs
git -C /home/alex/dev/cook commit -m "feat(cook-cli): surface claimed Cook Standard version in --version"
```

(`cli.rs` is included in the commit even if unmodified, in case Step 2.1 found a stray `version` attribute to remove. If `git add` reports no changes there, drop it from the commit command.)

### Task 3: README claims at v0.1

**Files:**
- Create: `cli/crates/cook-lang/README.md`
- Modify: `README.md`

- [ ] **Step 3.1: Create `cli/crates/cook-lang/README.md`**

```markdown
# cook-lang

The Cookfile parser: text in, AST out. The current reference implementation of the [Cook Standard](../../../standard/).

## Cook Standard claim

This crate claims to implement **Cook Standard v0.1**.

The claim lives in `src/lib.rs`:

```rust
pub const COOK_STANDARD_VERSION: &str = "0.1";
```

To verify the claim, run the conformance harness:

```bash
cargo test -p cook-lang --test conformance
```

To verify backwards conformance against a previously-cut version:

```bash
standard/scripts/check-conformance-against-tag.sh v0.1
```

See `CONFORMANCE.md` for details and pending CSes.
```

- [ ] **Step 3.2: Update root `README.md`**

Read the existing `/home/alex/dev/cook/README.md` and add a one-line claim near the top of the Cook description (e.g., immediately after the existing one-line description). The exact insertion point depends on the current README's structure; if the README does not yet describe the project, append the line:

```markdown
The reference implementation in `cli/crates/cook-lang/` claims **Cook Standard v0.1**.
```

If the README already has an "Implementation" or "Status" section, add the claim there instead.

- [ ] **Step 3.3: Commit**

```bash
git -C /home/alex/dev/cook add cli/crates/cook-lang/README.md README.md
git -C /home/alex/dev/cook commit -m "docs: state Cook Standard v0.1 conformance claim in READMEs"
```

### Task 4: Add `cli/crates/cook-lang/CONFORMANCE.md`

**Files:**
- Create: `cli/crates/cook-lang/CONFORMANCE.md`

- [ ] **Step 4.1: Write the file**

```markdown
# Conformance

This crate is the current reference implementation of the [Cook Standard](../../../standard/).

## Claim

`cook-lang` claims **Cook Standard v0.1**.

The claim is the constant `COOK_STANDARD_VERSION` in `src/lib.rs`. The constant is the single source of truth; the README and `cook --version` mirror it.

## Verifying the claim

The conformance harness walks `standard/conformance/` (relative to the workspace root) and asserts that every positive case parses into the expected AST shape and every negative case is rejected with the expected error substring.

```bash
cd cli && cargo test -p cook-lang --test conformance
```

## Backwards conformance

To verify that this parser still satisfies a previously-cut Standard version:

```bash
standard/scripts/check-conformance-against-tag.sh v0.1
```

The script materializes the corpus from the `cs-standard/v0.1` git tag into a temp directory and runs the harness against it. The corpus path is overridable via the `COOK_CONFORMANCE_CORPUS` environment variable.

## Pending CSes

CSes that this crate is in the middle of implementing — included here when the parser is mid-catch-up between cuts. The conformance harness output is authoritative; this list is a human summary.

- **CS-0011** (top-level VarDecl removal): pending. The parser currently accepts top-level `NAME "value"` as a variable declaration, which the Standard at v0.2 rejects. See `standard/specs/2026-04-26-remove-vardecl-design.md` for the spec design.

## Bumping the claim

When `cook-lang` finishes catching up to a new cut, bump `COOK_STANDARD_VERSION` in the same commit that closes the last gap, mirror the new value in `cli/crates/cook-lang/README.md` and the project root `README.md`, and clear the corresponding entry from the **Pending CSes** list above.
```

- [ ] **Step 4.2: Commit**

```bash
git -C /home/alex/dev/cook add cli/crates/cook-lang/CONFORMANCE.md
git -C /home/alex/dev/cook commit -m "docs(cook-lang): add CONFORMANCE.md describing the v0.1 claim"
```

### Task 5: Teach the harness `COOK_CONFORMANCE_CORPUS` and add a summary line

**Files:**
- Modify: `cli/crates/cook-lang/tests/conformance.rs`

- [ ] **Step 5.1: Replace `corpus_root()` with an env-aware version**

In `cli/crates/cook-lang/tests/conformance.rs`, replace lines 15–20 (the entire `corpus_root` function) with:

```rust
fn corpus_root() -> PathBuf {
    if let Ok(override_path) = std::env::var("COOK_CONFORMANCE_CORPUS") {
        return PathBuf::from(override_path)
            .canonicalize()
            .unwrap_or_else(|e| panic!("COOK_CONFORMANCE_CORPUS does not resolve: {}", e));
    }
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../../standard/conformance")
        .canonicalize()
        .expect("conformance corpus root missing")
}
```

- [ ] **Step 5.2: Add a `summary()` test that prints the claim and corpus location**

Append to the end of `cli/crates/cook-lang/tests/conformance.rs`:

```rust

#[test]
fn conformance_summary() {
    let root = corpus_root();
    eprintln!(
        "cook-lang claims Cook Standard v{} (corpus: {})",
        cook_lang::COOK_STANDARD_VERSION,
        root.display(),
    );
}
```

This is a separate test rather than embedded in the positive/negative tests so it always runs and prints regardless of corpus failures. `cargo test` shows `eprintln!` output via `--nocapture` or for tests that fail; for a passing test the line goes to the test runner's captured output, which is shown via `cargo test -- --nocapture`. That's an acceptable trade-off — the constant is also surfaced via `cook --version` for routine inspection.

- [ ] **Step 5.3: Run the harness to confirm everything still passes**

```bash
cd /home/alex/dev/cook/cli && cargo test -p cook-lang --test conformance
```

Expected: 3 tests pass (`positive_conformance_corpus`, `negative_conformance_corpus`, `conformance_summary`).

- [ ] **Step 5.4: Commit**

```bash
git -C /home/alex/dev/cook add cli/crates/cook-lang/tests/conformance.rs
git -C /home/alex/dev/cook commit -m "test(cook-lang): honor COOK_CONFORMANCE_CORPUS and add summary test"
```

### Task 6: Add the backwards-conformance script

**Files:**
- Create: `standard/scripts/check-conformance-against-tag.sh`

- [ ] **Step 6.1: Write the script**

```bash
#!/usr/bin/env bash
#
# Verify cook-lang against the Cook Standard corpus from a previously-cut
# version. Usage:
#
#     standard/scripts/check-conformance-against-tag.sh v0.1
#
# Materializes standard/conformance/ from the cs-standard/<version> tag into
# a temporary directory and runs the conformance harness with
# COOK_CONFORMANCE_CORPUS pointed at that directory.
#
# See standard/specs/2026-04-26-cli-standard-conformance-workflow-design.md.

set -euo pipefail

if [ "$#" -ne 1 ]; then
  echo "usage: $0 <version>  (e.g. v0.1)" >&2
  exit 2
fi

version="$1"
tag="cs-standard/${version}"

repo_root="$(git rev-parse --show-toplevel)"
cd "$repo_root"

if ! git rev-parse --verify --quiet "$tag" >/dev/null; then
  echo "error: tag '$tag' not found in this repository" >&2
  exit 1
fi

tmpdir="$(mktemp -d -t cook-conformance-XXXXXX)"
trap 'rm -rf "$tmpdir"' EXIT

# Restore the conformance/ subtree from the tag into tmpdir/conformance.
mkdir -p "$tmpdir/conformance"
git archive "$tag" "standard/conformance" \
  | tar -x -C "$tmpdir" --strip-components=2

if [ ! -d "$tmpdir/conformance/positive" ]; then
  echo "error: tag '$tag' did not contain standard/conformance/positive" >&2
  exit 1
fi

echo "Running cook-lang conformance harness against $tag"
echo "Corpus: $tmpdir/conformance"

COOK_CONFORMANCE_CORPUS="$tmpdir/conformance" \
  cargo test --manifest-path "$repo_root/cli/Cargo.toml" \
    -p cook-lang --test conformance
```

- [ ] **Step 6.2: Make it executable**

```bash
chmod +x /home/alex/dev/cook/standard/scripts/check-conformance-against-tag.sh
```

- [ ] **Step 6.3: Smoke test against the existing tag (if one exists) or skip until a tag is cut**

Check whether `cs-standard/v0.1` exists:

```bash
git -C /home/alex/dev/cook tag --list 'cs-standard/*'
```

If a tag exists, run the script and confirm it succeeds. If no tag yet exists (the repo is pre-cut), skip — the script will be exercised the first time a cut tag lands.

- [ ] **Step 6.4: Commit**

```bash
git -C /home/alex/dev/cook add standard/scripts/check-conformance-against-tag.sh
git -C /home/alex/dev/cook commit -m "tool(standard): add check-conformance-against-tag.sh for backwards conformance"
```

### Task 7: Extend the pre-commit hook with the second arm

**Files:**
- Modify: `.githooks/pre-commit`

The hook currently warns when language-surface code changes without `standard/`. Add a complementary arm that warns when `D-changes.mdx` adds a CS without growing `standard/conformance/`.

- [ ] **Step 7.1: Add a second-arm check before the existing warning block**

Open `/home/alex/dev/cook/.githooks/pre-commit`. After the existing arm (the `if [ "$touches_language" = "1" ] && [ "$touches_standard" = "0" ]; then` block ending with the `MSG` heredoc), add the following before the `exit 0` at the end of the file:

```bash

# Second arm: warn when D-changes.mdx grows a new CS-NNNN entry without
# growing standard/conformance/. Most language CSes ship a corpus case;
# spec-only CSes (rendering, navigation) legitimately don't, and can
# bypass this warning with COOK_STANDARD_BYPASS=1.

dchanges_path="standard/src/content/docs/appendix/D-changes.mdx"
dchanges_changed=0
conformance_grew=0

while IFS= read -r path; do
  [ -z "$path" ] && continue
  if [ "$path" = "$dchanges_path" ]; then
    dchanges_changed=1
  fi
  case "$path" in
    standard/conformance/*) conformance_grew=1 ;;
  esac
done <<EOF
$staged
EOF

if [ "$dchanges_changed" = "1" ]; then
  # Only warn if the diff adds at least one new "## CS-NNNN" heading.
  added_cs="$(git diff --cached -- "$dchanges_path" \
    | grep -E '^\+##[[:space:]]+CS-[0-9]{4}' || true)"
  if [ -n "$added_cs" ] && [ "$conformance_grew" = "0" ]; then
    cat >&2 <<'MSG'
warning: this commit adds a CS entry to D-changes.mdx without growing standard/conformance/.

Most language-surface CSes should ship at least one positive or negative
conformance case. Spec-only CSes (rendering plugins, doc cleanups,
navigation) legitimately do not; bypass with COOK_STANDARD_BYPASS=1.

Newly added CS heading(s):
MSG
    printf '%s\n' "$added_cs" >&2
    echo "" >&2
  fi
fi
```

- [ ] **Step 7.2: Verify the hook still runs cleanly with no relevant staged changes**

The hook is a no-op if nothing surfaces. Stage only this change to the hook file and run:

```bash
git -C /home/alex/dev/cook add .githooks/pre-commit
git -C /home/alex/dev/cook diff --cached --name-only
```

Expected: only `.githooks/pre-commit` listed; the hook would then exit 0 on commit (the hook does not warn about itself because `.githooks/pre-commit` is neither language-surface nor `D-changes.mdx`).

- [ ] **Step 7.3: Commit**

```bash
git -C /home/alex/dev/cook commit -m "tool(githooks): warn when D-changes.mdx adds a CS without conformance/ growth"
```

### Task 8: Update CONTRIBUTING.md

**Files:**
- Modify: `CONTRIBUTING.md`

CONTRIBUTING.md already covers spec-first rule, hook installation, language-surface paths, conformance, cut procedure, and implementation conformance claims. Add a new section between **Conformance** and **Cutting a Cook Standard version** documenting the workflow's two harness modes and the bumping ritual.

- [ ] **Step 8.1: Insert a new section**

After the existing **Conformance** section (which ends with the line `- A tree-sitter harness against the same corpus is planned; see `D-changes.mdx` CS-0002.`), insert:

```markdown

### `cook-lang` conformance workflow

The Rust parser claims a Cook Standard version via the `pub const COOK_STANDARD_VERSION: &str = "X.Y";` constant in `cli/crates/cook-lang/src/lib.rs`. This constant is the single source of truth; the README and `cook --version` output mirror it.

**Default harness mode.** `cargo test -p cook-lang --test conformance` walks `standard/conformance/` as it exists in the working tree. Every case must pass. When the parser falls behind a spec change, this gate goes red — that's the visible signal to catch up. There is no separate ledger of "pending CSes"; the failing harness is the ledger.

**Backwards-conformance mode.** `standard/scripts/check-conformance-against-tag.sh <vX.Y>` materializes the corpus from the `cs-standard/<vX.Y>` git tag and runs the harness against that corpus. Use this to verify the parser still satisfies a previously-cut version during a brief catch-up window, or to bisect when a regression appeared.

**Bumping the claim.** When the parser catches up to a new cut, bump `COOK_STANDARD_VERSION` in `cli/crates/cook-lang/src/lib.rs` to match `standard/VERSION` in the same commit. Update the claim in `cli/crates/cook-lang/README.md`, the project root `README.md`, and `cli/crates/cook-lang/CONFORMANCE.md`'s "Pending CSes" section. The conformance harness should be green at that commit.

**Brief catch-up windows.** A spec-side commit may land conformance cases for a CS without simultaneously implementing the parser change, in which case the default harness will fail until catch-up. This is allowed. The backwards-conformance script can verify the parser still conforms to the previous cut during the window.
```

- [ ] **Step 8.2: Verify the file builds (CONTRIBUTING.md is plain markdown — no build step, just sanity-read)**

Open the file and confirm the new section flows between **Conformance** and **Cutting a Cook Standard version** without breaking the surrounding hierarchy.

- [ ] **Step 8.3: Commit**

```bash
git -C /home/alex/dev/cook add CONTRIBUTING.md
git -C /home/alex/dev/cook commit -m "docs(contributing): document cook-lang conformance workflow and bumping ritual"
```

---

## Phase 2 — Close the v0.2 gap (CS-0011 conformance)

Phase 2 makes the parser conformant to Cook Standard v0.2. Task 9 lands the negative case alone, which fails CI on `main` — proving the workflow surfaces drift. Task 10 fixes everything in one tightly-coupled refactor; that commit lights the harness back to green.

### Task 9: Add the CS-0011 negative conformance case (CI will go red)

**Files:**
- Create: `standard/conformance/negative/007-bare-vardecl-rejected/Cookfile`
- Create: `standard/conformance/negative/007-bare-vardecl-rejected/error.txt`
- Create: `standard/conformance/negative/007-bare-vardecl-rejected/notes.md`

This task lands a negative case asserting that top-level `NAME "value"` is rejected, per CS-0011. The parser currently accepts the form, so the harness will fail on this commit and continue failing until Task 11 catches up. **This is intentional** — the failing harness is the workflow's signal that catch-up is owed.

- [ ] **Step 9.1: Make the case directory**

```bash
mkdir -p /home/alex/dev/cook/standard/conformance/negative/007-bare-vardecl-rejected
```

- [ ] **Step 9.2: Write the input Cookfile**

`standard/conformance/negative/007-bare-vardecl-rejected/Cookfile`:

```
CC "gcc"

recipe "build"
    @echo hello
end
```

- [ ] **Step 9.3: Write the expected error substring**

`standard/conformance/negative/007-bare-vardecl-rejected/error.txt`:

```
unexpected content outside of a recipe
```

The Standard at v0.2 specifies that top-level `NAME "value"` is no longer a recognized form; the parser MUST reject any non-recipe-non-config-non-use-non-import top-level content. Once `Token::VarDecl` is removed (Task 11), the lexer produces `Token::Content("CC \"gcc\"")` for this line, which `lib.rs::parse()` already rejects with the message `"unexpected content outside of a recipe"` (existing error at `lib.rs:102-107`). That's the substring the harness will look for.

- [ ] **Step 9.4: Write notes.md**

`standard/conformance/negative/007-bare-vardecl-rejected/notes.md`:

```markdown
Asserts that top-level `NAME "value"` is rejected at v0.2 of the Cook Standard. Per CS-0011 (App. D), the `variable_declaration` form was removed from §2 (lexical) and §3 (syntactic grammar) in favor of config-block-only variables (§3.6.1). The parser MUST therefore treat top-level non-keyword content as an error.

Exercises the negative side of §3.1 (top-level production list) and the absence of §3.3 (formerly variable_declaration).
```

- [ ] **Step 9.5: Run the harness — it WILL fail. That is intentional.**

```bash
cd /home/alex/dev/cook/cli && cargo test -p cook-lang --test conformance 2>&1 | tail -30
```

Expected: a `negative_conformance_corpus` failure reporting `case 007-bare-vardecl-rejected: expected parse error, got success`. The other tests pass.

- [ ] **Step 9.6: Commit anyway**

The pre-commit hook second arm (Task 7) will be silent here because the commit grows `standard/conformance/` — that's exactly the case the warning is encouraging.

```bash
git -C /home/alex/dev/cook add standard/conformance/negative/007-bare-vardecl-rejected/
git -C /home/alex/dev/cook commit -m "$(cat <<'EOF'
test(standard): add CS-0011 negative — top-level NAME "value" rejected

Lands the negative conformance case alone. The parser currently accepts
the rejected form, so cargo test -p cook-lang --test conformance will
fail until Task 10 of the workflow plan catches up the parser. The
failing harness is the workflow's signal that catch-up is owed.
EOF
)"
```

### Task 10: Remove `Token::VarDecl` and update all consumers (gap-close commit)

**Files:** (all modify, one commit)
- `cli/crates/cook-lang/src/lexer.rs`
- `cli/crates/cook-lang/src/recipe.rs`
- `cli/crates/cook-lang/src/ast.rs`
- `cli/crates/cook-lang/src/cook_line.rs`
- `cli/crates/cook-lang/src/lib.rs`
- `cli/crates/cook-lang/src/tests.rs`
- `cli/crates/cook-lang/tests/conformance.rs`
- `cli/crates/cook-cli/src/env.rs`
- `cli/crates/cook-luagen/src/dep_ref.rs`
- `cli/crates/cook-luagen/src/tests.rs`
- All 14 `standard/conformance/positive/*/parse.txt`

This is the gap-close: removing `Token::VarDecl` cascades through the workspace because the variant, the AST field `vars`, and several test constructions all reference it. The change must compile as a unit; treat the steps below as sub-edits within a single commit.

- [ ] **Step 10.1: Remove `Token::VarDecl` from the lexer's token enum**

In `cli/crates/cook-lang/src/lexer.rs`, line 8, delete:

```rust
    VarDecl { name: String, value: String },
```

After deletion, the `Token` enum's `Comment`, `RecipeHeader`, `ConfigHeader` lines flow directly into `UseDecl`, `ImportDecl`, etc.

- [ ] **Step 10.2: Remove the `try_parse_var_decl` helper**

In `cli/crates/cook-lang/src/lexer.rs`, delete the entire function spanning lines 88–109 (`fn try_parse_var_decl(...) -> Option<(String, String)> { ... }`).

- [ ] **Step 10.3: Remove the three `try_parse_var_decl` call sites in `tokenize`**

In `cli/crates/cook-lang/src/lexer.rs`, the three call sites are inside the trailing-token classification cascade (lines 186–219). Replace the entire block from `} else if !line.starts_with(...)` (line 186) through `};` (line 219) with the simplified version that drops VarDecl entirely:

```rust
        } else if !line.starts_with(|c: char| c.is_whitespace()) {
            // Bare top-level line.
            if let Some(colon_pos) = trimmed.find(':') {
                let potential_name = &trimmed[..colon_pos];
                if !potential_name.is_empty()
                    && is_ident_start(potential_name.as_bytes()[0] as char)
                    && potential_name.chars().all(is_ident_char)
                {
                    check_reserved_recipe_name(potential_name, line_num)?;
                    let after_colon = trimmed[colon_pos + 1..].trim();
                    let deps = parse_names(after_colon, line_num)?;
                    Token::RecipeHeader {
                        name: potential_name.to_string(),
                        deps,
                    }
                } else {
                    Token::Content(trimmed.to_string())
                }
            } else {
                Token::Content(trimmed.to_string())
            }
        } else {
            // Indented line: shell command or `@`-prefix interactive shell.
            Token::Content(trimmed.to_string())
        };
```

This collapses the three `try_parse_var_decl` arms into plain `Token::Content`. Top-level `NAME "value"` becomes `Token::Content` and gets rejected by `parse()` in `lib.rs`. Indented `NAME "value"` becomes `Token::Content` and gets handled by `parse_recipe`'s existing fallthrough that pushes a `Step::Shell` (recipe.rs lines 211–215). Behavior at the recipe level is preserved.

- [ ] **Step 10.4: Remove the two VarDecl unit tests in the lexer's `mod tests`**

In `cli/crates/cook-lang/src/lexer.rs`, delete the two tests:

- `test_var_decl` (currently lines 487–497).
- `test_var_decl_with_spaces_in_value` (currently lines 499–510).

Also delete `test_use_decl_not_var` (currently lines 538–542) — it asserts that a `use` line is not a VarDecl, which becomes meaningless once `Token::VarDecl` no longer exists.

- [ ] **Step 10.5: Remove the `Token::VarDecl` arm from `parse_recipe`**

In `cli/crates/cook-lang/src/recipe.rs`, delete the arm at lines 260–269:

```rust
            Token::VarDecl { name: var_name, value } => {
                // Inside a recipe, NAME "value" is a shell command, not a var decl
                let command = format!("{} \"{}\"", var_name, value);
                steps.push(Step::Shell {
                    command,
                    line: tok.line,
                    interactive: false,
                });
                pos += 1;
            }
```

The `Token::Content` arm earlier in the same `match` (lines 143–218) handles indented `NAME "value"` lines as shell steps via the fallthrough at lines 211–215 (`steps.push(Step::Shell { command: text.clone(), ... })`). The reconstructed `format!` from the deleted arm produces the same string content as `text.clone()` for a line like `CC "gcc"`, so behavior is preserved.

- [ ] **Step 10.6: Remove the `vars` field from the `Cookfile` AST**

In `cli/crates/cook-lang/src/ast.rs`, line 23, delete:

```rust
    pub vars: Vec<(String, String)>,
```

Also remove the `vars: vec![],` line from the two `Cookfile { ... }` test constructions in the same file:

- Line 166 (`test_cookfile_with_uses`).
- Line 201 (`test_cookfile_with_config_blocks`).

- [ ] **Step 10.7: Remove the VarDecl arm and `vars` plumbing from `lib.rs::parse()`**

In `cli/crates/cook-lang/src/lib.rs`:

- Delete the line `let mut vars = Vec::new();` (currently line 26).
- Delete the entire `Token::VarDecl { name, value } => { ... }` arm (currently lines 38–47).
- Update the final `Ok(Cookfile { vars, config_blocks, recipes, uses, imports })` (currently line 150) to drop the `vars,` field, becoming:

```rust
    Ok(Cookfile { config_blocks, recipes, uses, imports })
```

- [ ] **Step 10.8: Remove the dead `parse_quoted_strings_parser` from `cook_line.rs`**

In `cli/crates/cook-lang/src/cook_line.rs`, delete lines 19–38 (`pub(crate) fn parse_quoted_strings_parser(...) { ... }`). It existed only to support VarDecl-related parsing and is unused (the `dead_code` warning visible in `cargo build` confirms this).

- [ ] **Step 10.9: Update `cli/crates/cook-lang/src/tests.rs`**

Delete or rewrite the following tests:

- **Delete `test_bare_vars_parsed`** (currently lines 420–434): it asserts that two bare top-level vars parse into `result.vars`. After CS-0011, top-level `CC "gcc"` is rejected, so the test's source no longer parses successfully.

- **Rewrite `test_vars_and_configs_together`** (currently lines 456–472): the input contains `CC "gcc"` at top level, which now fails. Delete the test entirely — config-block-only variable surface is already covered by `test_mixed_named_and_unnamed_config_blocks` (lines 436–454) and the new positive conformance case in Task 11.

- **Rewrite `test_var_after_recipe_is_shell_command`** (currently lines 488–501) → rename to `test_indented_quoted_pair_is_shell_command` and update the `assert_eq!(result.vars.len(), 0);` line. The `Cookfile` now no longer has a `vars` field, so the assertion goes. The other assertions (a `Step::Shell` containing `"CC"`) still hold. Replace the test body with:

```rust
#[test]
fn test_indented_quoted_pair_is_shell_command() {
    let source = r#"recipe "build"
    CC "gcc"
end
"#;
    let result = parse(source).unwrap();
    assert_eq!(result.recipes.len(), 1);
    assert!(matches!(
        &result.recipes[0].steps[0],
        Step::Shell { command, .. } if command.contains("CC")
    ));
}
```

- **Update `test_parse_use_with_vars_and_configs`** (currently lines 633–640): the source contains `CC "gcc"`, which now fails. Replace the source with one that uses a config block instead:

```rust
#[test]
fn test_parse_use_with_configs() {
    let source = "use \"cpp\"\n\nconfig \"debug\"\n    env.CFLAGS = \"-g\"\nend\n\nrecipe \"build\"\n    @echo hello\nend\n";
    let cookfile = crate::parse(source).unwrap();
    assert_eq!(cookfile.uses.len(), 1);
    assert_eq!(cookfile.config_blocks.len(), 1);
    assert_eq!(cookfile.recipes.len(), 1);
}
```

(Renamed to drop `_vars_` from the test name.)

- [ ] **Step 10.10: Update `cli/crates/cook-cli/src/env.rs`**

In `cli/crates/cook-cli/src/env.rs`:

- Update the docstring layer list at lines 1–9 to drop layer 2 ("Cookfile bare vars"). Replace lines 1–9 with:

```rust
//! Environment resolution: layered variable loading.
//!
//! Layer order (later wins):
//!   1. System env
//!   2. .env file (dotenvy)
//!   3. CLI --set flags
//!
//! Cookfile-defined variables live inside `config ... end` Lua blocks
//! and are applied at runtime, not as part of this static layering.
```

- Delete the loop at lines 40–43 (`// Layer 2: Cookfile bare vars`...`for (k, v) in &cookfile.vars { ... }`). Renumber the remaining inline `// Layer 3:` and `// Layer 4:` comments to `// Layer 2:` and `// Layer 3:`.

- The `cookfile: &cook_lang::ast::Cookfile` parameter is still used downstream (or the call site passes it in). After this edit, `cookfile` is unused in `resolve_env`'s body. Either:
  - Remove the parameter entirely (preferred — touch the call site too); or
  - Add a leading `_ = cookfile;` line to suppress the unused-variable warning, and drop the parameter in a follow-up.

  Choose the first. Find the single call to `resolve_env` (search via `grep -rn 'resolve_env(' cli/crates/cook-cli/src/`) and remove the `cookfile,` argument from that call site. Update the function signature to remove `cookfile: &cook_lang::ast::Cookfile,`.

- [ ] **Step 10.11: Update `cli/crates/cook-luagen/src/dep_ref.rs`**

In `cli/crates/cook-luagen/src/dep_ref.rs`, line 129, delete the `vars: vec![],` line inside the `Cookfile { ... }` literal.

- [ ] **Step 10.12: Update `cli/crates/cook-luagen/src/tests.rs`**

Delete the seven `vars: vec![],` lines in `Cookfile { ... }` constructions at lines 9, 514, 607, 980, 998, 1017, and 1030 (the line numbers may shift slightly after earlier deletions; use grep to find them):

```bash
grep -n 'vars: vec!\[\],' /home/alex/dev/cook/cli/crates/cook-luagen/src/tests.rs
```

Each match is a single line inside a struct literal. Delete the line itself.

- [ ] **Step 10.13: Update the conformance harness `format_cookfile`**

In `cli/crates/cook-lang/tests/conformance.rs`:

- Delete `format_var` at lines 110–112.
- Delete the `vars` lines inside `format_cookfile` at lines 132–133:

```rust
    let vars: Vec<String> = c.vars.iter().map(format_var).collect();
    out.push_str(&format!("  vars: [{}]\n", vars.join(", ")));
```

After deletion, `format_cookfile` outputs `uses`, `imports`, `config_blocks`, `recipes` — no more `vars` line.

- [ ] **Step 10.14: Update all 14 positive `parse.txt` files**

Each positive case's `parse.txt` currently includes a line `  vars: []` between `imports:` and `config_blocks:`. Delete that line from every file.

```bash
for f in /home/alex/dev/cook/standard/conformance/positive/*/parse.txt; do
  sed -i '/^  vars: \[\]$/d' "$f"
done
```

After running, sanity-check one file:

```bash
head /home/alex/dev/cook/standard/conformance/positive/001-empty-recipe/parse.txt
```

Expected: the `vars: []` line is gone; output starts with `Cookfile`, `  uses: []`, `  imports: []`, `  config_blocks: []`, `  recipes:`.

- [ ] **Step 10.15: Build and run the full test suite**

```bash
cd /home/alex/dev/cook/cli && cargo build -q 2>&1 | tail -20
```

Expected: clean build, no errors, no `dead_code` warning for `parse_quoted_strings_parser` (it was deleted in 10.8).

```bash
cd /home/alex/dev/cook/cli && cargo test 2>&1 | tail -30
```

Expected: all tests pass, including `positive_conformance_corpus`, `negative_conformance_corpus` (now containing the CS-0011 negative case from Task 9), and `conformance_summary`.

If anything fails, fix the offending code/test before committing. Common issues:

- `vars` referenced somewhere not listed above: grep the workspace (`grep -rn 'cookfile.vars\|\.vars\b\|vars:' cli/crates/`) and update.
- AST test `test_recipe_construction` or similar still constructing `Cookfile { vars: vec![], ... }`: drop the `vars` field there too.

- [ ] **Step 10.16: Commit the entire gap-close as one commit**

```bash
git -C /home/alex/dev/cook add \
  cli/crates/cook-lang/src/lexer.rs \
  cli/crates/cook-lang/src/recipe.rs \
  cli/crates/cook-lang/src/ast.rs \
  cli/crates/cook-lang/src/cook_line.rs \
  cli/crates/cook-lang/src/lib.rs \
  cli/crates/cook-lang/src/tests.rs \
  cli/crates/cook-lang/tests/conformance.rs \
  cli/crates/cook-cli/src/env.rs \
  cli/crates/cook-luagen/src/dep_ref.rs \
  cli/crates/cook-luagen/src/tests.rs \
  standard/conformance/positive/

git -C /home/alex/dev/cook commit -m "$(cat <<'EOF'
feat(cook-lang): implement CS-0011 — remove top-level VarDecl

Closes the v0.2 conformance gap. Removes Token::VarDecl from the lexer
(variant, helper, three producer call sites, three unit tests),
removes the recipe-body fallthrough in recipe.rs (the existing
Token::Content arm covers indented NAME "value" via Step::Shell
fallthrough), removes the vars field from Cookfile AST, removes the
vars layer from cook-cli/src/env.rs, and updates cook-luagen tests.
The conformance harness drops format_var and the corresponding parse.txt
line; all 14 existing positive parse.txt fixtures are regenerated to
match.

Verified by the new CS-0011 negative case (007-bare-vardecl-rejected)
landed in the previous commit, which now passes.
EOF
)"
```

### Task 11: Add the CS-0011 positive conformance case

**Files:**
- Create: `standard/conformance/positive/015-config-block-only-vars/Cookfile`
- Create: `standard/conformance/positive/015-config-block-only-vars/parse.txt`
- Create: `standard/conformance/positive/015-config-block-only-vars/notes.md`

A positive case showing the canonical replacement surface for top-level vars: a config block setting variables via `cook.env.X` (or the `env` alias normatively introduced in CS-0011 via §6).

- [ ] **Step 11.1: Make the case directory**

```bash
mkdir -p /home/alex/dev/cook/standard/conformance/positive/015-config-block-only-vars
```

- [ ] **Step 11.2: Write the input Cookfile**

`standard/conformance/positive/015-config-block-only-vars/Cookfile`:

```
config
    cook.env.CC = "gcc"
    cook.env.CFLAGS = "-Wall"
end

recipe "build"
    @echo hello
end
```

- [ ] **Step 11.3: Generate the expected parse.txt**

Rather than hand-write the AST dump, run the parser to produce it, copy it to `parse.txt`, then verify:

```bash
cd /home/alex/dev/cook/cli
cargo test -p cook-lang --test conformance positive_conformance_corpus 2>&1 \
  | grep -A 50 '015-config-block-only-vars'
```

The harness will fail on this case because `parse.txt` doesn't exist yet, and the output includes the actual AST shape under `--- actual ---`. Copy that exact text into `parse.txt`, taking care to preserve indentation. The expected shape, given the harness format and the input above:

`standard/conformance/positive/015-config-block-only-vars/parse.txt`:

```
Cookfile
  uses: []
  imports: []
  config_blocks: [ConfigBlock name=None body="    cook.env.CC = \"gcc\"\n    cook.env.CFLAGS = \"-Wall\"" line=1]
  recipes:
    Recipe name="build" line=6
      deps: []
      ingredients: []
      excludes: []
      steps:
        Shell interactive=true command="echo hello"
```

If the harness output differs from the above (line numbers, body whitespace, escaped quotes), use the harness output as authoritative. Whitespace-trim the trailing newline.

- [ ] **Step 11.4: Write notes.md**

`standard/conformance/positive/015-config-block-only-vars/notes.md`:

```markdown
Demonstrates the canonical replacement for the removed top-level `variable_declaration` form (CS-0011). Variables are written into `cook.env.X` from inside an unnamed (base) `config` block, exercising §3.6.1 (config-block composition) and §6 (Cook Lua API).

Replaces the now-rejected pattern that the negative case `007-bare-vardecl-rejected` covers.
```

- [ ] **Step 11.5: Run the harness — should now pass**

```bash
cd /home/alex/dev/cook/cli && cargo test -p cook-lang --test conformance
```

Expected: all 3 tests pass; the new positive case is silently included.

- [ ] **Step 11.6: Commit**

```bash
git -C /home/alex/dev/cook add standard/conformance/positive/015-config-block-only-vars/
git -C /home/alex/dev/cook commit -m "test(standard): add CS-0011 positive — config-block-only variables"
```

### Task 12: Bump the claim to v0.2

**Files:**
- Modify: `cli/crates/cook-lang/src/lib.rs`
- Modify: `cli/crates/cook-lang/CONFORMANCE.md`
- Modify: `cli/crates/cook-lang/README.md`
- Modify: `README.md`

The parser is now conformant to v0.2. Time to advertise it.

- [ ] **Step 12.1: Bump the constant**

In `cli/crates/cook-lang/src/lib.rs`, change:

```rust
pub const COOK_STANDARD_VERSION: &str = "0.1";
```

to:

```rust
pub const COOK_STANDARD_VERSION: &str = "0.2";
```

- [ ] **Step 12.2: Update `CONFORMANCE.md`**

In `cli/crates/cook-lang/CONFORMANCE.md`, change:

- The `## Claim` line `cook-lang claims **Cook Standard v0.1**.` → `cook-lang claims **Cook Standard v0.2**.`
- The "Pending CSes" section: remove the CS-0011 bullet so the list is empty. Replace the bullet with the line:

```markdown
None at this version.
```

- [ ] **Step 12.3: Update `cli/crates/cook-lang/README.md`**

Change `claims **Cook Standard v0.1**` → `claims **Cook Standard v0.2**`. Update the example invocation of the backwards-conformance script to reference `v0.1` (the previously-cut version that the parser still satisfies):

```markdown
standard/scripts/check-conformance-against-tag.sh v0.1
```

(That line may already say `v0.1` — if so, leave it.)

- [ ] **Step 12.4: Update the project root `README.md`**

Change the project description's claim line from `claims **Cook Standard v0.1**` → `claims **Cook Standard v0.2**`.

- [ ] **Step 12.5: Verify `cook --version` output**

```bash
cd /home/alex/dev/cook/cli && cargo build -q --bin cook && ./target/debug/cook --version
```

Expected: `cook 0.1.0 (Cook Standard v0.2)`.

- [ ] **Step 12.6: Commit**

```bash
git -C /home/alex/dev/cook add \
  cli/crates/cook-lang/src/lib.rs \
  cli/crates/cook-lang/CONFORMANCE.md \
  cli/crates/cook-lang/README.md \
  README.md
git -C /home/alex/dev/cook commit -m "chore(cook-lang): bump COOK_STANDARD_VERSION to 0.2"
```

---

## Final verification

After all tasks complete, the full repo should be in a green, conformant state. Run:

```bash
cd /home/alex/dev/cook/cli && cargo build -q && cargo test -q
cd /home/alex/dev/cook/standard && pnpm build && pnpm lint:keywords
./target/debug/cook --version  # if standard's pnpm build doesn't change directories, run from cli/
```

Expected:
- `cargo build`: clean.
- `cargo test`: all tests pass, including 3 conformance harness tests.
- `pnpm build`: clean Astro build (no rehype-bare-ref-lint errors).
- `pnpm lint:keywords`: no lowercase normative keywords flagged.
- `cook --version`: prints `cook 0.1.0 (Cook Standard v0.2)`.

If a previous-version git tag exists (e.g., `cs-standard/v0.1` was cut at some point in the future), also run:

```bash
standard/scripts/check-conformance-against-tag.sh v0.1
```

Expected: harness passes against the v0.1 corpus, demonstrating backwards conformance.

## Self-review notes

- **Spec coverage:** Every section of the design has at least one task. §3 (boundary): tasks 9, 11 (impl-agnostic case format with prose-only `notes.md`). §4 (mechanism): tasks 1, 2, 5, 6 (constant, version surface, harness env override, backwards script). §5 (workflow): task 8 (CONTRIBUTING.md). §6 (tooling): tasks 6, 7 (script, hook). §7 (step-zero migration, items 1–9): tasks 1–12 cover all nine items.
- **Type/name consistency:** `COOK_STANDARD_VERSION` used identically across lib.rs, CONFORMANCE.md, README, and CONTRIBUTING.md. `COOK_CONFORMANCE_CORPUS` env var consistent in conformance.rs and the script. Function `format_cookfile` referenced once.
- **No placeholders:** Every step contains the actual code or text to write. The only "use the harness output as authoritative" instruction (Step 11.3) is a deliberate trade-off — generating the parse.txt by running the parser is more reliable than hand-writing the AST dump and risking format drift.
- **Out-of-scope confirmed:** Runtime-effects harness, tree-sitter conformance, multi-implementation coordination — all explicitly deferred per design §2 and §8.
