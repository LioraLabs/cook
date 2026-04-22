# Contributing to Cook

## The Cook Standard

The Cookfile language is defined by the Cook Standard in [`docs/standard/`](docs/standard/). The Standard is the authoritative reference for the language. The Rust parser in `cli/crates/cook-lang/` is the current reference implementation; it is the de-facto authority for any Cookfile construct whose Standard chapter is presently a `NORMATIVE-TODO` stub.

### Spec-first rule

Any change that affects Cookfile surface syntax, execution semantics, the Cook Lua API, or the module system MUST:

1. Update `docs/standard/` in the same commit that modifies the implementation.
2. Add one entry to `docs/standard/D-changes.mdx` with a new stable `CS-NNNN` ID, a one-line summary, the sections affected, and the commit reference.
3. If the grammar changes, update `docs/standard/A-grammar.mdx`.
4. If the change is observable from a Cookfile, add at least one case to `docs/standard/conformance/positive/` or `docs/standard/conformance/negative/`.

Non-trivial language changes SHOULD be designed at the Standard level first; the implementation follows.

### Local enforcement

The repo ships a portable `pre-commit` hook at `.githooks/pre-commit` that inspects the staged diff and warns when you've touched language-surface code without also touching `docs/standard/`. Install it once per clone:

````bash
git config core.hooksPath .githooks
````

The hook's goal is to make language impact visible at commit time. If you're making a non-language-affecting change (refactor, performance work, error-message rewording), set `COOK_STANDARD_BYPASS=1` for that commit.

### Language-surface paths (what the hook watches)

- `cli/crates/cook-lang/**` — the lexer, parser, and AST
- `cli/crates/cook-luagen/**` — codegen that materializes language constructs
- `cli/crates/cook-register/**` — Cook Lua API registration
- `tree-sitter-cook/grammar.js` — tree-sitter grammar (currently out of date; see `docs/standard/D-changes.mdx` CS-0002)
- `tree-sitter-cook/src/**` — tree-sitter externals

If you add a new crate that contributes to language surface, update both this list and the hook.

### Conformance

- `cli/crates/cook-lang/tests/conformance.rs` walks `docs/standard/conformance/` and asserts the Rust parser's behavior. Run it with `cargo test -p cook-lang --test conformance`.
- A tree-sitter harness against the same corpus is planned; see `D-changes.mdx` CS-0002.

### Running the normative-keyword lint

````bash
bash scripts/check-normative-keywords.sh
````

The lint flags lowercase `must`/`shall`/`should`/`may` occurrences in normative chapters. Review each hit: either promote to all-caps (if the clause is meant to be binding) or reword (if the clause is descriptive).
