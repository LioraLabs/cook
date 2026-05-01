# Cook Standard CS-0024 — `plate`/`test` unification Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Land CS-0024 — make `plate` and `test` "cook steps with no declared outputs": same body grammar (shell block + Lua block), same placeholder vocabulary, mode (one-to-one / many-to-one / one-shot) deduced from body content (`{in}`/`{all}` for shell, `input`/`inputs` for Lua). Remove the STRING-only surface, the `{out}` misnomer, and unconditional iteration. Caching stays out of the Standard.

**Architecture:** Lockstep — the Standard, the Rust parser (`cli/crates/cook-lang`), the codegen (`cli/crates/cook-luagen`), the tree-sitter grammar (`tree-sitter-cook/`), the conformance corpus (`standard/conformance/`), the in-repo example Cookfiles, and the `cook_modules/` Lua all move together inside this CS. The plate/test parser path generalizes to share `shell_block` / `using_lua_block` productions with cook; the codegen factors mode-deduction into a single helper used by both the shell (placeholder scan) and Lua (identifier scan) paths.

**Tech Stack:** Rust (parser/codegen/engine), MDX/Astro Starlight (Standard), tree-sitter (grammar.js + C scanner), pnpm + vitest (Standard tests), `cargo test -p cook-lang --test conformance` (corpus harness), `cargo test --workspace` (full test).

**Spec:** `standard/specs/2026-05-01-plate-test-unification-design.md` (commit `60b04a6`).

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

**Spec-side tasks** end with:

```bash
cd /home/alex/dev/cook/standard
pnpm build              # rehype-bare-ref-lint + rehype-clause-anchors gate slug consistency
pnpm test               # vitest plugin tests
pnpm lint:keywords      # RFC-2119 lowercase-keyword lint
```

**Parser/codegen tasks** end with:

```bash
cd /home/alex/dev/cook
cargo test -p cook-lang                       # unit + lexer + recipe tests
cargo test -p cook-lang --test conformance   # corpus harness
cargo test -p cook-luagen                     # codegen tests
cargo test --workspace                        # full workspace (post any cross-crate change)
```

**Tree-sitter tasks** end with:

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
| **CS-0024** | plate/test unification | Spec at `standard/specs/2026-05-01-plate-test-unification-design.md` |

Next free ID is **CS-0024** (CS-0023 was the v0.5 cut entry per the most recent App. D update; commit `e41eafb`).

---

## File structure

| File | Responsibility | Tasks |
|---|---|---|
| `standard/src/content/docs/04-recipes.mdx` | §4.7 (`plate` rewritten — body grammar, three modes), §4.8 (`test` rewritten — body grammar, three modes, modifiers retained), §4.4 step-kinds table & Note 4.4.2 unchanged in shape (plate/test stay declarative-region). | 1 |
| `standard/src/content/docs/05-cross-recipe-references.mdx` | §5.5 surface-list extension: "plate command / test command" → "plate body / test body". §5.4 firewall preserved. | 2 |
| `standard/src/content/docs/06-cook-lua-api.mdx` | §6.4 Lua-binding table extends with plate/test rows. §6.7 placeholder vocabulary unchanged for cook; new §6.7.x covers plate/test placeholder rules. | 2 |
| `standard/src/content/docs/08-execution-model.mdx` | §8.1.2 phase-classification table gains plate/test body-form rows. §8.3 step-group rule unchanged. | 2 |
| `standard/src/content/docs/appendix/A-grammar.mdx` | `plate_step` and `test_step` productions updated per §3.2 of the spec. Step-dispatch list unchanged. | 3 |
| `standard/src/content/docs/appendix/B-rationale.mdx` | **Delete** B.4.7. Add new B.4.x covering: plate/test as cook with no outputs, body-driven mode for plate/test, `{out}` rejection, `{lib.X}` rejection, why static Lua identifier scan. | 3 |
| `standard/src/content/docs/appendix/D-changes.mdx` | New entry for CS-0024 with the parse-format-altering note. | 16 |
| `cli/crates/cook-lang/src/ast.rs` | `PlateStep` and `TestStep` swap `command: String` for `body: Body`; new `Body` enum (alias of or replacement for `UsingClause`). | 4 |
| `cli/crates/cook-lang/src/recipe.rs` | Plate/test parser arms (lines ~200, ~209) drop `parse_single_quoted_string` / `parse_test_command`; parse a body via the same dispatch shape used in `cook_line.rs`. | 5 |
| `cli/crates/cook-lang/src/cook_line.rs` | Factor the using-payload dispatch (lines 158–217) into a reusable `parse_body(after_open, line, tokens, pos, source_lines)` helper, callable from cook_line.rs and recipe.rs. The cook-side migration diagnostic for the dropped string form is preserved by a separate code path. | 5 |
| `cli/crates/cook-lang/src/tests.rs` | Update `test_plate_step`, `test_test_step_*`, and the dropped-form fixtures; add new tests for plate/test block parsing and the migration diagnostic. | 6 |
| `cli/crates/cook-lang/tests/conformance.rs` | `repr` formatter for plate/test renders bodies, not strings; canonical re-emission updated. | 6 |
| `cli/crates/cook-luagen/src/template.rs` | New `detect_body_mode(body: &Body)` function + new `validate_plate_test_placeholders(body, mode)` pass. Drop the bespoke `expand_plate_cmd_with_deps` / `expand_test_cmd_with_deps`; route plate/test through `expand_template_to_lua_with_deps` with iteration-binding name parameter. | 7 |
| `cli/crates/cook-luagen/src/plate_step.rs` | Rewrite `generate_plate_step` to dispatch on `Body` × mode (six arms); shared helpers from `template.rs`. | 8 |
| `cli/crates/cook-luagen/src/test_step.rs` | Rewrite `generate_test_step` analogously, preserving `timeout` / `should_fail` field emission. | 8 |
| `cli/crates/cook-luagen/src/dep_ref.rs` | `Step::Plate { … }` / `Step::Test { … }` arms (lines ~46, ~47) read tokens from `body.text()` instead of `command`. | 8 |
| `cli/crates/cook-luagen/src/recipe.rs` | Call sites unchanged in shape; the `prev_cook_index` / source-fallback computation moves into the plate/test generators (now mode-aware). | 8 |
| `cli/crates/cook-luagen/src/tests.rs` | Update the `test_plate_step` / `test_test_step_*` codegen tests; add coverage for each of the six mode/form combinations and each rejection. | 9 |
| `tree-sitter-cook/grammar.js` | `plate_step` / `test_step` rules drop the `field("command", $.string)` arm; gain `field("body", choice($.shell_block, $.using_lua_block))`; `test_step` retains modifier fields. | 10 |
| `tree-sitter-cook/src/parser.c` | Regenerated. | 10 |
| `tree-sitter-cook/queries/highlights.scm` | Drop the `(plate_step command: (string) @string.special)` and `(test_step command: (string) @string.special)` rules. | 10 |
| `tree-sitter-cook/queries/injections.scm` | Existing `shell_block` and `using_lua_block` injections cover plate/test; verify. | 10 |
| `tree-sitter-cook/test/corpus/*.txt` | Replace plate/test corpus cases with body-form cases; add multi-mode coverage. | 11 |
| `standard/conformance/positive/009-test-step/` | Migrate `Cookfile` and `parse.txt` to body form. | 12 |
| `standard/conformance/positive/011-cross-recipe-bare-reference/` etc. | Migrate any positive fixture that uses `plate "…"` / `test "…"`. | 12 |
| `standard/conformance/positive/` (new) | New fixtures: `027-plate-shell-one-to-one`, `028-plate-shell-many-to-one`, `029-test-shell-one-shot`, `030-plate-lua-one-to-one`, `031-test-lua-many-to-one`, `032-plate-lua-one-shot`. | 13 |
| `standard/conformance/negative/` (new) | New fixtures: `024-plate-out-rejected`, `025-plate-mixed-in-and-all`, `026-plate-mixed-input-and-inputs`, `027-plate-lib-accessor-rejected`, `028-plate-bare-stem-rejected`, `029-plate-string-form-rejected`, `030-test-string-form-rejected`, `031-one-to-one-empty-source-rejected`. | 13 |
| `Cookfile`, `cli/Cookfile`, `tree-sitter-cook/Cookfile`, `standard/Cookfile`, `examples/*/Cookfile`, `cook_modules/*.lua`, `examples/*/cook_modules/*.lua`, `standard/cook_modules/*.lua` | Migrate `plate "cmd"` / `test "cmd"` → block form; rename body `{out}` → `{in}`. | 14 |
| `examples/iteration_benchmarks/Cookfile` | Add plate/test recipes covering each mode/form combination, parallel to the existing eight cook benchmark recipes. | 15 |
| `examples/iteration_benchmarks/README.md` | Document the new plate/test recipes. | 15 |
| `standard/VERSION` | Untouched (CS-0024 is post-v0.5; no cut here). | — |

Net: **16 tasks**.

---

# Phase A — Standard documentation (Tasks 1–3)

## Task 1: §4.7 `plate` and §4.8 `test` rewrite

**Files:**
- Modify: `standard/src/content/docs/04-recipes.mdx`

- [ ] **Step 1.1: Replace §4.7 `plate` step**

In `standard/src/content/docs/04-recipes.mdx`, find §4.7 (currently around lines 280–298). Replace the entire section with:

```mdx
## 4.7. `plate` step [#recipes.plate-step]
A `plate_step` (§{grammar-appendix.steps}, `plate_step`) is **a `cook` step with no declared outputs**. It has a body — either a `shell_block` (§{grammar-appendix.steps}) or a `using_lua_block` (§{grammar-appendix.steps}) — and no output pattern. It registers work units that perform side-effecting actions whose results do not contribute to the recipe's output list (which remains passthrough per §{xref.dep-recipe-output}).

```ebnf
plate_step ::= "plate" body NEWLINE
body       ::= shell_block | using_lua_block
```

The body grammar is identical to the body grammar of a `cook_step`'s `using_clause` (§{recipes.cook-single-output}). The placeholder vocabulary inside a `plate_step` body is specified in §{lua.shell-placeholders} (shell-block bodies) and §{lua.using-block-globals} (Lua-block bodies), with the per-step-kind admission rules of §{recipes.iteration-mode-plate-test}.

A `plate_step` admits no `using` keyword, no leading `STRING`, and no trailing modifiers. The body follows the keyword directly.

### 4.7.1. Iteration mode [#recipes.iteration-mode-plate-test]

A `plate_step` (and a `test_step`, §{recipes.test-step}) is in one of three iteration modes, determined entirely by its body's content:

| Shell-block body contains | Lua-block body references | Mode |
|---|---|---|
| `{in}` or `{in.ACCESSOR}` (no `{all}`) | `input` (no `inputs`) | **one-to-one** over source |
| `{all}` (no `{in}` / `{in.ACCESSOR}`) | `inputs` (no `input`) | **many-to-one** |
| neither | neither | **one-shot** |
| both | both | **error** (mixed iteration signal) |

The **iteration source** for a `plate_step` (and `test_step`) is the preceding `cook_step`'s output list (flattened across all units it produces), falling back to the recipe's resolved ingredients (§{recipes.ingredients}) if no `cook_step` precedes it. A step in **one-to-one** or **many-to-one** mode MUST have a non-empty source; a step in **one-shot** mode does not consult the source.

A conforming implementation MUST reject:
- A body that contains both an iteration-item placeholder/binding (`{in}` / `{in.ACCESSOR}` / `input`) and a batched-source placeholder/binding (`{all}` / `inputs`).
- A `plate_step` or `test_step` in one-to-one or many-to-one mode whose source is empty.

The Lua identifier scan operates on the same lexical text the brace-balance lexer (§{lexical.brace-blocks}) operates on — strings, comments, and long strings are excluded; only free-identifier-position references count.

### Example 4.7.1

```cook
recipe install
    ingredients "src/*.c"
    cook "build/bin/{in.stem}" using { cc {in} -o {out} }
    plate {
        install -d $PREFIX/bin
        install -m755 {in} $PREFIX/bin/{in.name}
    }
```

The `plate` step iterates over the preceding `cook` step's output list (one unit per binary), installing each binary into `$PREFIX/bin`.

### Example 4.7.2

```cook
recipe bundle
    ingredients "src/*.c"
    cook "build/bin/{in.stem}" using { cc {in} -o {out} }
    plate {
        tar -czf build/bundle.tgz {all}
    }
```

The body uses `{all}` and no `{in}`; the step is many-to-one and registers exactly one unit, with `{all}` substituted as the space-joined preceding-cook output list.

### Note 4.7.1

A `plate_step`'s body grammar matches a `cook_step`'s `using` payload, but the keyword `using` is not used. A `plate_step` has no separate output declaration to separate from the body, so a separator keyword is not needed. See rationale §{rationale.plate-test-cook-with-no-outputs}.
```

- [ ] **Step 1.2: Replace §4.8 `test` step**

Find §4.8 (currently around lines 300–331). Replace with:

```mdx
## 4.8. `test` step [#recipes.test-step]
A `test_step` (§{grammar-appendix.steps}, `test_step`) has the same body grammar as a `plate_step` (§{recipes.plate-step}) and additionally admits two trailing modifiers:

```ebnf
test_step      ::= "test" body test_modifiers NEWLINE
test_modifiers ::= ("timeout" NUMBER)? "should_fail"?
```

The body and iteration-mode rules of §{recipes.plate-step} apply verbatim. A `test_step` registers work units that report pass/fail rather than producing artifacts; iteration source, mode determination, placeholder vocabulary, and §{xref.string-substitution} cross-recipe substitution are identical to `plate_step`.

The optional modifiers MUST appear in the order shown above. A conforming implementation MUST accept all four permutations:

| Form | Behaviour |
|---|---|
| `test BODY` | Run the body unit; expect success (no Lua error, no non-zero `cook.sh`/`cook.exec`); no time bound. |
| `test BODY timeout N` | Run the body unit; expect success within `N` seconds. |
| `test BODY should_fail` | Run the body unit; expect failure (a Lua error or a non-zero `cook.sh`/`cook.exec` return); no time bound. |
| `test BODY timeout N should_fail` | Run the body unit; expect failure within `N` seconds. |

`NUMBER` is the `NUMBER` token class of §{lexical.numbers}. The upper bound on representable timeouts is implementation-defined.

### Example 4.8.1

```cook
recipe smoke
    ingredients "tests/*.c"
    cook "build/{in.stem}" using { cc {in} -o {out} }
    test { ./{in} } timeout 60
```

Each test binary is run with a 60-second time bound, and the step expects each to exit zero (no `should_fail`).

### Example 4.8.2

```cook
recipe coverage
    ingredients "src/*.c"
    cook "build/bin/{in.stem}.test" using { cc {in} -o {out} }
    test >{
        for _, bin in ipairs(inputs) do
            cook.sh("./" .. bin .. " --check")
        end
    } timeout 300
```

The Lua body references `inputs` (no `input`); the step is many-to-one and runs once with all test binaries visible as `inputs`.

### Note 4.8.1

`should_fail` is a bare word, not a quoted string. It is recognised only as the trailing modifier on a `test` line; elsewhere it has no syntactic meaning and is not a reserved word (§{lexical.keywords}). The same posture applies to `timeout`.

A `test_step` in one-to-one mode that runs a Lua body where `cook.sh`/`cook.exec` returns non-zero fails by the same rule as a Lua error; `should_fail` inverts the pass/fail decision.
```

- [ ] **Step 1.3: Verify Standard build**

```bash
cd /home/alex/dev/cook/standard
pnpm build
pnpm test
pnpm lint:keywords
```

Expected: all three pass.

- [ ] **Step 1.4: Commit**

```bash
git add standard/src/content/docs/04-recipes.mdx
git commit -m "spec(CS-0024): §4.7 plate / §4.8 test — body grammar, three modes"
```

## Task 2: §5.5 cross-recipe surface, §6.4 Lua bindings, §6.7 placeholder vocabulary, §8.1.2 phase classification

**Files:**
- Modify: `standard/src/content/docs/05-cross-recipe-references.mdx`
- Modify: `standard/src/content/docs/06-cook-lua-api.mdx`
- Modify: `standard/src/content/docs/08-execution-model.mdx`

- [ ] **Step 2.1: Update §5.5 surface list**

In `standard/src/content/docs/05-cross-recipe-references.mdx`, find the §5.5 paragraph that names the surfaces in which `{NAME}` substitution applies. Replace the phrase "in a `using`-string, `plate` command, `test` command, or bare shell" (or its current equivalent post-CS-0022) with:

```mdx
in a `cook` `using` shell block, **`plate` shell block, `test` shell block,** or bare `shell_command`
```

The surrounding paragraph wording MUST continue to specify "space-joined concatenation of the named recipe's output list (§{xref.dep-recipe-output})." Lua bodies use `cook.dep_output()` / `cook.dep_output_list()` (unchanged).

- [ ] **Step 2.2: Update §5.4 firewall paragraph**

Find the §5.4 paragraph that names "using shell block / plate command / test command" in the firewall (currently around line 57 of `05-cross-recipe-references.mdx`). Update it to:

```mdx
- **Accessor placeholder in a using-clause body or plate/test body.** A `{lib.ACCESSOR}` appearing in a `cook` `using` `shell_block` body, a `plate` body (shell or Lua), a `test` body (shell or Lua), or a bare `shell_command` MUST be rejected with a diagnostic. Plate and test steps have no output pattern and therefore can never declare a `lib`-driven driver; the rejection is universal in those bodies.
```

- [ ] **Step 2.3: Extend §6.4 binding table**

In `standard/src/content/docs/06-cook-lua-api.mdx`, locate the §6.4 binding table (around the `Example 6.4.1` heading). Append the following rows (or add a new subsection §6.4.x specifically for plate/test if the table is per-step-kind):

```mdx
### 6.4.x. Plate and test Lua-block bindings [#lua.using-block-globals-plate-test]

A Lua block in a `plate_step` or `test_step` body (§{recipes.plate-step}, §{recipes.test-step}) is bound according to the step's mode (§{recipes.iteration-mode-plate-test}):

| Binding | Mode | Type | Meaning |
|---|---|---|---|
| `input` | one-to-one | string | The current iteration item — one of the source items |
| `inputs` | many-to-one | table (1-indexed) | The full source list |

A conforming implementation MUST bind exactly one of `input` / `inputs` per unit, per the mode the body's content selected. The other is unbound (Lua `nil` on reference).

A conforming implementation MUST NOT bind `output` / `outputs` for plate or test units; these have no meaning in a step that declares no outputs. A conforming implementation MUST NOT bind `input_N` (indexed input access) for plate or test units; the source list is exposed wholesale via `inputs` in many-to-one mode.

Cross-recipe references inside a plate or test Lua body use `cook.dep_output()` / `cook.dep_output_list()`, identical to a `using >{ … }` Lua body (§{lua.using-block-globals}).
```

- [ ] **Step 2.4: Extend §6.7 placeholder vocabulary**

In `standard/src/content/docs/06-cook-lua-api.mdx`, locate §6.7 (the placeholder-vocabulary section CS-0022 rewrote). Append a new subsection §6.7.x or extend the existing table to cover plate/test shell-block bodies:

```mdx
### 6.7.x. Plate and test shell-block placeholders [#lua.shell-placeholders-plate-test]

A `plate_step` or `test_step` body that is a `shell_block` admits the following placeholders:

| Placeholder | Valid in mode | Meaning |
|---|---|---|
| `{in}` | one-to-one | The current iteration item (path) |
| `{in.ACCESSOR}` | one-to-one | `path.ACCESSOR(in)` per §{lua.path-helpers} |
| `{all}` | many-to-one | The source list, space-joined |
| `{NAME}` (NAME is a recipe in scope) | any mode | `cook.dep_output(NAME)` per §{xref.string-substitution} |
| `{TOKEN}` (none of the above) | any mode | `cook.env[TOKEN]` per §{xref.resolution} |

A conforming implementation MUST reject:
- `{in}` or `{in.ACCESSOR}` in a many-to-one or one-shot body (no current iteration);
- `{all}` in a one-to-one or one-shot body (no batched input list);
- Any `{out}` / `{out.ACCESSOR}` / `{out_N}` / `{out_N.ACCESSOR}` token in a plate or test body — the step declares no outputs. The diagnostic MUST point to `{in}` as the iteration item.
- `{NAME.ACCESSOR}` for any recipe `NAME` in scope (preserves the §{xref.dep-driven} firewall).

Bare path-accessors (`{stem}`, `{name}`, `{ext}`, `{dir}`) in a plate or test body fall through to `cook.env[TOKEN]` per §{xref.resolution} step 4. A conforming implementation MUST emit a specific diagnostic for these four names that points to `{in.ACCESSOR}` (same diagnostic CS-0022 introduced for cook bodies).
```

- [ ] **Step 2.5: Extend §8.1.2 phase classification**

In `standard/src/content/docs/08-execution-model.mdx`, locate §8.1.2 (the phase-classification table). Append rows for plate/test body forms:

```mdx
| `plate { … }` (shell block) | register (substitution) → execute (shell) | declarative |
| `plate >{ … }` (Lua block)  | register (binding setup) → execute (Lua)  | declarative |
| `test { … }` (shell block)  | register (substitution) → execute (shell) | declarative |
| `test >{ … }` (Lua block)   | register (binding setup) → execute (Lua)  | declarative |
```

The §8.3 step-group rule (one step group per plate / per test) is preserved verbatim.

- [ ] **Step 2.6: Verify Standard build**

```bash
cd /home/alex/dev/cook/standard
pnpm build
pnpm test
pnpm lint:keywords
```

Expected: all three pass.

- [ ] **Step 2.7: Commit**

```bash
git add standard/src/content/docs/05-cross-recipe-references.mdx \
        standard/src/content/docs/06-cook-lua-api.mdx \
        standard/src/content/docs/08-execution-model.mdx
git commit -m "spec(CS-0024): §5.5 surface / §5.4 firewall / §6.4 bindings / §6.7 placeholders / §8.1.2 phases for plate/test"
```

## Task 3: App. A grammar deltas, App. B rationale rewrite

**Files:**
- Modify: `standard/src/content/docs/appendix/A-grammar.mdx`
- Modify: `standard/src/content/docs/appendix/B-rationale.mdx`

- [ ] **Step 3.1: Update A.4 productions**

In `standard/src/content/docs/appendix/A-grammar.mdx` at A.4 (around lines 109 and 111), replace the existing productions:

```ebnf
plate_step            ::= "plate" STRING NEWLINE

test_step             ::= "test" STRING ("timeout" NUMBER)? "should_fail"? NEWLINE
```

with:

```ebnf
plate_step            ::= "plate" body NEWLINE
test_step             ::= "test"  body test_modifiers NEWLINE
body                  ::= shell_block | using_lua_block
test_modifiers        ::= ("timeout" NUMBER)? "should_fail"?
```

The `shell_block` and `using_lua_block` productions defined in the cook-step area are reused as-is.

The "Step-dispatch priority" list (entries 3 and 4) is unchanged. The "Chore step-kind ban" paragraph (around line 92) keeps `plate_step` and `test_step` in its banned list — both keep their declarative-region status, both stay banned in chore bodies.

- [ ] **Step 3.2: Add CS-0024 normative paragraph**

After the "Iteration coherence (CS-0022)" paragraph in A.4 (around line 140), add:

```mdx
**Plate/test mode coherence (CS-0024).** A `plate_step` or `test_step` body MUST NOT contain both an iteration-item placeholder/binding and a batched-source placeholder/binding. For shell-block bodies, this means a body MUST NOT contain both `{in}` (or `{in.ACCESSOR}`) and `{all}`. For Lua-block bodies, the body MUST NOT reference both `input` and `inputs` (free-identifier scan, ignoring strings/comments/long-strings per §{lexical.brace-blocks}). A conforming implementation MUST reject a body that violates this rule with a diagnostic naming both tokens. See §{recipes.iteration-mode-plate-test}.

**Plate/test source presence (CS-0024).** A `plate_step` or `test_step` in one-to-one or many-to-one mode MUST have a non-empty iteration source — that is, the recipe must contain a preceding `cook_step` whose output list is non-empty, or an `ingredients_step` whose resolved path list is non-empty. A conforming implementation MUST reject a `plate_step` or `test_step` whose mode requires source consultation but whose recipe provides none. A `plate_step` or `test_step` in one-shot mode does not consult the source and MAY appear in a recipe with neither.
```

- [ ] **Step 3.3: Delete B.4.7 and add CS-0024 rationale subsections**

In `standard/src/content/docs/appendix/B-rationale.mdx`:

1. **Delete** §B.4.7 ("Why `plate` is one command template, not a list", currently around lines 102–107). Its premise — that richer plate forms require a Lua block — is invalidated by CS-0024.

2. Add a new subsection. Insert after the deletion site:

```mdx
### B.4.7. Why `plate` and `test` are cook steps with no declared outputs [#rationale.plate-test-cook-with-no-outputs]
Once cook gained block bodies and three iteration modes (CS-0022), the only structural difference between a cook step and a plate or test step was the presence of an output declaration. Modeling plate and test as "cook with no outputs" makes the body grammar uniform, the placeholder vocabulary uniform, the cross-recipe substitution rules uniform, and the substitution timing uniform. The author's mental model collapses from three step kinds with three surfaces to three step kinds with one surface.

The two keywords survive because they signal author intent that a single keyword would lose: a reader scanning a recipe sees `test` and knows "this is a validation gate." `test` additionally carries genuine extra structure (`timeout` and `should_fail` modifiers) that a generic "cook with no outputs" form would not.

The `using` keyword does not appear on plate or test lines. Cook earns it because its line carries two things — output pattern list and body — that need a separator. Plate and test have only a body; a `using` keyword would be ceremony with no semantic load.

### B.4.8. Why iteration mode is deduced from the body for plate and test [#rationale.plate-test-body-deduction]
Cook owns iteration mode via the output pattern (CS-0022) because the output pattern is *declarative*: it states the unit's filenames and falls out the iteration shape. Plate and test have no outputs to declare, so the body's placeholder content is the only available signal. The CS-0022 footgun — an output-pattern signal contradicting a body signal — cannot occur in a step that has no output pattern. Deducing mode from body content is therefore sound for plate and test, even though it would be incoherent for cook.

The deduction is purely syntactic: it scans the body's text (shell) or its free-identifier references (Lua) at register time. It does not require Lua evaluation. The alternative — letting the runtime decide which binding to populate based on which one was named — would require dynamic mode selection, breaking the static-shape DAG `cook.add_unit` produces.

### B.4.9. Why `{out}` is rejected in plate and test bodies [#rationale.plate-test-out-rejected]
CS-0022 fixed `{out}` to mean the unit's declared output. Plate and test declare no outputs, so `{out}` has no referent. The pre-CS-0024 plate surface used `{out}` to mean the iteration item — but that is what `{in}` means everywhere else in the language. A per-step-kind name for "the iteration item" would re-introduce the position-dependent rule that CS-0022 paid to remove.

### B.4.10. Why `{lib.ACCESSOR}` is rejected in plate and test bodies [#rationale.plate-test-lib-accessor-rejected]
Plate and test have no output pattern and therefore cannot declare a `lib`-driven iteration. §{xref.dep-driven}'s firewall — `{lib.ACCESSOR}` rejected in any using-clause body — applies trivially to plate and test bodies: there is never a position in such a body where `{lib.ACCESSOR}` could be valid. A bare `{lib}` reference is admitted and substitutes per §{xref.string-substitution}.

### B.4.11. Why a static Lua identifier scan, not a runtime decision [#rationale.plate-test-lua-static-scan]
A Lua body that references both `input` and `inputs` is making contradictory claims about iteration shape. Detecting that at register time produces a sharp diagnostic with a line number; deferring it to "the runtime decides which binding is bound based on which one was named" would either require dynamic mode selection (incompatible with the static-shape DAG) or two distinct mode-resolution passes (one syntactic for shell, one runtime for Lua). One pass, applied uniformly, is the better trade.

The scan is whole-word, identifier-position. A property-name suffix (`cook.input`, `obj:input(...)`) is not a reference to the binding because the Lua grammar resolves the name as a field access. The scan respects the existing string/comment/long-string ignore rules of §{lexical.brace-blocks}.
```

3. Renumber any later B.4.x entries if needed (none should — B.4.7 is the last one in the §B.4 series before §B.5; verify by reading the file end-to-end).

- [ ] **Step 3.4: Verify Standard build**

```bash
cd /home/alex/dev/cook/standard
pnpm build
pnpm test
pnpm lint:keywords
```

Expected: all three pass. The bare-ref-lint and clause-anchors will catch broken `§{...}` references — fix any that report.

- [ ] **Step 3.5: Commit**

```bash
git add standard/src/content/docs/appendix/A-grammar.mdx \
        standard/src/content/docs/appendix/B-rationale.mdx
git commit -m "spec(CS-0024): App. A productions / App. B rationale for plate-test unification"
```

---

# Phase B — Parser (Tasks 4–6)

## Task 4: AST — `Body` enum, restructure `PlateStep` and `TestStep`

**Files:**
- Modify: `cli/crates/cook-lang/src/ast.rs`

- [ ] **Step 4.1: Introduce a `Body` type alias**

In `cli/crates/cook-lang/src/ast.rs`, immediately after the existing `UsingClause` definition (around line 49), add:

```rust
/// CS-0024: a step body — same grammar as `using_clause`'s payload.
/// Used by `cook_step` (via `UsingClause`), `plate_step`, and `test_step`.
/// Aliased to `UsingClause` so the codegen can share substitution / mode
/// detection helpers without duplicating the enum.
pub type Body = UsingClause;
```

- [ ] **Step 4.2: Replace `PlateStep`**

Find the `PlateStep` struct (around lines 60–63):

```rust
#[derive(Debug, Clone, PartialEq)]
pub struct PlateStep {
    pub command: String,
}
```

Replace with:

```rust
#[derive(Debug, Clone, PartialEq)]
pub struct PlateStep {
    pub body: Body,
}
```

- [ ] **Step 4.3: Replace `TestStep`**

Find the `TestStep` struct (around lines 65–70):

```rust
#[derive(Debug, Clone, PartialEq)]
pub struct TestStep {
    pub command: String,
    pub timeout: Option<u64>,
    pub should_fail: bool,
}
```

Replace with:

```rust
#[derive(Debug, Clone, PartialEq)]
pub struct TestStep {
    pub body: Body,
    pub timeout: Option<u64>,
    pub should_fail: bool,
}
```

- [ ] **Step 4.4: Update the in-file `tests` module**

The `#[cfg(test)] mod tests` block at the bottom of `ast.rs` (around lines 104+) constructs `PlateStep` / `TestStep` literals. Update each one to use `body: Body::ShellBlock(vec!["…".into()])` instead of `command: "…".into()`.

For example, `test_plate_step` (currently at line ~172) becomes:

```rust
#[test]
fn test_plate_step() {
    let _step = PlateStep {
        body: Body::ShellBlock(vec!["./{in}".to_string()]),
    };
}
```

- [ ] **Step 4.5: Compile**

```bash
cd /home/alex/dev/cook
cargo build -p cook-lang
```

Expected: the `cook-lang` crate builds. Errors at this point are **expected** in dependent crates (`cook-luagen`, etc.) — those land in later tasks.

- [ ] **Step 4.6: Commit**

```bash
git add cli/crates/cook-lang/src/ast.rs
git commit -m "ast(CS-0024): plate/test bodies share UsingClause via Body alias

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

## Task 5: Parser — plate/test arms parse bodies

**Files:**
- Modify: `cli/crates/cook-lang/src/recipe.rs`
- Modify: `cli/crates/cook-lang/src/cook_line.rs`

- [ ] **Step 5.1: Factor body-payload parsing out of `cook_line.rs`**

In `cli/crates/cook-lang/src/cook_line.rs`, locate the using-payload dispatch (lines 158–217). Extract the dispatch logic — the `>>{` / `>{` / `{` / `"` / fallthrough cascade — into a new public helper. Add a new function below the existing `parse_cook_line`:

```rust
/// Parse a body payload — the `body` production from App. A.4 (CS-0024).
///
/// `after_kw` is the line text after the body's introducer keyword
/// (`using` for cook, the empty string for plate/test). `kw_for_diag` is
/// the keyword name used in error messages (`cook using`, `plate`, `test`).
///
/// Returns the parsed `Body` plus the new token-stream position.
pub(crate) fn parse_body_payload(
    after_kw: &str,
    line: usize,
    tokens: &[crate::lexer::TokenWithLine],
    current_pos: usize,
    source_lines: &[String],
    kw_for_diag: &str,
) -> Result<(crate::ast::Body, usize), ParseError> {
    let trimmed = after_kw.trim_start();

    if trimmed.starts_with(">>{") {
        return Err(ParseError::Parse {
            line,
            message: format!(
                "{}: `>>{{ … }}` (register-phase Lua block) is not a valid body — use `>{{ … }}` for an execute-phase Lua block",
                kw_for_diag
            ),
        });
    }

    if trimmed.starts_with(">{") {
        let (code, new_pos) = collect_lua_block(line, tokens, current_pos + 1, source_lines)?;
        return Ok((crate::ast::Body::LuaBlock(code), new_pos));
    }

    if trimmed.starts_with('{') {
        let after_open = &trimmed[1..];
        if let Some(commands) = crate::shell_block::try_inline_shell_block(after_open) {
            let mut new_pos = current_pos + 1;
            while new_pos < tokens.len() && tokens[new_pos].line <= line {
                new_pos += 1;
            }
            return Ok((crate::ast::Body::ShellBlock(commands), new_pos));
        }
        let (commands, new_pos) =
            crate::shell_block::collect_shell_block(line, tokens, current_pos + 1, source_lines)?;
        return Ok((crate::ast::Body::ShellBlock(commands), new_pos));
    }

    if trimmed.starts_with('"') {
        return Err(ParseError::Parse {
            line,
            message: format!(
                "{}: the bare-string form `\"cmd\"` was removed in CS-0024; rewrite as `{{ cmd }}` (one-line shell block)",
                kw_for_diag
            ),
        });
    }

    Err(ParseError::Parse {
        line,
        message: format!(
            "{}: expected `>{{ Lua block }}` or `{{ shell block }}`, found: {}",
            kw_for_diag, trimmed
        ),
    })
}
```

Then refactor the existing `parse_cook_line` (around lines 158–217) to call `parse_body_payload(after_using, line, tokens, current_pos, source_lines, "cook using")` instead of inlining the dispatch. The cook-side migration diagnostic stays in this helper — both cook and plate/test issue the same migration message.

(Note: `crate::lexer::TokenWithLine` is the existing token type — verify the actual type alias name in `lexer.rs` and adjust the function signature if needed. The same applies to `collect_lua_block`'s import path.)

- [ ] **Step 5.2: Replace the plate parser arm in `recipe.rs`**

In `cli/crates/cook-lang/src/recipe.rs`, find the plate arm (around lines 200–208):

```rust
} else if let Some(rest) = strip_keyword(text, "plate") {
    if let Some(started) = imperative_began {
        return Err(region_violation("plate", tok.line, started));
    }
    let command = parse_single_quoted_string(rest, tok.line)?;
    steps.push(Step::Plate {
        step: PlateStep { command },
        line: tok.line,
    });
}
```

Replace with:

```rust
} else if let Some(rest) = strip_keyword(text, "plate") {
    if let Some(started) = imperative_began {
        return Err(region_violation("plate", tok.line, started));
    }
    let (body, new_pos) = crate::cook_line::parse_body_payload(
        rest, tok.line, tokens, pos, source_lines, "plate",
    )?;
    steps.push(Step::Plate {
        step: PlateStep { body },
        line: tok.line,
    });
    pos = new_pos;
    continue;
}
```

- [ ] **Step 5.3: Replace the test parser arm in `recipe.rs`**

Find the test arm (around lines 209–223):

```rust
} else if let Some(rest) = strip_keyword(text, "test") {
    if let Some(started) = imperative_began {
        return Err(region_violation("test", tok.line, started));
    }
    let (command, rest) = parse_test_command(rest, tok.line)?;
    let (timeout, rest) = parse_test_timeout(rest);
    let should_fail = rest.trim() == "should_fail";
    steps.push(Step::Test {
        step: TestStep {
            command,
            timeout,
            should_fail,
        },
        line: tok.line,
    });
}
```

Replace with:

```rust
} else if let Some(rest) = strip_keyword(text, "test") {
    if let Some(started) = imperative_began {
        return Err(region_violation("test", tok.line, started));
    }
    // Parse the body first; the trailing modifiers come after the closing
    // brace of the body (or its inline form). For inline shell blocks /
    // single-line Lua blocks the modifiers are on the same source line; for
    // multi-line bodies they are on the closing-`}` line.
    let (body, new_pos) = crate::cook_line::parse_body_payload(
        rest, tok.line, tokens, pos, source_lines, "test",
    )?;
    // Reach past the closing brace and read the trailing-modifier text.
    let modifier_line = if new_pos > 0 && new_pos <= tokens.len() {
        // The token at new_pos - 1 is the last consumed token of the body.
        // For multi-line bodies, modifiers may follow on its line. We rely
        // on the lexer producing modifier text in the same Token::Content
        // (single-line) or as a fragment on the closing-`}` line.
        match tokens.get(new_pos - 1) {
            Some(t) => &source_lines[t.line.saturating_sub(1)],
            None => "",
        }
    } else {
        ""
    };
    let modifier_tail = parse_test_modifier_tail(modifier_line, tok.line)?;
    steps.push(Step::Test {
        step: TestStep {
            body,
            timeout: modifier_tail.timeout,
            should_fail: modifier_tail.should_fail,
        },
        line: tok.line,
    });
    pos = new_pos;
    continue;
}
```

(Note: this introduces `parse_test_modifier_tail` — Step 5.4 defines it.)

- [ ] **Step 5.4: Add `parse_test_modifier_tail` helper**

In `cli/crates/cook-lang/src/recipe.rs`, after the existing `parse_test_command` and `parse_test_timeout` helpers (search for `fn parse_test_command` to locate), replace those two helpers with a new single helper that operates on the modifier-suffix text following the closing brace. The helper extracts the substring after the rightmost `}` on the line (the one that closes the body's inline form, or the closing-line of a multi-line body) and parses `timeout N` and `should_fail` from it.

```rust
struct TestModifierTail {
    timeout: Option<u64>,
    should_fail: bool,
}

/// Parse the trailing modifier suffix on a `test` line — the text that
/// follows the closing brace of the body. Accepts:
///     timeout N
///     should_fail
///     timeout N should_fail
///     <empty>
/// in any of the four orderings the spec admits (only the four listed in
/// §4.8 are valid; the implementation rejects others).
fn parse_test_modifier_tail(line_text: &str, line: usize) -> Result<TestModifierTail, ParseError> {
    // Find the closing `}` and take everything after it.
    let suffix = match line_text.rfind('}') {
        Some(idx) => &line_text[idx + 1..],
        None => "",
    };
    let trimmed = suffix.trim();

    let mut timeout: Option<u64> = None;
    let mut should_fail = false;
    let mut tokens = trimmed.split_whitespace().peekable();

    while let Some(tok) = tokens.next() {
        if tok == "timeout" {
            let n = tokens.next().ok_or_else(|| ParseError::Parse {
                line,
                message: "test: `timeout` requires a numeric argument".to_string(),
            })?;
            timeout = Some(n.parse().map_err(|_| ParseError::Parse {
                line,
                message: format!("test: invalid timeout value: {}", n),
            })?);
        } else if tok == "should_fail" {
            should_fail = true;
        } else {
            return Err(ParseError::Parse {
                line,
                message: format!("test: unexpected modifier `{}`", tok),
            });
        }
    }

    Ok(TestModifierTail {
        timeout,
        should_fail,
    })
}
```

The previous `parse_test_command` and `parse_test_timeout` helpers can be removed entirely; `parse_single_quoted_string`'s use sites for plate/test are also gone (it remains used elsewhere — search uses before deletion).

- [ ] **Step 5.5: Update imports in `recipe.rs`**

The `recipe.rs` file imports may have changed. Verify:

```rust
use crate::ast::{Body, /* … */};
```

is present and the now-unused `parse_single_quoted_string` import (if any) is removed.

- [ ] **Step 5.6: Compile and run unit tests**

```bash
cd /home/alex/dev/cook
cargo build -p cook-lang
cargo test -p cook-lang --lib
```

Expected: `cook-lang` library builds. Some tests in `tests.rs` will fail because they exercise the old `PlateStep`/`TestStep::command` field — these are fixed in Task 6.

- [ ] **Step 5.7: Commit**

```bash
git add cli/crates/cook-lang/src/recipe.rs cli/crates/cook-lang/src/cook_line.rs
git commit -m "parser(CS-0024): plate/test parse bodies via shared parse_body_payload

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

## Task 6: Parser tests + conformance formatter

**Files:**
- Modify: `cli/crates/cook-lang/src/tests.rs`
- Modify: `cli/crates/cook-lang/tests/conformance.rs`

- [ ] **Step 6.1: Update `test_plate_step`**

In `cli/crates/cook-lang/src/tests.rs`, find `test_plate_step` (around line 259):

```rust
fn test_plate_step() {
    let source = "recipe \"test\"\n    ingredients \"tests/*.c\"\n    cook \"build/{stem}\" using {\n        cc {in} -o {out}\n    }\n    plate \"./{out}\"\n";
    /* … */
}
```

Replace with a body-form fixture:

```rust
#[test]
fn test_plate_step() {
    let source = "recipe test_recipe\n    ingredients \"tests/*.c\"\n    cook \"build/{in.stem}\" using {\n        cc {in} -o {out}\n    }\n    plate {\n        ./{in}\n    }\n";
    let cookfile = parse(source).expect("should parse");
    let recipe = &cookfile.recipes[0];
    assert_eq!(recipe.steps.len(), 3);
    match &recipe.steps[2] {
        Step::Plate { step, .. } => match &step.body {
            Body::ShellBlock(lines) => {
                assert_eq!(lines.len(), 1);
                assert_eq!(lines[0].trim(), "./{in}");
            }
            other => panic!("expected ShellBlock, got {:?}", other),
        },
        other => panic!("expected Plate step, got {:?}", other),
    }
}
```

- [ ] **Step 6.2: Update `test_test_step` and friends**

Find any `test_test_step_*` tests in `tests.rs`. Convert each to a body fixture. For `test_test_step_with_should_fail`:

```rust
#[test]
fn test_test_step_with_should_fail() {
    let source = "recipe r\n    ingredients \"tests/*.c\"\n    cook \"build/{in.stem}\" using { cc {in} -o {out} }\n    test { ./{in} } should_fail\n";
    let cookfile = parse(source).expect("should parse");
    match &cookfile.recipes[0].steps[2] {
        Step::Test { step, .. } => {
            assert!(matches!(step.body, Body::ShellBlock(_)));
            assert!(step.should_fail);
            assert_eq!(step.timeout, None);
        }
        other => panic!("expected Test, got {:?}", other),
    }
}
```

For `test_test_step_with_timeout_and_should_fail`:

```rust
#[test]
fn test_test_step_with_timeout_and_should_fail() {
    let source = "recipe r\n    ingredients \"tests/*.c\"\n    cook \"build/{in.stem}\" using { cc {in} -o {out} }\n    test { ./{in} } timeout 60 should_fail\n";
    let cookfile = parse(source).expect("should parse");
    match &cookfile.recipes[0].steps[2] {
        Step::Test { step, .. } => {
            assert!(matches!(step.body, Body::ShellBlock(_)));
            assert!(step.should_fail);
            assert_eq!(step.timeout, Some(60));
        }
        other => panic!("expected Test, got {:?}", other),
    }
}
```

Add similar coverage for `timeout`-only and the bare `test { … }` (no modifiers) form.

- [ ] **Step 6.3: Add migration-diagnostic tests**

After the existing plate/test tests, add:

```rust
#[test]
fn test_plate_string_form_rejected() {
    let source = "recipe r\n    ingredients \"tests/*.c\"\n    cook \"build/{in.stem}\" using { cc {in} -o {out} }\n    plate \"./{out}\"\n";
    let err = parse(source).unwrap_err();
    let msg = format!("{}", err);
    assert!(
        msg.contains("plate") && msg.contains("CS-0024") && msg.contains("{ cmd }"),
        "expected migration diagnostic for plate string form, got: {}",
        msg
    );
}

#[test]
fn test_test_string_form_rejected() {
    let source = "recipe r\n    ingredients \"tests/*.c\"\n    cook \"build/{in.stem}\" using { cc {in} -o {out} }\n    test \"./{out}\" timeout 60\n";
    let err = parse(source).unwrap_err();
    let msg = format!("{}", err);
    assert!(
        msg.contains("test") && msg.contains("CS-0024") && msg.contains("{ cmd }"),
        "expected migration diagnostic for test string form, got: {}",
        msg
    );
}

#[test]
fn test_plate_lua_block_parses() {
    let source = "recipe r\n    ingredients \"src/*.c\"\n    cook \"build/{in.stem}\" using { cc {in} -o {out} }\n    plate >{\n        cook.sh(\"strip \" .. input)\n    }\n";
    let cookfile = parse(source).expect("should parse");
    match &cookfile.recipes[0].steps[2] {
        Step::Plate { step, .. } => assert!(matches!(step.body, Body::LuaBlock(_))),
        other => panic!("expected Plate Lua, got {:?}", other),
    }
}
```

- [ ] **Step 6.4: Update legacy fixture-references**

Search for any other tests in `tests.rs` that reference `PlateStep { command: …` or `TestStep { command: …`:

```bash
grep -n "PlateStep { command\|TestStep { command" /home/alex/dev/cook/cli/crates/cook-lang/src/tests.rs
```

Convert each to body form. The `test_chore_with_plate_rejected` fixture (line ~875) likely just needs its source string updated to body form.

- [ ] **Step 6.5: Update the conformance formatter**

In `cli/crates/cook-lang/tests/conformance.rs`, locate any `repr_plate_step` / `repr_test_step` / `repr_step` arms that format plate/test. The current formatter likely emits `plate "CMD"`; update it to emit the body form:

```rust
fn repr_plate_step(step: &PlateStep) -> String {
    match &step.body {
        Body::ShellBlock(lines) if lines.len() == 1 => format!("plate {{ {} }}", lines[0].trim()),
        Body::ShellBlock(lines) => {
            let mut s = String::from("plate {\n");
            for line in lines {
                s.push_str("    ");
                s.push_str(line);
                s.push('\n');
            }
            s.push('}');
            s
        }
        Body::LuaBlock(code) => format!("plate >{{\n{}\n}}", code),
    }
}

fn repr_test_step(step: &TestStep) -> String {
    let body = match &step.body {
        Body::ShellBlock(lines) if lines.len() == 1 => format!("{{ {} }}", lines[0].trim()),
        Body::ShellBlock(lines) => {
            let mut s = String::from("{\n");
            for line in lines {
                s.push_str("    ");
                s.push_str(line);
                s.push('\n');
            }
            s.push('}');
            s
        }
        Body::LuaBlock(code) => format!(">{{\n{}\n}}", code),
    };
    let mut s = format!("test {}", body);
    if let Some(t) = step.timeout {
        s.push_str(&format!(" timeout {}", t));
    }
    if step.should_fail {
        s.push_str(" should_fail");
    }
    s
}
```

If the formatter shape in the actual file differs, adapt — the goal is canonical body-form re-emission.

- [ ] **Step 6.6: Run tests**

```bash
cd /home/alex/dev/cook
cargo test -p cook-lang
```

Expected: all `cook-lang` unit tests pass. If conformance fixtures still use the old plate/test form, the conformance harness fails — that's expected and fixed in Task 12.

- [ ] **Step 6.7: Commit**

```bash
git add cli/crates/cook-lang/src/tests.rs cli/crates/cook-lang/tests/conformance.rs
git commit -m "test(CS-0024): plate/test parser tests + canonical re-emission of bodies

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

# Phase C — Codegen (Tasks 7–9)

## Task 7: Codegen template — mode detection + placeholder validation for plate/test

**Files:**
- Modify: `cli/crates/cook-luagen/src/template.rs`

- [ ] **Step 7.1: Add the mode-detection helper**

In `cli/crates/cook-luagen/src/template.rs`, after the existing `expand_with_deps_fallback` (around line 335), add:

```rust
use cook_lang::ast::Body;

/// CS-0024 §3.4: the iteration mode of a plate/test step body.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PlateTestMode {
    /// Body references {in}/{in.X} (shell) or `input` (Lua), and not the
    /// batched form. One unit per source item.
    OneToOne,
    /// Body references {all} (shell) or `inputs` (Lua), and not the
    /// per-item form. Exactly one unit, full source visible.
    ManyToOne,
    /// Body references neither. Exactly one unit, source not consulted.
    OneShot,
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum PlateTestModeError {
    #[error("body contains both per-item and batched references — `{0}` and `{1}` cannot both appear")]
    Mixed(&'static str, &'static str),
}

pub(crate) fn detect_plate_test_mode(body: &Body) -> Result<PlateTestMode, PlateTestModeError> {
    match body {
        Body::ShellBlock(lines) => {
            let joined: String = lines.join("\n");
            let has_in = body_text_has_in_placeholder(&joined);
            let has_all = body_text_has_token(&joined, "all");
            match (has_in, has_all) {
                (true, true) => Err(PlateTestModeError::Mixed("{in}", "{all}")),
                (true, false) => Ok(PlateTestMode::OneToOne),
                (false, true) => Ok(PlateTestMode::ManyToOne),
                (false, false) => Ok(PlateTestMode::OneShot),
            }
        }
        Body::LuaBlock(code) => {
            let has_input = lua_has_free_identifier(code, "input");
            let has_inputs = lua_has_free_identifier(code, "inputs");
            match (has_input, has_inputs) {
                (true, true) => Err(PlateTestModeError::Mixed("input", "inputs")),
                (true, false) => Ok(PlateTestMode::OneToOne),
                (false, true) => Ok(PlateTestMode::ManyToOne),
                (false, false) => Ok(PlateTestMode::OneShot),
            }
        }
    }
}

/// Scan a shell-body text for any `{in}` or `{in.ACCESSOR}` placeholder.
fn body_text_has_in_placeholder(text: &str) -> bool {
    let mut rest = text;
    while let Some(open) = rest.find('{') {
        let after = &rest[open + 1..];
        if let Some(close) = after.find('}') {
            let inner = &after[..close];
            if inner == "in" || inner.starts_with("in.") {
                return true;
            }
            rest = &after[close + 1..];
        } else {
            break;
        }
    }
    false
}

/// Scan a shell-body text for `{TOKEN}` literally equal to `token`.
fn body_text_has_token(text: &str, token: &str) -> bool {
    let mut rest = text;
    while let Some(open) = rest.find('{') {
        let after = &rest[open + 1..];
        if let Some(close) = after.find('}') {
            let inner = &after[..close];
            if inner == token {
                return true;
            }
            rest = &after[close + 1..];
        } else {
            break;
        }
    }
    false
}

/// Scan a Lua source text for a free-identifier reference to `name`.
///
/// Skips:
/// - text inside `"…"` and `'…'` short strings (with `\` escape rules);
/// - text inside `[[…]]` long strings (any `=` count between brackets);
/// - text inside `--` line comments and `--[[…]]` block comments;
/// - identifier-name positions immediately preceded by `.` or `:` (these
///   are property/method accesses, not free identifiers).
///
/// The scan recognises `name` only as a whole-word identifier, bordered
/// by Lua identifier-character boundaries.
fn lua_has_free_identifier(code: &str, name: &str) -> bool {
    let bytes = code.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];

        // Skip line comments: `-- … <newline>`.
        if b == b'-' && i + 1 < bytes.len() && bytes[i + 1] == b'-' {
            // Long comment: `--[[ … ]]` or `--[==[ … ]==]`.
            if i + 2 < bytes.len() && bytes[i + 2] == b'[' {
                let (eq_count, after_open) = count_long_bracket_eqs(&bytes[i + 3..]);
                if let Some(after_open_pos) = after_open {
                    let close_marker = format!("]{}]", "=".repeat(eq_count));
                    if let Some(rel) = code[i + 3 + after_open_pos..].find(&close_marker) {
                        i = i + 3 + after_open_pos + rel + close_marker.len();
                        continue;
                    } else {
                        return false; // unterminated — treat as unscannable
                    }
                }
            }
            // Line comment.
            while i < bytes.len() && bytes[i] != b'\n' {
                i += 1;
            }
            continue;
        }

        // Skip short strings.
        if b == b'"' || b == b'\'' {
            let quote = b;
            i += 1;
            while i < bytes.len() && bytes[i] != quote {
                if bytes[i] == b'\\' && i + 1 < bytes.len() {
                    i += 2;
                } else {
                    i += 1;
                }
            }
            i += 1; // skip closing quote
            continue;
        }

        // Skip long strings: `[[ … ]]` or `[==[ … ]==]`.
        if b == b'[' {
            let (eq_count, after_open) = count_long_bracket_eqs(&bytes[i + 1..]);
            if let Some(after_open_pos) = after_open {
                let close_marker = format!("]{}]", "=".repeat(eq_count));
                if let Some(rel) = code[i + 1 + after_open_pos..].find(&close_marker) {
                    i = i + 1 + after_open_pos + rel + close_marker.len();
                    continue;
                } else {
                    return false;
                }
            }
        }

        // Identifier match.
        if is_lua_ident_start(b) {
            let ident_start = i;
            while i < bytes.len() && is_lua_ident_cont(bytes[i]) {
                i += 1;
            }
            // Check property-access suffix: was the char immediately before
            // ident_start a `.` or `:`?
            let before_is_field_access = ident_start > 0
                && (bytes[ident_start - 1] == b'.' || bytes[ident_start - 1] == b':');
            if !before_is_field_access && &code[ident_start..i] == name {
                return true;
            }
            continue;
        }

        i += 1;
    }
    false
}

/// Helper: at byte position `bytes[0]` we're past the leading `[`. If the
/// next chars are `=*[`, we have a long-bracket open. Returns
/// (equality count, byte offset just past the second `[`).
fn count_long_bracket_eqs(bytes: &[u8]) -> (usize, Option<usize>) {
    let mut eq = 0;
    while eq < bytes.len() && bytes[eq] == b'=' {
        eq += 1;
    }
    if eq < bytes.len() && bytes[eq] == b'[' {
        (eq, Some(eq + 1))
    } else {
        (0, None)
    }
}

fn is_lua_ident_start(b: u8) -> bool {
    b.is_ascii_alphabetic() || b == b'_'
}

fn is_lua_ident_cont(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}
```

- [ ] **Step 7.2: Add the placeholder validator**

After the mode-detection code, add:

```rust
#[derive(Debug, thiserror::Error)]
pub(crate) enum PlateTestPlaceholderError {
    #[error("`{token}` is not valid in {mode_name} mode (line text: `{line}`)")]
    BadPlaceholder { token: String, mode_name: String, line: String },
    #[error("`{token}` is not valid in a plate or test body — the iteration item is `{{in}}`")]
    OutForbidden { token: String },
    #[error("bare path-accessor `{{{accessor}}}` is no longer valid; use `{{in.{accessor}}}` (CS-0022/CS-0024)")]
    BareAccessor { accessor: String },
    #[error("`{{{name}.{accessor}}}` is not valid in a plate or test body (the §5.4 firewall applies — plate/test have no output pattern)")]
    LibAccessor { name: String, accessor: String },
}

pub(crate) fn validate_plate_test_placeholders(
    body: &Body,
    mode: PlateTestMode,
    recipe_names: &BTreeSet<String>,
) -> Result<(), PlateTestPlaceholderError> {
    if let Body::ShellBlock(lines) = body {
        for line in lines {
            let mut rest = line.as_str();
            while let Some(open) = rest.find('{') {
                let after = &rest[open + 1..];
                if let Some(close) = after.find('}') {
                    let inner = &after[..close];
                    validate_token(inner, mode, line, recipe_names)?;
                    rest = &after[close + 1..];
                } else {
                    break;
                }
            }
        }
    }
    // Lua bodies validate per the binding rules of §6.4.x at runtime; the
    // mode-deduction's mixed-binding check (Task 7.1) is sufficient for
    // load-time rejection of contradictory bindings. Stray `output` /
    // `outputs` references in a Lua body raise a Lua nil-error at execute,
    // which is the natural Lua failure mode.
    Ok(())
}

fn validate_token(
    inner: &str,
    mode: PlateTestMode,
    line: &str,
    recipe_names: &BTreeSet<String>,
) -> Result<(), PlateTestPlaceholderError> {
    // {out}, {out_N}, {out.X}, {out_N.X}: all rejected.
    if inner == "out"
        || inner.starts_with("out.")
        || (inner.starts_with("out_") && inner[4..].chars().next().map_or(false, |c| c.is_ascii_digit()))
    {
        return Err(PlateTestPlaceholderError::OutForbidden {
            token: format!("{{{}}}", inner),
        });
    }

    // {in} or {in.X}: must be in OneToOne.
    if inner == "in" || inner.starts_with("in.") {
        if mode != PlateTestMode::OneToOne {
            return Err(PlateTestPlaceholderError::BadPlaceholder {
                token: format!("{{{}}}", inner),
                mode_name: format!("{:?}", mode),
                line: line.to_string(),
            });
        }
        return Ok(());
    }

    // {all}: must be in ManyToOne.
    if inner == "all" {
        if mode != PlateTestMode::ManyToOne {
            return Err(PlateTestPlaceholderError::BadPlaceholder {
                token: "{all}".to_string(),
                mode_name: format!("{:?}", mode),
                line: line.to_string(),
            });
        }
        return Ok(());
    }

    // Bare path-accessor: rejected.
    if matches!(inner, "stem" | "name" | "ext" | "dir") {
        return Err(PlateTestPlaceholderError::BareAccessor {
            accessor: inner.to_string(),
        });
    }

    // {NAME.ACCESSOR} where NAME is a recipe in scope: rejected (§5.4 firewall).
    if let Some((prefix, suffix)) = inner.rsplit_once('.') {
        if recipe_names.contains(prefix) && matches!(suffix, "stem" | "name" | "ext" | "dir") {
            return Err(PlateTestPlaceholderError::LibAccessor {
                name: prefix.to_string(),
                accessor: suffix.to_string(),
            });
        }
    }

    // Anything else (including bare {NAME} cross-recipe ref and {TOKEN} env
    // lookup) is admitted; substitution happens via the standard pipeline.
    Ok(())
}
```

- [ ] **Step 7.3: Remove the old `expand_plate_cmd_with_deps` and `expand_test_cmd_with_deps`**

Delete both functions (lines ~206–263 and ~266–317 of `template.rs`). The plate/test codegen will route through `expand_template_to_lua_with_deps` with an iteration-binding-name parameter — that change lands in Task 8.

- [ ] **Step 7.4: Generalise `expand_template_to_lua_with_deps`**

`expand_template_to_lua_with_deps` currently substitutes `{in}` / `{out}` / `{out_N}` / `{all}` to fixed Lua names (`_cook_in`, `_cook_out`, `_cook_outs[N]`, `_cook_all`). Add a new sibling that takes the iteration-binding name as a parameter. Add after `expand_template_to_lua_with_deps`:

```rust
/// Plate/test variant: substitute `{in}` to `iter_var`, `{all}` to `all_var`,
/// and reject `{out}` / `{out_N}` (use `validate_plate_test_placeholders`
/// before calling). `{NAME}` resolves to `cook.dep_output(NAME)` if `NAME`
/// is a recipe; otherwise to `cook.env[NAME]` (matches cook-side rules).
pub(crate) fn expand_plate_test_body(
    template: &str,
    recipe_names: &BTreeSet<String>,
    iter_var: &str,
    all_var: &str,
) -> String {
    // Implementation parallels `expand_with_deps_fallback`, but maps:
    //   {in}          → iter_var
    //   {in.ACCESSOR} → path.ACCESSOR(iter_var)
    //   {all}         → all_var
    //   {NAME}        → cook.dep_output("NAME")  (if NAME is a recipe)
    //   {TOKEN}       → cook.env["TOKEN"]
    //   anything else (already rejected by validate_plate_test_placeholders)
    let mut parts: Vec<String> = Vec::new();
    let mut remaining = template;
    while !remaining.is_empty() {
        match remaining.find('{') {
            None => {
                parts.push(format!("\"{}\"", escape_lua_string(remaining)));
                break;
            }
            Some(brace_start) => {
                if brace_start > 0 {
                    parts.push(format!(
                        "\"{}\"",
                        escape_lua_string(&remaining[..brace_start])
                    ));
                }
                let after = &remaining[brace_start..];
                if let Some(close) = after.find('}') {
                    let inner = &after[1..close];
                    let lua = if inner == "in" {
                        iter_var.to_string()
                    } else if let Some(acc) = inner.strip_prefix("in.") {
                        format!("path.{}({})", acc, iter_var)
                    } else if inner == "all" {
                        format!("table.concat({}, \" \")", all_var)
                    } else if recipe_names.contains(inner) {
                        format!("cook.dep_output(\"{}\")", escape_lua_string(inner))
                    } else {
                        format!("cook.env[\"{}\"]", escape_lua_string(inner))
                    };
                    parts.push(lua);
                    remaining = &remaining[brace_start + close + 1..];
                } else {
                    parts.push(format!(
                        "\"{}\"",
                        escape_lua_string(&remaining[brace_start..])
                    ));
                    break;
                }
            }
        }
    }
    if parts.is_empty() {
        "\"\"".to_string()
    } else if parts.len() == 1 {
        parts.into_iter().next().unwrap()
    } else {
        parts.join(" .. ")
    }
}
```

(The cook-side `expand_template_to_lua_with_deps` keeps its existing fixed-name substitution since cook codegen has its own bindings — no change there.)

- [ ] **Step 7.5: Compile**

```bash
cd /home/alex/dev/cook
cargo build -p cook-luagen
```

Expected: `cook-luagen` builds. Errors at the `Step::Plate` / `Step::Test` arms in `dep_ref.rs` and the call sites in `plate_step.rs` / `test_step.rs` are expected — those land in Task 8.

- [ ] **Step 7.6: Commit**

```bash
git add cli/crates/cook-luagen/src/template.rs
git commit -m "luagen(CS-0024): mode detection + plate/test placeholder validator + body-expander

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

## Task 8: Codegen — rewrite `plate_step.rs` and `test_step.rs` for six-arm mode dispatch

**Files:**
- Modify: `cli/crates/cook-luagen/src/plate_step.rs`
- Modify: `cli/crates/cook-luagen/src/test_step.rs`
- Modify: `cli/crates/cook-luagen/src/dep_ref.rs`
- Modify: `cli/crates/cook-luagen/src/recipe.rs`

- [ ] **Step 8.1: Rewrite `generate_plate_step`**

Replace the entire contents of `cli/crates/cook-luagen/src/plate_step.rs` with:

```rust
use std::collections::BTreeSet;

use cook_lang::ast::*;

use crate::template::{
    detect_plate_test_mode, escape_lua_string, expand_plate_test_body,
    validate_plate_test_placeholders, PlateTestMode,
};

pub(crate) fn generate_plate_step(
    out: &mut String,
    plate_step: &PlateStep,
    line: usize,
    last_cook_index: Option<usize>,
    recipe_names: &BTreeSet<String>,
) -> Result<(), CodegenError> {
    let mode = detect_plate_test_mode(&plate_step.body)
        .map_err(|e| CodegenError::PlateTestMode { line, source: e })?;
    validate_plate_test_placeholders(&plate_step.body, mode, recipe_names)
        .map_err(|e| CodegenError::Placeholder { line, source: e })?;

    let source_expr = if let Some(idx) = last_cook_index {
        format!("_cook_outputs_{}", idx)
    } else {
        "recipe.ingredients[1]".to_string()
    };

    match (&plate_step.body, mode) {
        // (1) Shell, OneToOne — loop over source, one unit per item.
        (Body::ShellBlock(lines), PlateTestMode::OneToOne) => {
            let cmd_text = build_shell_block_command(lines);
            let cmd_expr =
                expand_plate_test_body(&cmd_text, recipe_names, "_plate_in", "{}");
            out.push_str(&format!(
                "    for _, _plate_in in ipairs({}) do\n        cook.add_unit({{command = {}, cache = false}})\n    end\n",
                source_expr, cmd_expr
            ));
        }
        // (2) Shell, ManyToOne — one unit, source visible as {all}.
        (Body::ShellBlock(lines), PlateTestMode::ManyToOne) => {
            let cmd_text = build_shell_block_command(lines);
            let cmd_expr =
                expand_plate_test_body(&cmd_text, recipe_names, "\"\"", &source_expr);
            out.push_str(&format!(
                "    cook.add_unit({{command = {}, cache = false}})\n",
                cmd_expr
            ));
        }
        // (3) Shell, OneShot — one unit, no source.
        (Body::ShellBlock(lines), PlateTestMode::OneShot) => {
            let cmd_text = build_shell_block_command(lines);
            let cmd_expr =
                expand_plate_test_body(&cmd_text, recipe_names, "\"\"", "{}");
            out.push_str(&format!(
                "    cook.add_unit({{command = {}, cache = false}})\n",
                cmd_expr
            ));
        }
        // (4) Lua, OneToOne — loop, body sees `input`.
        (Body::LuaBlock(code), PlateTestMode::OneToOne) => {
            out.push_str(&format!(
                "    for _, _plate_in in ipairs({}) do\n",
                source_expr
            ));
            out.push_str(&format!(
                "        cook.add_unit({{cache = false, lua_code = {}, _bind_input = _plate_in}})\n",
                lua_chunk_literal(code)
            ));
            out.push_str("    end\n");
        }
        // (5) Lua, ManyToOne — one unit, body sees `inputs`.
        (Body::LuaBlock(code), PlateTestMode::ManyToOne) => {
            out.push_str(&format!(
                "    cook.add_unit({{cache = false, lua_code = {}, _bind_inputs = {}}})\n",
                lua_chunk_literal(code),
                source_expr
            ));
        }
        // (6) Lua, OneShot — one unit, no source binding.
        (Body::LuaBlock(code), PlateTestMode::OneShot) => {
            out.push_str(&format!(
                "    cook.add_unit({{cache = false, lua_code = {}}})\n",
                lua_chunk_literal(code)
            ));
        }
    }

    Ok(())
}

fn build_shell_block_command(lines: &[String]) -> String {
    let mut s = String::from("set -e");
    for line in lines {
        s.push('\n');
        s.push_str(line);
    }
    s
}

fn lua_chunk_literal(code: &str) -> String {
    format!("[==[\n{}\n]==]", code)
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum CodegenError {
    #[error("plate/test mode error at line {line}: {source}")]
    PlateTestMode {
        line: usize,
        source: crate::template::PlateTestModeError,
    },
    #[error("plate/test placeholder error at line {line}: {source}")]
    Placeholder {
        line: usize,
        source: crate::template::PlateTestPlaceholderError,
    },
}
```

(Note: the `_bind_input` / `_bind_inputs` keys on `cook.add_unit({...})` are illustrative — adapt to whatever the runtime expects. If `cook.add_unit` does not accept arbitrary `_bind_*` fields, route the binding via a small wrapper Lua chunk that sets the local before evaluating `lua_code`. The cook-side using-block lua codegen at `cook_step.rs` is the reference for the actual runtime contract.)

- [ ] **Step 8.2: Rewrite `generate_test_step`**

Replace `cli/crates/cook-luagen/src/test_step.rs` with the analogous code, substituting `cook.add_test` for `cook.add_unit` and adding `timeout` / `should_fail` fields:

```rust
use std::collections::BTreeSet;

use cook_lang::ast::*;

use crate::plate_step::CodegenError;
use crate::template::{
    detect_plate_test_mode, escape_lua_string, expand_plate_test_body,
    validate_plate_test_placeholders, PlateTestMode,
};

pub(crate) fn generate_test_step(
    out: &mut String,
    test_step: &TestStep,
    line: usize,
    last_cook_index: Option<usize>,
    recipe_names: &BTreeSet<String>,
) -> Result<(), CodegenError> {
    let mode = detect_plate_test_mode(&test_step.body)
        .map_err(|e| CodegenError::PlateTestMode { line, source: e })?;
    validate_plate_test_placeholders(&test_step.body, mode, recipe_names)
        .map_err(|e| CodegenError::Placeholder { line, source: e })?;

    let source_expr = if let Some(idx) = last_cook_index {
        format!("_cook_outputs_{}", idx)
    } else {
        "recipe.ingredients[1]".to_string()
    };
    let timeout = test_step.timeout.unwrap_or(300);
    let should_fail = if test_step.should_fail { "true" } else { "false" };

    match (&test_step.body, mode) {
        (Body::ShellBlock(lines), PlateTestMode::OneToOne) => {
            let cmd_text = build_shell_block_command(lines);
            let cmd_expr =
                expand_plate_test_body(&cmd_text, recipe_names, "_test_in", "{}");
            out.push_str(&format!(
                "    for _, _test_in in ipairs({}) do\n        cook.add_test({{command = {}, timeout = {}, should_fail = {}}})\n    end\n",
                source_expr, cmd_expr, timeout, should_fail
            ));
        }
        (Body::ShellBlock(lines), PlateTestMode::ManyToOne) => {
            let cmd_text = build_shell_block_command(lines);
            let cmd_expr =
                expand_plate_test_body(&cmd_text, recipe_names, "\"\"", &source_expr);
            out.push_str(&format!(
                "    cook.add_test({{command = {}, timeout = {}, should_fail = {}}})\n",
                cmd_expr, timeout, should_fail
            ));
        }
        (Body::ShellBlock(lines), PlateTestMode::OneShot) => {
            let cmd_text = build_shell_block_command(lines);
            let cmd_expr =
                expand_plate_test_body(&cmd_text, recipe_names, "\"\"", "{}");
            out.push_str(&format!(
                "    cook.add_test({{command = {}, timeout = {}, should_fail = {}}})\n",
                cmd_expr, timeout, should_fail
            ));
        }
        (Body::LuaBlock(code), PlateTestMode::OneToOne) => {
            out.push_str(&format!(
                "    for _, _test_in in ipairs({}) do\n",
                source_expr
            ));
            out.push_str(&format!(
                "        cook.add_test({{lua_code = {}, _bind_input = _test_in, timeout = {}, should_fail = {}}})\n",
                lua_chunk_literal(code), timeout, should_fail
            ));
            out.push_str("    end\n");
        }
        (Body::LuaBlock(code), PlateTestMode::ManyToOne) => {
            out.push_str(&format!(
                "    cook.add_test({{lua_code = {}, _bind_inputs = {}, timeout = {}, should_fail = {}}})\n",
                lua_chunk_literal(code), source_expr, timeout, should_fail
            ));
        }
        (Body::LuaBlock(code), PlateTestMode::OneShot) => {
            out.push_str(&format!(
                "    cook.add_test({{lua_code = {}, timeout = {}, should_fail = {}}})\n",
                lua_chunk_literal(code), timeout, should_fail
            ));
        }
    }

    Ok(())
}

fn build_shell_block_command(lines: &[String]) -> String {
    let mut s = String::from("set -e");
    for line in lines {
        s.push('\n');
        s.push_str(line);
    }
    s
}

fn lua_chunk_literal(code: &str) -> String {
    format!("[==[\n{}\n]==]", code)
}
```

(`CodegenError` is exported from `plate_step.rs`; both crates share the same error enum.)

- [ ] **Step 8.3: Update `dep_ref.rs`**

In `cli/crates/cook-luagen/src/dep_ref.rs`, find the `Step::Plate` and `Step::Test` arms (lines ~46–47):

```rust
Step::Plate { step: plate_step, .. } => extract_brace_tokens(&plate_step.command),
Step::Test  { step: test_step,  .. } => extract_brace_tokens(&test_step.command),
```

Replace with:

```rust
Step::Plate { step: plate_step, .. } => extract_body_tokens(&plate_step.body),
Step::Test  { step: test_step,  .. } => extract_body_tokens(&test_step.body),
```

Add the helper at the bottom of the file:

```rust
fn extract_body_tokens(body: &cook_lang::ast::Body) -> Vec<String> {
    use cook_lang::ast::Body;
    match body {
        Body::ShellBlock(lines) => {
            let joined = lines.join("\n");
            extract_brace_tokens(&joined)
        }
        // Lua bodies do not participate in cross-recipe `{NAME}` substitution
        // (Lua syntax owns the braces). Cross-recipe access in Lua bodies is
        // via `cook.dep_output()` — not extracted here.
        Body::LuaBlock(_) => Vec::new(),
    }
}
```

- [ ] **Step 8.4: Update `recipe.rs` call sites**

In `cli/crates/cook-luagen/src/recipe.rs`, find the plate/test step-emission arms (around lines 264 and 572). Each currently calls `generate_plate_step(...)` / `generate_test_step(...)` without a `?`. Wrap the call:

```rust
plate_step::generate_plate_step(
    &mut out, plate_step_val, *line, prev_cook_index, recipe_names,
)?;
```

and likewise for test. The function body's overall return type may need to change from `String` to `Result<String, CodegenError>` — propagate `?` appropriately and update the function signature. The error variant flows up to whatever caller in `lib.rs` already handles `CodegenError`.

- [ ] **Step 8.5: Compile**

```bash
cd /home/alex/dev/cook
cargo build -p cook-luagen
```

Expected: builds. Tests will fail in Task 9 (next), but the binary builds.

- [ ] **Step 8.6: Commit**

```bash
git add cli/crates/cook-luagen/src/plate_step.rs \
        cli/crates/cook-luagen/src/test_step.rs \
        cli/crates/cook-luagen/src/dep_ref.rs \
        cli/crates/cook-luagen/src/recipe.rs
git commit -m "luagen(CS-0024): plate/test six-arm dispatch, mode-aware codegen

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

## Task 9: Codegen tests

**Files:**
- Modify: `cli/crates/cook-luagen/src/tests.rs`

- [ ] **Step 9.1: Update `test_plate_step`**

Find the existing `test_plate_step` (around line 246). Replace with a fixture that exercises the new code path. For one-to-one shell:

```rust
#[test]
fn test_plate_step_shell_one_to_one() {
    let src = "recipe r\n    ingredients \"src/*.c\"\n    cook \"build/{in.stem}\" using { cc {in} -o {out} }\n    plate {\n        ./{in}\n    }\n";
    let cookfile = cook_lang::parse(src).unwrap();
    let lua = generate_lua(&cookfile).expect("codegen");
    assert!(
        lua.contains("for _, _plate_in in ipairs(_cook_outputs_1)"),
        "expected one-to-one plate loop, got:\n{}", lua
    );
    assert!(
        lua.contains("cook.add_unit") && lua.contains("cache = false"),
        "expected cache=false plate add_unit, got:\n{}", lua
    );
}
```

Add fixtures for each of the six mode/form combinations:

```rust
#[test]
fn test_plate_step_shell_many_to_one() {
    let src = "recipe r\n    ingredients \"src/*.c\"\n    cook \"build/{in.stem}\" using { cc {in} -o {out} }\n    plate { tar -czf bundle.tgz {all} }\n";
    let cookfile = cook_lang::parse(src).unwrap();
    let lua = generate_lua(&cookfile).expect("codegen");
    assert!(!lua.contains("for _, _plate_in"), "many-to-one should not emit a loop");
    assert!(lua.contains("table.concat(_cook_outputs_1, \" \")"));
    assert!(lua.contains("cook.add_unit"));
}

#[test]
fn test_plate_step_shell_one_shot() {
    let src = "recipe r\n    plate { echo build complete }\n";
    let cookfile = cook_lang::parse(src).unwrap();
    let lua = generate_lua(&cookfile).expect("codegen");
    assert!(!lua.contains("for _, _plate_in"));
    assert!(lua.contains("cook.add_unit"));
}

#[test]
fn test_plate_step_lua_one_to_one() {
    let src = "recipe r\n    ingredients \"src/*.c\"\n    cook \"build/{in.stem}\" using { cc {in} -o {out} }\n    plate >{ cook.sh(\"strip \" .. input) }\n";
    let cookfile = cook_lang::parse(src).unwrap();
    let lua = generate_lua(&cookfile).expect("codegen");
    assert!(lua.contains("for _, _plate_in"));
    assert!(lua.contains("_bind_input"));
}

#[test]
fn test_plate_step_lua_many_to_one() {
    let src = "recipe r\n    ingredients \"src/*.c\"\n    cook \"build/{in.stem}\" using { cc {in} -o {out} }\n    plate >{ for _, b in ipairs(inputs) do cook.sh(\"strip \" .. b) end }\n";
    let cookfile = cook_lang::parse(src).unwrap();
    let lua = generate_lua(&cookfile).expect("codegen");
    assert!(!lua.contains("for _, _plate_in"));
    assert!(lua.contains("_bind_inputs"));
}

#[test]
fn test_plate_step_lua_one_shot() {
    let src = "recipe r\n    plate >{ os.execute(\"echo done\") }\n";
    let cookfile = cook_lang::parse(src).unwrap();
    let lua = generate_lua(&cookfile).expect("codegen");
    assert!(!lua.contains("for _, _plate_in"));
    assert!(!lua.contains("_bind_input"));
}
```

- [ ] **Step 9.2: Add `test_test_step_*` symmetric coverage**

Mirror the six tests above for `test`, also exercising `timeout` and `should_fail`:

```rust
#[test]
fn test_test_step_shell_one_to_one_with_modifiers() {
    let src = "recipe r\n    ingredients \"src/*.c\"\n    cook \"build/{in.stem}\" using { cc {in} -o {out} }\n    test { ./{in} } timeout 60 should_fail\n";
    let cookfile = cook_lang::parse(src).unwrap();
    let lua = generate_lua(&cookfile).expect("codegen");
    assert!(lua.contains("for _, _test_in"));
    assert!(lua.contains("cook.add_test"));
    assert!(lua.contains("timeout = 60"));
    assert!(lua.contains("should_fail = true"));
}

// … plus the other five test fixtures, each with a representative
// timeout / should_fail combination.
```

- [ ] **Step 9.3: Add rejection tests**

```rust
#[test]
fn test_plate_out_rejected() {
    let src = "recipe r\n    ingredients \"src/*.c\"\n    cook \"build/{in.stem}\" using { cc {in} -o {out} }\n    plate { ./{out} }\n";
    let cookfile = cook_lang::parse(src).unwrap();
    let err = generate_lua(&cookfile).unwrap_err();
    assert!(format!("{}", err).contains("{out}"), "expected {{out}} rejection, got: {}", err);
}

#[test]
fn test_plate_mixed_in_and_all_rejected() {
    let src = "recipe r\n    ingredients \"src/*.c\"\n    cook \"build/{in.stem}\" using { cc {in} -o {out} }\n    plate { echo {in} {all} }\n";
    let cookfile = cook_lang::parse(src).unwrap();
    let err = generate_lua(&cookfile).unwrap_err();
    let msg = format!("{}", err);
    assert!(msg.contains("{in}") && msg.contains("{all}"), "expected mixed-mode rejection, got: {}", msg);
}

#[test]
fn test_plate_lua_mixed_input_and_inputs_rejected() {
    let src = "recipe r\n    ingredients \"src/*.c\"\n    cook \"build/{in.stem}\" using { cc {in} -o {out} }\n    plate >{ print(input); print(inputs[1]) }\n";
    let cookfile = cook_lang::parse(src).unwrap();
    let err = generate_lua(&cookfile).unwrap_err();
    let msg = format!("{}", err);
    assert!(msg.contains("input") && msg.contains("inputs"), "expected mixed-binding rejection, got: {}", msg);
}

#[test]
fn test_plate_bare_stem_rejected() {
    let src = "recipe r\n    ingredients \"src/*.c\"\n    cook \"build/{in.stem}\" using { cc {in} -o {out} }\n    plate { ./{stem}.out }\n";
    let cookfile = cook_lang::parse(src).unwrap();
    let err = generate_lua(&cookfile).unwrap_err();
    assert!(format!("{}", err).contains("{in.stem}"), "expected migration hint, got: {}", err);
}

#[test]
fn test_plate_lib_accessor_rejected() {
    let src = "recipe lib\n    ingredients \"x/*.c\"\n    cook \"build/{in.stem}.o\" using { cc -c {in} -o {out} }\nrecipe r: lib\n    cook \"build/app\" using { cc {lib} -o {out} }\n    plate { echo {lib.stem} }\n";
    let cookfile = cook_lang::parse(src).unwrap();
    let err = generate_lua(&cookfile).unwrap_err();
    assert!(format!("{}", err).contains("firewall"), "expected lib-accessor firewall msg, got: {}", err);
}
```

- [ ] **Step 9.4: Run codegen tests**

```bash
cd /home/alex/dev/cook
cargo test -p cook-luagen
```

Expected: all `cook-luagen` tests pass. If any older test still references plate/test `command:` field, update or delete it.

- [ ] **Step 9.5: Run full workspace tests**

```bash
cd /home/alex/dev/cook
cargo test --workspace
```

Expected: workspace passes except for conformance fixtures (still on old surface — fixed in Task 12).

- [ ] **Step 9.6: Commit**

```bash
git add cli/crates/cook-luagen/src/tests.rs
git commit -m "test(CS-0024): codegen coverage for six plate/test mode/form arms + rejections

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

# Phase D — Tree-sitter (Tasks 10–11)

## Task 10: Tree-sitter grammar — plate/test rules + queries

**Files:**
- Modify: `tree-sitter-cook/grammar.js`
- Modify: `tree-sitter-cook/src/parser.c` (regenerated)
- Modify: `tree-sitter-cook/queries/highlights.scm`
- Modify: `tree-sitter-cook/queries/injections.scm`

- [ ] **Step 10.1: Update `plate_step` and `test_step` rules**

In `tree-sitter-cook/grammar.js`, find the `plate_step` and `test_step` rules. Replace each with:

```js
plate_step: $ => seq(
    'plate',
    field('body', choice($.shell_block, $.using_lua_block)),
    optional($._newline),
),

test_step: $ => seq(
    'test',
    field('body', choice($.shell_block, $.using_lua_block)),
    optional(seq('timeout', field('timeout', $.number))),
    optional(field('should_fail', 'should_fail')),
    optional($._newline),
),
```

Verify `$.shell_block` and `$.using_lua_block` are already defined (they are, by the cook_step rule). Verify `$.number` exists (it does, used by test today).

- [ ] **Step 10.2: Update highlights**

In `tree-sitter-cook/queries/highlights.scm`, search for and **delete**:

```scm
(plate_step command: (string) @string.special)
(test_step  command: (string) @string.special)
```

(or whatever the existing rule shape is). The `shell_block` injection takes over.

- [ ] **Step 10.3: Verify injections cover plate/test**

In `tree-sitter-cook/queries/injections.scm`, the existing rule:

```scm
(shell_block (shell_content) @injection.content (#set! injection.language "bash"))
```

automatically applies to plate/test shell blocks once the grammar binds them as `shell_block`. Verify by running the corpus tests in Step 10.5. The Lua-block injection rule applies similarly.

- [ ] **Step 10.4: Regenerate `parser.c`**

```bash
cd /home/alex/dev/cook/tree-sitter-cook
npm run generate
```

Expected: `src/parser.c` is regenerated. Commit it as a binary blob.

- [ ] **Step 10.5: Run tree-sitter tests**

```bash
cd /home/alex/dev/cook/tree-sitter-cook
npm test
```

Expected: existing corpus tests for plate/test fail (they use the old surface). Fix them in Task 11.

- [ ] **Step 10.6: Commit**

```bash
git add tree-sitter-cook/grammar.js tree-sitter-cook/src/parser.c \
        tree-sitter-cook/queries/highlights.scm \
        tree-sitter-cook/queries/injections.scm
git commit -m "ts(CS-0024): plate_step / test_step grammar — body forms, drop string

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

## Task 11: Tree-sitter corpus

**Files:**
- Modify: `tree-sitter-cook/test/corpus/*.txt`

- [ ] **Step 11.1: Survey existing plate/test corpus cases**

```bash
cd /home/alex/dev/cook/tree-sitter-cook
grep -rln 'plate "\|test "' test/corpus/
```

Each file the search returns has at least one case that needs updating.

- [ ] **Step 11.2: Migrate each case**

For each surface form found, rewrite per the migration table from §4 of the spec:
- `plate "CMD"` → `plate { CMD }`
- `test "CMD"` → `test { CMD }`
- `test "CMD" timeout 60` → `test { CMD } timeout 60`
- Inside the body, `{out}` → `{in}`, `{out.X}` → `{in.X}`.

The expected-tree side of each test case rewrites correspondingly: `(plate_step command: (string))` becomes `(plate_step body: (shell_block (shell_content)))`.

- [ ] **Step 11.3: Add new positive corpus cases**

Add a new file `tree-sitter-cook/test/corpus/plate-test-bodies.txt` covering:

- plate one-to-one shell, one-line and multi-line
- plate many-to-one shell with `{all}`
- plate one-shot shell (no `{in}`, no `{all}`)
- plate one-to-one Lua (uses `input`)
- plate many-to-one Lua (uses `inputs`)
- plate one-shot Lua (uses neither)
- test all six forms with timeout/should_fail combinations

Each test case: source on the left of `===`, expected tree on the right of `---`. Mirror the existing cook_step corpus shape.

- [ ] **Step 11.4: Regenerate and run corpus tests**

```bash
cd /home/alex/dev/cook/tree-sitter-cook
npm test
node scripts/conformance.mjs
```

Expected: all tree-sitter tests pass. The conformance script checks the tree-sitter parser against the cook-lang parser using the standard's positive fixtures — once both are on the new surface, it should pass.

- [ ] **Step 11.5: Commit**

```bash
git add tree-sitter-cook/test/corpus/
git commit -m "ts(CS-0024): corpus updates for plate/test body forms

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

# Phase E — Conformance fixtures (Tasks 12–13)

## Task 12: Migrate existing positive fixtures

**Files:**
- Modify: `standard/conformance/positive/009-test-step/Cookfile` and `parse.txt`
- Modify: any other positive fixture using `plate`/`test`

- [ ] **Step 12.1: Survey**

```bash
cd /home/alex/dev/cook
grep -rln 'plate "\|test "' standard/conformance/positive/
```

- [ ] **Step 12.2: Update each Cookfile**

For each fixture's `Cookfile`: rewrite plate/test lines per the migration table. Keep ingredient and cook lines unchanged.

For example, `standard/conformance/positive/009-test-step/Cookfile` becomes:

```cook
recipe run-tests
    ingredients "tests/*.c"
    cook "build/{in.stem}" using { cc {in} -o {out} }
    test { ./{in} } timeout 60 should_fail
```

- [ ] **Step 12.3: Regenerate `parse.txt`**

The conformance harness (`cargo test -p cook-lang --test conformance`) prints expected output if a fixture's `parse.txt` is missing or stale. Use it to regenerate:

```bash
cd /home/alex/dev/cook
UPDATE_CONFORMANCE_FIXTURES=1 cargo test -p cook-lang --test conformance
```

(If the harness uses a different env var or `cargo test -- --update`, follow that convention — check `cli/crates/cook-lang/tests/conformance.rs` for the regeneration mechanism.)

Inspect each updated `parse.txt` to confirm it shows `body: ShellBlock(["./{in}"])` etc. instead of the old `command:` field.

- [ ] **Step 12.4: Run conformance**

```bash
cd /home/alex/dev/cook
cargo test -p cook-lang --test conformance
```

Expected: all positive fixtures pass.

- [ ] **Step 12.5: Commit**

```bash
git add standard/conformance/positive/
git commit -m "conformance(CS-0024): migrate existing plate/test fixtures to body form

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

## Task 13: New conformance fixtures — positive + negative

**Files:**
- Create: 6 new directories under `standard/conformance/positive/`
- Create: 8 new directories under `standard/conformance/negative/`

- [ ] **Step 13.1: Create positive fixtures**

For each of the six mode/form combinations, create `standard/conformance/positive/<NN>-<name>/Cookfile`:

| Directory | Surface |
|---|---|
| `027-plate-shell-one-to-one` | `plate { ./{in} }` after a `cook` step |
| `028-plate-shell-many-to-one` | `plate { tar czf bundle.tgz {all} }` |
| `029-test-shell-one-shot` | `test { make integration } timeout 30` |
| `030-plate-lua-one-to-one` | `plate >{ cook.sh("strip " .. input) }` |
| `031-test-lua-many-to-one` | `test >{ for _, b in ipairs(inputs) do cook.sh("./" .. b) end } timeout 300` |
| `032-plate-lua-one-shot` | `plate >{ os.execute("echo done") }` |

Each directory needs a `Cookfile` and (after Step 13.3) a `parse.txt`.

Example `027-plate-shell-one-to-one/Cookfile`:

```cook
recipe install
    ingredients "src/*.c"
    cook "build/bin/{in.stem}" using { cc {in} -o {out} }
    plate {
        install -d $PREFIX/bin
        install -m755 {in} $PREFIX/bin/{in.name}
    }
```

- [ ] **Step 13.2: Create negative fixtures**

For each of the eight rejections, create `standard/conformance/negative/<NN>-<name>/Cookfile` plus an `error.txt` containing the expected diagnostic substring:

| Directory | Surface | Error substring |
|---|---|---|
| `024-plate-out-rejected` | `plate { ./{out} }` | `{out}` |
| `025-plate-mixed-in-and-all` | `plate { echo {in} {all} }` | `{in}` and `{all}` |
| `026-plate-mixed-input-and-inputs` | `plate >{ print(input); print(inputs) }` | `input` and `inputs` |
| `027-plate-lib-accessor-rejected` | `plate { echo {lib.stem} }` | `firewall` |
| `028-plate-bare-stem-rejected` | `plate { ./{stem}.out }` | `{in.stem}` |
| `029-plate-string-form-rejected` | `plate "./{out}"` | `CS-0024` |
| `030-test-string-form-rejected` | `test "./{out}" timeout 60` | `CS-0024` |
| `031-one-to-one-empty-source-rejected` | `plate { ./{in} }` in a recipe with no preceding cook and no ingredients | `non-empty source` |

Each `Cookfile` should be the minimal surface that triggers the diagnostic; the `error.txt` should be the substring the harness asserts present in the parse error.

- [ ] **Step 13.3: Regenerate `parse.txt` for positive fixtures**

```bash
cd /home/alex/dev/cook
UPDATE_CONFORMANCE_FIXTURES=1 cargo test -p cook-lang --test conformance
```

Inspect each `parse.txt` to confirm correctness.

- [ ] **Step 13.4: Run conformance**

```bash
cd /home/alex/dev/cook
cargo test -p cook-lang --test conformance
```

Expected: all positive and negative fixtures pass.

- [ ] **Step 13.5: Commit**

```bash
git add standard/conformance/positive/02{7,8,9}-* \
        standard/conformance/positive/03{0,1,2}-* \
        standard/conformance/negative/02{4,5,6,7,8,9}-* \
        standard/conformance/negative/03{0,1}-*
git commit -m "conformance(CS-0024): six positive + eight negative fixtures for plate/test bodies

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

# Phase F — Migration & examples (Tasks 14–15)

## Task 14: Migrate in-repo Cookfiles and modules

**Files:**
- Modify: top-level `Cookfile`, `cli/Cookfile`, `tree-sitter-cook/Cookfile`, `standard/Cookfile`
- Modify: `examples/*/Cookfile`
- Modify: `examples/*/cook_modules/*.lua`, `cook_modules/*.lua`, `standard/cook_modules/*.lua`

- [ ] **Step 14.1: Survey**

```bash
cd /home/alex/dev/cook
grep -rln 'plate "\|test "' Cookfile cli/Cookfile tree-sitter-cook/Cookfile \
                            standard/Cookfile examples/ cook_modules/ \
                            standard/cook_modules/ 2>/dev/null
```

- [ ] **Step 14.2: Migrate each occurrence**

For each result, apply the migration rules:
- `plate "CMD"` → `plate { CMD }`
- `test "CMD"` → `test { CMD }`
- `test "CMD" timeout N` → `test { CMD } timeout N`
- `test "CMD" should_fail` → `test { CMD } should_fail`
- Inside the body: `{out}` → `{in}`, `{out.X}` → `{in.X}`.

Multi-line bodies are encouraged where the original `&&`-chained one-liner was awkward — opportunistically split, but don't over-restructure (keep diffs minimal where the one-liner was fine).

For `cook_modules/*.lua` files that emit plate/test surface text (search `plate ` / `test ` literals in Lua strings), update the emitted text to body form. The `&` joiner that today produces `plate "CMD"` becomes `plate { CMD }`.

- [ ] **Step 14.3: Run end-to-end on each example**

For each `examples/*/Cookfile`, run a quick build to confirm:

```bash
cd /home/alex/dev/cook/examples/<name>
cargo run -p cook -- --emit-lua <some-recipe> | head -40
```

(Or use the example's own README'd invocation.) The emitted Lua should compile (no syntax errors) and the recipe selection should not produce `plate`/`test`-surface diagnostics. A full execution isn't required for every example; spot-check one or two `plate`-using and `test`-using examples (cross-recipe-deps and pnpm-monorepo if they use plate/test).

- [ ] **Step 14.4: Run workspace tests**

```bash
cd /home/alex/dev/cook
cargo test --workspace
```

Expected: pass.

- [ ] **Step 14.5: Commit**

```bash
git add Cookfile cli/Cookfile tree-sitter-cook/Cookfile standard/Cookfile \
        examples/ cook_modules/ standard/cook_modules/
git commit -m "migrate(CS-0024): in-repo Cookfiles and cook_modules to plate/test body form

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

## Task 15: Iteration benchmarks — plate/test recipes

**Files:**
- Modify: `examples/iteration_benchmarks/Cookfile`
- Modify: `examples/iteration_benchmarks/README.md`

- [ ] **Step 15.1: Add plate/test mode recipes to `Cookfile`**

Append six new recipes after the existing eight cook benchmark recipes (parallel naming `plate_*` and `test_*`):

```cook
# Plate mode 1: One-to-one, shell.
# Body uses {in} ⇒ iterates over the preceding cook's outputs.
recipe plate_one_to_one_shell
    ingredients "src/inputs/*.txt"
    cook "build/plate_one_to_one_shell/{in.stem}.out" using {
        mkdir -p build/plate_one_to_one_shell
        sleep {SLEEP}
        echo "plated: {in}" > {out}
    }
    plate {
        sleep {SLEEP}
        echo "plate one-to-one: {in}" >> {in}
    }

# Plate mode 2: Many-to-one, shell.
# Body uses {all} ⇒ one unit, full source visible.
recipe plate_many_to_one_shell
    ingredients "src/inputs/*.txt"
    cook "build/plate_many_to_one_shell/{in.stem}.out" using {
        mkdir -p build/plate_many_to_one_shell
        sleep {SLEEP}
        echo "plated: {in}" > {out}
    }
    plate {
        sleep {SLEEP}
        echo "plate many-to-one inputs: {all}" > build/plate_many_to_one_shell/all.report
    }

# Plate mode 3: One-shot, shell.
# Body references neither {in} nor {all} ⇒ runs once.
recipe plate_one_shot_shell
    ingredients "src/inputs/*.txt"
    cook "build/plate_one_shot_shell/{in.stem}.out" using {
        mkdir -p build/plate_one_shot_shell
        sleep {SLEEP}
        echo "plated: {in}" > {out}
    }
    plate {
        sleep {SLEEP}
        echo "plate one-shot ran" > build/plate_one_shot_shell/.flag
    }

# Plate mode 4: One-to-one, Lua.
# Body references `input` ⇒ iterates.
recipe plate_one_to_one_lua
    ingredients "src/inputs/*.txt"
    cook "build/plate_one_to_one_lua/{in.stem}.out" using {
        mkdir -p build/plate_one_to_one_lua
        sleep {SLEEP}
        echo "plated: {in}" > {out}
    }
    plate >{
        os.execute("sleep " .. (os.getenv("SLEEP") or "0.5"))
        local f = io.open(input, "a")
        f:write("plate one-to-one lua: " .. input .. "\n")
        f:close()
    }

# Plate mode 5: Many-to-one, Lua.
# Body references `inputs` ⇒ one unit.
recipe plate_many_to_one_lua
    ingredients "src/inputs/*.txt"
    cook "build/plate_many_to_one_lua/{in.stem}.out" using {
        mkdir -p build/plate_many_to_one_lua
        sleep {SLEEP}
        echo "plated: {in}" > {out}
    }
    plate >{
        os.execute("sleep " .. (os.getenv("SLEEP") or "0.5"))
        local f = io.open("build/plate_many_to_one_lua/all.report", "w")
        f:write("plate many-to-one lua: " .. #inputs .. " inputs\n")
        f:close()
    }

# Plate mode 6: One-shot, Lua.
recipe plate_one_shot_lua
    ingredients "src/inputs/*.txt"
    cook "build/plate_one_shot_lua/{in.stem}.out" using {
        mkdir -p build/plate_one_shot_lua
        sleep {SLEEP}
        echo "plated: {in}" > {out}
    }
    plate >{
        os.execute("sleep " .. (os.getenv("SLEEP") or "0.5"))
        os.execute("touch build/plate_one_shot_lua/.flag")
    }
```

Add an analogous `test_*` recipe for each of the six modes that exercises a `test` step (e.g., `test { test -f {in} } timeout 5`).

Update the `benchmarks` orchestrator's dep list (around line 155 in the current Cookfile) to include all twelve new recipes.

- [ ] **Step 15.2: Update README**

Append a "Plate/test modes" section documenting the new recipes — mirror the existing "The eight modes" table format.

- [ ] **Step 15.3: Test the benchmarks**

```bash
cd /home/alex/dev/cook/examples/iteration_benchmarks
cargo run -p cook -- clean
cargo run -p cook -- --no-ui benchmarks
ls build/plate_*  # confirm each mode produced its expected artifacts
```

Expected: clean exit; each mode's artifacts exist.

- [ ] **Step 15.4: Commit**

```bash
git add examples/iteration_benchmarks/
git commit -m "example(CS-0024): plate/test recipes — all 6 modes + 6 test variants

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

# Phase G — Final (Task 16)

## Task 16: App. D entry + workspace verification

**Files:**
- Modify: `standard/src/content/docs/appendix/D-changes.mdx`

- [ ] **Step 16.1: Add CS-0024 entry**

In `standard/src/content/docs/appendix/D-changes.mdx`, find the existing "CS-NNNN" entries (CS-0023 was the v0.5 cut). Add a new entry after CS-0023:

```mdx
### D.NN. CS-0024 — `plate` / `test` unification

**Date:** 2026-05-NN
**Type:** Parse-format-altering change.

**What changed.**
- `plate_step` and `test_step` now have `body` (shell or Lua block) in place of `STRING`. The `using` keyword is not used.
- Iteration mode (one-to-one / many-to-one / one-shot) is deduced from body content: `{in}` / `{all}` for shell, `input` / `inputs` for Lua.
- `{out}` and the `{out_N}` family are rejected in plate/test bodies — these steps declare no outputs.
- §5.5 cross-recipe `{NAME}` substitution applies in plate/test shell bodies (already applied in plate/test command STRINGs).
- §5.4's `{lib.ACCESSOR}` firewall extended to plate/test bodies (universal rejection — plate/test cannot drive iteration).
- `should_fail` and `timeout N` modifiers continue to apply to `test`; they trail the body.

**Migration.**
- `plate "CMD"` → `plate { CMD }`
- `test "CMD" timeout N should_fail` → `test { CMD } timeout N should_fail`
- Inside body: `{out}` → `{in}`, `{out.X}` → `{in.X}`.
- A plate that previously ran N times without referencing `{out}` now runs **once** (one-shot mode). Inspect each such plate; the previous behavior was almost always unintended.

**Spec sections touched.** §4.7, §4.8, §5.4, §5.5, §6.4, §6.7, §8.1.2, App. A.4, App. B.4.7–B.4.11.
```

(Numbering: substitute the next free D-section number for `D.NN`. Today's date for `2026-05-NN`.)

- [ ] **Step 16.2: Run all checks**

```bash
cd /home/alex/dev/cook
cargo test --workspace
cd standard && pnpm build && pnpm test && pnpm lint:keywords && cd ..
cd tree-sitter-cook && npm test && node scripts/conformance.mjs && cd ..
```

Expected: every command exits 0.

- [ ] **Step 16.3: Run iteration_benchmarks end-to-end as final smoke**

```bash
cd /home/alex/dev/cook/examples/iteration_benchmarks
cargo run -p cook -- clean
cargo run -p cook -- --no-ui benchmarks
```

Expected: completes; the artifact tree under `build/` includes each cook mode's outputs and each plate/test mode's outputs.

- [ ] **Step 16.4: Commit**

```bash
git add standard/src/content/docs/appendix/D-changes.mdx
git commit -m "spec(CS-0024): App. D entry — plate/test unification

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

- [ ] **Step 16.5: Final summary**

The implementation series is complete. Confirm with one last sweep:

```bash
cd /home/alex/dev/cook
git log --oneline -20
git status
```

Expected: 16 new commits, working tree clean.

---

## Self-review — spec coverage

| Spec § | Plan task |
|---|---|
| §3.1 model: cook step with no declared outputs | Task 1 (§4.7 / §4.8 prose) |
| §3.2 grammar | Tasks 1, 3, 4, 5, 10 |
| §3.3 source rule | Tasks 1, 8 (codegen) |
| §3.4 mode deduction (shell + Lua) | Tasks 1, 7, 8 |
| §3.4.1 worked examples | Tasks 1 (spec), 13 (positive fixtures), 15 (benchmarks) |
| §3.5 shell-block placeholder vocabulary | Tasks 2, 7 |
| §3.6 Lua-block bindings | Tasks 2, 8 |
| §3.7 cross-recipe substitution | Task 2 |
| §3.8 substitution timing & codegen | Tasks 7, 8 |
| §3.9 phase classification | Task 2 |
| §3.10 App. A grammar | Task 3 |
| §3.11 tree-sitter | Tasks 10, 11 |
| §4 migration touch points | Tasks 12, 13, 14, 15 |
| §5.1 cook-lang AST | Task 4 |
| §5.1 parser | Tasks 5, 6 |
| §5.2 cook-luagen | Tasks 7, 8, 9 |
| §5.3 tree-sitter | Tasks 10, 11 |
| §5.4 conformance | Tasks 12, 13 |
| §6 open questions | Resolved by Tasks 5 (parser), 7 (Lua scan), 13 (negative fixtures) |
| §7 rationale | Task 3 |
| §8 acceptance criteria | Task 16 |
