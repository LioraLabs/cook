CS-0185 §22.4. `suite` was removed with `cook.add_test` and is not accepted.

It captured nothing: it defaulted to the enclosing recipe's qualified name, was
excluded from the cache key as display metadata, played no part in the test
identity, and was ignored by the JUnit writer that derives `<testsuite>`
grouping per recipe. Passing it is an error rather than a silent no-op, because
a caller who writes it means something by it.

Register-phase, not syntactic: the tree-sitter harness records this as a
semantic skip.
