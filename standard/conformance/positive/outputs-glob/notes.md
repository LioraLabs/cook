# outputs-glob

Covers CS-0085: `outputs[]` accepts glob pattern entries alongside literal
paths. The pattern entries (`"build/**"`, `"dist/*.js"`) pass through the
parser unchanged — they are just strings inside the Lua table at parse
time. Resolution happens post-execute and is specified normatively in
§17.6; engine wiring is exercised end-to-end by
`cli/crates/cook-engine/tests/outputs_glob_e2e.rs` and the cross-recipe
terminality rule by `cli/crates/cook-engine/tests/cross_recipe_glob_edges.rs`.
