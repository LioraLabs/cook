# Cook CLI changelog

## Unreleased

### Breaking

- **The legacy "second bare positional = config preset" CLI rule is removed
  (COOK-36).** `cook NAME PRESET` no longer selects a config preset. Use the
  `@PRESET` sigil or the `--config PRESET` / `-c PRESET` flag. The diagnostic
  for a now-broken legacy invocation includes a migration hint suggesting
  the new form.

### Added

- **`cook affected --since=<git-ref>` (COOK-58)** — lists every recipe whose
  declared file inputs (or any transitive downstream consumer) would be
  invalidated by the diff between `<ref>` and the working tree. Three-dot
  merge-base semantics; includes staged + unstaged + untracked-non-ignored
  files. Supports `--recipe=<name>` (filter by base name) and `--json`.
- **`cook <recipe> --affected --since=<git-ref>` (COOK-58)** — drives the
  scheduler with the affected slice only. Same selection logic as
  `cook affected`, applied as a filter before the executor runs. Both flag
  orderings work: globals-first (`cook --affected --since=main build`) and
  Turborepo-style (`cook build --affected --since=main`).
- **Chore parameters (COOK-36)** — positional, defaulted-string,
  Lua-expression-default (`=(EXPR)`), and variadic (`+NAME`, `*NAME`) forms
  on chore headers. Parameters bind as Lua locals, as `$<name>` placeholders
  in shell steps, and as environment variables in shell child processes.
- **`@PRESET` sigil and `--config NAME` / `-c NAME` flag (COOK-36)** —
  equivalent forms for selecting a config preset on the CLI. The `--`
  end-of-options separator passes subsequent tokens through as literal
  parameter values (escape hatch for values starting with `@` or `-`).
