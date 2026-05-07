//! `AppState` + `IndexTree`. See design spec §Index, §Camera.

use crate::dag_data::WaveDagData;

pub const PIN_SLOTS: usize = 9;

/// Up to 9 pinned node IDs, indexed by slot. Slot N holds the node ID
/// pinned in that slot; `None` is an empty slot. See spec §4.3.
#[derive(Debug, Clone)]
pub struct PinState {
    slots: [Option<String>; PIN_SLOTS],
}

impl Default for PinState {
    fn default() -> Self {
        Self {
            slots: std::array::from_fn(|_| None),
        }
    }
}

impl PinState {
    /// Pin `node_id` to the lowest empty slot. Returns the slot index
    /// (0-indexed). Idempotent: re-pinning an already-pinned node
    /// returns its existing slot. Returns `None` if all slots are full.
    pub fn pin(&mut self, node_id: &str) -> Option<usize> {
        if let Some(existing) = self.slot_of(node_id) {
            return Some(existing);
        }
        for (i, slot) in self.slots.iter_mut().enumerate() {
            if slot.is_none() {
                *slot = Some(node_id.to_string());
                return Some(i);
            }
        }
        None
    }

    /// Unpin `node_id`. Returns `true` if it was pinned.
    pub fn unpin(&mut self, node_id: &str) -> bool {
        for slot in self.slots.iter_mut() {
            if slot.as_deref() == Some(node_id) {
                *slot = None;
                return true;
            }
        }
        false
    }

    pub fn slot_of(&self, node_id: &str) -> Option<usize> {
        self.slots
            .iter()
            .position(|s| s.as_deref() == Some(node_id))
    }

    pub fn id_at(&self, slot: usize) -> Option<&str> {
        self.slots.get(slot).and_then(|s| s.as_deref())
    }

    pub fn iter(&self) -> impl Iterator<Item = (usize, &str)> {
        self.slots
            .iter()
            .enumerate()
            .filter_map(|(i, s)| s.as_deref().map(|id| (i, id)))
    }

    pub fn clear(&mut self) {
        for slot in self.slots.iter_mut() {
            *slot = None;
        }
    }

    pub fn is_empty(&self) -> bool {
        self.slots.iter().all(|s| s.is_none())
    }

    pub fn is_full(&self) -> bool {
        self.slots.iter().all(|s| s.is_some())
    }
}

/// One-shot footer messages from pin actions. The bottom hint bar
/// shows the message for the next render frame, then clears it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PinMsg {
    Full,
    OnFile,
    EmptySlot(usize),
    ClearedAll(usize),
}

impl PinMsg {
    pub fn render(self) -> String {
        match self {
            Self::Full => "pin slots full — clear with X".to_string(),
            Self::OnFile => "bulk-pin needs a unit selection".to_string(),
            Self::EmptySlot(n) => format!("slot {} empty", n + 1),
            Self::ClearedAll(n) => format!("cleared {n} pin{}", if n == 1 { "" } else { "s" }),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnitRow {
    pub node_id: String,
    pub label: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecipeRow {
    pub name: String,
    pub units: Vec<UnitRow>,
    pub expanded: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileRow {
    pub node_id: String,
    pub label: String,
    /// Mirrors `NodeData.discovered == Some(true)`: file came from a
    /// depfile rather than from any unit's `meta.input_paths`.
    pub discovered: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WaveRow {
    pub label: String,
    pub files: Vec<FileRow>,
    pub recipes: Vec<RecipeRow>,
    pub expanded: bool,
    pub files_expanded: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexTree {
    pub waves: Vec<WaveRow>,
}

impl IndexTree {
    pub fn from_graph(g: &WaveDagData) -> Self {
        let mut waves = Vec::with_capacity(g.waves.len());
        for (wi, wave) in g.waves.iter().enumerate() {
            let mut recipes: Vec<RecipeRow> = wave
                .recipes
                .iter()
                .map(|name| RecipeRow {
                    name: name.clone(),
                    units: Vec::new(),
                    expanded: false,
                })
                .collect();

            let mut files: Vec<FileRow> = Vec::new();

            for n in &wave.nodes {
                match n.kind.as_str() {
                    "unit" => {
                        let Some(recipe) = n.recipe.as_deref() else {
                            continue;
                        };
                        let Some(row) = recipes.iter_mut().find(|r| r.name == recipe) else {
                            continue;
                        };
                        row.units.push(UnitRow {
                            node_id: n.id.clone(),
                            label: n.label.clone(),
                        });
                    }
                    "file" => {
                        files.push(FileRow {
                            node_id: n.id.clone(),
                            label: n.label.clone(),
                            discovered: n.discovered == Some(true),
                        });
                    }
                    _ => {}
                }
            }

            files.sort_by(|a, b| a.label.cmp(&b.label));

            waves.push(WaveRow {
                label: format!("Wave {} ({} recipes)", wi, recipes.len()),
                files,
                recipes,
                expanded: wi == 0,
                files_expanded: false,
            });
        }
        Self { waves }
    }
}

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Selection {
    pub wave: usize,
    pub leaf: Option<SelectionLeaf>,
}

impl Selection {
    pub fn first() -> Self {
        Self { wave: 0, leaf: None }
    }

    /// Wave-level selection (no leaf).
    pub fn wave_only(wave: usize) -> Self {
        Self { wave, leaf: None }
    }

    /// Recipe row, no unit.
    pub fn recipe(wave: usize, recipe: usize) -> Self {
        Self {
            wave,
            leaf: Some(SelectionLeaf::Recipe { recipe, unit: None }),
        }
    }

    /// Unit row inside a recipe.
    pub fn unit(wave: usize, recipe: usize, unit: usize) -> Self {
        Self {
            wave,
            leaf: Some(SelectionLeaf::Recipe { recipe, unit: Some(unit) }),
        }
    }

    /// Files folder header row inside a wave.
    pub fn files_folder(wave: usize) -> Self {
        Self {
            wave,
            leaf: Some(SelectionLeaf::FilesFolder),
        }
    }

    /// File row in the wave's Files folder.
    pub fn file(wave: usize, file: usize) -> Self {
        Self { wave, leaf: Some(SelectionLeaf::File(file)) }
    }

    pub fn recipe_index(&self) -> Option<usize> {
        match self.leaf {
            Some(SelectionLeaf::Recipe { recipe, .. }) => Some(recipe),
            _ => None,
        }
    }

    pub fn unit_index(&self) -> Option<usize> {
        match self.leaf {
            Some(SelectionLeaf::Recipe { unit, .. }) => unit,
            _ => None,
        }
    }

    pub fn file_index(&self) -> Option<usize> {
        match self.leaf {
            Some(SelectionLeaf::File(i)) => Some(i),
            _ => None,
        }
    }

    /// Resolve the selection to a graph node id.
    ///
    /// Returns `None` for wave-only, recipe-only, and files-folder
    /// selections — they are container rows, not single nodes. The
    /// focus subgraph fans those out (recipe = all units in recipe +
    /// 1-hop; wave / files-folder = full wave); callers that need a
    /// node id must guard against `None` rather than expecting a
    /// synthetic id.
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
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Normal,
    Search,
    EdgePicker,
    Help,
    DetailOverlay,
}

pub struct AppState {
    pub tree: IndexTree,
    pub selection: Selection,
    pub mode: Mode,
    pub camera_x: i32,
    pub camera_y: i32,
    pub follow: bool,
    pub should_quit: bool,
    pub edge_picker: EdgePicker,
    pub search: crate::render::search::SearchState,
    pub graph: std::sync::Arc<WaveDagData>,
    pub theme: crate::theme::Theme,
    pub pins: PinState,
    pub last_pin_message: Option<PinMsg>,
    /// First visible row in the index tree, as an index into
    /// `visible_rows()`. Updated each render to keep the selection in
    /// view; persisted in state so navigation feels sticky (the
    /// viewport doesn't snap when the selection stays in range).
    pub index_scroll: usize,
}

impl AppState {
    pub fn new(graph: &WaveDagData) -> Self {
        let arc = std::sync::Arc::new(graph.clone());
        Self {
            tree: IndexTree::from_graph(&arc),
            selection: Selection::first(),
            mode: Mode::Normal,
            camera_x: 0,
            camera_y: 0,
            follow: true,
            should_quit: false,
            edge_picker: EdgePicker::default(),
            search: Default::default(),
            graph: arc,
            theme: Default::default(),
            pins: PinState::default(),
            last_pin_message: None,
            index_scroll: 0,
        }
    }

    pub fn with_theme(graph: &WaveDagData, theme: crate::theme::Theme) -> Self {
        let mut me = Self::new(graph);
        me.theme = theme;
        me
    }

    pub fn toggle_pin_selected(&mut self) {
        let Some(node_id) = self.selection.node_id(&self.tree) else {
            return;
        };
        let owned = node_id.to_string();
        if self.pins.unpin(&owned) {
            return;
        }
        if self.pins.pin(&owned).is_none() {
            self.last_pin_message = Some(PinMsg::Full);
        }
    }

    pub fn clear_all_pins(&mut self) {
        let n = self.pins.iter().count();
        self.pins.clear();
        self.last_pin_message = Some(PinMsg::ClearedAll(n));
    }

    pub fn jump_to_pin_slot(&mut self, slot: usize) {
        let Some(target_id) = self.pins.id_at(slot).map(|s| s.to_string()) else {
            self.last_pin_message = Some(PinMsg::EmptySlot(slot));
            return;
        };
        for (wi, wave) in self.tree.waves.iter().enumerate() {
            for (fi, file) in wave.files.iter().enumerate() {
                if file.node_id == target_id {
                    self.selection = Selection::file(wi, fi);
                    if let Some(w) = self.tree.waves.get_mut(wi) {
                        w.expanded = true;
                        w.files_expanded = true;
                    }
                    return;
                }
            }
            for (ri, recipe) in wave.recipes.iter().enumerate() {
                for (ui, unit) in recipe.units.iter().enumerate() {
                    if unit.node_id == target_id {
                        self.selection = Selection::unit(wi, ri, ui);
                        // Mirror the search-jump expansion behaviour.
                        if let Some(w) = self.tree.waves.get_mut(wi) {
                            w.expanded = true;
                            if let Some(r) = w.recipes.get_mut(ri) {
                                r.expanded = true;
                            }
                        }
                        return;
                    }
                }
            }
        }
    }

    pub fn bulk_pin_recipe(&mut self, graph: &WaveDagData) {
        let Some(selected_id) = self.selection.node_id(&self.tree) else {
            self.last_pin_message = Some(PinMsg::OnFile);
            return;
        };
        let selected_owned = selected_id.to_string();

        // Locate the selected node and confirm it's a unit with a recipe.
        let mut recipe_name: Option<String> = None;
        let mut wave_idx: Option<usize> = None;
        for (wi, wave) in graph.waves.iter().enumerate() {
            if let Some(node) = wave.nodes.iter().find(|n| n.id == selected_owned) {
                if node.kind != "unit" {
                    self.last_pin_message = Some(PinMsg::OnFile);
                    return;
                }
                recipe_name = node.recipe.clone();
                wave_idx = Some(wi);
                break;
            }
        }
        let Some(recipe) = recipe_name else {
            self.last_pin_message = Some(PinMsg::OnFile);
            return;
        };
        let Some(wi) = wave_idx else { return };

        let wave_units: Vec<String> = graph.waves[wi]
            .nodes
            .iter()
            .filter(|n| n.kind == "unit" && n.recipe.as_deref() == Some(&recipe))
            .map(|n| n.id.clone())
            .collect();
        if wave_units.is_empty() {
            return;
        }

        // If all units are already pinned, unpin them all.
        if wave_units.iter().all(|id| self.pins.slot_of(id).is_some()) {
            for id in &wave_units {
                self.pins.unpin(id);
            }
            return;
        }

        // Otherwise pin missing ones; stop at first Full.
        for id in wave_units {
            if self.pins.slot_of(&id).is_some() {
                continue;
            }
            if self.pins.pin(&id).is_none() {
                self.last_pin_message = Some(PinMsg::Full);
                return;
            }
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct EdgePicker {
    pub direction: PickerDir,
    pub candidates: Vec<String>,
    pub cursor: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PickerDir {
    #[default]
    Downstream,
    Upstream,
}

impl AppState {
    pub fn open_edge_picker(&mut self, graph: &WaveDagData, dir: PickerDir) {
        let Some(node_id) = self.selection.node_id(&self.tree).map(str::to_string) else {
            return;
        };
        let candidates = adjacency(graph, &node_id, dir);
        if candidates.is_empty() {
            return;
        }
        if candidates.len() == 1 {
            self.jump_to_node(&candidates[0]);
            return;
        }
        self.edge_picker = EdgePicker { direction: dir, candidates, cursor: 0 };
        self.mode = Mode::EdgePicker;
    }

    pub fn open_edge_picker_for_selection(&mut self, dir: PickerDir) {
        let g = self.graph.clone();
        self.open_edge_picker(&g, dir);
    }

    pub fn jump_to_node(&mut self, node_id: &str) {
        for (wi, wave) in self.tree.waves.iter_mut().enumerate() {
            for (fi, file) in wave.files.iter().enumerate() {
                if file.node_id == node_id {
                    wave.expanded = true;
                    wave.files_expanded = true;
                    self.selection = Selection::file(wi, fi);
                    return;
                }
            }
            for (ri, recipe) in wave.recipes.iter_mut().enumerate() {
                for (ui, unit) in recipe.units.iter().enumerate() {
                    if unit.node_id == node_id {
                        wave.expanded = true;
                        recipe.expanded = true;
                        self.selection = Selection::unit(wi, ri, ui);
                        return;
                    }
                }
            }
        }
    }
}

// Adjacency lookup walks all wave edges + inter-wave edges.
fn adjacency(graph: &WaveDagData, node_id: &str, dir: PickerDir) -> Vec<String> {
    let mut out = Vec::new();
    for wave in &graph.waves {
        for e in &wave.edges {
            match dir {
                PickerDir::Downstream if e.from == node_id => out.push(e.to.clone()),
                PickerDir::Upstream if e.to == node_id => out.push(e.from.clone()),
                _ => {}
            }
        }
    }
    for e in &graph.inter_wave_edges {
        match dir {
            PickerDir::Downstream if e.from == node_id => out.push(e.to.clone()),
            PickerDir::Upstream if e.to == node_id => out.push(e.from.clone()),
            _ => {}
        }
    }
    out
}

impl AppState {
    /// Move the selection one visible row down (or up if `up`).
    pub fn move_cursor(&mut self, up: bool) {
        let visible = self.visible_rows();
        let Some(idx) = visible.iter().position(|s| *s == self.selection) else {
            self.selection = visible.first().copied().unwrap_or(self.selection);
            return;
        };
        let new = if up { idx.saturating_sub(1) } else { (idx + 1).min(visible.len() - 1) };
        self.selection = visible[new];
    }

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

    pub fn jump_first(&mut self) {
        if let Some(first) = self.visible_rows().first() {
            self.selection = *first;
        }
    }

    pub fn jump_last(&mut self) {
        if let Some(last) = self.visible_rows().last() {
            self.selection = *last;
        }
    }

    /// Adjust `index_scroll` so the selected row sits in the visible
    /// window of `available_rows`. Sticky: scroll only changes when the
    /// selection would otherwise leave the viewport.
    pub fn ensure_index_visible(&mut self, available_rows: usize) {
        if available_rows == 0 {
            return;
        }
        let visible = self.visible_rows();
        let Some(idx) = visible.iter().position(|s| *s == self.selection) else {
            return;
        };
        if idx < self.index_scroll {
            self.index_scroll = idx;
        } else if idx >= self.index_scroll + available_rows {
            self.index_scroll = idx + 1 - available_rows;
        }
        let max_scroll = visible.len().saturating_sub(available_rows);
        self.index_scroll = self.index_scroll.min(max_scroll);
    }

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
}

impl AppState {
    pub fn pan_camera(
        &mut self,
        dx: i32,
        dy: i32,
        layout: &crate::render::layout::Layout,
        pane: ratatui::layout::Rect,
    ) {
        use crate::render::camera::Camera;
        let cam = Camera { x: self.camera_x, y: self.camera_y };
        let panned = cam.pan(dx, dy, layout, pane);
        self.camera_x = panned.x;
        self.camera_y = panned.y;
        self.follow = false;
    }

    pub fn recenter(
        &mut self,
        layout: &crate::render::layout::Layout,
        pane: ratatui::layout::Rect,
    ) {
        use crate::render::camera::Camera;
        let cam = Camera::fit_bounds(layout, pane);
        self.camera_x = cam.x;
        self.camera_y = cam.y;
        self.follow = true;
    }

    pub fn auto_fit(
        &mut self,
        layout: &crate::render::layout::Layout,
        pane: ratatui::layout::Rect,
    ) {
        use crate::render::camera::Camera;
        let cam = Camera::auto_fit(layout, pane);
        self.camera_x = cam.x;
        self.camera_y = cam.y;
        self.follow = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dag_data::{EdgeData, NodeData, WaveData, WaveDagData};

    fn unit(id: &str, recipe: &str, label: &str) -> NodeData {
        NodeData {
            id: id.into(),
            kind: "unit".into(),
            label: label.into(),
            recipe: Some(recipe.into()),
            command: Some("cmd".into()),
            output: None,
            cached: Some(true),
            dep_kind: Some("sequential".into()),
            group_index: None,
            modified: None,
            discovered: None,
        }
    }

    fn graph_2x2() -> WaveDagData {
        WaveDagData {
            schema_version: crate::VIEWER_SCHEMA_VERSION,
            target: "build".into(),
            waves: vec![
                WaveData {
                    recipes: vec!["a".into(), "b".into()],
                    nodes: vec![
                        unit("unit:a:0", "a", "a0"),
                        unit("unit:a:1", "a", "a1"),
                        unit("unit:b:0", "b", "b0"),
                    ],
                    edges: vec![],
                },
                WaveData {
                    recipes: vec!["c".into()],
                    nodes: vec![unit("unit:c:0", "c", "c0")],
                    edges: vec![],
                },
            ],
            inter_wave_edges: vec![],
        }
    }

    #[test]
    fn tree_groups_units_by_recipe() {
        let t = IndexTree::from_graph(&graph_2x2());
        assert_eq!(t.waves.len(), 2);
        assert_eq!(t.waves[0].recipes.len(), 2);
        assert_eq!(t.waves[0].recipes[0].name, "a");
        assert_eq!(t.waves[0].recipes[0].units.len(), 2);
        assert_eq!(t.waves[0].recipes[0].units[0].label, "a0");
        assert_eq!(t.waves[0].recipes[1].name, "b");
        assert_eq!(t.waves[0].recipes[1].units.len(), 1);
        assert_eq!(t.waves[1].recipes[0].name, "c");
    }

    #[test]
    fn wave_zero_is_expanded_by_default() {
        let t = IndexTree::from_graph(&graph_2x2());
        assert!(t.waves[0].expanded);
        assert!(!t.waves[1].expanded);
    }

    #[test]
    fn recipes_default_collapsed() {
        let t = IndexTree::from_graph(&graph_2x2());
        for w in &t.waves {
            for r in &w.recipes {
                assert!(!r.expanded);
            }
        }
    }

    #[test]
    fn selection_node_id_returns_unit_id_when_fully_qualified() {
        let t = IndexTree::from_graph(&graph_2x2());
        let sel = Selection::unit(0, 0, 1);
        assert_eq!(sel.node_id(&t), Some("unit:a:1"));
    }

    #[test]
    fn selection_node_id_is_none_at_wave_or_recipe_level() {
        let t = IndexTree::from_graph(&graph_2x2());
        assert_eq!(Selection::wave_only(0).node_id(&t), None);
        assert_eq!(Selection::recipe(0, 0).node_id(&t), None);
    }

    #[test]
    fn app_state_starts_with_first_selection_and_follow_on() {
        let app = AppState::new(&graph_2x2());
        assert_eq!(app.selection, Selection::first());
        assert!(app.follow);
        assert_eq!(app.mode, Mode::Normal);
    }

    #[test]
    fn move_cursor_down_steps_through_waves() {
        let g = graph_2x2();
        let mut app = AppState::new(&g);
        // Wave 0 expanded by default but recipes collapsed.
        // Visible: W0, recipe a, recipe b, W1.
        assert_eq!(app.selection, Selection::wave_only(0));
        app.move_cursor(false);
        assert_eq!(app.selection, Selection::recipe(0, 0));
        app.move_cursor(false);
        assert_eq!(app.selection, Selection::recipe(0, 1));
        app.move_cursor(false);
        assert_eq!(app.selection, Selection::wave_only(1));
    }

    #[test]
    fn expand_then_step_in_descends_into_units() {
        let g = graph_2x2();
        let mut app = AppState::new(&g);
        app.move_cursor(false); // recipe a
        app.expand_or_step_in();
        assert!(app.tree.waves[0].recipes[0].expanded);
        app.move_cursor(false); // first unit a0
        assert_eq!(app.selection, Selection::unit(0, 0, 0));
    }

    #[test]
    fn open_edge_picker_zero_candidates_no_op() {
        let g = graph_2x2();
        let mut app = AppState::new(&g);
        app.tree.waves[0].recipes[0].expanded = true;
        app.selection = Selection::unit(0, 0, 0);
        app.open_edge_picker(&g, PickerDir::Downstream);
        assert_eq!(app.mode, Mode::Normal);
    }

    #[test]
    fn open_edge_picker_single_candidate_jumps_directly() {
        let mut g = graph_2x2();
        g.inter_wave_edges.push(EdgeData {
            from: "unit:a:0".into(),
            to: "unit:c:0".into(),
        });
        let mut app = AppState::new(&g);
        app.tree.waves[0].recipes[0].expanded = true;
        app.selection = Selection::unit(0, 0, 0);
        app.open_edge_picker(&g, PickerDir::Downstream);
        assert_eq!(app.mode, Mode::Normal);
        assert_eq!(app.selection.node_id(&app.tree), Some("unit:c:0"));
    }

    #[test]
    fn open_edge_picker_multiple_candidates_opens_picker() {
        let mut g = graph_2x2();
        g.inter_wave_edges.push(EdgeData {
            from: "unit:a:0".into(),
            to: "unit:c:0".into(),
        });
        g.waves[0].edges.push(EdgeData {
            from: "unit:a:0".into(),
            to: "unit:b:0".into(),
        });
        let mut app = AppState::new(&g);
        app.tree.waves[0].recipes[0].expanded = true;
        app.selection = Selection::unit(0, 0, 0);
        app.open_edge_picker(&g, PickerDir::Downstream);
        assert_eq!(app.mode, Mode::EdgePicker);
        assert_eq!(app.edge_picker.candidates.len(), 2);
    }

    #[test]
    fn pan_camera_disables_follow() {
        let g = graph_2x2();
        let mut app = AppState::new(&g);
        let layout = crate::render::layout::compute(&g, crate::render::layout::LayoutDims::FULL);
        app.pan_camera(10, 10, &layout, ratatui::layout::Rect::new(0, 0, 80, 24));
        assert!(!app.follow);
    }

    #[test]
    fn recenter_reengages_follow() {
        let g = graph_2x2();
        let mut app = AppState::new(&g);
        app.tree.waves[0].recipes[0].expanded = true;
        app.selection = Selection::unit(0, 0, 0);
        let layout = crate::render::layout::compute(&g, crate::render::layout::LayoutDims::FULL);
        app.follow = false;
        app.recenter(&layout, ratatui::layout::Rect::new(0, 0, 80, 24));
        assert!(app.follow);
    }

    #[test]
    fn pin_state_starts_empty() {
        let p = PinState::default();
        assert!(p.is_empty());
        assert!(!p.is_full());
        assert_eq!(p.iter().count(), 0);
    }

    #[test]
    fn pin_returns_first_empty_slot() {
        let mut p = PinState::default();
        assert_eq!(p.pin("a"), Some(0));
        assert_eq!(p.pin("b"), Some(1));
        assert_eq!(p.pin("c"), Some(2));
    }

    #[test]
    fn pin_is_idempotent_for_same_id() {
        let mut p = PinState::default();
        p.pin("a");
        p.pin("b");
        assert_eq!(p.pin("a"), Some(0), "re-pinning returns existing slot");
    }

    #[test]
    fn pin_returns_none_when_full() {
        let mut p = PinState::default();
        for i in 0..PIN_SLOTS {
            p.pin(&format!("n{i}"));
        }
        assert!(p.is_full());
        assert_eq!(p.pin("overflow"), None);
    }

    #[test]
    fn unpin_clears_slot_and_returns_true() {
        let mut p = PinState::default();
        p.pin("a");
        assert_eq!(p.unpin("a"), true);
        assert!(p.is_empty());
    }

    #[test]
    fn unpin_returns_false_when_not_pinned() {
        let mut p = PinState::default();
        assert_eq!(p.unpin("nonesuch"), false);
    }

    #[test]
    fn slot_of_finds_existing_pin() {
        let mut p = PinState::default();
        p.pin("a");
        p.pin("b");
        assert_eq!(p.slot_of("a"), Some(0));
        assert_eq!(p.slot_of("b"), Some(1));
        assert_eq!(p.slot_of("c"), None);
    }

    #[test]
    fn id_at_returns_pinned_id() {
        let mut p = PinState::default();
        p.pin("a");
        assert_eq!(p.id_at(0), Some("a"));
        assert_eq!(p.id_at(1), None);
    }

    #[test]
    fn iter_yields_pairs_in_slot_order_skipping_empty() {
        let mut p = PinState::default();
        p.pin("a");
        p.pin("b");
        p.pin("c");
        p.unpin("b");
        let pairs: Vec<(usize, &str)> = p.iter().collect();
        assert_eq!(pairs, vec![(0, "a"), (2, "c")]);
    }

    #[test]
    fn pin_after_unpin_reuses_freed_slot() {
        let mut p = PinState::default();
        p.pin("a");
        p.pin("b");
        p.unpin("a");
        assert_eq!(p.pin("c"), Some(0), "should reuse the lowest empty slot");
    }

    #[test]
    fn clear_empties_all_slots() {
        let mut p = PinState::default();
        p.pin("a");
        p.pin("b");
        p.clear();
        assert!(p.is_empty());
    }

    #[test]
    fn pin_msg_full_renders_clear_hint() {
        assert_eq!(
            PinMsg::Full.render(),
            "pin slots full — clear with X"
        );
    }

    #[test]
    fn pin_msg_cleared_all_handles_singular_and_plural() {
        assert_eq!(PinMsg::ClearedAll(1).render(), "cleared 1 pin");
        assert_eq!(PinMsg::ClearedAll(3).render(), "cleared 3 pins");
        assert_eq!(PinMsg::ClearedAll(0).render(), "cleared 0 pins");
    }

    #[test]
    fn pin_msg_empty_slot_uses_one_indexed_label() {
        assert_eq!(PinMsg::EmptySlot(0).render(), "slot 1 empty");
        assert_eq!(PinMsg::EmptySlot(8).render(), "slot 9 empty");
    }

    fn graph_with_files() -> WaveDagData {
        use crate::dag_data::EdgeData;
        WaveDagData {
            schema_version: crate::VIEWER_SCHEMA_VERSION,
            target: "build".into(),
            waves: vec![WaveData {
                recipes: vec!["a".into()],
                nodes: vec![
                    NodeData {
                        id: "file:foo.cpp".into(),
                        kind: "file".into(),
                        label: "foo.cpp".into(),
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
                edges: vec![EdgeData { from: "file:foo.cpp".into(), to: "unit:a:0".into() }],
            }],
            inter_wave_edges: vec![],
        }
    }

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

    #[test]
    fn jump_to_node_on_file_lands_on_file_row_and_expands_folder() {
        let g = graph_with_files();
        let mut app = AppState::new(&g);
        app.tree.waves[0].files_expanded = false;
        app.jump_to_node("file:foo.cpp");
        assert_eq!(app.selection, Selection::file(0, 0));
        assert!(app.tree.waves[0].expanded);
        assert!(app.tree.waves[0].files_expanded);
    }

    #[test]
    fn selection_node_id_resolves_file_leaf() {
        let g = graph_with_files();
        let app = AppState::new(&g);
        let sel = Selection::file(0, 0);
        assert_eq!(sel.node_id(&app.tree), Some("file:foo.cpp"));
    }

    #[test]
    fn bulk_pin_recipe_on_file_selection_emits_on_file() {
        let g = graph_with_files();
        let mut app = AppState::new(&g);
        app.tree.waves[0].files_expanded = true;
        app.selection = Selection::file(0, 0);
        app.bulk_pin_recipe(&g);
        assert_eq!(app.last_pin_message, Some(PinMsg::OnFile));
        assert!(app.pins.is_empty());
    }

    #[test]
    fn bulk_pin_recipe_on_files_folder_selection_emits_on_file() {
        let g = graph_with_files();
        let mut app = AppState::new(&g);
        app.selection = Selection::files_folder(0);
        app.bulk_pin_recipe(&g);
        assert_eq!(app.last_pin_message, Some(PinMsg::OnFile));
        assert!(app.pins.is_empty());
    }

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

    #[test]
    fn collapse_step_out_on_collapsed_recipe_row_returns_to_wave() {
        let g = graph_2x2();
        let mut app = AppState::new(&g);
        app.selection = Selection::recipe(0, 0);
        // Recipe is collapsed by default.
        app.collapse_or_step_out();
        assert_eq!(app.selection, Selection::wave_only(0));
    }

    fn tall_graph(unit_count: usize) -> WaveDagData {
        WaveDagData {
            schema_version: crate::VIEWER_SCHEMA_VERSION,
            target: "build".into(),
            waves: vec![WaveData {
                recipes: vec!["a".into()],
                nodes: (0..unit_count).map(|i| unit(&format!("unit:a:{i}"), "a", &format!("u{i}"))).collect(),
                edges: vec![],
            }],
            inter_wave_edges: vec![],
        }
    }

    #[test]
    fn ensure_index_visible_keeps_in_view_selection_does_not_move_scroll() {
        let g = tall_graph(20);
        let mut app = AppState::new(&g);
        app.tree.waves[0].recipes[0].expanded = true;
        // Visible rows: wave_only, recipe(0), unit(0..20). Total 22 rows.
        // Pretend the pane is 10 rows tall.
        app.index_scroll = 5;
        // Selection at row 8 — already in [5, 15).
        app.selection = Selection::unit(0, 0, 6); // logical idx = 2 (wave) + 1 (recipe) + 6 = wait
        // Visible rows order: wave_only(0)=0, recipe(0)=1, unit(0,0,0)=2 ... unit(0,0,19)=21.
        // Selection unit(0,0,6) is at logical idx 2 + 6 = 8. In [5, 15). Should not move scroll.
        app.ensure_index_visible(10);
        assert_eq!(app.index_scroll, 5);
    }

    #[test]
    fn ensure_index_visible_scrolls_down_when_selection_below_viewport() {
        let g = tall_graph(20);
        let mut app = AppState::new(&g);
        app.tree.waves[0].recipes[0].expanded = true;
        app.index_scroll = 0;
        // Selection at unit(0,0,15) → logical idx 17. With pane height 10, viewport [0..10). 17 >= 10 → scroll = 17 + 1 - 10 = 8.
        app.selection = Selection::unit(0, 0, 15);
        app.ensure_index_visible(10);
        assert_eq!(app.index_scroll, 8);
    }

    #[test]
    fn ensure_index_visible_scrolls_up_when_selection_above_viewport() {
        let g = tall_graph(20);
        let mut app = AppState::new(&g);
        app.tree.waves[0].recipes[0].expanded = true;
        app.index_scroll = 10;
        // Selection at unit(0,0,2) → logical idx 4. 4 < 10 → scroll = 4.
        app.selection = Selection::unit(0, 0, 2);
        app.ensure_index_visible(10);
        assert_eq!(app.index_scroll, 4);
    }

    #[test]
    fn ensure_index_visible_clamps_scroll_when_visible_rows_shrink() {
        let g = tall_graph(20);
        let mut app = AppState::new(&g);
        app.tree.waves[0].recipes[0].expanded = true;
        // 22 visible rows, pane 10 → max_scroll = 12.
        app.index_scroll = 50; // bogus large value
        app.selection = Selection::wave_only(0); // logical 0
        app.ensure_index_visible(10);
        // First the "selection above viewport" branch sets scroll = 0 (idx=0 < 50).
        // Then clamp to max_scroll = 12. Result is 0 because 0.min(12) = 0.
        assert_eq!(app.index_scroll, 0);
    }

    #[test]
    fn ensure_index_visible_no_op_when_pane_height_is_zero() {
        let g = tall_graph(20);
        let mut app = AppState::new(&g);
        app.index_scroll = 7;
        app.ensure_index_visible(0);
        assert_eq!(app.index_scroll, 7);
    }
}
