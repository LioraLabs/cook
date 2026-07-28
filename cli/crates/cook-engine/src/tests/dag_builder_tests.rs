use super::*;
use std::path::PathBuf;

fn shell(cmd: &str) -> WorkPayload {
    WorkPayload::Shell {
        cmd: cmd.to_string(),
        line: 0,
    }
}

fn default_wd() -> PathBuf {
    PathBuf::from(".")
}

fn default_env() -> BTreeMap<String, String> {
    BTreeMap::new()
}

fn probe(key: &str) -> WorkPayload {
    WorkPayload::Probe {
        key: key.to_string(),
        produce: "return 1".to_string(),
        line: 0,
    }
}

/// CS-0074 Bug 2 regression: DAG builder must add probe→consumer edges from
/// CapturedUnit.probes. This verifies that when a probe unit precedes a
/// consumer unit in units and the consumer's probes lists the probe key,
/// the resulting DAG consumer node has the probe node as a dependency.
#[test]
fn dag_builder_adds_probe_to_consumer_edge() {
    let units = RecipeUnits {
        recipe_name: "build".into(),
        deps: vec![],
        units: vec![
            // Probe unit first (as cook.probe is called first in register block)
            CapturedUnit {
                payload: probe("cc:zlib"),
                cache_meta: None,
                dep_kind: DepKind::Sequential,
                probes: vec![],
                unit_env_vars: Default::default(),
                member: None,
                output_paths: Vec::new(),
            },
            // Consumer unit with probes = ["cc:zlib"]
            CapturedUnit {
                payload: shell("gcc -o app main.c"),
                cache_meta: None,
                dep_kind: DepKind::Sequential,
                probes: vec!["cc:zlib".to_string()],
                unit_env_vars: Default::default(),
                member: None,
                output_paths: Vec::new(),
            },
        ],
        step_groups: vec![],
        working_dir: default_wd(),
        env_vars: default_env(),
        terminal_outputs: vec![],
        dep_edges: vec![],
        probes: vec![],
    };
    let dag = build_dag(vec![units]).expect("no collision");
    assert_eq!(dag.len(), 2);
    // Probe node (0) has no deps.
    assert_eq!(dag.node(0).remaining_deps(), 0, "probe node must have no deps");
    // Consumer node (1) depends on: sequential barrier (probe node 0) + probes edge (also probe 0).
    // The probes edge is deduplicated since it's the same node, so remaining_deps = 1.
    assert_eq!(
        dag.node(1).remaining_deps(),
        1,
        "consumer must depend on probe node via probes edge"
    );
}

#[test]
fn test_build_single_recipe_sequential() {
    let units = RecipeUnits {
        recipe_name: "build".into(),
        deps: vec![],
        units: vec![
            CapturedUnit {
                payload: shell("echo a"),
                cache_meta: None,
                dep_kind: DepKind::Sequential,
                probes: vec![],
                unit_env_vars: Default::default(),
                member: None,
                output_paths: Vec::new(),
            },
            CapturedUnit {
                payload: shell("echo b"),
                cache_meta: None,
                dep_kind: DepKind::Sequential,
                probes: vec![],
                unit_env_vars: Default::default(),
                member: None,
                output_paths: Vec::new(),
            },
        ],
        step_groups: vec![],
        working_dir: default_wd(),
        env_vars: default_env(),
        terminal_outputs: vec![],
        dep_edges: vec![],
        probes: vec![],
    };
    let dag = build_dag(vec![units]).expect("no collision");
    assert_eq!(dag.len(), 2);
    // Second node should depend on first
    assert_eq!(dag.node(0).remaining_deps(), 0);
    assert_eq!(dag.node(1).remaining_deps(), 1);
}

#[test]
fn test_build_step_group() {
    // A step group of 2 units, then a sequential unit after
    let units = RecipeUnits {
        recipe_name: "build".into(),
        deps: vec![],
        units: vec![
            CapturedUnit {
                payload: shell("gcc -c a.c"),
                cache_meta: None,
                dep_kind: DepKind::StepGroup(0),
                probes: vec![],
                unit_env_vars: Default::default(),
                member: None,
                output_paths: Vec::new(),
            },
            CapturedUnit {
                payload: shell("gcc -c b.c"),
                cache_meta: None,
                dep_kind: DepKind::StepGroup(0),
                probes: vec![],
                unit_env_vars: Default::default(),
                member: None,
                output_paths: Vec::new(),
            },
            CapturedUnit {
                payload: shell("ar rcs lib.a"),
                cache_meta: None,
                dep_kind: DepKind::Sequential,
                probes: vec![],
                unit_env_vars: Default::default(),
                member: None,
                output_paths: Vec::new(),
            },
        ],
        step_groups: vec![vec![0, 1]],
        working_dir: default_wd(),
        env_vars: default_env(),
        terminal_outputs: vec![],
        dep_edges: vec![],
        probes: vec![],
    };
    let dag = build_dag(vec![units]).expect("no collision");
    assert_eq!(dag.len(), 3);
    // Step group units have 0 deps (first in recipe)
    assert_eq!(dag.node(0).remaining_deps(), 0);
    assert_eq!(dag.node(1).remaining_deps(), 0);
    // Sequential unit after group depends on both group members
    assert_eq!(dag.node(2).remaining_deps(), 2);
}

#[test]
fn test_build_cross_recipe_deps() {
    let setup = RecipeUnits {
        recipe_name: "setup".into(),
        deps: vec![],
        units: vec![CapturedUnit {
            payload: shell("mkdir build"),
            cache_meta: None,
            dep_kind: DepKind::Sequential,
            probes: vec![],
            unit_env_vars: Default::default(),
            member: None,
            output_paths: Vec::new(),
        }],
        step_groups: vec![],
        working_dir: default_wd(),
        env_vars: default_env(),
        terminal_outputs: vec![],
        dep_edges: vec![],
        probes: vec![],
    };
    let build = RecipeUnits {
        recipe_name: "build".into(),
        deps: vec!["setup".into()],
        units: vec![CapturedUnit {
            payload: shell("gcc main.c"),
            cache_meta: None,
            dep_kind: DepKind::Sequential,
            probes: vec![],
            unit_env_vars: Default::default(),
            member: None,
            output_paths: Vec::new(),
        }],
        step_groups: vec![],
        working_dir: default_wd(),
        env_vars: default_env(),
        terminal_outputs: vec![],
        dep_edges: vec![],
        probes: vec![],
    };
    let dag = build_dag(vec![setup, build]).expect("no collision");
    assert_eq!(dag.len(), 2);
    // build's unit should depend on setup's unit
    assert_eq!(dag.node(1).remaining_deps(), 1);
}

#[test]
fn test_build_empty() {
    let dag = build_dag(vec![]).expect("no collision");
    assert!(dag.is_empty());
}

#[test]
fn test_fine_grained_cross_recipe_deps() {
    // libmath: compile group (2 units) -> archive (sequential)
    let libmath = RecipeUnits {
        recipe_name: "libmath".into(),
        deps: vec![],
        units: vec![
            CapturedUnit {
                payload: shell("gcc -c add.c"),
                cache_meta: None,
                dep_kind: DepKind::StepGroup(0),
                probes: vec![],
                unit_env_vars: Default::default(),
                member: None,
                output_paths: Vec::new(),
            },
            CapturedUnit {
                payload: shell("gcc -c mul.c"),
                cache_meta: None,
                dep_kind: DepKind::StepGroup(0),
                probes: vec![],
                unit_env_vars: Default::default(),
                member: None,
                output_paths: Vec::new(),
            },
            CapturedUnit {
                payload: shell("ar rcs libmath.a"),
                cache_meta: None,
                dep_kind: DepKind::Sequential,
                probes: vec![],
                unit_env_vars: Default::default(),
                member: None,
                output_paths: Vec::new(),
            },
        ],
        step_groups: vec![vec![0, 1]],
        working_dir: default_wd(),
        env_vars: default_env(),
        terminal_outputs: vec!["libmath.a".into()],
        dep_edges: vec![],
        probes: vec![],
    };

    // app: compile (1 unit, step group) -> link (sequential, depends on libmath)
    let app = RecipeUnits {
        recipe_name: "app".into(),
        deps: vec![],
        units: vec![
            CapturedUnit {
                payload: shell("gcc -c main.c"),
                cache_meta: None,
                dep_kind: DepKind::StepGroup(0),
                probes: vec![],
                unit_env_vars: Default::default(),
                member: None,
                output_paths: Vec::new(),
            },
            CapturedUnit {
                payload: shell("gcc -o app main.o libmath.a"),
                cache_meta: None,
                dep_kind: DepKind::Sequential,
                probes: vec![],
                unit_env_vars: Default::default(),
                member: None,
                output_paths: Vec::new(),
            },
        ],
        step_groups: vec![vec![0]],
        working_dir: default_wd(),
        env_vars: default_env(),
        terminal_outputs: vec!["app".into()],
        dep_edges: vec![(1, "libmath".into())], // unit 1 (link) depends on libmath
        probes: vec![],
    };

    let dag = build_dag(vec![libmath, app]).expect("no collision");
    assert_eq!(dag.len(), 5);

    // Nodes: 0=add.c, 1=mul.c, 2=archive, 3=main.c, 4=link

    // app's compile (node 3) should have 0 deps — can run in parallel with libmath
    assert_eq!(
        dag.node(3).remaining_deps(),
        0,
        "app compile should start immediately (no cross-recipe dep)"
    );

    // app's link (node 4) should depend on:
    // - node 3 (within-recipe: sequential after step group [3])
    // - node 2 (fine-grained: libmath's terminal node = archive)
    // Total: 2 deps
    assert_eq!(
        dag.node(4).remaining_deps(),
        2,
        "app link should depend on app compile + libmath archive"
    );
}

#[test]
fn test_fine_grained_no_dep_edges_unchanged() {
    // Verify backward compat: recipes with dep_edges: vec![] behave as before
    let setup = RecipeUnits {
        recipe_name: "setup".into(),
        deps: vec![],
        units: vec![CapturedUnit {
            payload: shell("mkdir build"),
            cache_meta: None,
            dep_kind: DepKind::Sequential,
            probes: vec![],
            unit_env_vars: Default::default(),
            member: None,
            output_paths: Vec::new(),
        }],
        step_groups: vec![],
        working_dir: default_wd(),
        env_vars: default_env(),
        terminal_outputs: vec![],
        dep_edges: vec![],
        probes: vec![],
    };
    let build = RecipeUnits {
        recipe_name: "build".into(),
        deps: vec!["setup".into()],
        units: vec![CapturedUnit {
            payload: shell("gcc main.c"),
            cache_meta: None,
            dep_kind: DepKind::Sequential,
            probes: vec![],
            unit_env_vars: Default::default(),
            member: None,
            output_paths: Vec::new(),
        }],
        step_groups: vec![],
        working_dir: default_wd(),
        env_vars: default_env(),
        terminal_outputs: vec![],
        dep_edges: vec![],
        probes: vec![],
    };
    let dag = build_dag(vec![setup, build]).expect("no collision");
    assert_eq!(dag.len(), 2);
    // build's unit depends on setup's unit via coarse deps
    assert_eq!(dag.node(1).remaining_deps(), 1);
}

/// A `dep_edges` entry naming a recipe absent from the slice passed to
/// `build_dag` must raise `EngineError::DanglingDepEdge` naming both the
/// referring recipe and the dep, instead of silently vanishing.
///
/// This is defensive hardening, not a live-path regression test: on the
/// live path a `$<sigil>` ref merges into `requires` too (codegen's
/// `unified_requires_field`), and `requires` is analyzer-validated
/// before `build_dag` ever runs — so this condition is unreachable from
/// a Cookfile today. We construct `RecipeUnits` directly to exercise the
/// defensive check.
#[test]
fn dep_edges_entry_naming_recipe_outside_closure_diagnoses() {
    let app = RecipeUnits {
        recipe_name: "app".into(),
        deps: vec![],
        units: vec![CapturedUnit {
            payload: shell("gcc -o app main.c libmath.a"),
            cache_meta: None,
            dep_kind: DepKind::Sequential,
            probes: vec![],
            unit_env_vars: Default::default(),
            member: None,
            output_paths: Vec::new(),
        }],
        step_groups: vec![],
        working_dir: default_wd(),
        env_vars: default_env(),
        terminal_outputs: vec![],
        // "libmath" is never passed to build_dag — outside the closure.
        dep_edges: vec![(0, "libmath".into())],
        probes: vec![],
    };

    let err = build_dag(vec![app]).expect_err(
        "dep_edges entry naming an out-of-closure recipe must error, not vanish silently",
    );
    let msg = err.to_string();
    assert!(msg.contains("app"), "message must name the referring recipe: {msg}");
    assert!(msg.contains("libmath"), "message must name the dep: {msg}");
    assert!(
        msg.contains(": libmath"),
        "message must hint at the recipe-header fix: {msg}"
    );
    assert!(
        msg.contains("cook.require_recipe(\"libmath\")"),
        "message must hint at the cook.require_recipe fix: {msg}"
    );
    match err {
        EngineError::DanglingDepEdge { referring_recipe, dep_name } => {
            assert_eq!(referring_recipe, "app");
            assert_eq!(dep_name, "libmath");
        }
        other => panic!("expected DanglingDepEdge, got: {other:?}"),
    }
}

/// A `dep_edges` entry naming a recipe that IS present in the slice but
/// contributes zero units (its leaf set is `Some(empty)`, not absent)
/// must NOT diagnose — this is the "distinguish absent from empty" case
/// the pre-walk validation exists to get right.
#[test]
fn dep_edges_entry_naming_in_closure_zero_unit_recipe_does_not_diagnose() {
    let noop = RecipeUnits {
        recipe_name: "noop".into(),
        deps: vec![],
        units: vec![], // zero units — legitimate, not an absence
        step_groups: vec![],
        working_dir: default_wd(),
        env_vars: default_env(),
        terminal_outputs: vec![],
        dep_edges: vec![],
        probes: vec![],
    };
    let app = RecipeUnits {
        recipe_name: "app".into(),
        deps: vec![],
        units: vec![CapturedUnit {
            payload: shell("gcc -o app main.c"),
            cache_meta: None,
            dep_kind: DepKind::Sequential,
            probes: vec![],
            unit_env_vars: Default::default(),
            member: None,
            output_paths: Vec::new(),
        }],
        step_groups: vec![],
        working_dir: default_wd(),
        env_vars: default_env(),
        terminal_outputs: vec![],
        dep_edges: vec![(0, "noop".into())],
        probes: vec![],
    };

    // noop must be passed ahead of app to respect the topo-order
    // contract build_dag relies on for recipe_leaves.
    let dag = build_dag(vec![noop, app]).expect(
        "an in-closure zero-unit dep must not diagnose — Some(empty) is legitimate",
    );
    // Only app's single unit produces a node; noop contributes none.
    assert_eq!(dag.len(), 1);
    assert_eq!(
        dag.node(0).remaining_deps(),
        0,
        "no leaves to depend on since noop's leaf set is empty, not absent"
    );
}

/// The core bug: a zero-unit recipe used as a meta-target must forward
/// its prerequisites' leaves as its own leaf set, not register an empty
/// one. `producer` (1 unit) -> `middle` (0 units, `deps: ["producer"]`)
    /// -> `consumer` (1 unit, `deps: ["middle"]`). Without the fix,
/// `middle`'s leaf set is `Some(empty)` and `consumer` ends up with zero
/// deps, running concurrently with `producer` instead of after it.
#[test]
fn zero_unit_recipe_forwards_producer_leaf_to_downstream_consumer() {
    let producer = RecipeUnits {
        recipe_name: "producer".into(),
            deps: vec![],
            units: vec![CapturedUnit {
                payload: shell("touch build/gen.a"),
            cache_meta: None,
            dep_kind: DepKind::Sequential,
            probes: vec![],
            unit_env_vars: Default::default(),
            member: None,
            output_paths: Vec::new(),
        }],
        step_groups: vec![],
        working_dir: default_wd(),
        env_vars: default_env(),
        terminal_outputs: vec![],
        dep_edges: vec![],
        probes: vec![],
    };
    let middle = RecipeUnits {
        recipe_name: "middle".into(),
        deps: vec!["producer".into()],
            units: vec![], // zero units — the meta-target shape
            step_groups: vec![],
            working_dir: default_wd(),
            env_vars: default_env(),
            terminal_outputs: vec![],
            dep_edges: vec![],
            probes: vec![],
        };
        let consumer = RecipeUnits {
            recipe_name: "consumer".into(),
        deps: vec!["middle".into()],
        units: vec![CapturedUnit {
            payload: shell("cp build/gen.a ."),
            cache_meta: None,
            dep_kind: DepKind::Sequential,
            probes: vec![],
            unit_env_vars: Default::default(),
            member: None,
            output_paths: Vec::new(),
        }],
        step_groups: vec![],
        working_dir: default_wd(),
        env_vars: default_env(),
        terminal_outputs: vec![],
        dep_edges: vec![],
        probes: vec![],
    };

    let dag = build_dag(vec![producer, middle, consumer]).expect("no collision");
    // Nodes: 0 = producer's unit, 1 = consumer's unit (middle contributes none).
    assert_eq!(dag.len(), 2);
    assert_eq!(
        dag.node(1).remaining_deps(),
        1,
        "consumer must depend on producer's leaf, forwarded through middle"
    );
    assert_eq!(
        dag.deps(1),
        &[0],
        "consumer's actual edge must point at producer's node (id 0), not merely be nonzero"
    );
}

/// Two-hop zero-unit chain: the forwarding must compose transitively,
/// not just one hop. `producer` -> `m1` (0 units) -> `m2` (0 units) ->
/// `consumer` (1 unit) must still reach `producer`'s node.
#[test]
fn two_hop_zero_unit_chain_forwards_leaf_transitively() {
    let producer = RecipeUnits {
        recipe_name: "producer".into(),
            deps: vec![],
            units: vec![CapturedUnit {
                payload: shell("touch build/gen.a"),
            cache_meta: None,
            dep_kind: DepKind::Sequential,
            probes: vec![],
            unit_env_vars: Default::default(),
            member: None,
            output_paths: Vec::new(),
        }],
        step_groups: vec![],
        working_dir: default_wd(),
        env_vars: default_env(),
        terminal_outputs: vec![],
        dep_edges: vec![],
        probes: vec![],
    };
    let m1 = RecipeUnits {
        recipe_name: "m1".into(),
        deps: vec!["producer".into()],
            units: vec![],
            step_groups: vec![],
            working_dir: default_wd(),
            env_vars: default_env(),
            terminal_outputs: vec![],
            dep_edges: vec![],
            probes: vec![],
        };
        let m2 = RecipeUnits {
            recipe_name: "m2".into(),
        deps: vec!["m1".into()],
        units: vec![],
        step_groups: vec![],
        working_dir: default_wd(),
        env_vars: default_env(),
        terminal_outputs: vec![],
        dep_edges: vec![],
        probes: vec![],
    };
    let consumer = RecipeUnits {
        recipe_name: "consumer".into(),
            deps: vec!["m2".into()],
        units: vec![CapturedUnit {
            payload: shell("cp build/gen.a ."),
            cache_meta: None,
            dep_kind: DepKind::Sequential,
            probes: vec![],
            unit_env_vars: Default::default(),
            member: None,
            output_paths: Vec::new(),
        }],
        step_groups: vec![],
        working_dir: default_wd(),
        env_vars: default_env(),
        terminal_outputs: vec![],
        dep_edges: vec![],
        probes: vec![],
    };

    let dag = build_dag(vec![producer, m1, m2, consumer]).expect("no collision");
    // Nodes: 0 = producer's unit, 1 = consumer's unit (m1, m2 contribute none).
    assert_eq!(dag.len(), 2);
    assert_eq!(
        dag.deps(1),
        &[0],
        "consumer must reach producer's node through a two-hop zero-unit chain"
    );
}

/// Diamond through two zero-unit recipes: `producer` -> `b` (0 units) and
/// `producer` -> `c` (0 units), then `d : b c` (1 unit). Both `b` and `c`
/// forward the SAME producer leaf, so `d` must end up with exactly one
/// dep (the dedup in `Dag::add_node` collapses the duplicate) — not two,
/// which would indicate a phantom dep count.
#[test]
fn diamond_through_zero_unit_recipes_dedups_to_one_dep() {
    let producer = RecipeUnits {
        recipe_name: "producer".into(),
            deps: vec![],
            units: vec![CapturedUnit {
                payload: shell("touch build/gen.a"),
            cache_meta: None,
            dep_kind: DepKind::Sequential,
            probes: vec![],
            unit_env_vars: Default::default(),
            member: None,
            output_paths: Vec::new(),
        }],
        step_groups: vec![],
        working_dir: default_wd(),
        env_vars: default_env(),
        terminal_outputs: vec![],
        dep_edges: vec![],
        probes: vec![],
    };
    let b = RecipeUnits {
        recipe_name: "b".into(),
        deps: vec!["producer".into()],
            units: vec![],
            step_groups: vec![],
            working_dir: default_wd(),
            env_vars: default_env(),
            terminal_outputs: vec![],
            dep_edges: vec![],
            probes: vec![],
        };
        let c = RecipeUnits {
            recipe_name: "c".into(),
        deps: vec!["producer".into()],
            units: vec![],
            step_groups: vec![],
            working_dir: default_wd(),
            env_vars: default_env(),
            terminal_outputs: vec![],
            dep_edges: vec![],
            probes: vec![],
        };
        let d = RecipeUnits {
            recipe_name: "d".into(),
        deps: vec!["b".into(), "c".into()],
        units: vec![CapturedUnit {
            payload: shell("echo done"),
            cache_meta: None,
            dep_kind: DepKind::Sequential,
            probes: vec![],
            unit_env_vars: Default::default(),
            member: None,
            output_paths: Vec::new(),
        }],
        step_groups: vec![],
        working_dir: default_wd(),
        env_vars: default_env(),
        terminal_outputs: vec![],
        dep_edges: vec![],
        probes: vec![],
    };

    let dag = build_dag(vec![producer, b, c, d]).expect("no collision");
    // Nodes: 0 = producer's unit, 1 = d's unit (b, c contribute none).
    assert_eq!(dag.len(), 2);
    assert_eq!(
        dag.node(1).remaining_deps(),
        1,
        "diamond through two zero-unit forwarders must dedup to a single dep"
    );
    assert_eq!(dag.deps(1), &[0], "d's sole dep must be producer's node");
}

/// A zero-unit recipe with no deps of its own has nothing to forward:
/// empty forwards empty. An independent downstream recipe naming it in
/// `deps` must end up with zero deps, not spuriously pick up unrelated
/// nodes.
#[test]
fn zero_unit_recipe_with_no_deps_forwards_empty_leaf_set() {
    let noop = RecipeUnits {
        recipe_name: "noop".into(),
        deps: vec![], // no deps of its own — nothing to forward
        units: vec![],
        step_groups: vec![],
        working_dir: default_wd(),
        env_vars: default_env(),
        terminal_outputs: vec![],
        dep_edges: vec![],
        probes: vec![],
    };
    let downstream = RecipeUnits {
        recipe_name: "downstream".into(),
        deps: vec!["noop".into()],
        units: vec![CapturedUnit {
            payload: shell("echo hi"),
            cache_meta: None,
            dep_kind: DepKind::Sequential,
            probes: vec![],
            unit_env_vars: Default::default(),
            member: None,
            output_paths: Vec::new(),
        }],
        step_groups: vec![],
        working_dir: default_wd(),
        env_vars: default_env(),
        terminal_outputs: vec![],
        dep_edges: vec![],
        probes: vec![],
    };

    let dag = build_dag(vec![noop, downstream]).expect("no collision");
    assert_eq!(dag.len(), 1);
    assert_eq!(
        dag.node(0).remaining_deps(),
        0,
        "empty leaf set forwards empty — nothing to depend on"
    );
}

/// Discriminates the trigger from the rejected "zero units" rule: `middle`
/// has a NON-empty `units` list (one demand-pruned probe), yet still ends
/// its unit loop with an empty barrier — probes never advance the barrier
/// (see the skip in the unit loop above), and this probe's key is
/// referenced by nobody downstream, so it is pruned before any `add_node`
/// call and contributes zero DAG nodes. A naive `if ru.units.is_empty()`
/// implementation would see `middle.units.len() == 1` and record the
/// (empty) `barrier` as `middle`'s leaves instead of forwarding
/// `cross_deps`, severing `consumer` from `producer`. The real
/// `barrier.is_empty()` trigger fires regardless of `units.len()` and
/// forwards correctly.
#[test]
fn recipe_with_only_a_pruned_probe_unit_still_forwards_cross_deps() {
    let producer = RecipeUnits {
        recipe_name: "producer".into(),
            deps: vec![],
            units: vec![CapturedUnit {
                payload: shell("touch build/gen.a"),
            cache_meta: None,
            dep_kind: DepKind::Sequential,
            probes: vec![],
            unit_env_vars: Default::default(),
            member: None,
            output_paths: Vec::new(),
        }],
        step_groups: vec![],
        working_dir: default_wd(),
        env_vars: default_env(),
        terminal_outputs: vec![],
        dep_edges: vec![],
        probes: vec![],
    };
    let middle = RecipeUnits {
        recipe_name: "middle".into(),
        deps: vec!["producer".into()],
            // NON-empty units: one probe, but its key is never referenced by
            // any non-probe unit anywhere (in `middle` or `consumer`), so
            // demand-driven pruning (§22.5.7) omits it from the DAG entirely.
            // Even if it survived pruning, probes never advance the barrier
            // (see the `is_probe` skip in the unit loop) — either way
            // `middle`'s barrier ends empty despite `units.len() == 1`.
            units: vec![CapturedUnit {
                payload: probe("mid:unreferenced"),
            cache_meta: None,
            dep_kind: DepKind::Sequential,
            probes: vec![],
            unit_env_vars: Default::default(),
            member: None,
            output_paths: Vec::new(),
        }],
        step_groups: vec![],
        working_dir: default_wd(),
        env_vars: default_env(),
        terminal_outputs: vec![],
        dep_edges: vec![],
        probes: vec![],
    };
    let consumer = RecipeUnits {
        recipe_name: "consumer".into(),
            deps: vec!["middle".into()],
        units: vec![CapturedUnit {
            payload: shell("cp build/gen.a ."),
            cache_meta: None,
            dep_kind: DepKind::Sequential,
            probes: vec![],
            unit_env_vars: Default::default(),
            member: None,
            output_paths: Vec::new(),
        }],
        step_groups: vec![],
        working_dir: default_wd(),
        env_vars: default_env(),
        terminal_outputs: vec![],
        dep_edges: vec![],
        probes: vec![],
    };

    let dag = build_dag(vec![producer, middle, consumer]).expect("no collision");
    // Nodes: 0 = producer's unit, 1 = consumer's unit. `middle`
    // contributes none — its sole unit is a pruned probe.
    assert_eq!(dag.len(), 2);
    assert_eq!(
        dag.deps(1),
        &[0],
        "consumer must reach producer's node even though middle.units is non-empty"
    );
}

#[test]
fn test_build_presatisfied_units() {
    let units = RecipeUnits {
        recipe_name: "build".into(),
        deps: vec![],
        units: vec![
            CapturedUnit {
                payload: WorkPayload::Shell {
                    cmd: String::new(),
                    line: 0,
                },
                cache_meta: None,
                dep_kind: DepKind::Sequential,
                probes: vec![],
                unit_env_vars: Default::default(),
                member: None,
                output_paths: Vec::new(),
            },
            CapturedUnit {
                payload: shell("echo real work"),
                cache_meta: None,
                dep_kind: DepKind::Sequential,
                probes: vec![],
                unit_env_vars: Default::default(),
                member: None,
                output_paths: Vec::new(),
            },
        ],
        step_groups: vec![],
        working_dir: default_wd(),
        env_vars: default_env(),
        terminal_outputs: vec![],
        dep_edges: vec![],
        probes: vec![],
    };
    let dag = build_dag(vec![units]).expect("no collision");
    assert_eq!(dag.len(), 2);
    // First node is presatisfied (no payload)
    assert!(dag.node(0).payload().payload.is_none());
    // Second node has payload
    assert!(dag.node(1).payload().payload.is_some());
}

fn cache_meta_for(recipe: &str, outputs: &[&str]) -> cook_contracts::CacheMeta {
    cook_contracts::CacheMeta {
        recipe_name: recipe.to_string(),
        project_id: String::new(),
        cookfile_path: String::new(),
        cache_key: format!("k_{recipe}"),
        inputs: vec![],
        consumes: Vec::new(),
        member_keyed: false,
        output_paths: outputs.iter().map(|s| s.to_string()).collect(),
        command_hash: 0,
        env_contribution: 0,
        consulted_env: BTreeMap::new(),
        discovered_inputs: None,
        seal_keys: Default::default(),
        sharing: Default::default(),
        record: false,
    }
}

#[test]
fn test_output_collision_unrelated_recipes_rejected() {
    // Two recipes, no dep edge, both declare the same output path.
    // build_dag MUST return EngineError::OutputCollision at plan time.
    let a = RecipeUnits {
        recipe_name: "a".into(),
        deps: vec![],
        units: vec![CapturedUnit {
            payload: shell("touch out"),
            cache_meta: Some(cache_meta_for("a", &["build/shared.bin"])),
            dep_kind: DepKind::Sequential,
            probes: vec![],
            unit_env_vars: Default::default(),
            member: None,
            output_paths: Vec::new(),
        }],
        step_groups: vec![],
        working_dir: default_wd(),
        env_vars: default_env(),
        terminal_outputs: vec!["build/shared.bin".into()],
        dep_edges: vec![],
        probes: vec![],
    };
    let b = RecipeUnits {
        recipe_name: "b".into(),
        deps: vec![],
        units: vec![CapturedUnit {
            payload: shell("touch out"),
            cache_meta: Some(cache_meta_for("b", &["build/shared.bin"])),
            dep_kind: DepKind::Sequential,
            probes: vec![],
            unit_env_vars: Default::default(),
            member: None,
            output_paths: Vec::new(),
        }],
        step_groups: vec![],
        working_dir: default_wd(),
        env_vars: default_env(),
        terminal_outputs: vec!["build/shared.bin".into()],
        dep_edges: vec![],
        probes: vec![],
    };
    let err = build_dag(vec![a, b]).expect_err("expected OutputCollision");
    match err {
        EngineError::OutputCollision { path, recipes } => {
            assert_eq!(path, default_wd().join("build/shared.bin"));
            assert!(recipes.contains(&"a".to_string()));
            assert!(recipes.contains(&"b".to_string()));
        }
        other => panic!("expected OutputCollision, got: {other:?}"),
    }
}

#[test]
fn test_output_collision_dep_related_recipes_allowed() {
    // Two recipes, b depends on a, both touch same output. Allowed because
    // the dep edge enforces ordering — no race.
    let a = RecipeUnits {
        recipe_name: "a".into(),
        deps: vec![],
        units: vec![CapturedUnit {
            payload: shell("touch out"),
            cache_meta: Some(cache_meta_for("a", &["build/shared.bin"])),
            dep_kind: DepKind::Sequential,
            probes: vec![],
            unit_env_vars: Default::default(),
            member: None,
            output_paths: Vec::new(),
        }],
        step_groups: vec![],
        working_dir: default_wd(),
        env_vars: default_env(),
        terminal_outputs: vec!["build/shared.bin".into()],
        dep_edges: vec![],
        probes: vec![],
    };
    let b = RecipeUnits {
        recipe_name: "b".into(),
        deps: vec!["a".into()],
        units: vec![CapturedUnit {
            payload: shell("touch out"),
            cache_meta: Some(cache_meta_for("b", &["build/shared.bin"])),
            dep_kind: DepKind::Sequential,
            probes: vec![],
            unit_env_vars: Default::default(),
            member: None,
            output_paths: Vec::new(),
        }],
        step_groups: vec![],
        working_dir: default_wd(),
        env_vars: default_env(),
        terminal_outputs: vec!["build/shared.bin".into()],
        dep_edges: vec![],
        probes: vec![],
    };
    let dag = build_dag(vec![a, b]).expect("dep edge allows shared output");
    assert_eq!(dag.len(), 2);
}

#[test]
fn unreached_probe_is_pruned_from_dag() {
    use cook_contracts::{CapturedUnit, DepKind, ProbeUnit, ProbeInputs, WorkPayload};

    let probe_payload = WorkPayload::Probe {
        key: "k:unused".to_string(),
        produce: "return 1".to_string(),
        line: 1,
    };
    let probe_meta = ProbeUnit {
        key: "k:unused".to_string(),
        produce_source: "return 1".to_string(),
        produce_line: 1,
        inputs: ProbeInputs::default(),
    };

    let units = RecipeUnits {
        recipe_name: "r".to_string(),
            deps: vec![],
            units: vec![
                CapturedUnit {
                    payload: probe_payload,
                    cache_meta: None,
                    dep_kind: DepKind::Sequential,
                    probes: vec![],
                    unit_env_vars: Default::default(),
                    member: None,
                    output_paths: Vec::new(),
                },
                CapturedUnit {
                    payload: WorkPayload::Shell {
                        cmd: "echo hello".to_string(),
                    line: 2,
                },
                cache_meta: None,
                dep_kind: DepKind::Sequential,
                probes: vec![],
                unit_env_vars: Default::default(),
                member: None,
                output_paths: Vec::new(),
            },
        ],
        step_groups: vec![],
        working_dir: std::path::PathBuf::from("/"),
        env_vars: std::collections::BTreeMap::new(),
        terminal_outputs: vec![],
        dep_edges: vec![],
        probes: vec![probe_meta],
    };

    let dag = build_dag(vec![units]).expect("dag build");
    let probe_nodes: Vec<_> = (0..dag.len())
        .map(|i| dag.node(i))
        .filter(|n| matches!(n.payload().payload, Some(WorkPayload::Probe { .. })))
        .collect();
    assert!(probe_nodes.is_empty(), "unreached probe must not appear in DAG");
}

#[test]
fn probe_chain_keeps_upstream_when_downstream_consumed() {
    use cook_contracts::{CapturedUnit, DepKind, ProbeUnit, ProbeInputs, WorkPayload};

    let probe_a_payload = WorkPayload::Probe {
        key: "k:a".to_string(),
        produce: "return 1".to_string(),
        line: 1,
    };
    let probe_b_payload = WorkPayload::Probe {
        key: "k:b".to_string(),
        produce: "return 2".to_string(),
        line: 2,
    };
    let probe_a_meta = ProbeUnit {
        key: "k:a".to_string(),
        produce_source: "return 1".to_string(),
        produce_line: 1,
        inputs: ProbeInputs::default(),
    };
    let probe_b_meta = ProbeUnit {
        key: "k:b".to_string(),
        produce_source: "return 2".to_string(),
        produce_line: 2,
        inputs: ProbeInputs {
            requires: vec!["k:a".to_string()],
            ..ProbeInputs::default()
        },
    };
    let probe_a = CapturedUnit {
        payload: probe_a_payload,
        cache_meta: None,
        dep_kind: DepKind::Sequential,
        probes: vec![],
        unit_env_vars: Default::default(),
        member: None,
        output_paths: Vec::new(),
    };
    let probe_b = CapturedUnit {
        payload: probe_b_payload,
        cache_meta: None,
        dep_kind: DepKind::Sequential,
        probes: vec![],
        unit_env_vars: Default::default(),
        member: None,
        output_paths: Vec::new(),
    };
    let consumer = CapturedUnit {
        payload: WorkPayload::Shell { cmd: "echo".to_string(), line: 3 },
        cache_meta: None,
        dep_kind: DepKind::Sequential,
        probes: vec!["k:b".to_string()],
        unit_env_vars: Default::default(),
        member: None,
        output_paths: Vec::new(),
    };

    let make_ru = |units: Vec<CapturedUnit>| RecipeUnits {
        recipe_name: "r".to_string(),
            units,
            deps: vec![],
            step_groups: vec![],
            working_dir: std::path::PathBuf::from("/"),
        env_vars: std::collections::BTreeMap::new(),
        terminal_outputs: vec![],
        dep_edges: vec![],
        probes: vec![probe_a_meta.clone(), probe_b_meta.clone()],
    };

    let with_consumer = make_ru(vec![probe_a.clone(), probe_b.clone(), consumer]);
    let dag = build_dag(vec![with_consumer]).unwrap();
    let probe_count = (0..dag.len())
        .map(|i| dag.node(i))
        .filter(|n| matches!(n.payload().payload, Some(WorkPayload::Probe { .. })))
        .count();
    assert_eq!(probe_count, 2, "both probes must be present when downstream is consumed");

    let without_consumer = make_ru(vec![probe_a, probe_b]);
    let dag2 = build_dag(vec![without_consumer]).unwrap();
    let probe_count2 = (0..dag2.len())
        .map(|i| dag2.node(i))
        .filter(|n| matches!(n.payload().payload, Some(WorkPayload::Probe { .. })))
        .count();
    assert_eq!(probe_count2, 0, "both probes must be pruned when nothing consumes downstream");
}

/// SHI-222 Phase 8 regression: top-level register-scope probes (whose
/// metadata flows into `RecipeUnits.probes` but which are NOT present as
/// `WorkPayload::Probe` entries in `RecipeUnits.units`) must materialise
/// as DAG nodes when a consumer's `probes` field references them.
/// Pre-fix, these probes were silently dropped; the consumer's
/// `cook.probes.get` returned nil at execute time.
#[test]
fn top_level_probe_materialises_when_consumer_references_it() {
    use cook_contracts::{CapturedUnit, DepKind, ProbeInputs, ProbeUnit, WorkPayload};

    let probe_meta = ProbeUnit {
        key: "cc:has_stdint_h".to_string(),
        produce_source: "return { ok = true }".to_string(),
        produce_line: 7,
        inputs: ProbeInputs::default(),
    };
    let ru = RecipeUnits {
        recipe_name: "game".into(),
        deps: vec![],
        // Note: NO Probe entry in units — only the consumer.
        units: vec![CapturedUnit {
            payload: shell("cc -o game main.c"),
            cache_meta: None,
            dep_kind: DepKind::Sequential,
            probes: vec!["cc:has_stdint_h".into()],
            unit_env_vars: Default::default(),
            member: None,
            output_paths: Vec::new(),
        }],
        step_groups: vec![],
        working_dir: default_wd(),
        env_vars: default_env(),
        terminal_outputs: vec![],
        dep_edges: vec![],
        probes: vec![probe_meta],
    };
    let dag = build_dag(vec![ru]).expect("no collision");
    assert_eq!(
        dag.len(),
        2,
        "expected synthesised probe node + consumer; got {} nodes",
        dag.len()
    );
    // Node 0 should be the synthesised Probe (no deps).
    assert!(
        matches!(dag.node(0).payload().payload, Some(WorkPayload::Probe { .. })),
        "node 0 must be the synthesised Probe"
    );
    assert_eq!(dag.node(0).remaining_deps(), 0);
    // Node 1 (consumer) depends on the synthesised probe.
    assert_eq!(
        dag.node(1).remaining_deps(),
        1,
        "consumer must depend on synthesised probe"
    );
}

/// SHI-222 Phase 8: synthesis must respect demand-driven scheduling —
/// a top-level probe that no consumer references is not synthesised.
#[test]
fn top_level_probe_not_synthesised_when_no_consumer() {
    use cook_contracts::{CapturedUnit, DepKind, ProbeInputs, ProbeUnit, WorkPayload};

    let probe_meta = ProbeUnit {
        key: "cc:unused".to_string(),
        produce_source: "return 1".to_string(),
        produce_line: 1,
        inputs: ProbeInputs::default(),
    };
    let ru = RecipeUnits {
        recipe_name: "r".into(),
            deps: vec![],
            units: vec![CapturedUnit {
                payload: shell("true"),
            cache_meta: None,
            dep_kind: DepKind::Sequential,
            probes: vec![], // no references
            unit_env_vars: Default::default(),
            member: None,
            output_paths: Vec::new(),
        }],
        step_groups: vec![],
        working_dir: default_wd(),
        env_vars: default_env(),
        terminal_outputs: vec![],
        dep_edges: vec![],
        probes: vec![probe_meta],
    };
    let dag = build_dag(vec![ru]).expect("no collision");
    let probe_nodes = (0..dag.len())
        .filter(|i| matches!(dag.node(*i).payload().payload, Some(WorkPayload::Probe { .. })))
        .count();
    assert_eq!(probe_nodes, 0, "unreferenced top-level probe must not be synthesised");
}

/// SHI-222 Phase 8: probe-on-probe transitive synthesis. If consumer
/// references probe B, and probe B's `inputs.requires` lists probe A,
/// both A and B must be synthesised, with B depending on A.
#[test]
fn top_level_probe_chain_synthesised_transitively() {
    use cook_contracts::{CapturedUnit, DepKind, ProbeInputs, ProbeUnit, WorkPayload};

    let probe_a = ProbeUnit {
        key: "cc:a".into(),
        produce_source: "return 1".into(),
        produce_line: 1,
        inputs: ProbeInputs::default(),
    };
    let probe_b = ProbeUnit {
        key: "cc:b".into(),
        produce_source: "return 2".into(),
        produce_line: 2,
        inputs: ProbeInputs {
            requires: vec!["cc:a".into()],
            ..ProbeInputs::default()
        },
    };
    let ru = RecipeUnits {
        recipe_name: "r".into(),
            deps: vec![],
            units: vec![CapturedUnit {
                payload: shell("true"),
            cache_meta: None,
            dep_kind: DepKind::Sequential,
            probes: vec!["cc:b".into()],
            unit_env_vars: Default::default(),
            member: None,
            output_paths: Vec::new(),
        }],
        step_groups: vec![],
        working_dir: default_wd(),
        env_vars: default_env(),
        terminal_outputs: vec![],
        dep_edges: vec![],
        probes: vec![probe_a, probe_b],
    };
    let dag = build_dag(vec![ru]).expect("no collision");
    // 2 probes + 1 consumer = 3
    assert_eq!(dag.len(), 3);
    // Find nodes by key.
    let mut a_id = None;
    let mut b_id = None;
    for i in 0..dag.len() {
        if let Some(WorkPayload::Probe { key, .. }) = &dag.node(i).payload().payload {
            if key == "cc:a" {
                a_id = Some(i);
            } else if key == "cc:b" {
                b_id = Some(i);
            }
        }
    }
    let a_id = a_id.expect("probe A must be synthesised");
    let b_id = b_id.expect("probe B must be synthesised");
    // A has no deps, B depends on A.
    assert_eq!(dag.node(a_id).remaining_deps(), 0, "probe A must have no deps");
    assert_eq!(dag.node(b_id).remaining_deps(), 1, "probe B must depend on probe A");
    // Topo order: A added before B (A's dag_id < B's).
    assert!(a_id < b_id, "probe A must be added before probe B");
}

/// Regression: body-scope probe-on-body-scope-probe chains must NOT be
/// demand-pruned. The cook_cc `needs = {...}` shape registers a chain
/// where `cc:find:NAME` (body-scope) declares `inputs.requires =
/// ["cc:linker-search-dirs", ...]` and `cc:linker-search-dirs` is also a
/// body-scope probe. Pre-fix, `compute_consumed_probe_keys` only walked
/// upstreams through `ru.probes` (top-level), so the body-scope upstream
/// was never added to `consumed` and got dropped from the DAG, causing
/// `cook-fingerprint` to fail with "requires upstream X which has no
/// fingerprint" at execute time.
#[test]
fn body_scope_probe_chain_not_pruned() {
    use cook_contracts::{CapturedUnit, DepKind, WorkPayload};

    // Body-scope upstream probe (e.g. `cc:linker-search-dirs`).
    let upstream_probe = CapturedUnit {
        payload: WorkPayload::Probe {
            key: "cc:linker-search-dirs".into(),
            produce: "return {}".into(),
            line: 1,
        },
        cache_meta: None,
        dep_kind: DepKind::Sequential,
        probes: vec![], // no upstream of its own
        unit_env_vars: Default::default(),
        member: None,
        output_paths: Vec::new(),
    };
    // Body-scope consumer probe (e.g. `cc:find:SDL3`) requiring the
    // upstream body-scope probe.
    let downstream_probe = CapturedUnit {
        payload: WorkPayload::Probe {
            key: "cc:find:SDL3".into(),
            produce: "return {}".into(),
            line: 2,
        },
        cache_meta: None,
        dep_kind: DepKind::Sequential,
        probes: vec!["cc:linker-search-dirs".into()],
        unit_env_vars: Default::default(),
        member: None,
        output_paths: Vec::new(),
    };
    // Non-probe consumer (the link unit) listing only the downstream
    // probe in its `probes`. The upstream must still survive the
    // demand-driven prune via the transitive closure across body-scope
    // probes.
    let link_unit = CapturedUnit {
        payload: shell("link"),
        cache_meta: None,
        dep_kind: DepKind::Sequential,
        probes: vec!["cc:find:SDL3".into()],
        unit_env_vars: Default::default(),
        member: None,
        output_paths: Vec::new(),
    };

    let ru = RecipeUnits {
        recipe_name: "game".into(),
        deps: vec![],
        units: vec![upstream_probe, downstream_probe, link_unit],
        step_groups: vec![],
        working_dir: default_wd(),
        env_vars: default_env(),
        terminal_outputs: vec![],
        dep_edges: vec![],
        probes: vec![], // body-scope probes are NOT mirrored into ru.probes
    };
    let dag = build_dag(vec![ru]).expect("no collision");
    // Both probe nodes must survive; otherwise pruning regressed.
    let probe_keys: BTreeSet<String> = (0..dag.len())
        .filter_map(|i| match &dag.node(i).payload().payload {
            Some(WorkPayload::Probe { key, .. }) => Some(key.clone()),
            _ => None,
        })
        .collect();
    assert!(
        probe_keys.contains("cc:linker-search-dirs"),
        "body-scope upstream probe must survive demand prune, got nodes: {probe_keys:?}"
    );
    assert!(
        probe_keys.contains("cc:find:SDL3"),
        "body-scope consumer probe must survive demand prune, got nodes: {probe_keys:?}"
    );
    assert_eq!(dag.len(), 3, "expected 2 probes + 1 link unit");
}

/// Independent body-scope probes (no `inputs.requires` between them) must
/// not be serialised through the per-recipe barrier. Each must have zero
/// remaining deps so the executor can dispatch them in parallel.
///
/// This is the sdl3-game case: `cook_cc.bin` registers `cc:compiler:auto`,
/// `cc:linker-search-dirs`, `cc:cmake-driver` (all sibling, none requires
/// another) before the link unit. Pre-fix the dag-builder treated the
/// probes' `DepKind::Sequential` as advancing the barrier and produced a
/// chain A→B→C, so they ran one at a time.
#[test]
fn independent_body_scope_probes_run_in_parallel() {
    let probe_a = CapturedUnit {
        payload: probe("cc:a"),
        cache_meta: None,
        dep_kind: DepKind::Sequential,
        probes: vec![],
        unit_env_vars: Default::default(),
        member: None,
        output_paths: Vec::new(),
    };
    let probe_b = CapturedUnit {
        payload: probe("cc:b"),
        cache_meta: None,
        dep_kind: DepKind::Sequential,
        probes: vec![],
        unit_env_vars: Default::default(),
        member: None,
        output_paths: Vec::new(),
    };
    let probe_c = CapturedUnit {
        payload: probe("cc:c"),
        cache_meta: None,
        dep_kind: DepKind::Sequential,
        probes: vec![],
        unit_env_vars: Default::default(),
        member: None,
        output_paths: Vec::new(),
    };
    let consumer = CapturedUnit {
        payload: shell("link"),
        cache_meta: None,
        dep_kind: DepKind::Sequential,
        probes: vec!["cc:a".into(), "cc:b".into(), "cc:c".into()],
        unit_env_vars: Default::default(),
        member: None,
        output_paths: Vec::new(),
    };
    let ru = RecipeUnits {
        recipe_name: "game".into(),
        deps: vec![],
        units: vec![probe_a, probe_b, probe_c, consumer],
        step_groups: vec![],
        working_dir: default_wd(),
        env_vars: default_env(),
        terminal_outputs: vec![],
        dep_edges: vec![],
        probes: vec![],
    };
    let dag = build_dag(vec![ru]).expect("no collision");
    assert_eq!(dag.len(), 4, "3 probes + 1 consumer");
        // Each probe is a root (no deps).
        for i in 0..3 {
            assert_eq!(
                dag.node(i).remaining_deps(),
                0,
                "probe node {i} must have no deps; got {}",
            dag.node(i).remaining_deps()
        );
    }
    // Consumer (node 3) depends on all 3 probes via its `probes` list.
    assert_eq!(
        dag.node(3).remaining_deps(),
        3,
        "consumer must depend on all 3 probes"
    );
}

/// Probe chains via `inputs.requires` must still serialise correctly even
/// when the dag-builder skips barrier participation for probes.
/// Companion to `independent_body_scope_probes_run_in_parallel`.
#[test]
fn dependent_body_scope_probes_still_serialise_through_inputs_requires() {
    let probe_a = CapturedUnit {
        payload: probe("cc:a"),
        cache_meta: None,
        dep_kind: DepKind::Sequential,
        probes: vec![],
        unit_env_vars: Default::default(),
        member: None,
        output_paths: Vec::new(),
    };
    let probe_b = CapturedUnit {
        payload: probe("cc:b"),
        cache_meta: None,
        dep_kind: DepKind::Sequential,
        probes: vec!["cc:a".into()],
        unit_env_vars: Default::default(),
        member: None,
        output_paths: Vec::new(),
    };
    let probe_c = CapturedUnit {
        payload: probe("cc:c"),
        cache_meta: None,
        dep_kind: DepKind::Sequential,
        probes: vec![],
        unit_env_vars: Default::default(),
        member: None,
        output_paths: Vec::new(),
    };
    let consumer = CapturedUnit {
        payload: shell("link"),
        cache_meta: None,
        dep_kind: DepKind::Sequential,
        probes: vec!["cc:b".into(), "cc:c".into()],
        unit_env_vars: Default::default(),
        member: None,
        output_paths: Vec::new(),
    };
    let ru = RecipeUnits {
        recipe_name: "game".into(),
        deps: vec![],
        units: vec![probe_a, probe_b, probe_c, consumer],
        step_groups: vec![],
        working_dir: default_wd(),
        env_vars: default_env(),
        terminal_outputs: vec![],
        dep_edges: vec![],
        probes: vec![],
    };
    let dag = build_dag(vec![ru]).expect("no collision");
    assert_eq!(dag.len(), 4);
    // A: root (no deps).
    assert_eq!(dag.node(0).remaining_deps(), 0, "probe A has no deps");
    // B: depends on A only.
    assert_eq!(
        dag.node(1).remaining_deps(),
        1,
        "probe B depends only on A"
    );
    // C: root (no deps, sibling to A).
    assert_eq!(dag.node(2).remaining_deps(), 0, "probe C has no deps");
    // Consumer: depends on B and C (2 distinct edges).
    assert_eq!(
        dag.node(3).remaining_deps(),
        2,
        "consumer depends on B and C"
    );
}

/// A non-probe unit appearing between probes must still impose a sequential
/// barrier: probes do not advance the barrier, but they do not break an
/// existing one either. This keeps the build/link ordering correct in
/// recipes that interleave probes with shell work.
#[test]
fn non_probe_units_around_probes_keep_barrier() {
    let pre_shell = CapturedUnit {
        payload: shell("pre"),
        cache_meta: None,
        dep_kind: DepKind::Sequential,
        probes: vec![],
        unit_env_vars: Default::default(),
        member: None,
        output_paths: Vec::new(),
    };
    let p = CapturedUnit {
        payload: probe("cc:x"),
        cache_meta: None,
        dep_kind: DepKind::Sequential,
        probes: vec![],
        unit_env_vars: Default::default(),
        member: None,
        output_paths: Vec::new(),
    };
    let post_shell = CapturedUnit {
        payload: shell("post"),
        cache_meta: None,
        dep_kind: DepKind::Sequential,
        probes: vec!["cc:x".into()],
        unit_env_vars: Default::default(),
        member: None,
        output_paths: Vec::new(),
    };
    let ru = RecipeUnits {
        recipe_name: "r".into(),
            deps: vec![],
            units: vec![pre_shell, p, post_shell],
            step_groups: vec![],
            working_dir: default_wd(),
            env_vars: default_env(),
            terminal_outputs: vec![],
            dep_edges: vec![],
            probes: vec![],
        };
        let dag = build_dag(vec![ru]).expect("no collision");
    assert_eq!(dag.len(), 3);
    assert_eq!(dag.node(0).remaining_deps(), 0, "pre shell is root");
    // Probe sees barrier=[pre_shell] but does not depend on it sequentially.
    // (Per design: probes are pure fact-gathering; ordering vs surrounding
    // work flows through inputs.requires / consumer.probes.)
    assert_eq!(dag.node(1).remaining_deps(), 0, "probe is independent");
    // post_shell depends on pre_shell (barrier preserved through the probe)
    // and on the probe (via probes edge). Dedup keeps it at 2 if both are
    // distinct nodes — they are.
    assert_eq!(
        dag.node(2).remaining_deps(),
        2,
        "post shell depends on pre shell (barrier) + probe (probes edge)"
    );
}

#[test]
fn multi_recipe_wave_prunes_independently() {
    use cook_contracts::{CapturedUnit, DepKind, ProbeUnit, ProbeInputs, WorkPayload};

    fn make_recipe(name: &str, has_consumer: bool) -> RecipeUnits {
        let probe_meta = ProbeUnit {
            key: "k:p".to_string(),
            produce_source: "return 1".to_string(),
            produce_line: 1,
            inputs: ProbeInputs::default(),
        };
        let mut units = vec![CapturedUnit {
            payload: WorkPayload::Probe {
                key: "k:p".to_string(),
                produce: "return 1".to_string(),
                line: 1,
            },
            cache_meta: None,
            dep_kind: DepKind::Sequential,
            probes: vec![],
            unit_env_vars: Default::default(),
            member: None,
            output_paths: Vec::new(),
        }];
        units.push(CapturedUnit {
            payload: WorkPayload::Shell { cmd: "echo".to_string(), line: 2 },
            cache_meta: None,
            dep_kind: DepKind::Sequential,
            probes: if has_consumer { vec!["k:p".to_string()] } else { vec![] },
            unit_env_vars: Default::default(),
            member: None,
            output_paths: Vec::new(),
        });
        RecipeUnits {
            recipe_name: name.to_string(),
            units,
            deps: vec![],
            step_groups: vec![],
            working_dir: std::path::PathBuf::from("/"),
            env_vars: std::collections::BTreeMap::new(),
            terminal_outputs: vec![],
            dep_edges: vec![],
            probes: vec![probe_meta],
        }
    }

    let foo = make_recipe("foo", true);
    let bar = make_recipe("bar", false);
        let dag = build_dag(vec![foo, bar]).unwrap();
        let probe_node_recipes: Vec<String> = (0..dag.len())
            .map(|i| dag.node(i))
            .filter(|n| matches!(n.payload().payload, Some(WorkPayload::Probe { .. })))
            .map(|n| n.payload().recipe_name.clone())
            .collect();
        assert_eq!(probe_node_recipes, vec!["foo".to_string()],
        "probe present only in the recipe that consumes it");
}

#[test]
fn test_output_collision_distinct_outputs_allowed() {
    let a = RecipeUnits {
        recipe_name: "a".into(),
        deps: vec![],
        units: vec![CapturedUnit {
            payload: shell("touch out"),
            cache_meta: Some(cache_meta_for("a", &["build/a.bin"])),
            dep_kind: DepKind::Sequential,
            probes: vec![],
            unit_env_vars: Default::default(),
            member: None,
            output_paths: Vec::new(),
        }],
        step_groups: vec![],
        working_dir: default_wd(),
        env_vars: default_env(),
        terminal_outputs: vec!["build/a.bin".into()],
        dep_edges: vec![],
        probes: vec![],
    };
    let b = RecipeUnits {
        recipe_name: "b".into(),
        deps: vec![],
        units: vec![CapturedUnit {
            payload: shell("touch out"),
            cache_meta: Some(cache_meta_for("b", &["build/b.bin"])),
            dep_kind: DepKind::Sequential,
            probes: vec![],
            unit_env_vars: Default::default(),
            member: None,
            output_paths: Vec::new(),
        }],
        step_groups: vec![],
        working_dir: default_wd(),
        env_vars: default_env(),
        terminal_outputs: vec!["build/b.bin".into()],
        dep_edges: vec![],
        probes: vec![],
    };
    let dag = build_dag(vec![a, b]).expect("distinct outputs OK");
    assert_eq!(dag.len(), 2);
}
