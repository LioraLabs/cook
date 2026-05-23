# Cook CLI changelog

## v0.4.1 — 2026-05-23

Claims Cook Standard v0.11.

### Fixed

- **Chore-param sibling-validation regression (COOK-61).** Invoking any chore
  in a Cookfile no longer triggers required-no-default parameter validation
  on unrelated sibling chores. The Standard §7.5.1 register-time check is
  now correctly scoped to chores reachable from the dispatch target via the
  `requires` graph — unrelated parametric siblings are skipped (mirroring
  the no-target `cook list` path). Authors who worked around v0.4.0 by
  giving every required param a `=""` default may tighten back to the
  required form.

### Cookfile language (Cook Standard v0.11)

- **CS-0088 — §7.5.1 Note 7.5.1.1 (informative).** Makes explicit that the
  register-time parameter check in §7.5.1 ("a parametric chore depended on
  by the dispatch target runs with no argv supplied; a required parameter
  without a default is a configuration error") is scoped to chores
  reachable from the dispatch target. Unreachable parametric siblings are
  not validated during a given dispatch; their bodies are not invoked.

## v0.4.0 — 2026-05-23

Claims Cook Standard v0.11.

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

### Cookfile language (Cook Standard v0.11)

- **CS-0085 — `outputs[]` accepts glob patterns** with post-execute
  resolution. Recipes that produce a dynamic file set (e.g. compiled
  artifacts whose names aren't known up front) can now declare
  `outputs = {"build/**/*.o"}` and have the glob expanded after the
  step runs.
- **CS-0078 — multi-line `cook` outputs and ingredients.** The shorthand
  forms now span lines for readable long-list declarations.
- **CS-0079 — `fs.glob` accepts an array of patterns**, removing the
  earlier `fs.glob_many` workaround.
- **tree-sitter-cook v0.12 conformance audit (CS-0086 / COOK-50..57)** —
  closes out the long-running tree-sitter grammar gap against the
  Standard. Affects editor tooling, not the runtime.
