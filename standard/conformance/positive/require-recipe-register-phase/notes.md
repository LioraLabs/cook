Pins Standard §22.8 / CS-0144: a bare `cook.require_recipe("producer")` line
inside a recipe body classifies as CS-0134's register-phase `InlineLua` step,
the same shape as `cook.recipe_name()` (`recipe-name-register-phase/`) and
`cook.log(...)` (`recipe-body-bare-module-call-is-register/`) — the
`<id>.<id>(...)` bare-module-call rule does not special-case the `cook` prefix,
and a string argument does not change the classification.

This fixture is parse-only by design: it carries no `register_ok.txt`, because
`producer` is not declared here and §22.8 requires a register-phase error for a
name not registered in the current pass. What is pinned here is the syntactic
classification, nothing more.

Runtime semantics — the register-order guarantee (forcing `producer`'s body to
completion before the call returns), the dep-list-equivalent edge merged into
`requires`, bare-name resolution, and the error contract — are pinned by the
cook-register tests (`cli/crates/cook-register/src/tests.rs`) and by the
engine's cross-recipe edge tests
(`cli/crates/cook-engine/tests/module_declared_cross_recipe_edge.rs`). The
outside-a-recipe-body rejection is additionally pinned as an executable
assertion by
`standard/conformance/negative/require-recipe-outside-recipe-body-rejected/`.
