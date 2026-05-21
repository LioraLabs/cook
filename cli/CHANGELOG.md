# Cook CLI changelog

## Unreleased

### Breaking

- **The legacy "second bare positional = config preset" CLI rule is removed
  (COOK-36).** `cook NAME PRESET` no longer selects a config preset. Use the
  `@PRESET` sigil or the `--config PRESET` / `-c PRESET` flag. The diagnostic
  for a now-broken legacy invocation includes a migration hint suggesting
  the new form.

### Added

- **Chore parameters (COOK-36)** — positional, defaulted-string,
  Lua-expression-default (`=(EXPR)`), and variadic (`+NAME`, `*NAME`) forms
  on chore headers. Parameters bind as Lua locals, as `$<name>` placeholders
  in shell steps, and as environment variables in shell child processes.
- **`@PRESET` sigil and `--config NAME` / `-c NAME` flag (COOK-36)** —
  equivalent forms for selecting a config preset on the CLI. The `--`
  end-of-options separator passes subsequent tokens through as literal
  parameter values (escape hatch for values starting with `@` or `-`).
