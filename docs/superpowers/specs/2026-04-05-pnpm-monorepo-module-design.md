# pnpm Monorepo Module

## Goal

Ship a `pnpm` cook_module that orchestrates pnpm workspace builds through Cook's native DAG, with correct resolution of `workspace:*`, `catalog:`, and `catalog:name` dependency specifiers — the exact resolution step that turborepo fumbled (vercel/turborepo#10785, #10048).

Demonstrate the module via `examples/monorepo/`: a three-package pnpm workspace exercising both specifier types, with Cook providing build ordering, caching, and parallel execution.

## Motivation

pnpm 9.5 introduced catalogs — centralized dependency version declarations in `pnpm-workspace.yaml`. When a `catalog:name` specifier resolves to a workspace package, turborepo's graph builder failed to recognize it as an internal dependency, producing missing edges in the task DAG. This caused nondeterministic build ordering, broken builds, and failures in `turbo prune`/`turbo-ignore`.

Cook's architecture — explicit inputs/outputs on `add_unit`, content-hashed caching, topological scheduling — is well-suited to solve this correctly. The `pnpm` module reads `pnpm-workspace.yaml` catalogs and `package.json` dependencies, resolves all specifier types to workspace edges, and emits properly-ordered `add_unit` calls that Cook's scheduler executes with full caching.

## Prerequisites (cook-core changes, separate PR)

### Wire up env caching

`RecipeCache.env_hash`, `invalidate_recipe()`, and `hash_env()` exist in cook-cache but the executor never calls them. Env var changes don't currently invalidate caches.

**Fix — three touch points:**

1. `cook-engine/src/executor.rs` — in the recipe registration loop, call `cache_manager.invalidate_recipe(env_hash, ...)` before checking individual unit caches. Compute `env_hash` from the merged env map using the existing `hash_env()` from `cook-cache/src/check.rs`.
2. `cook-cache/src/check.rs` — extend `needs_rebuild_cook()` (or add a wrapper) to compare cached `env_hash` against current. If mismatch → full recipe invalidation (all steps re-run).
3. `cook-cache/src/manager.rs` — ensure `invalidate_recipe()` is reachable from the executor. It's currently `pub` but uncalled.

~30-50 lines of Rust. Env hash changes invalidate the entire recipe's cache (not per-unit), matching the granularity of `invalidate_recipe()`.

### Add `cook.json_decode(str)` and `cook.yaml_decode(str)`

Register two Lua functions backed by `serde_json` and `serde_yaml`. Both accept a string, return a Lua table. Error on invalid input.

Location: `cook-register/src/` alongside existing `cook.*` API registration. ~20 lines each.

## Design

### Module architecture

Single file: `cook_modules/pnpm.lua` (~300 lines). Follows the cpp module pattern: module-level state table, public API functions, internal helpers.

```
pnpm.lua
├── pnpm.init()           — auto-called on `use pnpm`, does all discovery
├── pnpm.install()        — emits add_unit for pnpm install
├── pnpm.run(task)        — core: filter → topo sort → emit add_units
└── internal helpers
    ├── resolve_specifier()   — workspace:* | catalog: | catalog:name → bool
    ├── build_input_list()    — glob-minus-exclusions + anchors + upstream markers
    └── topo_sort()           — DFS post-order on resolved workspace deps
```

### Module state

```lua
local pnpm = {}
pnpm.state = {
    workspace = nil,    -- parsed pnpm-workspace.yaml
    packages = {},      -- name → {dir, json, workspace_deps}
    initialized = false,
}
```

### `pnpm.init()`

Called automatically on `use pnpm`:

1. Read and parse `pnpm-workspace.yaml` via `cook.yaml_decode(fs.read("pnpm-workspace.yaml"))`
2. Glob workspace package dirs from `packages:` patterns (e.g., `packages/*`)
3. For each dir: read + `cook.json_decode(fs.read(dir .. "/package.json"))` 
4. For each package: walk `dependencies` + `devDependencies`, resolve each specifier via `resolve_specifier()`, collect workspace deps
5. Store in `pnpm.state.packages[name] = {dir, json, workspace_deps}`

### `pnpm.install()`

Emits a single `add_unit`:

```lua
cook.add_unit({
    inputs  = { "pnpm-workspace.yaml", "pnpm-lock.yaml", "package.json" },
    output  = ".cook/install.done",
    command = "pnpm install && mkdir -p .cook && touch .cook/install.done",
})
```

### `pnpm.run(task)`

Core function:

1. Filter `pnpm.state.packages` to those whose `json.scripts` contains `task`. Packages without the script are silently skipped.
2. Topological sort filtered packages by `workspace_deps` (DFS post-order).
3. For each package in topo order, emit:

```lua
cook.add_unit({
    inputs  = build_input_list(pkg, task),
    output  = pkg.dir .. "/.cook/" .. task .. ".done",
    command = "pnpm --filter " .. pkg.name .. " run " .. task
           .. " && mkdir -p " .. pkg.dir .. "/.cook"
           .. " && touch " .. pkg.dir .. "/.cook/" .. task .. ".done",
})
```

### Specifier resolution

```lua
local function resolve_specifier(dep_name, spec, workspace)
    -- workspace:* or workspace:^ → internal dep
    if spec:match("^workspace:") then
        return true
    end

    -- catalog: → lookup in default catalog
    if spec == "catalog:" then
        local resolved = workspace.catalog and workspace.catalog[dep_name]
        if resolved and resolved:match("^workspace:") then return true end
        return false
    end

    -- catalog:name → lookup in named catalog
    local cat_name = spec:match("^catalog:(.+)$")
    if cat_name then
        local cat = workspace.catalogs and workspace.catalogs[cat_name]
        if not cat then
            error("unknown catalog '" .. cat_name .. "' referenced by " .. dep_name)
        end
        local resolved = cat[dep_name]
        if resolved and resolved:match("^workspace:") then return true end
        return false
    end

    return false  -- external dep, not a workspace edge
end
```

This is the exact resolution step turborepo missed: `catalog:internal` → look up dep name in `catalogs.internal` → get `workspace:*` → mark as internal workspace dependency.

### Input list construction

Per-package, per-task:

```lua
local excludes = {
    "/node_modules/", "/.cook/", "/dist/", "/build/",
    "/.next/", "/out/", "/coverage/", "/.turbo/",
    "/.parcel-cache/",
}

local function build_input_list(pkg, task)
    local inputs = {}

    -- 1. All files in package dir, minus exclusions
    for _, f in ipairs(fs.glob(pkg.dir .. "/**")) do
        local dominated = false
        for _, ex in ipairs(excludes) do
            if f:find(ex, 1, true) then dominated = true; break end
        end
        if not dominated then inputs[#inputs + 1] = f end
    end

    -- 2. Anchor files
    inputs[#inputs + 1] = "pnpm-lock.yaml"
    inputs[#inputs + 1] = "pnpm-workspace.yaml"
    if fs.exists(".npmrc") then inputs[#inputs + 1] = ".npmrc" end

    -- 3. Upstream markers
    for _, dep_name in ipairs(pkg.workspace_deps) do
        local dep = pnpm.state.packages[dep_name]
        if dep and dep.json.scripts and dep.json.scripts[task] then
            inputs[#inputs + 1] = dep.dir .. "/.cook/" .. task .. ".done"
        end
    end

    return inputs
end
```

### Topological sort

DFS post-order, mirroring Cook's own `analyzer/graph.rs`:

```lua
local function topo_sort(packages, task)
    local order = {}
    local visited, visiting = {}, {}

    local function visit(name)
        if visited[name] then return end
        if visiting[name] then error("cycle detected: " .. name) end
        visiting[name] = true
        local pkg = packages[name]
        if pkg then
            for _, dep in ipairs(pkg.workspace_deps) do
                if packages[dep] and packages[dep].json.scripts
                   and packages[dep].json.scripts[task] then
                    visit(dep)
                end
            end
        end
        visiting[name] = nil
        visited[name] = true
        if pkg and pkg.json.scripts and pkg.json.scripts[task] then
            order[#order + 1] = name
        end
    end

    for name, _ in pairs(packages) do visit(name) end
    return order
end
```

### Error handling

| Condition | Behavior |
|---|---|
| Missing `pnpm-workspace.yaml` | Error at `pnpm.init()` with clear message |
| Missing `package.json` in globbed dir | Skip dir (may be non-package) |
| Invalid JSON in package.json | Error with file path |
| Cycle in workspace deps | Error listing the cycle |
| `catalog:name` referencing nonexistent catalog | Error listing available catalogs |
| Package without `scripts` field | Silently skipped by `pnpm.run()` |
| Package without requested script | Silently skipped (matches turbo behavior) |

## Example project

### Directory layout

```
examples/monorepo/
├── Cookfile
├── README.md
├── .gitignore
├── package.json                  (workspace root)
├── pnpm-workspace.yaml           (catalogs + packages globs)
├── pnpm-lock.yaml
├── tsconfig.base.json
├── cook_modules/
│   └── pnpm.lua
└── packages/
    ├── shared-utils/             (leaf — no workspace deps)
    │   ├── package.json
    │   ├── tsconfig.json
    │   └── src/index.ts
    ├── ui/                       (depends on shared-utils via catalog:internal)
    │   ├── package.json
    │   ├── tsconfig.json
    │   └── src/index.ts
    └── web/                      (depends on ui via workspace:*)
        ├── package.json
        ├── tsconfig.json
        └── src/index.ts
```

### pnpm-workspace.yaml

```yaml
packages:
  - "packages/*"

catalog:
  typescript: ^5.4.0

catalogs:
  internal:
    shared-utils: workspace:*
    ui: workspace:*
```

### Package dependency graph

```
shared-utils (leaf)
    ↑ catalog:internal
    ui
    ↑ workspace:*
    web
```

- `shared-utils`: no workspace deps, `devDependencies: { typescript: "catalog:" }`
- `ui`: `dependencies: { "shared-utils": "catalog:internal" }` — the specifier turborepo mishandled
- `web`: `dependencies: { "ui": "workspace:*" }` — standard workspace protocol

### Cookfile

```
use pnpm

recipe install
    > pnpm.install()
end

recipe build: install
    > pnpm.run("build")
end

recipe test: build
    > pnpm.run("test")
end

recipe clean
    rm -rf packages/*/dist packages/*/.cook .cook
end
```

### What `cook build` produces

The module resolves the full graph:
1. `shared-utils` has no workspace deps → builds first
2. `ui` depends on `shared-utils` via `catalog:internal` → resolved to `workspace:*` → builds second
3. `web` depends on `ui` via `workspace:*` → builds third

Three `add_unit` calls in topo order, cross-package edges expressed via upstream marker files in downstream inputs. Cook's scheduler respects input→output edges and caches via content hashing.

## Testing

### Module correctness (via example)

- **Graph ordering**: `cook build` succeeds — proves catalog resolution wired correct edges (if wrong, tsc fails on missing deps)
- **Cache hit**: second `cook build` skips all packages
- **Cache invalidation (source)**: edit `shared-utils/src/index.ts` → rebuild cascades through ui → web
- **Cache invalidation (env)**: `cook build --set NODE_ENV=production` → full rebuild (env hash changed)

### Cook-core prerequisites

- `cook.json_decode()`: unit test with nested objects, arrays, null, booleans, numbers. Error case: invalid JSON.
- `cook.yaml_decode()`: unit test with pnpm-workspace.yaml shape. Error case: invalid YAML.
- Env cache invalidation: integration test — run → change env via `--set` → re-run → assert re-execution.

## Explicit non-goals

- Module-registered recipes (future cook-core feature)
- `turbo.json` import/compatibility layer
- `turbo prune` / `turbo-ignore` equivalents
- CLI-level `--filter` scoping
- Per-package input/output overrides
- Remote caching
- npm/yarn workspace support
- Per-unit env var declaration (`env_deps`)

## Work breakdown

| # | Deliverable | Type | Dependency |
|---|---|---|---|
| 1 | Wire up env caching | cook-core fix | none |
| 2 | `cook.json_decode()` + `cook.yaml_decode()` | cook-core feature | none |
| 3 | `pnpm.lua` module | cook_module | 1, 2 |
| 4 | `examples/monorepo/` project | example | 3 |
| 5 | Testing + README | docs/test | 4 |

Steps 1-2 merge to `main` first (same or separate PRs). Steps 3-5 ship together on a feature branch.
