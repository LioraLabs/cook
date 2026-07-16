Pins the milestone's central recipe-body shape (COOK-246): a maker call whose
table argument spans multiple lines, with a nested call (`srcs("neo/idlib")`)
on the first line and continuation lines carrying list-valued keys — one of
which (`test = {...}`) collides lexically with the `test` step keyword. Per
App. A.4's `module_call` production ("Braces may span subsequent lines") and
the Step-dispatch priority cascade (§{steps.dispatch} rule 6, reinstated by
CS-0134), a recipe-body `module_call` is collected by brace-balancing across
lines *before* any per-line keyword dispatch runs — so the `test = {...}`
continuation line is never mistaken for a `test_step` opener. This is the
one-step invariant the cook_cc 0.13.0 rewrite depends on: the whole call
compiles to a single register-phase `Step::InlineLua`, not a `test_step`
followed by parse garbage.

The fixture asserts exactly one `InlineLua` step whose `code` joins all four
source lines with `\n`: the first line is stored trimmed (its leading
indentation is dropped), while continuation lines retain their original
source indentation verbatim. This first-line/continuation-line asymmetry is
existing `collect_module_call` behaviour (`cli/crates/cook-lang/src/recipe.rs`)
and is pinned here as-is, not endorsed as ideal.

Parse-only scope: `cook_cc` and `srcs()` are not resolved or executed by this
harness; only the AST shape is asserted.
