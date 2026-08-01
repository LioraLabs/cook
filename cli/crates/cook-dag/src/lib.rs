use std::collections::BTreeSet;
use std::fmt;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

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
    /// Set by the first [`Dag::complete`] for this node (COOK-400).
    ///
    /// The decrement below is unsigned and wraps, so a second completion used
    /// to take a dependent's counter through zero and release it while a real
    /// predecessor was still running. The guard makes the corruption
    /// impossible here rather than relying on every caller to avoid it.
    pub(crate) completed: AtomicBool,
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
    ///
    /// # Why there is no cycle check (COOK-400)
    ///
    /// This rejects any `dep_id >= id`, and it is the only `&mut self` method,
    /// so every edge points to a strictly smaller id. Insertion order IS a
    /// topological order and a `Dag` built through this API cannot contain a
    /// cycle. The crate used to carry Kahn's algorithm, a cycle-path
    /// extractor and a `CycleError` (about a third of this file) to detect one
    /// anyway; none of it could fire, and both of its tests had to reach into
    /// the private `deps`/`nodes` vectors to forge a cycle. It was deleted
    /// rather than kept as reassurance.
    ///
    /// The cost is that forward references are not expressible: a node must be
    /// added after everything it depends on. If that ever needs to change,
    /// this check is what relaxes, and cycle detection has to come back with
    /// it.
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
            completed: AtomicBool::new(false),
        };
        self.nodes.push(node);

        // Wire forward edges: each (deduped) dependency gains this node as a dependent.
        for &dep_id in &dedup {
            self.nodes[dep_id].dependents.push(id);
        }

        self.deps.push(dedup);

        Ok(id)
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
    /// Completing a node more than once does nothing and returns an empty
    /// vec: only the first call for a given `id` decrements anything.
    ///
    /// Thread-safe: uses atomic operations so multiple threads can call
    /// `complete` concurrently on different node IDs without external locking.
    /// The once-only guard is a `swap`, so two threads racing on the SAME id
    /// still produce exactly one set of decrements.
    ///
    /// # The guard (COOK-400)
    ///
    /// `remaining_deps` is unsigned and the decrement wraps. A second
    /// completion therefore took a dependent's counter from 0 to `usize::MAX`
    /// and, for a two-dependency node, made `prev == 1` fire on the *first*
    /// real predecessor, releasing the node while the second was still
    /// running. Not a panic: a node scheduled early, with whatever
    /// corruption that produced downstream.
    ///
    /// That invariant used to be owned by the eight `dag.complete(` sites in
    /// `cook-engine`'s executor. It is owned here now, so no caller can
    /// reintroduce it.
    ///
    /// Deliberately not a panic, and not a `debug_assert` either. A repeat
    /// completion is a scheduler bug, but a build tool that aborts on an
    /// internal invariant serves its user worse than one that absorbs it and
    /// finishes. A caller that wants to detect the condition has
    /// [`is_completed`](Dag::is_completed) and the empty return.
    pub fn complete(&self, id: usize) -> Vec<usize> {
        if self.nodes[id].completed.swap(true, Ordering::SeqCst) {
            return Vec::new();
        }
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

    /// Whether [`complete`](Dag::complete) has already been called for `id`.
    ///
    /// # Panics
    ///
    /// Panics if `id` is out of range, mirroring [`Dag::node`].
    pub fn is_completed(&self, id: usize) -> bool {
        self.nodes[id].completed.load(Ordering::SeqCst)
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
