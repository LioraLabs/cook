# `$<NAME>` Sigil Placeholder Syntax — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Apply the design in `standard/specs/2026-05-03-sigil-placeholder-syntax-design.md`. After this plan executes, every Cook placeholder in shell text uses the strict `$<IDENT>` sigil shape; `{TOKEN}` is no longer recognized as a placeholder anywhere; resolution is closed-set (builtin → in-scope recipe → declared env var → hard error); the legacy `{...}` scanner is removed; the conformance corpus, examples, and in-tree Cookfiles are migrated; the Standard records the change as CS-0033 cut at v0.7.

**Architecture:** The implementation lands in three layers, in order: (1) parser/scanner/resolver in `cli/crates/cook-{lang,luagen,register}` — strict `$<IDENT>` lexer, closed-set codegen-time resolver for builtins and recipes, runtime `cook.require_env` for declared env vars; (2) parser-aware migration tool exposed as `cook --migrate-sigil` plus a `standard.migrate` chore wrapper; (3) mass corpus/example/in-tree migration via the tool, then spec doc updates and v0.7 cut. Each task is a coherent, revertable commit. TDD throughout: failing test → minimal implementation → green test → commit.

**Tech Stack:** Rust (edition 2021, workspace at `cli/`), `cargo test`, mlua/Lua 5.4 for runtime, MDX (Astro Starlight) for the Standard, pnpm + vitest for spec linting, the in-repo conformance harness (`cargo test -p cook-lang --test conformance`).

---

## Working directory and prerequisites

All paths are relative to `/home/alex/dev/cook` unless noted.

Confirm the spec-first hook is installed (it should already be):

```bash
git -C /home/alex/dev/cook config --get core.hooksPath
# Expected: .githooks
```

If empty, run: `git -C /home/alex/dev/cook config core.hooksPath .githooks`

This plan touches both `standard/` and `cli/`. The hook will require any Cookfile-language change in `cli/` to ride with a `standard/` update — the spec already exists in `standard/specs/`, so the hook check passes once Task 21 (changelog entry) lands.

## Per-task verification commands

Each implementation task ends with one or more of these:

| Scope | Command | Expected |
|---|---|---|
| Lang unit tests | `cd cli && cargo test -p cook-lang` | clean |
| Luagen unit tests | `cd cli && cargo test -p cook-luagen` | clean |
| Register unit tests | `cd cli && cargo test -p cook-register` | clean |
| Conformance harness | `cd cli && cargo test -p cook-lang --test conformance` | clean |
| Whole CLI test suite | `cd cli && cargo test` | clean |
| Spec build | `cd standard && pnpm build` | exit 0, no `error` lines |
| Spec lints | `cd standard && pnpm lint:keywords && pnpm test` | exit 0 |
| Conformance against tag | `cd standard && cook against-tag --set VERSION=0.6` | clean (until Task 22) |

**Important:** The conformance harness runs against `standard/conformance/`. It will FAIL after Task 1 (parser change) until Task 12 (corpus migration) lands. To keep CI green between, mark the harness `#[ignore]` in Task 1 and re-enable in Task 12 (instructions inside those tasks). The repo's pre-commit hook does not run the harness; the failure is local-only.

---

## File structure

| File | Responsibility | Tasks |
|---|---|---|
| `cli/crates/cook-lang/src/lexer.rs` | Reserve `env` segment; reject `env.X` first-segment recipes | 1 |
| `cli/crates/cook-luagen/src/sigil.rs` (new) | Strict `$<IDENT>` scanner: token detection, identifier validation, span enumeration | 2 |
| `cli/crates/cook-luagen/src/resolver.rs` (new) | Closed-set placeholder resolver: builtin → recipe → env-runtime emission | 3 |
| `cli/crates/cook-luagen/src/template.rs` | Replace `expand_with_deps_fallback` with the new substitution path; delegate to sigil + resolver modules | 4 |
| `cli/crates/cook-luagen/src/dep_ref.rs` | Replace `extract_brace_tokens` with `extract_sigil_tokens`; update body-token extraction to sigil shape | 4 |
| `cli/crates/cook-luagen/src/recipe.rs` | Wire substitution into all four shell-text emission sites (output pattern, using-block body, plate/test body, bare shell command in recipe + chore bodies) | 5, 6, 7, 8 |
| `cli/crates/cook-register/src/env_api.rs` (new) | `cook.require_env(name)` Lua-exported helper; freezes `cook.env` keyset after config-block evaluation | 9 |
| `cli/crates/cook-register/src/engine.rs` | Call `freeze_env_keyset()` after config-block phase completes; expose `require_env` | 9 |
| `cli/crates/cook-cli/src/cli.rs` | Add `--migrate-sigil` flag and `--migrate-sigil-check` (dry-run) | 11 |
| `cli/crates/cook-cli/src/migrate_sigil.rs` (new) | Parser-aware `{TOKEN}` → `$<TOKEN>` rewriter | 11 |
| `standard/Cookfile` | Add `migrate` and `migrate-check` chores wrapping the CLI flags | 11 |
| `standard/conformance/positive/**/Cookfile` + `parse.txt` | Mass migration via the tool | 12 |
| `standard/conformance/negative/**/Cookfile` + `parse.txt` | Mass migration via the tool | 12 |
| `examples/**/Cookfile` | Mass migration via the tool | 13 |
| `Cookfile`, `cli/Cookfile`, `standard/Cookfile`, `tree-sitter-cook/Cookfile` | Mass migration via the tool | 14 |
| `standard/conformance/positive/NNN-shell-idioms-*` (new) | Positive fixtures pinning legitimate shell `{}` idioms passing through unchanged | 15 |
| `standard/conformance/negative/NNN-undeclared-env`, `NNN-reserved-env-recipe`, etc. (new) | Negative fixtures pinning hard-error diagnostics | 16 |
| `standard/src/content/docs/02-lexical.mdx` | New §2.X "Placeholders in shell text" subsection | 17 |
| `standard/src/content/docs/05-cross-recipe-references.mdx` | §xref.resolution rewrite (drop step-4 fallthrough; add closed-set rule) | 17 |
| `standard/src/content/docs/06-cook-lua-api.mdx` | §6.7 + §6.7.1 rewrite to `$<...>` shape | 17 |
| `standard/src/content/docs/appendix/A-grammar.mdx` | New `placeholder` production | 18 |
| `standard/src/content/docs/appendix/B-rationale.mdx` | New rationale section explaining sigil choice + closed-set resolution | 19 |
| `standard/src/content/docs/appendix/D-changes.mdx` | CS-0033 entry + v0.7 cut entry | 19 |
| `standard/src/content/docs/appendix/E-pre-v1-checklist.mdx` | E.2 → fully resolved; E.8 → resolved | 19 |
| `tree-sitter-cook/grammar.js` | Bump version banner to `cs-standard/v0.7` (header comment only — grammar update deferred to CS-0002 follow-up) | 20 |
| `standard/VERSION` | `0.7` | 22 |

No file is fully deleted in this plan; legacy code paths (`extract_brace_tokens`, `expand_with_deps_fallback`) are removed in Task 4 from existing files.

---

## Task 1: Reserve `env` and reject `env.X` first-segment recipe names

**Files:**
- Modify: `cli/crates/cook-lang/src/lexer.rs` (around line 37 and around `check_reserved_recipe_name`)
- Test: `cli/crates/cook-lang/src/tests.rs`

**Why first:** The reserved-namespace rule from §3.2.1 of the design must be in place before any later task can rely on `env.X` being unambiguous. This task is parser-only, ships independently, and adds two negative tests.

- [ ] **Step 1.1: Write a failing test for the last-segment rejection (already covered) and a NEW failing test for first-segment `env.X` rejection**

In `cli/crates/cook-lang/src/tests.rs`, add at the bottom:

```rust
#[test]
fn rejects_recipe_with_env_first_segment() {
    let source = r#"recipe "env.foo"
end"#;
    let result = parse(source);
    assert!(result.is_err(), "expected parse error for env.foo recipe");
    let err = result.unwrap_err().to_string();
    assert!(
        err.contains("env") && err.contains("reserved"),
        "diagnostic must name 'env' and 'reserved'; got: {}",
        err
    );
}

#[test]
fn rejects_recipe_with_env_last_segment() {
    let source = r#"recipe "foo.env"
end"#;
    let result = parse(source);
    assert!(result.is_err(), "expected parse error for foo.env recipe");
}
```

- [ ] **Step 1.2: Run the tests — verify they fail**

```bash
cd cli && cargo test -p cook-lang rejects_recipe_with_env -- --nocapture
```

Expected: both tests FAIL. The first-segment test fails because `env` is not yet enforced as a first segment. The last-segment test may already pass (depending on existing code); if it does, leave it as a regression pin.

- [ ] **Step 1.3: Add `env` to `RESERVED_RECIPE_SEGMENTS`**

In `cli/crates/cook-lang/src/lexer.rs`, around line 37:

```rust
const RESERVED_RECIPE_SEGMENTS: &[&str] = &["stem", "name", "ext", "dir", "in", "out", "all", "env"];
```

- [ ] **Step 1.4: Tighten `check_reserved_recipe_name` to also reject `env` as the FIRST segment**

In `cli/crates/cook-lang/src/lexer.rs`, locate the `check_reserved_recipe_name` function. Add a separate first-segment check before the existing last-segment check:

```rust
fn check_reserved_recipe_name(name: &str, line: usize) -> Result<(), LexError> {
    let first_segment = name.split('.').next().unwrap_or(name);
    if first_segment == "env" {
        return Err(LexError::ReservedRecipeName {
            line,
            segment: "env".to_string(),
        });
    }
    let last_segment = name.rsplit('.').next().unwrap_or(name);
    if RESERVED_RECIPE_SEGMENTS.contains(&last_segment) {
        return Err(LexError::ReservedRecipeName {
            line,
            segment: last_segment.to_string(),
        });
    }
    Ok(())
}
```

(The variable rename `segment → first_segment / last_segment` improves diagnostics in later debugging; the LexError enum is unchanged.)

- [ ] **Step 1.5: Run the tests — verify they pass**

```bash
cd cli && cargo test -p cook-lang rejects_recipe_with_env -- --nocapture
```

Expected: both tests PASS.

- [ ] **Step 1.6: Run the full lang test suite**

```bash
cd cli && cargo test -p cook-lang
```

Expected: all tests pass. If any existing test creates a recipe named `env.something`, update it (none should — verify with `grep -rn 'recipe.*env\.' cli/crates/cook-lang/src/`).

- [ ] **Step 1.7: Mark the conformance harness `#[ignore]` for the duration of the parser-rewrite phase**

In `cli/crates/cook-lang/tests/conformance.rs`, add a `#[ignore = "re-enabled in Task 12 after corpus migration"]` attribute above each `#[test]` function (or the most-encompassing one). The exact location varies; identify each test by reading the file first.

- [ ] **Step 1.8: Run conformance — verify it's now ignored, not failing**

```bash
cd cli && cargo test -p cook-lang --test conformance
```

Expected: `running N tests` shows N ignored, 0 failed.

- [ ] **Step 1.9: Commit**

```bash
cd /home/alex/dev/cook
git add cli/crates/cook-lang/src/lexer.rs cli/crates/cook-lang/src/tests.rs cli/crates/cook-lang/tests/conformance.rs
git commit -m "$(cat <<'EOF'
parser: reserve 'env' as recipe-name segment (CS-0033 prep)

Adds 'env' to RESERVED_RECIPE_SEGMENTS and tightens check_reserved_recipe_name
to reject 'env' as either the first or last segment of a dotted recipe name.
This is the parser-side precondition for §3.2.1 of the sigil-placeholder
design: a placeholder TOKEN beginning with 'env.' is always an env-var
lookup, never a recipe accessor, so a recipe whose first segment is 'env'
would create resolution ambiguity.

Conformance harness marked ignore for the duration of the parser rewrite;
re-enabled in the corpus-migration task.
EOF
)"
```

---

## Task 2: Strict `$<IDENT>` scanner module

**Files:**
- Create: `cli/crates/cook-luagen/src/sigil.rs`
- Modify: `cli/crates/cook-luagen/src/lib.rs` (add `mod sigil;`)
- Test: tests live inside `sigil.rs` (`#[cfg(test)] mod tests`)

**Goal:** A self-contained scanner that, given a shell-text string, returns the spans and inner identifiers of every well-formed `$<IDENT>` placeholder. Anything malformed (bad identifier, no closing `>`, empty, etc.) is left literal — the scanner does not search forward for a `>` past a malformed `IDENT`. No resolution yet; this is shape detection only.

- [ ] **Step 2.1: Create `cli/crates/cook-luagen/src/sigil.rs` with the scanner skeleton and failing tests**

Write the file with:

```rust
//! Strict `$<IDENT>` placeholder scanner per CS-0033 §3.1.
//!
//! A Cook placeholder in shell text matches exactly:
//!   $<IDENT>
//! where IDENT is one of:
//!   bare_ident       := ALPHA (ALPHA | DIGIT | "_" | ".")*
//!   out_indexed      := "out_" DIGIT+
//!   out_indexed_acc  := "out_" DIGIT+ "." accessor
//!   ACC              := "stem" | "name" | "ext" | "dir"
//!   ALPHA            := "a"…"z" | "A"…"Z" | "_"
//!
//! Anything not matching the strict shape is literal shell text. The scanner
//! does not search forward for a `>` past a malformed inner — a `$<foo bar>`
//! is literal, not an unclosed-placeholder error.

use std::ops::Range;

/// One placeholder occurrence in a shell text string.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlaceholderSpan {
    /// Byte range of the entire placeholder, including `$<` and `>`.
    pub range: Range<usize>,
    /// The IDENT content between `$<` and `>`.
    pub ident: String,
}

/// Scan `text` for all well-formed `$<IDENT>` placeholders.
/// Returns spans in source order. Malformed `$<...` sequences are skipped
/// (treated as literal shell text).
pub fn scan(text: &str) -> Vec<PlaceholderSpan> {
    let mut out = Vec::new();
    let bytes = text.as_bytes();
    let mut i = 0;
    while i + 1 < bytes.len() {
        if bytes[i] == b'$' && bytes[i + 1] == b'<' {
            if let Some(span) = try_match_placeholder(text, i) {
                let end = span.range.end;
                out.push(span);
                i = end;
                continue;
            }
        }
        i += 1;
    }
    out
}

/// If `text[start..]` begins with a well-formed `$<IDENT>`, return the span.
/// Otherwise None.
fn try_match_placeholder(text: &str, start: usize) -> Option<PlaceholderSpan> {
    let bytes = text.as_bytes();
    debug_assert_eq!(bytes[start], b'$');
    debug_assert_eq!(bytes[start + 1], b'<');
    let ident_start = start + 2;
    let mut i = ident_start;

    // First IDENT character must be ALPHA (a-z, A-Z, _).
    if i >= bytes.len() || !is_alpha(bytes[i]) {
        return None;
    }
    i += 1;

    // Subsequent characters: ALPHA | DIGIT | _ | .
    while i < bytes.len() && is_ident_continue(bytes[i]) {
        i += 1;
    }

    // Must be followed immediately by `>`.
    if i >= bytes.len() || bytes[i] != b'>' {
        return None;
    }

    let ident = text[ident_start..i].to_string();
    Some(PlaceholderSpan {
        range: start..i + 1,
        ident,
    })
}

#[inline]
fn is_alpha(b: u8) -> bool {
    b.is_ascii_alphabetic() || b == b'_'
}

#[inline]
fn is_ident_continue(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b == b'.'
}

#[cfg(test)]
mod tests {
    use super::*;

    fn idents(text: &str) -> Vec<String> {
        scan(text).into_iter().map(|s| s.ident).collect()
    }

    #[test]
    fn matches_simple_ident() {
        assert_eq!(idents("echo $<HOME>"), vec!["HOME"]);
    }

    #[test]
    fn matches_dotted_ident() {
        assert_eq!(idents("$<in.stem>.o"), vec!["in.stem"]);
    }

    #[test]
    fn matches_out_indexed() {
        assert_eq!(idents("cp src $<out_1> $<out_2>"), vec!["out_1", "out_2"]);
    }

    #[test]
    fn matches_out_indexed_accessor() {
        assert_eq!(idents("$<out_1.stem>"), vec!["out_1.stem"]);
    }

    #[test]
    fn matches_multiple_in_one_string() {
        assert_eq!(
            idents("gcc -c $<in> -o $<out>"),
            vec!["in", "out"]
        );
    }

    #[test]
    fn rejects_empty_ident() {
        assert!(scan("$<>").is_empty());
    }

    #[test]
    fn rejects_ident_starting_with_digit() {
        assert!(scan("$<1foo>").is_empty());
    }

    #[test]
    fn rejects_ident_with_space() {
        assert!(scan("$<foo bar>").is_empty());
    }

    #[test]
    fn rejects_ident_with_comma() {
        assert!(scan("$<a,b,c>").is_empty());
    }

    #[test]
    fn rejects_ident_with_colon() {
        assert!(scan("$<HOME:-default>").is_empty());
    }

    #[test]
    fn rejects_unclosed_placeholder() {
        assert!(scan("$<foo").is_empty());
        assert!(scan("$<foo bar baz").is_empty());
    }

    #[test]
    fn does_not_search_forward_for_close() {
        // A `>` appearing later in the string MUST NOT be treated as the close
        // of a malformed `$<...`. Verifies the strict-bail behavior.
        assert!(scan("$<foo bar> baz").is_empty());
    }

    #[test]
    fn literal_dollar_alone_is_not_placeholder() {
        assert!(scan("echo $HOME").is_empty());
        assert!(scan("echo $1").is_empty());
        assert!(scan("price: $5").is_empty());
    }

    #[test]
    fn literal_braces_are_not_placeholders() {
        // The strict rule: only $< triggers the scanner.
        assert!(scan("{a,b,c}").is_empty());
        assert!(scan("${HOME}").is_empty()); // `${` is not `$<`
        assert!(scan("awk '{print $1}'").is_empty());
    }

    #[test]
    fn span_includes_dollar_and_close() {
        let spans = scan("hi $<foo> there");
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].range, 3..9);
        assert_eq!(spans[0].ident, "foo");
    }

    #[test]
    fn dollar_lt_followed_by_dollar_lt() {
        // $<$<x>> — outer $< followed by literal $, then identifier-shaped
        // content fails (first char of IDENT is `$`, which is not ALPHA), so
        // outer is literal. The inner $<x> is a valid placeholder at offset 2.
        let spans = scan("$<$<x>>");
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].ident, "x");
        assert_eq!(spans[0].range, 2..6);
    }
}
```

- [ ] **Step 2.2: Add the module to `lib.rs`**

In `cli/crates/cook-luagen/src/lib.rs`, add (in alphabetical order with existing `mod` lines):

```rust
mod sigil;
```

If the `sigil` API needs to be public to the crate (it does — Task 3 will use it), make it `pub(crate) mod sigil;`. If other crates need it later, promote to `pub mod`.

- [ ] **Step 2.3: Run the new tests — verify they pass**

```bash
cd cli && cargo test -p cook-luagen sigil:: -- --nocapture
```

Expected: 14 tests PASS.

- [ ] **Step 2.4: Run the full luagen suite to make sure nothing regresses**

```bash
cd cli && cargo test -p cook-luagen
```

Expected: all tests pass (the sigil module is not yet wired into the substitution path, so existing behavior is unchanged).

- [ ] **Step 2.5: Commit**

```bash
cd /home/alex/dev/cook
git add cli/crates/cook-luagen/src/sigil.rs cli/crates/cook-luagen/src/lib.rs
git commit -m "luagen: add strict \$<IDENT> placeholder scanner (CS-0033 prep)

Self-contained scanner per §3.1 of the sigil-placeholder design. Detects
well-formed \$<IDENT> placeholders only; malformed sequences are literal
(scanner does not search forward for a closing > past a bad inner). Not
yet wired into substitution — Task 4 replaces the legacy {TOKEN} scanner."
```

---

## Task 3: Closed-set resolver

**Files:**
- Create: `cli/crates/cook-luagen/src/resolver.rs`
- Modify: `cli/crates/cook-luagen/src/lib.rs` (add `mod resolver;`)
- Test: inside `resolver.rs`

**Goal:** Given a `PlaceholderSpan.ident` and the current step's context (recipe-names-in-scope, iteration mode, output count), return one of:

- `Resolved::Builtin(BuiltinKind)` — emit the builtin's substitution form
- `Resolved::Recipe { name, accessor }` — emit `cook.dep_output(name)` or accessor variant
- `Resolved::EnvRuntime(name)` — emit `cook.require_env("name")` (the runtime check is from Task 9)
- `Resolved::Error(ResolveError)` — codegen-time hard error (e.g., builtin used in wrong mode)

The resolver does NOT perform the env-declared check at codegen — that's deferred to runtime via `cook.require_env`. Codegen-time errors are limited to: builtin used in wrong iteration mode, builtin used with wrong output count, malformed `out_N` (N=0 or non-numeric), and ambiguity warnings.

- [ ] **Step 3.1: Create `cli/crates/cook-luagen/src/resolver.rs` with the type skeleton and failing tests**

Write the file with:

```rust
//! Closed-set placeholder resolver per CS-0033 §3.2.
//!
//! Given an IDENT from the sigil scanner plus the current step's context,
//! decide whether the placeholder resolves to a builtin, an in-scope recipe,
//! or an env-var runtime lookup. Codegen-time errors are limited to builtin
//! mode/count violations; the env-declared check is deferred to runtime via
//! `cook.require_env` (see cook-register/env_api.rs from Task 9).

use std::collections::BTreeSet;

/// Iteration mode of the enclosing step (for builtin validity).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IterMode {
    OneToOne,
    ManyToOne,
    OneShot,
}

/// Output declaration shape of the enclosing step.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputShape {
    /// No declared outputs (plate, test, bare shell command).
    None,
    /// Exactly one declared output (single-output cook step).
    Single,
    /// N declared outputs (multi-output cook step).
    Multi(usize),
}

/// Context passed to the resolver for a single shell-text scan.
pub struct ResolveCtx<'a> {
    pub mode: IterMode,
    pub outputs: OutputShape,
    pub recipes_in_scope: &'a BTreeSet<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BuiltinKind {
    In,                    // {in}
    InAccessor(String),    // {in.stem} etc — accessor stored
    Out,                   // {out}
    OutAccessor(String),   // {out.stem} etc
    OutIndexed(usize),     // {out_1}
    OutIndexedAccessor(usize, String), // {out_1.stem}
    All,                   // {all}
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resolved {
    Builtin(BuiltinKind),
    Recipe { name: String, accessor: Option<String> },
    EnvRuntime(String),
    Error(ResolveError),
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ResolveError {
    #[error("placeholder $<{ident}>: '{builtin}' is not valid in {mode:?} mode")]
    BuiltinWrongMode { ident: String, builtin: String, mode: IterMode },
    #[error("placeholder $<{ident}>: '{builtin}' requires {required} declared output(s); step declares {actual}")]
    BuiltinWrongOutputCount { ident: String, builtin: String, required: String, actual: usize },
    #[error("placeholder $<{ident}>: malformed out_N (N must be ≥ 1)")]
    MalformedOutIndex { ident: String },
}

const ACCESSORS: &[&str] = &["stem", "name", "ext", "dir"];

pub fn resolve(ident: &str, ctx: &ResolveCtx<'_>) -> Resolved {
    // Try builtin first.
    if let Some(b) = match_builtin(ident) {
        return validate_builtin(ident, b, ctx);
    }
    // Try recipe (own-name or recipe.accessor).
    if ctx.recipes_in_scope.contains(ident) {
        return Resolved::Recipe { name: ident.to_string(), accessor: None };
    }
    if let Some(dot) = ident.rfind('.') {
        let prefix = &ident[..dot];
        let suffix = &ident[dot + 1..];
        if ACCESSORS.contains(&suffix) && ctx.recipes_in_scope.contains(prefix) {
            return Resolved::Recipe {
                name: prefix.to_string(),
                accessor: Some(suffix.to_string()),
            };
        }
    }
    // Otherwise env-runtime. Strip the explicit "env." prefix if present.
    let env_key = ident.strip_prefix("env.").unwrap_or(ident);
    Resolved::EnvRuntime(env_key.to_string())
}

fn match_builtin(ident: &str) -> Option<BuiltinKind> {
    match ident {
        "in" => Some(BuiltinKind::In),
        "out" => Some(BuiltinKind::Out),
        "all" => Some(BuiltinKind::All),
        _ => {
            if let Some(rest) = ident.strip_prefix("in.") {
                if ACCESSORS.contains(&rest) {
                    return Some(BuiltinKind::InAccessor(rest.to_string()));
                }
                None
            } else if let Some(rest) = ident.strip_prefix("out.") {
                if ACCESSORS.contains(&rest) {
                    return Some(BuiltinKind::OutAccessor(rest.to_string()));
                }
                None
            } else if let Some(rest) = ident.strip_prefix("out_") {
                let (num_str, acc) = match rest.find('.') {
                    Some(dot) => (&rest[..dot], Some(&rest[dot + 1..])),
                    None => (rest, None),
                };
                let n: usize = num_str.parse().ok()?;
                if n == 0 {
                    return None; // caller emits MalformedOutIndex
                }
                match acc {
                    None => Some(BuiltinKind::OutIndexed(n)),
                    Some(a) if ACCESSORS.contains(&a) => {
                        Some(BuiltinKind::OutIndexedAccessor(n, a.to_string()))
                    }
                    _ => None,
                }
            } else {
                None
            }
        }
    }
}

fn validate_builtin(ident: &str, b: BuiltinKind, ctx: &ResolveCtx<'_>) -> Resolved {
    use BuiltinKind::*;
    use IterMode::*;
    use OutputShape::*;

    match &b {
        In | InAccessor(_) => {
            if ctx.mode != OneToOne {
                return Resolved::Error(ResolveError::BuiltinWrongMode {
                    ident: ident.to_string(),
                    builtin: format!("{:?}", b),
                    mode: ctx.mode,
                });
            }
        }
        All => {
            if ctx.mode != ManyToOne {
                return Resolved::Error(ResolveError::BuiltinWrongMode {
                    ident: ident.to_string(),
                    builtin: format!("{:?}", b),
                    mode: ctx.mode,
                });
            }
        }
        Out | OutAccessor(_) => {
            if !matches!(ctx.outputs, Single) {
                let actual = match ctx.outputs {
                    None => 0,
                    Single => 1,
                    Multi(n) => n,
                };
                return Resolved::Error(ResolveError::BuiltinWrongOutputCount {
                    ident: ident.to_string(),
                    builtin: format!("{:?}", b),
                    required: "exactly 1".to_string(),
                    actual,
                });
            }
        }
        OutIndexed(n) | OutIndexedAccessor(n, _) => {
            if let Multi(declared) = ctx.outputs {
                if *n > declared {
                    return Resolved::Error(ResolveError::BuiltinWrongOutputCount {
                        ident: ident.to_string(),
                        builtin: format!("{:?}", b),
                        required: format!("≥ {}", n),
                        actual: declared,
                    });
                }
            } else {
                let actual = match ctx.outputs {
                    None => 0,
                    Single => 1,
                    Multi(n) => n,
                };
                return Resolved::Error(ResolveError::BuiltinWrongOutputCount {
                    ident: ident.to_string(),
                    builtin: format!("{:?}", b),
                    required: format!("multi-output ≥ {}", n),
                    actual,
                });
            }
        }
    }
    Resolved::Builtin(b)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx_oneone_single<'a>(recipes: &'a BTreeSet<String>) -> ResolveCtx<'a> {
        ResolveCtx { mode: IterMode::OneToOne, outputs: OutputShape::Single, recipes_in_scope: recipes }
    }
    fn ctx_oneshot_none<'a>(recipes: &'a BTreeSet<String>) -> ResolveCtx<'a> {
        ResolveCtx { mode: IterMode::OneShot, outputs: OutputShape::None, recipes_in_scope: recipes }
    }
    fn empty() -> BTreeSet<String> { BTreeSet::new() }

    #[test]
    fn resolves_in_to_builtin() {
        let r = empty();
        let ctx = ctx_oneone_single(&r);
        assert_eq!(resolve("in", &ctx), Resolved::Builtin(BuiltinKind::In));
    }

    #[test]
    fn resolves_in_stem_to_builtin() {
        let r = empty();
        let ctx = ctx_oneone_single(&r);
        assert_eq!(resolve("in.stem", &ctx), Resolved::Builtin(BuiltinKind::InAccessor("stem".to_string())));
    }

    #[test]
    fn resolves_recipe_in_scope() {
        let mut r = BTreeSet::new();
        r.insert("build".to_string());
        let ctx = ctx_oneshot_none(&r);
        assert_eq!(
            resolve("build", &ctx),
            Resolved::Recipe { name: "build".to_string(), accessor: None }
        );
    }

    #[test]
    fn resolves_recipe_accessor() {
        let mut r = BTreeSet::new();
        r.insert("lib".to_string());
        let ctx = ctx_oneshot_none(&r);
        assert_eq!(
            resolve("lib.stem", &ctx),
            Resolved::Recipe { name: "lib".to_string(), accessor: Some("stem".to_string()) }
        );
    }

    #[test]
    fn unknown_token_falls_through_to_env_runtime() {
        let r = empty();
        let ctx = ctx_oneshot_none(&r);
        assert_eq!(resolve("HOME", &ctx), Resolved::EnvRuntime("HOME".to_string()));
    }

    #[test]
    fn explicit_env_prefix_strips_to_env_runtime() {
        let r = empty();
        let ctx = ctx_oneshot_none(&r);
        assert_eq!(resolve("env.HOME", &ctx), Resolved::EnvRuntime("HOME".to_string()));
    }

    #[test]
    fn explicit_env_overrides_recipe_match() {
        let mut r = BTreeSet::new();
        r.insert("HOME".to_string());
        let ctx = ctx_oneshot_none(&r);
        // Bare HOME → recipe (recipe wins over env).
        assert!(matches!(resolve("HOME", &ctx), Resolved::Recipe { .. }));
        // env.HOME → always env, even if HOME is a recipe.
        assert_eq!(resolve("env.HOME", &ctx), Resolved::EnvRuntime("HOME".to_string()));
    }

    #[test]
    fn in_in_many_to_one_is_error() {
        let r = empty();
        let ctx = ResolveCtx {
            mode: IterMode::ManyToOne,
            outputs: OutputShape::Single,
            recipes_in_scope: &r,
        };
        assert!(matches!(resolve("in", &ctx), Resolved::Error(ResolveError::BuiltinWrongMode { .. })));
    }

    #[test]
    fn out_in_multi_output_is_error() {
        let r = empty();
        let ctx = ResolveCtx {
            mode: IterMode::ManyToOne,
            outputs: OutputShape::Multi(2),
            recipes_in_scope: &r,
        };
        assert!(matches!(resolve("out", &ctx), Resolved::Error(ResolveError::BuiltinWrongOutputCount { .. })));
    }

    #[test]
    fn out_n_overflow_is_error() {
        let r = empty();
        let ctx = ResolveCtx {
            mode: IterMode::ManyToOne,
            outputs: OutputShape::Multi(2),
            recipes_in_scope: &r,
        };
        assert!(matches!(resolve("out_3", &ctx), Resolved::Error(ResolveError::BuiltinWrongOutputCount { .. })));
    }
}
```

- [ ] **Step 3.2: Add the module to `lib.rs`**

In `cli/crates/cook-luagen/src/lib.rs`, add:

```rust
pub(crate) mod resolver;
```

- [ ] **Step 3.3: Run the resolver tests — verify all pass**

```bash
cd cli && cargo test -p cook-luagen resolver:: -- --nocapture
```

Expected: 10 tests PASS.

- [ ] **Step 3.4: Run the full luagen suite**

```bash
cd cli && cargo test -p cook-luagen
```

Expected: all pass.

- [ ] **Step 3.5: Commit**

```bash
cd /home/alex/dev/cook
git add cli/crates/cook-luagen/src/resolver.rs cli/crates/cook-luagen/src/lib.rs
git commit -m "luagen: add closed-set placeholder resolver (CS-0033 prep)

Resolves IDENT → Builtin | Recipe | EnvRuntime per §3.2 of the
sigil-placeholder design. Codegen-time errors limited to builtin
mode/count violations; env-declared check is deferred to runtime
via cook.require_env (Task 9). Not yet wired into the substitution
path — Task 4 connects scanner + resolver into template.rs."
```

---

## Task 4: Replace `extract_brace_tokens` and `expand_with_deps_fallback` with sigil-based equivalents

**Files:**
- Modify: `cli/crates/cook-luagen/src/template.rs` (full rewrite of the `expand_with_deps_fallback` function and its callers' call sites)
- Modify: `cli/crates/cook-luagen/src/dep_ref.rs` (replace `extract_brace_tokens` with `extract_sigil_tokens`)
- Test: tests inside both files

**Goal:** Wire the scanner from Task 2 and the resolver from Task 3 into a single substitution function. Replace every consumer of the legacy `{TOKEN}` scanner. After this task, no code path in cook-luagen reads `{...}` as a placeholder.

- [ ] **Step 4.1: Read the current `template.rs` to understand the call surface**

```bash
sed -n '60,200p' cli/crates/cook-luagen/src/template.rs
```

Identify every public function that callers depend on. Common ones: `expand_with_deps_fallback` (used by the bare-shell-substitution path), `expand_body_token` (helper), and accessor-expansion helpers used by output-pattern emission. Note their signatures.

- [ ] **Step 4.2: Add `extract_sigil_tokens` to `dep_ref.rs` and a failing test**

In `cli/crates/cook-luagen/src/dep_ref.rs`, add at the bottom (do not yet remove `extract_brace_tokens`):

```rust
/// Extract all $<IDENT> tokens from a template string. Returns the IDENT
/// content of every well-formed sigil placeholder in source order.
pub fn extract_sigil_tokens(template: &str) -> Vec<String> {
    crate::sigil::scan(template)
        .into_iter()
        .map(|s| s.ident)
        .collect()
}

#[cfg(test)]
#[test]
fn test_extract_sigil_tokens_basic() {
    let toks = extract_sigil_tokens("gcc -c $<in> -o $<out>");
    assert_eq!(toks, vec!["in", "out"]);
}

#[cfg(test)]
#[test]
fn test_extract_sigil_tokens_skips_braces() {
    let toks = extract_sigil_tokens("for i in {1..3}; do echo $i; done");
    assert!(toks.is_empty());
}
```

- [ ] **Step 4.3: Run the new tests — verify they pass**

```bash
cd cli && cargo test -p cook-luagen extract_sigil_tokens -- --nocapture
```

Expected: 2 tests PASS.

- [ ] **Step 4.4: Add a new top-level `expand_sigil_template` function to `template.rs`**

Add (do not yet remove `expand_with_deps_fallback`):

```rust
use crate::resolver::{resolve, BuiltinKind, IterMode, OutputShape, ResolveCtx, Resolved, ResolveError};
use crate::sigil;

/// Expand a shell-text string by substituting every `$<IDENT>` placeholder.
///
/// Returns either a Lua expression (string-concatenated) suitable for
/// inlining into a `cook.add_unit` `command = ...` field, or a structured
/// resolution error.
///
/// Builtins are inlined directly; recipes lower to `cook.dep_output(...)`;
/// env vars lower to `cook.require_env("...")`. Builtin substitution forms
/// (the strings `_cook_in`, `_cook_out`, etc.) match the existing iteration
/// driver naming in `cli/crates/cook-luagen/src/recipe.rs`.
pub(crate) fn expand_sigil_template(
    template: &str,
    ctx: &ResolveCtx<'_>,
    consulted_env: &mut ConsultedEnv,
) -> Result<String, ResolveError> {
    let spans = sigil::scan(template);
    if spans.is_empty() {
        return Ok(format!("\"{}\"", escape_lua_string(template)));
    }
    let mut parts: Vec<String> = Vec::new();
    let mut cursor = 0;
    for span in &spans {
        if span.range.start > cursor {
            let lit = &template[cursor..span.range.start];
            parts.push(format!("\"{}\"", escape_lua_string(lit)));
        }
        match resolve(&span.ident, ctx) {
            Resolved::Builtin(b) => parts.push(builtin_to_lua(&b)),
            Resolved::Recipe { name, accessor } => {
                let call = match &accessor {
                    None => format!("cook.dep_output(\"{}\")", escape_lua_string(&name)),
                    Some(acc) => format!(
                        "path.{}(cook.dep_output(\"{}\"))",
                        acc,
                        escape_lua_string(&name)
                    ),
                };
                parts.push(call);
            }
            Resolved::EnvRuntime(key) => {
                consulted_env.insert(key.clone());
                parts.push(format!("cook.require_env(\"{}\")", escape_lua_string(&key)));
            }
            Resolved::Error(e) => return Err(e),
        }
        cursor = span.range.end;
    }
    if cursor < template.len() {
        let lit = &template[cursor..];
        parts.push(format!("\"{}\"", escape_lua_string(lit)));
    }
    Ok(parts.join(" .. "))
}

fn builtin_to_lua(b: &BuiltinKind) -> String {
    match b {
        BuiltinKind::In => "_cook_in".to_string(),
        BuiltinKind::InAccessor(acc) => format!("path.{}(_cook_in)", acc),
        BuiltinKind::Out => "_cook_out".to_string(),
        BuiltinKind::OutAccessor(acc) => format!("path.{}(_cook_out)", acc),
        BuiltinKind::OutIndexed(n) => format!("_cook_out_{}", n),
        BuiltinKind::OutIndexedAccessor(n, acc) => {
            format!("path.{}(_cook_out_{})", acc, n)
        }
        BuiltinKind::All => "_cook_all".to_string(),
    }
}
```

If `ConsultedEnv` is not pub-visible to `template`, add the path or `pub(crate) use` it where it's defined.

- [ ] **Step 4.5: Add unit tests for `expand_sigil_template`**

At the bottom of `template.rs` (inside the existing `#[cfg(test)] mod tests` block, or in a new one):

```rust
#[cfg(test)]
mod sigil_template_tests {
    use super::*;
    use crate::resolver::{IterMode, OutputShape, ResolveCtx};
    use std::collections::BTreeSet;

    fn ctx_oneone_single<'a>(recipes: &'a BTreeSet<String>) -> ResolveCtx<'a> {
        ResolveCtx { mode: IterMode::OneToOne, outputs: OutputShape::Single, recipes_in_scope: recipes }
    }
    fn ctx_oneshot_none<'a>(recipes: &'a BTreeSet<String>) -> ResolveCtx<'a> {
        ResolveCtx { mode: IterMode::OneShot, outputs: OutputShape::None, recipes_in_scope: recipes }
    }
    fn empty() -> BTreeSet<String> { BTreeSet::new() }

    #[test]
    fn no_placeholders_passes_through_quoted() {
        let r = empty();
        let mut ce = ConsultedEnv::default();
        let out = expand_sigil_template("hello world", &ctx_oneshot_none(&r), &mut ce).unwrap();
        assert_eq!(out, r#""hello world""#);
    }

    #[test]
    fn shell_braces_pass_through_literal() {
        let r = empty();
        let mut ce = ConsultedEnv::default();
        let out = expand_sigil_template("for i in {1..3}; do echo $i; done", &ctx_oneshot_none(&r), &mut ce).unwrap();
        assert_eq!(out, r#""for i in {1..3}; do echo $i; done""#);
        assert!(ce.is_empty());
    }

    #[test]
    fn dollar_braces_pass_through_literal() {
        let r = empty();
        let mut ce = ConsultedEnv::default();
        let out = expand_sigil_template("echo ${HOME:-fallback}", &ctx_oneshot_none(&r), &mut ce).unwrap();
        assert_eq!(out, r#""echo ${HOME:-fallback}""#);
        assert!(ce.is_empty());
    }

    #[test]
    fn awk_inline_brace_passes_through() {
        let r = empty();
        let mut ce = ConsultedEnv::default();
        let out = expand_sigil_template("awk '{print $1}' file", &ctx_oneshot_none(&r), &mut ce).unwrap();
        assert_eq!(out, r#""awk '{print $1}' file""#);
    }

    #[test]
    fn builtin_in_lowers_to_cook_in() {
        let r = empty();
        let mut ce = ConsultedEnv::default();
        let out = expand_sigil_template("gcc -c $<in> -o $<out>", &ctx_oneone_single(&r), &mut ce).unwrap();
        assert_eq!(out, r#""gcc -c " .. _cook_in .. " -o " .. _cook_out"#);
    }

    #[test]
    fn recipe_lowers_to_dep_output() {
        let mut r = BTreeSet::new();
        r.insert("lib".to_string());
        let mut ce = ConsultedEnv::default();
        let out = expand_sigil_template("cat $<lib> > merged", &ctx_oneshot_none(&r), &mut ce).unwrap();
        assert_eq!(out, r#""cat " .. cook.dep_output("lib") .. " > merged""#);
    }

    #[test]
    fn env_lowers_to_require_env_and_records_consultation() {
        let r = empty();
        let mut ce = ConsultedEnv::default();
        let out = expand_sigil_template("echo $<HOME>", &ctx_oneshot_none(&r), &mut ce).unwrap();
        assert_eq!(out, r#""echo " .. cook.require_env("HOME")"#);
        assert!(ce.contains("HOME"));
    }

    #[test]
    fn explicit_env_prefix_strips() {
        let r = empty();
        let mut ce = ConsultedEnv::default();
        let out = expand_sigil_template("echo $<env.HOME>", &ctx_oneshot_none(&r), &mut ce).unwrap();
        assert_eq!(out, r#""echo " .. cook.require_env("HOME")"#);
        assert!(ce.contains("HOME"));
    }

    #[test]
    fn builtin_mode_violation_returns_error() {
        let r = empty();
        let mut ce = ConsultedEnv::default();
        let ctx = ResolveCtx {
            mode: IterMode::ManyToOne,
            outputs: OutputShape::Single,
            recipes_in_scope: &r,
        };
        let res = expand_sigil_template("$<in>", &ctx, &mut ce);
        assert!(res.is_err());
    }
}
```

If `ConsultedEnv::default()` and `contains` / `is_empty` aren't on the existing type, add them (the type is in `template.rs` already; the methods are likely there or need a one-line addition).

- [ ] **Step 4.6: Run the new tests — verify they pass**

```bash
cd cli && cargo test -p cook-luagen sigil_template_tests:: -- --nocapture
```

Expected: 9 tests PASS.

- [ ] **Step 4.7: Replace every internal caller of the legacy scanner**

```bash
grep -rn "extract_brace_tokens\|expand_with_deps_fallback\|expand_body_token" cli/crates/cook-luagen/src/
```

For each non-test call site, switch to the sigil-equivalent. Common patterns:

- `extract_brace_tokens(s)` → `extract_sigil_tokens(s)`
- `expand_with_deps_fallback(template, recipes, &mut ce)` → `expand_sigil_template(template, &ctx, &mut ce)?`

Build the `ResolveCtx` from the current step's iteration mode and output shape (the recipe.rs codegen already knows these). Propagate the `ResolveError` through the existing `RecipeCompileError` enum (add a new variant `ResolveError(ResolveError)` if not already convertible).

After replacement, **delete** `extract_brace_tokens`, `expand_with_deps_fallback`, `expand_body_token`, and any private helper that has no remaining callers.

- [ ] **Step 4.8: Run the full luagen suite**

```bash
cd cli && cargo test -p cook-luagen
```

Expected: all pass. Some existing tests likely encode `{NAME}` strings; those tests are migrated in Task 12 along with the conformance corpus. For now, if a unit test in `cli/crates/cook-luagen/src/tests.rs` fails because it asserts on the old substitution shape, update its input string from `{X}` to `$<X>` and its expected output (the structure should match — only the source syntax changes).

- [ ] **Step 4.9: Commit**

```bash
cd /home/alex/dev/cook
git add cli/crates/cook-luagen/src/
git commit -m "$(cat <<'EOF'
luagen: replace {TOKEN} scanner with sigil-based substitution (CS-0033)

The legacy extract_brace_tokens / expand_with_deps_fallback scanner
walked every `{` in shell text and consumed any `}`-terminated content
as a placeholder candidate, breaking legitimate shell brace use
(${VAR:-default}, {1..3}, awk '{print $1}', JSON literals). It also
produced false consulted_env_keys entries that poisoned cache state.

Replaced with the strict $<IDENT> scanner (Task 2) feeding into the
closed-set resolver (Task 3): builtins inline, recipes via
cook.dep_output, env vars via cook.require_env (runtime check from
Task 9). Bytes outside well-formed $<IDENT> spans pass through
verbatim, so shell idioms work unchanged.

The conformance corpus, examples, and in-tree Cookfiles still use the
legacy {TOKEN} syntax; they are migrated in Tasks 12-14 by the
parser-aware migration tool from Task 11.
EOF
)"
```

---

## Task 5: Wire substitution into cook-step output patterns

**Files:**
- Modify: `cli/crates/cook-luagen/src/recipe.rs` (the function that lowers `cook "PATTERN"` output declarations)

**Goal:** Output patterns like `"build/$<in.stem>.o"` resolve through the new substitution path. Today the existing code in `template.rs` has a separate `expand_template_for_output_pattern` (or similarly-named) function — confirm by grep, and route it through `expand_sigil_template` with the right `ResolveCtx` (`OutputShape::None` is wrong for output patterns; output patterns specifically allow `{in}`/`{in.ACC}` for one-to-one mode and resolve `{out}` only after the pattern emits its iteration count, so the right context is the same as the body).

- [ ] **Step 5.1: Find the output-pattern emission path**

```bash
grep -n "output_pattern\|cook_step\|output.*template\|expand_template" cli/crates/cook-luagen/src/recipe.rs | head -20
```

Identify the function that takes a single output-pattern string and emits its substituted Lua form.

- [ ] **Step 5.2: Write a failing test**

In `cli/crates/cook-luagen/src/tests.rs`, add:

```rust
#[test]
fn output_pattern_uses_sigil_substitution() {
    let cookfile = r#"recipe "build"
    ingredients "main.c"
    cook "build/$<in.stem>.o" using { gcc -c $<in> -o $<out> }
end"#;
    let lua = compile(cookfile).expect("compile");
    assert!(
        lua.contains("path.stem(_cook_in)") || lua.contains("path.stem(input)"),
        "expected output pattern to lower $<in.stem>; got:\n{}",
        lua
    );
}
```

(Function name `compile` is a stand-in — use the existing top-level `compile_cookfile` or whatever the crate exposes for end-to-end testing. Find via `grep -n "pub fn compile" cli/crates/cook-luagen/src/lib.rs`.)

- [ ] **Step 5.3: Run the test — verify it fails**

```bash
cd cli && cargo test -p cook-luagen output_pattern_uses_sigil -- --nocapture
```

Expected: FAIL (output pattern still uses the legacy scanner).

- [ ] **Step 5.4: Replace the output-pattern expansion call site**

In the function identified in Step 5.1, replace the legacy expansion call with `expand_sigil_template`. Build the `ResolveCtx` with the iteration mode the cook step is being driven in (one-to-one for `{in}`-bearing patterns, many-to-one for `{all}`-bearing or otherwise).

- [ ] **Step 5.5: Run the test — verify it passes**

```bash
cd cli && cargo test -p cook-luagen output_pattern_uses_sigil -- --nocapture
```

Expected: PASS.

- [ ] **Step 5.6: Run the full luagen suite**

```bash
cd cli && cargo test -p cook-luagen
```

Expected: all pass.

- [ ] **Step 5.7: Commit**

```bash
cd /home/alex/dev/cook
git add cli/crates/cook-luagen/src/recipe.rs cli/crates/cook-luagen/src/tests.rs
git commit -m "luagen: route cook-step output patterns through sigil substitution (CS-0033)"
```

---

## Task 6: Wire substitution into `using { ... }` shell-block bodies

**Files:**
- Modify: `cli/crates/cook-luagen/src/recipe.rs` (the function that lowers `using { ... }` blocks)

**Goal:** Shell-block bodies under `cook ... using { ... }` resolve through the new substitution path. After this task, a `using { for i in {1..3}; do … done }` block emits the brace expansion verbatim; only `$<...>` is substituted.

- [ ] **Step 6.1: Write a failing test**

In `cli/crates/cook-luagen/src/tests.rs`:

```rust
#[test]
fn using_block_passes_shell_braces_literally() {
    let cookfile = r#"recipe "build"
    ingredients "src.txt"
    cook "out.txt" using {
        for i in {1..3}; do echo "iter $i"; done > $<out>
        echo ${HOME:-fallback}
        awk '{print $1}' $<in>
    }
end"#;
    let lua = compile(cookfile).expect("compile");
    assert!(lua.contains("for i in {1..3}"), "literal {{1..3}} must survive; got:\n{}", lua);
    assert!(lua.contains("${HOME:-fallback}"), "literal ${{HOME:-fallback}} must survive; got:\n{}", lua);
    assert!(lua.contains("awk '{print $1}'"), "literal awk script must survive; got:\n{}", lua);
}
```

- [ ] **Step 6.2: Run — verify it fails**

```bash
cd cli && cargo test -p cook-luagen using_block_passes_shell_braces -- --nocapture
```

Expected: FAIL (literal braces are mangled by the legacy substitution, if any path still hits it).

- [ ] **Step 6.3: Confirm or replace the using-block emission call site**

After Task 4, the using-block path may already be on the new substitution. If the test fails because of a missed call site, find it via:

```bash
grep -n "using\|shell_block\|UsingBlock" cli/crates/cook-luagen/src/recipe.rs | head -20
```

Update any remaining call to use `expand_sigil_template`. Build context from the step's mode / output count.

- [ ] **Step 6.4: Run — verify pass**

```bash
cd cli && cargo test -p cook-luagen using_block_passes_shell_braces -- --nocapture
cd cli && cargo test -p cook-luagen
```

Both expected: PASS.

- [ ] **Step 6.5: Commit**

```bash
cd /home/alex/dev/cook
git add cli/crates/cook-luagen/src/recipe.rs cli/crates/cook-luagen/src/tests.rs
git commit -m "luagen: confirm using-block bodies use sigil substitution (CS-0033)

Adds end-to-end regression test pinning that {1..3}, \${HOME:-fallback},
and awk '{print \$1}' survive verbatim through using-block lowering.
EOF
"
```

---

## Task 7: Wire substitution into plate and test bodies

**Files:**
- Modify: `cli/crates/cook-luagen/src/recipe.rs` (plate / test step lowering)

- [ ] **Step 7.1: Write failing tests for plate + test**

```rust
#[test]
fn plate_body_substitutes_sigils_and_passes_braces() {
    let cookfile = r#"recipe "deploy"
    ingredients "*.bin"
    plate {
        cp $<in> dest/
        echo ${VAR:-x} > log
    }
end"#;
    let lua = compile(cookfile).expect("compile");
    assert!(lua.contains("cp \" .. _cook_in") || lua.contains("cp \" .. input"));
    assert!(lua.contains("${VAR:-x}"));
}

#[test]
fn test_body_substitutes_sigils_and_passes_braces() {
    let cookfile = r#"recipe "verify"
    ingredients "*.bin"
    test {
        diff $<in> expected/$<in.name>
        find . -exec rm {} \;
    }
end"#;
    let lua = compile(cookfile).expect("compile");
    assert!(lua.contains("path.name(_cook_in)") || lua.contains("path.name(input)"));
    assert!(lua.contains("-exec rm {} \\;"));
}
```

- [ ] **Step 7.2: Run — verify they fail (or skip if already passing from Task 4)**

```bash
cd cli && cargo test -p cook-luagen plate_body_substitutes test_body_substitutes
```

- [ ] **Step 7.3: Update the plate / test lowering call sites if needed**

```bash
grep -n "plate\|test_step\|PlateStep\|TestStep" cli/crates/cook-luagen/src/recipe.rs | head -20
```

Route any remaining calls through `expand_sigil_template` with `OutputShape::None` (plate/test produce no declared outputs — see §6.7.1 of the spec).

- [ ] **Step 7.4: Run — verify pass**

```bash
cd cli && cargo test -p cook-luagen
```

Expected: all pass.

- [ ] **Step 7.5: Commit**

```bash
cd /home/alex/dev/cook
git add cli/crates/cook-luagen/src/recipe.rs cli/crates/cook-luagen/src/tests.rs
git commit -m "luagen: route plate and test bodies through sigil substitution (CS-0033)"
```

---

## Task 8: Wire substitution into bare shell commands (recipe + chore bodies)

**Files:**
- Modify: `cli/crates/cook-luagen/src/recipe.rs` (`compile_recipe`'s shell-step path AND `compile_chore`'s shell-step path)

**Goal:** Close App. E.8. After this task, `chore devices` containing `$<ADB> devices` emits a Lua command that resolves `$<ADB>` via `cook.require_env("ADB")`, and the same applies to bare shell commands in recipe bodies.

- [ ] **Step 8.1: Write failing tests for recipe + chore bare shell**

```rust
#[test]
fn recipe_body_bare_shell_substitutes_sigil() {
    let cookfile = r#"config
    env.ADB = "adb"

recipe "devices"
    @$<ADB> devices
end"#;
    let lua = compile(cookfile).expect("compile");
    assert!(
        lua.contains("cook.require_env(\"ADB\")"),
        "expected cook.require_env wiring; got:\n{}",
        lua
    );
    assert!(!lua.contains("[[$<ADB>"), "raw $<ADB> must not survive into command field");
}

#[test]
fn chore_body_bare_shell_substitutes_sigil() {
    let cookfile = r#"config
    env.ADB = "adb"

chore devices
    $<ADB> devices
end"#;
    let lua = compile(cookfile).expect("compile");
    assert!(
        lua.contains("cook.require_env(\"ADB\")"),
        "expected cook.require_env wiring; got:\n{}",
        lua
    );
    assert!(!lua.contains("[[$<ADB>"), "raw $<ADB> must not survive into command field");
}

#[test]
fn chore_body_passes_literal_braces() {
    let cookfile = r#"chore demo
    for i in {1..3}; do echo "$i"; done
    awk '{print $1}' file.txt
end"#;
    let lua = compile(cookfile).expect("compile");
    assert!(lua.contains("for i in {1..3}"));
    assert!(lua.contains("awk '{print $1}'"));
}
```

- [ ] **Step 8.2: Run — verify they fail**

```bash
cd cli && cargo test -p cook-luagen recipe_body_bare_shell chore_body_bare_shell chore_body_passes_literal -- --nocapture
```

Expected: tests FAIL (today's chore + recipe-body codegen emits the raw `[[command]]`).

- [ ] **Step 8.3: Update `compile_chore` to apply substitution**

In `compile_chore` (around line 666 of `recipe.rs`), the `Step::Shell { command, line, interactive: true }` arm currently does:

```rust
let wrapped = wrap_lua_string(command);
out.push_str(&format!(
    "    cook.add_unit({{command = {}, interactive = true, line = {}, cache = false}})\n",
    wrapped, line
));
```

Replace with substitution-aware emission:

```rust
let mut consulted = ConsultedEnv::default();
let ctx = ResolveCtx {
    mode: IterMode::OneShot,
    outputs: OutputShape::None,
    recipes_in_scope: recipe_names_in_scope, // pulled from compile_chore's caller
};
let cmd_expr = expand_sigil_template(command, &ctx, &mut consulted)
    .map_err(RecipeCompileError::Resolve)?;
let env_keys = consulted_keys_lua_table(&consulted);
out.push_str(&format!(
    "    cook.add_unit({{command = {}, interactive = true, line = {}, cache = false, consulted_env_keys = {}}})\n",
    cmd_expr, line, env_keys
));
```

The `recipe_names_in_scope` parameter must be threaded into `compile_chore` from its caller in `lib.rs` or wherever the per-Cookfile compile entry computes the recipe-name set. The unreachable second arm (`Step::Shell { interactive: false, .. }`) is updated identically.

- [ ] **Step 8.4: Apply the same substitution to `compile_recipe`'s bare-shell path**

Find the equivalent `Step::Shell { … }` arm in `compile_recipe` (around the same file). Apply the same substitution wrapper. Use `cache = true` (the default for non-chore recipe shells) instead of `cache = false`.

- [ ] **Step 8.5: Run — verify pass**

```bash
cd cli && cargo test -p cook-luagen
```

Expected: all pass.

- [ ] **Step 8.6: End-to-end smoke test against the E.8 repro**

```bash
mkdir -p /tmp/sigil-smoke
cat > /tmp/sigil-smoke/Cookfile <<'EOF'
config
    env.ADB = "echo MOCK-ADB"

chore devices
    $<ADB> devices
EOF
cd /tmp/sigil-smoke && /home/alex/dev/cook/cli/target/debug/cook --emit-lua | head -10
```

Expected output contains `cook.require_env("ADB")` (not literal `$<ADB>`).

- [ ] **Step 8.7: Commit**

```bash
cd /home/alex/dev/cook
git add cli/crates/cook-luagen/src/recipe.rs cli/crates/cook-luagen/src/tests.rs
git commit -m "$(cat <<'EOF'
luagen: apply sigil substitution to bare shell in recipe + chore bodies (closes App. E.8)

Pre-CS-0033, bare shell commands in recipe and chore bodies bypassed the
placeholder-substitution pass entirely. The literal `$<ADB>` (or pre-cut
`{ADB}`) reached the shell unchanged and failed with `command not found`.
This was framed as a chore-only issue in App. E.8, but applied to recipe
shell steps too.

Both compile_chore and compile_recipe's Shell arms now route through
expand_sigil_template with the right ResolveCtx. Chores stay cache=false;
recipe shells stay cache=true. consulted_env_keys is now populated with
the real env names, not the bogus tokens the legacy scanner produced.
EOF
)"
```

---

## Task 9: `cook.require_env` runtime helper + env-keyset freeze

**Files:**
- Create: `cli/crates/cook-register/src/env_api.rs`
- Modify: `cli/crates/cook-register/src/engine.rs` (call `freeze_env_keyset` after config-block phase; expose `require_env`)
- Modify: `cli/crates/cook-register/src/lib.rs` (add `mod env_api;`)
- Test: inside `env_api.rs`

**Goal:** Implement the runtime side of the closed-set resolver. After config-block evaluation completes, the `cook.env` table's keyset is captured as the "declared" set. `cook.require_env(name)` returns `cook.env[name]` if name is in the declared set; otherwise raises a Lua error with a diagnostic message that lists the declared set.

- [ ] **Step 9.1: Create `env_api.rs` with the helper and failing tests**

```rust
//! cook.require_env runtime helper per CS-0033 §3.2 step 4.
//!
//! After config-block evaluation completes, the engine calls
//! `freeze_env_keyset` to capture the set of declared env-var names. From
//! that point forward, `cook.require_env(name)` raises a Lua error if
//! `name` is not in the captured set; otherwise it returns the env value
//! (which may be the empty string).

use mlua::{Lua, Table, Value};
use std::cell::RefCell;
use std::collections::BTreeSet;
use std::rc::Rc;

/// Per-Lua-state storage for the frozen env keyset.
#[derive(Default, Clone)]
pub struct EnvKeyset {
    inner: Rc<RefCell<Option<BTreeSet<String>>>>,
}

impl EnvKeyset {
    pub fn new() -> Self {
        Self::default()
    }

    /// Capture the current `cook.env` table's keyset as the declared set.
    /// Idempotent: subsequent calls are no-ops (config blocks may execute
    /// multiple times under config presets, but the declared set is the
    /// union across all runs).
    pub fn freeze(&self, env_table: &Table) -> mlua::Result<()> {
        let mut existing = self.inner.borrow_mut();
        let mut set = existing.take().unwrap_or_default();
        for pair in env_table.clone().pairs::<String, Value>() {
            let (key, _) = pair?;
            set.insert(key);
        }
        *existing = Some(set);
        Ok(())
    }

    pub fn contains(&self, key: &str) -> bool {
        self.inner
            .borrow()
            .as_ref()
            .map(|s| s.contains(key))
            .unwrap_or(false)
    }

    pub fn declared_list(&self) -> Vec<String> {
        self.inner
            .borrow()
            .as_ref()
            .map(|s| s.iter().cloned().collect())
            .unwrap_or_default()
    }
}

/// Install `cook.require_env(name)` on the given `cook` table.
pub fn install_require_env(lua: &Lua, cook_table: &Table, keyset: EnvKeyset) -> mlua::Result<()> {
    let env_table: Table = cook_table.get("env")?;
    let env_clone = env_table.clone();
    let ks = keyset.clone();
    let f = lua.create_function(move |_, name: String| -> mlua::Result<Value> {
        if !ks.contains(&name) {
            let declared = ks.declared_list();
            let msg = if declared.is_empty() {
                format!(
                    "placeholder $<{}>: env var '{}' was not declared in any config block; \
                     declare it with `env.{} = os.getenv(\"{}\") or \"\"` (or similar) in a config block",
                    name, name, name, name
                )
            } else {
                format!(
                    "placeholder $<{}>: env var '{}' was not declared. Declared env vars: {}. \
                     Add `env.{} = ...` to a config block.",
                    name,
                    name,
                    declared.join(", "),
                    name
                )
            };
            return Err(mlua::Error::RuntimeError(msg));
        }
        env_clone.get::<_, Value>(name)
    })?;
    cook_table.set("require_env", f)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use mlua::Lua;

    fn setup() -> (Lua, Table, EnvKeyset) {
        let lua = Lua::new();
        let cook: Table = lua.create_table().unwrap();
        let env: Table = lua.create_table().unwrap();
        cook.set("env", env).unwrap();
        let ks = EnvKeyset::new();
        install_require_env(&lua, &cook, ks.clone()).unwrap();
        lua.globals().set("cook", cook.clone()).unwrap();
        (lua, cook, ks)
    }

    #[test]
    fn returns_value_for_declared_key() {
        let (lua, cook, ks) = setup();
        let env: Table = cook.get("env").unwrap();
        env.set("HOME", "/home/alex").unwrap();
        ks.freeze(&env).unwrap();
        let v: String = lua.load(r#"return cook.require_env("HOME")"#).eval().unwrap();
        assert_eq!(v, "/home/alex");
    }

    #[test]
    fn returns_empty_string_for_declared_but_empty() {
        let (lua, cook, ks) = setup();
        let env: Table = cook.get("env").unwrap();
        env.set("EMPTY", "").unwrap();
        ks.freeze(&env).unwrap();
        let v: String = lua.load(r#"return cook.require_env("EMPTY")"#).eval().unwrap();
        assert_eq!(v, "");
    }

    #[test]
    fn errors_for_undeclared_key() {
        let (lua, cook, ks) = setup();
        let env: Table = cook.get("env").unwrap();
        env.set("HOME", "x").unwrap();
        ks.freeze(&env).unwrap();
        let res: mlua::Result<String> = lua.load(r#"return cook.require_env("HOEM")"#).eval();
        assert!(res.is_err());
        let msg = format!("{}", res.unwrap_err());
        assert!(msg.contains("HOEM"));
        assert!(msg.contains("declared"));
        assert!(msg.contains("HOME"));
    }

    #[test]
    fn errors_when_no_declarations_at_all() {
        let (lua, cook, ks) = setup();
        let env: Table = cook.get("env").unwrap();
        ks.freeze(&env).unwrap();
        let res: mlua::Result<String> = lua.load(r#"return cook.require_env("X")"#).eval();
        assert!(res.is_err());
        let msg = format!("{}", res.unwrap_err());
        assert!(msg.contains("not declared in any config block"));
    }
}
```

- [ ] **Step 9.2: Add the module + run tests**

In `cli/crates/cook-register/src/lib.rs`, add `pub mod env_api;`.

```bash
cd cli && cargo test -p cook-register env_api:: -- --nocapture
```

Expected: 4 tests PASS.

- [ ] **Step 9.3: Wire `freeze_env_keyset` into the engine**

In `cli/crates/cook-register/src/engine.rs`, locate the function that runs config blocks (search: `__cook_run_config_blocks` or `config_block` or `run_config`). After the call returns successfully, call `keyset.freeze(&env_table)`. The `EnvKeyset` instance lives on the `Registry` (add a field if not present); the same instance is passed into `install_require_env` at Lua-environment-construction time.

If multiple config presets are evaluated (per the `--config` selector logic), call `freeze` once per evaluation; `EnvKeyset::freeze` is idempotent under union.

- [ ] **Step 9.4: Run register tests**

```bash
cd cli && cargo test -p cook-register
```

Expected: all pass. If existing tests inject env vars without a config block (some may), update them to populate `cook.env` and call `freeze` before invoking `cook.require_env`-using code.

- [ ] **Step 9.5: Commit**

```bash
cd /home/alex/dev/cook
git add cli/crates/cook-register/src/env_api.rs cli/crates/cook-register/src/engine.rs cli/crates/cook-register/src/lib.rs
git commit -m "$(cat <<'EOF'
register: cook.require_env runtime helper + env-keyset freeze (CS-0033)

The codegen side (Task 4) emits cook.require_env("X") for sigil placeholders
that resolve to env vars. This task implements the runtime side: after
config-block evaluation completes, the engine captures the cook.env table's
keyset as the declared set. Subsequent require_env calls error if the
requested key is not declared, with a diagnostic listing the declared set.

This closes the closed-set resolution story from §3.2: every $<X> resolves
to a builtin (codegen-time), a recipe (codegen-time), or a declared env
(register-time via require_env), or fails loudly. The silent fallthrough
to empty-string env lookup is gone.
EOF
)"
```

---

## Task 10: AST source-span tracking for the migration tool

**Files:**
- Modify: `cli/crates/cook-lang/src/ast.rs` (add byte-offset spans to placeholder-bearing nodes)
- Modify: `cli/crates/cook-lang/src/recipe.rs`, `cook_line.rs`, `shell_block.rs` (record spans during parsing)

**Goal:** The migration tool needs to know the byte ranges of every shell-text region in a Cookfile so it can rewrite `{X}` → `$<X>` only inside those regions (never inside Lua bodies, comments, or globs). Today's AST nodes carry line numbers but not byte ranges. This task adds byte-range tracking to the four placeholder-bearing node kinds: output-pattern strings, shell-block bodies, plate/test bodies, bare shell commands.

- [ ] **Step 10.1: Add a `Span` type and a span field to each placeholder-bearing node**

In `cli/crates/cook-lang/src/ast.rs`, add at the top:

```rust
/// Byte range within the source file (start..end half-open).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Span {
    pub start: usize,
    pub end: usize,
}
```

For each AST struct that carries a shell-text region, add the corresponding span fields directly (no accessor methods needed — the migration tool reads them as fields):

- `CookStep`: add `pub output_pattern_spans: Vec<Span>` (one entry per declared output pattern string) and `pub body_span: Option<Span>` (None if no `using { ... }` block).
- `Step::Shell { span: Span, ... }`: add `span: Span` (existing line number stays alongside; both are useful).
- `Step::PlateBlock { body_span: Span, ... }`: add `body_span: Span`.
- `Step::TestBlock { body_span: Span, ... }`: add `body_span: Span`.

Use field access exclusively in Task 11; do not add `body_span()` / `pattern_spans()` accessor methods.

- [ ] **Step 10.2: Plumb span recording through the parser**

In each parsing function that constructs one of these nodes, capture the byte offsets of the source slice it was built from. The tokenizer in `lexer.rs` already iterates `source.lines()`; switch to `source.split('\n')` paired with a running byte offset, or use `source.lines()` plus per-line offset accumulation, to know each line's start byte.

Concretely: change `fn tokenize(source: &str) -> Result<Vec<Located<Token>>, LexError>` to also record the byte offset of each line, and propagate that into `Located<T>` (add `pub byte_offset: usize`). Then each AST-construction site reads the byte offset from the tokens it consumed.

- [ ] **Step 10.3: Write a test pinning span correctness**

In `cli/crates/cook-lang/src/tests.rs`:

```rust
#[test]
fn cook_step_records_body_span() {
    let source = r#"recipe "build"
    ingredients "main.c"
    cook "out.o" using { gcc -c {in} -o {out} }
end
"#;
    let cookfile = parse(source).expect("parse");
    let recipe = &cookfile.recipes[0];
    // Find the cook step (should be at index 1, after ingredients)
    let cook_step = recipe.steps.iter().find_map(|s| match s {
        Step::Cook(cs) => Some(cs),
        _ => None,
    }).expect("cook step");
    let span = cook_step.body_span;
    let body_text = &source[span.start..span.end];
    assert!(body_text.contains("gcc -c"), "body span must cover the using-block content; got: {:?}", body_text);
    assert!(body_text.contains("{in}"));
}
```

- [ ] **Step 10.4: Run — verify pass**

```bash
cd cli && cargo test -p cook-lang cook_step_records_body_span -- --nocapture
cd cli && cargo test -p cook-lang
```

Expected: new test passes; full suite passes.

- [ ] **Step 10.5: Commit**

```bash
cd /home/alex/dev/cook
git add cli/crates/cook-lang/src/
git commit -m "lang: track byte-range spans on placeholder-bearing AST nodes (CS-0033 prep)

Required by the parser-aware migration tool (Task 11): given a parsed
Cookfile, the tool needs to know which byte ranges of the source are
shell-text (subject to {X} → \$<X> rewrite) and which are not (Lua
bodies, comments, ingredient globs). Spans on CookStep, Step::Shell,
and plate/test step nodes provide the answer."
```

---

## Task 11: `cook --migrate-sigil` CLI subcommand + `standard.migrate` chore

**Files:**
- Create: `cli/crates/cook-cli/src/migrate_sigil.rs`
- Modify: `cli/crates/cook-cli/src/cli.rs` (add flags)
- Modify: `cli/crates/cook-cli/src/main.rs` (route the flag to the new module)
- Modify: `standard/Cookfile` (add `migrate` and `migrate-check` chores)

- [ ] **Step 11.1: Add the flags to the clap struct**

In `cli/crates/cook-cli/src/cli.rs`, after the existing `--logs` flag:

```rust
    /// Rewrite all in-scope Cookfiles from {NAME} placeholder syntax to $<NAME>
    #[arg(
        long = "migrate-sigil",
        help_heading = "Built-in commands",
        conflicts_with_all = ["menu", "init", "serve", "test", "logs"]
    )]
    pub migrate_sigil: bool,

    /// Dry-run companion of --migrate-sigil; prints the unified diff without writing
    #[arg(
        long = "migrate-sigil-check",
        help_heading = "Built-in commands",
        conflicts_with_all = ["menu", "init", "serve", "test", "logs", "migrate_sigil"]
    )]
    pub migrate_sigil_check: bool,
```

- [ ] **Step 11.2: Create the migration module**

`cli/crates/cook-cli/src/migrate_sigil.rs`:

```rust
//! `cook --migrate-sigil` implementation per CS-0033 §5.2.
//!
//! Parser-aware rewrite of `{IDENT}` → `$<IDENT>` in shell-text positions
//! within every Cookfile reachable from the current workspace. Output
//! patterns, using-block bodies, plate/test bodies, and bare shell
//! commands are rewritten; Lua bodies, comments, and ingredient globs
//! are left untouched. Recipe-name and ingredient strings (which use
//! the same `{...}` byte sequences only by coincidence) are out of
//! scope — the {...} placeholder layer never applied there.

use cook_lang::ast::{Cookfile, Step};
use cook_lang::parse;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, thiserror::Error)]
pub enum MigrateError {
    #[error("io error on {path}: {source}")]
    Io { path: PathBuf, source: std::io::Error },
    #[error("parse error on {path}: {source}")]
    Parse { path: PathBuf, source: cook_lang::ParseError },
}

pub struct MigrateOpts {
    pub root: PathBuf,
    pub dry_run: bool,
}

pub fn run(opts: MigrateOpts) -> Result<MigrateReport, MigrateError> {
    let cookfiles = collect_cookfiles(&opts.root);
    let mut report = MigrateReport::default();
    for path in &cookfiles {
        let original = fs::read_to_string(path).map_err(|e| MigrateError::Io {
            path: path.clone(),
            source: e,
        })?;
        let parsed = parse(&original).map_err(|e| MigrateError::Parse {
            path: path.clone(),
            source: e,
        })?;
        let rewritten = rewrite(&original, &parsed);
        if rewritten != original {
            report.changed.push(path.clone());
            if opts.dry_run {
                let diff = unified_diff(&original, &rewritten, path);
                println!("{}", diff);
            } else {
                fs::write(path, &rewritten).map_err(|e| MigrateError::Io {
                    path: path.clone(),
                    source: e,
                })?;
            }
        } else {
            report.unchanged.push(path.clone());
        }
    }
    Ok(report)
}

#[derive(Default)]
pub struct MigrateReport {
    pub changed: Vec<PathBuf>,
    pub unchanged: Vec<PathBuf>,
}

fn collect_cookfiles(root: &Path) -> Vec<PathBuf> {
    walkdir::WalkDir::new(root)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name() == "Cookfile")
        .filter(|e| {
            // Skip vendor / build dirs.
            let p = e.path().to_string_lossy();
            !p.contains("/target/") && !p.contains("/node_modules/") && !p.contains("/.git/")
        })
        .map(|e| e.into_path())
        .collect()
}

/// Rewrite shell-text spans in `source` from `{IDENT}` to `$<IDENT>`.
/// Identifies spans via the parsed Cookfile's recorded byte ranges (Task 10).
fn rewrite(source: &str, parsed: &Cookfile) -> String {
    let mut spans: Vec<(usize, usize)> = Vec::new();
    for recipe in &parsed.recipes {
        for step in &recipe.steps {
            collect_shell_spans(step, &mut spans);
        }
    }
    for chore in &parsed.chores {
        for step in &chore.steps {
            collect_shell_spans(step, &mut spans);
        }
    }
    spans.sort_by_key(|(s, _)| *s);
    rewrite_spans(source, &spans)
}

fn collect_shell_spans(step: &Step, out: &mut Vec<(usize, usize)>) {
    match step {
        Step::Cook(cs) => {
            for pat_span in &cs.output_pattern_spans {
                out.push((pat_span.start, pat_span.end));
            }
            if let Some(body_span) = &cs.body_span {
                out.push((body_span.start, body_span.end));
            }
        }
        Step::Shell { span, .. } => out.push((span.start, span.end)),
        Step::PlateBlock { body_span, .. } | Step::TestBlock { body_span, .. } => {
            out.push((body_span.start, body_span.end));
        }
        _ => {}
    }
}

fn rewrite_spans(source: &str, spans: &[(usize, usize)]) -> String {
    let mut out = String::with_capacity(source.len() + 64);
    let mut cursor = 0;
    for (s, e) in spans {
        out.push_str(&source[cursor..*s]);
        out.push_str(&rewrite_one_span(&source[*s..*e]));
        cursor = *e;
    }
    out.push_str(&source[cursor..]);
    out
}

/// Within a single shell-text span, rewrite every `{IDENT}` whose IDENT
/// would have been a valid placeholder under the legacy semantics
/// (identifier-shape inner) to `$<IDENT>`. Inners that fail the shape
/// (e.g., `{a,b,c}`, `{1..3}`) are left literal.
fn rewrite_one_span(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'{' {
            if let Some(end) = scan_legacy_placeholder(text, i) {
                let inner = &text[i + 1..end];
                out.push_str("$<");
                out.push_str(inner);
                out.push('>');
                i = end + 1;
                continue;
            }
        }
        out.push(bytes[i] as char);
        i += 1;
    }
    out
}

/// If `text[start..]` begins with a legacy `{IDENT}` placeholder
/// (where IDENT matches the same shape Task 2's sigil scanner accepts),
/// return the byte offset of the closing `}`. Otherwise None.
fn scan_legacy_placeholder(text: &str, start: usize) -> Option<usize> {
    let bytes = text.as_bytes();
    debug_assert_eq!(bytes[start], b'{');
    let mut i = start + 1;
    if i >= bytes.len() {
        return None;
    }
    let first = bytes[i];
    if !(first.is_ascii_alphabetic() || first == b'_') {
        return None;
    }
    i += 1;
    while i < bytes.len() {
        let b = bytes[i];
        if b.is_ascii_alphanumeric() || b == b'_' || b == b'.' {
            i += 1;
            continue;
        }
        break;
    }
    if i < bytes.len() && bytes[i] == b'}' {
        Some(i)
    } else {
        None
    }
}

fn unified_diff(_a: &str, _b: &str, path: &Path) -> String {
    // Minimal unified-diff format. Use the `similar` crate if available,
    // otherwise a hand-rolled per-line diff. Prefer the similar crate.
    use similar::TextDiff;
    let diff = TextDiff::from_lines(_a, _b);
    let mut out = format!("--- {}\n+++ {}\n", path.display(), path.display());
    for op in diff.ops() {
        for change in diff.iter_changes(op) {
            let sign = match change.tag() {
                similar::ChangeTag::Delete => "-",
                similar::ChangeTag::Insert => "+",
                similar::ChangeTag::Equal => " ",
            };
            out.push_str(sign);
            out.push_str(change.value());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rewrite_one_span_basic() {
        assert_eq!(rewrite_one_span("gcc -c {in} -o {out}"), "gcc -c $<in> -o $<out>");
    }

    #[test]
    fn rewrite_one_span_dotted() {
        assert_eq!(rewrite_one_span("build/{in.stem}.o"), "build/$<in.stem>.o");
    }

    #[test]
    fn rewrite_one_span_skips_shell_braces() {
        assert_eq!(rewrite_one_span("for i in {1..3}; do echo $i; done"), "for i in {1..3}; do echo $i; done");
        assert_eq!(rewrite_one_span("cp file.{c,h} dest/"), "cp file.{c,h} dest/");
        assert_eq!(rewrite_one_span("awk '{print $1}'"), "awk '{print $1}'");
        assert_eq!(rewrite_one_span("${HOME:-x}"), "${HOME:-x}");
        assert_eq!(rewrite_one_span("find . -exec rm {} \\;"), "find . -exec rm {} \\;");
    }
}
```

Add `walkdir` and `similar` to `cli/crates/cook-cli/Cargo.toml` `[dependencies]` if not already present:

```toml
walkdir = "2"
similar = "2"
```

- [ ] **Step 11.3: Wire the flag in `main.rs`**

```rust
if cli.migrate_sigil || cli.migrate_sigil_check {
    let opts = migrate_sigil::MigrateOpts {
        root: std::env::current_dir()?,
        dry_run: cli.migrate_sigil_check,
    };
    let report = migrate_sigil::run(opts)?;
    eprintln!(
        "migrate-sigil: {} files changed, {} unchanged",
        report.changed.len(),
        report.unchanged.len()
    );
    return Ok(());
}
```

Place this before any other command dispatch.

- [ ] **Step 11.4: Add the chore wrapper to `standard/Cookfile`**

Append to `standard/Cookfile`:

```cook
chore migrate
    @cd .. && cook --migrate-sigil

chore migrate-check
    @cd .. && cook --migrate-sigil-check
```

- [ ] **Step 11.5: Run the migration tool's own unit tests**

```bash
cd cli && cargo test -p cook-cli migrate_sigil:: -- --nocapture
```

Expected: 3 tests PASS.

- [ ] **Step 11.6: Smoke-test the binary end-to-end**

```bash
cd cli && cargo build --bin cook
mkdir -p /tmp/migrate-smoke && cat > /tmp/migrate-smoke/Cookfile <<'EOF'
recipe "build"
    ingredients "main.c"
    cook "build/{in.stem}.o" using { gcc -c {in} -o {out} }
end
EOF
cd /tmp/migrate-smoke && /home/alex/dev/cook/cli/target/debug/cook --migrate-sigil-check
```

Expected: dry-run diff shows `{in.stem}` → `$<in.stem>`, `{in}` → `$<in>`, `{out}` → `$<out>`. No changes to comments or globs.

- [ ] **Step 11.7: Commit**

```bash
cd /home/alex/dev/cook
git add cli/crates/cook-cli/src/cli.rs cli/crates/cook-cli/src/main.rs cli/crates/cook-cli/src/migrate_sigil.rs cli/crates/cook-cli/Cargo.toml standard/Cookfile
git commit -m "cli: cook --migrate-sigil rewrites {IDENT} → \$<IDENT> (CS-0033)

Parser-aware migration tool per §5.2 of the design. Walks every
Cookfile reachable from the current workspace; rewrites placeholder
spans only inside output patterns, using-block bodies, plate/test
bodies, and bare shell commands. Lua bodies, comments, ingredient
globs, and recipe-name strings are untouched.

--migrate-sigil-check is the dry-run companion (prints unified diff,
makes no changes). The standard.migrate / standard.migrate-check
chores wrap both for in-repo use."
```

---

## Task 12: Migrate the conformance corpus + re-enable the harness

**Files:**
- Modify: every `standard/conformance/{positive,negative}/**/Cookfile`
- Regenerate: every `standard/conformance/{positive,negative}/**/parse.txt`
- Modify: `cli/crates/cook-lang/tests/conformance.rs` (remove the `#[ignore]` from Task 1)

- [ ] **Step 12.1: Run the migration tool against the corpus**

```bash
cd /home/alex/dev/cook/standard/conformance && /home/alex/dev/cook/cli/target/debug/cook --migrate-sigil
```

Expected: `migrate-sigil: N files changed, M unchanged` printed to stderr. N+M should equal the number of fixture Cookfiles (33 positive + 34 negative = 67 — confirm with `find . -name Cookfile | wc -l`).

- [ ] **Step 12.2: Regenerate `parse.txt` files**

The conformance harness compares parser dumps against the per-fixture `parse.txt`. The migrated Cookfiles produce identical AST shapes (only the source bytes changed), but the dumps include source-text echoing in some places. Regenerate:

```bash
cd cli && CARGO_TEST_REGENERATE=1 cargo test -p cook-lang --test conformance -- --ignored
```

If the harness doesn't support a regenerate mode, regenerate manually with a one-off chore:

```bash
cd /home/alex/dev/cook
for d in standard/conformance/positive/*/ standard/conformance/negative/*/; do
    /home/alex/dev/cook/cli/target/debug/cook --emit-parse-tree "$d/Cookfile" > "$d/parse.txt" 2>/dev/null || true
done
```

(Confirm the binary's parse-emit flag name; adjust if different.)

- [ ] **Step 12.3: Re-enable the conformance harness**

In `cli/crates/cook-lang/tests/conformance.rs`, remove every `#[ignore = "re-enabled in Task 12 ..."]` attribute added in Task 1.

- [ ] **Step 12.4: Run the harness — verify all pass**

```bash
cd cli && cargo test -p cook-lang --test conformance
```

Expected: all 67 fixtures pass (or whatever the current count is).

- [ ] **Step 12.5: Commit**

```bash
cd /home/alex/dev/cook
git add standard/conformance/ cli/crates/cook-lang/tests/conformance.rs
git commit -m "$(cat <<'EOF'
conformance: migrate corpus to \$<IDENT> sigil syntax (CS-0033)

Mass migration of all positive (33) and negative (34) fixtures via
cook --migrate-sigil. Parse.txt files regenerated. Conformance harness
re-enabled (was marked ignore during the parser-rewrite phase since
Task 1).

No semantic changes — every fixture's AST is identical to its
pre-migration shape; only the source bytes use the new placeholder
syntax.
EOF
)"
```

---

## Task 13: Migrate `examples/`

**Files:** every `examples/**/Cookfile`

- [ ] **Step 13.1: Run the tool on examples**

```bash
cd /home/alex/dev/cook/examples && /home/alex/dev/cook/cli/target/debug/cook --migrate-sigil
```

- [ ] **Step 13.2: Smoke-test a representative example**

```bash
cd /home/alex/dev/cook/examples/cpp-project && /home/alex/dev/cook/cli/target/debug/cook --emit-lua | head -20
```

Expected: lowered Lua references `cook.dep_output`, `cook.require_env`, builtins as `_cook_in`/`_cook_out`. No literal `$<` strings remain in `command =` fields (which would mean substitution didn't fire).

- [ ] **Step 13.3: Run any example's verify script if present**

```bash
cd /home/alex/dev/cook/examples/cmd_test_inferred_deps_e10 && bash verify.sh
```

Expected: pass.

- [ ] **Step 13.4: Commit**

```bash
cd /home/alex/dev/cook
git add examples/
git commit -m "examples: migrate all fixtures to \$<IDENT> sigil syntax (CS-0033)"
```

---

## Task 14: Migrate in-tree top-level Cookfiles

**Files:** `Cookfile`, `cli/Cookfile`, `standard/Cookfile`, `tree-sitter-cook/Cookfile`

- [ ] **Step 14.1: Run the tool on each top-level Cookfile**

```bash
cd /home/alex/dev/cook && /home/alex/dev/cook/cli/target/debug/cook --migrate-sigil
```

The tool already walks the workspace from the current dir, so this catches every top-level Cookfile that wasn't already migrated by Tasks 12 / 13.

- [ ] **Step 14.2: Commit**

```bash
cd /home/alex/dev/cook
git status --short
git add Cookfile cli/Cookfile standard/Cookfile tree-sitter-cook/Cookfile
git commit -m "in-tree: migrate top-level Cookfiles to \$<IDENT> sigil syntax (CS-0033)"
```

---

## Task 15: Add positive conformance fixtures for legitimate shell `{}` idioms

**Files:**
- Create: `standard/conformance/positive/034-shell-brace-expansion/{Cookfile,parse.txt,notes.md}`
- Create: `standard/conformance/positive/035-shell-parameter-expansion/{Cookfile,parse.txt,notes.md}`
- Create: `standard/conformance/positive/036-shell-awk-script/{Cookfile,parse.txt,notes.md}`
- Create: `standard/conformance/positive/037-shell-find-exec/{Cookfile,parse.txt,notes.md}`
- Create: `standard/conformance/positive/038-shell-json-literal/{Cookfile,parse.txt,notes.md}`

(Numbering picks up from the highest existing positive case + 1.)

For each fixture:

- [ ] **Step 15.1: Create `034-shell-brace-expansion/Cookfile`**

```cook
recipe "build"
    ingredients "src.txt"
    cook "out.txt" using {
        for i in {1..3}; do echo "iter $i" >> $<out>; done
        cp file.{c,h} dest/ 2>/dev/null || true
    }
end
```

- [ ] **Step 15.2: Generate `parse.txt`**

```bash
cd /home/alex/dev/cook/cli && cargo build --bin cook
/home/alex/dev/cook/cli/target/debug/cook --emit-parse-tree /home/alex/dev/cook/standard/conformance/positive/034-shell-brace-expansion/Cookfile > /home/alex/dev/cook/standard/conformance/positive/034-shell-brace-expansion/parse.txt
```

- [ ] **Step 15.3: Create `notes.md`**

```markdown
# 034 — shell brace expansion in `using` body

Pins the CS-0033 contract that legitimate Bash brace expansion (`{1..3}`,
`{c,h}`) inside a `cook ... using { ... }` body passes verbatim to the
shell. Pre-CS-0033, the `{TOKEN}` placeholder scanner consumed these
sequences as env-var lookups (`cook.env["1..3"]` etc.), corrupting the
emitted command.
```

- [ ] **Step 15.4: Repeat for fixtures 035–038**

Use the spec's table of shell idioms (§1 of the design doc) for the bodies. Each fixture's Cookfile contains exactly one cook step whose `using` body exercises the shell construct in question; the `notes.md` names the construct and points to CS-0033.

- [ ] **Step 15.5: Run the harness**

```bash
cd cli && cargo test -p cook-lang --test conformance
```

Expected: all fixtures (including the new ones) pass.

- [ ] **Step 15.6: Commit**

```bash
cd /home/alex/dev/cook
git add standard/conformance/positive/034-shell-brace-expansion standard/conformance/positive/035-shell-parameter-expansion standard/conformance/positive/036-shell-awk-script standard/conformance/positive/037-shell-find-exec standard/conformance/positive/038-shell-json-literal
git commit -m "conformance: positive fixtures pinning shell {} idioms pass through (CS-0033)"
```

---

## Task 16: Add negative conformance fixtures for hard-error cases

**Files:**
- Create: `standard/conformance/negative/035-undeclared-env-sigil/{Cookfile,parse.txt,notes.md}`
- Create: `standard/conformance/negative/036-reserved-env-recipe/{Cookfile,parse.txt,notes.md}`
- Create: `standard/conformance/negative/037-builtin-wrong-mode-sigil/{Cookfile,parse.txt,notes.md}`

(Numbering picks up from the highest existing negative case + 1.)

- [ ] **Step 16.1: `035-undeclared-env-sigil/Cookfile`**

```cook
recipe "demo"
    @echo $<UNDECLARED>
end
```

`notes.md`:

```markdown
# 035 — $<UNDECLARED> with no config block declaring it

Pins the CS-0033 §3.2 closed-set rule. With no `env.UNDECLARED = ...`
in any config block, the `$<UNDECLARED>` lookup must fail at register
time via `cook.require_env`. The diagnostic must name the missing
key and list declared env vars.
```

The fixture is "negative" in the runtime sense: parsing succeeds, but execution fails. Conformance fixtures today distinguish parse-time from runtime negatives — confirm by reading `cli/crates/cook-lang/tests/conformance.rs` and the existing negative fixtures' notes.md entries. If the conformance harness only covers parse-time, route this fixture into `examples/` instead with a `verify.sh` that asserts the runtime error.

- [ ] **Step 16.2: `036-reserved-env-recipe/Cookfile`**

```cook
recipe "env.foo"
end
```

`notes.md`:

```markdown
# 036 — recipe with `env` first segment is rejected

Pins the CS-0033 §3.2.1 rule: `env` is reserved as a first dotted
segment in recipe names because `$<env.X>` is reserved as the explicit
env-var-lookup form. Allowing `recipe env.foo` would create
resolution ambiguity for `$<env.foo>`.
```

- [ ] **Step 16.3: `037-builtin-wrong-mode-sigil/Cookfile`**

```cook
recipe "build"
    ingredients "*.txt"
    cook "out.txt" using {
        cat $<all> | head > $<in>
    }
end
```

(`{in}` in many-to-one mode is a load-time error; this fixture pins the diagnostic shape under the new syntax.)

- [ ] **Step 16.4: Generate parse.txt for each, run the harness, commit**

```bash
for f in 035-undeclared-env-sigil 036-reserved-env-recipe 037-builtin-wrong-mode-sigil; do
    /home/alex/dev/cook/cli/target/debug/cook --emit-parse-tree "/home/alex/dev/cook/standard/conformance/negative/$f/Cookfile" > "/home/alex/dev/cook/standard/conformance/negative/$f/parse.txt" 2>&1 || true
done
cd cli && cargo test -p cook-lang --test conformance
```

Then:

```bash
cd /home/alex/dev/cook
git add standard/conformance/negative/035-undeclared-env-sigil standard/conformance/negative/036-reserved-env-recipe standard/conformance/negative/037-builtin-wrong-mode-sigil
git commit -m "conformance: negative fixtures pinning closed-set resolution (CS-0033)"
```

---

## Task 17: Spec — §2 lexical, §xref.resolution, §6.7 rewrites

**Files:**
- Modify: `standard/src/content/docs/02-lexical.mdx`
- Modify: `standard/src/content/docs/05-cross-recipe-references.mdx`
- Modify: `standard/src/content/docs/06-cook-lua-api.mdx`

These three changes are tightly coupled and ride in one commit so the spec is internally consistent at every snapshot.

- [ ] **Step 17.1: Add §2.X "Placeholders in shell text" to `02-lexical.mdx`**

Append a new subsection after the existing `STRING` / `BARE_IDENTIFIER` sections (locate by reading the current file). The section text:

````markdown
## 2.X. Placeholders in shell text [#lexical.placeholders]

> **Normative.** A **placeholder** is the byte sequence `$<IDENT>` appearing in shell text (defined in §{lua.shell-placeholders}). The lexer recognizes a placeholder by the production:

```
placeholder       = "$<" placeholder_ident ">"
placeholder_ident = bare_ident | out_indexed | out_indexed_acc
bare_ident        = ALPHA (ALPHA | DIGIT | "_" | ".")*
out_indexed       = "out_" DIGIT+
out_indexed_acc   = "out_" DIGIT+ "." accessor
accessor          = "stem" | "name" | "ext" | "dir"
ALPHA             = "a"…"z" | "A"…"Z" | "_"
DIGIT             = "0"…"9"
```

A byte sequence beginning with `$<` that does not match this production in full — by malformed `IDENT`, missing `>`, or `IDENT` exceeding the production's character set — is **not** a placeholder. Its bytes are literal shell text. A conforming implementation MUST NOT search forward beyond the first non-`IDENT` byte for a closing `>`.
````

- [ ] **Step 17.2: Update §xref.resolution in `05-cross-recipe-references.mdx`**

Find §xref.resolution (search: `## .*resolution\|xref.resolution`). Replace the step-4 fallthrough rule with the closed-set rule. The new wording:

```markdown
A conforming implementation MUST resolve a placeholder `$<TOKEN>` by applying these steps in order; first match wins:

1. **Builtin.** If `TOKEN` matches one of the builtin shapes from §{lua.shell-placeholders}, the placeholder substitutes per the builtin's row.
2. **Recipe in scope.** If `TOKEN` is the name of a recipe reachable from the current Cookfile (own, `use`-imported, or sigil-imported per §7), the placeholder substitutes to that recipe's terminal output(s) per §{xref.string-substitution}. If `TOKEN` has the shape `RECIPE.ACCESSOR`, the accessor applies to the recipe's single output via §{xref.path-accessors}.
3. **Declared env var.** If `TOKEN` (or `env.TOKEN` in its explicit form, see §{xref.env-namespace}) appears as `env.TOKEN = ...` in a config block reachable from this Cookfile's config-block scope, the placeholder substitutes to `cook.env["TOKEN"]`. The implementation MUST capture the consulted key in the unit's `consulted_env_keys` for cache invalidation.
4. **Hard error.** A conforming implementation MUST emit a load-time diagnostic for any placeholder that fails to resolve under steps 1–3. The diagnostic MUST name the unresolved TOKEN and SHOULD enumerate declared env vars and recipes in scope.
```

Add §xref.env-namespace describing the explicit `env.` prefix per §3.2.1 of the design.

- [ ] **Step 17.3: Rewrite §6.7 in `06-cook-lua-api.mdx`**

Locate the existing §6.7 (around line 288). Replace the placeholder table's "Placeholder" column to use the `$<...>` shape:

| Old | New |
|---|---|
| `{in}` | `$<in>` |
| `{in.ACCESSOR}` | `$<in.ACCESSOR>` |
| `{out}` | `$<out>` |
| `{out.ACCESSOR}` | `$<out.ACCESSOR>` |
| `{out_N}` | `$<out_N>` |
| `{out_N.ACCESSOR}` | `$<out_N.ACCESSOR>` |
| `{all}` | `$<all>` |

Update Examples 6.7.1 and 6.7.2 to use the new syntax. Update the §6.7.1 plate/test placeholder table identically. Remove the `{TOKEN} (none of the above) → cook.env[TOKEN]` row entirely — closed-set resolution per §xref.resolution replaces it.

The "Phase" preamble adds: "Substitution is performed by the code generator at register time over the strict `$<IDENT>` lexical shape defined in §{lexical.placeholders}."

- [ ] **Step 17.4: Run the spec build**

```bash
cd standard && pnpm build && pnpm test && pnpm lint:keywords
```

Expected: clean exit. The build will validate cross-references and bare-ref linting.

- [ ] **Step 17.5: Commit**

```bash
cd /home/alex/dev/cook
git add standard/src/content/docs/02-lexical.mdx standard/src/content/docs/05-cross-recipe-references.mdx standard/src/content/docs/06-cook-lua-api.mdx
git commit -m "spec(CS-0033): §2 placeholders + §xref.resolution + §6.7 sigil rewrite"
```

---

## Task 18: Spec — Appendix A grammar production

**Files:**
- Modify: `standard/src/content/docs/appendix/A-grammar.mdx`

- [ ] **Step 18.1: Add the `placeholder` production**

Find the EBNF block (search: `placeholder\|output_pattern\|shell_block`). Add the production from §3.1 of the design (or copy from §2.X). Reference it from `output_pattern`, `shell_block_body`, `bare_shell_command`, and `plate_test_shell_block_body` productions.

- [ ] **Step 18.2: Build + commit**

```bash
cd standard && pnpm build
cd /home/alex/dev/cook
git add standard/src/content/docs/appendix/A-grammar.mdx
git commit -m "spec(CS-0033): App. A grammar — \$<IDENT> placeholder production"
```

---

## Task 19: Spec — App. B rationale, App. D changelog, App. E updates

**Files:**
- Modify: `standard/src/content/docs/appendix/B-rationale.mdx`
- Modify: `standard/src/content/docs/appendix/D-changes.mdx`
- Modify: `standard/src/content/docs/appendix/E-pre-v1-checklist.mdx`

- [ ] **Step 19.1: App. B — new rationale section**

Add a section to App. B explaining why CS-0033 chose the sigil syntax and closed-set resolution. The section MUST cover:

- Why `{NAME}` collided with shell (cite the empirical breakage table from the design's §1)
- Why `$<...>` was selected over alternative sigils (`${...}`, `$(...)`, `@{...}`, `%{...}`)
- Why closed-set resolution was preferred over the silent-empty-string fallthrough
- Why a flag-day cut was preferred over dual-grammar transition

Source material: §1, §3.1, §6 of `standard/specs/2026-05-03-sigil-placeholder-syntax-design.md`. Compress to ~300 words.

- [ ] **Step 19.2: App. D — CS-0033 entry + v0.7 cut entry**

Add two entries to `D-changes.mdx`. Match the format of CS-0031 (v0.6 cut) and CS-0027 (substantive change).

CS-0033 entry header:

```markdown
## D.33. CS-0033 — `$<IDENT>` sigil-disambiguated placeholder syntax with closed-set resolution. [#changes.cs-0033]

**Date:** 2026-05-03

**Version:** v0.7

**Sections affected:** §{lexical.placeholders} (new), §{xref.resolution}, §{xref.env-namespace} (new), §{lua.shell-placeholders}, App. A (`placeholder` production), App. E (E.2 closed structurally, E.8 closed by uniform substitution).
```

Followed by Summary, Why, Implementation notes, Conformance impact, Reference subsections. Source material: the design doc.

CS-0034 (or whatever the next CS number is) — Standard cut v0.7:

```markdown
## D.34. CS-0034 — Standard cut: v0.7. [#changes.cs-0034]
```

Match the form of D.31 (v0.6 cut) and D.23 (v0.5 cut).

- [ ] **Step 19.3: App. E — E.2 and E.8 closure**

E.2: rewrite the **Status** line to "Fully resolved by [CS-0033](D-changes#changes-cs-0033). The collision between Cook's placeholder syntax and shell `{...}` constructs is structurally eliminated — Cook placeholders now use `$<IDENT>`, which has no collision with POSIX shell."

E.8: rewrite the **Status** to "Resolved by [CS-0033](D-changes#changes-cs-0033). The chore-body shell substitution was a symptom of the broader spec/impl mismatch in the brace-based placeholder layer. CS-0033 unifies substitution across all shell-text contexts (recipe-body bare shell, chore-body bare shell, `using { ... }`, plate, test) under the new `$<IDENT>` syntax."

- [ ] **Step 19.4: Build + commit**

```bash
cd standard && pnpm build && pnpm test && pnpm lint:keywords
cd /home/alex/dev/cook
git add standard/src/content/docs/appendix/B-rationale.mdx standard/src/content/docs/appendix/D-changes.mdx standard/src/content/docs/appendix/E-pre-v1-checklist.mdx
git commit -m "spec(CS-0033): App. B rationale, App. D changelog, App. E.2/E.8 closure"
```

---

## Task 20: Tree-sitter version banner

**Files:**
- Modify: `tree-sitter-cook/grammar.js` (header comment only)

- [ ] **Step 20.1: Update the version banner**

Edit the header comment in `tree-sitter-cook/grammar.js`:

```javascript
// tree-sitter-cook claims conformance with Cook Standard v0.4 + CS-0022
// (cs-standard/v0.4). See standard/src/content/docs/appendix/A-grammar.mdx
// for the normative grammar this file mirrors.
```

Becomes:

```javascript
// tree-sitter-cook claims conformance with Cook Standard v0.4 + CS-0022.
// The grammar is STALE relative to v0.7 (cs-standard/v0.7); it does not
// implement CS-0023 onward (plate/test block bodies, `//`-anchored sigil
// imports, `$<IDENT>` placeholder syntax). See standard/src/content/docs/
// appendix/A-grammar.mdx for the normative grammar; the catch-up is
// tracked by CS-0002 (planned tree-sitter-cook conformance audit).
```

- [ ] **Step 20.2: Commit**

```bash
cd /home/alex/dev/cook
git add tree-sitter-cook/grammar.js
git commit -m "tree-sitter-cook: bump version banner to flag v0.7 staleness (CS-0033)"
```

---

## Task 21: Final verification — full test suite + conformance + spec build

This is a verification-only task; no code changes.

- [ ] **Step 21.1: Run the full Rust test suite**

```bash
cd cli && cargo test
```

Expected: all green.

- [ ] **Step 21.2: Run the conformance harness**

```bash
cd cli && cargo test -p cook-lang --test conformance
```

Expected: all green.

- [ ] **Step 21.3: Run the spec build + lints**

```bash
cd standard && pnpm build && pnpm test && pnpm lint:keywords
```

Expected: all green.

- [ ] **Step 21.4: Run a representative example end-to-end**

```bash
cd /home/alex/dev/cook/examples/cmd_test_inferred_deps_e10 && bash verify.sh
cd /home/alex/dev/cook/examples/cross_cookfile_test && bash walkthrough.sh
```

Expected: both succeed.

- [ ] **Step 21.5: Verify the E.8 repro now passes**

```bash
mkdir -p /tmp/sigil-final
cat > /tmp/sigil-final/Cookfile <<'EOF'
config
    env.ADB = "echo MOCK-ADB"

chore devices
    $<ADB> devices
EOF
cd /tmp/sigil-final && /home/alex/dev/cook/cli/target/debug/cook devices
```

Expected: prints `MOCK-ADB devices`. (Pre-CS-0033 this failed with `{ADB}: command not found`.)

If anything fails, fix the regression in a follow-up task before proceeding to Task 22.

---

## Task 22: Cut v0.7

**Files:**
- Modify: `standard/VERSION` (single line: `0.7`)

- [ ] **Step 22.1: Bump VERSION**

```bash
cd /home/alex/dev/cook
echo "0.7" > standard/VERSION
```

- [ ] **Step 22.2: Verify the changelog already records v0.7**

```bash
grep -n "v0.7\|cs-standard/v0.7" standard/src/content/docs/appendix/D-changes.mdx
```

Expected: matches from the CS-0033 + v0.7-cut entries added in Task 19.

- [ ] **Step 22.3: Commit and tag**

```bash
cd /home/alex/dev/cook
git add standard/VERSION
git commit -m "spec: cut Cook Standard v0.7 (CS-0033, sigil placeholders)"
git tag cs-standard/v0.7 HEAD
```

The `against-tag` chore can now be re-run to verify the Standard's surface against the new tag:

```bash
cd standard && cook against-tag
```

---

## Self-review checklist (run after the plan executes)

- [ ] Every task in the plan has a commit on the branch
- [ ] `cargo test` is green at every commit (no broken intermediate states except Task 1's intentional `#[ignore]`)
- [ ] No `{IDENT}` placeholder syntax remains in any in-tree Cookfile (`grep -rn '{[a-zA-Z_][a-zA-Z0-9_.]*}' --include=Cookfile`) except inside Lua bodies, comments, or ingredient globs (which is fine)
- [ ] `cook --migrate-sigil-check` is a no-op on the migrated tree
- [ ] The new conformance fixtures (Tasks 15, 16) are referenced in App. E or in the §6.7 rewrite (Task 17) as worked examples
- [ ] App. D's CS-0033 entry exists and links to this design doc
