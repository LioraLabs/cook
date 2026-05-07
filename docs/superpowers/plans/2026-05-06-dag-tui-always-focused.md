# DAG TUI Always-Focused View Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Collapse the viewer's two density modes into a single always-focused render path (Compact's selection-driven subgraph + Full's boxed-node visuals), and make the Files folder header a real selectable index row.

**Architecture:** Additive changes first (new `SelectionLeaf::FilesFolder` variant + index row + focus-set arm). Then a single switch in `render::pick_layout` and `render::canvas::render` to remove the `DensityMode` dispatch. Then mechanical removal of dead code (enum, keybinding, hint string, `LayoutDims::COMPACT`, compact-only tests).

**Tech Stack:** Rust 2024 edition, ratatui, crossterm. Tests use ratatui's `Buffer` and the in-process `SnapshotFrame`.

**Spec:** [`docs/superpowers/specs/2026-05-06-dag-tui-always-focused-design.md`](../specs/2026-05-06-dag-tui-always-focused-design.md)

**Working directory for all `cargo` invocations:** `cli/` (the workspace root). All file paths in this plan are relative to the repo root.

---

## File Structure

| File | Disposition |
|---|---|
| `cli/crates/cook-dag-viewer/src/state.rs` | Modify: add `SelectionLeaf::FilesFolder`, `Selection::files_folder`, update `node_id`, `visible_rows`, `expand_or_step_in`, `collapse_or_step_out`, `bulk_pin_recipe`. Remove `DensityMode`, `choose_initial_mode`, `AppState.density` and their tests. Update two existing tests for new step-in/step-out semantics. |
| `cli/crates/cook-dag-viewer/src/render/focus.rs` | Modify: add `FilesFolder` arm in `focus_set` and short-circuit in `expand_one_hop`. |
| `cli/crates/cook-dag-viewer/src/render/index.rs` | Modify: highlight folder header with REVERSED when selected. |
| `cli/crates/cook-dag-viewer/src/render/mod.rs` | Modify: collapse `pick_layout` to single-path; strip `m mode` from bottom-bar hint; remove `DensityMode` import. |
| `cli/crates/cook-dag-viewer/src/render/canvas.rs` | Modify: strip `app.density` dispatch; delete `draw_compact`, `truncate_to`, and the two compact-mode unit tests. |
| `cli/crates/cook-dag-viewer/src/render/layout.rs` | Modify: delete `LayoutDims::COMPACT`; update doc comment. |
| `cli/crates/cook-dag-viewer/src/input.rs` | Modify: delete the `m` keybinding handler. |
| `cli/crates/cook-dag-viewer/tests/compact_snapshots.rs` | Delete. |
| `cli/crates/cook-dag-viewer/tests/tui_integration.rs` | Modify: delete `m_cycles_density_mode`, `compact_mode_layout_filters_to_one_hop_for_unit_selection`, and the comparison test using `LayoutDims::COMPACT`. |

---

## Task 1: Add `SelectionLeaf::FilesFolder` variant and constructor

**Files:**
- Modify: `cli/crates/cook-dag-viewer/src/state.rs:216-281`

- [ ] **Step 1: Write the failing tests**

Append to the `#[cfg(test)] mod tests` block in `cli/crates/cook-dag-viewer/src/state.rs` (alongside the other selection tests, e.g. after `selection_node_id_resolves_file_leaf`):

```rust
#[test]
fn files_folder_constructor_builds_expected_selection() {
    let sel = Selection::files_folder(2);
    assert_eq!(sel.wave, 2);
    assert!(matches!(sel.leaf, Some(SelectionLeaf::FilesFolder)));
}

#[test]
fn selection_node_id_returns_none_for_files_folder() {
    let g = graph_with_files();
    let app = AppState::new(&g);
    assert_eq!(Selection::files_folder(0).node_id(&app.tree), None);
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run from `cli/`:

```
cargo test -p cook-dag-viewer files_folder_constructor_builds_expected_selection
```

Expected: compilation error or test failure — `SelectionLeaf::FilesFolder` doesn't exist yet, neither does `Selection::files_folder`.

- [ ] **Step 3: Add the variant and constructor**

In `cli/crates/cook-dag-viewer/src/state.rs`, change the `SelectionLeaf` enum (currently lines 216-223):

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelectionLeaf {
    /// Selection inside the recipe subtree of a wave. `unit = None` means
    /// the recipe row itself is selected; `unit = Some(_)` means a unit row.
    Recipe { recipe: usize, unit: Option<usize> },
    /// Selection on the wave's `Files (N)` folder header row. Container
    /// row — has no resolvable graph node id, focuses on the whole wave.
    FilesFolder,
    /// Selection on a file row inside the wave's Files folder.
    File(usize),
}
```

Add the constructor inside `impl Selection` (next to the existing `wave_only`, `recipe`, `unit`, `file` constructors):

```rust
/// Files folder header row inside a wave.
pub fn files_folder(wave: usize) -> Self {
    Self {
        wave,
        leaf: Some(SelectionLeaf::FilesFolder),
    }
}
```

Update `Selection::node_id` (currently lines 290-303). Add the `FilesFolder` arm — it returns `None` like the wave-only and recipe-row cases:

```rust
pub fn node_id<'a>(&self, tree: &'a IndexTree) -> Option<&'a str> {
    let w = tree.waves.get(self.wave)?;
    match self.leaf? {
        SelectionLeaf::Recipe { recipe, unit } => {
            let r = w.recipes.get(recipe)?;
            let u = r.units.get(unit?)?;
            Some(&u.node_id)
        }
        SelectionLeaf::FilesFolder => None,
        SelectionLeaf::File(idx) => {
            let f = w.files.get(idx)?;
            Some(&f.node_id)
        }
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

```
cargo test -p cook-dag-viewer files_folder_constructor_builds_expected_selection
cargo test -p cook-dag-viewer selection_node_id_returns_none_for_files_folder
```

Expected: both PASS.

- [ ] **Step 5: Run the full state.rs test suite**

```
cargo test -p cook-dag-viewer --lib state::
```

Expected: all green. Existing pattern matches over `SelectionLeaf` may now warn about non-exhaustive matches — those will be fixed as we touch each call site in subsequent tasks. They should still type-check because the `_ => {}` or `Some(_) | None` arms in the existing code cover the new variant. If anything fails to compile, fix the offending match by adding a `SelectionLeaf::FilesFolder => { /* same as wave_only */ }` arm where appropriate.

- [ ] **Step 6: Commit**

```
git add cli/crates/cook-dag-viewer/src/state.rs
git commit -m "feat(cook-dag-viewer): selection variant for files folder header"
```

---

## Task 2: Files folder is selectable in `visible_rows`

**Files:**
- Modify: `cli/crates/cook-dag-viewer/src/state.rs:631-658`

- [ ] **Step 1: Write the failing test**

Append to the test module in `cli/crates/cook-dag-viewer/src/state.rs`:

```rust
#[test]
fn visible_rows_includes_files_folder_when_wave_expanded_and_has_files() {
    let g = graph_with_files();
    let app = AppState::new(&g);
    // graph_with_files() has wave 0 with one file (foo.cpp) and one unit.
    // Wave 0 is expanded by default. files_expanded is false by default.
    let rows = app.visible_rows();
    // Expected order:
    //   wave_only(0)
    //   files_folder(0)            ← new: present even when files collapsed
    //   recipe(0, 0)
    assert_eq!(rows[0], Selection::wave_only(0));
    assert_eq!(rows[1], Selection::files_folder(0));
    assert_eq!(rows[2], Selection::recipe(0, 0));
}

#[test]
fn visible_rows_omits_files_folder_when_wave_has_no_files() {
    let g = graph_2x2(); // no files in either wave
    let app = AppState::new(&g);
    let rows = app.visible_rows();
    // Wave 0 expanded, two recipes collapsed, then wave 1 collapsed.
    assert_eq!(rows[0], Selection::wave_only(0));
    assert_eq!(rows[1], Selection::recipe(0, 0));
    assert_eq!(rows[2], Selection::recipe(0, 1));
    assert_eq!(rows[3], Selection::wave_only(1));
    assert!(!rows.iter().any(|s| matches!(s.leaf, Some(SelectionLeaf::FilesFolder))));
}

#[test]
fn move_cursor_lands_on_files_folder_after_wave() {
    let g = graph_with_files();
    let mut app = AppState::new(&g);
    assert_eq!(app.selection, Selection::wave_only(0));
    app.move_cursor(false);
    assert_eq!(app.selection, Selection::files_folder(0));
    app.move_cursor(false);
    // files_expanded is still false, so next row is the recipe.
    assert_eq!(app.selection, Selection::recipe(0, 0));
}
```

The `visible_rows` method is currently private (`fn visible_rows`). For these tests to call it, change the signature to `pub fn visible_rows`. The method already returns `Vec<Selection>` so no other change is needed.

- [ ] **Step 2: Run tests to verify they fail**

```
cargo test -p cook-dag-viewer visible_rows_includes_files_folder_when_wave_expanded_and_has_files
cargo test -p cook-dag-viewer move_cursor_lands_on_files_folder_after_wave
```

Expected: FAIL — `visible_rows` does not yet emit a `files_folder` row.

- [ ] **Step 3: Update `visible_rows`**

Replace the `visible_rows` method body in `cli/crates/cook-dag-viewer/src/state.rs` (currently lines 631-658). Make it `pub`:

```rust
pub fn visible_rows(&self) -> Vec<Selection> {
    let mut out = Vec::new();
    for (wi, wave) in self.tree.waves.iter().enumerate() {
        out.push(Selection::wave_only(wi));
        if !wave.expanded {
            continue;
        }
        // Files folder header is selectable whenever the wave has any files.
        // Its presence in visible_rows does not depend on files_expanded;
        // only whether the file leaf rows below it are present does.
        if !wave.files.is_empty() {
            out.push(Selection::files_folder(wi));
            if wave.files_expanded {
                for fi in 0..wave.files.len() {
                    out.push(Selection::file(wi, fi));
                }
            }
        }
        for (ri, recipe) in wave.recipes.iter().enumerate() {
            out.push(Selection::recipe(wi, ri));
            if !recipe.expanded {
                continue;
            }
            for ui in 0..recipe.units.len() {
                out.push(Selection::unit(wi, ri, ui));
            }
        }
    }
    out
}
```

- [ ] **Step 4: Run tests to verify they pass**

```
cargo test -p cook-dag-viewer --lib state::
```

Expected: the three new tests PASS. The existing test `move_cursor_walks_through_file_rows_when_folder_expanded` (currently expects `wave_only → file(0,0) → recipe(0,0)`) will now FAIL because the folder row sits between wave and file. Update that test to match the new ordering:

In `cli/crates/cook-dag-viewer/src/state.rs`, find the test `move_cursor_walks_through_file_rows_when_folder_expanded` and update its body:

```rust
#[test]
fn move_cursor_walks_through_file_rows_when_folder_expanded() {
    let g = graph_with_files();
    let mut app = AppState::new(&g);
    app.tree.waves[0].files_expanded = true;
    // Visible rows in order:
    //   wave_only(0)
    //   files_folder(0)
    //   file(0, 0)            ← foo.cpp
    //   recipe(0, 0)
    assert_eq!(app.selection, Selection::wave_only(0));
    app.move_cursor(false);
    assert_eq!(app.selection, Selection::files_folder(0));
    app.move_cursor(false);
    assert_eq!(app.selection, Selection::file(0, 0));
    app.move_cursor(false);
    assert_eq!(app.selection, Selection::recipe(0, 0));
}
```

Re-run:

```
cargo test -p cook-dag-viewer --lib state::
```

Expected: all green.

- [ ] **Step 5: Commit**

```
git add cli/crates/cook-dag-viewer/src/state.rs
git commit -m "feat(cook-dag-viewer): files folder header is a selectable index row"
```

---

## Task 3: Update `expand_or_step_in` semantics

**Files:**
- Modify: `cli/crates/cook-dag-viewer/src/state.rs:593-617`

- [ ] **Step 1: Write the failing tests**

Append to the state test module:

```rust
#[test]
fn expand_step_in_on_wave_with_files_steps_into_folder_row() {
    let g = graph_with_files();
    let mut app = AppState::new(&g);
    // Wave 0 already expanded by default. Selection = wave_only(0).
    app.expand_or_step_in();
    // New behavior: stepping into an already-expanded wave with files
    // moves selection to the folder row (does NOT toggle files_expanded).
    assert_eq!(app.selection, Selection::files_folder(0));
    assert!(!app.tree.waves[0].files_expanded);
}

#[test]
fn expand_step_in_on_files_folder_collapsed_expands_it() {
    let g = graph_with_files();
    let mut app = AppState::new(&g);
    app.selection = Selection::files_folder(0);
    app.expand_or_step_in();
    assert!(app.tree.waves[0].files_expanded);
    // Selection stays on the folder row after expansion.
    assert_eq!(app.selection, Selection::files_folder(0));
}

#[test]
fn expand_step_in_on_files_folder_expanded_steps_into_first_file() {
    let g = graph_with_files();
    let mut app = AppState::new(&g);
    app.tree.waves[0].files_expanded = true;
    app.selection = Selection::files_folder(0);
    app.expand_or_step_in();
    assert_eq!(app.selection, Selection::file(0, 0));
}

#[test]
fn expand_step_in_on_wave_with_no_files_steps_into_first_recipe() {
    let g = graph_2x2();
    let mut app = AppState::new(&g);
    // Wave 0 expanded by default, no files.
    app.expand_or_step_in();
    assert_eq!(app.selection, Selection::recipe(0, 0));
}
```

- [ ] **Step 2: Run tests to verify they fail**

```
cargo test -p cook-dag-viewer --lib expand_step_in_on
```

Expected: FAIL — current behavior auto-toggles `files_expanded` instead of stepping selection.

- [ ] **Step 3: Update `expand_or_step_in`**

Replace the body in `cli/crates/cook-dag-viewer/src/state.rs` (currently lines 593-617):

```rust
pub fn expand_or_step_in(&mut self) {
    let wi = self.selection.wave;
    match self.selection.leaf {
        None => {
            let Some(w) = self.tree.waves.get_mut(wi) else { return };
            if !w.expanded {
                w.expanded = true;
                return;
            }
            if !w.files.is_empty() {
                self.selection = Selection::files_folder(wi);
                return;
            }
            if !w.recipes.is_empty() {
                self.selection = Selection::recipe(wi, 0);
            }
        }
        Some(SelectionLeaf::FilesFolder) => {
            let Some(w) = self.tree.waves.get_mut(wi) else { return };
            if !w.files_expanded {
                w.files_expanded = true;
                return;
            }
            if !w.files.is_empty() {
                self.selection = Selection::file(wi, 0);
            }
        }
        Some(SelectionLeaf::Recipe { recipe, unit: None }) => {
            if let Some(w) = self.tree.waves.get_mut(wi) {
                if let Some(r) = w.recipes.get_mut(recipe) {
                    if !r.expanded {
                        r.expanded = true;
                        return;
                    }
                    if !r.units.is_empty() {
                        self.selection = Selection::unit(wi, recipe, 0);
                    }
                }
            }
        }
        Some(SelectionLeaf::Recipe { unit: Some(_), .. }) | Some(SelectionLeaf::File(_)) => {
            // Already at a leaf row.
        }
    }
}
```

- [ ] **Step 4: Update the now-stale test**

Find the existing test `expand_step_in_on_wave_opens_files_folder_when_already_expanded` in `cli/crates/cook-dag-viewer/src/state.rs` and delete it — its expected behavior (`l` on an expanded wave auto-toggles `files_expanded`) is gone. The four new tests in Step 1 cover the replacement behavior.

- [ ] **Step 5: Run tests to verify they pass**

```
cargo test -p cook-dag-viewer --lib state::
```

Expected: all green. The existing `expand_then_step_in_descends_into_units` test still passes (it tests the recipe-row descent, which is now unified in the new `expand_or_step_in`).

- [ ] **Step 6: Commit**

```
git add cli/crates/cook-dag-viewer/src/state.rs
git commit -m "feat(cook-dag-viewer): l steps through wave → folder → recipe → unit tiers"
```

---

## Task 4: Update `collapse_or_step_out` semantics

**Files:**
- Modify: `cli/crates/cook-dag-viewer/src/state.rs:564-591`

- [ ] **Step 1: Write the failing tests**

Append to the state test module:

```rust
#[test]
fn collapse_step_out_on_file_returns_to_folder_row() {
    let g = graph_with_files();
    let mut app = AppState::new(&g);
    app.tree.waves[0].files_expanded = true;
    app.selection = Selection::file(0, 0);
    app.collapse_or_step_out();
    assert_eq!(app.selection, Selection::files_folder(0));
    // Folder stays expanded; we only step the cursor up one level.
    assert!(app.tree.waves[0].files_expanded);
}

#[test]
fn collapse_step_out_on_files_folder_expanded_collapses_folder() {
    let g = graph_with_files();
    let mut app = AppState::new(&g);
    app.tree.waves[0].files_expanded = true;
    app.selection = Selection::files_folder(0);
    app.collapse_or_step_out();
    assert!(!app.tree.waves[0].files_expanded);
    // Selection stays on the folder row after collapse.
    assert_eq!(app.selection, Selection::files_folder(0));
}

#[test]
fn collapse_step_out_on_files_folder_collapsed_returns_to_wave() {
    let g = graph_with_files();
    let mut app = AppState::new(&g);
    app.selection = Selection::files_folder(0);
    assert!(!app.tree.waves[0].files_expanded);
    app.collapse_or_step_out();
    assert_eq!(app.selection, Selection::wave_only(0));
}
```

- [ ] **Step 2: Run tests to verify they fail**

```
cargo test -p cook-dag-viewer --lib collapse_step_out_on_file_returns_to_folder_row
```

Expected: FAIL — current behavior steps a file selection back to `wave_only` and forces `files_expanded = false`.

- [ ] **Step 3: Update `collapse_or_step_out`**

Replace the body in `cli/crates/cook-dag-viewer/src/state.rs` (currently lines 564-591):

```rust
pub fn collapse_or_step_out(&mut self) {
    let wi = self.selection.wave;
    match self.selection.leaf {
        Some(SelectionLeaf::Recipe { recipe, unit: Some(_) }) => {
            self.selection.leaf = Some(SelectionLeaf::Recipe { recipe, unit: None });
        }
        Some(SelectionLeaf::Recipe { recipe, unit: None }) => {
            let collapsed = if let Some(w) = self.tree.waves.get_mut(wi) {
                if let Some(r) = w.recipes.get_mut(recipe) {
                    if r.expanded {
                        r.expanded = false;
                        true
                    } else {
                        false
                    }
                } else {
                    false
                }
            } else {
                false
            };
            if !collapsed {
                self.selection.leaf = None;
            }
        }
        Some(SelectionLeaf::File(_)) => {
            self.selection = Selection::files_folder(wi);
        }
        Some(SelectionLeaf::FilesFolder) => {
            let collapsed = if let Some(w) = self.tree.waves.get_mut(wi) {
                if w.files_expanded {
                    w.files_expanded = false;
                    true
                } else {
                    false
                }
            } else {
                false
            };
            if !collapsed {
                self.selection.leaf = None;
            }
        }
        None => {
            if let Some(w) = self.tree.waves.get_mut(wi) {
                w.expanded = false;
            }
        }
    }
}
```

- [ ] **Step 4: Update the now-stale test**

Find the existing test `collapse_step_out_on_file_collapses_folder_and_returns_to_wave` in `cli/crates/cook-dag-viewer/src/state.rs` and delete it — its expected behavior is gone. The three new tests in Step 1 cover the replacement.

Also add this regression test to confirm the recipe-row collapse path still works (the rewrite changed how the wave-row fallback fires):

```rust
#[test]
fn collapse_step_out_on_collapsed_recipe_row_returns_to_wave() {
    let g = graph_2x2();
    let mut app = AppState::new(&g);
    app.selection = Selection::recipe(0, 0);
    // Recipe is collapsed by default.
    app.collapse_or_step_out();
    assert_eq!(app.selection, Selection::wave_only(0));
}
```

- [ ] **Step 5: Run tests to verify they pass**

```
cargo test -p cook-dag-viewer --lib state::
```

Expected: all green.

- [ ] **Step 6: Commit**

```
git add cli/crates/cook-dag-viewer/src/state.rs
git commit -m "feat(cook-dag-viewer): h steps file → folder → wave with explicit collapse pass"
```

---

## Task 5: Files folder header gets REVERSED highlight when selected

**Files:**
- Modify: `cli/crates/cook-dag-viewer/src/render/index.rs:36-42`

- [ ] **Step 1: Write the failing test**

Append to the test module in `cli/crates/cook-dag-viewer/src/render/index.rs`:

```rust
#[test]
fn files_folder_header_is_reversed_when_selected() {
    let g = graph_with_files();
    let mut app = AppState::new(&g);
    app.selection = Selection::files_folder(0);
    let frame = SnapshotFrame::new(g);
    let area = Rect::new(0, 0, 28, 6);
    let mut buf = Buffer::empty(area);
    render(area, &mut buf, &app, &frame);

    // Row 1 = Files folder header. The first non-blank cell (the ▶/▼ glyph
    // at indent 2) must carry REVERSED.
    let cell = buf.cell((2, 1)).unwrap();
    assert!(
        cell.style().add_modifier.contains(Modifier::REVERSED),
        "expected folder-header glyph to be REVERSED when selected"
    );
}

#[test]
fn files_folder_header_is_not_reversed_when_unselected() {
    let g = graph_with_files();
    let app = AppState::new(&g); // default selection = wave_only(0)
    let frame = SnapshotFrame::new(g);
    let area = Rect::new(0, 0, 28, 6);
    let mut buf = Buffer::empty(area);
    render(area, &mut buf, &app, &frame);

    let cell = buf.cell((2, 1)).unwrap();
    assert!(!cell.style().add_modifier.contains(Modifier::REVERSED));
}
```

The test module already imports `crate::state::Selection`. Add `use crate::state::SelectionLeaf;` at the top of the test module if it isn't already imported. Also import `Modifier`: `use ratatui::style::Modifier;`.

- [ ] **Step 2: Run tests to verify they fail**

```
cargo test -p cook-dag-viewer files_folder_header_is_reversed_when_selected
```

Expected: FAIL — current code passes `Style::default()` to the folder-header `write_line`.

- [ ] **Step 3: Highlight the folder header when selected**

In `cli/crates/cook-dag-viewer/src/render/index.rs`, find the folder-header rendering (currently lines 36-42):

```rust
        // Files folder (rendered only when the wave has any files).
        if !wave.files.is_empty() {
            if row >= area.y + area.height {
                break 'outer;
            }
            let glyph = if wave.files_expanded { '▼' } else { '▶' };
            let line = format!("{} Files ({})", glyph, wave.files.len());
            write_line(area, buf, row, 2, &line, Style::default());
            row += 1;
```

Change the `Style::default()` argument to use `sel_style`:

```rust
        // Files folder (rendered only when the wave has any files).
        if !wave.files.is_empty() {
            if row >= area.y + area.height {
                break 'outer;
            }
            let glyph = if wave.files_expanded { '▼' } else { '▶' };
            let line = format!("{} Files ({})", glyph, wave.files.len());
            let style = sel_style(app.selection, Selection::files_folder(wi));
            write_line(area, buf, row, 2, &line, style);
            row += 1;
```

- [ ] **Step 4: Run tests to verify they pass**

```
cargo test -p cook-dag-viewer --lib render::index::
```

Expected: all green. The existing `files_folder_header_renders_with_count` and `files_folder_expanded_lists_files_alphabetical` tests continue to pass — they only assert on text content, not style.

- [ ] **Step 5: Commit**

```
git add cli/crates/cook-dag-viewer/src/render/index.rs
git commit -m "feat(cook-dag-viewer): files folder header highlights when selected"
```

---

## Task 6: `focus_subgraph` treats `FilesFolder` as wave-level

**Files:**
- Modify: `cli/crates/cook-dag-viewer/src/render/focus.rs:18-94`

- [ ] **Step 1: Write the failing test**

Append to the test module in `cli/crates/cook-dag-viewer/src/render/focus.rs`. If the file has no test module, add one — first read the file to see its current end:

```
cat cli/crates/cook-dag-viewer/src/render/focus.rs | tail -40
```

Add (in the test module — create one with `#[cfg(test)] mod tests { use super::*; ... }` if none exists):

```rust
#[test]
fn focus_subgraph_for_files_folder_matches_wave_only() {
    use crate::dag_data::{EdgeData, NodeData, WaveData, WaveDagData};
    use crate::state::{AppState, Selection};

    let g = WaveDagData {
        schema_version: crate::VIEWER_SCHEMA_VERSION,
        target: "build".into(),
        waves: vec![WaveData {
            recipes: vec!["a".into()],
            nodes: vec![
                NodeData {
                    id: "file:foo.h".into(),
                    kind: "file".into(),
                    label: "foo.h".into(),
                    recipe: None,
                    command: None,
                    output: None,
                    cached: None,
                    dep_kind: None,
                    group_index: None,
                    modified: Some(false),
                    discovered: None,
                },
                NodeData {
                    id: "unit:a:0".into(),
                    kind: "unit".into(),
                    label: "a0".into(),
                    recipe: Some("a".into()),
                    command: Some("c".into()),
                    output: None,
                    cached: Some(true),
                    dep_kind: Some("sequential".into()),
                    group_index: None,
                    modified: None,
                    discovered: None,
                },
            ],
            edges: vec![EdgeData {
                from: "file:foo.h".into(),
                to: "unit:a:0".into(),
            }],
        }],
        inter_wave_edges: vec![],
    };

    let mut wave_app = AppState::new(&g);
    wave_app.selection = Selection::wave_only(0);
    let wave_sub = focus_subgraph(&g, &wave_app);

    let mut folder_app = AppState::new(&g);
    folder_app.selection = Selection::files_folder(0);
    let folder_sub = focus_subgraph(&g, &folder_app);

    assert_eq!(wave_sub, folder_sub);
}
```

The test requires `WaveDagData` to derive `PartialEq`. Verify with:

```
grep -n "derive(.*PartialEq.*).*\nstruct WaveDagData\|struct WaveDagData" cli/crates/cook-dag-viewer/src/dag_data.rs
```

If `WaveDagData` does not derive `PartialEq`, replace the final `assert_eq!` with a structural comparison instead:

```rust
assert_eq!(wave_sub.waves.len(), folder_sub.waves.len());
assert_eq!(wave_sub.waves[0].nodes, folder_sub.waves[0].nodes);
assert_eq!(wave_sub.waves[0].edges, folder_sub.waves[0].edges);
assert_eq!(wave_sub.inter_wave_edges, folder_sub.inter_wave_edges);
```

- [ ] **Step 2: Run test to verify it fails**

```
cargo test -p cook-dag-viewer focus_subgraph_for_files_folder_matches_wave_only
```

Expected: FAIL or compile error — `focus_set` does not yet handle `SelectionLeaf::FilesFolder`. The Rust compiler will reject the non-exhaustive match.

- [ ] **Step 3: Add the `FilesFolder` arm to `focus_set` and `expand_one_hop`**

In `cli/crates/cook-dag-viewer/src/render/focus.rs`, update the match in `focus_set` (currently lines 24-61). Add a `FilesFolder` arm that mirrors the `None` arm:

```rust
match app.selection.leaf {
    None | Some(SelectionLeaf::FilesFolder) => {
        for n in &wave.nodes {
            out.insert(n.id.clone());
        }
    }
    Some(SelectionLeaf::Recipe { recipe, unit }) => {
        // ... unchanged ...
    }
    Some(SelectionLeaf::File(fi)) => {
        // ... unchanged ...
    }
}
```

Update `expand_one_hop` (currently lines 65-94). Change the early-return guard so `FilesFolder` also short-circuits (no 1-hop expansion — wave-level focus shows the entire wave on its own):

```rust
fn expand_one_hop(
    graph: &WaveDagData,
    focus: &BTreeSet<String>,
    app: &AppState,
) -> BTreeSet<String> {
    let mut visible = focus.clone();
    // Wave-level and files-folder focus do not expand: they already
    // include every node in the wave.
    if matches!(
        app.selection.leaf,
        None | Some(SelectionLeaf::FilesFolder)
    ) {
        return visible;
    }
    // ... rest unchanged ...
}
```

- [ ] **Step 4: Run test to verify it passes**

```
cargo test -p cook-dag-viewer --lib render::focus::
```

Expected: all green.

- [ ] **Step 5: Commit**

```
git add cli/crates/cook-dag-viewer/src/render/focus.rs
git commit -m "feat(cook-dag-viewer): files-folder selection focuses on whole wave"
```

---

## Task 7: `bulk_pin_recipe` no-ops on `FilesFolder` selection

**Files:**
- Modify: `cli/crates/cook-dag-viewer/src/state.rs:412-467`

- [ ] **Step 1: Write the failing test**

Append to the state test module:

```rust
#[test]
fn bulk_pin_recipe_on_files_folder_selection_emits_on_file() {
    let g = graph_with_files();
    let mut app = AppState::new(&g);
    app.selection = Selection::files_folder(0);
    app.bulk_pin_recipe(&g);
    assert_eq!(app.last_pin_message, Some(PinMsg::OnFile));
    assert!(app.pins.is_empty());
}
```

- [ ] **Step 2: Run test to verify behavior**

```
cargo test -p cook-dag-viewer bulk_pin_recipe_on_files_folder_selection_emits_on_file
```

The current `bulk_pin_recipe` calls `self.selection.node_id(&self.tree)` which now returns `None` for `FilesFolder`, so it already emits `PinMsg::OnFile` via the existing early-return. The test should PASS without code change. If it fails, inspect the existing early-return logic in `bulk_pin_recipe` (lines 413-416):

```rust
let Some(selected_id) = self.selection.node_id(&self.tree) else {
    self.last_pin_message = Some(PinMsg::OnFile);
    return;
};
```

That branch already handles `FilesFolder` correctly. No code change needed — the test is a regression guard.

- [ ] **Step 3: Commit (test only)**

```
git add cli/crates/cook-dag-viewer/src/state.rs
git commit -m "test(cook-dag-viewer): bulk-pin no-ops on files folder selection"
```

---

## Task 8: Switch `pick_layout` to single always-focused path

**Files:**
- Modify: `cli/crates/cook-dag-viewer/src/render/mod.rs:18-33`

- [ ] **Step 1: Write the failing test**

Append to `cli/crates/cook-dag-viewer/src/render/mod.rs`. If the file has no test module, add one:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::dag_data::{EdgeData, NodeData, WaveData, WaveDagData};
    use crate::state::{AppState, Selection};

    fn unit_node(id: &str, recipe: &str, label: &str) -> NodeData {
        NodeData {
            id: id.into(),
            kind: "unit".into(),
            label: label.into(),
            recipe: Some(recipe.into()),
            command: Some("c".into()),
            output: None,
            cached: Some(true),
            dep_kind: Some("sequential".into()),
            group_index: None,
            modified: None,
            discovered: None,
        }
    }

    fn three_unit_chain() -> WaveDagData {
        WaveDagData {
            schema_version: crate::VIEWER_SCHEMA_VERSION,
            target: "build".into(),
            waves: vec![WaveData {
                recipes: vec!["a".into()],
                nodes: vec![
                    unit_node("unit:a:0", "a", "a0"),
                    unit_node("unit:a:1", "a", "a1"),
                    unit_node("unit:a:2", "a", "a2"),
                ],
                edges: vec![
                    EdgeData { from: "unit:a:0".into(), to: "unit:a:1".into() },
                    EdgeData { from: "unit:a:1".into(), to: "unit:a:2".into() },
                ],
            }],
            inter_wave_edges: vec![],
        }
    }

    #[test]
    fn pick_layout_always_runs_focus_filter() {
        let g = three_unit_chain();
        let mut app = AppState::new(&g);
        app.tree.waves[0].recipes[0].expanded = true;
        // Select the middle unit. Focus = a0, a1, a2 (a1 + 1-hop).
        app.selection = Selection::unit(0, 0, 1);
        let layout = pick_layout(&app, &g);
        let ids: std::collections::BTreeSet<&str> =
            layout.nodes.iter().map(|n| n.id.as_str()).collect();
        assert_eq!(
            ids,
            std::collections::BTreeSet::from(["unit:a:0", "unit:a:1", "unit:a:2"])
        );

        // Now select an end unit — focus drops the far one.
        app.selection = Selection::unit(0, 0, 0);
        let layout = pick_layout(&app, &g);
        let ids: std::collections::BTreeSet<&str> =
            layout.nodes.iter().map(|n| n.id.as_str()).collect();
        assert_eq!(
            ids,
            std::collections::BTreeSet::from(["unit:a:0", "unit:a:1"])
        );
    }
}
```

- [ ] **Step 2: Run test to verify behavior**

```
cargo test -p cook-dag-viewer pick_layout_always_runs_focus_filter
```

Today this PASSES because `AppState::new` defaults small graphs to `DensityMode::Full` — the assertion would actually fail currently because Full does NOT filter. Re-run to confirm it fails as written:

Expected: FAIL — `pick_layout` returns the full graph in Full mode.

- [ ] **Step 3: Collapse `pick_layout` to single path**

Replace `pick_layout` in `cli/crates/cook-dag-viewer/src/render/mod.rs` (currently lines 23-33). Also remove the `DensityMode` import on line 18:

```rust
use crate::state::{AppState, Mode};
```

```rust
/// Build the focused subgraph for the current selection and lay it out
/// with the boxed-node geometry. There is only one render path —
/// every render filters to the selection's neighborhood.
pub fn pick_layout(app: &AppState, graph: &WaveDagData) -> layout::Layout {
    let focused = focus::focus_subgraph(graph, app);
    layout::compute(&focused, layout::LayoutDims::FULL)
}
```

- [ ] **Step 4: Run test to verify it passes**

```
cargo test -p cook-dag-viewer --lib render::
```

Expected: the new test PASSES. The existing rendering tests in `render/canvas.rs` may break because they call `layout::compute(&g, layout::LayoutDims::COMPACT)` directly — those will be cleaned up in Task 9. Do not touch them yet.

If `cargo test -p cook-dag-viewer --lib` fails *only* on the canvas compact-mode tests (`compact_mode_renders_bracketed_label_in_one_row`, `compact_mode_label_inherits_cache_status_color`), that's expected — they're being deleted in Task 9.

- [ ] **Step 5: Commit**

```
git add cli/crates/cook-dag-viewer/src/render/mod.rs
git commit -m "feat(cook-dag-viewer): pick_layout always applies the selection focus filter"
```

---

## Task 9: Strip density dispatch from canvas + delete `draw_compact`

**Files:**
- Modify: `cli/crates/cook-dag-viewer/src/render/canvas.rs:14-31, 90-140, 325-362`

- [ ] **Step 1: Simplify `render`**

Replace the `render` function in `cli/crates/cook-dag-viewer/src/render/canvas.rs` (currently lines 14-31):

```rust
pub fn render<F: ViewFrame>(layout: &Layout, app: &AppState, frame: &F) -> Buffer {
    let area = Rect::new(0, 0, layout.canvas_w.max(1), layout.canvas_h.max(1));
    let mut buf = Buffer::empty(area);

    draw_edges(layout, area, &mut buf, &app.theme);
    draw_nodes(layout, area, &mut buf);
    overlay_badges(layout, frame, &mut buf, &app.theme);
    overlay_selection(layout, app, &mut buf);
    buf
}
```

- [ ] **Step 2: Delete `draw_compact` and its helper**

Delete the entire `draw_compact` function (currently lines 90-126) and the `truncate_to` helper (currently lines 128-140). Also delete the now-unused imports if any are exclusive to those functions:

- `Style` and `Modifier` are still used by `overlay_selection` — keep them.
- `status_color` is still used by `draw_compact` only — search to confirm (`grep -n status_color cli/crates/cook-dag-viewer/src/render/canvas.rs`). If `draw_compact` is the only caller, delete `status_color` too.

- [ ] **Step 3: Delete the two compact-mode unit tests**

In the same file, delete:
- `compact_mode_renders_bracketed_label_in_one_row` (currently lines 325-343)
- `compact_mode_label_inherits_cache_status_color` (currently lines 345-362)

- [ ] **Step 4: Run the canvas test suite**

```
cargo test -p cook-dag-viewer --lib render::canvas::
```

Expected: all green. If you see "unused import" warnings for `Style` or `Modifier`, address them by removing the unused names.

- [ ] **Step 5: Commit**

```
git add cli/crates/cook-dag-viewer/src/render/canvas.rs
git commit -m "refactor(cook-dag-viewer): canvas runs single boxed-node draw path"
```

---

## Task 10: Remove the `m` keybinding and bottom-bar hint

**Files:**
- Modify: `cli/crates/cook-dag-viewer/src/input.rs:103-105`
- Modify: `cli/crates/cook-dag-viewer/src/render/mod.rs:114-115`

- [ ] **Step 1: Delete the `m` handler**

In `cli/crates/cook-dag-viewer/src/input.rs`, delete the match arm (currently lines 103-105):

```rust
        (KeyCode::Char('m'), KeyModifiers::NONE) => {
            app.density = app.density.next();
        }
```

- [ ] **Step 2: Strip ` m mode` from the bottom-bar hint**

In `cli/crates/cook-dag-viewer/src/render/mod.rs`, find the `Mode::Normal` hint string (currently line 115):

```rust
" ? help · / · q · [/] · HJKL · m mode · p pin · 1-9 jump · X clear".to_string()
```

Change to:

```rust
" ? help · / · q · [/] · HJKL · p pin · 1-9 jump · X clear".to_string()
```

- [ ] **Step 3: Verify the binary still compiles**

```
cargo check -p cook-dag-viewer
```

Expected: clean compile. There may still be references to `app.density` elsewhere (state.rs). Those are removed in Task 11.

- [ ] **Step 4: Commit**

```
git add cli/crates/cook-dag-viewer/src/input.rs cli/crates/cook-dag-viewer/src/render/mod.rs
git commit -m "refactor(cook-dag-viewer): drop m keybinding for the removed density toggle"
```

---

## Task 11: Delete `DensityMode`, `choose_initial_mode`, `AppState.density`

**Files:**
- Modify: `cli/crates/cook-dag-viewer/src/state.rs`

- [ ] **Step 1: Delete the field and constructor wiring**

In `cli/crates/cook-dag-viewer/src/state.rs`:

- Remove the `pub density: DensityMode,` field from `AppState` (currently line 327).
- Remove the `density: choose_initial_mode(graph),` initializer from `AppState::new` (currently line 347).

- [ ] **Step 2: Delete the type and helper**

In the same file:

- Delete the `DensityMode` enum and its `impl` block (currently lines 5-19).
- Delete the `choose_initial_mode` function (currently lines 119-123).

- [ ] **Step 3: Delete the now-stale tests**

Delete these tests from the test module in `cli/crates/cook-dag-viewer/src/state.rs`:

- `density_mode_cycles_full_compact_full`
- `choose_initial_mode_picks_full_for_small_graphs`
- `choose_initial_mode_picks_compact_in_middle_band`
- `choose_initial_mode_picks_compact_for_big_graphs`
- `app_state_starts_with_density_chosen_from_node_count`

The `small_graph` helper used by those tests may have other callers. Run `grep -n "small_graph(" cli/crates/cook-dag-viewer/src/state.rs` — if no other callers, delete the helper too. If other tests use it, keep the helper.

- [ ] **Step 4: Run the lib test suite**

```
cargo test -p cook-dag-viewer --lib
```

Expected: clean compile, all green.

- [ ] **Step 5: Commit**

```
git add cli/crates/cook-dag-viewer/src/state.rs
git commit -m "refactor(cook-dag-viewer): remove DensityMode and density state"
```

---

## Task 12: Audit and remove `LayoutDims::COMPACT`

**Files:**
- Modify: `cli/crates/cook-dag-viewer/src/render/layout.rs:29-42`

- [ ] **Step 1: Audit remaining callers**

```
grep -rn "LayoutDims::COMPACT" cli/crates/cook-dag-viewer
```

After Tasks 8-11, only test callers should remain. Tests in `render/canvas.rs` were cleaned up in Task 9. The remaining caller is in `tests/tui_integration.rs` — that test will be deleted in Task 13.

- [ ] **Step 2: Delete `LayoutDims::COMPACT`**

In `cli/crates/cook-dag-viewer/src/render/layout.rs`, change the `impl LayoutDims` block (currently lines 39-42):

```rust
impl LayoutDims {
    pub const FULL: Self = Self { layer_width: 32, node_w: 22, node_h: 3, row_pad: 1 };
}
```

Also update the doc comment on `LayoutDims` (currently lines 28-30) so it stops referring to two presets:

```rust
/// Geometry preset for the layered layout. The renderer always uses
/// `LayoutDims::FULL`; the type is kept as a struct so the layout
/// engine remains parameterised on its dimensions for future presets.
```

- [ ] **Step 3: Verify**

```
cargo check -p cook-dag-viewer --tests
```

Expected: one or more errors in `tests/tui_integration.rs` (still references `LayoutDims::COMPACT`) and `tests/compact_snapshots.rs` (whole file references compact). Those are addressed in Tasks 13-14. Run lib-only to verify the source change is clean:

```
cargo check -p cook-dag-viewer --lib
```

Expected: clean.

- [ ] **Step 4: Commit**

```
git add cli/crates/cook-dag-viewer/src/render/layout.rs
git commit -m "refactor(cook-dag-viewer): drop LayoutDims::COMPACT preset"
```

---

## Task 13: Update `tests/tui_integration.rs`

**Files:**
- Modify: `cli/crates/cook-dag-viewer/tests/tui_integration.rs:96-115, 342-388, 388-420`

- [ ] **Step 1: Delete `m_cycles_density_mode`**

In `cli/crates/cook-dag-viewer/tests/tui_integration.rs`, delete the test `m_cycles_density_mode` (currently around lines 96-115). It tests the removed `m` keybinding behavior.

- [ ] **Step 2: Delete `compact_mode_layout_filters_to_one_hop_for_unit_selection`**

Delete the test starting at line 342 (`fn compact_mode_layout_filters_to_one_hop_for_unit_selection`) and continuing to its closing brace. It is replaced by `pick_layout_always_runs_focus_filter` from Task 8.

- [ ] **Step 3: Delete the comparison test that uses `LayoutDims::COMPACT`**

Find the test at line 388 (`use cook_dag_viewer::state::DensityMode;` is its first line) and delete the entire `#[test] fn ...` block. It compares Full and Compact rendering paths that no longer both exist.

- [ ] **Step 4: Verify and clean unused imports**

```
cargo test -p cook-dag-viewer --test tui_integration
```

Expected: clean compile, all remaining tests green. If `use cook_dag_viewer::state::DensityMode;` or `use cook_dag_viewer::render::layout::LayoutDims::COMPACT;` remain at file/module scope, remove them.

- [ ] **Step 5: Commit**

```
git add cli/crates/cook-dag-viewer/tests/tui_integration.rs
git commit -m "test(cook-dag-viewer): drop integration tests for the removed compact mode"
```

---

## Task 14: Delete `compact_snapshots.rs`

**Files:**
- Delete: `cli/crates/cook-dag-viewer/tests/compact_snapshots.rs`

- [ ] **Step 1: Inspect the file**

```
head -30 cli/crates/cook-dag-viewer/tests/compact_snapshots.rs
```

Confirm the file is a top-level integration test (`#[test]` functions at file scope, not nested inside a module that asserts both modes). It is — it was added by commit 9eb015a specifically to snapshot Compact-mode canvas output.

- [ ] **Step 2: Delete the file**

```
git rm cli/crates/cook-dag-viewer/tests/compact_snapshots.rs
```

- [ ] **Step 3: Run the integration test suite**

```
cargo test -p cook-dag-viewer --tests
```

Expected: clean compile, all green.

- [ ] **Step 4: Commit**

```
git commit -m "test(cook-dag-viewer): drop compact-mode canvas snapshot suite"
```

---

## Task 15: Final verification — formatter, clippy, full test run

**Files:** none modified directly; this is a verification gate.

- [ ] **Step 1: Format**

```
cargo fmt -p cook-dag-viewer
```

Expected: no output (no diff). If `cargo fmt` reformats anything, inspect with `git diff` and commit any changes:

```
git add cli/crates/cook-dag-viewer
git commit -m "style(cook-dag-viewer): cargo fmt"
```

- [ ] **Step 2: Clippy**

```
cargo clippy -p cook-dag-viewer --all-targets -- -D warnings
```

Expected: clean. Common likely warnings after this refactor:
- Unused imports in `state.rs`, `render/canvas.rs`, `render/mod.rs`, `tests/tui_integration.rs` — remove the offending `use` lines.
- `dead_code` for any helper that only the deleted `draw_compact` called — delete those helpers.

Re-run until clean. Commit any cleanups separately:

```
git add cli/crates/cook-dag-viewer
git commit -m "style(cook-dag-viewer): clippy cleanup after density-mode removal"
```

- [ ] **Step 3: Full crate test run**

```
cargo test -p cook-dag-viewer
```

Expected: all green. Confirm the test counts in the summary line — there should be substantially fewer integration tests than before (compact_snapshots removed, three tests removed from tui_integration).

- [ ] **Step 4: Workspace test run (smoke check the wider blast radius)**

```
cargo test --workspace
```

Expected: all green. Failures outside `cook-dag-viewer` indicate a missed external caller of `DensityMode`, `choose_initial_mode`, `LayoutDims::COMPACT`, or `AppState.density`. Search for the symbol and either remove the caller or restore the symbol if it has a real consumer outside the viewer (none should — these are crate-private usage patterns).

- [ ] **Step 5: Manual smoke test (optional but recommended)**

The viewer is interactive. Open a Cookfile that exercises file dependencies (e.g. `cli/Cookfile`) and run:

```
cargo run -p cook -- dag <target>
```

Walk through:
1. Press `j` repeatedly from the top — the cursor must land on the `Files (N)` folder header (not skip it).
2. With the folder selected, press `l` — folder expands.
3. Press `l` again — selection moves to the first file row.
4. Press `h` — selection returns to the folder row.
5. Select a unit — canvas filters to that unit + 1-hop.
6. Press `m` — should be a no-op (no panic, no visual change).
7. Confirm the bottom hint bar no longer mentions `m mode`.

This is a developer-facing TUI; visual verification matters.

- [ ] **Step 6: Final commit if Step 5 surfaces fixes**

If the manual smoke test surfaces issues, fix them with focused commits referencing the specific behavior. If clean, no commit needed — the verification step is itself the deliverable.

---

## Self-Review Notes

- **Spec coverage:** Every section of the spec is mapped:
  - §3.1 `pick_layout` collapse → Task 8
  - §3.2 `canvas::render` simplify + `draw_compact` deletion → Task 9
  - §3.3 removed-surface table → Tasks 10–12 (input/hint, state, layout)
  - §4.1 `SelectionLeaf::FilesFolder` + constructor + `node_id` → Task 1
  - §4.2 `visible_rows` ordering → Task 2
  - §4.3 `expand_or_step_in` table → Task 3
  - §4.3 `collapse_or_step_out` table → Task 4
  - §4.4 `focus_set` + `expand_one_hop` arms → Task 6
  - §4.5 index renderer highlight → Task 5
  - §4.6 `bulk_pin_recipe` regression guard → Task 7
  - §5 test list → folded into each implementing task
  - §5.6 snapshot deletions → Tasks 13–14
  - §6 migration (no schema change, hook stays clean) — no task needed; spec-first hook is a no-op for non-`standard/` paths

- **Rollback story:** Every task is a single commit. If a downstream task fails, the prior commits leave the code in a working state (the new behavior is additive through Task 7; the cut-over starts at Task 8).
