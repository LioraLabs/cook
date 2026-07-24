use std::collections::{BTreeSet, VecDeque};
use std::fmt;
use std::sync::atomic::{AtomicUsize, Ordering};

/// A node in the DAG, holding an arbitrary payload `T`.
///
/// Fields are crate-private to preserve invariants (e.g. `id` matches the
/// node's index in the parent `Dag`, `remaining_deps` matches the actual
/// inbound edge count). Use the accessor methods to read them.
#[derive(Debug)]
pub struct Node<T> {
    /// Unique identifier (index into the node vec).
    pub(crate) id: usize,
    /// User-supplied payload.
    pub(crate) payload: T,
    /// IDs of nodes that depend on *this* node (forward edges).
    pub(crate) dependents: Vec<usize>,
    /// Number of unsatisfied dependencies. Reaches 0 when all
    /// predecessors have been completed.
    pub(crate) remaining_deps: AtomicUsize,
}

impl<T> Node<T> {
    /// The node's identifier (its index in the owning `Dag`).
    pub fn id(&self) -> usize {
        self.id
    }

    /// Borrow the payload.
    pub fn payload(&self) -> &T {
        &self.payload
    }

    /// IDs of nodes that depend on *this* node (forward edges).
    pub fn dependents(&self) -> &[usize] {
        &self.dependents
    }

    /// Current count of unsatisfied dependencies. Reads the underlying
    /// atomic with `SeqCst`.
    pub fn remaining_deps(&self) -> usize {
        self.remaining_deps.load(Ordering::SeqCst)
    }
}

/// Errors returned by [`Dag`] mutation operations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DagError {
    /// A dependency referenced an ID that does not exist yet.
    DependencyOutOfRange {
        /// The offending dependency id.
        dep_id: usize,
        /// Number of nodes in the DAG at the time of the failed insert.
        num_nodes: usize,
    },
}

impl fmt::Display for DagError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DagError::DependencyOutOfRange { dep_id, num_nodes } => write!(
                f,
                "dependency id {dep_id} does not exist (only {num_nodes} nodes in the DAG)"
            ),
        }
    }
}

impl std::error::Error for DagError {}

/// Error returned when cycle detection finds a cycle.
///
/// `cycle_path` is a sequence of node IDs `[v_0, v_1, ..., v_k]` such that
/// each `v_i` depends on `v_{i+1}` and `v_k` depends on `v_0`, i.e. the
/// path is a closed loop with the implicit closing edge `v_k -> v_0`. The
/// path is non-empty whenever a cycle is reported.
///
/// `blocked` counts every node that could not be topologically scheduled
/// (the cycle members plus any nodes transitively downstream of the cycle).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CycleError {
    /// One concrete cycle witnessed in the graph, in dependency order.
    pub cycle_path: Vec<usize>,
    /// Number of nodes that are part of, or transitively blocked by, a cycle.
    pub blocked: usize,
}

impl fmt::Display for CycleError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.cycle_path.is_empty() {
            write!(
                f,
                "cycle detected: {} node(s) part of or blocked by a cycle",
                self.blocked
            )
        } else {
            let path = self
                .cycle_path
                .iter()
                .map(|id| id.to_string())
                .collect::<Vec<_>>()
                .join(" -> ");
            // Closing edge: last -> first
            let first = self.cycle_path[0];
            write!(
                f,
                "cycle detected: {path} -> {first} ({} node(s) part of or blocked by a cycle)",
                self.blocked
            )
        }
    }
}

impl std::error::Error for CycleError {}

/// A generic directed acyclic graph with topological traversal support.
///
/// Nodes are added with [`add_node`](Dag::add_node), specifying which
/// existing nodes a new node depends on. The DAG tracks dependency
/// counts atomically so that [`complete`](Dag::complete) can be called
/// from multiple threads without external locking.
pub struct Dag<T> {
    nodes: Vec<Node<T>>,
    /// For each node, the list of its *predecessors* (nodes it depends on).
    /// Stored separately to support cycle detection without duplicating
    /// the dependency info that is already encoded in `dependents` + `remaining_deps`.
    deps: Vec<Vec<usize>>,
}

impl<T: fmt::Debug> fmt::Debug for Dag<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Dag").field("nodes", &self.nodes).finish()
    }
}

impl<T> Default for Dag<T> {
    fn default() -> Self {
        Self {
            nodes: Vec::new(),
            deps: Vec::new(),
        }
    }
}

impl<T> Dag<T> {
    /// Create an empty DAG.
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a node with the given payload. `depends_on` lists the IDs of
    /// nodes that must complete before this node becomes ready. Duplicate
    /// IDs in `depends_on` are silently de-duplicated so the dependency
    /// count and forward-edge wiring stay consistent.
    ///
    /// Returns the new node's ID, or [`DagError::DependencyOutOfRange`] if
    /// any entry in `depends_on` references an id that does not exist yet.
    /// On error the DAG is left unchanged.
    pub fn add_node(&mut self, payload: T, depends_on: &[usize]) -> Result<usize, DagError> {
        let id = self.nodes.len();

        // Validate first; do not mutate on error.
        let mut unique_deps: BTreeSet<usize> = BTreeSet::new();
        for &dep_id in depends_on {
            if dep_id >= id {
                return Err(DagError::DependencyOutOfRange {
                    dep_id,
                    num_nodes: id,
                });
            }
            unique_deps.insert(dep_id);
        }

        let dedup: Vec<usize> = unique_deps.into_iter().collect();

        let node = Node {
            id,
            payload,
            dependents: Vec::new(),
            remaining_deps: AtomicUsize::new(dedup.len()),
        };
        self.nodes.push(node);

        // Wire forward edges: each (deduped) dependency gains this node as a dependent.
        for &dep_id in &dedup {
            self.nodes[dep_id].dependents.push(id);
        }

        self.deps.push(dedup);

        Ok(id)
    }

    /// Validate that the graph contains no cycles.
    ///
    /// Uses Kahn's algorithm for detection: repeatedly remove nodes with
    /// zero in-degree. If not every node is removed, the unconsumed
    /// sub-graph contains at least one cycle. We then walk the unconsumed
    /// predecessor edges to surface one concrete cycle path in the
    /// returned [`CycleError`].
    pub fn validate(&self) -> Result<(), CycleError> {
        let n = self.nodes.len();
        if n == 0 {
            return Ok(());
        }

        // Build in-degree counts from the stored deps.
        let mut in_degree: Vec<usize> = self.deps.iter().map(|d| d.len()).collect();

        let mut queue: VecDeque<usize> = VecDeque::new();
        for (i, &deg) in in_degree.iter().enumerate() {
            if deg == 0 {
                queue.push_back(i);
            }
        }

        let mut consumed = vec![false; n];
        let mut visited = 0usize;
        while let Some(node_id) = queue.pop_front() {
            consumed[node_id] = true;
            visited += 1;
            for &dep_id in &self.nodes[node_id].dependents {
                in_degree[dep_id] -= 1;
                if in_degree[dep_id] == 0 {
                    queue.push_back(dep_id);
                }
            }
        }

        if visited == n {
            return Ok(());
        }

        // ── extract one concrete cycle ────────────────────────────────────
        // Kahn left behind every node that is on a cycle or downstream of
        // one. Walk predecessor edges from any unconsumed node until we
        // revisit a node already on our stack — that's a cycle.
        let blocked = n - visited;
        let cycle_path = self.extract_cycle(&consumed);

        Err(CycleError {
            cycle_path,
            blocked,
        })
    }

    /// Walk predecessor edges among unconsumed nodes to surface one cycle.
    ///
    /// Returns the cycle in dependency order: `[v_0, v_1, ..., v_k]` with
    /// the implicit closing edge `v_k -> v_0`. Each `v_i` depends on the
    /// next entry. The returned vector is non-empty whenever any node is
    /// unconsumed; in the (impossible-by-construction) edge case where
    /// the walk fails to find a back-edge, an empty vec is returned.
    fn extract_cycle(&self, consumed: &[bool]) -> Vec<usize> {
        // Pick any unconsumed node as a starting point.
        let start = match consumed.iter().position(|&c| !c) {
            Some(s) => s,
            None => return Vec::new(),
        };

        // Walk one predecessor at a time, recording the path. Restrict the
        // walk to unconsumed nodes — every such node has at least one
        // unconsumed predecessor (otherwise Kahn would have removed it),
        // so the walk cannot dead-end.
        let mut path: Vec<usize> = Vec::new();
        let mut on_path: Vec<bool> = vec![false; self.nodes.len()];
        let mut current = start;
        loop {
            if on_path[current] {
                // Found the cycle. Trim the prefix that leads into it so the
                // returned vec contains only the cycle itself.
                let cut = path.iter().position(|&n| n == current).unwrap();
                let mut cycle = path.split_off(cut);
                // `path` now stores the dependency chain v_0 -> v_1 -> ...
                // -> v_k where each v_i depends on v_{i+1}. We want the
                // returned vec to read the same way, so reverse so the
                // first element depends on the second.
                cycle.reverse();
                return cycle;
            }
            on_path[current] = true;
            path.push(current);

            // Step to any unconsumed predecessor.
            let next = self.deps[current]
                .iter()
                .copied()
                .find(|&p| !consumed[p]);
            match next {
                Some(p) => current = p,
                None => {
                    // Defensive: every unconsumed node has an unconsumed
                    // predecessor by construction. Bail out if not.
                    return Vec::new();
                }
            }
        }
    }

    /// Return the IDs of all nodes whose dependencies are already satisfied
    /// (i.e. `remaining_deps == 0`).
    pub fn initial_ready(&self) -> Vec<usize> {
        self.nodes
            .iter()
            .filter(|n| n.remaining_deps.load(Ordering::SeqCst) == 0)
            .map(|n| n.id)
            .collect()
    }

    /// Mark node `id` as complete. Decrements `remaining_deps` on each
    /// dependent and returns the IDs of dependents that just became ready.
    ///
    /// Thread-safe: uses atomic operations so multiple threads can call
    /// `complete` concurrently on different node IDs without external locking.
    pub fn complete(&self, id: usize) -> Vec<usize> {
        let dependents = &self.nodes[id].dependents;
        let mut newly_ready = Vec::new();
        for &dep_id in dependents {
            let prev = self.nodes[dep_id]
                .remaining_deps
                .fetch_sub(1, Ordering::SeqCst);
            if prev == 1 {
                newly_ready.push(dep_id);
            }
        }
        newly_ready
    }

    /// Access a node by ID.
    ///
    /// # Panics
    ///
    /// Panics if `id` is out of range.
    pub fn node(&self, id: usize) -> &Node<T> {
        &self.nodes[id]
    }

    /// IDs of the nodes that `id` depends on (its predecessors), in
    /// ascending order (deduplicated at insert time).
    ///
    /// # Panics
    ///
    /// Panics if `id` is out of range, mirroring [`Dag::node`].
    pub fn deps(&self, id: usize) -> &[usize] {
        &self.deps[id]
    }

    /// Number of nodes in the DAG.
    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    /// Returns `true` if the DAG contains no nodes.
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }
}

#[cfg(test)]
#[path = "tests/dag_tests.rs"]
mod tests;
