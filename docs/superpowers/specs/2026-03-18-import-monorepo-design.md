# Import & Monorepo Support Design

## Summary

Cook gains an `import` keyword that lets a Cookfile pull in recipes from other Cookfiles in subdirectories. Imported recipes are namespaced with dot notation (`backend.build`). All recipes merge into a single DAG. Each imported Cookfile retains its own working directory, environment, and cache. This makes Cook a viable monorepo build tool.

## Motivation

Cook is currently single-Cookfile, single-working-directory. To compete with Turborepo/Nx as a monorepo solution, Cook needs to orchestrate builds across subdirectories with multiple languages — without sacrificing its simplicity.

## Design Decisions

### `import` Syntax

```
import <name> <relative-path>
```

- Appears at the top level of a Cookfile, alongside `use` and variable declarations (before recipes)
- `<name>` is the namespace prefix (bare identifier)
- `<path>` is a relative path to a directory containing a Cookfile

**Referencing imported recipes:**

```
import backend ./services/backend
import frontend ./apps/frontend

recipe "all": "backend.build" "frontend.build"
end
```

Dot notation: `<import-name>.<recipe-name>`. Dependencies use the existing quoted-string syntax (`"backend.build"`). Only direct children visible — no reaching through (e.g., root cannot reference `backend.proto.generate`).

> **Note:** There is a separate planned change to allow bare identifiers for recipe names and dependencies (unquoted syntax). When that lands, `backend.build` without quotes will also work. This spec does not depend on that change.

### Transitive Dependencies

If `backend/Cookfile` has `import proto ../libs/proto` and `backend.build` depends on `"proto.generate"`, Cook resolves that internally. Root doesn't see or name `proto` unless it declares its own import.

When root runs `backend.build`, Cook follows the dependency chain through backend's imports automatically.

### Deduplication

When multiple Cookfiles import the same directory (resolved to canonical path), Cook loads the Cookfile once. Recipes from that directory exist once in the DAG. The single Runtime for that directory uses its own Cookfile-declared vars with `--set` overrides applied. Cache lives in that directory's `.cook/`.

If two parents import the same canonical path, they share the same Runtime and the same recipe nodes. There is no per-parent customization of an imported Cookfile's environment — `--set` is global, and the imported Cookfile's own vars are its defaults. This keeps deduplication simple and unambiguous.

### Environment Isolation

Each imported Cookfile gets its own execution context:

- **Cookfile variables/configs** — scoped to that Cookfile. A subdir's variable declarations are its own; the parent's Cookfile-level variables do not leak in.
- **System environment variables** — inherited naturally by all child processes (`$PATH`, `$HOME`, `$GOROOT`, etc.). This is standard process inheritance, not Cook-specific.
- **Working directory** — the directory containing that Cookfile.
- **Cache** — `.cook/` within that working directory.

**`--set` propagation:** CLI `--set KEY=VALUE` overrides flow into all imports. A Cookfile's own variable declarations are defaults; `--set` wins. This is the coordination lever without breaking isolation.

### CLI Behavior

Each Cookfile is independently valid:

```bash
# From root — run specific imported recipe
cook run backend.build

# From root — run root recipe that pulls everything
cook run all

# From within a subdirectory — just that Cookfile's world
cd services/backend
cook run build
```

Running from a subdir only sees that Cookfile's scope (including its own imports). No workspace-level awareness needed.

### Recipe Discovery (`cook menu`)

`cook menu` shows all available recipes including imported ones. Imported recipes display with their namespace prefix:

```
Available recipes:
  build
  dev
  test
  clean
  backend.build
  backend.dev
  backend.test
  frontend.build
  frontend.dev
  frontend.serve
```

Nested imports (e.g., `backend.proto.generate`) are not shown from root since they are not directly referenceable. Running `cook menu` from within a subdir shows only that Cookfile's recipes.

## Architecture: Engine-First (Approach 2)

### Parser Changes

New AST node:

```rust
pub struct ImportDecl {
    pub name: String,      // namespace prefix
    pub path: String,      // relative path to directory
    pub line: usize,       // for error reporting
}
```

Added to `Cookfile`:

```rust
pub struct Cookfile {
    pub vars: Vec<(String, String)>,
    pub configs: BTreeMap<String, Vec<(String, String)>>,
    pub recipes: Vec<Recipe>,
    pub uses: Vec<UseStatement>,
    pub imports: Vec<ImportDecl>,  // new
}
```

**Parser validation:**
- Duplicate import names → error
- `import` appearing after recipes → error

Path resolution and Cookfile loading happen in the engine, not the parser.

### Engine Changes

**Loading phase:**

1. Engine reads root Cookfile, encounters `import` declarations
2. For each import, resolve path to canonical form and load that Cookfile
3. Each imported Cookfile gets its own `codegen::generate` pass to produce its Lua source
4. If an imported Cookfile has its own imports, recurse
5. Cycle detection via canonical path set — error: "Circular import detected: A → B → A"
6. Dedup: if a canonical path has already been loaded, reuse the existing Runtime and recipes

**Runtime model:**

Each loaded Cookfile gets its own Runtime:
- `working_dir` — the directory containing that Cookfile
- `vars` / `configs` — from that Cookfile's declarations, with `--set` overrides applied
- `cache_dir` — `.cook/` within that working directory

The root Runtime holds references to child Runtimes, keyed by import name.

**DAG construction:**

All recipes from all Cookfiles merge into one DAG. Recipe nodes are namespaced:
- Root recipes: `build`, `test`, `all`
- Imported recipes: `backend.build`, `frontend.serve`
- Nested imports: `backend.proto.generate` (internal name, not visible to root)

Each node carries a reference to its Runtime for execution context.

**Dependency resolution:**

When the engine encounters a dependency like `"proto.generate"`:
1. Check if `proto` is a declared import in the current Cookfile
2. If yes, look up `generate` in that import's recipes
3. If not found, error: "Recipe 'generate' not found in import 'proto' (imported from ./libs/proto)"
4. If user tries to reach through (e.g., `"backend.proto.generate"` from root), error: "Cannot reference nested import 'proto' through 'backend'. Only direct imports are visible. Add `import proto <path>` to this Cookfile to use proto recipes directly."

**Analyzer changes:**

The current `analyzer::resolve_execution_order` performs topological sort on a single Cookfile's recipes. With imports, the engine builds the full merged DAG (all namespaced recipes across all Cookfiles), then runs topological sort on the unified graph. The analyzer's graph-building logic extends to handle namespaced recipe names — a dependency `"backend.build"` resolves to the `build` node from backend's recipe set.

**Sequential registration constraint:**

The current pipeline registers and executes recipes one at a time because ingredient globs must resolve against outputs from earlier recipes. With the merged DAG, this constraint is preserved by the topological ordering — a recipe's dependencies are fully executed before it runs, so their outputs exist when ingredients are globbed. The DAG's topological sort replaces the sequential loop's implicit ordering with an explicit one.

### Scheduler & Execution

**Minimal scheduler changes.** The scheduler already executes a DAG in topological order with parallel execution. The changes needed:

- **`WorkItem`** — each work item carries its Runtime reference (working_dir, env_vars, cache_dir) instead of using a pool-wide global
- **`WorkerPool::spawn`** — currently takes a single `working_dir` and `env_vars`. Must be refactored so workers pull working_dir and env from each work item rather than shared pool state
- **`execute_shell` / `run_shell_in_worker`** — use per-item working_dir instead of pool-level
- **`execute_dag`** — `record_completion` and `run_interactive_on_main` calls use per-item working_dir
- **Cache manager** — receives per-item `cache_dir` from the Runtime reference

The DAG traversal, topological ordering, and parallel dispatch logic are unchanged.

**Parallel execution across imports:** Falls out naturally. Independent nodes in the DAG run in parallel regardless of which Runtime they belong to.

### Codegen

Each imported Cookfile gets its own `codegen::generate` pass. The engine calls `codegen::generate(cookfile)` independently for each loaded Cookfile, producing separate Lua sources. Each Runtime's Lua VM loads only its own generated source. This is consistent with the isolation model — no cross-contamination of Lua state between Cookfiles.

### Watcher

If Cook's file watcher is active (e.g., `cook serve`), it must watch all imported directories. Changes in an imported Cookfile's directory trigger rebuilds for recipes that depend on that import. The watcher registers watch paths for each loaded Runtime's `working_dir`, not just the root.

## Error Messages

| Failure | Message |
|---------|---------|
| Import path does not exist | `Import 'backend': directory './services/backend' not found` |
| No Cookfile in import path | `Import 'backend': no Cookfile found in './services/backend'` |
| Duplicate import name | `Duplicate import name 'backend' (first declared on line N)` |
| Circular import | `Circular import detected: ./root → ./services/backend → ./libs/proto → ./root` |
| Recipe not found in import | `Recipe 'generate' not found in import 'proto' (imported from ./libs/proto)` |
| Reaching through nested import | `Cannot reference nested import 'proto' through 'backend'. Only direct imports are visible.` |

## Example Monorepo

### Phase 1: Minimal but functional

```
examples/monorepo/
  Cookfile                    # root — imports backend and frontend (not proto directly)
  libs/
    proto/
      Cookfile                # generates types from .proto or OpenAPI spec
      api.proto               # one schema: Task message + CRUD service
  services/
    backend/
      Cookfile                # Go service — imports proto, one endpoint
      cmd/server/main.go
      go.mod
  apps/
    frontend/
      Cookfile                # TypeScript app — imports proto, one page
      package.json
      src/index.ts
```

**Root Cookfile** (demonstrates that root does NOT need to import proto — transitive deps handled automatically):

```
import backend ./services/backend
import frontend ./apps/frontend

recipe "build": "backend.build" "frontend.build"
end

recipe "dev": "backend.dev" "frontend.dev"
end

recipe "test": "backend.test" "frontend.test"
end

recipe "clean": "backend.clean" "frontend.clean"
end
```

**Backend Cookfile** (imports proto directly — transitive dep):

```
import proto ../../libs/proto

recipe "build": "proto.generate"
    cook "server" using "go build ./cmd/server"
end
```

Each sub-Cookfile is independently runnable. The root wires them together. Proto is a transitive dependency — root doesn't need to know about it.

### Phase 2: Realistic starter (follow-up)

Extend Phase 1 with Docker builds, dev servers, generated type bindings, and more realistic project scaffolding. No structural changes needed — just more recipes and code in the existing layout.

## Existing Examples Cleanup

The current `examples/` directory contains:
- `examples/Cookfile` — C math library (vec3, matrix)
- `examples/cpp-project/` — C++ project with Lua modules

These should be cleaned up and reorganized alongside the new monorepo example. Structure TBD during implementation planning.
