CS-0239 §8.2 Form 3. `gather $<gen>` makes `gen`'s declared output paths the
members of `check-confs`, so the `test` step registers one unit per artifact.

This is the shape the language could not express before. Dep-driven iteration
(§10.4) declares its driver through an accessor in an OUTPUT PATTERN, and a
`test` step has no output pattern — so "validate each artifact `gen` produced"
had no spelling. Declaring the driver on the `gather` line is what reaches a
step kind that has none.

The parser dump pins the desugar (`MemberSource source=RecipeRef("gen")`, and
`inputs` staying empty — the members are not recipe inputs). The codegen
corpus only asserts that this fixture lowers without error; the emitted shape
— the member list coming from `cook.dep_output_list`, each member unit
declaring its member path as a file input, and `requires` carrying the edge
the name reference implies (§10.6) — is pinned by
`cli/crates/cook-luagen/tests/gather_recipe_ref.rs`.
