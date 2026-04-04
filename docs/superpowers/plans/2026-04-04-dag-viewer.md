# DAG Viewer Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add `cook --dag [recipe]` that serves an interactive Cytoscape.js visualization of the build DAG with three-level drill-down (recipe topology, per-recipe mini-DAGs, node details).

**Architecture:** The `--dag` flag short-circuits the normal build pipeline after recipe registration. A new `cmd_dag()` function registers all recipes to collect `RecipeUnits`, checks cache status for each unit, serializes the data as JSON, injects it into an HTML template containing Cytoscape.js, and serves it via `tiny_http`. The HTML/JS is embedded as a Rust string constant.

**Tech Stack:** Rust, tiny_http, Cytoscape.js (CDN), cytoscape-dagre (CDN), serde_json

**Spec:** `docs/superpowers/specs/2026-04-04-dag-viewer-design.md`

---

## Chunk 1: CLI + Data Serialization

### Task 1: Add `--dag` CLI flag and wire to `cmd_dag()`

**Files:**
- Modify: `cli/crates/cook-cli/src/cli.rs`
- Modify: `cli/crates/cook-cli/src/main.rs`
- Modify: `cli/crates/cook-cli/src/pipeline.rs`

- [ ] **Step 1: Add `--dag` flag to Cli struct**

In `cli/crates/cook-cli/src/cli.rs`, add after the `emit_lua` field:

```rust
    /// Visualize the build DAG in a browser
    #[arg(long = "dag", global = true)]
    pub dag: bool,
```

- [ ] **Step 2: Add stub `cmd_dag()` to pipeline.rs**

Add to the bottom of `cli/crates/cook-cli/src/pipeline.rs`, before the workspace helpers section:

```rust
// ---------------------------------------------------------------------------
// cmd_dag
// ---------------------------------------------------------------------------

pub fn cmd_dag(cli: &Cli, recipe_name: &str, config: Option<&str>) -> Result<(), CookError> {
    eprintln!("cook: --dag is not yet implemented");
    Ok(())
}
```

- [ ] **Step 3: Wire `--dag` into main.rs**

In `cli/crates/cook-cli/src/main.rs`, update the imports to include `cmd_dag`:

```rust
use pipeline::{cmd_dag, cmd_init, cmd_menu, cmd_run, cmd_serve, cmd_test};
```

Then add the `--dag` check at the top of the `match` block. Replace the entire match expression:

```rust
    let result = if cli.dag {
        match &cli.command {
            Some(Command::External(args)) => {
                let recipe = args.first().map(|s| s.as_str()).unwrap_or("build");
                let config = args.get(1).map(|s| s.as_str());
                cmd_dag(&cli, recipe, config)
            }
            None => cmd_dag(&cli, "build", None),
            _ => {
                eprintln!("cook: --dag can only be used with a recipe target");
                std::process::exit(1);
            }
        }
    } else {
        match &cli.command {
            Some(Command::Init) => cmd_init(),
            Some(Command::Menu) => cmd_menu(&cli),
            Some(Command::Serve { recipe, config }) => {
                let recipe = recipe.clone();
                cmd_serve(&cli, &recipe, config.as_deref())
            }
            Some(Command::Test {
                filter,
                verbose,
                timeout_multiplier,
                wrapper,
                list,
            }) => cmd_test(
                &cli,
                filter.clone(),
                *verbose,
                *timeout_multiplier,
                wrapper.clone(),
                *list,
            ),
            Some(Command::External(args)) => {
                let recipe = args.first().map(|s| s.as_str()).unwrap_or("build");
                let config = args.get(1).map(|s| s.as_str());
                cmd_run(&cli, recipe, config)
            }
            None => cmd_run(&cli, "build", None),
        }
    };
```

- [ ] **Step 4: Compile and verify**

Run: `cd /home/alex/dev/cook/cli && cargo build`
Expected: Compiles. `cook --dag` prints "cook: --dag is not yet implemented".

- [ ] **Step 5: Commit**

```bash
git add cli/crates/cook-cli/src/cli.rs cli/crates/cook-cli/src/main.rs cli/crates/cook-cli/src/pipeline.rs
git commit -m "feat(cook-cli): add --dag flag stub"
```

---

### Task 2: Build DAG data model and serialization

**Files:**
- Create: `cli/crates/cook-cli/src/dag_data.rs`
- Modify: `cli/crates/cook-cli/src/main.rs` (add mod)
- Modify: `cli/crates/cook-cli/Cargo.toml` (add serde, serde_json)

- [ ] **Step 1: Add serde dependencies**

In `cli/crates/cook-cli/Cargo.toml`, add to `[dependencies]`:

```toml
serde = { version = "1", features = ["derive"] }
serde_json = "1"
```

- [ ] **Step 2: Add `mod dag_data` to main.rs**

In `cli/crates/cook-cli/src/main.rs`, add after the existing `mod` declarations:

```rust
mod dag_data;
```

- [ ] **Step 3: Create dag_data.rs with serializable types**

Create `cli/crates/cook-cli/src/dag_data.rs`:

```rust
//! DAG data model for JSON serialization to the frontend viewer.

use serde::Serialize;
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

use cook_cache::{needs_rebuild_cook, RebuildResult, ThreadSafeCacheManager};
use cook_contracts::{CapturedUnit, DepKind, RecipeUnits, WorkPayload};

#[derive(Serialize)]
pub struct DagData {
    pub target: String,
    pub recipes: Vec<RecipeData>,
}

#[derive(Serialize)]
pub struct RecipeData {
    pub name: String,
    pub deps: Vec<String>,
    pub units: Vec<UnitData>,
    pub step_groups: Vec<Vec<usize>>,
    pub internal_edges: Vec<InternalEdge>,
}

#[derive(Serialize)]
pub struct UnitData {
    pub id: String,
    pub label: String,
    pub command: String,
    pub inputs: Vec<String>,
    pub output: Option<String>,
    pub dep_kind: String,
    pub group_index: Option<usize>,
    pub cached: bool,
}

#[derive(Serialize)]
pub struct InternalEdge {
    pub from: usize,
    pub to: usize,
}

/// Build a UnitData from a CapturedUnit, checking cache status.
fn build_unit_data(
    recipe_name: &str,
    unit_idx: usize,
    unit: &CapturedUnit,
    cache_manager: Option<&Arc<ThreadSafeCacheManager>>,
    working_dir: &Path,
) -> UnitData {
    let command = match &unit.payload {
        WorkPayload::Shell { cmd, .. } => cmd.clone(),
        WorkPayload::Interactive { cmd, .. } => format!("@{cmd}"),
        WorkPayload::LuaChunk { code, .. } => format!("lua: {}", &code[..code.len().min(60)]),
        WorkPayload::Test { cmd, .. } => format!("test: {cmd}"),
    };

    let (inputs, output) = match &unit.cache_meta {
        Some(meta) => (meta.input_paths.clone(), meta.output_path.clone()),
        None => (vec![], None),
    };

    let label = if let Some(ref out) = output {
        // Use just the filename for the label
        Path::new(out)
            .file_name()
            .map(|f| f.to_string_lossy().to_string())
            .unwrap_or_else(|| out.clone())
    } else {
        command.chars().take(40).collect()
    };

    let (dep_kind_str, group_index) = match &unit.dep_kind {
        DepKind::StepGroup(idx) => ("step_group".to_string(), Some(*idx)),
        DepKind::Sequential => ("sequential".to_string(), None),
        DepKind::TestSibling(idx) => ("test_sibling".to_string(), Some(*idx)),
    };

    // Check cache status
    let cached = if let (Some(meta), Some(cm)) = (&unit.cache_meta, cache_manager) {
        if let Some(ref out) = meta.output_path {
            let cache = cm.get_or_load(&meta.recipe_name);
            let entry = cache.steps.get(&meta.cache_key);
            let input_refs: Vec<&str> = meta.input_paths.iter().map(|s| s.as_str()).collect();
            let (result, _) = needs_rebuild_cook(
                entry,
                &input_refs,
                out,
                meta.command_hash,
                working_dir,
            );
            matches!(result, RebuildResult::Skip)
        } else {
            false
        }
    } else {
        false
    };

    UnitData {
        id: format!("{}:{}", recipe_name, unit_idx),
        label,
        command,
        inputs,
        output,
        dep_kind: dep_kind_str,
        group_index,
        cached,
    }
}

/// Derive internal edges from step_groups and DepKind.
/// Mirrors the logic in dag_builder::build_dag.
fn derive_internal_edges(units: &[CapturedUnit], step_groups: &[Vec<usize>]) -> Vec<InternalEdge> {
    let mut edges = Vec::new();

    // Build a lookup: unit_idx -> which step_group it belongs to
    let mut unit_group: BTreeMap<usize, usize> = BTreeMap::new();
    for (gi, group) in step_groups.iter().enumerate() {
        for &unit_idx in group {
            unit_group.insert(unit_idx, gi);
        }
    }

    // Walk units and determine edges based on barrier logic
    let mut barrier: Vec<usize> = Vec::new();

    for (unit_idx, unit) in units.iter().enumerate() {
        match &unit.dep_kind {
            DepKind::Sequential => {
                // Sequential unit depends on current barrier
                for &b in &barrier {
                    edges.push(InternalEdge { from: b, to: unit_idx });
                }
                barrier = vec![unit_idx];
            }
            DepKind::StepGroup(gi) | DepKind::TestSibling(gi) => {
                // First unit in a group depends on the barrier
                let group = &step_groups[*gi];
                let is_first = group.first() == Some(&unit_idx);
                if is_first {
                    for &b in &barrier {
                        for &member in group {
                            edges.push(InternalEdge { from: b, to: member });
                        }
                    }
                }
                // Check if this is the last member — update barrier
                let is_last = group.last() == Some(&unit_idx);
                if is_last {
                    barrier = group.clone();
                }
            }
        }
    }

    edges
}

/// Build the full DagData from registered RecipeUnits.
pub fn build_dag_data(
    target: &str,
    all_units: &[(String, RecipeUnits)],
    recipe_deps: &BTreeMap<String, Vec<String>>,
    cache_managers: &BTreeMap<String, Arc<ThreadSafeCacheManager>>,
) -> DagData {
    let mut recipes = Vec::new();

    for (name, ru) in all_units {
        let deps = recipe_deps.get(name).cloned().unwrap_or_default();
        let cm = cache_managers.get(name);

        let units: Vec<UnitData> = ru
            .units
            .iter()
            .enumerate()
            .map(|(idx, unit)| build_unit_data(name, idx, unit, cm, &ru.working_dir))
            .collect();

        let internal_edges = derive_internal_edges(&ru.units, &ru.step_groups);

        recipes.push(RecipeData {
            name: name.clone(),
            deps,
            units,
            step_groups: ru.step_groups.clone(),
            internal_edges,
        });
    }

    DagData {
        target: target.to_string(),
        recipes,
    }
}
```

- [ ] **Step 4: Compile**

Run: `cd /home/alex/dev/cook/cli && cargo build`
Expected: Compiles with no errors (dag_data is imported but not yet called).

- [ ] **Step 5: Commit**

```bash
git add cli/crates/cook-cli/src/dag_data.rs cli/crates/cook-cli/src/main.rs cli/crates/cook-cli/Cargo.toml
git commit -m "feat(cook-cli): add DAG data model for viewer serialization"
```

---

### Task 3: Implement `cmd_dag()` — register recipes and build DAG data

**Files:**
- Modify: `cli/crates/cook-cli/src/pipeline.rs`

- [ ] **Step 1: Replace the stub `cmd_dag()` with the real implementation**

Replace the `cmd_dag` function in `cli/crates/cook-cli/src/pipeline.rs`:

```rust
pub fn cmd_dag(cli: &Cli, recipe_name: &str, config: Option<&str>) -> Result<(), CookError> {
    let (cookfile, lua_source) = read_and_parse(cli)?;

    let cookfile_dir = cli.file.parent().unwrap_or(Path::new("."));
    let cookfile_dir = if cookfile_dir.as_os_str().is_empty() {
        Path::new(".")
    } else {
        cookfile_dir
    };
    let dotenv_vars = load_env(cookfile_dir);
    let env_vars = resolve_env(&cookfile, config, dotenv_vars, &cli.set)?;

    let recipe_infos = build_single_recipe_infos(&cookfile);
    let targets = vec![recipe_name.to_string()];

    // Resolve dependency edges for the target
    let edges = cook_engine::analyzer::dependency_edges_multi(&recipe_infos, &targets)
        .map_err(|e| match e {
            cook_engine::analyzer::GraphError::CycleDetected(s) => {
                CookError::Other(format!("dependency cycle involving: {s}"))
            }
            cook_engine::analyzer::GraphError::UnknownRecipe(s) => CookError::RecipeNotFound(s),
        })?;

    // Determine execution order via recipe DAG
    let mut recipe_dag = cook_engine::recipe_dag::RecipeDag::new(&edges);
    let mut all_units: Vec<(String, cook_contracts::RecipeUnits)> = Vec::new();
    let mut cache_managers: std::collections::BTreeMap<String, std::sync::Arc<cook_cache::ThreadSafeCacheManager>> = std::collections::BTreeMap::new();

    let registry = cook_register::Registry::new(
        cookfile_dir.to_path_buf(),
        env_vars,
    );

    // Register all recipes wave by wave
    loop {
        let ready = recipe_dag.pop_ready();
        if ready.is_empty() {
            break;
        }

        for name in &ready {
            let units = registry.register_recipe(&lua_source, name).map_err(|e| {
                CookError::Other(format!("registration failed for '{name}': {e}"))
            })?;

            let cache_dir = cookfile_dir.join(".cook").join("cache");
            cache_managers
                .entry(name.clone())
                .or_insert_with(|| std::sync::Arc::new(cook_cache::ThreadSafeCacheManager::new(cache_dir)));

            all_units.push((name.clone(), units));
        }

        recipe_dag.mark_done(&ready);
    }

    let dag_data = crate::dag_data::build_dag_data(
        recipe_name,
        &all_units,
        &edges,
        &cache_managers,
    );

    let json = serde_json::to_string(&dag_data)
        .map_err(|e| CookError::Other(format!("failed to serialize DAG: {e}")))?;

    crate::dag_server::serve_dag(&json)?;

    Ok(())
}
```

- [ ] **Step 2: Add a temporary stub for dag_server::serve_dag**

Create `cli/crates/cook-cli/src/dag_server.rs` with a stub:

```rust
//! HTTP server for the DAG viewer.

use crate::error::CookError;

pub fn serve_dag(dag_json: &str) -> Result<(), CookError> {
    eprintln!("DAG JSON ({} bytes), server not yet implemented", dag_json.len());
    Ok(())
}
```

Add `mod dag_server;` to `cli/crates/cook-cli/src/main.rs` (after `mod dag_data;`).

- [ ] **Step 3: Compile and test the data pipeline**

Run: `cd /home/alex/dev/cook/cli && cargo build --release`

Then test:
```bash
cd /home/alex/dev/cook/examples/lua-build
../../cli/target/release/cook --dag build
```
Expected: Prints "DAG JSON (NNNN bytes), server not yet implemented" — confirms the full registration + serialization pipeline works.

- [ ] **Step 4: Commit**

```bash
git add cli/crates/cook-cli/src/pipeline.rs cli/crates/cook-cli/src/dag_server.rs cli/crates/cook-cli/src/main.rs
git commit -m "feat(cook-cli): implement cmd_dag registration and DAG data pipeline"
```

---

## Chunk 2: HTTP Server + Frontend

### Task 4: Implement the HTTP server with tiny_http

**Files:**
- Modify: `cli/crates/cook-cli/Cargo.toml` (add tiny_http, open)
- Modify: `cli/crates/cook-cli/src/dag_server.rs`

- [ ] **Step 1: Add tiny_http and open dependencies**

In `cli/crates/cook-cli/Cargo.toml`, add to `[dependencies]`:

```toml
tiny_http = "0.12"
open = "5"
```

- [ ] **Step 2: Implement serve_dag with tiny_http**

Replace `cli/crates/cook-cli/src/dag_server.rs`:

```rust
//! HTTP server for the DAG viewer.

use crate::error::CookError;

const HTML_TEMPLATE: &str = include_str!("dag_viewer.html");

pub fn serve_dag(dag_json: &str) -> Result<(), CookError> {
    let html = HTML_TEMPLATE.replace("/*DAG_DATA_PLACEHOLDER*/", dag_json);

    let server = tiny_http::Server::http("127.0.0.1:0")
        .map_err(|e| CookError::Other(format!("failed to start server: {e}")))?;

    let port = server.server_addr().to_ip().map(|a| a.port()).unwrap_or(0);
    let url = format!("http://127.0.0.1:{port}");

    eprintln!("cook: DAG viewer at {url}");
    eprintln!("cook: press Ctrl+C to stop");

    // Try to open in browser
    let _ = open::that(&url);

    loop {
        let request = match server.recv() {
            Ok(r) => r,
            Err(_) => break,
        };

        let response = tiny_http::Response::from_string(&html)
            .with_header(
                tiny_http::Header::from_bytes(&b"Content-Type"[..], &b"text/html; charset=utf-8"[..]).unwrap()
            );

        let _ = request.respond(response);
    }

    Ok(())
}
```

- [ ] **Step 3: Create the HTML template file**

Create `cli/crates/cook-cli/src/dag_viewer.html` with the full Cytoscape.js frontend. This is the most substantial file — it contains all the HTML, CSS, and JavaScript for the three-level DAG viewer.

```html
<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>cook dag</title>
<script src="https://unpkg.com/cytoscape@3.30.4/dist/cytoscape.min.js"></script>
<script src="https://unpkg.com/dagre@0.8.5/dist/dagre.min.js"></script>
<script src="https://unpkg.com/cytoscape-dagre@2.5.0/cytoscape-dagre.js"></script>
<style>
* { margin: 0; padding: 0; box-sizing: border-box; }
body { background: #1a1a2e; color: #e0e0e0; font-family: 'Inter', system-ui, -apple-system, sans-serif; }
#cy { width: 100vw; height: 100vh; }
#detail {
  position: fixed; top: 0; right: 0; width: 360px; height: 100vh;
  background: #16163a; border-left: 1px solid #2a2a4a;
  padding: 20px; overflow-y: auto; display: none;
  font-size: 13px; z-index: 10;
}
#detail.visible { display: block; }
#detail h2 { font-size: 16px; color: #818cf8; margin-bottom: 12px; }
#detail h3 { font-size: 12px; color: #6b7280; text-transform: uppercase; letter-spacing: 0.05em; margin-top: 16px; margin-bottom: 4px; }
#detail .value { color: #d1d5db; margin-bottom: 8px; word-break: break-all; }
#detail .cached-badge {
  display: inline-block; padding: 2px 8px; border-radius: 4px;
  font-size: 11px; font-weight: 600;
}
#detail .cached-badge.yes { background: #064e3b; color: #6ee7b7; }
#detail .cached-badge.no { background: #1f2937; color: #9ca3af; }
#detail .close {
  position: absolute; top: 12px; right: 12px;
  background: none; border: none; color: #6b7280; font-size: 18px; cursor: pointer;
}
#detail .close:hover { color: #e0e0e0; }
#header {
  position: fixed; top: 0; left: 0; right: 0; height: 40px;
  background: #16163a; border-bottom: 1px solid #2a2a4a;
  display: flex; align-items: center; padding: 0 16px;
  font-size: 13px; color: #8888cc; z-index: 10;
}
#header .title { font-weight: 600; color: #818cf8; margin-right: 16px; }
#legend {
  position: fixed; bottom: 16px; left: 16px;
  background: #16163a; border: 1px solid #2a2a4a; border-radius: 8px;
  padding: 12px 16px; font-size: 11px; color: #8888aa; z-index: 10;
}
#legend .item { display: flex; align-items: center; gap: 8px; margin-bottom: 4px; }
#legend .item:last-child { margin-bottom: 0; }
#legend .swatch {
  width: 14px; height: 14px; border-radius: 3px; flex-shrink: 0;
}
</style>
</head>
<body>

<div id="header">
  <span class="title">cook dag</span>
  <span id="info"></span>
</div>

<div id="cy"></div>

<div id="detail">
  <button class="close" onclick="closeDetail()">&times;</button>
  <div id="detail-content"></div>
</div>

<div id="legend">
  <div class="item"><div class="swatch" style="background:#6366f1"></div> Recipe (click to expand)</div>
  <div class="item"><div class="swatch" style="border:2px solid #22c55e;background:#1e293b"></div> Cached step</div>
  <div class="item"><div class="swatch" style="border:2px solid #f59e0b;background:#1e293b"></div> Many-to-one step</div>
  <div class="item"><div class="swatch" style="border:2px solid #6b7280;background:#1e293b"></div> Uncached step</div>
</div>

<script>
const DAG = /*DAG_DATA_PLACEHOLDER*/{};

// Track which recipes are expanded
const expanded = new Set();

function buildElements() {
  const elements = [];

  for (const recipe of DAG.recipes) {
    const isExpanded = expanded.has(recipe.name);

    // Recipe node (compound parent when expanded, regular when collapsed)
    elements.push({
      data: {
        id: 'recipe:' + recipe.name,
        label: recipe.name,
        type: 'recipe',
        expanded: isExpanded,
        unitCount: recipe.units.length,
        cachedCount: recipe.units.filter(u => u.cached).length,
      }
    });

    if (isExpanded) {
      // Add child nodes
      for (let i = 0; i < recipe.units.length; i++) {
        const unit = recipe.units[i];
        const isManyToOne = unit.dep_kind === 'sequential' && unit.output && recipe.units.length > 1;
        elements.push({
          data: {
            id: unit.id,
            parent: 'recipe:' + recipe.name,
            label: unit.label,
            type: 'unit',
            cached: unit.cached,
            manyToOne: isManyToOne,
            command: unit.command,
            inputs: unit.inputs,
            output: unit.output,
            depKind: unit.dep_kind,
            groupIndex: unit.group_index,
          }
        });
      }

      // Add internal edges
      for (const edge of recipe.internal_edges) {
        const fromId = recipe.name + ':' + edge.from;
        const toId = recipe.name + ':' + edge.to;
        elements.push({
          data: {
            id: 'ie:' + fromId + '->' + toId,
            source: fromId,
            target: toId,
            type: 'internal',
          }
        });
      }
    }

    // Cross-recipe edges
    for (const dep of recipe.deps) {
      elements.push({
        data: {
          id: 'edge:' + dep + '->' + recipe.name,
          source: 'recipe:' + dep,
          target: 'recipe:' + recipe.name,
          type: 'recipe',
        }
      });
    }
  }

  return elements;
}

function initCy() {
  const cy = window.cy = cytoscape({
    container: document.getElementById('cy'),
    elements: buildElements(),
    layout: {
      name: 'dagre',
      rankDir: 'TB',
      nodeSep: 30,
      rankSep: 60,
      padding: 60,
    },
    style: [
      // Collapsed recipe
      {
        selector: 'node[type="recipe"][!expanded]',
        style: {
          'background-color': '#6366f1',
          'color': '#ffffff',
          'label': 'data(label)',
          'text-valign': 'center',
          'text-halign': 'center',
          'font-size': '14px',
          'font-weight': 'bold',
          'width': 'label',
          'height': 40,
          'padding': '16px',
          'shape': 'round-rectangle',
          'border-width': 2,
          'border-color': '#818cf8',
        }
      },
      // Collapsed recipe with unit count
      {
        selector: 'node[type="recipe"][!expanded][unitCount > 0]',
        style: {
          'label': function(ele) {
            var d = ele.data();
            return d.label + ' (' + d.cachedCount + '/' + d.unitCount + ' cached)';
          },
        }
      },
      // Expanded recipe (compound parent)
      {
        selector: ':parent',
        style: {
          'background-color': '#1e1e3a',
          'border-color': '#6366f1',
          'border-width': 2,
          'border-style': 'dashed',
          'label': 'data(label)',
          'text-valign': 'top',
          'text-halign': 'center',
          'color': '#818cf8',
          'font-size': '12px',
          'padding': '20px',
          'text-margin-y': -8,
        }
      },
      // Cached work unit
      {
        selector: 'node[type="unit"][cached]',
        style: {
          'background-color': '#1e293b',
          'border-color': '#22c55e',
          'border-width': 2,
          'color': '#a0f0a0',
          'label': 'data(label)',
          'text-valign': 'center',
          'text-halign': 'center',
          'font-size': '10px',
          'width': 'label',
          'height': 30,
          'padding': '10px',
          'shape': 'round-rectangle',
        }
      },
      // Many-to-one step
      {
        selector: 'node[type="unit"][manyToOne]',
        style: {
          'border-color': '#f59e0b',
          'color': '#fbbf24',
        }
      },
      // Uncached work unit
      {
        selector: 'node[type="unit"][!cached]',
        style: {
          'background-color': '#1e293b',
          'border-color': '#6b7280',
          'border-width': 2,
          'color': '#d1d5db',
          'label': 'data(label)',
          'text-valign': 'center',
          'text-halign': 'center',
          'font-size': '10px',
          'width': 'label',
          'height': 30,
          'padding': '10px',
          'shape': 'round-rectangle',
        }
      },
      // Recipe edges
      {
        selector: 'edge[type="recipe"]',
        style: {
          'width': 2,
          'line-color': '#4a4a6a',
          'target-arrow-color': '#4a4a6a',
          'target-arrow-shape': 'triangle',
          'curve-style': 'bezier',
        }
      },
      // Internal edges
      {
        selector: 'edge[type="internal"]',
        style: {
          'width': 1.5,
          'line-color': '#3a3a5a',
          'target-arrow-color': '#3a3a5a',
          'target-arrow-shape': 'triangle',
          'curve-style': 'bezier',
        }
      },
    ],
  });

  // Click recipe to expand/collapse
  cy.on('tap', 'node[type="recipe"]', function(evt) {
    const name = evt.target.data('label');
    if (expanded.has(name)) {
      expanded.delete(name);
    } else {
      expanded.add(name);
    }
    rebuildGraph();
  });

  // Click unit to show detail
  cy.on('tap', 'node[type="unit"]', function(evt) {
    showDetail(evt.target.data());
  });

  // Click background to close detail
  cy.on('tap', function(evt) {
    if (evt.target === cy) {
      closeDetail();
    }
  });

  // Update info
  const totalUnits = DAG.recipes.reduce((s, r) => s + r.units.length, 0);
  const totalCached = DAG.recipes.reduce((s, r) => s + r.units.filter(u => u.cached).length, 0);
  document.getElementById('info').textContent =
    DAG.recipes.length + ' recipes, ' + totalUnits + ' units, ' + totalCached + ' cached — target: ' + DAG.target;
}

function rebuildGraph() {
  const cy = window.cy;
  cy.elements().remove();
  cy.add(buildElements());
  cy.layout({
    name: 'dagre',
    rankDir: 'TB',
    nodeSep: 30,
    rankSep: 60,
    padding: 60,
    animate: false,
  }).run();
}

function showDetail(data) {
  const el = document.getElementById('detail-content');
  const cachedClass = data.cached ? 'yes' : 'no';
  const cachedText = data.cached ? 'Cached' : 'Needs rebuild';

  let html = '<h2>' + escapeHtml(data.label) + '</h2>';
  html += '<span class="cached-badge ' + cachedClass + '">' + cachedText + '</span>';

  html += '<h3>Command</h3>';
  html += '<div class="value"><code>' + escapeHtml(data.command || 'N/A') + '</code></div>';

  if (data.output) {
    html += '<h3>Output</h3>';
    html += '<div class="value">' + escapeHtml(data.output) + '</div>';
  }

  if (data.inputs && data.inputs.length > 0) {
    html += '<h3>Inputs (' + data.inputs.length + ')</h3>';
    html += '<div class="value">' + data.inputs.map(escapeHtml).join('<br>') + '</div>';
  }

  html += '<h3>Dep Kind</h3>';
  html += '<div class="value">' + escapeHtml(data.depKind || 'N/A');
  if (data.groupIndex !== null && data.groupIndex !== undefined) {
    html += ' (group ' + data.groupIndex + ')';
  }
  html += '</div>';

  el.innerHTML = html;
  document.getElementById('detail').classList.add('visible');
}

function closeDetail() {
  document.getElementById('detail').classList.remove('visible');
}

function escapeHtml(s) {
  if (!s) return '';
  return s.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;').replace(/"/g, '&quot;');
}

initCy();
</script>
</body>
</html>
```

- [ ] **Step 4: Compile and run full test**

Run: `cd /home/alex/dev/cook/cli && cargo build --release`

Then test:
```bash
cd /home/alex/dev/cook/examples/lua-build
../../cli/target/release/cook --dag build
```
Expected: Opens a browser showing the DAG viewer with setup, lib, lua, luac, and build recipes. Click a recipe to expand its work units. Click a unit to see details.

- [ ] **Step 5: Test edge cases**

```bash
# Single recipe with no deps
../../cli/target/release/cook --dag setup

# Recipe that doesn't exist
../../cli/target/release/cook --dag nonexistent
```
Expected: Single recipe shows just that node. Nonexistent recipe shows error.

- [ ] **Step 6: Commit**

```bash
git add cli/crates/cook-cli/src/dag_server.rs cli/crates/cook-cli/src/dag_viewer.html cli/crates/cook-cli/Cargo.toml
git commit -m "feat(cook-cli): implement DAG viewer with Cytoscape.js frontend"
```
