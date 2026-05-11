//! TUI state.

use std::collections::BTreeSet;

use cook_progress::event::{NodeId, RecipeId, Stream};
use cook_progress::log_reader::{BuildSummary, BuildView, LoadDiagnostics, NodeView};
use cook_progress::model::NodeStatus;

#[derive(Debug, Clone)]
pub struct PickerState {
    pub builds: Vec<BuildSummary>,
    pub cursor: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Filter {
    All,
    FailedOnly,
    WithErrStream,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    Tree,
    Output,
}

#[derive(Debug, Clone)]
pub struct SearchState {
    pub pattern: String,
    pub matches: Vec<(RecipeId, NodeId, usize)>, // (recipe, node, line index)
    pub cursor: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlatRow {
    Recipe(RecipeId),
    Node(RecipeId, NodeId),
}

pub struct UiState {
    pub view: BuildView,
    pub diagnostics: LoadDiagnostics,
    pub flat: Vec<FlatRow>,
    pub expanded: BTreeSet<RecipeId>,
    pub selected: usize,
    pub scroll_y: u16,
    pub filter: Filter,
    pub search: Option<SearchState>,
    pub show_timestamps: bool,
    pub soft_wrap: bool,
    pub focus: Focus,
    pub picker: Option<PickerState>,
    pub show_help: bool,
}

impl UiState {
    pub fn new(view: BuildView, diagnostics: LoadDiagnostics) -> Self {
        let expanded: BTreeSet<RecipeId> = view.recipes.keys().copied().collect();
        let mut s = Self {
            view,
            diagnostics,
            flat: Vec::new(),
            expanded,
            selected: 0,
            scroll_y: 0,
            filter: Filter::All,
            search: None,
            show_timestamps: false,
            soft_wrap: false,
            focus: Focus::Tree,
            picker: None,
            show_help: false,
        };
        s.rebuild_flat();
        s.select_first_failed_or_first();
        s
    }

    pub fn rebuild_flat(&mut self) {
        self.flat.clear();
        for (rid, recipe) in &self.view.recipes {
            self.flat.push(FlatRow::Recipe(*rid));
            if !self.expanded.contains(rid) {
                continue;
            }
            for (nid, node) in &recipe.nodes {
                if !self.passes_filter(node) {
                    continue;
                }
                self.flat.push(FlatRow::Node(*rid, *nid));
            }
        }
        if self.selected >= self.flat.len() {
            self.selected = self.flat.len().saturating_sub(1);
        }
    }

    fn passes_filter(&self, node: &NodeView) -> bool {
        match self.filter {
            Filter::All => true,
            Filter::FailedOnly => node.status == NodeStatus::Failed,
            Filter::WithErrStream => node.lines.iter().any(|l| l.stream == Stream::Stderr),
        }
    }

    fn select_first_failed_or_first(&mut self) {
        for (i, row) in self.flat.iter().enumerate() {
            if let FlatRow::Node(rid, nid) = row {
                if let Some(r) = self.view.recipes.get(rid) {
                    if let Some(n) = r.nodes.get(nid) {
                        if n.status == NodeStatus::Failed {
                            self.selected = i;
                            return;
                        }
                    }
                }
            }
        }
        self.selected = 0;
    }

    pub fn selected_node(&self) -> Option<(RecipeId, NodeId)> {
        match self.flat.get(self.selected)? {
            FlatRow::Node(r, n) => Some((*r, *n)),
            FlatRow::Recipe(_) => None,
        }
    }

    pub fn cycle_filter(&mut self) {
        self.filter = match self.filter {
            Filter::All => Filter::FailedOnly,
            Filter::FailedOnly => Filter::WithErrStream,
            Filter::WithErrStream => Filter::All,
        };
        self.rebuild_flat();
        self.scroll_y = 0;
    }

    pub fn toggle_fold(&mut self) {
        if let Some(FlatRow::Recipe(rid)) = self.flat.get(self.selected).copied() {
            if !self.expanded.remove(&rid) {
                self.expanded.insert(rid);
            }
            self.rebuild_flat();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cook_progress::event::{NodeId, NodeKind, RecipeId};
    use cook_progress::log_reader::{BuildView, NodeView, RecipeView};
    use cook_progress::model::{NodeStatus, Status};
    use std::collections::BTreeMap;

    fn mk(failed_first_node: bool) -> BuildView {
        let mut nodes = BTreeMap::new();
        nodes.insert(
            NodeId::new(0),
            NodeView {
                name: "a".into(),
                status: if failed_first_node {
                    NodeStatus::Failed
                } else {
                    NodeStatus::Completed
                },
                kind: NodeKind::Cooked,
                started_at: None,
                ended_at: None,
                elapsed_ms: None,
                skip_reason: None,
                lines: vec![],
            },
        );
        nodes.insert(
            NodeId::new(1),
            NodeView {
                name: "b".into(),
                status: NodeStatus::Failed,
                kind: NodeKind::Cooked,
                started_at: None,
                ended_at: None,
                elapsed_ms: None,
                skip_reason: None,
                lines: vec![],
            },
        );
        let mut recipes = BTreeMap::new();
        recipes.insert(
            RecipeId::new(0),
            RecipeView {
                name: "lib".into(),
                status: Status::Failed,
                nodes,
            },
        );
        BuildView {
            build_id: "x".into(),
            started_at: "t".into(),
            ended_at: None,
            exit_code: Some(1),
            recipes,
        }
    }

    #[test]
    fn flat_index_includes_recipe_then_nodes_when_expanded() {
        let s = UiState::new(mk(false), LoadDiagnostics::default());
        assert_eq!(s.flat.len(), 3); // 1 recipe + 2 nodes
        assert!(matches!(s.flat[0], FlatRow::Recipe(_)));
        assert!(matches!(s.flat[1], FlatRow::Node(_, _)));
    }

    #[test]
    fn initial_selection_lands_on_first_failed_node() {
        let s = UiState::new(mk(true), LoadDiagnostics::default());
        assert!(matches!(s.flat[s.selected], FlatRow::Node(_, _)));
        if let FlatRow::Node(_, nid) = s.flat[s.selected] {
            assert_eq!(nid, NodeId::new(0)); // first failed
        }
    }

    #[test]
    fn picker_starts_closed_and_can_be_opened() {
        let mut s = UiState::new(mk(false), LoadDiagnostics::default());
        assert!(s.picker.is_none());
        s.picker = Some(PickerState { builds: vec![], cursor: 0 });
        assert!(s.picker.is_some());
    }

    #[test]
    fn cycle_filter_failed_only_hides_passing_nodes() {
        let mut s = UiState::new(mk(false), LoadDiagnostics::default());
        s.cycle_filter(); // -> FailedOnly
        // Recipe row + only the failing node (b)
        assert_eq!(s.flat.len(), 2);
    }
}
