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

// ── deps() predecessor accessor ────────────────────────────────────

#[test]
fn deps_returns_predecessor_ids() {
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

    assert!(dag.deps(a).is_empty());
    assert_eq!(dag.deps(b), &[a]);
    assert_eq!(dag.deps(c), &[a]);
    // b < c by id, so BTreeSet ordering gives [b, c]
    assert_eq!(dag.deps(d), &[b, c]);
}

// ── Send + Sync ────────────────────────────────────────────────────

#[test]
fn dag_is_send_and_sync() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<Dag<String>>();
    assert_send_sync::<Node<String>>();
}
