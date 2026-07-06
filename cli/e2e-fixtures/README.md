# e2e fixtures

Behavior-pinning corpus, relocated from `examples/` in the 2026-07-06
overhaul (COOK-194). These directories are **not documentation** — they are
audit repros, benchmark matrices, and characterization Cookfiles kept
runnable so the surface e2e conformance harness (COOK-192) can absorb them
as executable fixtures.

Conventions here are whatever each fixture needed at the time it pinned its
behavior; some deliberately use legacy or low-level API forms (that's the
point — they pin those paths). User-facing examples live in `examples/`.
