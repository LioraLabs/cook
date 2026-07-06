# 07 — testing

Tests are steps. They live next to the recipe they check, reference its
outputs with `$<recipename>`, run with the build, and **cache like build
steps** — a passing test whose inputs didn't change does not re-run.

```
recipe check: app
    test { grep -q HELLO $<app> } as 'greeting is shouted'
    test { test -s $<app> } as 'artifact not empty' timeout 5
    test { grep -q lowercase $<app> } as 'no lowercase leaked' should_fail
```

- `as 'name'` — display name (single-quoted)
- `timeout N` — seconds before the test is killed
- `should_fail` — the check is *expected* to fail; use it to pin error
  behavior (a rejected input, a lint that must fire)
- modifier order is fixed: `as` → `timeout` → `should_fail`

## Two ways to run

```
$ cook check          # tests run as part of the build

$ cook test           # the dedicated runner
running tests
test check::greeting is shouted ... ok (cached)
...
test result: ok. 3 passed (3 cached)
```

`cook test` gives per-test output, an honest exit code, and
`--rerun-failed` to iterate on a red run. Scope it like any target:
`cook test check`.
