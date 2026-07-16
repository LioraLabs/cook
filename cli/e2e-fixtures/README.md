# e2e fixtures

Behavior-pinning corpus, relocated from `examples/` in the 2026-07-06
overhaul (COOK-194). These directories are **not documentation** — they are
audit repros, benchmark matrices, and characterization Cookfiles kept
runnable so the surface e2e conformance harness (COOK-192) can absorb them
as executable fixtures.

Conventions here are whatever each fixture needed at the time it pinned its
behavior; some deliberately use legacy or low-level API forms (that's the
point — they pin those paths). User-facing examples live in `examples/`.

## surface/ — surface conformance corpus

Executable conformance fixtures for every Standard-blessed surface
consumption form, run by the surface e2e conformance harness
(`cli/crates/cook-cli/tests/surface_conformance.rs`) against the real,
freshly-built `cook` binary.

**How to run.** From `cli/`:

    cargo test -p cook-cli --test surface_conformance -- --nocapture

or `cook e2e` from `cli/`, or as part of `cook test` from the repo root
(`cook test` runs the whole `cargo test` suite, which includes this one).
To run a single fixture, filter by substring:

    COOK_SURFACE_FIXTURE=<substring> cargo test -p cook-cli --test surface_conformance -- --nocapture

**Fixture layout.** Each `<NN-name>/` directory holds a Cookfile, its input
files, and an `expect.toml` describing one or more sequential `cook`
invocations with exit-code and filesystem assertions. See the top doc
comment in `surface_conformance.rs` for the full `expect.toml` schema —
it is not duplicated here.

**Cache isolation.** Every fixture runs in a tempdir copy of its directory
with a private cache: the harness generates a `.cook/cloud.toml` pointing
at a tempdir `cache_dir` and sets an `XDG_CACHE_HOME` guard for the
subprocess. The user's real `~/.cache/cook/cloud` is never read or written.

**The expected-fail convention.** `xfail = "<ISSUE-KEY>"` in a fixture's
`expect.toml` pins an open bug: the fixture asserts the *post-fix* contract
and is expected to fail right now. When the fix lands, the harness reports
an XPASS failure telling you to delete the `xfail` line — flipping a
fixture from red to green is a mechanical one-line edit, not a rewrite.

**The review convention.** Any Standard change that adds or alters a
surface consumption form must add or update a fixture here. Absence of a
corresponding fixture fails review.
