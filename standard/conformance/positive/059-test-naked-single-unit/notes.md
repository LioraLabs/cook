Pins Standard §5: a naked `test { cargo test }` with no `ingredients` and
no upstream `cook` step in the recipe has no `$<in>` accessor and no file
source. Per the CS-0135 two-mode rule this is the single-unit case (no
`$<in>` in the body): the engine maps it to `OneShot` (no source ⇒ no
cache key). It is admitted, not rejected — a source-less test always runs
uncached and is still reported (`... ok`, no `(cached)`), because rejecting
it would force a fake `ingredients` glob that produces a wrong cache key
for opaque test runners like `cargo test` / `pytest` / `go test`.

This fixture is parser-only (parse + AST shape); the OneShot runtime
behaviour is exercised by the engine/e2e suite, not this corpus.
