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
mod tests {
    use super::*;

    // ── empty dag ──────────────────────────────────────────────────────

    #[test]
    fn empty_dag() {
        let dag: Dag<&str> = Dag::new();
        assert!(dag.is_empty());
        assert_eq!(dag.len(), 0);
        assert!(dag.initial_ready().is_empty());
        assert!(dag.validate().is_ok());
    }

    // ── single node ────────────────────────────────────────────────────

    #[test]
    fn single_node_is_initially_ready() {
        let mut dag = Dag::new();
        let id = dag.add_node("only", &[]).unwrap();
        assert_eq!(id, 0);
        assert_eq!(dag.len(), 1);
        assert!(!dag.is_empty());

        let ready = dag.initial_ready();
        assert_eq!(ready, vec![0]);
        assert!(dag.validate().is_ok());
    }

    // ── linear chain ───────────────────────────────────────────────────

    #[test]
    fn linear_chain_a_b_c() {
        let mut dag = Dag::new();
        let a = dag.add_node("a", &[]).unwrap();
        let b = dag.add_node("b", &[a]).unwrap();
        let c = dag.add_node("c", &[b]).unwrap();

        // Only a is initially ready.
        assert_eq!(dag.initial_ready(), vec![a]);

        // Complete a -> b becomes ready.
        assert_eq!(dag.complete(a), vec![b]);

        // Complete b -> c becomes ready.
        assert_eq!(dag.complete(b), vec![c]);

        // Complete c -> nothing new.
        assert!(dag.complete(c).is_empty());

        assert!(dag.validate().is_ok());
    }

    // ── diamond pattern ────────────────────────────────────────────────

    #[test]
    fn diamond_a_bc_d() {
        //   a
        //  / \
        // b   c
        //  \ /
        //   d
        let mut dag = Dag::new();
        let a = dag.add_node("a", &[]).unwrap();
        let b = dag.add_node("b", &[a]).unwrap();
        let c = dag.add_node("c", &[a]).unwrap();
        let d = dag.add_node("d", &[b, c]).unwrap();

        assert_eq!(dag.initial_ready(), vec![a]);

        // Complete a -> b and c become ready.
        let mut ready = dag.complete(a);
        ready.sort();
        assert_eq!(ready, vec![b, c]);

        // Complete b -> d still blocked on c.
        assert!(dag.complete(b).is_empty());
        assert_eq!(dag.node(d).remaining_deps(), 1);

        // Complete c -> d is now ready.
        assert_eq!(dag.complete(c), vec![d]);
        assert_eq!(dag.node(d).remaining_deps(), 0);

        assert!(dag.validate().is_ok());
    }

    // ── parallel roots ─────────────────────────────────────────────────

    #[test]
    fn parallel_roots() {
        let mut dag = Dag::new();
        let r0 = dag.add_node("root0", &[]).unwrap();
        let r1 = dag.add_node("root1", &[]).unwrap();
        let r2 = dag.add_node("root2", &[]).unwrap();

        let mut ready = dag.initial_ready();
        ready.sort();
        assert_eq!(ready, vec![r0, r1, r2]);
        assert_eq!(dag.len(), 3);
        assert!(dag.validate().is_ok());
    }

    // ── node access by ID ──────────────────────────────────────────────

    #[test]
    fn node_access_by_id() {
        let mut dag = Dag::new();
        dag.add_node("alpha", &[]).unwrap();
        dag.add_node("beta", &[0]).unwrap();
        dag.add_node("gamma", &[0, 1]).unwrap();

        assert_eq!(*dag.node(0).payload(), "alpha");
        assert_eq!(*dag.node(1).payload(), "beta");
        assert_eq!(*dag.node(2).payload(), "gamma");
        assert_eq!(dag.node(0).id(), 0);

        // Check dependents wiring.
        assert_eq!(dag.node(0).dependents(), &[1, 2][..]);
        assert_eq!(dag.node(1).dependents(), &[2][..]);
        assert!(dag.node(2).dependents().is_empty());

        // Check remaining deps.
        assert_eq!(dag.node(0).remaining_deps(), 0);
        assert_eq!(dag.node(1).remaining_deps(), 1);
        assert_eq!(dag.node(2).remaining_deps(), 2);
    }

    // ── cycle detection (two-node loop) ────────────────────────────────

    #[test]
    fn cycle_detection_two_node_loop() {
        // A real two-node cycle cannot be built through `add_node` (which
        // forbids forward references). Construct the deps directly via the
        // crate-private fields to exercise validate().
        let mut dag: Dag<&str> = Dag::new();
        // Insert two nodes with no deps via add_node.
        let a = dag.add_node("a", &[]).unwrap();
        let b = dag.add_node("b", &[]).unwrap();
        // Manually wire a cycle: a depends on b, b depends on a.
        dag.deps[a] = vec![b];
        dag.deps[b] = vec![a];
        dag.nodes[a].dependents = vec![b];
        dag.nodes[b].dependents = vec![a];
        dag.nodes[a].remaining_deps.store(1, Ordering::SeqCst);
        dag.nodes[b].remaining_deps.store(1, Ordering::SeqCst);

        let err = dag.validate().unwrap_err();
        assert_eq!(err.blocked, 2);
        assert_eq!(err.cycle_path.len(), 2);
        // Path must contain both nodes.
        assert!(err.cycle_path.contains(&a));
        assert!(err.cycle_path.contains(&b));
        // Each consecutive pair must be a real edge: cycle_path[i] depends
        // on cycle_path[i+1] (and last depends on first).
        for i in 0..err.cycle_path.len() {
            let from = err.cycle_path[i];
            let to = err.cycle_path[(i + 1) % err.cycle_path.len()];
            assert!(
                dag.deps[from].contains(&to),
                "cycle edge {from} -> {to} not in deps"
            );
        }
        let msg = format!("{err}");
        assert!(msg.contains("cycle detected"));
        assert!(msg.contains("->"));
    }

    // ── cycle detection (longer loop with tail) ────────────────────────

    #[test]
    fn cycle_detection_with_blocked_tail() {
        // Build:  0 -> 1 -> 2 -> 1 (cycle) and 3 depends on 2 (blocked).
        let mut dag: Dag<&str> = Dag::new();
        let n0 = dag.add_node("n0", &[]).unwrap();
        let n1 = dag.add_node("n1", &[n0]).unwrap();
        let n2 = dag.add_node("n2", &[n1]).unwrap();
        let n3 = dag.add_node("n3", &[n2]).unwrap();
        // Add the back-edge n1 -> n2 (i.e. n1 depends on n2).
        dag.deps[n1].push(n2);
        dag.nodes[n2].dependents.push(n1);
        dag.nodes[n1].remaining_deps.fetch_add(1, Ordering::SeqCst);

        let err = dag.validate().unwrap_err();
        // n1, n2, n3 are blocked (n0 still drains).
        assert_eq!(err.blocked, 3);
        // Cycle should be {n1, n2}.
        assert_eq!(err.cycle_path.len(), 2);
        assert!(err.cycle_path.contains(&n1));
        assert!(err.cycle_path.contains(&n2));
        for i in 0..err.cycle_path.len() {
            let from = err.cycle_path[i];
            let to = err.cycle_path[(i + 1) % err.cycle_path.len()];
            assert!(dag.deps[from].contains(&to));
        }
        // n3 is not part of the cycle (just blocked by it).
        assert!(!err.cycle_path.contains(&n3));
    }

    // ── add_node returns Err on bad dep ────────────────────────────────

    #[test]
    fn add_node_rejects_out_of_range_dep() {
        let mut dag: Dag<&str> = Dag::new();
        let err = dag.add_node("first", &[5]).unwrap_err();
        assert_eq!(
            err,
            DagError::DependencyOutOfRange {
                dep_id: 5,
                num_nodes: 0,
            }
        );
        // DAG must be untouched.
        assert!(dag.is_empty());
    }

    #[test]
    fn add_node_rejects_self_reference() {
        // Self-reference would be id == nodes.len() before insert, so it
        // is caught by the same range check.
        let mut dag: Dag<&str> = Dag::new();
        let err = dag.add_node("a", &[0]).unwrap_err();
        assert!(matches!(
            err,
            DagError::DependencyOutOfRange { dep_id: 0, num_nodes: 0 }
        ));
    }

    // ── add_node dedupes duplicate deps ────────────────────────────────

    #[test]
    fn add_node_dedupes_duplicate_deps() {
        let mut dag = Dag::new();
        let a = dag.add_node("a", &[]).unwrap();
        // List `a` three times — should be wired exactly once.
        let b = dag.add_node("b", &[a, a, a]).unwrap();

        assert_eq!(dag.node(b).remaining_deps(), 1);
        assert_eq!(dag.node(a).dependents(), &[b][..]);

        // Completing `a` should release `b` exactly once.
        let newly_ready = dag.complete(a);
        assert_eq!(newly_ready, vec![b]);
        assert_eq!(dag.node(b).remaining_deps(), 0);
    }

    // ── large fan-out ──────────────────────────────────────────────────

    #[test]
    fn large_fan_out() {
        let mut dag = Dag::new();
        let root = dag.add_node("root".to_string(), &[]).unwrap();

        let fan_size = 100;
        let mut children = Vec::with_capacity(fan_size);
        for i in 0..fan_size {
            let child = dag.add_node(format!("child-{i}"), &[root]).unwrap();
            children.push(child);
        }

        assert_eq!(dag.len(), fan_size + 1);
        assert_eq!(dag.initial_ready(), vec![root]);

        // Complete root -> all children become ready.
        let mut newly_ready = dag.complete(root);
        newly_ready.sort();
        assert_eq!(newly_ready, children);
    }

    // ── Default impl ───────────────────────────────────────────────────

    #[test]
    fn default_impl() {
        let dag: Dag<i32> = Dag::default();
        assert!(dag.is_empty());
        assert_eq!(dag.len(), 0);
    }

    // ── Send + Sync ────────────────────────────────────────────────────

    #[test]
    fn dag_is_send_and_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<Dag<String>>();
        assert_send_sync::<Node<String>>();
    }
}
