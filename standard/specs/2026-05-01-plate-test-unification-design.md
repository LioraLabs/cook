# Design: `plate` and `test` as cook steps with no declared outputs (`{in}` / `{all}` body deduction, block-only surface)

**Date:** 2026-05-01
**Status:** Design — pending implementation plan
**Standard change ID:** CS-NNNN (assigned at PR time)
**Scope:** Cook Standard (chapters §4.7, §4.8, §5.4–§5.5, §6.4, §8.1.2, App. A.4, App. B.4), the Rust parser (`cli/crates/cook-lang`), the codegen (`cli/crates/cook-luagen`), `tree-sitter-cook`, and conformance fixtures.

## 1. Motivation

CS-0022 unified the `cook` step on a single block-only surface: one body grammar (`{ … }` shell or `>{ … }` Lua), one placeholder vocabulary (`{in}` / `{in.X}` / `{out}` / `{out_N}` / `{all}` / `{lib}`), and one iteration rule (output pattern owns the mode). The `plate` and `test` steps did not move with it. They still have today's pre-CS-0022 shape:

```ebnf
plate_step ::= "plate" STRING NEWLINE
test_step  ::= "test"  STRING ("timeout" NUMBER)? "should_fail"? NEWLINE
```

Three concrete problems follow from leaving them behind:

1. **One-line shell only.** Plate and test bodies are a single `STRING`. A multi-line plate (e.g., `mkdir -p $DST && cp {out} $DST && chmod +x $DST/{out.name}`) requires either an `&&`-chained one-liner or a fall-through to a Lua block. CS-0022 made multi-line shell first-class for cook; plate and test still pay the one-line tax.

2. **No Lua-block surface.** `plate >{ … }` and `test >{ … }` are syntax errors. Authors who want a Lua plate or test write a regular `>{ … }` (execute-phase Lua step) and lose the declarative-region placement, the implicit binding to the preceding cook's output list, and the test-specific `timeout` / `should_fail` modifiers.

3. **`{out}` is a misnomer.** Today's plate/test command template uses `{out}` to mean the current iteration item — i.e., "the cook output we're currently plating / testing." But CS-0022 fixed `{out}` to mean **the unit's declared output**. A plate step has no declared output; reusing `{out}` for "the iteration item" is incoherent with the rest of the language. The right name for that handle is `{in}` — the current iteration item — same name CS-0022 uses across cook bodies.

A fourth problem is more subtle:

4. **Plate iterates unconditionally.** Today every plate step runs once per item in the preceding cook's output list, even if the body never references the iteration item. `plate "make install"` silently runs N times. The right behavior is to deduce iteration mode from the body — which is exactly the role CS-0022 gave to the output pattern in cook. Plate has no output pattern, but it has a body, and the body's placeholder content is sufficient signal.

A fifth problem is compositional:

5. **No many-to-one plate or test.** Today `plate "tar -czf bundle.tgz {all}"` is unreachable — `{all}` isn't in plate's vocabulary, and even if it were, plate's "iterate per output" rule would run the tar command N times. Authors fall back to a manual `>{ … }` Lua block. CS-0022 added many-to-one to cook; plate and test should compose the same way.

The cumulative effect is that **plate and test have stayed at the v0.4-pre surface while cook moved forward**. This design unifies them into the same body grammar, the same placeholder/binding vocabulary, and a single iteration-mode rule that produces three modes (one-to-one, many-to-one, one-shot) the same way cook produces its three modes.

## 2. Non-goals

- **Caching surface in the Standard.** The Standard does not specify whether plate/test units are cached. Today's implementation forces `cache = false` for both; whether to keep that default, expose a CLI flag, or honor a settings file is purely an implementation concern. The Standard says implementations MAY cache and stops there (mirroring §8.6's posture for cook).
- **A new step keyword that subsumes plate and test.** Plate and test stay distinct. Each carries different author intent (presentation vs validation), and `test` carries genuine extra structure (`timeout`, `should_fail`). Collapsing them into one keyword loses readability and gains nothing.
- **An explicit iteration source.** Plate/test continue to bind their iteration source implicitly to "the preceding `cook` step's output list, falling back to the recipe's resolved ingredients." No new `from cook { … }` or analogous syntax.
- **A `using` clause for plate/test.** Cook earns the `using` keyword because its line carries two things — output pattern and body — that need a separator. Plate and test have only a body; a `using` keyword would be ceremony with no semantic load.
- **Multi-output plate/test.** Plate and test produce no DAG outputs by definition. They are the "no declared outputs" shape of a cook step; their output list is passthrough per §5.4.1.
- **A general expression language inside `{ … }`.** Same posture as CS-0022: placeholders are `{NAME}` or `{NAME.ACCESSOR}` and nothing more.
- **Lua-body textual `{lib}` substitution.** §B.6.1's rationale (Lua has direct binding; shell does not) holds unchanged. Plate/test Lua blocks call `cook.dep_output()` for cross-recipe references, same as cook.

## 3. Design

### 3.1. The single-sentence model

A `plate` or `test` step is **a `cook` step that produces no DAG output unit**. It shares cook's body grammar, cook's placeholder/binding vocabulary, cook's substitution timing, and a parallel of cook's mode-determination rule. The only structural differences are:

- No output pattern list (and therefore no `{out}` / `{out_N}` in the body).
- Iteration mode is deduced from the body itself, since no output pattern exists to declare it.
- The step's contribution to the recipe's output list is passthrough (§5.4.1, unchanged).
- `test` admits two trailing modifiers, `timeout NUMBER` and `should_fail`, in that order.

Everything else — block grammar, cross-recipe substitution, register-time substitution timing, declarative-region placement, step-group recording — comes "for free" from the cook surface CS-0022 already specified.

### 3.2. Grammar

App. A.4 changes:

```ebnf
plate_step ::= "plate" body NEWLINE
test_step  ::= "test"  body test_modifiers NEWLINE
body            ::= shell_block | using_lua_block
test_modifiers  ::= ("timeout" NUMBER)? "should_fail"?
```

`shell_block` and `using_lua_block` are the same productions §A.4 already defines for cook bodies. The leading `STRING` is removed from both step kinds; the `using` keyword is **not** added. A plate or test line is the keyword followed directly by an opening brace (`{` for shell, `>{` for Lua); for `test`, the trailing modifiers follow the closing brace.

A single-line shell block on the same line as the keyword — `plate { ./{in} --selftest }` — is a valid `shell_block` per §2.9 (the brace-balance lexer already accepts a single-line block). No new lexical class is introduced.

The four-form normative table in §4.8 ("Form / Behaviour") is preserved verbatim; only the grammar shape changes from `test STRING …` to `test BODY …`.

The step-dispatch order in §A.4 (Step-dispatch priority) is unchanged — `plate` and `test` still dispatch by keyword + separator, and the body parse begins after the keyword.

### 3.3. Iteration source

A plate/test step's **iteration source** is determined at register time, before the body is consulted, by walking the preceding declarative-region steps in source order:

1. If a `cook_step` precedes it in the same recipe body (any iteration mode, any output count), the source is that cook step's output list — the **flattened** list across all units it produces. A one-to-one cook over N inputs producing two outputs each contributes a 2N-element source list.
2. Otherwise, if an `ingredients_step` precedes it, the source is the recipe's resolved ingredient paths (§4.3).
3. Otherwise, the source is the empty list.

A plate/test step in modes that consult the source (§3.4) MUST have a non-empty source. A plate/test step in **one-shot mode** (§3.4) does not consult the source and MAY appear in a recipe with neither cook nor ingredients.

The "preceding cook" rule matches today's reference implementation (`cook-luagen/src/plate_step.rs`, the `last_cook_index` lookup); the change is that source consultation now depends on mode (§3.4), and the multi-output flattening is now made explicit (today's lookup uses `_cook_outputs_{idx}` which was ill-defined for multi-output cooks; this design pins it to a flattened list).

### 3.4. Mode deduction

A plate/test step is in one of three modes, determined entirely by its body's content:

| Shell-block body contains | Lua-block body references | Mode | Behavior |
|---|---|---|---|
| `{in}` or `{in.ACCESSOR}` (no `{all}`) | `input` (no `inputs`) | **one-to-one** | One unit per source item; `{in}` / `input` is the current item |
| `{all}` (no `{in}`/`{in.ACCESSOR}`) | `inputs` (no `input`) | **many-to-one** | Exactly one unit; `{all}` / `inputs` sees the full source list |
| neither | neither | **one-shot** | Exactly one unit; source is not consulted |
| both | both | **error** | Mixed iteration signal — load-time rejection |

A conforming implementation MUST reject:
- A body that contains both `{in}` (or `{in.ACCESSOR}`) and `{all}` (shell), or both `input` and `inputs` (Lua), with a diagnostic that names both tokens and the line of the step.
- A body in **one-to-one** or **many-to-one** mode whose source is empty (no preceding cook, no ingredients) — the diagnostic MUST name the body token that signaled the mode and the absent source.

The Lua identifier scan is a **free-identifier-position scan** that respects the existing string/comment/long-string ignore rules of §2.9 (the brace-balance lexer). The same scan handles both `input` and `inputs` detection. The scan recognizes the identifier in any free-identifier position (a name not immediately preceded by `.` or `:`), including assignment LHS, RHS, function arguments, table indexers, and `local` declarations; a property-name suffix (e.g., `cook.input`, `obj:input(...)`) is **not** a reference to the binding because the Lua grammar resolves the name as a field access, not a free variable. A `local input = …` shadow inside the body counts as a reference to `input` — the author has named the binding, which is the same signal as a free reference. (Rationale §B-new.5: the alternative — runtime-only mode resolution — would defeat the load-time diagnostic.)

The mode determination is **purely syntactic** and happens at register time, in the same pass that performs placeholder substitution. It does not require Lua evaluation.

### 3.4.1. The four shapes — worked examples

```cook
# (1) One-to-one shell plate — N source items ⇒ N units.
recipe install_one_to_one
    ingredients "src/*.c"
    cook "build/bin/{in.stem}" using { cc {in} -o {out} }
    plate {
        install -d $PREFIX/bin
        install -m755 {in} $PREFIX/bin/{in.name}
    }

# (2) Many-to-one shell plate — N source items ⇒ 1 unit.
recipe bundle
    ingredients "src/*.c"
    cook "build/bin/{in.stem}" using { cc {in} -o {out} }
    plate {
        tar -czf build/bundle.tgz {all}
    }

# (3) One-shot shell test — runs once regardless of source.
recipe smoke
    ingredients "src/*.c"
    cook "build/bin/app" using { cc {all} -o {out} }
    test {
        ./build/bin/app --selftest
    } timeout 30

# (4) One-to-one Lua plate — `input` signals one-to-one over source.
recipe sign
    ingredients "src/*.c"
    cook "build/bin/{in.stem}" using { cc {in} -o {out} }
    plate >{
        cook.sh("codesign --sign 'Developer ID' " .. input)
    }

# (5) Many-to-one Lua test — `inputs` signals many-to-one.
recipe coverage
    ingredients "src/*.c"
    cook "build/bin/{in.stem}.test" using { cc {in} -o {out} }
    test >{
        for _, bin in ipairs(inputs) do
            cook.sh("./" .. bin .. " --check")
        end
    } timeout 300

# (6) One-shot Lua plate — neither binding referenced.
recipe announce
    plate >{
        cook.sh("echo build complete")
    }
```

In (1) and (4), iteration is over the cook's output list, one unit per cook output. In (2) and (5), one unit runs with the full output list visible as `{all}` / `inputs`. In (3) and (6), the body runs once and the cook's output list is not consulted by the plate/test (though the recipe's overall output list still includes the cook outputs by passthrough).

### 3.5. Placeholder vocabulary (shell block)

A new normative table for plate/test shell-block bodies, paralleling CS-0022's §6.7 table for cook bodies:

| Placeholder | Valid in mode | Meaning |
|---|---|---|
| `{in}` | one-to-one | The current iteration item (path) |
| `{in.ACCESSOR}` | one-to-one | `path.ACCESSOR(in)` per §6.6 |
| `{all}` | many-to-one | The source list, space-joined |
| `{NAME}` (NAME is a recipe in scope) | any mode | `cook.dep_output(NAME)` per §5.5 |
| `{NAME.ACCESSOR}` (NAME is a recipe in scope) | **rejected** | Preserves the §5.4 firewall — same rule as cook bodies |
| `{TOKEN}` (none of the above) | any mode | `cook.env[TOKEN]` per §5.2 step 4 |
| `{out}` / `{out.ACCESSOR}` / `{out_N}` / `{out_N.ACCESSOR}` | **rejected** | Plate/test produce no declared outputs |

A conforming implementation MUST reject:
- `{in}` or `{in.ACCESSOR}` in a many-to-one or one-shot plate/test (no current iteration);
- `{all}` in a one-to-one or one-shot plate/test (no batched input list);
- Any of the `{out}` / `{out_N}` family — the diagnostic MUST name the migration target: "`{out}` is not valid in a `plate`/`test` body; the iteration item is `{in}`."
- `{NAME.ACCESSOR}` for any recipe `NAME` in scope (§5.4 firewall, unchanged from cook bodies).

Bare path-accessors (`{stem}`, `{name}`, `{ext}`, `{dir}`) fall through to `cook.env[TOKEN]` per §5.2 step 4. The implementation MUST emit a specific diagnostic for these four names that points to `{in.ACCESSOR}` (same diagnostic CS-0022 introduced for cook bodies).

### 3.6. Lua-block bindings

A new table extends §6.4 for plate/test Lua blocks:

| Binding | Mode | Meaning |
|---|---|---|
| `input` | one-to-one | The current iteration item (string) |
| `inputs` | many-to-one | The full source list (table, 1-indexed) |
| `cook.*`, `fs.*`, `path.*` | any | Standard Cook Lua API per §6 |

A conforming implementation:
- MUST bind exactly one of `input` / `inputs` per unit, per the mode the body's content selected. The other is unbound (Lua `nil` on reference).
- MUST NOT bind `output` or `outputs` for plate/test units — these have no meaning when the step declares no outputs.
- MUST NOT bind `input_N` (indexed input access) for plate/test. (Rationale §B-new.4.)

The bindings live for the duration of the body unit's worker VM, the same as cook's `using >{ … }` bindings (§6.4).

### 3.7. Cross-recipe references

§5.5's surface-list paragraph extends to cover plate/test bodies (both block forms) the same way CS-0022 extended it to `using { … }`:

> A `{NAME}` bare reference in a `cook` `using` shell block, **`plate` shell block, `test` shell block,** plate/test command, or bare shell MUST be substituted by the space-joined concatenation of the named recipe's output list (§{xref.dep-recipe-output}).

Inside a Lua block (`plate >{ … }` / `test >{ … }`), cross-recipe references are accessed via `cook.dep_output(NAME)` / `cook.dep_output_list(NAME)` — same rule as cook (§B.6.1).

§5.4's firewall on `{lib.ACCESSOR}` in non-driving steps continues to apply to plate/test bodies. Plate/test have no output pattern, so they cannot drive iteration over a `lib`; therefore `{lib.ACCESSOR}` is universally rejected in plate/test bodies. A `{lib}` (bare) reference is admitted and substitutes per §5.5.

### 3.8. Substitution timing and codegen

Shell-block placeholder substitution happens at **register-time code generation**, mirroring CS-0022's rule for cook shell blocks. By the time a unit is recorded with `cook.add_unit({command = "…"})` (or `cook.add_test({…})` for tests), the command field is concrete text.

The reference implementation routes plate/test through the same `expand_template_to_lua_with_deps` path that cook bodies use, with the placeholder vocabulary of §3.5 substituted: `{in}` → `_plate_in` / `_test_in` (or the cook step's analogous binding), `{all}` → `_plate_all` / `_test_all`. The mode-selection logic dispatches on the body's placeholder content per §3.4, then emits one of:

- **One-to-one shell:** a `for _, _plate_in in ipairs(<source>) do cook.add_unit({command = …}) end` loop, parallel to today's loop body.
- **Many-to-one shell:** a single `cook.add_unit({command = …})` call with `_plate_all` bound to the source list.
- **One-shot shell:** a single `cook.add_unit({command = …})` call with no source binding.
- **One-to-one Lua:** a `for` loop registering one unit per source item, each unit's `lua_code` carrying the body with `input` set to the current item.
- **Many-to-one Lua:** a single unit whose `lua_code` carries the body with `inputs` set to the source list.
- **One-shot Lua:** a single unit whose `lua_code` carries the body with no source binding.

Test units use `cook.add_test` instead of `cook.add_unit` (existing convention, `test_step.rs:29`); the `timeout` and `should_fail` fields ride on the same call.

### 3.9. Phase classification

§8.1.2 ("Phase classification of every Lua-bearing surface form") gains rows for plate/test:

| Surface form | Phase | Region |
|---|---|---|
| `plate { … }` | register (substitution) → execute (shell) | declarative |
| `plate >{ … }` | register (binding setup) → execute (Lua) | declarative |
| `test { … }` | register → execute | declarative |
| `test >{ … }` | register → execute | declarative |

The classification matches cook's `using { … }` and `using >{ … }` rows verbatim — once plate/test are "cook with no outputs," their phase profile is identical.

### 3.10. App. A grammar deltas

Concrete edits to App. A.4:

- Replace `plate_step ::= "plate" STRING NEWLINE` with the production in §3.2.
- Replace `test_step ::= "test" STRING ("timeout" NUMBER)? "should_fail"? NEWLINE` with the production in §3.2.
- The "Step-dispatch priority" list (entries 3 and 4) is unchanged — keyword + separator dispatch still picks `plate_step` / `test_step`.

The chore step-kind ban paragraph (§A.3 normative paragraph) is unchanged: `plate_step` and `test_step` remain banned in chore bodies for the same reason they're banned today (B.4.15).

### 3.11. Tree-sitter parser deltas

`tree-sitter-cook`'s `grammar.js`:

- `plate_step`: drop the `field("command", $.string)` arm; add `field("body", choice($.shell_block, $.using_lua_block))`.
- `test_step`: same edit; preserve the existing `timeout` and `should_fail` modifier fields on the trailing slot.
- Highlights query (`queries/highlights.scm`): drop the `(plate_step command: (string) @string.special)` and `(test_step command: (string) @string.special)` rules.
- Injections query (`queries/injections.scm`): the existing `(shell_block (shell_content) @injection.content (#set! injection.language "bash"))` rule (added in CS-0022) automatically applies to plate/test shell blocks; the existing `using_lua_block` injection rule applies to plate/test Lua blocks. No new injection rule needed.

## 4. Migration

Pre-release lockstep posture (per `MEMORY.md` → "Cook Standard governs language changes"). Every existing `plate "STRING"` / `test "STRING"` instance in the repo is rewritten to the new surface in the same change set. The rewrite is mechanical:

- `plate "CMD"` → `plate { CMD }`
- `test "CMD"` → `test { CMD }`
- `test "CMD" timeout N` → `test { CMD } timeout N`
- `test "CMD" should_fail` → `test { CMD } should_fail`
- `test "CMD" timeout N should_fail` → `test { CMD } timeout N should_fail`
- Inside the body, every `{out}` becomes `{in}`; every `{out.ACCESSOR}` becomes `{in.ACCESSOR}`.

Touched surfaces:

- `standard/src/content/docs/04-recipes.mdx` — §4.7 and §4.8 rewritten per §3.2 and §3.4–§3.6 above.
- `standard/src/content/docs/05-cross-recipe-references.mdx` — §5.5 wording updated per §3.7.
- `standard/src/content/docs/06-cook-lua-api.mdx` — §6.4 binding table extended per §3.6.
- `standard/src/content/docs/08-execution-model.mdx` — §8.1.2 phase table extended per §3.9; §8.3 step-group rows for plate/test unchanged.
- `standard/src/content/docs/appendix/A-grammar.mdx` — productions updated per §3.10.
- `standard/src/content/docs/appendix/B-rationale.mdx` — B.4.7 ("why plate is one command") replaced with the rationale subsections in §7 of this design.
- `standard/src/content/docs/appendix/D-changes.mdx` — CS-NNNN entry.
- `standard/conformance/positive/` and `negative/` — fixtures updated per §3.4.1's worked examples and the new diagnostics in §3.4 / §3.5; new fixtures cover (a) one-to-one shell plate, (b) many-to-one shell plate, (c) one-shot shell test, (d) one-to-one Lua plate, (e) many-to-one Lua test, (f) one-shot Lua plate, (g) `{out}` rejection in plate body, (h) mixed `{in}` + `{all}` rejection, (i) mixed `input` + `inputs` rejection, (j) `{lib.X}` rejection in plate body, (k) bare `{stem}` rejection diagnostic, (l) one-to-one mode on a recipe with ingredients but no preceding cook.
- `examples/*/Cookfile` — every example using `plate` / `test` migrates surface; `examples/iteration_benchmarks/Cookfile` gains plate/test mode coverage analogous to the cook benchmark recipes.
- `cook_modules/*.lua` — modules that emit plate/test surface text update their string-build paths.
- `tree-sitter-cook/test/corpus/` — corpus fixtures parallel the conformance suite per §3.11.
- `Cookfile` (top-level) and any in-repo `Cookfile`s — mechanical rewrite per the rules above.

## 5. Implementation impact

### 5.1. `cli/crates/cook-lang`

- `ast::PlateStep` loses its `command: String` field; gains a `body: Body` field where `Body ::= ShellBlock(String) | LuaBlock(String)`. The `Body` enum is shared with `cook_step::UsingClause` (or, equivalently, the `UsingClause` enum is renamed/promoted to a top-level `Body` type — implementation-plan call).
- `ast::TestStep` mirrors `PlateStep` and retains its `timeout: Option<u64>` / `should_fail: bool` fields.
- `recipe.rs`'s plate/test parsing path drops the `STRING` arm; only the `{` (shell block) and `>{` (Lua block) dispatches admit. The `>>{` prefix is rejected with a diagnostic identical in shape to cook's existing rejection (App. A.4 "`using >>{` is rejected" rule, generalized).
- The parser-level `plate_step` / `test_step` tests (`tests.rs`) update their fixtures to the new surface.
- The diagnostic for the removed `STRING` form names the migration target: "the `plate \"cmd\"` form was removed in CS-NNNN; rewrite as `plate { cmd }`."

### 5.2. `cli/crates/cook-luagen`

- `plate_step.rs::generate_plate_step` and `test_step.rs::generate_test_step` are rewritten to dispatch on `Body` and on the body's mode (one-to-one / many-to-one / one-shot, per §3.4). The current "always emit a `for` loop" pattern is replaced with the three-arm match described in §3.8.
- A new `template::validate_plate_test_placeholders(body, mode)` pass walks each plate/test body and rejects placeholders that don't belong to the step's mode (§3.5's table). The same pass handles `{out}`-family rejection and bare `{stem}`-family rejection; the diagnostics name the migration target.
- A new `template::detect_plate_test_mode(body)` function computes mode from body content per §3.4 — for shell, by scanning `{in}` / `{all}` placeholder presence; for Lua, by whole-word identifier scan over `input` / `inputs` with the §2.9 string/comment/long-string ignore rules.
- `template::expand_template_to_lua_with_deps` is reused for placeholder substitution; the existing `expand_plate_cmd_with_deps` and `expand_test_cmd_with_deps` helpers are removed (they hardcoded `{out}` → iteration-binding, which is no longer the rule). The plate/test codegen passes `_plate_in` / `_plate_all` (or `_test_in` / `_test_all`) as the iteration-binding name through the standard expansion path.
- `dep_ref::extract_brace_tokens` already operates over the raw command string and continues to work; the `Step::Plate` and `Step::Test` arms in `dep_ref.rs:46-47` switch from `&plate_step.command` to `body.text()` (the body's raw shell or Lua content).
- `recipe.rs`'s plate/test step-emission arms call the rewritten `generate_*` functions; the `last_cook_index` / `prev_cook_index` lookup retains its current shape but the iteration source becomes a flattened list of all units' outputs (per §3.3).

### 5.3. `tree-sitter-cook`

Per §3.11. The corpus updates land alongside the grammar.js changes; the conformance harness already covers the parallel between tree-sitter and the Rust parser.

### 5.4. Conformance fixtures

Per §4. The conformance harness at `standard/conformance/` regenerates against the new AST. New positive fixtures cover the six §3.4.1 shapes (one-to-one / many-to-one / one-shot, each in shell and Lua); new negative fixtures cover each rejection in §3.4 and §3.5.

## 6. Open questions

None blocking. Three design choices the spec makes explicitly that the implementation plan should call out:

1. **Body content drives mode for plate/test, but output pattern drives mode for cook.** This is asymmetric with CS-0022 by necessity: cook has an output pattern as a stronger signal, plate/test do not. The asymmetry is acceptable because the *user's mental model* is uniform — "the body determines what runs and how many times" — and CS-0022's footgun (a body signal contradicting an output-pattern signal) cannot occur when there is no output pattern.

2. **Lua identifier scan for `input` vs `inputs`.** This is a textual scan that respects the existing brace-balance lexer's ignore rules. An alternative is to admit both bindings in every Lua body and have the runtime detect "which one was actually read" — but that pushes the iteration shape past register-time, defeating the load-time `cook.add_unit` count and breaking the DAG's static-shape invariant. Static scan is required.

3. **One-to-one + empty-source rejection.** A plate that says `plate { ./{in} }` in a recipe with no preceding cook and no ingredients is a load-time error rather than "zero units, no work." The error catches a real authoring bug (you wrote `{in}` so you expected something to iterate); the silent-zero-units path defers the bug to "nothing happened, why?" debugging.

## 7. Rationale (informative annex draft for App. B.4)

A new App. B.4 rationale subsection covers four points to be added to the Standard alongside the normative changes. The existing B.4.7 ("Why `plate` is one command template, not a list") is **deleted** — its premise (that richer plate forms require a Lua block) is invalidated by this design, which gives plate the same body grammar as cook.

- **Why plate/test are cook steps with no declared outputs.** Once cook gained block bodies and three iteration modes (CS-0022), the only difference between cook and plate/test was the presence of an output declaration. Modeling plate/test as "cook with no outputs" makes the body grammar uniform, the placeholder vocabulary uniform, the cross-recipe rules uniform, and the substitution timing uniform. The author's mental model collapses from three step kinds with three surfaces to three step kinds with one surface.

- **Why iteration mode is deduced from the body for plate/test.** Cook owns iteration mode via the output pattern because the output pattern is *declarative* — it states the unit's filenames and falls out the iteration shape. Plate/test have no outputs to declare, so the body's placeholder content is the only available signal. Deducing mode from body content is the same footgun CS-0022 banned for cook only when an output pattern says one thing and a body says another. With no output pattern, there is no contradicting signal; body deduction is sound.

- **Why `{out}` is rejected in plate/test bodies.** CS-0022 fixed `{out}` to mean the unit's declared output. Plate/test have no declared output, so `{out}` has no referent. The pre-CS-0022 plate surface used `{out}` to mean the iteration item — but that is what `{in}` means everywhere else in the language, and a per-step-kind name for "the iteration item" would re-introduce the position-dependent rule that CS-0022 paid to remove.

- **Why no `{lib.ACCESSOR}` in plate/test bodies.** Plate/test have no output pattern and therefore cannot declare a `lib`-driven iteration. §5.4's firewall — "`{lib.ACCESSOR}` is rejected in any using-clause body" — applies trivially: there is never a position in a plate/test body where `{lib.ACCESSOR}` could be valid. The diagnostic preserves the same wording as cook's.

- **Why the Lua identifier scan and not a runtime decision.** A Lua body that references both `input` and `inputs` is making contradictory claims about iteration shape. Detecting that at register time produces a sharp diagnostic with a line number; deferring it to "the runtime decides which binding is bound based on which one was named" would either require dynamic mode selection (incompatible with the static-shape DAG) or two distinct mode-resolution passes (one syntactic for shell, one runtime for Lua). One pass, applied uniformly, is the better trade.

## 8. Acceptance criteria

The Standard PR for CS-NNNN is acceptance-complete when:

1. §4.7, §4.8, §5.4, §5.5, §6.4, §8.1.2, App. A.4, and App. B.4 are updated as described.
2. The conformance fixtures listed in §4 exist and pass.
3. The reference implementation (`cook-lang`, `cook-luagen`) and `tree-sitter-cook` parse, codegen, and highlight the new surface; the `cargo test --workspace` and `cargo test -p cook-lang --test conformance` suites are green.
4. Every Cookfile in `examples/`, `cook_modules/`, the top-level repo, and the `tree-sitter-cook/` subproject uses the new surface.
5. `examples/iteration_benchmarks/` adds plate/test recipes covering each mode (one-to-one / many-to-one / one-shot, each in shell and Lua), parallel to the existing eight cook-mode recipes.
6. The Standard's "Recent Changes" appendix (App. D) gets an entry naming this CS-NNNN and the migration recipe.
