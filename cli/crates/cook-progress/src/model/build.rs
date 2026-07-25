//! BuildState — the pure state machine. ProgressEvent is the only input.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use crate::event::{ProgressEvent, RecipeId, RecipeTopo};
use crate::model::node::{NodeState, NodeStatus};
use crate::model::recipe::{RecipeState, Status};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Counters {
    pub done: usize,
    pub running: usize,
    pub waiting: usize,
    pub cached: usize,
    pub total_nodes: usize,
    pub completed_nodes: usize,
}

#[derive(Debug, Clone)]
pub struct BuildState {
    pub order: Vec<RecipeId>,
    pub recipes: BTreeMap<RecipeId, RecipeState>,
    pub started_at: Option<Instant>,
    pub totals: Counters,
    pub finished: Option<bool>,
}

impl BuildState {
    pub fn new() -> Self {
        Self {
            order: Vec::new(),
            recipes: BTreeMap::new(),
            started_at: None,
            totals: Counters::default(),
            finished: None,
        }
    }

    pub fn apply(&mut self, event: &ProgressEvent) {
        match event {
            ProgressEvent::BuildStarted { recipes, total_nodes } => {
                self.ingest_topology(recipes, *total_nodes);
            }
            ProgressEvent::RecipeStarted { recipe } => {
                if let Some(r) = self.recipes.get_mut(recipe) {
                    if r.status == Status::Waiting {
                        r.status = Status::Running;
                        self.totals.waiting = self.totals.waiting.saturating_sub(1);
                        self.totals.running += 1;
                    }
                }
            }
            ProgressEvent::RecipeCompleted { recipe, elapsed, cached, total, .. } => {
                if let Some(r) = self.recipes.get_mut(recipe) {
                    let was_running = r.status == Status::Running;
                    r.elapsed = Some(*elapsed);
                    r.progress = (*total, *total);
                    r.status = if *cached == *total && *total > 0 { Status::Cached } else { Status::Completed };
                    if was_running {
                        self.totals.running = self.totals.running.saturating_sub(1);
                        self.totals.done += 1;
                        if r.status == Status::Cached { self.totals.cached += 1; }
                    }
                }
            }
            ProgressEvent::RecipeFailed { recipe, elapsed, completed, total } => {
                if let Some(r) = self.recipes.get_mut(recipe) {
                    let was_running = r.status == Status::Running;
                    r.elapsed = Some(*elapsed);
                    r.progress = (*completed, *total);
                    r.status = Status::Failed;
                    if was_running {
                        self.totals.running = self.totals.running.saturating_sub(1);
                        self.totals.done += 1;
                    }
                }
            }
            ProgressEvent::RecipeSkipped { recipe, elapsed, completed, total, .. } => {
                if let Some(r) = self.recipes.get_mut(recipe) {
                    let was_running = r.status == Status::Running;
                    r.elapsed = Some(*elapsed);
                    r.progress = (*completed, *total);
                    r.status = Status::Skipped;
                    if was_running {
                        self.totals.running = self.totals.running.saturating_sub(1);
                    }
                }
            }
            ProgressEvent::NodeStarted { recipe, node, name, artifact, fallback_label, kind, cause: _ } => {
                if let Some(r) = self.recipes.get_mut(recipe) {
                    let mut ns = NodeState::new(*node, name.clone(), artifact.clone(), fallback_label.clone());
                    ns.status = NodeStatus::Running;
                    ns.started_at = Some(Instant::now());
                    ns.kind = *kind;
                    r.nodes.insert(*node, ns);
                }
            }
            ProgressEvent::NodeCompleted { recipe, node, elapsed: _, kind: _, cache_key: _ } => {
                if let Some(r) = self.recipes.get_mut(recipe) {
                    let bumped = if let Some(n) = r.nodes.get_mut(node) {
                        if n.status == NodeStatus::Running {
                            n.status = NodeStatus::Completed;
                            n.completed_at = Some(Instant::now());
                            true
                        } else {
                            false
                        }
                    } else {
                        false
                    };
                    if bumped {
                        r.progress.0 += 1;
                        self.totals.completed_nodes += 1;
                    }
                }
            }
            ProgressEvent::NodeFailed { recipe, node, elapsed: _, error } => {
                if let Some(r) = self.recipes.get_mut(recipe) {
                    let bumped = if let Some(n) = r.nodes.get_mut(node) {
                        if n.status == NodeStatus::Running {
                            n.status = NodeStatus::Failed;
                            n.completed_at = Some(Instant::now());
                            true
                        } else {
                            false
                        }
                    } else {
                        false
                    };
                    if bumped {
                        r.progress.0 += 1;
                        if r.error_summary.is_none() {
                            r.error_summary = Some(error.clone());
                        }
                        self.totals.completed_nodes += 1;
                    }
                }
            }
            ProgressEvent::NodeCacheHit { recipe, node, name, artifact, kind } => {
                if let Some(r) = self.recipes.get_mut(recipe) {
                    use std::collections::btree_map::Entry;
                    if let Entry::Vacant(e) = r.nodes.entry(*node) {
                        let mut ns = NodeState::new(*node, name.clone(), artifact.clone(), name.clone());
                        ns.kind = *kind;
                        ns.status = NodeStatus::Completed;
                        ns.completed_at = Some(Instant::now());
                        e.insert(ns);
                        r.cached_count += 1;
                        r.progress.0 += 1;
                        self.totals.completed_nodes += 1;
                    }
                }
            }
            ProgressEvent::NodeSkipped { recipe, node, name, reason } => {
                if let Some(r) = self.recipes.get_mut(recipe) {
                    use std::collections::btree_map::Entry;
                    if let Entry::Vacant(e) = r.nodes.entry(*node) {
                        r.skipped.push((*node, *reason));
                        e.insert(NodeState {
                            id: *node,
                            name: name.clone(),
                            artifact: None,
                            fallback_label: name.clone(),
                            status: NodeStatus::Skipped,
                            started_at: None,
                            completed_at: Some(Instant::now()),
                            kind: crate::event::NodeKind::Cooked,
                        });
                        r.progress.0 += 1;
                        self.totals.completed_nodes += 1;
                    }
                }
            }
            ProgressEvent::NodeOutput { .. } => { /* log store handles this */ }
            ProgressEvent::InteractiveStart { recipe, node, name, .. } => {
                // Register the interactive node so downstream renderers
                // (notably the JSON writer) can resolve its name from
                // BuildState rather than re-reading the inline `name` field.
                if let Some(r) = self.recipes.get_mut(recipe) {
                    use std::collections::btree_map::Entry;
                    if let Entry::Vacant(e) = r.nodes.entry(*node) {
                        let mut ns = NodeState::new(*node, name.clone(), None, name.clone());
                        ns.status = NodeStatus::Running;
                        ns.started_at = Some(Instant::now());
                        e.insert(ns);
                    }
                }
            }
            ProgressEvent::InteractiveEnd { recipe, node, name, .. } => {
                if let Some(r) = self.recipes.get_mut(recipe) {
                    use std::collections::btree_map::Entry;
                    match r.nodes.entry(*node) {
                        Entry::Occupied(mut e) => {
                            let n = e.get_mut();
                            if n.status == NodeStatus::Running {
                                n.status = NodeStatus::Completed;
                                n.completed_at = Some(Instant::now());
                            }
                        }
                        Entry::Vacant(e) => {
                            // Late InteractiveEnd without a matching Start —
                            // keep the name available for renderers anyway.
                            let mut ns = NodeState::new(*node, name.clone(), None, name.clone());
                            ns.status = NodeStatus::Completed;
                            ns.completed_at = Some(Instant::now());
                            e.insert(ns);
                        }
                    }
                }
            }
            ProgressEvent::Finished { success } => {
                self.finished = Some(*success);
            }
        }
    }

    fn ingest_topology(&mut self, recipes: &[RecipeTopo], total_nodes: usize) {
        self.order = recipes.iter().map(|r| r.id).collect();
        for topo in recipes {
            self.recipes.insert(
                topo.id,
                RecipeState::new(topo.id, topo.name.clone(), topo.deps.clone(), topo.expected_nodes),
            );
        }
        self.totals.waiting = recipes.len();
        self.totals.total_nodes = total_nodes;
        self.started_at = Some(Instant::now());
    }

    pub fn elapsed(&self) -> Duration {
        self.started_at.map(|t| t.elapsed()).unwrap_or_default()
    }
}

impl Counters {
    pub fn cached_node_count(&self, state: &BuildState) -> usize {
        state.recipes.values().map(|r| r.cached_count).sum()
    }
    pub fn failed_node_count(&self, state: &BuildState) -> usize {
        state.recipes.values()
            .flat_map(|r| r.nodes.values())
            .filter(|n| matches!(n.status, crate::model::node::NodeStatus::Failed))
            .count()
    }
    pub fn skipped_node_count(&self, state: &BuildState) -> usize {
        state.recipes.values().map(|r| r.skipped.len()).sum()
    }
}

impl Default for BuildState {
    fn default() -> Self { Self::new() }
}

#[cfg(test)]
#[path = "tests/build_tests.rs"]
mod tests;
