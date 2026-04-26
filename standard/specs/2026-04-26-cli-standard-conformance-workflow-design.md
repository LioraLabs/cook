# Design: Keeping `cook-lang` in sync with the Cook Standard

**Date:** 2026-04-26
**Status:** Design — pending implementation plan
**Standard change ID:** Project-side only; does not modify normative chapters. May warrant a CS for the new section in CONTRIBUTING.md and the new tooling under `standard/scripts/`, but no chapter or appendix changes are required.
**Scope:** The workflow, tooling, and conventions that govern how `cli/crates/cook-lang/` (the Rust parser, current reference implementation) tracks the Cook Standard. The Standard itself is unchanged: no per-CS implementation status, no per-case version metadata.

## 1. Motivation

The Cook Standard cut v0.2 with CS-0011 (VarDecl removal). The Rust parser still has `Token::VarDecl` wired through `lexer.rs`, `recipe.rs`, and `lib.rs`, and the conformance harness reports green because the corpus has no negative case for the now-rejected top-level `NAME "value"` form. The gap is invisible to CI.

The repo has the building blocks for an honest sync workflow — a portable pre-commit hook (`.githooks/pre-commit`), a conformance harness (`cli/crates/cook-lang/tests/conformance.rs`), and a project convention that each implementation expose a `COOK_STANDARD_VERSION` constant (CONTRIBUTING.md) — but they are not assembled into a workflow that catches drift. This design assembles them.

The workflow must also leave the Standard impl-agnostic: it is a normative document; it should not name `cook-lang`, `tree-sitter-cook`, or any future implementation in its body.

## 2. Non-goals

- **Runtime-effects conformance.** Cases assert parser behavior only — "parses" or "fails to parse with error matching X." Execution semantics, scheduler ordering, and Lua API runtime behavior are out of scope. They are tested elsewhere; if a future CS adjusts something the parser cannot observe, the impl maintainer is on the honor system. A runtime harness can be designed when an actual non-parser-observable CS lands.
- **Tree-sitter conformance.** `tree-sitter-cook` is formally stale (CS-0002). Its catch-up plan is its own future CS. This design touches only the Rust parser.
- **Per-CS implementation status fields in the Standard.** The Standard does not record which implementations have caught up to which CS. That information lives in each implementation's repo.
- **Per-case version metadata in the corpus.** Cases are not tagged with `cs:` or `since:`. The `cs-standard/vX.Y` git tags are the version axis; to inspect the corpus at a previous version, check out that tag.
- **Multi-implementation coordination protocols.** Pre-1.0, only `cook-lang` claims conformance. The model extends naturally — each impl gets its own version constant, harness, and status doc — but no abstractions are introduced for impls that do not yet exist.

## 3. The boundary between Standard and implementation

**`standard/` is normative and implementation-agnostic.** It contains:

- The chapters (`standard/src/content/docs/`).
- The corpus (`standard/conformance/positive/`, `standard/conformance/negative/`).
- The grammar appendix.
- The change log (`appendix/D-changes.mdx`).
- The `VERSION` file.

The Standard does not name implementations. It does not record which CSes are implemented where. A reader of the Standard learns what the language *is*, not who has caught up to it.

**Each implementation claims and verifies conformance from its own side.** For `cook-lang`:

- A `COOK_STANDARD_VERSION` constant in the crate, exported and surfaced in `cook --version` and the README.
- A conformance harness that walks `standard/conformance/`.
- An optional human-readable status doc (`cli/crates/cook-lang/CONFORMANCE.md`) summarizing what the constant means and how to run the harness.

Future implementations follow the same pattern in their own trees. CS-0002 already records that `tree-sitter-cook` is stale; it makes no claim and need not until its harness lands.

## 4. Mechanism

### 4.1 Corpus shape

The corpus lives at `standard/conformance/{positive,negative}/`. Each case directory contains:

- `Cookfile` — the input.
- `parse.txt` (positive) — the expected parse-tree dump.
- `error.txt` (negative) — the expected error substring.
- `notes.md` — human-readable description with §-refs to the chapters the case exercises.

`notes.md` is prose only. There is no frontmatter, no `meta.toml`, no `cs:` or `since:` field. The corpus is a single, current snapshot; its history is git's history; its versioning is the `cs-standard/vX.Y` tags.

### 4.2 Implementation claim

`cli/crates/cook-lang/src/lib.rs` exports:

```rust
/// Cook Standard version that this crate claims to fully implement.
/// Must equal `standard/VERSION` at the time the constant was last bumped.
pub const COOK_STANDARD_VERSION: &str = "0.2";
```

The constant is re-exported from `cook-cli` so `cook --version` can include it. The crate's `README.md` and the project root `README.md` state the claim.

### 4.3 Default conformance harness

`cli/crates/cook-lang/tests/conformance.rs` walks `standard/conformance/` *as it exists in the working tree* and asserts every case. Positives must parse and produce output matching `parse.txt`; negatives must fail with an error substring matching `error.txt`. This is the daily gate. It runs as part of `cargo test`.

When the parser is conformant to the Standard at HEAD, this gate is green. When a spec change lands without the parser change, the gate is red — the visibility comes from CI failing, not from a separate ledger.

### 4.4 Backwards-conformance verification

A script at `standard/scripts/check-conformance-against-tag.sh <vX.Y>` materializes the corpus from the `cs-standard/vX.Y` tag into a temp directory and runs the harness against it. The parser at HEAD is checked against the Standard's corpus at that prior cut. Use cases:

- Verifying the parser still conforms to a previous version during a brief catch-up window.
- Bisecting a regression: was this commit conformant to vX.Y at the time?
- Producing release-notes claims: "still conformant to v0.1, also v0.2."

Not part of the default test run; invoked manually or by ad-hoc CI checks.

### 4.5 `cook --version` output

```
cook 0.x.x (Cook Standard v0.2)
```

The Standard version is the single, advertised conformance claim.

## 5. Workflow

### 5.1 Spec change

1. Author the CS in `standard/src/content/docs/appendix/D-changes.mdx` with a new CS-NNNN ID, summary, sections affected, and Version line.
2. Update grammar appendix if relevant.
3. Add at least one case under `standard/conformance/{positive,negative}/` if the change is parser-observable. Cases follow the existing layout (`Cookfile`, `parse.txt` or `error.txt`, `notes.md`).
4. If parser-observable, the parser change lands in the same commit by default. Single-maintainer pre-1.0 discipline; CI being red is the forcing function.

### 5.2 Cut

Existing procedure (CONTRIBUTING.md "Cutting a Cook Standard version"):

1. Bump `standard/VERSION`.
2. Add or extend the App. D Versions index entry.
3. Set each batched CS's `**Version:**` line to the new version.
4. Tag `cs-standard/vX.Y` on the cut commit.

`COOK_STANDARD_VERSION` in `cook-lang` is bumped to match in the same commit, or in an immediately-following commit if the cut is purely a doc/rendering cut with no parser-observable surface.

### 5.3 Brief catch-up window (uncommon, allowed)

If the maintainer chooses to land a spec CS without simultaneously implementing it in the parser:

- The default harness goes red — visible signal that catch-up is owed.
- The backwards-conformance script can verify the parser still satisfies the prior cut, so claims about prior versions remain defensible during the window.
- The next parser PR closes the window: implements the CS, harness goes green.

This is an explicit, time-boxed lag. There is no separate ledger; the red harness *is* the ledger.

### 5.4 Spec-only CSes

CSes that touch only rendering plugins, doc prose, or navigational headings have no corpus impact and no parser impact. They land on the Standard side alone; the parser bumps `COOK_STANDARD_VERSION` on the next cut.

### 5.5 Non-parser-observable CSes (deferred)

CSes that change runtime semantics, scheduler ordering, or Lua API call behavior in ways the parser cannot observe are not gated by this workflow. The CS still ships; the impl maintainer updates behavior on the honor system; the README claim and `CONFORMANCE.md` note are the manual checkpoint. When such a CS lands and proves the gap matters, a follow-up design extends the harness with a runtime-effects mode.

## 6. Tooling

### 6.1 Pre-commit hook

`.githooks/pre-commit` keeps its existing arm — warns when a commit touches language-surface code without also touching `standard/`. Add a complementary arm: warn when `standard/src/content/docs/appendix/D-changes.mdx` grows a new CS-NNNN entry but `standard/conformance/` is not also touched. Bypassable for legitimately spec-only CSes via `COOK_STANDARD_BYPASS=1` (existing escape hatch).

The hook produces warnings, not blocks, consistent with its current behavior.

### 6.2 Backwards-conformance script

`standard/scripts/check-conformance-against-tag.sh <vX.Y>` (bash, matching the existing convention in `standard/scripts/check-normative-keywords.sh`):

1. Resolves the tag (`cs-standard/vX.Y`).
2. Materializes `standard/conformance/` from that tag into a temp directory.
3. Runs `cargo test -p cook-lang --test conformance` with an environment variable (`COOK_CONFORMANCE_CORPUS=<temp-dir>`) that the harness honors as an override.
4. Cleans up the temp dir.
5. Exits with the harness's exit code.

The harness must be taught to read `COOK_CONFORMANCE_CORPUS` and walk that path instead of `standard/conformance/` when set.

### 6.3 `cook --version`

`cli/crates/cook-cli` includes the Standard version in its version output. The constant is the source of truth.

### 6.4 README claims

- Project root `README.md`: states the claim.
- `cli/crates/cook-lang/README.md`: states the claim.
- The constant in `cli/crates/cook-lang/src/lib.rs` is the source of truth; READMEs are mirrors that get hand-updated when the constant moves.

### 6.5 `cli/crates/cook-lang/CONFORMANCE.md`

A short document on the impl side:

- States the current claim and links to the constant.
- Documents how to run the default harness.
- Documents how to run the backwards-conformance script.
- Lists currently-pending CSes if the parser is in a catch-up window (otherwise an empty section, or "no pending CSes").

This is human-maintained. The harness output is authoritative; this doc is for readers who want a quick prose summary.

## 7. Step-zero migration

The work to adopt the workflow and close the existing v0.2 gap. Each numbered item is intended as a separate commit unless noted.

1. **Author CS-0011 corpus cases.** One positive case under `standard/conformance/positive/` exercising config-block-only variables (the now-canonical form). One negative case under `standard/conformance/negative/` asserting that top-level `NAME "value"` is rejected with an appropriate error substring. The positive case's `parse.txt` reflects the parser's current dump format.

2. **Implement CS-0011 in the parser.** Remove `Token::VarDecl` from `cli/crates/cook-lang/src/lexer.rs`. Update `cli/crates/cook-lang/src/recipe.rs` and `cli/crates/cook-lang/src/lib.rs` to drop the variant. Remove `parse_quoted_strings_parser` from `cli/crates/cook-lang/src/cook_line.rs` if it exists only to support VarDecl. Update unit tests; the conformance harness should now report green with the new CS-0011 cases enforced. (Steps 1 and 2 are commit-tightly-coupled — combine if cleaner.)

3. **Add the version constant.** `pub const COOK_STANDARD_VERSION: &str = "0.2";` in `cli/crates/cook-lang/src/lib.rs`. Export through the public API.

4. **Surface the claim in `cook --version`.** Read the constant from `cook-cli` and append `(Cook Standard v0.2)` to the version output.

5. **Add `CONFORMANCE.md`.** `cli/crates/cook-lang/CONFORMANCE.md` covering claim, default harness, backwards-conformance script, pending list (initially empty). Link to it from the crate README.

6. **Update READMEs.** Project root and `cli/crates/cook-lang/` README state the claim.

7. **Add backwards-conformance script.** `standard/scripts/check-conformance-against-tag.sh`, accepting one positional argument (`vX.Y`). Teach the harness to honor `COOK_CONFORMANCE_CORPUS`.

8. **Extend the pre-commit hook.** Second arm warns when `D-changes.mdx` adds a CS without growth under `standard/conformance/`.

9. **Update CONTRIBUTING.md.** Document the workflow: how the parser claims a version, how to bump it, how the default harness gates, how the backwards-conformance script verifies prior versions, and the brief-catch-up-window rule. Cross-reference `CONFORMANCE.md`.

Step ordering rationale: 1 + 2 must be one logical change because removing `Token::VarDecl` without the negative case leaves the spec-defined rejection untested. 3 + 4 + 6 are tightly coupled (constant + surfacing + advertising). 5, 7, 8, 9 are independent and can land in any order.

## 8. Out-of-scope follow-ups

These are noted, not designed:

- **Runtime-effects harness.** Justified by a future non-parser-observable CS.
- **Tree-sitter conformance.** Tracked under CS-0002; its catch-up is its own future design.
- **Multi-implementation coordination.** Patterns will emerge organically as a second `*_VERSION` constant lands.
- **Automated bumping.** A script that validates `COOK_STANDARD_VERSION == standard/VERSION` and warns on mismatch could be added later. Pre-1.0 with one impl, manual discipline plus the red-harness signal is sufficient.

## 9. Open questions

None blocking. The design intentionally avoids decisions that would constrain future implementations.
