# 07 — testing

Tests are steps. They live next to the recipe they check, reference its
outputs with `$<recipename>`, run with the build, and **cache like build
steps** — a passing test whose inputs didn't change does not re-run.
There are no modifiers: a test either has an iteration source (one unit
per item) or is naked (one unit, unconditional), and it either passes or
it doesn't.

```
recipe check: app
    ingredients "tests/*.sh"
    test { ./$<in> }
    test { ! grep -q lowercase $<app> }
```

- `ingredients "tests/*.sh"` + `test { ./$<in> }` is **one-to-one**: every
  script under `tests/` runs as its own test unit, `$<in>` naming the
  current script.
- `test { ! grep -q lowercase $<app> }` is a **naked** one-shot test — no
  iteration source, exactly one unit. The `!` inverts a check that's
  expected to fail (no more `should_fail` modifier — invert the command
  in the body instead).
- unnamed tests display as `recipe@line`; there's no `as` override.

## Two ways to run

```
$ cook check          # tests run as part of the build

$ cook test           # the dedicated runner
running tests
test check@27 [tests/not_empty.sh] ... ok (cached)
test check@27 [tests/shouted.sh] ... ok (cached)
test check@28 ... ok (cached)
test result: ok. 3 passed (3 cached)
```

`cook test` gives per-test output, an honest exit code, and
`--rerun-failed` to iterate on a red run. Scope it like any target:
`cook test check`.
