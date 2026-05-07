# DAG TUI: Always-Focused View + Selectable Files Folder

Status: design
Owner: alex
Date: 2026-05-06
Related: [dag-tui-viewer](../../../docs/superpowers/specs/2026-05-05-dag-tui-viewer-design.md), [dag-tui-compact-focus-and-files](2026-05-06-dag-tui-compact-focus-and-files-design.md)

## 1. Motivation

The viewer currently has two density modes: `Full` (renders the whole graph with boxed nodes) and `Compact` (renders only the selection's 1-hop neighborhood with single-row bracketed labels). Compact's focus filter is the affordance that makes the viewer usable on graphs above ~30 nodes — but its bracketed-label rendering loses the cache/status badges that make Full informative. Users want Compact's *behavior* (focus on what's selected) with Full's *visuals* (boxed nodes + badges). Today they can't have both.

Separately, the index tree's Files folder header (`▶ Files (N)`) renders but the cursor skips over it during j/k navigation. The folder is not in `visible_rows`. A user pointing at the folder header expects either expansion control or some indication that the row exists for navigation; today they get neither.

## 2. Scope

### In scope

- Remove `DensityMode` entirely. The viewer has one rendering path: focus subgraph → `LayoutDims::FULL` layout → boxed-node canvas draw.
- Selection drives the focused subgraph at every tier (wave / files-folder / file / recipe / unit). Focus rules from the existing compact-focus-and-files spec are preserved unchanged.
- The Files folder header becomes a selectable index row. Pressing `l`/`Right` on it expands the folder; `h`/`Left` collapses and steps back to the wave row. Its focused subgraph is identical to the parent wave's (the whole wave).
- Delete dead code: `state::DensityMode`, `state::choose_initial_mode`, `AppState.density`, `render::canvas::draw_compact`, the `m` keybinding, and the `LayoutDims::COMPACT` dispatch branch.

### Out of scope

- No new focus-depth control (no 2-hop focus, no `[`/`]` to widen radius).
- No new "highlight a set of nodes on the canvas" affordance — the Files folder selection re-uses the wave-level subgraph as-is.
- No changes to the focus rules for unit, recipe, file-leaf, or wave-only selections.
- No changes to Flow mode (already removed in commit 938d16b).
- No wire-format change. `VIEWER_SCHEMA_VERSION` stays at 2.

## 3. Single Render Path

### 3.1 `pick_layout`

`render::pick_layout` collapses to a straight-line dispatcher:

```rust
pub fn pick_layout(app: &AppState, graph: &WaveDagData) -> layout::Layout {
    let focused = focus::focus_subgraph(graph, app);
    layout::compute(&focused, layout::LayoutDims::FULL)
}
```

No mode branching. Every render runs the focus filter, then the Full-dims layout.

### 3.2 `render::canvas::render`

The `match app.density { Compact => draw_compact, Full => draw_nodes }` dispatch is removed. Only `draw_nodes` (the boxed-node Full draw routine) remains. `draw_compact` and any helpers it owns (bracketed-label width math, single-row positioning) are deleted.

### 3.3 Removed surface

| Symbol | Location | Disposition |
|---|---|---|
| `DensityMode` enum | `state.rs` | Delete |
| `DensityMode::next` | `state.rs` | Delete |
| `choose_initial_mode` | `state.rs` | Delete |
| `AppState.density` | `state.rs` | Delete |
| `LayoutDims::COMPACT` | `render/layout.rs` | Delete iff no other caller. Audit before removal. |
| `draw_compact` and helpers | `render/canvas.rs` | Delete |
| `m` keybinding | `input.rs` | Delete |
| `m mode` hint in bottom bar | `render/mod.rs::draw_bottom_bar` | Delete from hint string |
| `m: cycle density` help line | help overlay | Delete |

## 4. Selectable Files Folder

### 4.1 New selection variant

```rust
pub enum SelectionLeaf {
    Recipe { recipe: usize, unit: Option<usize> },
    FilesFolder,            // new — selects the Files folder header row
    File(usize),
}
```

`Selection::files_folder(wave: usize) -> Self` constructor mirrors the existing `wave_only`, `recipe`, `unit`, `file` constructors.

`Selection::node_id` returns `None` for `FilesFolder` — like `wave_only` and `Recipe { unit: None }`, the folder header is a container row, not a single graph node.

### 4.2 `visible_rows` ordering

Per wave, in order:

1. `Selection::wave_only(wi)` — always
2. `Selection::files_folder(wi)` — only when `wave.expanded` and `!wave.files.is_empty()`
3. File leaf rows — only when (2) is present and `wave.files_expanded`
4. Recipe rows — only when `wave.expanded`
5. Unit rows — only when the parent recipe is expanded

The folder row is always present (and selectable) whenever the wave is expanded and has files, regardless of `files_expanded`. This is the key fix: the row is in `visible_rows`, so j/k can land on it.

### 4.3 Tree-navigation semantics

`expand_or_step_in` (`l` / `Right`):

| Current selection | Action |
|---|---|
| `wave_only(w)`, wave collapsed | Expand wave |
| `wave_only(w)`, wave expanded, has files | Step into `files_folder(w)` |
| `wave_only(w)`, wave expanded, no files, has recipes | Step into first recipe (`Selection::recipe(w, 0)`) |
| `wave_only(w)`, wave expanded, no files, no recipes | No-op |
| `files_folder(w)`, files collapsed | Expand files |
| `files_folder(w)`, files expanded | Step into first file (`Selection::file(w, 0)`) |
| `Recipe { unit: None }`, recipe collapsed | Expand recipe |
| `Recipe { unit: None }`, recipe expanded | Step into first unit |
| `Recipe { unit: Some(_) }` or `File(_)` | No-op (already at leaf) |

`collapse_or_step_out` (`h` / `Left`):

| Current selection | Action |
|---|---|
| `Recipe { unit: Some(_) }` | Step to `Recipe { unit: None }` |
| `Recipe { unit: None }`, expanded | Collapse recipe |
| `Recipe { unit: None }`, collapsed | Step to `wave_only` (parent) |
| `File(_)` | Step to `files_folder(w)` |
| `files_folder(w)`, files expanded | Collapse files |
| `files_folder(w)`, files collapsed | Step to `wave_only(w)` |
| `wave_only(w)`, expanded | Collapse wave |
| `wave_only(w)`, collapsed | No-op |

This replaces the current quirk where `expand_or_step_in` on an already-expanded wave with files would auto-open the files folder. Now the user explicitly steps into the folder row first, then expands it. Step-out from a file row goes to the folder row (not all the way back to the wave row), matching the index tree shape.

### 4.4 Focus subgraph for `FilesFolder`

`focus_set` adds:

```rust
Some(SelectionLeaf::FilesFolder) => {
    for n in &wave.nodes {
        out.insert(n.id.clone());
    }
}
```

— byte-identical to the `None` (wave-only) arm. `expand_one_hop` returns `visible` unchanged for `FilesFolder` (the wave-level early-return widens to cover it):

```rust
if matches!(
    app.selection.leaf,
    None | Some(SelectionLeaf::FilesFolder)
) {
    return visible;
}
```

Net effect: the focused subgraph for `files_folder(w)` is the same single-wave graph as for `wave_only(w)`. The user sees the same canvas; only the index-tree highlight moves.

### 4.5 Index renderer

`render::index::render_tree` already draws the folder header. Two changes:

1. The folder-header line gets the same `sel_style(app.selection, Selection::files_folder(wi))` REVERSED highlight treatment that other selectable rows use.
2. No layout/glyph change — `▶ Files (N)` / `▼ Files (N)` rendering is unchanged.

### 4.6 `jump_to_node` and `jump_to_pin_slot`

No semantic change. Both still land on a file leaf (`Selection::file(w, fi)`) when the target is a file id, and they still expand `files_expanded` to make the leaf visible. Neither ever lands on the folder row.

`bulk_pin_recipe` is a no-op on `FilesFolder` selection (no resolvable node id). It emits `PinMsg::OnFile` — the existing message text ("bulk-pin needs a unit selection") is accurate for any non-unit container row, so we reuse the variant rather than adding a new one.

## 5. Tests

### 5.1 `state.rs`

- `Selection::files_folder(0).node_id(&tree)` returns `None`.
- `visible_rows` for an expanded wave with files emits `[wave, files_folder, recipes…]` when `files_expanded == false` and `[wave, files_folder, files…, recipes…]` when `files_expanded == true`.
- `visible_rows` for an expanded wave with zero files omits the `files_folder` row.
- `move_cursor(false)` on `wave_only(0)` lands on `files_folder(0)` when the wave has files (regression: today it skips to the first recipe).
- `expand_or_step_in` on `files_folder(w)`: first call sets `files_expanded = true`; second call moves selection to `Selection::file(w, 0)`.
- `collapse_or_step_out` on `Selection::file(w, 0)` returns to `files_folder(w)` (regression: today it returns to `wave_only`).
- `collapse_or_step_out` on `files_folder(w)` with files expanded sets `files_expanded = false` (does not move selection).
- `collapse_or_step_out` on `files_folder(w)` with files collapsed returns to `wave_only(w)`.
- `bulk_pin_recipe` on `files_folder` selection emits the appropriate "no unit selected" message.

### 5.2 `render/focus.rs`

- `focus_subgraph` for `Selection::files_folder(w)` returns a graph byte-equal to `focus_subgraph` for `Selection::wave_only(w)` on the same input.
- All existing focus-rule tests for wave / recipe / unit / file selections continue to pass unchanged.

### 5.3 `render/index.rs`

- Folder header row is REVERSED-highlighted when `app.selection == Selection::files_folder(wi)`.
- Folder header is hidden in waves with zero files (existing test, regression guard).

### 5.4 `render/canvas.rs`

- The boxed-node canvas snapshot at unit-level focus contains only the focused unit + its 1-hop neighbors (not the whole wave).
- The boxed-node canvas snapshot at wave-level focus contains every node in that wave (and only that wave).
- The boxed-node canvas snapshot at `FilesFolder` selection equals the wave-level snapshot for the same wave.

### 5.5 Integration

- Pressing `m` is a no-op (no panic, no visual change).
- The bottom-bar hint string no longer contains `m mode`.

### 5.6 Snapshots to delete

- All Compact-mode bracketed-label snapshots (`tests/compact_snapshots.rs`).
- Any Full-mode whole-graph snapshot that asserted "the entire graph is rendered" — there is no longer a render path that produces that output. Replace with focused-subgraph snapshots covering each selection tier.

## 6. Migration / compatibility

- No wire-format change. `VIEWER_SCHEMA_VERSION` unchanged.
- No Cookfile language change. No Cook Standard touchpoint.
- The `m` key is freed up for future use; the design does not bind it to anything new.
- `PinMsg::OnFile` text ("bulk-pin needs a unit selection") already covers the `FilesFolder` case semantically; reuse rather than introducing a new variant.
- The previous "compact-as-focus" spec (2026-05-06-dag-tui-compact-focus-and-files-design.md) is superseded for the rendering-mode questions; its file-rows-in-the-index portion remains in force and is unchanged by this design.

## 7. Open questions

None — design accepted in conversation 2026-05-06.
