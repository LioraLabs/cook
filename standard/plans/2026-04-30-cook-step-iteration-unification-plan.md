# Cook Standard CS-0022 — Cook-step iteration unification Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Land CS-0022 — make the cook-step's output pattern the sole iteration source, collapse `using_clause` to two block forms (`using {…}` shell, `using >{…}` lua), remove the `using "cmd"` string form, replace bare path-accessors (`{stem}`, `{name}`, `{ext}`, `{dir}`) with the dotted `{in.ACCESSOR}` / `{out.ACCESSOR}` / `{out_N.ACCESSOR}` family, extend §5.5 cross-recipe substitution to shell blocks, and fix the multi-output Lua-block iteration wart so it follows the same single rule.

**Architecture:** Lockstep — the Standard, the Rust parser (`cli/crates/cook-lang`), the codegen (`cli/crates/cook-luagen`), the tree-sitter grammar (`tree-sitter-cook/`), the conformance corpus (`standard/conformance/`), the in-repo example Cookfiles, and the cook_modules library Lua all move together inside this CS. Implicit iteration is preserved end-to-end so the scheduler keeps producing one work unit per iteration item — that parallelism is the whole point of cook over a shell script.

**Tech Stack:** Rust (parser/codegen/engine), MDX/Astro Starlight (Standard), tree-sitter (grammar.js + C scanner), pnpm + vitest (Standard tests), bash (`scripts/check-normative-keywords.sh`), `cargo test -p cook-lang --test conformance` (corpus harness).

**Spec:** `standard/specs/2026-04-30-cook-step-iteration-unification-design.md` (commits `467b9ef` + `8803a0e`).

---

## Working directory and prerequisites

All paths are relative to `/home/alex/dev/cook` unless noted.

Confirm the spec-first hook is installed (one-time per clone):

```bash
git -C /home/alex/dev/cook config --get core.hooksPath
# Expected: .githooks
```

If empty: `git -C /home/alex/dev/cook config core.hooksPath .githooks`.

## Per-task verification commands

Each spec-side task ends with:

```bash
cd /home/alex/dev/cook/standard
pnpm build              # rehype-bare-ref-lint + rehype-clause-anchors gate slug consistency
pnpm test               # vitest plugin tests
pnpm lint:keywords      # RFC-2119 lowercase-keyword lint
```

Each parser/codegen task ends with:

```bash
cd /home/alex/dev/cook
cargo test -p cook-lang --test conformance   # corpus harness
cargo test -p cook-lang                       # unit + lexer + recipe tests
cargo test -p cook-luagen                     # codegen tests
cargo test                                    # full workspace
```

Each tree-sitter task ends with:

```bash
cd /home/alex/dev/cook/tree-sitter-cook
npm run generate
npm test
node scripts/conformance.mjs
```

Expected: clean exit (status 0).

## CS-ID assignment

| CS-ID | Subject | Source-of-rule |
|---|---|---|
| **CS-0022** | Cook-step iteration unification | Spec at `standard/specs/2026-04-30-cook-step-iteration-unification-design.md` |

Next free ID is `CS-0022` (CS-0021 was the v0.4 cut entry per `D-changes.mdx:271`).

---

## File structure

| File | Responsibility | Tasks |
|---|---|---|
| `standard/src/content/docs/04-recipes.mdx` | §4.5 (using_clause forms — drop the bare-string row), new §4.5.1 "Iteration mode" (single normative source), §4.6 (multi-output simplified). | 1 |
| `standard/src/content/docs/05-cross-recipe-references.mdx` | §5.5 surface list extended to include `shell_block`. §5.4 is preserved verbatim. | 2 |
| `standard/src/content/docs/06-cook-lua-api.mdx` | §6.7 rewritten — placeholder vocabulary table (no iteration logic), bare-accessor rejection, `{out_N}` and `{NAME.ACCESSOR}` forms. | 3 |
| `standard/src/content/docs/03-syntactic-grammar.mdx` | §3.8 step-dispatch table reflects two-form `using_clause` (no string variant). | 1 |
| `standard/src/content/docs/02-lexical.mdx` | No change — `{` and `>{` brace-block lexer rules already cover the two surviving forms. | — |
| `standard/src/content/docs/appendix/A-grammar.mdx` | Drop the `STRING` arm from `using_clause` (line 105). Drop the multi-output rejection paragraph at line 138. | 4 |
| `standard/src/content/docs/appendix/B-rationale.mdx` | New B.6.x covering: output-pattern as iteration source, removal of `using "cmd"`, removal of bare path-accessors, the `{lib.ACCESSOR}` firewall. | 5 |
| `standard/src/content/docs/appendix/D-changes.mdx` | New D.22 entry for CS-0022 with the parse-format-altering note. | 5, 17 |
| `cli/crates/cook-lang/src/ast.rs` | Drop `UsingClause::Shell(String)` variant. | 6 |
| `cli/crates/cook-lang/src/cook_line.rs` | Parser — drop the `STRING` acceptance branch in `using_clause` parsing (line 200 area); diagnostic names CS-0022 migration target. | 7 |
| `cli/crates/cook-lang/src/tests.rs` | Replace tests that exercise the string form; add tests for the migration diagnostic. | 8 |
| `cli/crates/cook-lang/tests/conformance.rs` | `repr_using_clause` formatter — drop `Shell(...)` arm. | 8 |
| `cli/crates/cook-luagen/src/cook_step.rs` | Rewrite `cook_step_mode` to read from output pattern only. Rewrite the `OneToOne`/`ManyToOne`/`BlockStep` arms; add shell-block placeholder substitution; drop the `Shell(cmd)` arms. | 9, 11, 12 |
| `cli/crates/cook-luagen/src/template.rs` | Extend the placeholder grammar — `{in}`, `{in.ACCESSOR}`, `{out}`, `{out.ACCESSOR}`, `{out_N}`, `{out_N.ACCESSOR}`, `{all}`, `{lib}`. Drop bare path-accessor recognition. New `validate_placeholders(body, mode, n_outputs)` pass. | 10, 11 |
| `cli/crates/cook-luagen/src/dep_ref.rs` | `parse_dep_token` accommodates dotted forms whose prefix isn't a recipe name (`{in.X}`, `{out.X}`, `{out_N.X}`); they are NOT dep refs. | 11 |
| `cli/crates/cook-luagen/src/recipe.rs` | `validate_accessor_placement` extended to enforce the §3.1 mode-coherence rule (mixed-driver multi-output rejected). | 9 |
| `tree-sitter-cook/grammar.js` | `using_clause` drops the `field("command", $.string)` arm. | 13 |
| `tree-sitter-cook/src/parser.c` | Regenerated. | 13 |
| `tree-sitter-cook/queries/highlights.scm` | Drop the `(using_clause command: (string) @string.special)` rule. | 13 |
| `tree-sitter-cook/queries/injections.scm` | Already has `(shell_block (shell_content) @injection.content (#set! injection.language "bash"))` from prior session work — verify still present. | 13 |
| `tree-sitter-cook/test/corpus/*.txt` | Strip the `using "cmd"` corpus cases; add cases for one-line shell block, multi-output `{out_N}`, `{in.ACCESSOR}` in body. | 14 |
| `standard/conformance/positive/*/Cookfile` + `parse.txt` | Migrate 8 fixtures from `using "cmd"` to `using {cmd}`; migrate bare `{stem}` to `{in.stem}`; regenerate `parse.txt`. | 15 |
| `standard/conformance/positive/` (new) | Fixtures: `022-shell-block-single-line`, `023-shell-block-multi-output-out-n`, `024-shell-block-cross-recipe-ref`, `025-many-to-one-lua-block`. | 16 |
| `standard/conformance/negative/` (new) | Fixtures: `017-bare-stem-rejected`, `018-using-string-form-rejected`, `019-out-n-in-single-output-rejected`, `020-out-bare-in-multi-output-rejected`, `021-mixed-driver-multi-output-rejected`, `022-lib-accessor-in-using-rejected`, `023-multi-output-one-to-one-mixed-rejected`. | 16 |
| `Cookfile`, `cli/Cookfile`, `tree-sitter-cook/Cookfile`, `standard/Cookfile`, `examples/*/Cookfile`, `cook_modules/*.lua`, `examples/*/cook_modules/*.lua`, `standard/cook_modules/*.lua` | Migrate `using "cmd"` → `using { cmd }`; migrate bare `{stem}`, `{name}`, `{ext}`, `{dir}` → `{in.X}`. | 17 |
| `standard/VERSION` | Untouched (CS-0022 is post-v0.4 pre-v0.5; no cut in this plan). | — |

Net: **17 tasks**.

---

# Phase A — Standard documentation (Tasks 1–5)

## Task 1: Spec — §4.5 `cook_step` single-output, new §4.5.1 "Iteration mode", §4.6 multi-output, §3.8 dispatch

**Files:**
- Modify: `standard/src/content/docs/04-recipes.mdx`
- Modify: `standard/src/content/docs/03-syntactic-grammar.mdx`

- [ ] **Step 1.1: Replace §4.5's "four forms" table with three forms**

In `standard/src/content/docs/04-recipes.mdx`, find §4.5 (currently around lines 160–204). Replace the four-row table (lines ~169–177) with:

```mdx
A conforming implementation MUST accept all three of the following forms:

| Form | `using_clause` | Semantic |
|---|---|---|
| `cook "out"`                    | absent                  | declaration-only: announces the output without a build command (§{grammar-appendix.steps}, "Declaration-only cook step"). The build command is expected to be registered by preceding Lua code or supplied by a later amendment. |
| `cook "out" using >{ ... }`     | `using_lua_block`       | execute-phase Lua block; produces the build effect, with `inputs` / `outputs` / `input` / `output` bindings per §{lua.using-block-globals}. |
| `cook "out" using { ... }`      | `shell_block`           | sequence of shell commands, one per non-blank line of the block (§{lexical.brace-blocks}, "Shell block line normalisation"). The block's textual placeholders (§{lua.shell-placeholders}) are substituted at register time. |
```

Then update the paragraph that follows (line ~178) — the "`using`-dispatch order" reference. Replace:

```
After the `using` keyword, the `using`-dispatch order of §{grammar-appendix.steps} ("`using` dispatch order") applies: `>{` is tested before `{`, and `{` before `"`. The first match wins.
```

with:

```
After the `using` keyword, the `using`-dispatch order of §{grammar-appendix.steps} ("`using` dispatch order") applies: `>{` is tested before `{`. The first match wins; the bare-string form is no longer admitted (CS-0022).
```

Update Example 4.5.1 (line ~182) — replace its `using "cmd"` examples with the block form:

```mdx
### Example 4.5.1

```cook
recipe lib
    ingredients "lib/*.c"
    cook "build/obj/{in.stem}.o" using { gcc -c {in} -o {out} }
    cook "build/libmath.a" using { ar rcs {out} {all} }
```

The first `cook` is a one-to-one mapping from each ingredient to one object file. The second is a many-to-one mapping producing a single archive from the collected set. Both use the inline (one-line) `shell_block` form of `using`.
```

Then update Note 4.5.1 (line ~204) — strip the `using "cmd"`-specific clause about `@`. The new wording:

```
The `@` character inside a `shell_block` body is preserved verbatim — it is part of the block's shell text, not an interactive-step marker. The interactive-step dispatch of §{recipes.shell-steps} applies only to a Content line's first non-whitespace character, not to text nested inside a `cook` step's `using` clause.
```

- [ ] **Step 1.2: Insert new §4.5.1 "Iteration mode"**

Immediately after Note 4.5.1 (and before §4.6 begins), insert:

```mdx
## 4.5.1. Iteration mode [#recipes.iteration-mode]
A `cook_step`'s **iteration mode** is determined by its output pattern list, before the `using` clause is consulted. Three modes are defined:

| Output pattern shape | Mode | Driver | Units produced |
|---|---|---|---|
| At least one output contains `{in.ACCESSOR}` (own-input accessor) | **One-to-one over own inputs** | The step's resolved `ingredients` list (§{recipes.ingredients}) | One per input |
| At least one output contains `{lib.ACCESSOR}` (dep-driven accessor, §{xref.dep-driven}) | **One-to-one over dep outputs** | `lib`'s output list (§{xref.dep-recipe-output}) | One per `lib` output |
| All outputs are literal (no accessor placeholders) | **Many-to-one** (the "one" refers to *one unit*, not one output) | None — one unit runs with the full input list visible | Exactly one |

A conforming implementation MUST reject, with a load-time diagnostic, a `cook_step` whose output patterns mix iteration sources — that is, where two outputs select different drivers (one bears `{in.ACCESSOR}` while another bears `{libmath.ACCESSOR}`, or one bears `{in.ACCESSOR}` while another is literal). All output patterns of a single `cook_step` MUST share one driver.

The iteration mode determines the number of work units produced; it is orthogonal to the `cook_step`'s declared output count. Each mode admits any number of declared outputs (one or more), as detailed in §{recipes.cook-multi-output}.

### Example 4.5.1.1

```cook
recipe build
    ingredients "src/*.c"
    cook "build/{in.stem}.o" using { gcc -c {in} -o {out} }    # one-to-one over own inputs
    cook "build/app" using { gcc {all} -o {out} }              # many-to-one (literal output)
```

### Note 4.5.1.1

The CS-0022 unification removes `{in}` from the using-clause's contents as a separate iteration trigger. Iteration mode is decided exclusively by the output pattern list. A using-clause that contains `{in}` while the output pattern list is fully literal is rejected at load time — see §{lua.shell-placeholders} for the diagnostic shape.
```

- [ ] **Step 1.3: Simplify §4.6 multi-output**

Find §4.6 (around lines 206–253). Replace the second paragraph (line ~213) — the one starting "When two or more outputs are present and a `using_clause` is given...":

with:

```
When two or more outputs are present and a `using_clause` is given, the `using_clause` is one of the two block forms (§{recipes.cook-single-output}): `using_lua_block` (`>{ ... }`) or `shell_block` (`{ ... }`). The bare-string form `cook "a" "b" using "cmd"` is not admitted at any output count (CS-0022, §{grammar-appendix.steps}).
```

Update Example 4.6.1 (line ~219) to use `{out_1}` / `{out_2}`:

```mdx
### Example 4.6.1

```cook
recipe generate
    ingredients "src/*.rs"
    cook "staging/out.js" "staging/out.wasm" using {
        ./fake-generator.sh
        mkdir -p staging
        cp pkg/out.js {out_1}
        cp pkg/out.wasm {out_2}
    }
```

One `cook` step declares two outputs produced together by a sequence of shell commands. The commands are the four non-blank lines of the shell block, in order (§{lexical.brace-blocks}). The placeholders `{out_1}` and `{out_2}` resolve to the unit's first and second declared outputs, in declaration order, per §{lua.shell-placeholders}.
```

Then update Note 4.6.1 (line ~252) — the diagnostic message changes since the rejected shape is gone:

```
The shape `cook "a" "b" using "cmd"` was rejected pre-CS-0022 with a "block body" diagnostic; under CS-0022 the bare-string form is gone entirely, so the diagnostic at this position now reads "the `using "cmd"` form was removed in CS-0022; rewrite as `using { cmd }`." See parser test `test_string_form_rejected_in_using`.
```

- [ ] **Step 1.4: Update §3.8 step-dispatch table**

In `standard/src/content/docs/03-syntactic-grammar.mdx`, find §3.8's `using_clause` row (search for "using_clause"). Update the cell describing accepted bodies:

Search:
```
`using` introduces a body that is one of: `using_lua_block` (`>{ … }`), `shell_block` (`{ … }`), or `STRING`.
```

Replace with:
```
`using` introduces a body that is one of: `using_lua_block` (`>{ … }`) or `shell_block` (`{ … }`). The bare-`STRING` form was removed in CS-0022.
```

- [ ] **Step 1.5: Verify**

```bash
cd /home/alex/dev/cook/standard
pnpm build
pnpm test
pnpm lint:keywords
```

Expected: clean exit. (The `rehype-clause-anchors` plugin checks that every `§{...}` reference resolves; the new `recipes.iteration-mode` slug is registered automatically because it's a heading anchor.)

- [ ] **Step 1.6: Commit**

```bash
git add standard/src/content/docs/04-recipes.mdx standard/src/content/docs/03-syntactic-grammar.mdx
git commit -m "spec(CS-0022): §4.5/§4.6/§3.8 collapse using_clause to two block forms; add §4.5.1 iteration mode"
```

---

## Task 2: Spec — §5.5 surface extension to shell blocks

**Files:**
- Modify: `standard/src/content/docs/05-cross-recipe-references.mdx`

- [ ] **Step 2.1: Extend §5.5 surface list**

In `standard/src/content/docs/05-cross-recipe-references.mdx`, find §5.5 (around line 86–104). Replace the rule paragraph (line ~87):

```
Rule. A `{NAME}` bare reference in a `cook` `using`-string, `plate` command, `test` command, or bare shell MUST be substituted by the space-joined concatenation of the named recipe's output list (§{xref.dep-recipe-output}).
```

with:

```
Rule. A `{NAME}` bare reference in a `cook` `using` `shell_block` body, a `plate` command, a `test` command, or a bare `shell_command` MUST be substituted by the space-joined concatenation of the named recipe's output list (§{xref.dep-recipe-output}). A `{NAME}` reference inside a `using_lua_block` is parsed as Lua syntax (a single-element table containing the local variable `NAME`); cross-recipe references in Lua bodies use `cook.dep_output(NAME)` and `cook.dep_output_list(NAME)` per §{lua.cook-table}.
```

- [ ] **Step 2.2: Update §5.5.1 example**

Find Example 5.5.1 (line ~92). Update its using-string to the shell-block form:

```cook
recipe libmath
    ingredients "src/math/*.c"
    cook "build/obj/math/{in.stem}.o" using { gcc -c {in} -o {out} }
    cook "build/lib/libmath.a" using { ar rcs {out} {all} }

recipe app
    cook "build/bin/app" using { gcc -o {out} main.c {libmath} }
```

The example's prose (line ~104) is already correct ("expands at register time to `\"build/lib/libmath.a\"`"); leave it unchanged.

- [ ] **Step 2.3: Verify**

```bash
cd /home/alex/dev/cook/standard
pnpm build && pnpm test && pnpm lint:keywords
```

- [ ] **Step 2.4: Commit**

```bash
git add standard/src/content/docs/05-cross-recipe-references.mdx
git commit -m "spec(CS-0022): §5.5 surface list extends to shell_block; Lua bodies use cook.dep_output"
```

---

## Task 3: Spec — §6.7 rewrite (placeholder vocabulary)

**Files:**
- Modify: `standard/src/content/docs/06-cook-lua-api.mdx`

- [ ] **Step 3.1: Replace §6.7 wholesale**

In `standard/src/content/docs/06-cook-lua-api.mdx`, find §6.7 (lines ~273–318). Replace the entire section through the end of "Note 6.7.1" with:

```mdx
## 6.7. Placeholder substitutions in shell text [#lua.shell-placeholders]
**Phase.** Substitution is performed by the code generator at register time; the resulting Lua expression is evaluated at register time when the unit is recorded.

A `cook` step's `using { ... }` shell-block body, every `plate` command, every `test` command, and every bare `shell_command` is a **shell text** for the purposes of this section. Before a conforming implementation records the resulting unit, it MUST substitute the placeholder set defined in this section. The set is closed: a `{TOKEN}` whose shape does not match any row of the placeholder table below resolves through §{xref.resolution}.

| Placeholder | Valid in iteration mode | Meaning |
|---|---|---|
| `{in}` | one-to-one (own or dep-driven) | The current iteration item (path) |
| `{in.ACCESSOR}` | one-to-one | `path.ACCESSOR(in)` per §{lua.path-helpers} |
| `{out}` | any mode, single declared output | The unit's single output path |
| `{out.ACCESSOR}` | any mode, single declared output | `path.ACCESSOR(out)` |
| `{out_N}` (`N` ∈ 1..) | any mode, multi-output | The unit's `N`-th declared output, in declaration order |
| `{out_N.ACCESSOR}` | any mode, multi-output | `path.ACCESSOR(out_N)` |
| `{all}` | many-to-one only | The unit's input list, joined by single space characters |

`ACCESSOR` is one of `stem`, `name`, `ext`, `dir`, with semantics defined by §{xref.path-accessors}.

A conforming implementation MUST reject the following shapes with a load-time diagnostic:

- `{in}` or `{in.ACCESSOR}` in a many-to-one step (no current iteration item exists).
- `{all}` in a one-to-one step.
- `{out}` in a multi-output step (ambiguous — use `{out_N}`).
- `{out_N}` in a single-output step (use plain `{out}`).
- `{out_N}` for `N` greater than the step's declared output count.
- `{stem}`, `{name}`, `{ext}`, or `{dir}` as a bare token (i.e. with no `NAME.` prefix). These were the pre-CS-0022 spelling for the current-iteration accessors and are no longer admitted at the placeholder layer; the current spelling is `{in.stem}` etc. The diagnostic MUST name the new form. (A bare `{stem}` would otherwise resolve through §{xref.resolution} step 4 to `cook.env["stem"]`, which is a confusing failure mode for code being migrated forward from a v0.4 surface.)

### Example 6.7.1

```cook
cook "build/{in.stem}.o" using { gcc -c {in} -o {out} }
```

with ingredient `main.c` records one shell unit per ingredient. For ingredient `main.c`, the unit's `command` field is `set -e\ngcc -c main.c -o build/main.o`. The output pattern's `{in.stem}` resolves to `main` before the body's `{out}` is substituted; the resulting `{out}` is `build/main.o`.

### Example 6.7.2

```cook
cook "out.js" "out.wasm" using {
    wasm-pack build
    cp pkg/main.js {out_1}
    cp pkg/main.wasm {out_2}
}
```

with any input list records one shell unit (many-to-one mode — outputs are literal). The body's `{out_1}` and `{out_2}` resolve to `out.js` and `out.wasm` respectively.

### Note 6.7.1

The block forms (`using { ... }` and `using >{ ... }`) and the placeholder layer compose differently. A `shell_block` is shell text and gets the substitution defined in this section. A `using_lua_block` is Lua text and does not get textual substitution; the same iteration data is exposed as Lua bindings per §{lua.using-block-globals}. The asymmetry is principled — see App. B.6.1 — and reflects that shell has no binding mechanism for "the current file" while Lua does.

### Note 6.7.2

Cross-recipe references inside a shell block follow §{xref.string-substitution}: a bare `{NAME}` substitutes to the named recipe's full output list, joined by space. The `{lib.ACCESSOR}` form is rejected inside any using-clause body, including a shell block, because it would smuggle a second iteration source past the output pattern's declaration. Authors who want a recipe's per-element accessors inside a step driven by something else use Lua (`path.stem(cook.dep_output_list("lib")[1])`).
```

- [ ] **Step 3.2: Verify**

```bash
cd /home/alex/dev/cook/standard
pnpm build && pnpm test && pnpm lint:keywords
```

Expected: clean exit. The `lib.shell-placeholders` slug name is preserved (the content under it is rewritten); existing cross-references continue to resolve.

- [ ] **Step 3.3: Commit**

```bash
git add standard/src/content/docs/06-cook-lua-api.mdx
git commit -m "spec(CS-0022): §6.7 rewrite — placeholder vocabulary, no iteration logic, {out_N} + {NAME.ACCESSOR} forms"
```

---

## Task 4: Spec — App. A grammar

**Files:**
- Modify: `standard/src/content/docs/appendix/A-grammar.mdx`

- [ ] **Step 4.1: Update `using_clause` production**

Line 105 currently reads:

```
using_clause          ::= "using" ( using_lua_block | shell_block | STRING )
```

Replace with:

```
using_clause          ::= "using" ( using_lua_block | shell_block )
```

- [ ] **Step 4.2: Drop the multi-output rejection paragraph at line 138**

Line 138 currently reads:

```
**Multi-output cook-step rule.** When `cook_step` has two or more `STRING` output patterns before `using`, the `using_clause` MUST use `using_lua_block` or `shell_block`. A conforming implementation MUST reject the form `cook "a" "b" using STRING`.
```

Replace with:

```
**Multi-output cook-step rule.** When `cook_step` has two or more `STRING` output patterns before `using`, the `using_clause` resolves per the unrestricted production above — the bare-`STRING` arm having been removed in CS-0022, the form `cook "a" "b" using STRING` is rejected by the production itself rather than by a separate well-formedness rule.
```

- [ ] **Step 4.3: Add the iteration-mode coherence rule paragraph**

Immediately after the paragraph from Step 4.2, insert:

```
**Iteration coherence (CS-0022).** All `STRING` output patterns of a single `cook_step` MUST share an iteration source: every output is literal (no accessor placeholder), or every output bears `{in.ACCESSOR}` (own-input driver), or every output bears `{lib.ACCESSOR}` for the same recipe `lib` (dep-driven driver). A conforming implementation MUST reject a `cook_step` whose output patterns mix sources, with a diagnostic naming the offending outputs. See §{recipes.iteration-mode}.
```

- [ ] **Step 4.4: Verify**

```bash
cd /home/alex/dev/cook/standard
pnpm build && pnpm test && pnpm lint:keywords
```

- [ ] **Step 4.5: Commit**

```bash
git add standard/src/content/docs/appendix/A-grammar.mdx
git commit -m "spec(CS-0022): App. A — drop STRING from using_clause; add iteration-coherence rule"
```

---

## Task 5: Spec — App. B rationale; App. D entry

**Files:**
- Modify: `standard/src/content/docs/appendix/B-rationale.mdx`
- Modify: `standard/src/content/docs/appendix/D-changes.mdx`

- [ ] **Step 5.1: Append §B.6.5 (CS-0022 rationale)**

In `standard/src/content/docs/appendix/B-rationale.mdx`, append a new subsection after §B.6.4 (the last B.6.* subsection currently). Insert:

```mdx
### B.6.5. Why output-pattern is the sole iteration source [#rationale.output-pattern-iteration]
Pre-CS-0022, iteration mode was decided by two non-adjacent regions: §6.7 said "{in} in the using-string drives one-to-one"; §5.4 said "{lib.ACCESSOR} in the output pattern drives dep-driven one-to-one." Both rules co-existed peacefully when authors wrote consistent code, but they failed silently on inconsistent code: `cook "out" using "do {in}"` (literal output, `{in}` in using-string) iterated per input under the §6.7 rule, with every iteration writing to the same literal output, clobbering successive results.

CS-0022 makes the output pattern the sole iteration source. The output pattern is the *declarative* part of a cook step — it describes what files the step produces. If those filenames are parameterized (via `{in.ACCESSOR}` or `{lib.ACCESSOR}`), iteration follows. If they are literal, the step does not iterate. Authors who wrote the silent-clobber pattern get a load-time error pointing at the literal-output / `{in}`-in-body mismatch.

### B.6.6. Why `using "cmd"` was removed [#rationale.using-cmd-removed]
The string and block forms expressed the same thing — a shell command — through different surfaces. With shell blocks first-class (CS-0022 also extends §{xref.string-substitution} and §{lua.shell-placeholders} to cover `shell_block` content) the string form bought one character of brevity per call site against the cost of a separate normative branch in §6.7 and a separate dispatch arm in `using_clause`. The cost was paid every time an author moved a one-line command onto two lines. The Standard now has one shell surface — `using { cmd }` for one-liners; `using { cmd1\n cmd2 }` for multi-line — and one Lua surface, `using >{ ... }`.

### B.6.7. Why bare path-accessors were removed [#rationale.bare-accessors-removed]
Two spellings of the same concept (`{stem}` and `{in.stem}`) with position-dependent admissibility (`{stem}` valid in output patterns, `{in.stem}` valid in using-clause bodies under earlier proposals) is the kind of memorise-by-rote rule a Standard pays for forever. CS-0022 collapses the surface to one spelling — `{NAME.ACCESSOR}` — valid in both positions, with `NAME` ∈ `{ in, out, out_N, libname }`. Bare `{stem}` reads, post-CS-0022, as a `{TOKEN}` resolving through §{xref.resolution} step 4 to `cook.env["stem"]`. That fallthrough is the wrong failure mode for code being migrated, so the implementation MUST emit a specific diagnostic for the four bare accessor names that points at the new spelling.

### B.6.8. Why `{lib.ACCESSOR}` is rejected in using-clause bodies [#rationale.lib-accessor-firewall]
The §{xref.dep-driven} firewall rejecting `{lib.ACCESSOR}` in a step whose output pattern does not declare `lib` as the driver is preserved verbatim by CS-0022. Inside a step where `lib` *is* the driver, the spelling `{in.ACCESSOR}` is used — the local name `in` for "the current iteration item" is universal across drivers, so the using-clause body never names the driver explicitly. Allowing `{lib.ACCESSOR}` in the body would let an author smuggle a second iteration source past the output-pattern declaration, undoing §{recipes.iteration-mode}'s single-driver invariant.
```

- [ ] **Step 5.2: Update §B.6.1 wording**

§B.6.1 currently contrasts "shell using-strings" with "Lua using-blocks." CS-0022 replaces "shell using-string" with "shell block." Find the §B.6.1 paragraph beginning "§{lua.shell-placeholders} defines `{in}`, `{out}`, `{all}`, `{stem}`, etc., for shell using-strings." (around B-rationale.mdx line 258).

Replace the first paragraph of B.6.1 with:

```
§{lua.shell-placeholders} defines `{in}`, `{out}`, `{all}`, `{out_N}`, and the dotted-accessor `{NAME.ACCESSOR}` family for shell blocks (and, by §{xref.string-substitution}, `plate` / `test` / bare `shell_command`). §{lua.using-block-globals} defines `input`, `output`, `inputs`, `outputs`, and `input_N` for `using >{ ... }` Lua-code blocks. Both express the same fundamental idea — "the current input and output of a step" — and an earlier draft of the Standard considered collapsing them to one.
```

The remaining two paragraphs of B.6.1 stay as written; their argument (shell has no binding mechanism, Lua does, so the asymmetry is principled) is unchanged.

- [ ] **Step 5.3: Add D.22 entry**

In `standard/src/content/docs/appendix/D-changes.mdx`, after the D.21 entry (the v0.4 cut), insert:

```mdx
## D.22. CS-0022 — Cook-step iteration unification: output-pattern as sole driver, `using "cmd"` removed, `{in.ACCESSOR}` family. [#changes.cs-0022]

**Sections affected:** §{recipes.cook-single-output} (table reduced to three forms); new §{recipes.iteration-mode}; §{recipes.cook-multi-output} (multi-output rule simplified); §{xref.string-substitution} (surface list extended to `shell_block`); §{lua.shell-placeholders} (rewritten — placeholder vocabulary, no iteration logic); §{grammar-appendix.steps} ("`using` dispatch order", "Multi-output cook-step rule", "Iteration coherence"); App. B.6.1 (wording), B.6.5–B.6.8 (new rationale).

**Summary:** The cook step's iteration mode (one-to-one over own inputs, one-to-one over dep outputs, or many-to-one) is decided exclusively by the output pattern list. The using-clause's body ceases to be an iteration trigger. The `using "cmd"` string form is removed; `using { cmd }` (a one-line `shell_block`) replaces it. The bare path-accessors `{stem}`, `{name}`, `{ext}`, `{dir}` are removed from the placeholder vocabulary in favour of the dotted family `{in.ACCESSOR}`, `{out.ACCESSOR}`, `{out_N.ACCESSOR}`. Multi-output indexing uses `{out_N}` (1-indexed). Cross-recipe references (§{xref.string-substitution}) extend to `shell_block` content; `using_lua_block` continues to use `cook.dep_output(NAME)` per B.6.1. The §{xref.dep-driven} firewall rejecting `{lib.ACCESSOR}` in non-driving using-clause bodies is preserved.

**Why:** The pre-CS-0022 surface had two non-adjacent normative sources for iteration mode (one in §6.7, one in §5.4) that overlapped, with a silent footgun when an author put `{in}` in a using-string while the output pattern was literal — successive iterations clobbered the same output. The string form `using "cmd"` and the block form `using { cmd }` expressed the same idea through two surfaces, and the placeholder layer was specified for the former only. Bare path-accessors and dotted accessors meant the same thing in different positions. CS-0022 collapses the surface: one rule for iteration (the output pattern), one shell surface (the block), one accessor spelling (dotted).

**Conformance impact.** `parse.txt` files in `standard/conformance/` change format only at the AST level: `Step::Cook` units with a using-clause now dump the inner shape as `ShellBlock([...])` or `LuaBlock(...)` exclusively (the former `Shell("...")` arm is gone). Per CONTRIBUTING.md's rule on parse-format-altering changes, the implementation lands lockstep with the corpus migration in this CS — the corpus and the parser agree at the merge commit.
```

- [ ] **Step 5.4: Verify**

```bash
cd /home/alex/dev/cook/standard
pnpm build && pnpm test && pnpm lint:keywords
```

- [ ] **Step 5.5: Commit**

```bash
git add standard/src/content/docs/appendix/B-rationale.mdx standard/src/content/docs/appendix/D-changes.mdx
git commit -m "spec(CS-0022): App. B.6.5–6.8 rationale; App. D.22 entry"
```

---

# Phase B — Rust parser AST cleanup (Tasks 6–8)

## Task 6: AST — drop `UsingClause::Shell(String)` variant

**Files:**
- Modify: `cli/crates/cook-lang/src/ast.rs`

- [ ] **Step 6.1: Read current AST shape**

```bash
grep -n "UsingClause\|Shell\|ShellBlock\|LuaBlock" cli/crates/cook-lang/src/ast.rs
```

Confirm `UsingClause` enum has three variants (around `ast.rs:50-55`):

```rust
pub enum UsingClause {
    Shell(String),
    ShellBlock(Vec<String>),
    LuaBlock(String),
}
```

- [ ] **Step 6.2: Remove the `Shell(String)` variant**

Edit `cli/crates/cook-lang/src/ast.rs`, lines around 50–55:

```rust
pub enum UsingClause {
    ShellBlock(Vec<String>),
    LuaBlock(String),
}
```

- [ ] **Step 6.3: Run `cargo check -p cook-lang` to enumerate compile errors**

```bash
cargo check -p cook-lang
```

Expected: compile errors at every site that pattern-matches on `UsingClause::Shell(...)` or constructs one. Note the file:line of each. Likely sites:

- `cli/crates/cook-lang/src/cook_line.rs` — parser construction (Task 7).
- `cli/crates/cook-lang/src/tests.rs` — test assertions on the variant.
- `cli/crates/cook-lang/tests/conformance.rs` — `repr_using_clause` formatter.
- `cli/crates/cook-luagen/src/cook_step.rs` — codegen match arms.
- `cli/crates/cook-luagen/src/dep_ref.rs` — dep extraction match arms.

Capture the list — Tasks 7, 8, 9, 11 each address a subset.

- [ ] **Step 6.4: Commit (intentionally broken — Task 7 makes it compile)**

For a subagent-driven workflow, this commit lands as part of Task 7's commit. For inline execution, defer the commit to the end of Task 7. Either way, do not push between Tasks 6 and 7.

---

## Task 7: Parser — drop the `STRING` acceptance branch

**Files:**
- Modify: `cli/crates/cook-lang/src/cook_line.rs`

- [ ] **Step 7.1: Locate the using-clause dispatch**

```bash
grep -n "using\|UsingClause" cli/crates/cook-lang/src/cook_line.rs | head -30
```

Find the dispatch around `cook_line.rs:165–205`. The current dispatch tries `>{` (LuaBlock), then `{` (ShellBlock), then `STRING` (Shell), then errors.

- [ ] **Step 7.2: Replace the `STRING` arm with a diagnostic**

Find the arm matching the bare-string form (around `cook_line.rs:185–199`). The current shape is similar to:

```rust
} else if let Token::String(cmd) = ... {
    // accept STRING form
    Some(UsingClause::Shell(cmd))
} else {
    return Err(ParseError::Parse {
        line,
        message: format!("cook using: expected quoted command, >{{ Lua block, or {{ shell block, found: {}", ...),
    });
}
```

Replace the body of the `String` arm so the parser emits a CS-0022-aware diagnostic instead of accepting the form:

```rust
} else if let Token::String(_) = ... {
    return Err(ParseError::Parse {
        line,
        message: format!(
            "cook using: the bare-string form `using \"cmd\"` was removed in CS-0022; \
             rewrite as `using {{ cmd }}` (one-line shell block)"
        ),
    });
} else {
    return Err(ParseError::Parse {
        line,
        message: format!("cook using: expected `>{{ Lua block }}` or `{{ shell block }}`, found: {}", ...),
    });
}
```

- [ ] **Step 7.3: Verify compile**

```bash
cargo check -p cook-lang
```

Expected: `cook-lang` compiles. (Downstream `cook-luagen` and the test crate may still fail — those are Tasks 8 and 9.)

- [ ] **Step 7.4: Commit (along with Task 6)**

```bash
git add cli/crates/cook-lang/src/ast.rs cli/crates/cook-lang/src/cook_line.rs
git commit -m "parser(CS-0022): drop UsingClause::Shell variant; emit migration diagnostic for `using \"cmd\"`"
```

---

## Task 8: Parser tests — replace string-form tests; add diagnostic test; update conformance formatter

**Files:**
- Modify: `cli/crates/cook-lang/src/tests.rs`
- Modify: `cli/crates/cook-lang/tests/conformance.rs`

- [ ] **Step 8.1: Update `tests/conformance.rs`'s `repr_using_clause`**

Find the function (around `conformance.rs:65-72`):

```rust
fn repr_using_clause(uc: &UsingClause) -> String {
    match uc {
        UsingClause::Shell(s) => format!("Shell({:?})", s),
        UsingClause::ShellBlock(xs) => format!("ShellBlock({})", repr_list(xs)),
        UsingClause::LuaBlock(s) => format!("LuaBlock({:?})", s),
    }
}
```

Replace with:

```rust
fn repr_using_clause(uc: &UsingClause) -> String {
    match uc {
        UsingClause::ShellBlock(xs) => format!("ShellBlock({})", repr_list(xs)),
        UsingClause::LuaBlock(s) => format!("LuaBlock({:?})", s),
    }
}
```

- [ ] **Step 8.2: Locate and update unit tests in `src/tests.rs`**

```bash
grep -n "UsingClause::Shell\b\|using \"" cli/crates/cook-lang/src/tests.rs
```

For each test that constructs or asserts on `UsingClause::Shell(...)`, rewrite the assertion target as `UsingClause::ShellBlock(vec![...])` and update the input Cookfile string from `using "cmd"` to `using { cmd }`.

Concrete: tests typically named `test_cook_step_shell`, `test_at_in_cook_using_is_not_interactive`, `test_cook_using_string_form`, `test_cook_using_with_placeholders`. For each:

1. Change the input Cookfile string `using "..."` → `using { ... }`.
2. Change the assertion `Some(UsingClause::Shell("..."))` → `Some(UsingClause::ShellBlock(vec!["...".to_string()]))`.

If a test's *purpose* was to assert the string form's existence (e.g., a name like `test_string_form_admitted`), delete the test outright and replace with a new test asserting the rejection diagnostic from Task 7.

- [ ] **Step 8.3: Add the migration-diagnostic test**

Append to `cli/crates/cook-lang/src/tests.rs`:

```rust
#[test]
fn test_using_string_form_rejected_with_migration_diagnostic() {
    let src = r#"recipe build
    cook "out" using "echo hi"
"#;
    let err = parse(src).expect_err("CS-0022: bare-string using form must be rejected");
    match err {
        ParseError::Parse { message, .. } => {
            assert!(message.contains("CS-0022"), "diagnostic should name CS-0022, got: {message}");
            assert!(message.contains("using {"), "diagnostic should name the new form, got: {message}");
        }
        e => panic!("expected ParseError::Parse, got {:?}", e),
    }
}
```

- [ ] **Step 8.4: Run cargo test**

```bash
cd /home/alex/dev/cook
cargo test -p cook-lang
```

Expected: PASS for the new diagnostic test; PASS for the rewritten unit tests; the `conformance` test target may still fail (corpus not yet migrated — Task 15).

- [ ] **Step 8.5: Commit**

```bash
git add cli/crates/cook-lang/src/tests.rs cli/crates/cook-lang/tests/conformance.rs
git commit -m "parser(CS-0022): tests — rewrite string-form tests; add migration-diagnostic test; update conformance formatter"
```

---

# Phase C — Rust codegen (Tasks 9–12)

## Task 9: Codegen — rewrite `cook_step_mode` to read from output pattern

**Files:**
- Modify: `cli/crates/cook-luagen/src/cook_step.rs`
- Modify: `cli/crates/cook-luagen/src/recipe.rs`

- [ ] **Step 9.1: Write the failing test for output-pattern-driven mode**

Append to `cli/crates/cook-luagen/src/cook_step.rs` (or to its `#[cfg(test)] mod tests` block, creating one if absent):

```rust
#[cfg(test)]
mod cs_0022_mode_tests {
    use super::*;
    use cook_lang::ast::*;

    fn step(outputs: &[&str], using_clause: Option<UsingClause>) -> CookStep {
        CookStep {
            outputs: outputs.iter().map(|s| s.to_string()).collect(),
            using_clause,
        }
    }

    #[test]
    fn literal_output_is_many_to_one_regardless_of_body() {
        let s = step(&["build/app"], Some(UsingClause::ShellBlock(vec!["gcc {in}".into()])));
        assert!(matches!(cook_step_mode(&s), CookMode::ManyToOne));
    }

    #[test]
    fn in_accessor_output_is_one_to_one() {
        let s = step(&["build/{in.stem}.o"], Some(UsingClause::ShellBlock(vec!["gcc {in} -o {out}".into()])));
        assert!(matches!(cook_step_mode(&s), CookMode::OneToOne));
    }

    #[test]
    fn lib_accessor_output_is_one_to_one_dep_driven() {
        let s = step(&["build/{libmath.stem}.x"], Some(UsingClause::ShellBlock(vec!["echo {in}".into()])));
        assert!(matches!(cook_step_mode(&s), CookMode::OneToOne));
    }

    #[test]
    fn multi_output_literal_is_block_step_many_to_one() {
        let s = step(&["a.js", "a.wasm"], Some(UsingClause::ShellBlock(vec!["gen".into()])));
        // Per spec §3.1, multi-output literal is many-to-one; codegen routes through BlockStep
        // for multi-output cases regardless. Both BlockStep and ManyToOne are accepted by
        // this assertion family — the right-shape rule is in cs_0022_mode_tests below.
        assert!(matches!(cook_step_mode(&s), CookMode::BlockStep | CookMode::ManyToOne));
    }

    #[test]
    fn declaration_only_no_using_clause() {
        let s = step(&["x"], None);
        assert!(matches!(cook_step_mode(&s), CookMode::DeclarationOnly));
    }
}
```

- [ ] **Step 9.2: Run tests (expect failure)**

```bash
cd /home/alex/dev/cook
cargo test -p cook-luagen --lib cs_0022_mode_tests
```

Expected: FAIL on at least the `literal_output_is_many_to_one_regardless_of_body` test (today's `cook_step_mode` returns `OneToOne` because the body contains `{in}`).

- [ ] **Step 9.3: Rewrite `cook_step_mode`**

In `cli/crates/cook-luagen/src/cook_step.rs:27-41`, replace the function body:

```rust
pub(crate) fn cook_step_mode(step: &CookStep) -> CookMode {
    use crate::template::output_pattern_kind;

    if step.using_clause.is_none() {
        return CookMode::DeclarationOnly;
    }

    // Multi-output blocks always route through BlockStep — codegen for them
    // emits a single cook.add_unit with the full inputs/outputs arrays, regardless
    // of mode. Iteration (when applicable) is implicit in the inputs list.
    if step.outputs.len() > 1 {
        return CookMode::BlockStep;
    }

    // Single-output: the output pattern decides iteration.
    match output_pattern_kind(&step.outputs[0]) {
        OutputPatternKind::OwnInputAccessor | OutputPatternKind::DepDriven { .. } => CookMode::OneToOne,
        OutputPatternKind::Literal => CookMode::ManyToOne,
    }
}
```

(`output_pattern_kind` and the `OutputPatternKind` enum may need new variants — implement those in Task 10.)

- [ ] **Step 9.4: Add `validate_accessor_placement` mode-coherence check**

In `cli/crates/cook-luagen/src/recipe.rs`, find the existing `validate_accessor_placement` function. Add the multi-output coherence rule:

```rust
fn check_multi_output_coherence(step: &CookStep, recipe_names: &BTreeSet<String>) -> Result<(), String> {
    if step.outputs.len() < 2 {
        return Ok(());
    }

    // Every output pattern must agree on the driver: all literal, or all bear
    // {in.X}, or all bear {libname.X} for the same `libname`.
    use crate::template::output_pattern_kind_with_recipes;
    let first = output_pattern_kind_with_recipes(&step.outputs[0], recipe_names);
    for (idx, out) in step.outputs.iter().enumerate().skip(1) {
        let kind = output_pattern_kind_with_recipes(out, recipe_names);
        if !drivers_match(&first, &kind) {
            return Err(format!(
                "CS-0022: cook step's output #1 ({:?}) and output #{} ({:?}) declare \
                 different iteration drivers; all output patterns must share a driver",
                step.outputs[0], idx + 1, out
            ));
        }
    }
    Ok(())
}

fn drivers_match(a: &OutputPatternKind, b: &OutputPatternKind) -> bool {
    use OutputPatternKind::*;
    match (a, b) {
        (Literal, Literal) => true,
        (OwnInputAccessor, OwnInputAccessor) => true,
        (DepDriven { dep_name: n1, .. }, DepDriven { dep_name: n2, .. }) => n1 == n2,
        _ => false,
    }
}
```

Wire `check_multi_output_coherence` into the existing accessor-validation pass.

- [ ] **Step 9.5: Run tests**

```bash
cargo test -p cook-luagen
```

Expected: the new `cs_0022_mode_tests` PASS; existing codegen tests may still fail (they exercise the now-removed `Shell(...)` arm — Task 11 fixes those).

- [ ] **Step 9.6: Commit**

```bash
git add cli/crates/cook-luagen/src/cook_step.rs cli/crates/cook-luagen/src/recipe.rs
git commit -m "codegen(CS-0022): cook_step_mode reads from output pattern; multi-output coherence check"
```

---

## Task 10: Codegen — placeholder vocabulary in `template.rs`

**Files:**
- Modify: `cli/crates/cook-luagen/src/template.rs`
- Modify: `cli/crates/cook-luagen/src/dep_ref.rs`

- [ ] **Step 10.1: Add `OutputPatternKind::OwnInputAccessor`**

In `cli/crates/cook-luagen/src/template.rs`, find the `OutputPatternKind` enum. Add a new variant:

```rust
pub enum OutputPatternKind {
    Literal,
    OwnInputAccessor,                                        // contains {in.ACCESSOR}
    DepDriven { dep_name: String, accessor: String, lua_expr: String },
}
```

(Drop the pre-CS-0022 `OwnInputs(String)` if it exists — that variant covered bare `{stem}` etc.; Task 17 migrates surface code so the bare form is gone.)

- [ ] **Step 10.2: Add `output_pattern_kind` and `output_pattern_kind_with_recipes`**

In `cli/crates/cook-luagen/src/template.rs`, define:

```rust
pub fn output_pattern_kind(pattern: &str) -> OutputPatternKind {
    if pattern.contains("{in.") {
        return OutputPatternKind::OwnInputAccessor;
    }
    if let Some((dep, accessor)) = first_dep_accessor(pattern, &Default::default()) {
        return OutputPatternKind::DepDriven {
            dep_name: dep,
            accessor,
            lua_expr: String::new(), // computed below per call site
        };
    }
    OutputPatternKind::Literal
}

pub fn output_pattern_kind_with_recipes(
    pattern: &str,
    recipe_names: &BTreeSet<String>,
) -> OutputPatternKind {
    if pattern.contains("{in.") {
        return OutputPatternKind::OwnInputAccessor;
    }
    if let Some((dep, accessor)) = first_dep_accessor(pattern, recipe_names) {
        let lua_expr = build_dep_lua_expr(&dep, &accessor);
        return OutputPatternKind::DepDriven { dep_name: dep, accessor, lua_expr };
    }
    OutputPatternKind::Literal
}
```

`first_dep_accessor(pattern, recipe_names)` walks the pattern's `{TOKEN.SUFFIX}` placeholders, returns the first whose `TOKEN` is in `recipe_names` and `SUFFIX` is one of `{stem, name, ext, dir}`. Existing helpers (`analyze_output_pattern`, in the same file) can be refactored to share this walker.

- [ ] **Step 10.3: Extend `expand_template_to_lua_with_deps` for the new placeholder family**

In `cli/crates/cook-luagen/src/template.rs`, find `expand_template_to_lua_with_deps` (the function called from `cook_step.rs:98` and elsewhere). It currently handles bare `{in}`, `{out}`, `{stem}`, `{name}`, `{ext}`, `{dir}`, `{all}`, plus `{libname}` and `{libname.ACCESSOR}`.

Rewrite the placeholder-recognition arm so the supported family is:

| Token shape | Lowering |
|---|---|
| `{in}` | reference to `_cook_in` |
| `{in.stem}` / `{in.name}` / `{in.ext}` / `{in.dir}` | `path.stem(_cook_in)` etc. |
| `{out}` | reference to `_cook_out` |
| `{out.stem}` / `{out.name}` / `{out.ext}` / `{out.dir}` | `path.stem(_cook_out)` etc. |
| `{out_N}` (N ∈ 1..) | reference to `_cook_outs[N]` (Lua 1-indexed) |
| `{out_N.stem}` etc. | `path.stem(_cook_outs[N])` etc. |
| `{all}` | reference to `_cook_all` (the space-joined input list) |
| `{libname}` | `cook.dep_output("libname")` |
| `{libname.stem}` / etc. | (in OUTPUT pattern only; inside using-clause body — REJECTED, Task 11) |
| `{TOKEN}` (anything else) | `cook.env["TOKEN"]` per §5.2 step 4 |

Drop the bare `{stem}`, `{name}`, `{ext}`, `{dir}` shorthand. Bare-token resolution falls through to `cook.env`.

- [ ] **Step 10.4: Add `validate_placeholders` pass**

Append to `cli/crates/cook-luagen/src/template.rs`:

```rust
pub struct PlaceholderValidationContext<'a> {
    pub mode: &'a CookMode,                    // imported from cook_step
    pub declared_output_count: usize,
    pub recipe_names: &'a BTreeSet<String>,
}

pub fn validate_placeholders(
    body_text: &str,
    ctx: &PlaceholderValidationContext,
) -> Result<(), String> {
    for tok in iter_placeholders(body_text) {
        let t = tok.trim_matches(|c| c == '{' || c == '}');
        if let Some((prefix, suffix)) = t.rsplit_once('.') {
            // Dotted form
            match prefix {
                "in" => {
                    if !is_iterating(ctx.mode) {
                        return Err(format!("CS-0022: {{in.{suffix}}} is invalid in many-to-one mode"));
                    }
                    if !is_path_accessor(suffix) {
                        return Err(format!("CS-0022: unknown accessor `{suffix}` (expected stem|name|ext|dir)"));
                    }
                }
                "out" => {
                    if ctx.declared_output_count != 1 {
                        return Err(format!("CS-0022: {{out.{suffix}}} requires single-output step (use {{out_N.{suffix}}})"));
                    }
                    if !is_path_accessor(suffix) {
                        return Err(format!("CS-0022: unknown accessor `{suffix}`"));
                    }
                }
                p if p.starts_with("out_") => {
                    let n: usize = p["out_".len()..].parse().map_err(|_| format!("CS-0022: invalid {{out_N}} index in `{p}`"))?;
                    if ctx.declared_output_count == 1 {
                        return Err(format!("CS-0022: {{out_{n}.{suffix}}} requires multi-output step (use {{out.{suffix}}})"));
                    }
                    if n < 1 || n > ctx.declared_output_count {
                        return Err(format!("CS-0022: {{out_{n}.{suffix}}} out of range (step has {} outputs)", ctx.declared_output_count));
                    }
                    if !is_path_accessor(suffix) {
                        return Err(format!("CS-0022: unknown accessor `{suffix}`"));
                    }
                }
                lib if ctx.recipe_names.contains(lib) => {
                    return Err(format!("CS-0022: {{{lib}.{suffix}}} is rejected inside using-clause body; use {{in.{suffix}}} if `{lib}` is the driver, or reach for Lua otherwise"));
                }
                _ => { /* falls through to cook.env at expansion time */ }
            }
        } else {
            // Bare token
            match t {
                "in" => {
                    if !is_iterating(ctx.mode) {
                        return Err(format!("CS-0022: {{in}} is invalid in many-to-one mode"));
                    }
                }
                "out" => {
                    if ctx.declared_output_count != 1 {
                        return Err(format!("CS-0022: {{out}} requires single-output step (use {{out_N}} for multi-output)"));
                    }
                }
                t if t.starts_with("out_") => {
                    let n: usize = t["out_".len()..].parse().map_err(|_| format!("CS-0022: invalid {{out_N}} index in `{t}`"))?;
                    if ctx.declared_output_count == 1 {
                        return Err(format!("CS-0022: {{out_{n}}} requires multi-output step (use {{out}})"));
                    }
                    if n < 1 || n > ctx.declared_output_count {
                        return Err(format!("CS-0022: {{out_{n}}} out of range (step has {} outputs)", ctx.declared_output_count));
                    }
                }
                "all" => {
                    if is_iterating(ctx.mode) {
                        return Err("CS-0022: {all} is invalid in one-to-one mode (use {in})".to_string());
                    }
                }
                "stem" | "name" | "ext" | "dir" => {
                    return Err(format!("CS-0022: bare {{{t}}} was removed; use {{in.{t}}} (or {{out.{t}}} / {{out_N.{t}}})"));
                }
                _ => { /* recipe name or env-var fallthrough */ }
            }
        }
    }
    Ok(())
}

fn is_iterating(m: &CookMode) -> bool {
    matches!(m, CookMode::OneToOne)
}

fn is_path_accessor(s: &str) -> bool {
    matches!(s, "stem" | "name" | "ext" | "dir")
}
```

`iter_placeholders(body_text)` walks the text and yields each `{...}` token in order. If a stripped-down version exists in the file (e.g., `iter_brace_tokens`), reuse it.

- [ ] **Step 10.5: Update `dep_ref::parse_dep_token`**

In `cli/crates/cook-luagen/src/dep_ref.rs`, find `parse_dep_token` (around line 95–115). It currently splits on `.` and checks if the prefix is in `recipe_names`. The new dotted shapes (`{in.X}`, `{out.X}`, `{out_N.X}`) have prefixes that are **not** recipe names — they should fall through cleanly. Verify by adding a unit test:

```rust
#[test]
fn cs_0022_in_and_out_are_not_dep_refs() {
    let mut names = BTreeSet::new();
    names.insert("libmath".to_string());

    assert!(parse_dep_token("in.stem", &names).is_none());
    assert!(parse_dep_token("out.dir", &names).is_none());
    assert!(parse_dep_token("out_1.stem", &names).is_none());
    assert_eq!(parse_dep_token("libmath.stem", &names).map(|d| d.recipe), Some("libmath".to_string()));
}
```

If the test passes without changes to `parse_dep_token`, no edit is needed; otherwise narrow the recipe-name check so dotted prefixes like `in`, `out`, `out_N` are not matched.

- [ ] **Step 10.6: Run cargo test**

```bash
cd /home/alex/dev/cook
cargo test -p cook-luagen
```

Expected: the new tests PASS; pre-existing codegen tests may still fail (those that exercised the old expansion of bare `{stem}` etc.); fixed in Task 11.

- [ ] **Step 10.7: Commit**

```bash
git add cli/crates/cook-luagen/src/template.rs cli/crates/cook-luagen/src/dep_ref.rs
git commit -m "codegen(CS-0022): placeholder vocabulary — {in.X}, {out.X}, {out_N}, {out_N.X}; validate_placeholders pass"
```

---

## Task 11: Codegen — wire shell-block placeholder substitution into `cook_step.rs`

**Files:**
- Modify: `cli/crates/cook-luagen/src/cook_step.rs`

- [ ] **Step 11.1: Update the `OneToOne` arm to drop the `Shell(cmd)` branch**

In `cli/crates/cook-luagen/src/cook_step.rs:96-119` (the `OneToOne` codegen arm, inside `generate_cook_step`), the current shape pattern-matches `UsingClause::Shell(cmd)`, `UsingClause::LuaBlock(code)`, `UsingClause::ShellBlock(_)` (unreachable), and `None` (unreachable in OneToOne).

The `Shell(cmd)` variant is gone. Rewrite the arm:

```rust
match &cook_step.using_clause {
    Some(UsingClause::ShellBlock(lines)) => {
        // Per CS-0022, shell-block contents go through expand_template_to_lua_with_deps,
        // line by line, with the same per-iteration substitution that the (removed)
        // Shell(cmd) arm used to do.
        let combined = build_shell_block_command(lines, recipe_names);
        let lua_expr = expand_template_to_lua_with_deps(&combined, recipe_names);
        out.push_str(&format!(
            "        cook.add_unit({{inputs = {{_cook_in}}, output = _cook_out, command = {}}})\n",
            lua_expr
        ));
    }
    Some(UsingClause::LuaBlock(code)) => {
        let code_literal = crate::lua_string::wrap_lua_string(code);
        let ing_groups = format_ingredient_groups(ingredients.len());
        out.push_str(&format!(
            "        cook.add_unit({{inputs = {{_cook_in}}, output = _cook_out, lua_code = {}, ingredient_groups = {}}})\n",
            code_literal, ing_groups
        ));
    }
    None => unreachable!("OneToOne mode requires a using-clause"),
}
```

- [ ] **Step 11.2: Add `build_shell_block_command` helper**

In the same file, near the bottom (or in a new private helper section):

```rust
/// Joins a shell-block's lines with `\n`, prepended with `set -e`. The result
/// is a single shell text suitable for /bin/sh -c — the same shape that
/// `using "cmd"` used to produce, just spelled multi-line.
fn build_shell_block_command(lines: &[String], _recipe_names: &BTreeSet<String>) -> String {
    let mut out = String::from("set -e");
    for line in lines {
        out.push('\n');
        out.push_str(line);
    }
    out
}
```

- [ ] **Step 11.3: Update the `ManyToOne` arm to drop the `Shell(cmd)` branch**

In `cli/crates/cook-luagen/src/cook_step.rs:127-152` (the `ManyToOne` arm), apply the parallel rewrite — replace `if let Some(UsingClause::Shell(cmd))` with:

```rust
match &cook_step.using_clause {
    Some(UsingClause::ShellBlock(lines)) => {
        let combined = build_shell_block_command(lines, recipe_names);
        let lua_expr = expand_template_to_lua_with_deps(&combined, recipe_names);
        out.push_str(&format!(
            "    cook.add_unit({{inputs = {}, output = _cook_out, command = {}}})\n",
            input_source, lua_expr
        ));
    }
    Some(UsingClause::LuaBlock(code)) => {
        // CS-0022: the wart-fix branch — many-to-one Lua block runs once with
        // the full inputs/outputs arrays. (Pre-CS-0022, this case routed
        // through OneToOne and iterated.)
        let code_literal = crate::lua_string::wrap_lua_string(code);
        let ing_groups = format_ingredient_groups(ingredients.len());
        out.push_str(&format!(
            "    cook.add_unit({{inputs = {}, output = _cook_out, lua_code = {}, ingredient_groups = {}}})\n",
            input_source, code_literal, ing_groups
        ));
    }
    None => unreachable!("ManyToOne mode requires a using-clause"),
}
```

- [ ] **Step 11.4: Update the `BlockStep` arm to apply substitution**

In `cli/crates/cook-luagen/src/cook_step.rs:153-202` (the `BlockStep` arm — multi-output), the existing `ShellBlock(lines)` branch escapes the lines verbatim. Replace with:

```rust
Some(UsingClause::ShellBlock(lines)) => {
    let combined = build_shell_block_command(lines, recipe_names);
    let lua_expr = expand_template_to_lua_with_deps(&combined, recipe_names);
    out.push_str(&format!(
        "    cook.add_unit({{inputs = _cook_ins, outputs = _cook_outs, command = {}}})\n",
        lua_expr
    ));
}
```

The `LuaBlock(code)` branch in `BlockStep` is unchanged.

- [ ] **Step 11.5: Wire `validate_placeholders` into `generate_cook_step`**

At the top of `generate_cook_step` (after `mode` is computed), call the validator on each shell-block line and, if it fails, propagate the error. Concrete sketch:

```rust
if let Some(UsingClause::ShellBlock(lines)) = &cook_step.using_clause {
    let ctx = PlaceholderValidationContext {
        mode: &mode,
        declared_output_count: cook_step.outputs.len(),
        recipe_names,
    };
    for (idx, line) in lines.iter().enumerate() {
        if let Err(msg) = validate_placeholders(line, &ctx) {
            // Promote to a load-time diagnostic via the codegen's existing
            // error-propagation surface (panic for now if no Result return;
            // see surrounding fn signature).
            panic!("cook step at line {_line}, body line {}: {}", idx + 1, msg);
        }
    }
}
```

If the surrounding function does not return `Result`, refactor it to do so or propagate the validation through the existing `validate_accessor_placement` path in `recipe.rs`. (Prefer the latter — keep `generate_cook_step` infallible by running validation in a pre-pass.)

- [ ] **Step 11.6: Run cargo test**

```bash
cargo test -p cook-luagen
cargo test -p cook-lang   # ensure nothing here regressed
```

Expected: PASS for codegen tests touching the new arms; the conformance suite may still fail because the corpus is not yet migrated (Task 15).

- [ ] **Step 11.7: Commit**

```bash
git add cli/crates/cook-luagen/src/cook_step.rs
git commit -m "codegen(CS-0022): shell-block lines go through expand_template_to_lua_with_deps; many-to-one Lua wart fixed"
```

---

## Task 12: Codegen — verify dep-ref extraction across the new shapes

**Files:**
- Modify: `cli/crates/cook-luagen/src/dep_ref.rs`

- [ ] **Step 12.1: Confirm `extract_dep_refs` walks shell-block content**

Read `cli/crates/cook-luagen/src/dep_ref.rs:27-70` (the `extract_dep_refs` function). The current shape pattern-matches each step kind. Verify that the `Step::Cook` arm (or wherever cook-step bodies are visited) handles `UsingClause::ShellBlock(lines)` by walking each line for `{NAME}` tokens, the same way it handles `UsingClause::Shell(cmd)` today.

If `ShellBlock` lines were skipped in the previous version (because dep-refs only came through the string form per pre-CS-0022 §5.5), add the walk. The new behaviour must parallel the §5.5 surface extension from Task 2.

Concrete: in the `Step::Cook` arm, replace any `if let UsingClause::Shell(cmd) = ...` walker with a loop over `ShellBlock(lines)` joining lines and feeding to the same token tokeniser, OR walking line-by-line.

- [ ] **Step 12.2: Add a unit test**

```rust
#[test]
fn cs_0022_shell_block_dep_ref_extraction() {
    let mut names = BTreeSet::new();
    names.insert("libmath".to_string());

    let recipe = parse_recipe(r#"recipe app
    cook "build/app" using {
        gcc -o {out} main.c {libmath}
    }
"#);
    let refs = extract_dep_refs(&recipe, &names);
    assert!(refs.iter().any(|r| matches!(r, DepRef::Bare(s) if s == "libmath")),
        "shell block must contribute its {{libmath}} reference to the dep graph");
}
```

(Use whatever helper lets the test parse a single-recipe Cookfile string; mirror existing tests in the same file.)

- [ ] **Step 12.3: Run tests**

```bash
cargo test -p cook-luagen --lib dep_ref
```

- [ ] **Step 12.4: Commit**

```bash
git add cli/crates/cook-luagen/src/dep_ref.rs
git commit -m "codegen(CS-0022): extract dep-refs from shell_block content (§5.5 surface extension)"
```

---

# Phase D — Tree-sitter (Tasks 13–14)

## Task 13: Tree-sitter — drop string arm from `using_clause`; verify queries

**Files:**
- Modify: `tree-sitter-cook/grammar.js`
- Modify: `tree-sitter-cook/queries/highlights.scm`
- Confirm: `tree-sitter-cook/queries/injections.scm` (already has shell_block injection)
- Regenerate: `tree-sitter-cook/src/parser.c` (via `npm run generate`)

- [ ] **Step 13.1: Edit `grammar.js`**

In `tree-sitter-cook/grammar.js:166-174`, the `using_clause` rule reads:

```js
using_clause: ($) =>
  seq(
    "using",
    choice(
      field("command", $.string),
      field("lua", $.using_lua_block),
      field("shell", $.shell_block),
    ),
  ),
```

Replace with:

```js
using_clause: ($) =>
  seq(
    "using",
    choice(
      field("lua", $.using_lua_block),
      field("shell", $.shell_block),
    ),
  ),
```

(The `block_using_clause` already only carries the two block forms — no change.)

Update the header comment claim from `Cook Standard v0.4` to `Cook Standard v0.4 + CS-0022`.

- [ ] **Step 13.2: Regenerate parser**

```bash
cd /home/alex/dev/cook/tree-sitter-cook
npm run generate
```

Expected: `src/parser.c` regenerated; `LANGUAGE_VERSION` may bump if tree-sitter-cli ABI changed; `SYMBOL_COUNT` will decrement by one.

- [ ] **Step 13.3: Update `highlights.scm`**

In `tree-sitter-cook/queries/highlights.scm`, find the rule that highlights `(using_clause command: (string))`. If present (looking at the file, the current `(cook_step outputs: (string) @string.special)` row is the single-string output highlight; verify whether a separate `using_clause command:` rule exists).

Concrete: search the file for `using_clause` as a query target. If a `(using_clause command: (string) ...)` rule exists, delete it. The `(using_clause (using_lua_block ...))` and `(using_clause (shell_block ...))` patterns are untouched (they highlight the inner content already).

- [ ] **Step 13.4: Confirm `injections.scm` still has shell-block bash injection**

```bash
grep -n "shell_block\|injection" tree-sitter-cook/queries/injections.scm
```

Expected: includes the rule

```
(shell_block
  (shell_content) @injection.content
  (#set! injection.language "bash"))
```

(Added in the prior session work, commit at `90b65a2`-ish or earlier.)

- [ ] **Step 13.5: Run tree-sitter test**

```bash
cd /home/alex/dev/cook/tree-sitter-cook
npm test
```

Expected: most tests PASS; corpus cases that exercise `using "cmd"` will FAIL (Task 14 migrates them).

- [ ] **Step 13.6: Commit**

```bash
git add tree-sitter-cook/grammar.js tree-sitter-cook/src/parser.c tree-sitter-cook/queries/highlights.scm
git commit -m "tree-sitter(CS-0022): drop STRING arm from using_clause; regenerate"
```

---

## Task 14: Tree-sitter — corpus migration + new cases

**Files:**
- Modify: `tree-sitter-cook/test/corpus/*.txt`

- [ ] **Step 14.1: Inventory corpus cases**

```bash
cd /home/alex/dev/cook/tree-sitter-cook
grep -lE 'using "' test/corpus/*.txt
```

For each file in the output, the cases inside that exercise `using "cmd"` need their input rewritten to `using { cmd }` and their expected parse-tree updated (the `command: (string ...)` field on `using_clause` becomes `shell: (shell_block (shell_content ...))`).

- [ ] **Step 14.2: Migrate each corpus file**

For each affected `.txt` file: open it, find each `==================` divider that introduces a `using "cmd"` test, rewrite the source half (above the `---`) to `using { cmd }`, rewrite the parse-tree half (below the `---`) to reflect the new AST shape.

Concrete rewrite pattern, source half:

```
cook "out.o" using "gcc -c {in} -o {out}"
```

becomes

```
cook "out.o" using { gcc -c {in} -o {out} }
```

Parse-tree half pattern: replace

```
(using_clause
  command: (string))
```

with

```
(using_clause
  shell: (shell_block
    (shell_content)))
```

- [ ] **Step 14.3: Add new corpus cases**

Append to a new file `test/corpus/cs_0022.txt`:

```
====================
shell block one-line
====================

recipe build
    cook "build/{in.stem}.o" using { gcc -c {in} -o {out} }

---

(source_file
  (recipe
    (explicit_recipe_header
      name: (identifier))
    (cook_step
      outputs: (string)
      (using_clause
        shell: (shell_block
          (shell_content))))))

====================
shell block multi-output indexed
====================

recipe gen
    cook "out.js" "out.wasm" using {
        wasm-pack build
        cp pkg/main.js {out_1}
        cp pkg/main.wasm {out_2}
    }

---

(source_file
  (recipe
    (explicit_recipe_header
      name: (identifier))
    (cook_step
      outputs: (string)
      outputs: (string)
      (block_using_clause
        shell: (shell_block
          (shell_content))))))

====================
in.accessor in output
====================

recipe build
    ingredients "src/*.c"
    cook "build/{in.stem}.o" using {
        gcc -c {in} -o {out}
    }

---

(source_file
  (recipe
    (explicit_recipe_header
      name: (identifier))
    (ingredients_step
      (string))
    (cook_step
      outputs: (string)
      (using_clause
        shell: (shell_block
          (shell_content))))))
```

- [ ] **Step 14.4: Run npm test + conformance**

```bash
cd /home/alex/dev/cook/tree-sitter-cook
npm test
node scripts/conformance.mjs
```

Expected: PASS.

- [ ] **Step 14.5: Commit**

```bash
git add tree-sitter-cook/test/corpus/
git commit -m "tree-sitter(CS-0022): corpus — migrate using-string cases; add CS-0022 cases"
```

---

# Phase E — Conformance fixtures (Tasks 15–16)

## Task 15: Conformance — migrate existing fixtures

**Files:**
- Modify: `standard/conformance/positive/*/Cookfile` (8 fixtures using `using "cmd"`)
- Modify: `standard/conformance/positive/*/parse.txt` (regenerate after Cookfile change)

- [ ] **Step 15.1: Inventory fixtures**

```bash
cd /home/alex/dev/cook
find standard/conformance -name 'Cookfile' | xargs grep -lE 'using "'
find standard/conformance -name 'Cookfile' | xargs grep -lE '\{(stem|name|ext|dir)\}'
```

Expected: 8 fixtures from the first command; some overlap with the second.

- [ ] **Step 15.2: Migrate each Cookfile**

For each fixture's `Cookfile`:

1. `using "cmd"` → `using { cmd }` (one-line block — preserves indentation if the original was multi-line).
2. Bare `{stem}` → `{in.stem}`. Same for `{name}`, `{ext}`, `{dir}`. Apply this to BOTH the output pattern and the using-clause body.

- [ ] **Step 15.3: Regenerate `parse.txt`**

For each migrated fixture:

```bash
cd /home/alex/dev/cook
cargo run -p cook-cli -- --emit-parse <path/to/fixture/Cookfile> > <path/to/fixture/parse.txt>
```

(Use whatever flag the cook CLI exposes for "dump parse"; if the conformance harness has its own regenerator, prefer that.)

For each, compare the diff to confirm the only changes are the `Shell(...)` → `ShellBlock([...])` AST swap and the bare-accessor → dotted-accessor swap.

- [ ] **Step 15.4: Run conformance**

```bash
cargo test -p cook-lang --test conformance
```

Expected: PASS.

- [ ] **Step 15.5: Commit**

```bash
git add standard/conformance/positive/
git commit -m "conformance(CS-0022): migrate 8 fixtures — using \"cmd\" → using {cmd}; bare accessors → {in.X}"
```

---

## Task 16: Conformance — add new fixtures

**Files:**
- Create: `standard/conformance/positive/022-shell-block-single-line/{Cookfile,parse.txt}`
- Create: `standard/conformance/positive/023-shell-block-multi-output-out-n/{Cookfile,parse.txt}`
- Create: `standard/conformance/positive/024-shell-block-cross-recipe-ref/{Cookfile,parse.txt}`
- Create: `standard/conformance/positive/025-many-to-one-lua-block/{Cookfile,parse.txt}`
- Create: `standard/conformance/negative/017-bare-stem-rejected/{Cookfile,error.txt}`
- Create: `standard/conformance/negative/018-using-string-form-rejected/{Cookfile,error.txt}`
- Create: `standard/conformance/negative/019-out-n-in-single-output-rejected/{Cookfile,error.txt}`
- Create: `standard/conformance/negative/020-out-bare-in-multi-output-rejected/{Cookfile,error.txt}`
- Create: `standard/conformance/negative/021-mixed-driver-multi-output-rejected/{Cookfile,error.txt}`
- Create: `standard/conformance/negative/022-lib-accessor-in-using-rejected/{Cookfile,error.txt}`
- Create: `standard/conformance/negative/023-multi-output-one-to-one-mixed-rejected/{Cookfile,error.txt}`

- [ ] **Step 16.1: Create positive fixtures**

For each positive fixture, write a `Cookfile` exercising the single shape under test. Examples:

`022-shell-block-single-line/Cookfile`:

```cook
recipe build
    cook "build/{in.stem}.o" using { gcc -c {in} -o {out} }
```

`023-shell-block-multi-output-out-n/Cookfile`:

```cook
recipe gen
    cook "out.js" "out.wasm" using {
        wasm-pack build
        cp pkg/main.js {out_1}
        cp pkg/main.wasm {out_2}
    }
```

`024-shell-block-cross-recipe-ref/Cookfile`:

```cook
recipe libmath
    ingredients "src/math/*.c"
    cook "build/lib/libmath.a" using { ar rcs {out} {all} }

recipe app
    cook "build/bin/app" using { gcc -o {out} main.c {libmath} }
```

`025-many-to-one-lua-block/Cookfile`:

```cook
recipe link
    ingredients "build/*.o"
    cook "build/app" using >{
        cook.sh("gcc " .. table.concat(inputs, " ") .. " -o " .. output)
    }
```

For each, generate the matching `parse.txt`:

```bash
cargo run -p cook-cli -- --emit-parse <Cookfile> > <parse.txt>
```

- [ ] **Step 16.2: Create negative fixtures**

For each negative fixture, write a `Cookfile` exhibiting the failure shape, plus an `error.txt` containing the exact diagnostic substring the harness checks. Examples:

`017-bare-stem-rejected/Cookfile`:

```cook
recipe build
    ingredients "src/*.c"
    cook "build/{stem}.o" using { gcc -c {in} -o {out} }
```

`017-bare-stem-rejected/error.txt`:

```
CS-0022: bare {stem} was removed; use {in.stem}
```

`018-using-string-form-rejected/Cookfile`:

```cook
recipe build
    cook "out" using "echo hi"
```

`018-using-string-form-rejected/error.txt`:

```
CS-0022
```

`019-out-n-in-single-output-rejected/Cookfile`:

```cook
recipe build
    cook "build/app" using { gcc {in} -o {out_1} }
```

`019.../error.txt`:

```
{out_1} requires multi-output step
```

`020-out-bare-in-multi-output-rejected/Cookfile`:

```cook
recipe build
    cook "a.js" "a.wasm" using { gen --js {out} }
```

`020.../error.txt`:

```
{out} requires single-output step
```

`021-mixed-driver-multi-output-rejected/Cookfile`:

```cook
recipe libmath
    ingredients "src/math/*.c"

recipe build
    ingredients "src/*.c"
    cook "{in.stem}.o" "{libmath.stem}.bin" using {
        do-stuff
    }
```

`021.../error.txt`:

```
mix iteration drivers
```

`022-lib-accessor-in-using-rejected/Cookfile`:

```cook
recipe libmath
    ingredients "src/math/*.c"

recipe build
    ingredients "src/*.c"
    cook "build/{in.stem}.o" using { gcc -c {in} -o {out} -L {libmath.dir} }
```

`022.../error.txt`:

```
{libmath.dir} is rejected inside using-clause body
```

`023-multi-output-one-to-one-mixed-rejected/Cookfile` covers the third sub-case in §3.1: one bears `{in.X}`, another is literal.

```cook
recipe build
    ingredients "src/*.rs"
    cook "{in.stem}.js" "out.wasm" using {
        gen
    }
```

`023.../error.txt`:

```
mix iteration drivers
```

- [ ] **Step 16.3: Run conformance**

```bash
cargo test -p cook-lang --test conformance
```

Expected: PASS — every new positive fixture round-trips; every new negative fixture's `error.txt` substring matches the actual diagnostic.

- [ ] **Step 16.4: Commit**

```bash
git add standard/conformance/positive/02{2,3,4,5}-* standard/conformance/negative/0{17,18,19,20,21,22,23}-*
git commit -m "conformance(CS-0022): 4 positive + 7 negative fixtures covering the new surface"
```

---

# Phase F — Repo migration (Task 17)

## Task 17: Migrate in-repo Cookfiles, cook_modules, README claims

**Files:**
- Modify: `Cookfile`, `cli/Cookfile`, `tree-sitter-cook/Cookfile`, `standard/Cookfile`, `examples/*/Cookfile`
- Modify: `cook_modules/release.lua`, `standard/cook_modules/checks.lua`, `examples/*/cook_modules/*.lua`
- Modify: `cli/crates/cook-lang/CONFORMANCE.md`, `cli/crates/cook-lang/README.md`, root `README.md`

- [ ] **Step 17.1: Migrate top-level and subdirectory Cookfiles**

For each `Cookfile` in the repo (excluding `.worktrees/`, `build/`, `.cook/cache/`, conformance fixtures already migrated in Task 15):

1. Replace each `using "cmd"` with `using { cmd }`.
2. Replace bare `{stem}` / `{name}` / `{ext}` / `{dir}` with `{in.stem}` / etc.

```bash
# discover candidates
cd /home/alex/dev/cook
find . -name 'Cookfile' \
    -not -path '*/build/*' \
    -not -path '*/.cook/*' \
    -not -path '*/.worktrees/*' \
    -not -path '*/conformance/*' \
    -print
```

Apply the rewrites file by file. Use Edit (not sed) so each rewrite is reviewable.

- [ ] **Step 17.2: Migrate cook_modules Lua files**

`cook_modules/*.lua` and `examples/*/cook_modules/*.lua` build command strings programmatically. Search each for `string.format` / `..` calls that produce `using "..."` cook-step lines or use bare-accessor placeholders in produced shell:

```bash
grep -rnE 'using "|{stem}|{name}|{ext}|{dir}' cook_modules/ standard/cook_modules/ examples/*/cook_modules/
```

For each match, rewrite:
- `using "..."` strings produced by Lua become `using { ... }` strings.
- Bare-accessor placeholders in produced shell become dotted forms.

Example (likely shape in `cook_modules/cpp.lua`): a function emitting

```lua
return string.format("cook %q using \"gcc -c {in} -o {out}\"", out)
```

becomes

```lua
return string.format("cook %q using { gcc -c {in} -o {out} }", out)
```

- [ ] **Step 17.3: Migrate README claims**

Update version-claim text in:
- `cli/crates/cook-lang/CONFORMANCE.md`
- `cli/crates/cook-lang/README.md`
- root `README.md`

Find any text claiming "Cook Standard v0.4" and append " + CS-0022" — or, if the team prefers, leave the version claim at v0.4 (CS-0022 is post-v0.4 pre-v0.5 and does not bump the cut).

- [ ] **Step 17.4: Final verification**

```bash
cd /home/alex/dev/cook
cargo test -p cook-lang --test conformance
cargo test -p cook-lang
cargo test -p cook-luagen
cargo test
```

Then exercise each migrated Cookfile end-to-end:

```bash
cook --help                                  # smoke
(cd examples/lua-build && cook .)            # exercise an iterating shell-block recipe
(cd examples/cpp-project && cook .)          # exercise cross-recipe ref + multi-output
(cd examples/multi-output && cook .)         # exercise {out_N} usage
```

Expected: all green; spurious `{out}` files (from earlier development loops, e.g. `tree-sitter-cook/{out}`) removed by hand if any survive.

```bash
cd /home/alex/dev/cook/standard
pnpm build && pnpm test && pnpm lint:keywords
```

```bash
cd /home/alex/dev/cook/tree-sitter-cook
npm run generate && npm test && node scripts/conformance.mjs
```

- [ ] **Step 17.5: Commit**

```bash
git add Cookfile cli/Cookfile tree-sitter-cook/Cookfile standard/Cookfile examples/ cook_modules/ standard/cook_modules/ cli/crates/cook-lang/CONFORMANCE.md cli/crates/cook-lang/README.md README.md
git commit -m "migrate(CS-0022): all in-repo Cookfiles and cook_modules use the new surface"
```

- [ ] **Step 17.6: Final integration**

Run the full test matrix once more from a clean state:

```bash
cd /home/alex/dev/cook
cargo clean && cargo test --workspace
(cd standard && pnpm build && pnpm test && pnpm lint:keywords)
(cd tree-sitter-cook && rm -rf node_modules && npm install && npm run generate && npm test && node scripts/conformance.mjs)
```

Expected: every command exits 0.

---

## Self-review checklist

After executing the plan and before opening the PR, verify:

1. **Every section of the spec has a task that implements it.**
   - §3.1 Iteration rule → Task 1 (§4.5.1) + Task 9 (codegen mode).
   - §3.2 using_clause collapse → Task 1 (§4.5 table), Task 4 (App. A grammar), Task 6 (AST), Task 7 (parser), Task 13 (tree-sitter).
   - §3.3 Placeholder vocabulary → Task 3 (§6.7), Task 10 (template + validate).
   - §3.4 Output pattern surface → Task 1 (§4.5.1 example) + Task 10 (`output_pattern_kind`).
   - §3.5 Lua bindings → Task 11 ManyToOne arm fix.
   - §3.6 Cross-recipe extension → Task 2 (§5.5), Task 12 (dep_ref).
   - §3.7 Substitution timing → Task 11 (`expand_template_to_lua_with_deps` runs at register time per existing infrastructure).
   - §3.8 App. A grammar deltas → Task 4.
   - §3.9 Tree-sitter deltas → Task 13.
   - §4 Migration → Tasks 15, 16, 17.
   - §5 Implementation impact → Tasks 6–14.
   - §6 Open questions documented in commits.
   - §7 Rationale → Task 5 (App. B.6.5–6.8).
   - §8 Acceptance criteria → Task 17.6 (final integration).

2. **No placeholders in step text.** Every code/text block is verbatim content the engineer pastes or types.

3. **Type and method-name consistency.**
   - `OutputPatternKind` variants (Task 10) consumed by `cook_step_mode` (Task 9) and `check_multi_output_coherence` (Task 9).
   - `validate_placeholders` signature (Task 10) consumed by `generate_cook_step` (Task 11).
   - `build_shell_block_command` defined in Task 11 used in three arms of the same task.

4. **Ordering is consistent.** Every cross-task reference (e.g., Task 7 → Task 8 → Task 11) is forward-only or makes its retroactive nature explicit.

If issues surface during execution, fix in place and update this plan section.

---
