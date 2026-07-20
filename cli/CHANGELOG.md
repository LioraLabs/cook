# Cook CLI changelog

## Unreleased

### Fixed

- **`tools { }` sealed values are portable across machines** (COOK-277,
  CS-0157). The native tools producer folded the resolved absolute path
  into the sealed value, so identical toolchains at different locations
  (homebrew vs `/usr/bin`, nix stores) produced different unit keys and
  heterogeneous fleets never shared sealed artifacts. The canonical value
  now carries the binary's content hash only. The resolved path still
  reaches consumers — `$<probe.NAME.path>` and `cook why`'s
  `(cc→/usr/bin/cc)` annotation — but from a per-run metadata channel
  resolved freshly on every run, which also fixes the staleness hazard
  where a cached value replayed a path recorded before the tool moved.
  Old path-bearing cached probe values are unreachable by construction
  (the produce body is part of the probe fingerprint and it changed);
  tools-sealing units re-key once.

### Added

- **`cook.tools.id(name)`** (COOK-277, CS-0158). Both-phase API returning
  `{ hash, path }` for a PATH-resolved tool — the machine-independent
  content-hash identity a module folds into a sealed probe value (per
  §12.7.5), and the current location for invocation. Backed by the
  fingerprint machinery's per-run tool-hash memo, so a 60MB binary is
  hashed at most once per run and never in Lua.

- **Edit-then-revert restores the cached artifact instead of re-executing**
  (COOK-278, CS-0156). Reverting a change (checkout, stash pop, toggling a
  diff to compare) re-ran discovered-input (depfile) units — a full
  `next build` each direction on the cap-cook showcase — because the fetch
  path probed the LAST run's concrete output names (wrong whenever output
  filenames depend on content, e.g. Next.js chunk hashes) and the
  discovered-inputs manifest kept only the newest discovered set. The store
  now retains every distinct discovered set per declared key (newest first,
  capped at 64), candidate keys are recomposed from current file content and
  validated by the key itself, and the winning key's determinant manifest
  supplies the true output list — which also lifts the old "glob outputs
  cannot cold-fetch" limitation. A fetch hit now records the same fat local
  entry an execution would (previously the thin entry silently re-fetched
  from the store on every subsequent run) and sweeps the edited build's
  stale outputs, so a revert settles to a plain local skip and the tree is
  byte-identical to a fresh build. Existing caches keep working unchanged in
  both directions — no key change, no store invalidation, and pre-fix
  binaries sharing a store still see the single-set manifest they expect.
- **`cook why` no longer reports a false shared miss for glob-output units.**
  The read-only shared probe used the raw declared patterns as artifact
  paths; it now probes the key's recorded concrete output list when the
  determinant manifest is present.

### Added

- **Warm re-runs name their cause inline** (COOK-276). When a unit with a
  previous fingerprint record misses its key, the build output says why at
  start of work — `Rebuilding web/.next — input changed:
  apps/web/app/.well-known/workflow/v1/manifest.json (+2 more)` (plain
  renderer: `rebuild (input changed: …)`), categorized as input
  changed/added/removed, env, command, seal, or output drift, capped at the
  first path plus a count. The diff is a map-compare of determinants already
  in hand at the miss — no new hashing on hits, default on, no flag. A cold
  unit (no history) is not attributed. The full listing stays in `cook why`.
  The local step index keys embed the env contribution, so an env flip used
  to present as an unattributable cold build; a sibling entry under the same
  output identity now attributes it to `env changed`. JSON progress events
  carry the cause as an additive `cause` field (schema v1, ignored by older
  readers).
- **`cook why` labels both cache tiers** (COOK-276). A locally-warm unit no
  longer leads with a bare `[MISS (shared)]` that reads as "will rebuild":
  statuses render as `[HIT (local), MISS (shared)]`-style dual labels, the
  shared tier is probed even on a local hit, and the JSON output gains
  explicit `local_hit` / `shared_present` fields alongside the legacy
  `status` string.

### Changed

- **Warm builds are quiet.** Per-node `Cached` lines are now held per recipe
  and only print when the recipe does real work; a recipe that finishes with
  nothing to do collapses to a single dim `Cached <recipe> (N nodes)` line
  (plain renderer: the `done (N/N cached)` row alone). A fully warm build
  prints one line per recipe plus `Finished in … (N nodes, all cached)`
  instead of hundreds of per-node rows. When real work does happen, held
  lines flush in front of it, capped at the per-recipe threshold with a
  single deferred `… (N more cached)` report. `--verbose` restores live,
  uncollapsed cached lines.
- **Toolchain probes group into one line.** `probe:<module>:<key>` nodes no
  longer print one row each; probes that actually ran collapse into
  `Resolved <module> toolchain for <recipe> (N probes) in …`, and a
  fully-cached probe set stays silent.
- **Internal recipes display their module tag.** Recipes following the
  double-underscore convention (`__cc_*` = internal cc tooling) render node
  lines under the module tag (`cc/build/config.h`) instead of the raw minted
  identifier, and print no queued/summary rows of their own.
- **Zero-node aggregator recipes** no longer print `queued (0 nodes)` /
  `done (0/0)` rows in the plain renderer (the inline renderer already
  suppressed them).
- **Fewer stray escapes.** The inline renderer only emits its
  clear-status-line sequence when an event actually prints while the status
  line is visible, so recorded ptys (script/asciinema/CI-with-tty) no longer
  fill with `\r\x1b[2K` runs; the final clear is skipped entirely when no
  status line was shown.

## v0.6.3 — 2026-07-19

Claims Cook Standard v0.14.

### Fixed

- **`cook affected` matches imported recipes** (COOK-274). An imported
  recipe's declared inputs are recorded relative to its own Cookfile's
  directory, while git changed paths are workspace-root-relative; the exact
  string intersection could therefore never match any imported recipe, so in
  a monorepo (the feature's flagship case) `cook affected` and
  `cook test --affected` silently selected nothing. Inputs are now re-rooted
  through the owning import's directory before intersecting. Also: git diffs
  run `--relative` so a workspace nested inside a larger repository compares
  in workspace-relative vocabulary, and `--recipe=<name>` now matches
  import-qualified names (`web.build`) in addition to pnpm-style task names
  (`web:build`).

## v0.6.2 — 2026-07-19

Claims Cook Standard v0.14.

### Fixed

- **Recipe names containing `/` persist to the recipe cache index** (COOK-273).
  A module-minted recipe like `@cap/env:build` (npm-scoped names from
  `cook_pnpm`) aimed its index write at a directory that never exists; the
  ENOENT was silently swallowed, so the recipe re-executed every run with no
  diagnostic. Cache file basenames now percent-encode the two path-hostile
  bytes (`%` → `%25`, `/` → `%2F`); every other name keeps its historical
  file name, so existing indexes are untouched. Failed index flushes now
  warn instead of being discarded.

## v0.6.1 — 2026-07-19

Claims Cook Standard v0.14.

### Fixed

- **CS-0155: literal-output steps in `ingredients <probe>` recipes gather.**
  Previously every cook step of a probe-driven recipe was member-iterated, so
  a literal-output step registered one colliding unit per member with the raw
  record JSON as `$<in>` — a tolerant command could cache a wrong artifact
  silently. A literal-output step now gathers the preceding step's collected
  outputs (the ordinary chained many-to-one, with real file input edges), so
  `fan out → pack` works in one recipe; a literal-output *first* step is a
  register-phase rejection (COOK-271).
- **`$<out_1>` / `$<out_2>` resolve in multi-output fan-out bodies.** The
  fan-out resolve context was hardcoded single-output, rejecting every
  indexed placeholder with a garbled count; it now follows the declared
  template count, and bare `$<out>` on a multi-output fan-out step is
  rejected as ambiguous, as elsewhere (COOK-270).

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
