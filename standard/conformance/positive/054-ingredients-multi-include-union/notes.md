Pins Standard §4.3: an `ingredients` line with multiple includes and
excludes resolves to `(union of includes) \ (union of excludes)`,
order-independent. The codegen iteration source for `cook`, `plate`,
and `test` steps must read the merged set, not the per-pattern table
`recipe.ingredients[1]`. Behavioral confirmation lives in the
cook-luagen unit tests; this fixture pins parse + codegen success
for the surface form.
