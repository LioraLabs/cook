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
#[path = "tests/state_tests.rs"]
mod tests;
