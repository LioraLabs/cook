Pins the milestone's central recipe-body shape (COOK-246): a maker call whose
table argument spans multiple lines, with a nested call (`srcs("neo/idlib")`)
on the first line and continuation lines carrying list-valued keys — two of
which (`test = {...}` and `ingredients = {...}`) collide lexically with real
step keywords (`test_step`'s `test` and the recipe-level `ingredients`
keyword, respectively). Per App. A.4's `module_call` production ("Braces may
span subsequent lines") and the Step-dispatch priority cascade
(§{steps.dispatch} rule 6, reinstated by CS-0134), a recipe-body
`module_call` is collected by brace-balancing across lines *before* any
per-line keyword dispatch runs — so neither the `test = {...}` nor the
`ingredients = {...}` continuation line is ever mistaken for a `test_step` or
an `ingredients` opener. This is the one-step invariant the cook_cc 0.13.0
rewrite depends on: the whole call parses to a single `Step::InlineLua`, not
a `test_step`/`ingredients` misparse followed by parse garbage.

The fixture asserts exactly one `InlineLua` step whose `code` joins all five
source lines with `\n`: the first line is stored trimmed (its leading
indentation is dropped), while continuation lines retain their original
source indentation verbatim. This first-line/continuation-line asymmetry is
existing `collect_module_call` behaviour (`cli/crates/cook-lang/src/recipe.rs`)
and is pinned here as-is, not endorsed as ideal.

Parse-only scope: `cook_cc` and `srcs()` are not resolved or executed by this
harness; only the AST shape is asserted. `Step::InlineLua` is itself a
parse-time artifact, not a codegen guarantee: this fixture proves nothing
about what codegen does with it. Separately, `cook-luagen`'s
`codegen_positive_conformance_corpus` sweep
(`cli/crates/cook-luagen/tests/conformance.rs:112`) runs every positive
fixture, including this one, through `generate_with_names_checked` and
asserts only that codegen does *not reject* it — it does not assert anything
about the shape or count of the emitted Lua.
