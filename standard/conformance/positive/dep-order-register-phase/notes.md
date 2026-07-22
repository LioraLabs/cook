Pins Standard §22.10 / CS-0161: a bare `cook.dep_order("producer")` line inside
a recipe body classifies as CS-0134's register-phase `InlineLua` step, exactly
as `cook.require_recipe("producer")` does (`require-recipe-register-phase/`).
The `<id>.<id>(...)` bare-module-call rule does not special-case the `cook`
prefix, and a string argument does not change the classification. The two
fixtures are deliberately parallel: `cook.dep_order` is the fine-grained
replacement for `cook.require_recipe` on the link path, and nothing about the
substitution is visible to the parser.

This fixture is parse-only by design: it carries no `register_ok.txt`, because
`producer` is not declared here and §22.10's register-order guarantee requires
forcing a recipe registered in the current pass. What is pinned here is the
syntactic classification, nothing more.

Runtime semantics are pinned as executable assertions elsewhere:

  - the register-order guarantee (forcing `producer`'s body to completion
    before the call returns, to the identical standard §22.8 requires) and the
    no-body-invocation-driver error contract — `cli/crates/cook-register/`
    (`context.rs`'s `register_dep_order_forcing`);
  - the per-unit edge, the absence of any cache-input fold, and the shared ref
    namespace with `cook.dep_output` —
    `cli/crates/cook-register/src/dep_output_api.rs` tests;
  - closure membership without a dep-list entry or `cook.require_recipe`
    (§22.10, "Closure membership is established") —
    `cli/crates/cook-engine/tests/dep_order_e2e.rs`, and the `orders` field
    threaded through `analyzer.rs` / `pipeline/recipe_info.rs`;
  - the empty-`step_group` idiom that separates the force from the edge
    (Note 22.10.1) — `cook_cc` 0.17.0's `declare_link_deps`, with its own
    spec coverage in `cook-modules/cook_cc/spec/`.

What this fixture must NOT grow into: an assertion that `cook.dep_order`
suppresses anything. The withdrawn fine-covered narrowing rule (App. E CS-0161,
"Rejected alternative") let a fine reference cancel a coarse one; the shipped
design is strictly additive, and a recipe that declares `requires` keeps
byte-identical whole-recipe ordering whether or not its units carry fine refs.
