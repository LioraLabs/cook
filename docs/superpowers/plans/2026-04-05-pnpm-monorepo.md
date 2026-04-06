# pnpm Monorepo Module Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship a `pnpm` cook_module + `examples/monorepo/` that orchestrates pnpm workspace builds through Cook's native DAG with correct `catalog:` specifier resolution.

**Architecture:** Two-phase delivery. Phase 1: cook-core prerequisites (env cache wiring + `cook.json_decode` + `cook.yaml_decode`). Phase 2: the `pnpm.lua` module and example project. The module reads `pnpm-workspace.yaml` catalogs + `package.json` deps, resolves `workspace:*`/`catalog:`/`catalog:name` to workspace edges, topo-sorts packages, and emits one `cook.add_unit()` per (package, task) tuple with marker-file outputs and globbed inputs.

**Tech Stack:** Rust (cook-core: serde_json, serde_yaml, mlua), Lua (pnpm.lua module), TypeScript (example packages), pnpm workspaces.

**Spec:** `docs/superpowers/specs/2026-04-05-pnpm-monorepo-module-design.md`

---

## File Structure

### Phase 1: cook-core changes

| Action | File | Responsibility |
|--------|------|----------------|
| Create | `cli/crates/cook-register/src/codec_api.rs` | `cook.json_decode(str)` and `cook.yaml_decode(str)` Lua functions |
| Modify | `cli/crates/cook-register/src/lib.rs:16` | Add `pub mod codec_api;` |
| Modify | `cli/crates/cook-register/src/engine.rs:73` | Call `codec_api::register_codec_api(&lua)?;` |
| Modify | `cli/crates/cook-register/Cargo.toml:13` | Add `serde_yaml = "0.9"` dependency |
| Modify | `cli/crates/cook-cache/src/manager.rs:82-183` | Add `invalidate_if_env_changed()` to `ThreadSafeCacheManager` |
| Modify | `cli/crates/cook-engine/src/run.rs:93-114` | Call env invalidation after recipe registration |

### Phase 2: pnpm module + example

| Action | File | Responsibility |
|--------|------|----------------|
| Create | `examples/monorepo/cook_modules/pnpm.lua` | The module: init, install, run, helpers |
| Create | `examples/monorepo/Cookfile` | Thin recipes delegating to pnpm.run |
| Create | `examples/monorepo/package.json` | Workspace root |
| Create | `examples/monorepo/pnpm-workspace.yaml` | Package globs + catalogs |
| Create | `examples/monorepo/tsconfig.base.json` | Shared TS config |
| Create | `examples/monorepo/.gitignore` | node_modules, dist, .cook |
| Create | `examples/monorepo/README.md` | Setup instructions + catalog explanation |
| Create | `examples/monorepo/packages/shared-utils/package.json` | Leaf package |
| Create | `examples/monorepo/packages/shared-utils/tsconfig.json` | TS config extending base |
| Create | `examples/monorepo/packages/shared-utils/src/index.ts` | Utility functions |
| Create | `examples/monorepo/packages/ui/package.json` | Depends on shared-utils via `catalog:internal` |
| Create | `examples/monorepo/packages/ui/tsconfig.json` | TS config |
| Create | `examples/monorepo/packages/ui/src/index.ts` | Re-exports from shared-utils |
| Create | `examples/monorepo/packages/web/package.json` | Depends on ui via `workspace:*` |
| Create | `examples/monorepo/packages/web/tsconfig.json` | TS config |
| Create | `examples/monorepo/packages/web/src/index.ts` | Imports from ui, main entry |

---

## Chunk 1: cook-core prerequisites

### Task 1: Add `cook.json_decode()` and `cook.yaml_decode()`

**Files:**
- Create: `cli/crates/cook-register/src/codec_api.rs`
- Modify: `cli/crates/cook-register/src/lib.rs`
- Modify: `cli/crates/cook-register/src/engine.rs`
- Modify: `cli/crates/cook-register/Cargo.toml`

- [ ] **Step 1: Add serde_yaml dependency**

In `cli/crates/cook-register/Cargo.toml`, add after the `serde_json = "1"` line (line 13):

```toml
serde_yaml = "0.9"
```

- [ ] **Step 2: Create `codec_api.rs` with tests**

Create `cli/crates/cook-register/src/codec_api.rs`:

```rust
use mlua::prelude::*;

use crate::module_loader::json_to_lua_value;

/// Register `cook.json_decode(str)` and `cook.yaml_decode(str)`.
pub fn register_codec_api(lua: &Lua) -> LuaResult<()> {
    let cook: LuaTable = lua.globals().get("cook")?;

    // cook.json_decode(json_string) -> lua table
    let json_decode = lua.create_function(|lua, s: String| {
        let val: serde_json::Value =
            serde_json::from_str(&s).map_err(|e| LuaError::runtime(format!("json error: {e}")))?;
        json_to_lua_value(lua, val)
    })?;
    cook.set("json_decode", json_decode)?;

    // cook.yaml_decode(yaml_string) -> lua table
    // Parse YAML into serde_json::Value (serde_yaml supports this) to reuse json_to_lua_value.
    let yaml_decode = lua.create_function(|lua, s: String| {
        let val: serde_json::Value = serde_yaml::from_str(&s)
            .map_err(|e| LuaError::runtime(format!("yaml error: {e}")))?;
        json_to_lua_value(lua, val)
    })?;
    cook.set("yaml_decode", yaml_decode)?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_lua() -> Lua {
        let lua = Lua::new();
        lua.globals()
            .set("cook", lua.create_table().unwrap())
            .unwrap();
        register_codec_api(&lua).unwrap();
        lua
    }

    #[test]
    fn test_json_decode_object() {
        let lua = make_lua();
        lua.load(r#"
            local t = cook.json_decode('{"name":"foo","version":1,"active":true,"items":[1,2,3]}')
            assert(t.name == "foo")
            assert(t.version == 1)
            assert(t.active == true)
            assert(t.items[1] == 1)
            assert(t.items[2] == 2)
            assert(t.items[3] == 3)
        "#)
        .exec()
        .unwrap();
    }

    #[test]
    fn test_json_decode_null() {
        let lua = make_lua();
        lua.load(r#"
            local t = cook.json_decode('{"a":null}')
            assert(t.a == nil)
        "#)
        .exec()
        .unwrap();
    }

    #[test]
    fn test_json_decode_nested() {
        let lua = make_lua();
        lua.load(r#"
            local t = cook.json_decode('{"scripts":{"build":"tsc","test":"jest"}}')
            assert(t.scripts.build == "tsc")
            assert(t.scripts.test == "jest")
        "#)
        .exec()
        .unwrap();
    }

    #[test]
    fn test_json_decode_error() {
        let lua = make_lua();
        let result = lua.load(r#"cook.json_decode("not json")"#).exec();
        assert!(result.is_err());
    }

    #[test]
    fn test_yaml_decode_workspace() {
        let lua = make_lua();
        // Test the exact pnpm-workspace.yaml shape
        lua.load(r#"
            local t = cook.yaml_decode([[
packages:
  - "packages/*"
catalog:
  typescript: "^5.4.0"
catalogs:
  internal:
    shared-utils: "workspace:*"
    ui: "workspace:*"
]])
            assert(t.packages[1] == "packages/*")
            assert(t.catalog.typescript == "^5.4.0")
            assert(t.catalogs.internal["shared-utils"] == "workspace:*")
            assert(t.catalogs.internal.ui == "workspace:*")
        "#)
        .exec()
        .unwrap();
    }

    #[test]
    fn test_yaml_decode_error() {
        let lua = make_lua();
        let result = lua
            .load(r#"cook.yaml_decode(":\n  :\n    - :")"#)
            .exec();
        assert!(result.is_err());
    }
}
```

- [ ] **Step 3: Wire codec_api into the module system**

In `cli/crates/cook-register/src/lib.rs`, add after line 16 (`pub mod unit_api;`):

```rust
pub mod codec_api;
```

In `cli/crates/cook-register/src/engine.rs`, add after line 73 (`crate::context::register_resolve_ingredients(&lua, &self.working_dir)?;`):

```rust
        crate::codec_api::register_codec_api(&lua)?;
```

- [ ] **Step 4: Run tests**

Run: `cd /home/alex/dev/cook && cargo test -p cook-register codec_api`

Expected: All 6 tests pass.

- [ ] **Step 5: Commit**

```bash
git add cli/crates/cook-register/src/codec_api.rs \
       cli/crates/cook-register/src/lib.rs \
       cli/crates/cook-register/src/engine.rs \
       cli/crates/cook-register/Cargo.toml
git commit -m "feat(core): add cook.json_decode and cook.yaml_decode"
```

---

### Task 2: Wire up env cache invalidation

**Files:**
- Modify: `cli/crates/cook-cache/src/manager.rs:82-183`
- Modify: `cli/crates/cook-engine/src/run.rs:93-114`

- [ ] **Step 1: Write test for `ThreadSafeCacheManager::invalidate_if_env_changed`**

In `cli/crates/cook-cache/src/manager.rs`, add at the end of the existing `#[cfg(test)] mod tests` block (before the closing `}`):

```rust
    #[test]
    fn test_invalidate_if_env_changed_clears_steps() {
        let dir = tempfile::tempdir().unwrap();
        let cm = ThreadSafeCacheManager::new(dir.path().to_path_buf());

        // Populate cache with a step
        cm.update_step(
            "build",
            "main.o",
            make_step_entry(0x1234),
        );

        // Same env hash — steps should survive
        cm.invalidate_if_env_changed("build", 100);
        let cache = cm.get_or_load("build");
        assert!(cache.steps.contains_key("main.o"), "step should survive same env hash");

        // Different env hash — steps should be cleared
        cm.invalidate_if_env_changed("build", 999);
        let cache = cm.get_or_load("build");
        assert!(cache.steps.is_empty(), "steps should be cleared on env hash change");
        assert_eq!(cache.env_hash, 999);
    }

    #[test]
    fn test_invalidate_if_env_changed_no_op_on_match() {
        let dir = tempfile::tempdir().unwrap();
        let cm = ThreadSafeCacheManager::new(dir.path().to_path_buf());

        cm.update_step("build", "main.o", make_step_entry(0x1234));

        // First call sets env_hash to 42
        cm.invalidate_if_env_changed("build", 42);
        let cache = cm.get_or_load("build");
        assert_eq!(cache.steps.len(), 1, "steps should survive when env hash matches");

        // Same hash again — no-op
        cm.invalidate_if_env_changed("build", 42);
        let cache = cm.get_or_load("build");
        assert_eq!(cache.steps.len(), 1);
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cd /home/alex/dev/cook && cargo test -p cook-cache invalidate_if_env`

Expected: Compilation error — `invalidate_if_env_changed` does not exist on `ThreadSafeCacheManager`.

- [ ] **Step 3: Implement `invalidate_if_env_changed` on `ThreadSafeCacheManager`**

In `cli/crates/cook-cache/src/manager.rs`, add this method inside the `impl ThreadSafeCacheManager` block, after `get_or_load` (after line 141):

```rust
    /// Check if the environment has changed since the last build. If so,
    /// clear all cached steps for this recipe (forcing a full rebuild).
    pub fn invalidate_if_env_changed(&self, recipe_name: &str, env_hash: u64) {
        let mut caches = self.caches.lock().unwrap();
        let cache = caches
            .entry(recipe_name.to_string())
            .or_insert_with(|| RecipeCache::load(&self.cache_dir, recipe_name).unwrap_or_default());

        if cache.env_hash != env_hash {
            cache.steps.clear();
            cache.env_hash = env_hash;
            drop(caches);
            let mut dirty = self.dirty.lock().unwrap();
            dirty.insert(recipe_name.to_string());
        }
    }
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cd /home/alex/dev/cook && cargo test -p cook-cache invalidate_if_env`

Expected: Both tests pass.

- [ ] **Step 5: Wire env invalidation into the run loop**

In `cli/crates/cook-engine/src/run.rs`, add this import at line 2 (after `use std::collections::BTreeMap;`):

```rust
use cook_cache::hash_env;
```

Then inside the `for name in &ready` loop, after the cache manager is created (after line 112: `.or_insert_with(|| Arc::new(ThreadSafeCacheManager::new(cache_dir)));`), add:

```rust
            // Invalidate cache if environment changed since last build.
            let env_hash = hash_env(&units.env_vars);
            wave_cache_managers
                .get(name)
                .unwrap()
                .invalidate_if_env_changed(name, env_hash);
```

- [ ] **Step 6: Run full test suite**

Run: `cd /home/alex/dev/cook && cargo test`

Expected: All tests pass including existing cache and executor tests.

- [ ] **Step 7: Commit**

```bash
git add cli/crates/cook-cache/src/manager.rs \
       cli/crates/cook-engine/src/run.rs
git commit -m "feat(cache): wire up env hash invalidation

Environment variable changes now invalidate the recipe cache,
forcing a full rebuild. Uses the existing hash_env() and env_hash
infrastructure that was previously defined but not connected."
```

---

## Chunk 2: pnpm module + example project

### Task 3: Scaffold example project files

**Files:**
- Create: `examples/monorepo/package.json`
- Create: `examples/monorepo/pnpm-workspace.yaml`
- Create: `examples/monorepo/tsconfig.base.json`
- Create: `examples/monorepo/.gitignore`
- Create: `examples/monorepo/packages/shared-utils/package.json`
- Create: `examples/monorepo/packages/shared-utils/tsconfig.json`
- Create: `examples/monorepo/packages/shared-utils/src/index.ts`
- Create: `examples/monorepo/packages/ui/package.json`
- Create: `examples/monorepo/packages/ui/tsconfig.json`
- Create: `examples/monorepo/packages/ui/src/index.ts`
- Create: `examples/monorepo/packages/web/package.json`
- Create: `examples/monorepo/packages/web/tsconfig.json`
- Create: `examples/monorepo/packages/web/src/index.ts`

- [ ] **Step 1: Create root project files**

`examples/monorepo/package.json`:
```json
{
  "private": true,
  "packageManager": "pnpm@9.15.0"
}
```

`examples/monorepo/pnpm-workspace.yaml`:
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

`examples/monorepo/tsconfig.base.json`:
```json
{
  "compilerOptions": {
    "target": "ES2020",
    "module": "commonjs",
    "declaration": true,
    "strict": true,
    "esModuleInterop": true,
    "outDir": "dist",
    "rootDir": "src"
  }
}
```

`examples/monorepo/.gitignore`:
```
node_modules/
dist/
.cook/
pnpm-lock.yaml
```

- [ ] **Step 2: Create shared-utils package**

`examples/monorepo/packages/shared-utils/package.json`:
```json
{
  "name": "shared-utils",
  "version": "1.0.0",
  "main": "dist/index.js",
  "scripts": {
    "build": "tsc",
    "test": "node -e \"require('./dist/index.js'); console.log('shared-utils: ok')\""
  },
  "devDependencies": {
    "typescript": "catalog:"
  }
}
```

`examples/monorepo/packages/shared-utils/tsconfig.json`:
```json
{
  "extends": "../../tsconfig.base.json",
  "compilerOptions": {
    "outDir": "dist",
    "rootDir": "src"
  },
  "include": ["src"]
}
```

`examples/monorepo/packages/shared-utils/src/index.ts`:
```typescript
export function capitalize(s: string): string {
  return s.charAt(0).toUpperCase() + s.slice(1);
}

export function slugify(s: string): string {
  return s.toLowerCase().replace(/\s+/g, "-");
}
```

- [ ] **Step 3: Create ui package (depends on shared-utils via catalog:internal)**

`examples/monorepo/packages/ui/package.json`:
```json
{
  "name": "ui",
  "version": "1.0.0",
  "main": "dist/index.js",
  "scripts": {
    "build": "tsc",
    "test": "node -e \"require('./dist/index.js'); console.log('ui: ok')\""
  },
  "dependencies": {
    "shared-utils": "catalog:internal"
  },
  "devDependencies": {
    "typescript": "catalog:"
  }
}
```

`examples/monorepo/packages/ui/tsconfig.json`:
```json
{
  "extends": "../../tsconfig.base.json",
  "compilerOptions": {
    "outDir": "dist",
    "rootDir": "src"
  },
  "include": ["src"]
}
```

`examples/monorepo/packages/ui/src/index.ts`:
```typescript
import { capitalize } from "shared-utils";

export function formatLabel(text: string): string {
  return `[${capitalize(text)}]`;
}
```

- [ ] **Step 4: Create web package (depends on ui via workspace:\*)**

`examples/monorepo/packages/web/package.json`:
```json
{
  "name": "web",
  "version": "1.0.0",
  "scripts": {
    "build": "tsc",
    "test": "node -e \"require('./dist/index.js'); console.log('web: ok')\""
  },
  "dependencies": {
    "ui": "workspace:*"
  },
  "devDependencies": {
    "typescript": "catalog:"
  }
}
```

`examples/monorepo/packages/web/tsconfig.json`:
```json
{
  "extends": "../../tsconfig.base.json",
  "compilerOptions": {
    "outDir": "dist",
    "rootDir": "src"
  },
  "include": ["src"]
}
```

`examples/monorepo/packages/web/src/index.ts`:
```typescript
import { formatLabel } from "ui";

const greeting = formatLabel("hello from web");
console.log(greeting);
```

- [ ] **Step 5: Commit scaffold**

```bash
git add examples/monorepo/
git commit -m "scaffold: add monorepo example project structure

Three-package pnpm workspace: shared-utils (leaf), ui (catalog:internal dep),
web (workspace:* dep). Minimal TypeScript to demonstrate build ordering."
```

---

### Task 4: Write `pnpm.lua` module — init and workspace discovery

**Files:**
- Create: `examples/monorepo/cook_modules/pnpm.lua`

- [ ] **Step 1: Create module with init + workspace discovery**

Create `examples/monorepo/cook_modules/pnpm.lua`:

```lua
-- pnpm cook_module: orchestrate pnpm workspace builds through Cook's DAG
-- with correct catalog: specifier resolution.

local pnpm = {}

pnpm.state = {
    workspace = nil,    -- parsed pnpm-workspace.yaml
    packages = {},      -- name -> {name, dir, json, workspace_deps}
    initialized = false,
}

-- ---------------------------------------------------------------------------
-- Specifier resolution
-- ---------------------------------------------------------------------------

--- Check if a dependency specifier refers to a workspace package.
--- Resolves workspace:*, catalog:, and catalog:name through the catalog chain.
local function is_workspace_specifier(dep_name, spec, workspace)
    -- workspace:* or workspace:^ -> internal dep
    if spec:match("^workspace:") then
        return true
    end

    -- catalog: (default catalog)
    if spec == "catalog:" then
        local resolved = workspace.catalog and workspace.catalog[dep_name]
        if resolved and type(resolved) == "string" and resolved:match("^workspace:") then
            return true
        end
        return false
    end

    -- catalog:name (named catalog)
    local cat_name = spec:match("^catalog:(.+)$")
    if cat_name then
        local cat = workspace.catalogs and workspace.catalogs[cat_name]
        if not cat then
            error("pnpm: unknown catalog '" .. cat_name .. "' referenced by dependency '" .. dep_name .. "'")
        end
        local resolved = cat[dep_name]
        if resolved and type(resolved) == "string" and resolved:match("^workspace:") then
            return true
        end
        return false
    end

    return false
end

--- Collect workspace dependency names from a package.json's
--- dependencies and devDependencies.
local function collect_workspace_deps(pkg_json, workspace, all_pkg_names)
    local deps = {}
    local dep_sections = { pkg_json.dependencies, pkg_json.devDependencies }
    for _, section in ipairs(dep_sections) do
        if section then
            for dep_name, spec in pairs(section) do
                if all_pkg_names[dep_name] and is_workspace_specifier(dep_name, spec, workspace) then
                    deps[#deps + 1] = dep_name
                end
            end
        end
    end
    table.sort(deps) -- deterministic order
    return deps
end

-- ---------------------------------------------------------------------------
-- Topological sort
-- ---------------------------------------------------------------------------

local function topo_sort(packages, task)
    local order = {}
    local visited = {}
    local visiting = {}

    local function visit(name)
        if visited[name] then return end
        if visiting[name] then
            error("pnpm: dependency cycle detected involving '" .. name .. "'")
        end
        visiting[name] = true
        local pkg = packages[name]
        if pkg then
            for _, dep_name in ipairs(pkg.workspace_deps) do
                local dep = packages[dep_name]
                if dep and dep.json.scripts and dep.json.scripts[task] then
                    visit(dep_name)
                end
            end
        end
        visiting[name] = nil
        visited[name] = true
        if pkg and pkg.json.scripts and pkg.json.scripts[task] then
            order[#order + 1] = name
        end
    end

    for name, _ in pairs(packages) do
        visit(name)
    end
    return order
end

-- ---------------------------------------------------------------------------
-- Input list construction
-- ---------------------------------------------------------------------------

local EXCLUDE_PATTERNS = {
    "/node_modules/", "/.cook/", "/dist/", "/build/",
    "/.next/", "/out/", "/coverage/", "/.turbo/",
    "/.parcel-cache/",
}

local function build_input_list(pkg, task, packages)
    local inputs = {}

    -- 1. All files in package dir, minus exclusions
    local all_files = fs.glob(pkg.dir .. "/**")
    for _, f in ipairs(all_files) do
        local excluded = false
        for _, pattern in ipairs(EXCLUDE_PATTERNS) do
            if f:find(pattern, 1, true) then
                excluded = true
                break
            end
        end
        if not excluded then
            inputs[#inputs + 1] = f
        end
    end

    -- 2. Anchor files (always included)
    inputs[#inputs + 1] = "pnpm-lock.yaml"
    inputs[#inputs + 1] = "pnpm-workspace.yaml"
    if fs.exists(".npmrc") then
        inputs[#inputs + 1] = ".npmrc"
    end

    -- 3. Upstream markers (for workspace deps that have this task)
    for _, dep_name in ipairs(pkg.workspace_deps) do
        local dep = packages[dep_name]
        if dep and dep.json.scripts and dep.json.scripts[task] then
            inputs[#inputs + 1] = dep.dir .. "/.cook/" .. task .. ".done"
        end
    end

    return inputs
end

-- ---------------------------------------------------------------------------
-- Public API
-- ---------------------------------------------------------------------------

function pnpm.init()
    if pnpm.state.initialized then return end

    -- 1. Read and parse pnpm-workspace.yaml
    local ws_path = "pnpm-workspace.yaml"
    if not fs.exists(ws_path) then
        error("pnpm: pnpm-workspace.yaml not found in working directory")
    end
    local ws_str = fs.read(ws_path)
    pnpm.state.workspace = cook.yaml_decode(ws_str)

    -- 2. Discover workspace packages via glob patterns
    local ws = pnpm.state.workspace
    local pkg_patterns = ws.packages or {}
    local pkg_dirs = {}
    for _, pattern in ipairs(pkg_patterns) do
        local dirs = fs.glob(pattern)
        for _, d in ipairs(dirs) do
            -- Only include directories that have a package.json
            if fs.exists(d .. "/package.json") then
                pkg_dirs[#pkg_dirs + 1] = d
            end
        end
    end

    -- 3. Parse each package.json
    local all_pkg_names = {} -- name -> true, for fast lookup
    local raw_packages = {}  -- ordered list for processing
    for _, dir in ipairs(pkg_dirs) do
        local json_str = fs.read(dir .. "/package.json")
        local pkg_json = cook.json_decode(json_str)
        if pkg_json.name then
            all_pkg_names[pkg_json.name] = true
            raw_packages[#raw_packages + 1] = { name = pkg_json.name, dir = dir, json = pkg_json }
        end
    end

    -- 4. Resolve workspace deps for each package
    for _, pkg in ipairs(raw_packages) do
        pkg.workspace_deps = collect_workspace_deps(pkg.json, ws, all_pkg_names)
        pnpm.state.packages[pkg.name] = pkg
    end

    pnpm.state.initialized = true
end

function pnpm.install()
    cook.add_unit({
        inputs = { "pnpm-workspace.yaml", "pnpm-lock.yaml", "package.json" },
        output = ".cook/install.done",
        command = "pnpm install && mkdir -p .cook && touch .cook/install.done",
    })
end

function pnpm.run(task)
    -- Ensure init has run
    pnpm.init()

    local packages = pnpm.state.packages
    local order = topo_sort(packages, task)

    for _, name in ipairs(order) do
        local pkg = packages[name]
        local inputs = build_input_list(pkg, task, packages)
        local marker_dir = pkg.dir .. "/.cook"
        local marker = marker_dir .. "/" .. task .. ".done"

        cook.add_unit({
            inputs = inputs,
            output = marker,
            command = "pnpm --filter " .. pkg.name .. " run " .. task
                .. " && mkdir -p " .. marker_dir
                .. " && touch " .. marker,
        })
    end
end

return pnpm
```

- [ ] **Step 2: Commit module**

```bash
git add examples/monorepo/cook_modules/pnpm.lua
git commit -m "feat(pnpm): add pnpm cook_module

Orchestrates pnpm workspace builds through Cook's native DAG.
Resolves workspace:*, catalog:, and catalog:name specifiers to
workspace edges. Topo-sorts packages and emits one add_unit per
(package, task) tuple with marker-file outputs and globbed inputs."
```

---

### Task 5: Write Cookfile and README

**Files:**
- Create: `examples/monorepo/Cookfile`
- Create: `examples/monorepo/README.md`

- [ ] **Step 1: Write the Cookfile**

Create `examples/monorepo/Cookfile`:

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

- [ ] **Step 2: Write the README**

Create `examples/monorepo/README.md`:

```markdown
# Monorepo Example — pnpm cook_module

Demonstrates Cook orchestrating a pnpm workspace with correct
`catalog:` specifier resolution.

## Prerequisites

- Node.js (>= 18)
- pnpm (>= 9.5, for catalog support)
- Cook

## Setup

```bash
cd examples/monorepo
pnpm install
```

## Usage

```bash
cook build    # builds all packages in dependency order
cook test     # runs tests (depends on build)
cook clean    # removes dist/ and .cook/ artifacts
```

## What this demonstrates

This workspace has three packages with two types of internal dependency
specifiers:

- **shared-utils**: leaf package, no workspace dependencies
- **ui**: depends on `shared-utils` via `catalog:internal` — a pnpm catalog
  specifier that turborepo failed to resolve (vercel/turborepo#10785)
- **web**: depends on `ui` via `workspace:*` — standard pnpm workspace protocol

The `pnpm` cook_module reads `pnpm-workspace.yaml` (including `catalog:` and
`catalogs:` entries), resolves all specifiers to workspace edges, topologically
sorts packages, and emits one `cook.add_unit()` per (package, task) tuple.

Cook's native caching means subsequent runs skip packages whose inputs
haven't changed.

## The catalog resolution bug

pnpm 9.5 introduced catalogs — centralized dependency versions in
`pnpm-workspace.yaml`. When a `catalog:name` specifier resolved to a workspace
package, turborepo's graph builder didn't recognize it as an internal dependency,
causing missing edges in the task DAG. This module resolves the full
`catalog:name` → catalog lookup → `workspace:*` chain correctly.
```

- [ ] **Step 3: Commit**

```bash
git add examples/monorepo/Cookfile examples/monorepo/README.md
git commit -m "feat(monorepo): add Cookfile and README for monorepo example"
```

---

### Task 6: End-to-end validation

- [ ] **Step 1: Run `cargo test` to verify cook-core changes**

Run: `cd /home/alex/dev/cook && cargo test`

Expected: All tests pass.

- [ ] **Step 2: Initialize the example workspace**

Run: `cd /home/alex/dev/cook/examples/monorepo && pnpm install`

Expected: pnpm resolves catalogs, installs typescript, creates `pnpm-lock.yaml` and `node_modules/`.

- [ ] **Step 3: Run `cook build` and verify ordering**

Run: `cd /home/alex/dev/cook/examples/monorepo && cook build`

Expected: Cook registers `install` recipe, then `build` recipe. The build recipe emits three add_units in topo order: `shared-utils` → `ui` → `web`. All three packages produce `dist/` directories. All three produce `.cook/<task>.done` marker files.

Verify: `ls packages/shared-utils/dist/index.js packages/ui/dist/index.js packages/web/dist/index.js`

- [ ] **Step 4: Verify caching — second run skips**

Run: `cook build` again.

Expected: All steps show as cache hits. No tsc invocations.

- [ ] **Step 5: Verify cache invalidation — source change cascades**

Run:
```bash
echo '// modified' >> packages/shared-utils/src/index.ts
cook build
```

Expected: `shared-utils` rebuilds (source changed). `ui` rebuilds (upstream marker changed). `web` rebuilds (upstream marker changed). Verify by checking Cook's output shows 3 steps executed.

Then restore the file:
```bash
git checkout packages/shared-utils/src/index.ts
```

- [ ] **Step 6: Run `cook test`**

Run: `cook test`

Expected: Tests pass for all three packages. Each test script runs `node -e "require('./dist/index.js')"` which proves the build output is correct.

- [ ] **Step 7: Run `cook clean` and verify cleanup**

Run: `cook clean && ls packages/*/dist 2>&1`

Expected: All `dist/` and `.cook/` directories removed. `ls` shows "No such file or directory".

- [ ] **Step 8: Commit pnpm-lock.yaml if generated**

If `pnpm install` generated a `pnpm-lock.yaml`, commit it:

```bash
git add examples/monorepo/pnpm-lock.yaml
git commit -m "chore: add pnpm-lock.yaml for monorepo example"
```

Note: If the `.gitignore` excludes `pnpm-lock.yaml`, remove that line first — lockfiles should be committed for reproducible builds.
