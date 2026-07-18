## Supporting Modules: Analyzer, Watcher, Env, Progress

Four focused modules that underpin Cook's build pipeline. The analyzer determines which recipes to run and in what order; the watcher powers `cook serve`; the env loader feeds the layered variable resolution applied before each build; and the progress crate renders build output to the terminal and persists build logs for post-hoc inspection.

Module locations have shifted as the build system was split into workspace crates. Old single-tree paths (`src/analyzer/`, `src/watcher/`, `src/env/`) no longer exist — every module now lives inside one of the `cli/crates/*` crates.

---

## 1. Analyzer (`cli/crates/cook-engine/src/analyzer.rs`)

### Purpose

The analyzer determines the execution order of recipes. It runs once early in every build (and every `cook serve` rebuild) and returns either an ordered list of recipe names (`topological_sort`) or a recipe -> dependency-names map (`dependency_edges` / `dependency_edges_multi`). The scheduler enqueues recipes in the order returned here.

The module works entirely with string-keyed `BTreeMap<String, RecipeInfo>` — it does not depend on the parser AST. The translation from `cook_lang::ast::Cookfile` to `RecipeInfo` is done by `pipeline::recipe_info::build_single_recipe_infos` (`cli/crates/cook-engine/src/pipeline/recipe_info.rs:22`) and `pipeline::recipe_info::build_workspace_recipe_info`.

### Structures

`RecipeInfo` is defined at `cli/crates/cook-engine/src/analyzer.rs:37`:

```rust
pub struct RecipeInfo {
    pub ingredients: Vec<String>,
    pub serves: Vec<String>,
    pub requires: Vec<String>,
}
```

`ingredients` are glob patterns the recipe consumes; `serves` are cook-step output patterns; `requires` are the recipe names listed after `:` in the recipe header. The shape matches the historical struct exactly, but the file it lives in has moved.

`WorkspaceLayout` (`analyzer.rs:188`) and the helper `NamespaceEntry = (PathBuf, String, PathBuf)` carry the canonical-path + import-name information needed to compute fully-qualified recipe names across imported Cookfiles. `build_workspace_recipe_info(layout)` returns a `BTreeMap<String, RecipeInfo>` whose keys are dotted-prefix names like `"backend.proto.generate"`.

### Algorithms

**Adjacency.** `build_adjacency` (`analyzer.rs:55`) maps each recipe name to the `BTreeSet<&str>` of names it depends on. **Edges come from explicit `requires` only.** Every other kind of cross-recipe edge enters the pipeline elsewhere.

> **Important: implicit ingredient-serves matching has been removed.**
>
> The historical rule — "if recipe A's `ingredients` contains a path string that another recipe B has in its `serves`, infer A depends on B" — is gone. `build_adjacency` no longer looks at `ingredients` or `serves` at all; it only resolves `requires`. Path-string equality between an ingredient and another recipe's cook-output is opaque and produces no edge. See Cook Standard § 5.6 and rationale B.5.N. The removal is pinned by `test_ingredient_serves_string_match_is_opaque` and `test_path_match_does_not_imply_dep` (`analyzer.rs:622`, `analyzer.rs:640`), and `test_dependency_edges_no_implicit_via_serves` (`analyzer.rs:836`) confirms the same for `dependency_edges`.
>
> Cross-recipe edges from name references in recipe bodies (`{lib}` / `{lib.accessor}`) are not produced by the analyzer either. They are extracted by codegen — `cook_luagen::dep_ref::extract_dep_refs` (driven from `cli/crates/cook-engine/src/pipeline/inferred_deps.rs:27`, `:155`) — and stitched into the runtime DAG separately as "inferred deps" rather than walked through `build_adjacency`. The analyzer's job is now strictly the explicit-`requires` graph.

**Topological sort.** `topological_sort(recipes, target)` (`analyzer.rs:128`) is a recursive DFS with three node states (`Unvisited` / `Visiting` / `Visited`). Returns recipes in post-order DFS, so index 0 has no remaining dependencies and the target recipe is last.

- **Only reachable recipes are returned.** Recipes elsewhere in the Cookfile that are not reachable from `target` never appear in the output. Pinned by `test_only_needed_recipes_included` (`analyzer.rs:733`).
- **Diamond dependencies are emitted once.** The `Visited` state short-circuits re-entry.

**Dependency edges.** `dependency_edges(recipes, target)` (`analyzer.rs:74`) computes `topological_sort` and then projects each reachable recipe's adjacency to a sorted `Vec<String>`, filtered to dependencies that are themselves reachable. `dependency_edges_multi` (`analyzer.rs:103`) merges per-target results into a single map.

**Workspace recipe registration for `cook test`.** `register_workspace_for_test(project_root)` (`analyzer.rs:338`) walks the import graph from `project_root/Cookfile`, BFS-deduplicating by canonical path. Every reachable recipe is registered — even ones not referenced by any target — so `cook test` can discover all `test_step` units across the workspace. Imports use `cook_lang::ast::ImportPath::Tree` (resolved relative to the importing dir) or `ImportPath::Sigil` (resolved relative to `project_root`).

### Errors

`GraphError` (`analyzer.rs:17`) has four variants:

| Variant | Condition |
|---|---|
| `GraphError::CycleDetected(name)` | A node was encountered while already in `Visiting` state (direct self-dep or transitive cycle). |
| `GraphError::UnknownRecipe(name)` | `target` is not in the recipes map, or some recipe's `requires` names a recipe that doesn't exist. |
| `GraphError::Io(msg)` | Filesystem error while walking the workspace import graph (only emitted by `register_workspace_for_test` / `build_recipe_info_for_targets`). |
| `GraphError::Parse(msg)` | `cook_lang::parse` failed on a Cookfile encountered during workspace walk. |

CLI-side translation to `CookError` lives in `cli/crates/cook-cli/src/pipeline.rs` (see `cmd_serve` at `pipeline.rs:1116`).

---

## 2. Watcher (`cli/crates/cook-cli/src/watcher.rs`)

### Purpose

The watcher powers `cook serve` — the continuous rebuild mode. It watches ingredient directories and every Cookfile in the workspace for changes, debounces rapid filesystem events, and invokes a callback that triggers a rebuild.

### Structures

```rust
pub struct CookWatcher {
    pub globs: Vec<String>,
    pub cookfile_paths: Vec<PathBuf>,
}
```

Defined at `cli/crates/cook-cli/src/watcher.rs:8`. Note `cookfile_paths` is **plural** — workspaces can have multiple Cookfiles via `import`, and every imported Cookfile is watched so a change in any of them re-parses and rebuilds.

`globs` is populated by `CookWatcher::collect_globs_for_recipes(cookfile, recipe_names)` (`watcher.rs:21`), which iterates the recipes of a *single* `cook_lang::ast::Cookfile` and collects the `ingredients` patterns of every recipe whose name appears in `recipe_names`. The function takes one Cookfile at a time; the workspace driver (`cmd_serve` in `cli/crates/cook-cli/src/pipeline.rs:1087`) collects globs per Cookfile and accumulates `cookfile_paths` for every imported file.

### Algorithms

**Directory registration.** `watch` (`watcher.rs:46`) creates a `notify::RecommendedWatcher`. For each glob pattern, it extracts the parent directory via `Path::new(pattern).parent()` and — if the directory exists and has not already been registered — calls `watcher.watch(dir, RecursiveMode::Recursive)` (`watcher.rs:61-67`). A glob like `"src/**/*.c"` therefore watches the entire `src/` tree.

Each Cookfile's parent directory is then registered with `RecursiveMode::NonRecursive` (`watcher.rs:69-75`), so a Cookfile edit fires but unrelated siblings of the Cookfile do not. The `watched_dirs` `HashSet` deduplicates registrations across the two passes.

**Debounce.** A trailing 200 ms debounce (`watcher.rs:77-78`): `let debounce = Duration::from_millis(200)`, with `last_trigger` checked against `Instant::now()` before each callback invocation. Bursts of filesystem events (editor save fsync sequences, atomic-rename pairs) collapse into a single rebuild.

**Change classification.** When a relevant event arrives, the callback receives a boolean `cookfile_changed` (`watcher.rs:87-93`):

- `true` — the event's path list contains any of `self.cookfile_paths`. The caller is expected to re-parse the Cookfile from scratch.
- `false` — a non-Cookfile path matched one of the ingredient globs. The caller rebuilds from the already-parsed Cookfile.

An event with no Cookfile-matching path and no glob-matching path is ignored entirely.

Glob matching is done via `glob::Pattern::new(pattern).matches(&path)` in `matches_any_glob` (`watcher.rs:34`); patterns that fail to compile are silently skipped.

### Interactive-step rejection

`cook serve` rejects any recipe whose body contains an `@`-prefixed interactive shell step. The check lives in `cmd_serve` (`cli/crates/cook-cli/src/pipeline.rs:1097-1111`), before `CookWatcher` is constructed — the watcher itself has no concept of interactivity.

---

## 3. Env (`cli/crates/cook-engine/src/pipeline/env.rs` + `cli/crates/cook-cli/src/pipeline.rs`)

### Purpose

Build a single `HashMap<String, String>` of environment variables to hand to the runtime, from three layered sources. The result is the input env for every shell step the engine runs.

There is no longer a `src/env/mod.rs`. The `.env` reader and the layered merge both live in `cli/crates/cook-engine/src/pipeline/env.rs`; the CLI wires them together in `cli/crates/cook-cli/src/pipeline.rs` (see `cmd_run` at `pipeline.rs:477-479` and `cmd_dag` at `pipeline.rs:1231-1233`).

### Structures and functions

`load_env(cookfile_dir: &Path) -> HashMap<String, String>` (`env.rs:20`) reads `<cookfile_dir>/.env` via `dotenvy::from_path_iter`. Missing-file and parse-error cases return an empty map — the function never errors. The `dotenvy` crate handles standard `.env` syntax: comments (`# …`), blank lines, and single- or double-quoted values (quotes are stripped).

`resolve_env(selected_config, dotenv_vars, overrides) -> Result<HashMap<String, String>, PipelineError>` (`env.rs:33`) performs the layered merge.

`parse_cli_overrides(overrides) -> Result<HashMap<String, String>, PipelineError>` (`env.rs:60`) splits `KEY=VALUE` strings on the first `=`. Missing `=` produces `PipelineError::InvalidSet`. The engine also calls this directly to re-apply CLI overrides on top of any values a `config` block writes to `cook.env`.

### Algorithms

The merge order in `resolve_env` is (later layers overwrite earlier):

| Priority | Source | Notes |
|---|---|---|
| 1 (lowest) | `std::env::vars()` — system environment | `env.rs:41` |
| 2 | `.env` file | Loaded by `load_env`; merged at `env.rs:43-46` |
| 3 (highest) | `--set KEY=VALUE` CLI flags | Split via `parse_cli_overrides`; merged at `env.rs:48-51` |

Cookfile-defined variables are **not** a layer in `resolve_env`. The historical "bare cookfile vars" form (`CC "gcc"` at file scope) was removed — bare top-level declarations don't compose with named-config override, so the language now requires all Cookfile-level variables to be set inside a `config NAME ... end` block (or an unnamed `config ... end` block). Those values are applied at runtime by the Lua-executed config block, and CLI overrides (layer 3) are re-applied on top via `parse_cli_overrides` so explicit `--set` flags always win over config-block defaults. The `selected_config` argument to `resolve_env` is accepted but unused — kept only to avoid churning call sites; it flows separately to the runtime for config-block dispatch.

### Errors

`PipelineError::InvalidSet(arg)` — a `--set` argument without `=` (e.g. `--set DEBUG` instead of `--set DEBUG=1`). Surfaced by `resolve_env` and `parse_cli_overrides`.

---

## 4. Progress (`cook-progress` crate)

### Purpose

Renders build progress to the terminal and persists every build to `.cook/logs/<build-id>/` for post-hoc inspection. Owns no mutable build state beyond what is derived from `ProgressEvent` inputs.

### Architecture

cook-engine emits `EngineEvent`s over an `mpsc::Sender`. `cook-cli` bridges those to `cook_progress::ProgressEvent` (interning recipe/node names into typed `RecipeId`/`NodeId`, and tagging each node with a `NodeKind` — `Cooked` by default, `Test` for test-step nodes, with `Compile`/`Link`/`Resolve`/`Generate`/`Write` available for richer producers). The `Driver` consumes events, applies them to a pure `BuildState`, records them to an optional `LogStore`, then hands them to a `Renderer`.

```
cook-engine ──EngineEvent──▶ bridge ──ProgressEvent──▶ Driver
                                                         │
                                                         ├──▶ BuildState (pure state machine)
                                                         ├──▶ LogStore  (optional; writes .cook/logs/)
                                                         └──▶ Renderer
                                                                   ├── InlineRenderer (cargo-style, TTY default)
                                                                   ├── PlainRenderer  (non-TTY / CI)
                                                                   └── JsonWriter    (--output=json)
```

The wire-format schema version is pinned at `PROGRESS_SCHEMA_VERSION = 1` (`cli/crates/cook-progress/src/event.rs:20`); writers emit it as the top-level integer `v` field on every `events.jsonl` line, and readers refuse lines whose `v` exceeds the highest version they recognise.

### State model

`BuildState` is the single mutation point. `apply(&mut self, event: &ProgressEvent)` is the only way it changes. Everything else (rendering, logging) is a pure read.

- `order: Vec<RecipeId>` — topological order frozen at `BuildStarted`.
- `recipes: BTreeMap<RecipeId, RecipeState>` — per-recipe status, progress, nodes, cached count, error summary.
- `RecipeState.nodes: BTreeMap<NodeId, NodeState>` — live node tracking (artifact path, status, timestamps).
- Cached nodes are NOT stored individually; they collapse into `RecipeState.cached_count` to keep memory bounded for recipes with hundreds of cache hits.

Guards in `apply` prevent duplicate events from corrupting counters (important for log replay / recap).

The `ProgressEvent` enum (`cli/crates/cook-progress/src/event.rs:106`) currently carries `BuildStarted`, `RecipeStarted`, `RecipeCompleted` (with `RecipeKind::Recipe` or `RecipeKind::Chore` for the summary detail string), `RecipeFailed`, `NodeStarted`, `NodeCompleted`, `NodeFailed`, `NodeCacheHit`, `NodeSkipped`, `NodeOutput`, `InteractiveStart` (with `chore_step_count` for chore windows), `InteractiveEnd` (with `is_terminal` and the optional `failed_step` of a chore window), and `Finished`.

### Renderers

`cook-progress` exposes the `ProgressEvent` API and three renderers: `InlineRenderer` (cargo-style append-only event lines + sticky status line, default on TTY), `PlainRenderer` (append-only text, default off-TTY), and `JsonWriter` (one event per line, opt-in via `--output=json`). The inline renderer is built from two decoupled components — `EventWriter` for verb-prefixed event lines and `StatusLine` for a single redrawn bottom-of-terminal status line — coordinated through a shared stderr lock.

Selected by the driver based on flags + environment:

| Context | Renderer |
|---|---|
| stderr is a TTY | InlineRenderer (cargo-style verbs + sticky status line) |
| stderr not a TTY, or `--no-ui`, or `--output=plain`, or `CI=true`, or `TERM=dumb` | PlainRenderer |
| `--output=json` | JsonWriter |

**InlineRenderer** is composed of two pieces sharing a stderr lock:

- `EventWriter` (`render/event_writer.rs`) prints right-aligned 12-column verb-prefixed lines as events arrive — `Cooked` / `Tested` / `Compiled` / `Linked` / etc. for completed nodes (verb chosen from `NodeKind`), `Cached` for cache hits, `Skipped` for guard-skipped nodes (with collapsing into `… (N more cached)` past a threshold), `Failed` with an indented stderr block, `Running` for interactive handoffs, and `Finished` / `Failed` for build summaries. `--verbose` streams per-node stdout/stderr inline with `[recipe/node]` prefixes; `--quiet` drops per-node lines.
- `StatusLine` (`render/status_line.rs`) keeps a single sticky line at the bottom of the terminal — verb `Cooking` followed by `[N/M] <currently-running, …>`. It runs a 10 Hz tick thread that reads an `arc_swap`'d `StatusSnapshot` (`render/snapshot.rs`) and redraws via `\r` + clear-to-EOL. The snapshot is rebuilt by the driver from `BuildState` on every event apply. Width is queried per redraw via `terminal_size`. `render_status_line` is a pure function (snapshot + width → string), tested independently of any terminal.

`style.rs` is the verb vocabulary — `LineKind` × `NodeKind` → `Verb { text, color, bold }`, plus a tiny ANSI formatter that honors `NO_COLOR` / non-TTY by emitting plain text.

**PlainRenderer** writes chronological append-only text with `[recipe/node]` prefixes. Safe to pipe, grep, tee, or consume from CI.

**JsonWriter** emits one JSON object per line, transforming `RecipeId`/`NodeId` into human-readable names and `Duration` fields into integer `elapsed_ms` — same shape as what LogStore writes to `events.jsonl`.

### Interactive command takeover

When a node is interactive (gdb, REPL), the inline renderer hides its sticky status line so the child has clean stdio. The append-only event log is unaffected — printed lines stay in scrollback like any other output.

1. On `InteractiveStart`: `EventWriter` prints a `Running <recipe>/<node>` line, then `StatusLine::hide()` flips a flag so the tick thread no-ops (no further redraws). The child process inherits stdio and runs on fresh lines below.
2. On `InteractiveEnd { is_terminal: false }`: refresh the snapshot from `BuildState` and call `show()` — the tick thread resumes redrawing at the bottom.
3. On `InteractiveEnd { is_terminal: true }` or `Finished`: leave the status line hidden. The terminal ends with the event log and the interactive child's output in scrollback, no live tail.

### Log store (`.cook/logs/<build-id>/`)

Written on every build (unless `LogConfig.events_jsonl = false`). Independent of which renderer is active.

```
.cook/logs/<build-id>/
├── events.jsonl       # append-only ProgressEvent stream, same shape as --output=json
├── manifest.toml      # start/end time, build id, exit code, schema version
└── nodes/
    └── <recipe>/
        └── <node>.log # per-node stdout+stderr, [out]/[err] prefixed
```

Rotation is enforced at `LogStore::open` time, before any event is recorded:

- `keep_builds` (default 20) — count-based trim of oldest directories.
- `max_total_bytes` (default 500 MiB) — size-based trim.
- `max_bytes_per_node` (default 2 MiB) — per-file truncation with a `--- truncated ---` marker.

Recipe and node names are sanitized to `[a-zA-Z0-9._-]` when used as path components.

### `cook logs` built-in

Text-only reader of `.cook/logs/`. No TUI.

```
cook logs                          # list recent builds (newest first)
cook logs <recipe>                 # dump every node log for a recipe from most recent build
cook logs <recipe>:<node>          # dump one node's log
cook logs <selector> --build <id>  # pick a specific build
cook logs --failed                 # grep events.jsonl for "node-failed" entries
```

### Not in scope (follow-up)

- Full TUI (`cook recap`, `u` hot-swap). Tracked separately.
- Remote recap / web viewer of `events.jsonl`.
- OTLP / metrics exporter.
