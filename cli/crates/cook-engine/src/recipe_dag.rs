//! Recipe-level wave scheduling.
//!
//! `RecipeDag` tracks which recipes are ready to run, in-flight, or done.
//! It does **not** handle work-unit-level scheduling — that is handled by
//! `cook_dag::Dag<WorkNode>`.

use std::collections::BTreeMap;

struct RecipeDagNode {
    deps: Vec<String>,
    remaining_deps: usize,
    in_flight: bool,
    done: bool,
}

/// Recipe-level DAG for wave scheduling.
///
/// Given a map of `recipe_name -> [dependency_names]`, this struct tracks
/// which recipes are ready to run (all deps satisfied), which are in-flight,
/// and which are done. Call [`pop_ready`] to get the next wave, then
/// [`mark_done`] when recipes complete.
pub struct RecipeDag {
    nodes: BTreeMap<String, RecipeDagNode>,
}

impl RecipeDag {
    pub fn new(dep_edges: &BTreeMap<String, Vec<String>>) -> Self {
        let nodes = dep_edges
            .iter()
            .map(|(name, deps)| {
                (
                    name.clone(),
                    RecipeDagNode {
                        remaining_deps: deps.len(),
                        deps: deps.clone(),
                        in_flight: false,
                        done: false,
                    },
                )
            })
            .collect();
        RecipeDag { nodes }
    }

    /// Return all recipes with no remaining deps that aren't in-flight or done.
    /// Marks returned recipes as in-flight.
    pub fn pop_ready(&mut self) -> Vec<String> {
        let ready: Vec<String> = self
            .nodes
            .iter()
            .filter(|(_, node)| node.remaining_deps == 0 && !node.in_flight && !node.done)
            .map(|(name, _)| name.clone())
            .collect();

        for name in &ready {
            if let Some(node) = self.nodes.get_mut(name) {
                node.in_flight = true;
            }
        }

        ready
    }

    /// Mark recipes as done and decrement remaining_deps on their dependents.
    pub fn mark_done(&mut self, names: &[String]) {
        let done_set: std::collections::HashSet<&str> =
            names.iter().map(|s| s.as_str()).collect();

        for name in names {
            if let Some(node) = self.nodes.get_mut(name) {
                node.done = true;
                node.in_flight = false;
            }
        }

        // Decrement remaining_deps for any node that depends on a completed recipe
        for (_, node) in self.nodes.iter_mut() {
            if node.done {
                continue;
            }
            for dep in &node.deps {
                if done_set.contains(dep.as_str()) {
                    node.remaining_deps = node.remaining_deps.saturating_sub(1);
                }
            }
        }
    }
}

#[cfg(test)]
#[path = "tests/recipe_dag_tests.rs"]
mod tests;
