use super::*;

// ── empty dag ──────────────────────────────────────────────────────

#[test]
fn empty_dag() {
    let dag: Dag<&str> = Dag::new();
    assert!(dag.is_empty());
    assert_eq!(dag.len(), 0);
    assert!(dag.initial_ready().is_empty());
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

// ── once-only completion (COOK-400) ────────────────────────────────

/// `remaining_deps` is unsigned and its decrement wraps. Before the guard, a
/// second `complete(b)` took `d`'s counter from 1 to 0 and saw `prev == 1`,
/// releasing `d` while `c` had not run at all.
///
/// Note the shape: the double-completed node must still have a WAITING
/// dependent. Completing `a` twice here corrupts `b` and `c`, which are
/// already at zero, and nothing downstream notices. The first version of this
/// test did exactly that and passed with the guard removed.
#[test]
fn completing_twice_does_not_release_a_node_early() {
    //   a
    //  / \
    // b   c      (b completed twice, by mistake; c never runs)
    //  \ /
    //   d
    let mut dag = Dag::new();
    let a = dag.add_node("a", &[]).unwrap();
    let b = dag.add_node("b", &[a]).unwrap();
    let c = dag.add_node("c", &[a]).unwrap();
    let d = dag.add_node("d", &[b, c]).unwrap();

    let mut ready = dag.complete(a);
    ready.sort();
    assert_eq!(ready, vec![b, c]);
    assert_eq!(dag.node(d).remaining_deps(), 2);

    assert!(dag.complete(b).is_empty(), "d still waits on c");
    assert_eq!(dag.node(d).remaining_deps(), 1);

    // The bug: a second completion of b.
    assert!(
        dag.complete(b).is_empty(),
        "a repeat completion must release nothing; without the guard this \
         returns [d] while c has not run"
    );
    assert_eq!(
        dag.node(d).remaining_deps(),
        1,
        "d must still be waiting on c"
    );

    // Only c may release d.
    assert_eq!(dag.complete(c), vec![d]);
    assert_eq!(dag.node(d).remaining_deps(), 0);
}

#[test]
fn is_completed_reports_completion() {
    let mut dag = Dag::new();
    let a = dag.add_node("a", &[]).unwrap();
    let b = dag.add_node("b", &[a]).unwrap();

    assert!(!dag.is_completed(a));
    assert!(!dag.is_completed(b));
    dag.complete(a);
    assert!(dag.is_completed(a));
    assert!(!dag.is_completed(b));
}

/// Two threads racing on the SAME id must still produce exactly one set of
/// decrements, which is why the guard is a `swap` rather than a load/store.
#[test]
fn concurrent_completion_of_one_id_decrements_once() {
    use std::sync::Arc;

    let mut dag = Dag::new();
    let a = dag.add_node("a", &[]).unwrap();
    let _b = dag.add_node("b", &[a]).unwrap();
    let dag = Arc::new(dag);

    let mut released = 0usize;
    let handles: Vec<_> = (0..8)
        .map(|_| {
            let d = Arc::clone(&dag);
            std::thread::spawn(move || d.complete(a).len())
        })
        .collect();
    for h in handles {
        released += h.join().unwrap();
    }

    assert_eq!(
        released, 1,
        "exactly one of the racing calls may report b as newly ready"
    );
    // The decisive assertion: without the guard the first call reports the
    // release and the other seven wrap `b`'s counter below zero, which the
    // return value alone does not reveal.
    assert_eq!(
        dag.node(_b).remaining_deps(),
        0,
        "b's dependency count must be 0, not wrapped around by the losers"
    );
}
