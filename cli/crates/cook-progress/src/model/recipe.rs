//! RecipeState — live per-recipe status.

use std::collections::BTreeMap;
use std::time::Duration;

use crate::event::{NodeId, RecipeId, SkipReason};
use crate::model::node::NodeState;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Waiting,
    Running,
    Completed,
    Failed,
    Skipped,
    Cached,
}

#[derive(Debug, Clone)]
pub struct RecipeState {
    pub id: RecipeId,
    pub name: String,
    pub deps: Vec<RecipeId>,
    pub status: Status,
    pub progress: (usize, usize),
    pub elapsed: Option<Duration>,
    pub nodes: BTreeMap<NodeId, NodeState>,
    pub cached_count: usize,
    pub skipped: Vec<(NodeId, SkipReason)>,
    pub error_summary: Option<String>,
}

impl RecipeState {
    pub fn new(id: RecipeId, name: String, deps: Vec<RecipeId>, expected_nodes: usize) -> Self {
        Self {
            id,
            name,
            deps,
            status: Status::Waiting,
            progress: (0, expected_nodes),
            elapsed: None,
            nodes: BTreeMap::new(),
            cached_count: 0,
            skipped: Vec::new(),
            error_summary: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_recipe_is_waiting_with_zero_progress() {
        let r = RecipeState::new(RecipeId::new(0), "deps".into(), vec![], 12);
        assert_eq!(r.status, Status::Waiting);
        assert_eq!(r.progress, (0, 12));
        assert!(r.nodes.is_empty());
        assert_eq!(r.cached_count, 0);
        assert!(r.error_summary.is_none());
    }
}
