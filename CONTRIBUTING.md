# Contributing to Cook

## The Cook Standard

The Cookfile language is defined by the Cook Standard in [`standard/`](standard/). The Standard is the authoritative reference for the language. The Rust parser in `cli/crates/cook-lang/` is the current reference implementation; it is the de-facto authority for any Cookfile construct whose Standard chapter is presently a `NORMATIVE-TODO` stub.

### Spec-first rule

Any change that affects Cookfile surface syntax, execution semantics, the Cook Lua API, or the module system MUST:

1. Update `standard/` in the same commit that modifies the implementation.
2. Add one entry to `standard/src/content/docs/appendix/E-changes.mdx` with a new stable `CS-NNNN` ID, a one-line summary, the sections affected, and the commit reference.
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
- `tree-sitter-cook/grammar.js` — tree-sitter grammar (claims Cook Standard v0.2; see CS-0014)
- `tree-sitter-cook/src/**` — tree-sitter externals

If you add a new crate that contributes to language surface, update both this list and the hook.

### Conformance

- `cli/crates/cook-lang/tests/conformance.rs` walks `standard/conformance/` and asserts the Rust parser's behavior. Run it with `cargo test -p cook-lang --test conformance`.
- `tree-sitter-cook/scripts/conformance.mjs` walks the same corpus and asserts every positive parses cleanly and every syntactic negative produces an `ERROR`/`MISSING` node. Three negatives are semantic-only and recorded as accepted with a note. Run it with `node scripts/conformance.mjs` from `tree-sitter-cook/`, or `npm run conformance`. See CS-0014.

### `cook-lang` conformance workflow

The Rust parser claims a Cook Standard version via the `pub const COOK_STANDARD_VERSION: &str = "X.Y";` constant in `cli/crates/cook-lang/src/lib.rs`. This constant is the single source of truth; the README and `cook --version` output mirror it.

**Default harness mode.** `cargo test -p cook-lang --test conformance` walks `standard/conformance/` as it exists in the working tree. Every case must pass. When the parser falls behind a spec change, this gate goes red — that's the visible signal to catch up. There is no separate ledger of "pending CSes"; the failing harness is the ledger.

**Backwards-conformance mode.** `cook --set VERSION=X.Y standard.against-tag` materializes the corpus from the `cs-standard/<vX.Y>` git tag and runs the harness against that corpus. The recipe routes through `standard/cook_modules/checks.lua` (`checks.against_tag`). Use this to verify the parser still satisfies a previously-cut version during a brief catch-up window, or to bisect when a regression appeared.

**Bumping the claim.** When the parser catches up to a new cut, bump `COOK_STANDARD_VERSION` in `cli/crates/cook-lang/src/lib.rs` to match `standard/VERSION` in the same commit. Update the claim in `cli/crates/cook-lang/README.md`, the project root `README.md`, and `cli/crates/cook-lang/CONFORMANCE.md`'s "Pending CSes" section. The conformance harness should be green at that commit.

**Brief catch-up windows.** A spec-side commit may land conformance cases for a CS without simultaneously implementing the parser change, in which case the default harness will fail until catch-up. This is allowed. The backwards-conformance script can verify the parser still conforms to the previous cut during the window.

### Cutting a Cook Standard version

The Standard uses `MAJOR.MINOR` versioning pre-1.0 (see [`§ 0.5`](standard/src/content/docs/00-introduction.mdx)). A *cut* publishes a new MINOR by performing three actions in a single commit on `main`:

1. Bump `standard/VERSION` to the next MINOR (e.g. `0.2` → `0.3`).
2. Add a new entry to the top of the App. E **Versions** index in `standard/src/content/docs/appendix/E-changes.mdx`, listing the CSes the cut covers and the cut date.
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
- **When a parser change alters the conformance dump format (the `parse.txt` files), the `cs-standard/vX.Y` tag for the in-progress version MUST be force-moved to the merge commit where the parser and the corpus agree.** The `parse.txt` dumps are coupled to the parser's serialization; a tag whose corpus predates a format change cannot be verified by the current parser via `cook standard.against-tag`. (Example: CS-0011 dropped the `vars: []` line in v0.2; the `cs-standard/v0.2` tag was force-moved to the merge commit so the recipe keeps working. The `cs-standard/v0.1` tag was left in place as a frozen historical record — the script does not pass against it post-CS-0011, and that is documented in `cli/crates/cook-lang/CONFORMANCE.md`.) Pre-1.0 only; revisit before 1.0 by either making the dump format impl-agnostic or moving `parse.txt` out of the normative corpus.

### Implementation conformance claims

The Cook Standard does not normatively require an implementation to expose its claimed Standard version (see [`§ 0.7`](standard/src/content/docs/00-introduction.mdx)). As a project convention:

- **`cli/crates/cook-lang`** — set a `pub const COOK_STANDARD_VERSION: &str = "X.Y";` in the crate root, mirrored into the README badge or status line.
- **`tree-sitter-cook`** — set the claimed version in a header comment in `grammar.js`, mirrored in `package.json` and `tree-sitter.json`'s `version`/`description` fields. Currently claims v0.2 (see CS-0014).
- **Each implementation's README** — state the claimed version in the project description.

These are not enforced by any automated check pre-1.0; they are a project discipline. When the Standard cuts a new version, each implementation is responsible for either updating its claim or accepting that it now lags the Standard by one version.

### Running the normative-keyword lint

````bash
cook standard.lint
````

The lint routes through `standard/cook_modules/checks.lua` (`checks.lint_keywords`) and flags lowercase `must`/`shall`/`should`/`may` occurrences in normative chapters. Review each hit: either promote to all-caps (if the clause is meant to be binding) or reword (if the clause is descriptive).
