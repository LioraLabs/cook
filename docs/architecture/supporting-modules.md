# Supporting Modules: Analyzer, Watcher, Env

Three focused modules that underpin Cook's build pipeline. The analyzer runs before any recipe executes; the watcher powers `cook serve`; and the env loader feeds the five-layer variable resolution system.

---

## 1. Analyzer (`src/analyzer/`)

### Purpose

The analyzer determines which recipes to run and in what order. It is called early in every `cook run` and `cook serve` invocation — before any recipe execution begins — and returns an ordered list of recipe names that the scheduler then enqueues.

Entry point: `src/analyzer/mod.rs:35` `resolve_execution_order(cookfile, target)` → `Vec<String>`.

---

### RecipeInfo Struct

Defined at `src/analyzer/graph.rs:12`:

```rust
pub struct RecipeInfo {
    pub ingredients: Vec<String>,  // glob input patterns from the recipe header
    pub serves: Vec<String>,       // output paths produced by Cook steps
    pub requires: Vec<String>,     // explicit deps from the recipe header
}
```

`build_recipe_info` (`src/analyzer/mod.rs:7`) populates one `RecipeInfo` per recipe by iterating over `cookfile.recipes`. The `serves` field is extracted by filtering each recipe's steps for `Step::Cook` variants and collecting `cook_step.output_pattern` (`src/analyzer/mod.rs:12-22`). For example, a recipe with a `cook "build/{stem}.o"` step will have `"build/{stem}.o"` in its `serves` list.

---

### Dependency Types

There are two independent mechanisms for establishing that recipe B must run before recipe A.

**Explicit dependencies** are declared in the recipe header:

```
recipe "build": "setup"
```

This causes `"setup"` to appear in `recipe.deps` and therefore in `RecipeInfo.requires`. The graph builder validates each explicit dep against the known recipe names and returns `GraphError::UnknownRecipe` if any is missing (`src/analyzer/graph.rs:41-44`).

**Implicit file-based dependencies** are derived automatically. The graph builds a reverse lookup `serves_map: HashMap<&str, &str>` mapping each served path to the name of the recipe that produces it (`src/analyzer/graph.rs:29-34`). Then, for every ingredient of every recipe, it performs an exact string lookup in that map (`src/analyzer/graph.rs:47-53`).

> **Important: implicit matching is exact string equality, not glob matching.**
>
> If recipe `gen` serves `"src/gen.c"` and recipe `build` has ingredient `"src/*.c"`, the ingredient `"src/*.c"` does **not** match the served path `"src/gen.c"`. The lookup key is the literal ingredient string — no pattern expansion is performed. This is confirmed by the dedicated test at `src/analyzer/graph.rs:147-161` (`test_glob_pattern_does_not_trigger_implicit_dep`).
>
> Implicit dependency is only triggered when an ingredient string is identical to a served path string. Glob patterns in ingredients are for the file-watching and runtime globbing layers, not for dependency resolution.

Both dependency types are merged into a single `HashSet<&str>` per recipe, so a recipe that declares `"compile"` in both `requires` and as an implicit match via ingredients produces the same graph edge as one that declares it only once (`src/analyzer/graph.rs:267-281`, `test_duplicate_edges_are_harmless`).

---

### Topological Sort

Implemented at `src/analyzer/graph.rs:20` as a standard DFS with three node states: `Unvisited`, `Visiting`, `Visited`.

Key properties:

- **Only reachable recipes are included.** The DFS starts at `target` and only visits recipes reachable through the dependency graph. Unrelated recipes in the Cookfile are never returned (`src/analyzer/graph.rs:256-265`, `test_only_needed_recipes_included`).
- **Diamond dependencies are handled correctly.** A shared dependency visited via two paths is emitted exactly once because the `Visited` state short-circuits re-entry (`src/analyzer/graph.rs:77-78`).
- **Output order.** A recipe is pushed to `order` only after all its dependencies have been pushed (post-order DFS), so index 0 is always a recipe with no remaining dependencies and the target recipe is always last.

---

### Error Detection

| Error | Condition | Location |
|---|---|---|
| `GraphError::UnknownRecipe(name)` | Target recipe not in map; or an explicit `requires` names a recipe that doesn't exist | `graph.rs:24-26`, `graph.rs:41-43` |
| `GraphError::CycleDetected(name)` | A node is encountered while already in `Visiting` state (direct self-dep or transitive cycle) | `graph.rs:79` |

These errors are mapped to user-facing `CookError` variants in `src/cli/mod.rs:204-209`.

---

## 2. Watcher (`src/watcher/mod.rs`)

### Purpose

The watcher powers `cook serve` — the continuous rebuild mode. It watches ingredient directories and the Cookfile for changes, debounces rapid file system events, and invokes a callback that triggers a rebuild.

---

### CookWatcher Struct

Defined at `src/watcher/mod.rs:6`:

```rust
pub struct CookWatcher {
    pub globs: Vec<String>,       // ingredient glob patterns for all watched recipes
    pub cookfile_path: PathBuf,   // path to the Cookfile being watched
}
```

`globs` is populated by `collect_globs_for_recipes` (`src/watcher/mod.rs:19`), which iterates over the Cookfile's recipes in execution order and collects all ingredient patterns for each recipe in the target set.

---

### Directory Setup

The `watch` method (`src/watcher/mod.rs:44`) uses the `notify` crate (`RecommendedWatcher`). Directory registration works as follows:

1. For each glob pattern in `self.globs`, the parent directory is extracted via `Path::new(pattern).parent()` (`src/watcher/mod.rs:61`).
2. Each unique directory that exists on disk is registered with `RecursiveMode::Recursive` — so a glob like `"src/**/*.c"` causes the entire `src/` tree to be watched.
3. The Cookfile's parent directory is always registered separately with `RecursiveMode::NonRecursive` (`src/watcher/mod.rs:67-69`), so changes to the Cookfile itself are detected without recursing into siblings.

---

### Debounce

The watcher applies a 200ms trailing debounce (`src/watcher/mod.rs:71`). A rebuild is only triggered if at least 200ms has elapsed since the last trigger. Rapid successive file system events (e.g., an editor writing multiple files atomically) collapse into a single rebuild.

---

### Change Classification

When a relevant event arrives, the callback receives a boolean `cookfile_changed` (`src/watcher/mod.rs:81-88`):

- `true` — the changed path matches `self.cookfile_path` exactly. The caller re-parses the Cookfile from scratch.
- `false` — a non-Cookfile path matched one of the ingredient globs. The caller rebuilds using the already-parsed Cookfile.

An event is ignored entirely if no changed path matches either the Cookfile or any ingredient glob.

---

### Interactive Step Rejection

`cook serve` rejects any recipe that contains an `@`-prefixed interactive step. This check is enforced in `src/cli/mod.rs` before the watcher is started, not inside the watcher itself. See `src/cli/mod.rs` `cmd_serve` for the validation logic.

---

## 3. Env (`src/env/mod.rs`)

### Purpose

Loads a `.env` file from the Cookfile's working directory and returns its contents as a flat `HashMap<String, String>`. The result is one input layer into the five-layer variable resolution system assembled in `src/cli/mod.rs`.

---

### Implementation

```rust
pub fn load_env(cookfile_dir: &Path) -> HashMap<String, String>
```

Located at `src/env/mod.rs:4`. It constructs the path `cookfile_dir/.env` and uses `dotenvy::from_path_iter` to parse it. If the file does not exist or cannot be read, the function returns an empty map — it never errors (`src/env/mod.rs:6-9`).

The `dotenvy` crate handles standard `.env` syntax: comments (`# …`), blank lines, single-quoted and double-quoted values (quotes are stripped from the stored value).

---

### Five-Layer Variable Resolution

`load_env` provides only one layer. The full resolution is assembled in `src/cli/mod.rs` `resolve_env` (`src/cli/mod.rs:132`). Each layer overwrites keys from all lower layers:

| Priority | Source | Notes |
|---|---|---|
| 1 (lowest) | `std::env::vars()` — system environment | Shell exports and OS env |
| 2 | Cookfile bare variables (`CC "gcc"`) | `cookfile.vars` |
| 3 | Selected config block (`config "debug" … end`) | Only applied when a config is named on the CLI |
| 4 | `.env` file | Loaded by `load_env`; `src/cli/mod.rs:201` |
| 5 (highest) | `--set KEY=VALUE` CLI flags | `cli.set`; split on the first `=` |

The merged `HashMap<String, String>` is passed directly to the scheduler and runtime. There is no further variable resolution at execution time.
