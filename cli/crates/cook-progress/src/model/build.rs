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
            ProgressEvent::RecipeCompleted { recipe, elapsed, cached, total } => {
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
            ProgressEvent::NodeStarted { recipe, node, name, artifact, fallback_label } => {
                if let Some(r) = self.recipes.get_mut(recipe) {
                    let mut ns = NodeState::new(*node, name.clone(), artifact.clone(), fallback_label.clone());
                    ns.status = NodeStatus::Running;
                    ns.started_at = Some(Instant::now());
                    r.nodes.insert(*node, ns);
                }
            }
            ProgressEvent::NodeCompleted { recipe, node, elapsed: _ } => {
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
            ProgressEvent::NodeCacheHit { recipe, node, name, artifact } => {
                if let Some(r) = self.recipes.get_mut(recipe) {
                    use std::collections::btree_map::Entry;
                    if let Entry::Vacant(e) = r.nodes.entry(*node) {
                        let mut ns = NodeState::new(*node, name.clone(), artifact.clone(), String::new());
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
                        });
                        r.progress.0 += 1;
                        self.totals.completed_nodes += 1;
                    }
                }
            }
            ProgressEvent::NodeOutput { .. } => { /* log store handles this */ }
            ProgressEvent::InteractiveStart { .. } => {}
            ProgressEvent::InteractiveEnd { .. } => {}
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

impl Default for BuildState {
    fn default() -> Self { Self::new() }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::NodeId;
    use std::path::PathBuf;

    fn topo(recipes: &[(u32, &str, &[u32], usize)]) -> Vec<RecipeTopo> {
        recipes.iter().map(|(id, name, deps, n)| RecipeTopo {
            id: RecipeId::new(*id),
            name: (*name).to_string(),
            deps: deps.iter().map(|d| RecipeId::new(*d)).collect(),
            expected_nodes: *n,
        }).collect()
    }

    #[test]
    fn build_started_seeds_recipes_in_topo_order() {
        let mut s = BuildState::new();
        s.apply(&ProgressEvent::BuildStarted {
            recipes: topo(&[(0, "deps", &[], 12), (1, "lib", &[0], 6)]),
            total_nodes: 18,
        });
        assert_eq!(s.order, vec![RecipeId::new(0), RecipeId::new(1)]);
        assert_eq!(s.recipes.len(), 2);
        assert_eq!(s.totals.waiting, 2);
        assert_eq!(s.totals.total_nodes, 18);
    }

    #[test]
    fn recipe_started_transitions_waiting_to_running() {
        let mut s = BuildState::new();
        s.apply(&ProgressEvent::BuildStarted {
            recipes: topo(&[(0, "deps", &[], 2)]), total_nodes: 2,
        });
        s.apply(&ProgressEvent::RecipeStarted { recipe: RecipeId::new(0) });
        assert_eq!(s.recipes[&RecipeId::new(0)].status, Status::Running);
        assert_eq!(s.totals.running, 1);
        assert_eq!(s.totals.waiting, 0);
    }

    #[test]
    fn node_started_inserts_running_node() {
        let mut s = BuildState::new();
        s.apply(&ProgressEvent::BuildStarted {
            recipes: topo(&[(0, "lib", &[], 1)]), total_nodes: 1,
        });
        s.apply(&ProgressEvent::NodeStarted {
            recipe: RecipeId::new(0),
            node: NodeId::new(0),
            name: "lvm.c".into(),
            artifact: Some(PathBuf::from("build/obj/lvm.o")),
            fallback_label: "clang -c lvm.c".into(),
        });
        let r = &s.recipes[&RecipeId::new(0)];
        assert_eq!(r.nodes.len(), 1);
        assert_eq!(r.nodes[&NodeId::new(0)].status, NodeStatus::Running);
    }

    #[test]
    fn cache_hit_increments_counter_and_progress() {
        let mut s = BuildState::new();
        s.apply(&ProgressEvent::BuildStarted {
            recipes: topo(&[(0, "deps", &[], 3)]), total_nodes: 3,
        });
        s.apply(&ProgressEvent::NodeCacheHit {
            recipe: RecipeId::new(0), node: NodeId::new(0),
            name: "a".into(), artifact: None,
        });
        let r = &s.recipes[&RecipeId::new(0)];
        assert_eq!(r.cached_count, 1);
        assert_eq!(r.progress, (1, 3));
        assert_eq!(s.totals.completed_nodes, 1);
    }

    #[test]
    fn recipe_completed_marks_cached_when_all_cached() {
        let mut s = BuildState::new();
        s.apply(&ProgressEvent::BuildStarted {
            recipes: topo(&[(0, "deps", &[], 2)]), total_nodes: 2,
        });
        s.apply(&ProgressEvent::RecipeStarted { recipe: RecipeId::new(0) });
        s.apply(&ProgressEvent::RecipeCompleted {
            recipe: RecipeId::new(0),
            elapsed: Duration::from_millis(10),
            cached: 2, total: 2,
        });
        assert_eq!(s.recipes[&RecipeId::new(0)].status, Status::Cached);
        assert_eq!(s.totals.cached, 1);
    }

    #[test]
    fn recipe_failed_records_first_error_summary() {
        let mut s = BuildState::new();
        s.apply(&ProgressEvent::BuildStarted {
            recipes: topo(&[(0, "lib", &[], 1)]), total_nodes: 1,
        });
        s.apply(&ProgressEvent::RecipeStarted { recipe: RecipeId::new(0) });
        s.apply(&ProgressEvent::NodeStarted {
            recipe: RecipeId::new(0), node: NodeId::new(0),
            name: "x".into(), artifact: None, fallback_label: "x".into(),
        });
        s.apply(&ProgressEvent::NodeFailed {
            recipe: RecipeId::new(0), node: NodeId::new(0),
            elapsed: Duration::from_millis(10),
            error: "boom".into(),
        });
        s.apply(&ProgressEvent::RecipeFailed {
            recipe: RecipeId::new(0),
            elapsed: Duration::from_millis(20),
            completed: 1, total: 1,
        });
        let r = &s.recipes[&RecipeId::new(0)];
        assert_eq!(r.status, Status::Failed);
        assert_eq!(r.error_summary.as_deref(), Some("boom"));
    }

    #[test]
    fn duplicate_recipe_completed_does_not_double_count_counters() {
        let mut s = BuildState::new();
        s.apply(&ProgressEvent::BuildStarted {
            recipes: topo(&[(0, "deps", &[], 1)]), total_nodes: 1,
        });
        s.apply(&ProgressEvent::RecipeStarted { recipe: RecipeId::new(0) });
        s.apply(&ProgressEvent::RecipeCompleted {
            recipe: RecipeId::new(0),
            elapsed: Duration::from_millis(10),
            cached: 0, total: 1,
        });
        let totals_after_first = s.totals;
        s.apply(&ProgressEvent::RecipeCompleted {
            recipe: RecipeId::new(0),
            elapsed: Duration::from_millis(10),
            cached: 0, total: 1,
        });
        assert_eq!(s.totals, totals_after_first, "duplicate RecipeCompleted must not mutate counters");
    }

    #[test]
    fn duplicate_node_completed_does_not_double_count_progress() {
        let mut s = BuildState::new();
        s.apply(&ProgressEvent::BuildStarted {
            recipes: topo(&[(0, "lib", &[], 2)]), total_nodes: 2,
        });
        s.apply(&ProgressEvent::RecipeStarted { recipe: RecipeId::new(0) });
        s.apply(&ProgressEvent::NodeStarted {
            recipe: RecipeId::new(0), node: NodeId::new(0),
            name: "a".into(), artifact: None, fallback_label: "a".into(),
        });
        s.apply(&ProgressEvent::NodeCompleted {
            recipe: RecipeId::new(0), node: NodeId::new(0),
            elapsed: Duration::from_millis(1),
        });
        assert_eq!(s.recipes[&RecipeId::new(0)].progress, (1, 2));
        assert_eq!(s.totals.completed_nodes, 1);

        // Duplicate — must not advance progress.
        s.apply(&ProgressEvent::NodeCompleted {
            recipe: RecipeId::new(0), node: NodeId::new(0),
            elapsed: Duration::from_millis(1),
        });
        assert_eq!(s.recipes[&RecipeId::new(0)].progress, (1, 2));
        assert_eq!(s.totals.completed_nodes, 1);
    }
}
