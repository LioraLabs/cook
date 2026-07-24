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
    pub editing: bool, // true while user is typing; false after Enter
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
    pub tree_scroll: usize,
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
            tree_scroll: 0,
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
        self.tree_scroll = 0;
    }

    pub fn toggle_fold(&mut self) {
        if let Some(FlatRow::Recipe(rid)) = self.flat.get(self.selected).copied() {
            if !self.expanded.remove(&rid) {
                self.expanded.insert(rid);
            }
            self.rebuild_flat();
        }
    }

    /// Adjust `tree_scroll` so `selected` sits inside the visible window.
    /// Sticky: scroll only changes when the selection would otherwise leave
    /// the viewport. No-op when `available_rows == 0`.
    pub fn ensure_tree_visible(&mut self, available_rows: usize) {
        if available_rows == 0 {
            return;
        }
        if self.selected < self.tree_scroll {
            self.tree_scroll = self.selected;
        } else if self.selected >= self.tree_scroll + available_rows {
            self.tree_scroll = self.selected + 1 - available_rows;
        }
        let max_scroll = self.flat.len().saturating_sub(available_rows);
        self.tree_scroll = self.tree_scroll.min(max_scroll);
    }

    pub fn set_search_pattern(&mut self, pat: String) {
        let mut matches = Vec::new();
        let needle = pat.to_lowercase();
        if !needle.is_empty() {
            for (rid, recipe) in &self.view.recipes {
                for (nid, node) in &recipe.nodes {
                    for (i, line) in node.lines.iter().enumerate() {
                        if line.text.to_lowercase().contains(&needle) {
                            matches.push((*rid, *nid, i));
                        }
                    }
                }
            }
        }
        self.search = Some(SearchState { pattern: pat, matches, cursor: 0, editing: false });
        self.jump_to_current_match();
    }

    pub fn jump_to_next_match(&mut self, dir: i32) {
        let len_opt = self.search.as_ref().map(|s| s.matches.len());
        let Some(len) = len_opt else { return };
        if len == 0 { return; }
        if let Some(s) = self.search.as_mut() {
            let len_i = len as i32;
            s.cursor = ((s.cursor as i32 + dir).rem_euclid(len_i)) as usize;
        }
        self.jump_to_current_match();
    }

    fn jump_to_current_match(&mut self) {
        let target = self.search.as_ref()
            .and_then(|s| s.matches.get(s.cursor).copied());
        let Some((rid, nid, line_idx)) = target else { return };
        if let Some(pos) = self.flat.iter().position(|r| {
            matches!(r, FlatRow::Node(r1, n1) if *r1 == rid && *n1 == nid)
        }) {
            self.selected = pos;
        }
        self.scroll_y = line_idx as u16;
    }
}

#[cfg(test)]
#[path = "tests/state_tests.rs"]
mod tests;
