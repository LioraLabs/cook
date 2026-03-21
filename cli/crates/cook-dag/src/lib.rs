use std::collections::VecDeque;
use std::fmt;
use std::sync::atomic::{AtomicUsize, Ordering};

/// A node in the DAG, holding an arbitrary payload `T`.
#[derive(Debug)]
pub struct Node<T> {
    /// Unique identifier (index into the node vec).
    pub id: usize,
    /// User-supplied payload.
    pub payload: T,
    /// IDs of nodes that depend on *this* node (forward edges).
    pub dependents: Vec<usize>,
    /// Number of unsatisfied dependencies. Reaches 0 when all
    /// predecessors have been completed.
    pub remaining_deps: AtomicUsize,
}

/// Error returned when cycle detection finds a cycle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CycleError {
    /// Human-readable description of the cycle.
    pub message: String,
}

impl fmt::Display for CycleError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "cycle detected: {}", self.message)
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
    /// nodes that must complete before this node becomes ready.
    ///
    /// Returns the new node's ID.
    ///
    /// # Panics
    ///
    /// Panics if any ID in `depends_on` is out of range.
    pub fn add_node(&mut self, payload: T, depends_on: &[usize]) -> usize {
        let id = self.nodes.len();

        for &dep_id in depends_on {
            assert!(
                dep_id < id,
                "dependency id {dep_id} does not exist (only {id} nodes in the DAG)"
            );
        }

        let node = Node {
            id,
            payload,
            dependents: Vec::new(),
            remaining_deps: AtomicUsize::new(depends_on.len()),
        };
        self.nodes.push(node);
        self.deps.push(depends_on.to_vec());

        // Wire forward edges: each dependency gains this node as a dependent.
        for &dep_id in depends_on {
            self.nodes[dep_id].dependents.push(id);
        }

        id
    }

    /// Validate that the graph contains no cycles.
    ///
    /// Uses Kahn's algorithm: repeatedly remove nodes with zero in-degree.
    /// If not all nodes are removed, a cycle exists.
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

        let mut visited = 0usize;
        while let Some(node_id) = queue.pop_front() {
            visited += 1;
            for &dep_id in &self.nodes[node_id].dependents {
                in_degree[dep_id] -= 1;
                if in_degree[dep_id] == 0 {
                    queue.push_back(dep_id);
                }
            }
        }

        if visited == n {
            Ok(())
        } else {
            Err(CycleError {
                message: format!(
                    "{} of {n} nodes are part of or blocked by a cycle",
                    n - visited
                ),
            })
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
        let id = dag.add_node("only", &[]);
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
        let a = dag.add_node("a", &[]);
        let b = dag.add_node("b", &[a]);
        let c = dag.add_node("c", &[b]);

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
        let a = dag.add_node("a", &[]);
        let b = dag.add_node("b", &[a]);
        let c = dag.add_node("c", &[a]);
        let d = dag.add_node("d", &[b, c]);

        assert_eq!(dag.initial_ready(), vec![a]);

        // Complete a -> b and c become ready.
        let mut ready = dag.complete(a);
        ready.sort();
        assert_eq!(ready, vec![b, c]);

        // Complete b -> d still blocked on c.
        assert!(dag.complete(b).is_empty());
        assert_eq!(dag.node(d).remaining_deps.load(Ordering::SeqCst), 1);

        // Complete c -> d is now ready.
        assert_eq!(dag.complete(c), vec![d]);
        assert_eq!(dag.node(d).remaining_deps.load(Ordering::SeqCst), 0);

        assert!(dag.validate().is_ok());
    }

    // ── parallel roots ─────────────────────────────────────────────────

    #[test]
    fn parallel_roots() {
        let mut dag = Dag::new();
        let r0 = dag.add_node("root0", &[]);
        let r1 = dag.add_node("root1", &[]);
        let r2 = dag.add_node("root2", &[]);

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
        dag.add_node("alpha", &[]);
        dag.add_node("beta", &[0]);
        dag.add_node("gamma", &[0, 1]);

        assert_eq!(dag.node(0).payload, "alpha");
        assert_eq!(dag.node(1).payload, "beta");
        assert_eq!(dag.node(2).payload, "gamma");

        // Check dependents wiring.
        assert_eq!(dag.node(0).dependents, vec![1, 2]);
        assert_eq!(dag.node(1).dependents, vec![2]);
        assert!(dag.node(2).dependents.is_empty());

        // Check remaining deps.
        assert_eq!(dag.node(0).remaining_deps.load(Ordering::SeqCst), 0);
        assert_eq!(dag.node(1).remaining_deps.load(Ordering::SeqCst), 1);
        assert_eq!(dag.node(2).remaining_deps.load(Ordering::SeqCst), 2);
    }

    // ── cycle detection ────────────────────────────────────────────────

    #[test]
    fn cycle_detection() {
        // We cannot create a true cycle through add_node (it only allows
        // references to already-existing nodes). So we test validate() by
        // manually constructing a DAG with a back-edge.
        let dag = Dag {
            nodes: vec![
                Node {
                    id: 0,
                    payload: "a",
                    dependents: vec![1],
                    remaining_deps: AtomicUsize::new(1), // depends on node 1 (cycle!)
                },
                Node {
                    id: 1,
                    payload: "b",
                    dependents: vec![0],
                    remaining_deps: AtomicUsize::new(1), // depends on node 0
                },
            ],
            deps: vec![vec![1], vec![0]], // a depends on b, b depends on a
        };

        let result = dag.validate();
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.message.contains("cycle"), "error should mention cycle: {err}");
    }

    // ── large fan-out ──────────────────────────────────────────────────

    #[test]
    fn large_fan_out() {
        let mut dag = Dag::new();
        let root = dag.add_node("root".to_string(), &[]);

        let fan_size = 100;
        let mut children = Vec::with_capacity(fan_size);
        for i in 0..fan_size {
            let child = dag.add_node(format!("child-{i}"), &[root]);
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
