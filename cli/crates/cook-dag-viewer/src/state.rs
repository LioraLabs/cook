//! `AppState` + `IndexTree`. See design spec §Index, §Camera.

use crate::dag_data::WaveDagData;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DensityMode {
    Full,
    Compact,
    Dot,
}

impl DensityMode {
    /// Cycle order for the `m` key: Dot → Compact → Full → Dot.
    pub fn next(self) -> Self {
        match self {
            Self::Dot => Self::Compact,
            Self::Compact => Self::Full,
            Self::Full => Self::Dot,
        }
    }
}

pub const PIN_SLOTS: usize = 9;

/// Numbered-circle glyph for pin slot N (0-indexed). Slot 0 → `❶`.
/// Out-of-range slots return `'●'`.
pub fn pin_glyph(slot: usize) -> char {
    const GLYPHS: [char; PIN_SLOTS] = ['❶', '❷', '❸', '❹', '❺', '❻', '❼', '❽', '❾'];
    GLYPHS.get(slot).copied().unwrap_or('●')
}

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
            Self::OnFile => "bulk-pin only applies to recipe units".to_string(),
            Self::EmptySlot(n) => format!("slot {} empty", n + 1),
            Self::ClearedAll(n) => format!("cleared {n} pin{}", if n == 1 { "" } else { "s" }),
        }
    }
}

/// Pick a default density from the snapshot's node count. See spec §5.1.
pub fn choose_initial_mode(g: &WaveDagData) -> DensityMode {
    let total: usize = g.waves.iter().map(|w| w.nodes.len()).sum();
    match total {
        0..=20 => DensityMode::Full,
        21..=80 => DensityMode::Compact,
        _ => DensityMode::Dot,
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
pub struct WaveRow {
    pub label: String,
    pub recipes: Vec<RecipeRow>,
    pub expanded: bool,
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

            for n in &wave.nodes {
                if n.kind != "unit" {
                    continue;
                }
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

            waves.push(WaveRow {
                label: format!("Wave {} ({} recipes)", wi, recipes.len()),
                recipes,
                expanded: wi == 0, // Wave 0 expanded by default.
            });
        }
        Self { waves }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Selection {
    pub wave: usize,
    pub recipe: Option<usize>,
    pub unit: Option<usize>,
}

impl Selection {
    pub fn first() -> Self {
        Self { wave: 0, recipe: None, unit: None }
    }

    pub fn node_id<'a>(&self, tree: &'a IndexTree) -> Option<&'a str> {
        let w = tree.waves.get(self.wave)?;
        let r = w.recipes.get(self.recipe?)?;
        let u = r.units.get(self.unit?)?;
        Some(&u.node_id)
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
    pub density: DensityMode,
    pub pins: PinState,
    pub last_pin_message: Option<PinMsg>,
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
            density: choose_initial_mode(graph),
            pins: PinState::default(),
            last_pin_message: None,
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
            for (ri, recipe) in wave.recipes.iter_mut().enumerate() {
                for (ui, unit) in recipe.units.iter().enumerate() {
                    if unit.node_id == node_id {
                        wave.expanded = true;
                        recipe.expanded = true;
                        self.selection = Selection { wave: wi, recipe: Some(ri), unit: Some(ui) };
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
        match (self.selection.recipe, self.selection.unit) {
            (Some(_), Some(_)) => {
                self.selection.unit = None;
            }
            (Some(ri), None) => {
                self.tree.waves[self.selection.wave].recipes[ri].expanded = false;
            }
            (None, _) => {
                self.tree.waves[self.selection.wave].expanded = false;
            }
        }
    }

    pub fn expand_or_step_in(&mut self) {
        match (self.selection.recipe, self.selection.unit) {
            (None, _) => {
                let w = self.selection.wave;
                self.tree.waves[w].expanded = true;
            }
            (Some(ri), None) => {
                self.tree.waves[self.selection.wave].recipes[ri].expanded = true;
            }
            (Some(_), Some(_)) => { /* already at leaf */ }
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

    fn visible_rows(&self) -> Vec<Selection> {
        let mut out = Vec::new();
        for (wi, wave) in self.tree.waves.iter().enumerate() {
            out.push(Selection { wave: wi, recipe: None, unit: None });
            if !wave.expanded {
                continue;
            }
            for (ri, recipe) in wave.recipes.iter().enumerate() {
                out.push(Selection { wave: wi, recipe: Some(ri), unit: None });
                if !recipe.expanded {
                    continue;
                }
                for ui in 0..recipe.units.len() {
                    out.push(Selection { wave: wi, recipe: Some(ri), unit: Some(ui) });
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
        if let Some(node_id) = self.selection.node_id(&self.tree) {
            if let Some(node) = layout.nodes.iter().find(|n| n.id == node_id) {
                let cam = Camera::center_on(node, pane);
                self.camera_x = cam.x;
                self.camera_y = cam.y;
                self.follow = true;
            }
        }
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
        let sel = Selection { wave: 0, recipe: Some(0), unit: Some(1) };
        assert_eq!(sel.node_id(&t), Some("unit:a:1"));
    }

    #[test]
    fn selection_node_id_is_none_at_wave_or_recipe_level() {
        let t = IndexTree::from_graph(&graph_2x2());
        assert_eq!(Selection { wave: 0, recipe: None, unit: None }.node_id(&t), None);
        assert_eq!(
            Selection { wave: 0, recipe: Some(0), unit: None }.node_id(&t),
            None
        );
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
        assert_eq!(app.selection, Selection { wave: 0, recipe: None, unit: None });
        app.move_cursor(false);
        assert_eq!(app.selection, Selection { wave: 0, recipe: Some(0), unit: None });
        app.move_cursor(false);
        assert_eq!(app.selection, Selection { wave: 0, recipe: Some(1), unit: None });
        app.move_cursor(false);
        assert_eq!(app.selection, Selection { wave: 1, recipe: None, unit: None });
    }

    #[test]
    fn expand_then_step_in_descends_into_units() {
        let g = graph_2x2();
        let mut app = AppState::new(&g);
        app.move_cursor(false); // recipe a
        app.expand_or_step_in();
        assert!(app.tree.waves[0].recipes[0].expanded);
        app.move_cursor(false); // first unit a0
        assert_eq!(app.selection, Selection { wave: 0, recipe: Some(0), unit: Some(0) });
    }

    #[test]
    fn open_edge_picker_zero_candidates_no_op() {
        let g = graph_2x2();
        let mut app = AppState::new(&g);
        app.tree.waves[0].recipes[0].expanded = true;
        app.selection = Selection { wave: 0, recipe: Some(0), unit: Some(0) };
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
        app.selection = Selection { wave: 0, recipe: Some(0), unit: Some(0) };
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
        app.selection = Selection { wave: 0, recipe: Some(0), unit: Some(0) };
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
        app.selection = Selection { wave: 0, recipe: Some(0), unit: Some(0) };
        let layout = crate::render::layout::compute(&g, crate::render::layout::LayoutDims::FULL);
        app.follow = false;
        app.recenter(&layout, ratatui::layout::Rect::new(0, 0, 80, 24));
        assert!(app.follow);
    }

    #[test]
    fn density_mode_cycles_dot_compact_full_dot() {
        let mut m = DensityMode::Dot;
        m = m.next();
        assert_eq!(m, DensityMode::Compact);
        m = m.next();
        assert_eq!(m, DensityMode::Full);
        m = m.next();
        assert_eq!(m, DensityMode::Dot);
    }

    #[test]
    fn choose_initial_mode_picks_full_for_small_graphs() {
        let g = small_graph(15);
        assert_eq!(choose_initial_mode(&g), DensityMode::Full);
    }

    #[test]
    fn choose_initial_mode_picks_compact_in_middle_band() {
        let g = small_graph(50);
        assert_eq!(choose_initial_mode(&g), DensityMode::Compact);
    }

    #[test]
    fn choose_initial_mode_picks_dot_for_big_graphs() {
        let g = small_graph(200);
        assert_eq!(choose_initial_mode(&g), DensityMode::Dot);
    }

    #[test]
    fn app_state_starts_with_density_chosen_from_node_count() {
        let g = small_graph(15);
        let app = AppState::new(&g);
        assert_eq!(app.density, DensityMode::Full);
    }

    fn small_graph(n: usize) -> WaveDagData {
        WaveDagData {
            schema_version: crate::VIEWER_SCHEMA_VERSION,
            target: "build".into(),
            waves: vec![WaveData {
                recipes: vec!["a".into()],
                nodes: (0..n)
                    .map(|i| NodeData {
                        id: format!("unit:a:{i}"),
                        kind: "unit".into(),
                        label: format!("u{i}"),
                        recipe: Some("a".into()),
                        command: Some("c".into()),
                        output: None,
                        cached: Some(true),
                        dep_kind: Some("sequential".into()),
                        group_index: None,
                        modified: None,
                        discovered: None,
                    })
                    .collect(),
                edges: vec![],
            }],
            inter_wave_edges: vec![],
        }
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
}
