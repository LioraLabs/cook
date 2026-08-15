Pins `gather` with multiple includes and
excludes resolves to `(union of includes) \ (union of excludes)`,
order-independent. The codegen iteration source for `cook` and `test`
steps must read the merged set, not only the first pattern. Behavioral confirmation lives in the
cook-luagen unit tests; this fixture pins parse + codegen success
for the surface form.
