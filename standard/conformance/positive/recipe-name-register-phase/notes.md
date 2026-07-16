Pins Standard §22.7 / CS-0141: a bare `cook.recipe_name()` line inside a
recipe body classifies as CS-0134's register-phase `InlineLua` step, the
same shape as `cook.log(...)` and `recipe-body-bare-module-call-is-register/`
(the `<id>.<id>(...)` bare-module-call rule does not special-case the `cook`
prefix). Runtime semantics — returning the fully-qualified enclosing recipe
name and hard-erroring when called outside a recipe body — are pinned by the
cook-register tests (`cli/crates/cook-register/src/tests.rs`). The
outside-a-recipe-body rejection is additionally pinned as an executable
assertion by `standard/conformance/negative/recipe-name-outside-recipe-body-rejected/`.
