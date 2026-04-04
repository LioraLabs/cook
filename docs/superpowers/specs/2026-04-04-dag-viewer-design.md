# DAG Viewer — Design Spec

**Date:** 2026-04-04
**Status:** Approved

## Goal

Add `cook --dag [recipe]` that launches a local web server serving an interactive visualization of the build DAG. Shows three levels of detail: recipe-level topology, per-recipe internal mini-DAGs (step groups and barriers), and individual node details with cache status.

## CLI

`--dag` is a global flag on the `cook` CLI. When set:

1. Parse the Cookfile and resolve dependencies (existing pipeline through registration)
2. Build the full DAG data structure (recipes, work units, edges, cache status)
3. Serialize to JSON, embed in an HTML template
4. Start a local HTTP server on a random port
5. Print the URL to stderr and open it in the default browser
6. Block until Ctrl+C

No build executes. The `[recipe]` argument defaults to `"build"` as usual — it determines the root of the DAG (only reachable recipes are shown).

## Three-Level Visualization

### Level 1: Recipe DAG (default view)

All recipes shown as collapsed nodes. Edges represent cross-recipe dependencies from `requires` and implicit file dependencies. This is the high-level build topology.

```
[setup] → [lib] → [lua]  → [build]
                → [luac] ↗
```

### Level 2: Recipe Mini-DAG (click to expand)

Each recipe contains its own internal DAG of work units. Click a recipe node to expand it into a compound node showing:

- **Step groups** — units in the same step group are parallel (laid out side-by-side)
- **Sequential barriers** — units with `DepKind::Sequential` depend on the previous barrier
- **Internal edges** — step group N feeds into the next sequential barrier

Example for `lib`:
```
┌─ lib ──────────────────────────────────────────────┐
│  [lapi.o] [lauxlib.o] [lbaselib.o] ... [lzio.o]   │  ← step group 0 (parallel)
│                        │                            │
│                  [liblua.a]                         │  ← sequential (many-to-one)
└─────────────────────────────────────────────────────┘
```

### Level 3: Node Details (click a node)

Click an individual work unit to see a detail panel:
- Command template (e.g., `gcc -O2 -Wall ... -c {in} -o {out}`)
- Inputs and output paths
- Cache status: cached / rebuild (and reason if rebuild)
- Step group index

## Data Model

The Rust side serializes this JSON structure:

```json
{
  "target": "build",
  "recipes": [
    {
      "name": "lib",
      "deps": ["setup"],
      "units": [
        {
          "id": "lib:0",
          "label": "lapi.o",
          "command": "gcc -O2 ... -c lua-5.4.7/src/lapi.c -o build/obj/lapi.o",
          "inputs": ["lua-5.4.7/src/lapi.c"],
          "output": "build/obj/lapi.o",
          "dep_kind": "step_group",
          "group_index": 0,
          "cached": true
        },
        {
          "id": "lib:32",
          "label": "liblua.a",
          "command": "ar rcs build/liblua.a ...",
          "inputs": ["build/obj/lapi.o", "..."],
          "output": "build/liblua.a",
          "dep_kind": "sequential",
          "group_index": null,
          "cached": false
        }
      ],
      "step_groups": [[0, 1, 2, "...indices..."], [32]],
      "internal_edges": [
        {"from": 0, "to": 32},
        {"from": 1, "to": 32}
      ]
    }
  ]
}
```

The internal edges are derived from the step group / sequential barrier structure:
- All units in step group N have edges to the first sequential unit after the group (or the next step group's barrier)
- This mirrors exactly what `dag_builder::build_dag` computes

## Building the DAG Data

To build the full DAG without executing:

1. Use existing `analyzer::dependency_edges_multi` to get the recipe DAG
2. Use `RecipeDag` to determine wave order
3. For each recipe: call `registry.register_recipe()` to get `RecipeUnits`
4. For each unit with `cache_meta`: call `needs_rebuild_cook` to annotate cache status
5. Derive internal edges from `step_groups` + `DepKind` (same logic as `dag_builder` but serialized instead of executed)
6. Collect everything into the JSON structure above

## Frontend

**Single HTML page** embedded as a Rust string constant in the binary.

**Libraries (CDN):**
- Cytoscape.js — graph rendering, pan/zoom, interaction
- cytoscape-dagre — DAG layout algorithm (top-to-bottom)

**Layout:**
- Top-to-bottom DAG using dagre
- Compound nodes for recipes (Cytoscape parent/child)
- Recipe nodes start collapsed; children hidden
- Click toggles expand/collapse

**Styling (CSS-like Cytoscape selectors):**

| Element | Style |
|---|---|
| Collapsed recipe | Indigo fill (`#6366f1`), white label, rounded rectangle |
| Expanded recipe (parent) | Dark background (`#1e1e3a`), indigo dashed border |
| Cached work unit | Dark fill, green border (`#22c55e`) |
| Uncached work unit | Dark fill, gray border (`#6b7280`) |
| Many-to-one step | Dark fill, amber border (`#f59e0b`) |
| Shell step (cache=false) | Dark fill, dim gray border |
| Edges | Gray arrows (`#4a4a6a`) |

**Detail panel:** Sidebar or tooltip on node click showing command, inputs, output, cache status.

**Dark theme** matching the mockup — `#1a1a2e` background.

## Server

Use `tiny_http` crate:
- Bind to `127.0.0.1:0` (random available port)
- Serve the HTML page on `GET /`
- Serve the DAG JSON on `GET /dag.json` (alternative to inlining)
- Print URL to stderr
- Attempt to open in default browser via `xdg-open` / `open`
- Block on request loop until process is killed

## Files

| File | Action | Purpose |
|---|---|---|
| `cli/crates/cook-cli/src/cli.rs` | Modify | Add `--dag` flag |
| `cli/crates/cook-cli/src/main.rs` | Modify | Wire `--dag` to `cmd_dag()` |
| `cli/crates/cook-cli/src/pipeline.rs` | Modify | Add `cmd_dag()` function |
| `cli/crates/cook-cli/src/dag_server.rs` | Create | HTTP server + HTML template |
| `cli/crates/cook-cli/Cargo.toml` | Modify | Add `tiny_http` dependency |

## Out of Scope

- Live build updates (websocket streaming of execution events)
- Interactive build triggering from the UI
- Editing the DAG
- Exporting to image/SVG
- Build timing waterfall / critical path analysis
