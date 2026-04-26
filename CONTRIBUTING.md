# Contributing to Cook

## The Cook Standard

The Cookfile language is defined by the Cook Standard in [`standard/`](standard/). The Standard is the authoritative reference for the language. The Rust parser in `cli/crates/cook-lang/` is the current reference implementation; it is the de-facto authority for any Cookfile construct whose Standard chapter is presently a `NORMATIVE-TODO` stub.

### Spec-first rule

Any change that affects Cookfile surface syntax, execution semantics, the Cook Lua API, or the module system MUST:

1. Update `standard/` in the same commit that modifies the implementation.
2. Add one entry to `standard/src/content/docs/appendix/D-changes.mdx` with a new stable `CS-NNNN` ID, a one-line summary, the sections affected, and the commit reference.
3. If the grammar changes, update `standard/src/content/docs/appendix/A-grammar.mdx`.
4. If the change is observable from a Cookfile, add at least one case to `standard/conformance/positive/` or `standard/conformance/negative/`.

Non-trivial language changes SHOULD be designed at the Standard level first; the implementation follows.

Rendering infrastructure changes in `standard/src/plugins/`, `standard/src/styles/`, `standard/astro.config.mjs`, `standard/package.json`, and `standard/tsconfig.json` are not spec changes. They do not require a `CS-NNNN` entry and do not have to be bundled with a language-surface change.

### Local enforcement

The repo ships a portable `pre-commit` hook at `.githooks/pre-commit` that inspects the staged diff and warns when you've touched language-surface code without also touching `standard/`. Install it once per clone:

````bash
git config core.hooksPath .githooks
````

The hook's goal is to make language impact visible at commit time. If you're making a non-language-affecting change (refactor, performance work, error-message rewording), set `COOK_STANDARD_BYPASS=1` for that commit.

### Language-surface paths (what the hook watches)

- `cli/crates/cook-lang/**` — the lexer, parser, and AST
- `cli/crates/cook-luagen/**` — codegen that materializes language constructs
- `cli/crates/cook-register/**` — Cook Lua API registration
- `tree-sitter-cook/grammar.js` — tree-sitter grammar (currently out of date; see `standard/src/content/docs/appendix/D-changes.mdx` CS-0002)
- `tree-sitter-cook/src/**` — tree-sitter externals

If you add a new crate that contributes to language surface, update both this list and the hook.

### Conformance

- `cli/crates/cook-lang/tests/conformance.rs` walks `standard/conformance/` and asserts the Rust parser's behavior. Run it with `cargo test -p cook-lang --test conformance`.
- A tree-sitter harness against the same corpus is planned; see `D-changes.mdx` CS-0002.

### Cutting a Cook Standard version

The Standard uses `MAJOR.MINOR` versioning pre-1.0 (see [`§ 0.5`](standard/src/content/docs/00-introduction.mdx)). A *cut* publishes a new MINOR by performing three actions in a single commit on `main`:

1. Bump `standard/VERSION` to the next MINOR (e.g. `0.2` → `0.3`).
2. Add a new entry to the top of the App. D **Versions** index in `standard/src/content/docs/appendix/D-changes.mdx`, listing the CSes the cut covers and the cut date.
3. Set each batched CS body's `**Version:**` line to the new version.

After the commit lands on `main`, tag it:

```bash
git tag cs-standard/vX.Y
git push origin cs-standard/vX.Y
```

The tag and the index entry together constitute the published cut.

The cut commit MAY also batch the CS that introduced the cut-worthy change (i.e. the CS that adds the `**Version:**` line and the index entry can do so in the same commit that adds its own body). There is no rule against a cut containing exactly one CS — it is simply not required.

**Operating rules.**

- **`**Version:**` records when a CS entry was authored, not when its work ships.** It is never rewritten retroactively. If a CS forward-references work that later ships in a higher version, record the completion as a new CS in the higher version, not by editing the original entry's `**Version:**` line. (Example: CS-0002 forward-references the planned tree-sitter conformance audit; it carries `**Version:** v0.1`. When the audit ships, it gets its own CS entry under the then-current version, and CS-0002 stays at v0.1.)
- **Update the Versions index date field when additional CSes land in the same in-progress version.** A cut that initially contained only CS-0011 on 2026-04-26 lists the date as `2026-04-26`; if CS-0013 lands in v0.2 on a later day, widen the entry to a date range (`2026-04-26..YYYY-MM-DD`) at that time.
- **Informative-appendix navigational headings (e.g. App. D's Versions index, future Index/Acknowledgements sections) use Starlight's natural slugifier.** Do NOT add a `[#slug]` marker. The `rehype-clause-anchors` plugin only strips `[#slug]` markers from clause-numbered headings (`N.` or `[A-Z].` prefix); other headings get the marker text leaked into their rendered HTML id. Cross-references to navigational headings use plain markdown links (e.g. `[Versions](#versions)`), not `§{...}` slug refs.

### Implementation conformance claims

The Cook Standard does not normatively require an implementation to expose its claimed Standard version (see [`§ 0.7`](standard/src/content/docs/00-introduction.mdx)). As a project convention:

- **`cli/crates/cook-lang`** — set a `pub const COOK_STANDARD_VERSION: &str = "X.Y";` in the crate root, mirrored into the README badge or status line.
- **`tree-sitter-cook`** (when CS-0002 lands) — set the claimed version in a header comment in `grammar.js`.
- **Each implementation's README** — state the claimed version in the project description.

These are not enforced by any automated check pre-1.0; they are a project discipline. When the Standard cuts a new version, each implementation is responsible for either updating its claim or accepting that it now lags the Standard by one version.

### Running the normative-keyword lint

````bash
cd standard && pnpm lint:keywords
````

The lint flags lowercase `must`/`shall`/`should`/`may` occurrences in normative chapters. Review each hit: either promote to all-caps (if the clause is meant to be binding) or reword (if the clause is descriptive).
