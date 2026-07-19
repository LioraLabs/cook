# Cook CLI changelog

## v0.6.0 — 2026-07-19

Claims Cook Standard v0.14.

### Changed

- **CS-0154: a brace-balanced block's body is the character span between the
  braces.** The opening-line remainder (`json { echo '[`, `>{ return {`) and
  the closing-line prefix are body segments instead of being silently
  discarded, and shell single-/double-quote state carries across lines — so a
  POSIX quoted string spanning newlines (inline JSON in a probe producer) and
  a heredoc opened beside the `{` now parse (COOK-267, COOK-268). The inline
  single-line form is the same span walk, fixing a latent quote-naive
  `{ echo '}' }` miscount. Text after a block's closing `}` is the enclosing
  production's trailer: cook/test modifier tails keep their meaning (now read
  from the exact close position), while stray trailer text on probe producers
  and chore Lua blocks is a parse error instead of being silently dropped.

### Fixed

- **Cold-restored units are recorded in the local cache index** (COOK-269).
  A unit served by a cold fetch-by-key from the shared store (fresh clone,
  lost `.cook`) restored its outputs but recorded nothing locally, so §17.7
  stale-output reconciliation had no prior state and outputs orphaned by a
  later shrink were never swept.

## v0.5.0 — 2026-07-18

Claims Cook Standard v0.14.

### Changed

- **`cook list` is now an alias for `cook menu`.** It previously printed bare
  names, one per line, as a machine-readable surface for pipelines such as
  `cook list | fzf | xargs -r cook`. Shell tab completion now covers name
  discovery, so the second render path earned nothing and has been removed:
  `cook list` prints exactly what `cook menu` prints, including each chore's
  parameters. A recipe named `list` is still reported by name in the
  shadowing notice, and is still buildable as `cook +list`.
- **`cook --version` now reports the release version.** Every crate previously
  hardcoded `0.1.0`, so every published binary self-reported `0.1.0` regardless
  of its tag. The version is now single-sourced from `[workspace.package]` in
  `cli/Cargo.toml`, and the release workflow refuses to build when the pushed
  tag disagrees with it.

### Removed

- **`cook list --recipes-only` / `--chores-only`.** Both filtered the bare
  listing that no longer exists. `cook menu` renders the kind of every entry.

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
