use super::*;
use crate::CapturedUnit;

fn shell(cmd: &str) -> WorkPayload {
    WorkPayload::Shell {
        cmd: cmd.to_string(),
        line: 0,
    }
}

fn probe(key: &str) -> WorkPayload {
    WorkPayload::Probe {
        key: key.to_string(),
        produce: "return 1".to_string(),
        line: 0,
    }
}

fn unit(payload: WorkPayload, dep_kind: DepKind, probes: Vec<String>) -> CapturedUnit {
    CapturedUnit {
        payload,
        cache_meta: None,
        dep_kind,
        probes,
        unit_env_vars: Default::default(),
        member: None,
        output_paths: Vec::new(),
        test_name: None,
    }
}

fn seq_unit(cmd: &str) -> CapturedUnit {
    unit(shell(cmd), DepKind::Sequential, vec![])
}

fn recipe(name: &str, deps: &[&str], units: Vec<CapturedUnit>) -> RecipeUnits {
    RecipeUnits {
        recipe_name: name.into(),
        deps: deps.iter().map(|s| s.to_string()).collect(),
        units,
        step_groups: vec![],
        working_dir: std::path::PathBuf::from("."),
        env_vars: BTreeMap::new(),
        terminal_outputs: vec![],
        dep_edges: vec![],
        probes: vec![],
    }
}

/// Resolve a `(recipe, unit_idx)` origin to its node id.
fn unit_id(g: &UnitGraph, recipe: &str, unit_idx: usize) -> usize {
    g.nodes
        .iter()
        .position(|n| {
            matches!(&n.origin, NodeOrigin::Unit { recipe: r, unit_idx: i }
                     if r == recipe && *i == unit_idx)
        })
        .unwrap_or_else(|| panic!("no node for unit:{recipe}:{unit_idx} in {:?}", g.nodes))
}

fn has_edge(g: &UnitGraph, from: usize, to: usize, kind: EdgeProvenance) -> bool {
    g.nodes[to].deps.iter().any(|&(d, k)| d == from && k == kind)
}

fn edge_list(g: &UnitGraph) -> String {
    g.nodes
        .iter()
        .enumerate()
        .flat_map(|(to, n)| {
            n.deps
                .iter()
                .map(move |&(from, k)| format!("{from} -> {to} [{k:?}]"))
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// Every wiring rule leaves its kind on the recorded dependency: serial
/// barriers, group entry, probe consumption, coarse barriers, fine dep_order
/// refs.
#[test]
fn plan_records_provenance_per_edge_kind() {
    let producer = recipe("producer", &[], vec![seq_unit("make gen")]);
    let mut consumer = recipe(
        "consumer",
        &["producer"],
        vec![
            unit(probe("cc:flags"), DepKind::Sequential, vec![]),
            unit(shell("gcc -c a.c"), DepKind::StepGroup(0), vec!["cc:flags".into()]),
            unit(shell("gcc -c b.c"), DepKind::StepGroup(0), vec![]),
            seq_unit("ld -o app a.o b.o"),
        ],
    );
    consumer.step_groups = vec![vec![1, 2]];
    // The link unit also carries a fine-grained ref to the producer.
    consumer.dep_edges = vec![(3, "producer".into())];

    let g = plan(&[producer, consumer]).expect("plans");

    let p0 = unit_id(&g, "producer", 0);
    let probe0 = unit_id(&g, "consumer", 0);
    let cc_a = unit_id(&g, "consumer", 1);
    let cc_b = unit_id(&g, "consumer", 2);
    let link = unit_id(&g, "consumer", 3);

    // Coarse dep-list barrier: producer leaf -> every consumer root.
    assert!(has_edge(&g, p0, cc_a, EdgeProvenance::Barrier), "{}", edge_list(&g));
    assert!(has_edge(&g, p0, cc_b, EdgeProvenance::Barrier), "{}", edge_list(&g));
    // Probe consumption.
    assert!(has_edge(&g, probe0, cc_a, EdgeProvenance::Probe), "{}", edge_list(&g));
    // Group members become the barrier; the link unit enters sequentially.
    assert!(has_edge(&g, cc_a, link, EdgeProvenance::Serial), "{}", edge_list(&g));
    assert!(has_edge(&g, cc_b, link, EdgeProvenance::Serial), "{}", edge_list(&g));
    // Fine-grained ref on the link unit, additive with the barrier above.
    assert!(has_edge(&g, p0, link, EdgeProvenance::DepOrder), "{}", edge_list(&g));
}

/// The additive rule (CS-0161, "Rejected alternative"): a declared
/// `requires` keeps its whole-recipe barrier whether or not a unit also
/// fine-covers the same producer with `dep_order`.
#[test]
fn plan_keeps_declared_barrier_alongside_fine_cover() {
    let producer = recipe("producer", &[], vec![seq_unit("make gen")]);
    let mut consumer = recipe(
        "consumer",
        &["producer"],
        vec![seq_unit("cp src first"), seq_unit("cp gen out")],
    );
    consumer.dep_edges = vec![(1, "producer".into())];

    let g = plan(&[producer, consumer]).expect("plans");

    let p0 = unit_id(&g, "producer", 0);
    let root = unit_id(&g, "consumer", 0);
    let fine = unit_id(&g, "consumer", 1);

    assert!(
        has_edge(&g, p0, root, EdgeProvenance::Barrier),
        "the declared requires barrier must survive fine coverage: {}",
        edge_list(&g)
    );
    assert!(has_edge(&g, p0, fine, EdgeProvenance::DepOrder), "{}", edge_list(&g));
}

/// Leaf pass-through: a unit-less meta-target forwards its deps' leaves, so
/// a downstream barrier or fine ref lands on the real producer's units
/// instead of vanishing.
#[test]
fn plan_routes_edges_through_a_unit_less_meta_target() {
    let producer = recipe("producer", &[], vec![seq_unit("make gen")]);
    let middle = recipe("middle", &["producer"], vec![]);
    let mut consumer = recipe("consumer", &["middle"], vec![seq_unit("use gen")]);
    consumer.dep_edges = vec![(0, "middle".into())];

    let g = plan(&[producer, middle, consumer]).expect("plans");

    let p0 = unit_id(&g, "producer", 0);
    let c0 = unit_id(&g, "consumer", 0);

    assert!(
        has_edge(&g, p0, c0, EdgeProvenance::Barrier),
        "the coarse dep on a unit-less middle must forward to the producer's \
         leaves: {}",
        edge_list(&g)
    );
    assert!(
        has_edge(&g, p0, c0, EdgeProvenance::DepOrder),
        "the fine ref on a unit-less middle must forward too: {}",
        edge_list(&g)
    );
}

/// A `dep_edges` entry naming a recipe absent from the slice is the plan's
/// to reject — nothing upstream validates this channel.
#[test]
fn plan_rejects_a_dangling_dep_edge() {
    let mut consumer = recipe("consumer", &[], vec![seq_unit("use gen")]);
    consumer.dep_edges = vec![(0, "nosuch".into())];

    let err = plan(&[consumer]).expect_err("must reject");
    assert_eq!(
        err,
        UnitGraphError::DanglingDepEdge {
            referring_recipe: "consumer".into(),
            dep_name: "nosuch".into(),
        }
    );
}

/// An unconsumed probe is pruned; the barrier flows past it untouched.
#[test]
fn plan_prunes_an_unconsumed_probe_and_keeps_the_barrier_intact() {
    let r = recipe(
        "build",
        &[],
        vec![
            seq_unit("first"),
            unit(probe("cc:unused"), DepKind::Sequential, vec![]),
            seq_unit("second"),
        ],
    );

    let g = plan(&[r]).expect("plans");

    // The pruned probe has no node at all.
    assert!(
        !g.nodes.iter().any(|n| matches!(&n.origin,
            NodeOrigin::Unit { unit_idx: 1, .. })),
        "unconsumed probe must be pruned: {:?}",
        g.nodes
    );
    // `second` serially depends on `first` — the probe did not sever it.
    let first = unit_id(&g, "build", 0);
    let second = unit_id(&g, "build", 2);
    assert!(has_edge(&g, first, second, EdgeProvenance::Serial), "{}", edge_list(&g));
}

/// The closure-map filter: `orders`-derived names never become coarse deps.
#[test]
fn declared_coarse_deps_filters_orders_only_names() {
    let declared = vec!["a".to_string()];
    let closure = vec!["a".to_string(), "b-via-orders".to_string()];
    assert_eq!(declared_coarse_deps(&declared, &closure), vec!["a".to_string()]);
    assert!(declared_coarse_deps(&[], &closure).is_empty());
}

/// Deterministic dependency-first order, cycle rejection naming the recipes.
#[test]
fn toposort_orders_deps_first_and_names_cycles() {
    let edges: BTreeMap<String, Vec<String>> = [
        ("b".to_string(), vec!["a".to_string()]),
        ("c".to_string(), vec!["b".to_string()]),
    ]
    .into();
    let reachable: BTreeSet<String> =
        ["a", "b", "c"].iter().map(|s| s.to_string()).collect();
    assert_eq!(
        toposort_recipes(&edges, &reachable).unwrap(),
        vec!["a".to_string(), "b".to_string(), "c".to_string()]
    );

    let cyclic: BTreeMap<String, Vec<String>> = [
        ("a".to_string(), vec!["b".to_string()]),
        ("b".to_string(), vec!["a".to_string()]),
    ]
    .into();
    let two: BTreeSet<String> = ["a", "b"].iter().map(|s| s.to_string()).collect();
    let err = toposort_recipes(&cyclic, &two).expect_err("cycle");
    assert!(matches!(err, UnitGraphError::Cycle { ref unresolved }
        if unresolved.contains(&"a".to_string()) && unresolved.contains(&"b".to_string())));
}
