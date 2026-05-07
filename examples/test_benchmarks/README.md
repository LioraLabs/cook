# `examples/test_benchmarks/`

Fixture pinning the v1.0 test runner across every shape of test the runner
handles. Paired with `walkthrough.sh`, which is the runtime conformance pin
in CI.

Recipes:

| Recipe | Pins |
|---|---|
| `pass_basic` | Green path, terminal output, cache write |
| `pass_iterated` | Per-iteration discriminator, 12 separate cache entries |
| `pass_should_fail` | `should_fail` semantics survive caching |
| `fail_basic` | Failure capture, never-cached invariant |
| `fail_partial` | Continue-past-failure, mixed pass/fail aggregation |
| `blocked_by_build` | Blocked status (upstream cook failed) |
| `slow_timeout` | Timed-out outcome |
| `named_test` | `as 'name'` modifier (Phase 2.5+) |
| `cached_replay` | Cache hit on second run |
| `rerun_failed_set` | `--rerun-failed` selection |

Run: `cook --test` (after Phase 4) — see `walkthrough.sh` for the full
conformance assertions.
