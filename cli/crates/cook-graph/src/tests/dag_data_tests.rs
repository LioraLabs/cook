use super::*;
use cook_contracts::{
    CapturedUnit, DepKind, DiscoveredInputs, RecipeUnits, WorkPayload,
};
use std::collections::BTreeMap;
use std::sync::Arc;
use tempfile::TempDir;

/// Build a single-unit RecipeUnits whose cache_meta declares
/// `discovered_inputs = { from = depfile_rel, format = "make" }`.
fn recipe_with_depfile(
    recipe_name: &str,
    working_dir: std::path::PathBuf,
    source: &str,
    output: &str,
    depfile_rel: &str,
) -> (String, RecipeUnits) {
    let cache_meta = cook_contracts::CacheMeta {
        recipe_name: recipe_name.into(),
        project_id: "p".into(),
        cookfile_path: "Cookfile".into(),
        cache_key: "k0".into(),
        inputs: vec![source.into()],
        consumes: Vec::new(),
        member_keyed: false,
        output_paths: vec![output.into()],
        command_hash: 0,
        env_contribution: 0,
        consulted_env: BTreeMap::new(),
        discovered_inputs: Some(DiscoveredInputs {
            from: depfile_rel.into(),
            format: "make".into(),
        }),
        seal_keys: Default::default(),
        sharing: Default::default(),
        record: false,
    };
    let unit = CapturedUnit {
        payload: WorkPayload::Shell {
            cmd: format!("clang++ -c {source} -o {output}"),
            line: 1,
        },
        cache_meta: Some(cache_meta),
        dep_kind: DepKind::Sequential,
        probes: vec![],
        unit_env_vars: Default::default(),
        member: None,
        output_paths: Vec::new(),
            test_name: None,
    };
    let ru = RecipeUnits {
        recipe_name: recipe_name.into(),
        deps: vec![],
        units: vec![unit],
        step_groups: vec![],
        working_dir,
        env_vars: BTreeMap::new(),
        terminal_outputs: vec![output.into()],
        dep_edges: vec![],
        probes: vec![],
    };
    (recipe_name.into(), ru)
}

fn write_depfile(working_dir: &std::path::Path, rel: &str, body: &str) {
    let path = working_dir.join(rel);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(&path, body).unwrap();
}

/// Make sure each header listed in the depfile actually exists on disk
/// (the parser drops nonexistent paths). Touches an empty file at each.
fn touch(working_dir: &std::path::Path, rels: &[&str]) {
    for rel in rels {
        let path = working_dir.join(rel);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(&path, "").unwrap();
    }
}

/// Probe-to-probe edges via `inputs.requires` must be the only sequencing
/// the viewer renders between probes. Independent sibling probes must NOT
/// have a barrier-driven chain edge between them — that misrepresents
/// parallelism in the visualisation and contradicts the engine's
/// dag_builder which (post-fix) keeps independent probes parallel.
#[test]
fn independent_probes_have_no_edges_between_them() {
    let probe_a = CapturedUnit {
        payload: WorkPayload::Probe {
            key: "cc:a".into(),
            produce: "return 1".into(),
            line: 1,
        },
        cache_meta: None,
        dep_kind: DepKind::Sequential,
        probes: vec![],
        unit_env_vars: Default::default(),
        member: None,
        output_paths: Vec::new(),
            test_name: None,
    };
    let probe_b = CapturedUnit {
        payload: WorkPayload::Probe {
            key: "cc:b".into(),
            produce: "return 2".into(),
            line: 2,
        },
        cache_meta: None,
        dep_kind: DepKind::Sequential,
        probes: vec![],
        unit_env_vars: Default::default(),
        member: None,
        output_paths: Vec::new(),
            test_name: None,
    };
    let consumer = CapturedUnit {
        payload: WorkPayload::Shell { cmd: "link".into(), line: 3 },
        cache_meta: None,
        dep_kind: DepKind::Sequential,
        probes: vec!["cc:a".into(), "cc:b".into()],
        unit_env_vars: Default::default(),
        member: None,
        output_paths: Vec::new(),
            test_name: None,
    };
    let ru = RecipeUnits {
        recipe_name: "game".into(),
        deps: vec![],
        units: vec![probe_a, probe_b, consumer],
        step_groups: vec![],
        working_dir: std::path::PathBuf::from("/"),
        env_vars: BTreeMap::new(),
        terminal_outputs: vec![],
        dep_edges: vec![],
        probes: vec![],
    };
    let all_units = vec![("game".into(), ru)];
    let explicit: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let cms: BTreeMap<String, Arc<cook_cache::ThreadSafeCacheManager>> = BTreeMap::new();

    let g = build_dag_data("game", &all_units, &explicit, &cms);
    let edges = &g.edges;
    // No edge between the two independent probes.
    let probe_to_probe = edges.iter().any(|e| {
        (e.from == "unit:game:0" && e.to == "unit:game:1")
            || (e.from == "unit:game:1" && e.to == "unit:game:0")
    });
    assert!(
        !probe_to_probe,
        "independent probes must not be chained; edges: {}",
        edges
            .iter()
            .map(|e| format!("{}->{}", e.from, e.to))
            .collect::<Vec<_>>()
            .join(", ")
    );
    // Consumer must still depend on both probes — sequencing flows through
    // each unit's `probes` field, not the barrier.
    let has_edge = |from: &str, to: &str| {
        edges.iter().any(|e| e.from == from && e.to == to)
    };
    assert!(
        has_edge("unit:game:0", "unit:game:2"),
        "consumer must depend on probe A",
    );
    assert!(
        has_edge("unit:game:1", "unit:game:2"),
        "consumer must depend on probe B",
    );
}

#[test]
fn build_dag_data_emits_discovered_file_nodes() {
    let tmp = TempDir::new().unwrap();
    let wd = tmp.path().to_path_buf();

    // Touch the source and the headers so they all exist; the depfile
    // parser filters out nonexistent paths.
    touch(&wd, &["bar.cpp", "helpers.h", "math.h"]);
    write_depfile(
        &wd,
        "bar.d",
        "bar.o: bar.cpp \\\n  helpers.h \\\n  math.h\n",
    );

    let (name, ru) = recipe_with_depfile("compile", wd.clone(), "bar.cpp", "bar.o", "bar.d");
    let all_units = vec![(name.clone(), ru)];
    let explicit: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let cms: BTreeMap<String, Arc<cook_cache::ThreadSafeCacheManager>> = BTreeMap::new();

    let g = build_dag_data("build", &all_units, &explicit, &cms);
    let nodes = &g.nodes;
    let by_id = |id: &str| nodes.iter().find(|n| n.id == id);

    let bar_cpp = by_id("file:bar.cpp").expect("declared file node missing");
    assert_eq!(bar_cpp.discovered, None, "declared file should not be flagged discovered");

    let helpers = by_id("file:helpers.h").expect("discovered helpers.h missing");
    assert_eq!(helpers.discovered, Some(true));

    let math = by_id("file:math.h").expect("discovered math.h missing");
    assert_eq!(math.discovered, Some(true));

    let edges = &g.edges;
    let has_edge = |from: &str, to: &str| {
        edges.iter().any(|e| e.from == from && e.to == to)
    };
    assert!(has_edge("file:bar.cpp", "unit:compile:0"));
    assert!(has_edge("file:helpers.h", "unit:compile:0"));
    assert!(has_edge("file:math.h", "unit:compile:0"));
}

#[test]
fn discovered_path_declared_by_other_unit_is_classified_declared() {
    let tmp = TempDir::new().unwrap();
    let wd = tmp.path().to_path_buf();
    touch(&wd, &["a.cpp", "b.cpp", "shared.h"]);
    // a discovers shared.h via depfile.
    write_depfile(&wd, "a.d", "a.o: a.cpp shared.h\n");
    // b declares shared.h explicitly (no depfile).

    // Recipe A is processed first (alphabetical via wave_grouper) — it
    // would otherwise set `discovered = Some(true)` on shared.h.
    let cm_a = cook_contracts::CacheMeta {
        recipe_name: "a".into(),
        project_id: "p".into(),
        cookfile_path: "Cookfile".into(),
        cache_key: "k_a".into(),
        inputs: vec!["a.cpp".into()],
        consumes: Vec::new(),
        member_keyed: false,
        output_paths: vec!["a.o".into()],
        command_hash: 0,
        env_contribution: 0,
        consulted_env: BTreeMap::new(),
        discovered_inputs: Some(DiscoveredInputs {
            from: "a.d".into(),
            format: "make".into(),
        }),
        seal_keys: Default::default(),
        sharing: Default::default(),
        record: false,
    };
    let unit_a = CapturedUnit {
        payload: WorkPayload::Shell { cmd: "clang -c a.cpp".into(), line: 1 },
        cache_meta: Some(cm_a),
        dep_kind: DepKind::Sequential,
        probes: vec![],
        unit_env_vars: Default::default(),
        member: None,
        output_paths: Vec::new(),
            test_name: None,
    };
    let ru_a = RecipeUnits {
        recipe_name: "a".into(),
        deps: vec![],
        units: vec![unit_a],
        step_groups: vec![],
        working_dir: wd.clone(),
        env_vars: BTreeMap::new(),
        terminal_outputs: vec!["a.o".into()],
        dep_edges: vec![],
        probes: vec![],
    };

    let cm_b = cook_contracts::CacheMeta {
        recipe_name: "b".into(),
        project_id: "p".into(),
        cookfile_path: "Cookfile".into(),
        cache_key: "k_b".into(),
        inputs: vec!["b.cpp".into(), "shared.h".into()],
        consumes: Vec::new(),
        member_keyed: false,
        output_paths: vec!["b.o".into()],
        command_hash: 0,
        env_contribution: 0,
        consulted_env: BTreeMap::new(),
        discovered_inputs: None,
        seal_keys: Default::default(),
        sharing: Default::default(),
        record: false,
    };
    let unit_b = CapturedUnit {
        payload: WorkPayload::Shell { cmd: "clang -c b.cpp".into(), line: 1 },
        cache_meta: Some(cm_b),
        dep_kind: DepKind::Sequential,
        probes: vec![],
        unit_env_vars: Default::default(),
        member: None,
        output_paths: Vec::new(),
            test_name: None,
    };
    let ru_b = RecipeUnits {
        recipe_name: "b".into(),
        deps: vec![],
        units: vec![unit_b],
        step_groups: vec![],
        working_dir: wd,
        env_vars: BTreeMap::new(),
        terminal_outputs: vec!["b.o".into()],
        dep_edges: vec![],
        probes: vec![],
    };

    let all_units = vec![("a".into(), ru_a), ("b".into(), ru_b)];
    let explicit: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let cms: BTreeMap<String, Arc<cook_cache::ThreadSafeCacheManager>> = BTreeMap::new();

    let g = build_dag_data("build", &all_units, &explicit, &cms);

    // shared.h is declared by one unit and discovered by the other; the
    // declared classification wins regardless of processing order.
    let shared = g
        .nodes
        .iter()
        .find(|n| n.id == "file:shared.h")
        .expect("shared.h node missing");
    assert_eq!(
        shared.discovered, None,
        "a path declared by another unit must not be classified discovered",
    );

    // Both units have an edge from shared.h.
    let has_edge = |to: &str| {
        g.edges
            .iter()
            .any(|e| e.from == "file:shared.h" && e.to == to)
    };
    assert!(has_edge("unit:a:0"));
    assert!(has_edge("unit:b:0"));
}

#[test]
fn missing_depfile_does_not_panic_or_emit_discovered() {
    let tmp = TempDir::new().unwrap();
    let wd = tmp.path().to_path_buf();
    touch(&wd, &["bar.cpp"]);
    // Note: no bar.d on disk.

    let (name, ru) = recipe_with_depfile("compile", wd.clone(), "bar.cpp", "bar.o", "bar.d");
    let all_units = vec![(name, ru)];
    let explicit: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let cms: BTreeMap<String, Arc<cook_cache::ThreadSafeCacheManager>> = BTreeMap::new();

    let g = build_dag_data("build", &all_units, &explicit, &cms);

    
    // Declared file is present; no discovered nodes.
    assert!(g.nodes.iter().any(|n| n.id == "file:bar.cpp"));
    assert!(
        !g.nodes.iter().any(|n| n.discovered == Some(true)),
        "no discovered nodes when depfile is missing",
    );
}

#[test]
fn malformed_depfile_does_not_panic_or_emit_discovered() {
    let tmp = TempDir::new().unwrap();
    let wd = tmp.path().to_path_buf();
    touch(&wd, &["bar.cpp"]);
    // No ':' in the file → `parse_make_depfile` returns Malformed.
    write_depfile(&wd, "bar.d", "this is not a valid depfile body\n");

    let (name, ru) = recipe_with_depfile("compile", wd.clone(), "bar.cpp", "bar.o", "bar.d");
    let all_units = vec![(name, ru)];
    let explicit: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let cms: BTreeMap<String, Arc<cook_cache::ThreadSafeCacheManager>> = BTreeMap::new();

    let g = build_dag_data("build", &all_units, &explicit, &cms);

    
    assert!(g.nodes.iter().any(|n| n.id == "file:bar.cpp"));
    assert!(
        !g.nodes.iter().any(|n| n.discovered == Some(true)),
        "no discovered nodes when depfile is malformed",
    );
}

#[test]
fn discovered_path_that_is_a_unit_output_is_not_emitted_as_file() {
    let tmp = TempDir::new().unwrap();
    let wd = tmp.path().to_path_buf();
    // Two units: archive consumes a.o (the compile unit's output).
    // Contrived: archive's depfile lists a.o (which would normally be
    // an inter-unit edge, not a file node). The discovered loop must
    // skip it because a.o is in unit_output_paths.
    touch(&wd, &["a.cpp", "a.o"]);
    write_depfile(&wd, "archive.d", "libfoo.a: a.o\n");

    let cm_compile = cook_contracts::CacheMeta {
        recipe_name: "compile".into(),
        project_id: "p".into(),
        cookfile_path: "Cookfile".into(),
        cache_key: "k_c".into(),
        inputs: vec!["a.cpp".into()],
        consumes: Vec::new(),
        member_keyed: false,
        output_paths: vec!["a.o".into()],
        command_hash: 0,
        env_contribution: 0,
        consulted_env: BTreeMap::new(),
        discovered_inputs: None,
        seal_keys: Default::default(),
        sharing: Default::default(),
        record: false,
    };
    let unit_compile = CapturedUnit {
        payload: WorkPayload::Shell { cmd: "clang -c a.cpp".into(), line: 1 },
        cache_meta: Some(cm_compile),
        dep_kind: DepKind::Sequential,
        probes: vec![],
        unit_env_vars: Default::default(),
        member: None,
        output_paths: Vec::new(),
            test_name: None,
    };
    let ru_compile = RecipeUnits {
        recipe_name: "compile".into(),
        deps: vec![],
        units: vec![unit_compile],
        step_groups: vec![],
        working_dir: wd.clone(),
        env_vars: BTreeMap::new(),
        terminal_outputs: vec!["a.o".into()],
        dep_edges: vec![],
        probes: vec![],
    };

    let cm_archive = cook_contracts::CacheMeta {
        recipe_name: "archive".into(),
        project_id: "p".into(),
        cookfile_path: "Cookfile".into(),
        cache_key: "k_a".into(),
        inputs: vec!["a.o".into()],
        consumes: Vec::new(),
        member_keyed: false,
        output_paths: vec!["libfoo.a".into()],
        command_hash: 0,
        env_contribution: 0,
        consulted_env: BTreeMap::new(),
        discovered_inputs: Some(DiscoveredInputs {
            from: "archive.d".into(),
            format: "make".into(),
        }),
        seal_keys: Default::default(),
        sharing: Default::default(),
        record: false,
    };
    let unit_archive = CapturedUnit {
        payload: WorkPayload::Shell { cmd: "ar rcs libfoo.a a.o".into(), line: 1 },
        cache_meta: Some(cm_archive),
        dep_kind: DepKind::Sequential,
        probes: vec![],
        unit_env_vars: Default::default(),
        member: None,
        output_paths: Vec::new(),
            test_name: None,
    };
    let ru_archive = RecipeUnits {
        recipe_name: "archive".into(),
        deps: vec![],
        units: vec![unit_archive],
        step_groups: vec![],
        working_dir: wd,
        env_vars: BTreeMap::new(),
        terminal_outputs: vec!["libfoo.a".into()],
        dep_edges: vec![],
        probes: vec![],
    };

    let all_units = vec![("compile".into(), ru_compile), ("archive".into(), ru_archive)];
    let explicit: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let cms: BTreeMap<String, Arc<cook_cache::ThreadSafeCacheManager>> = BTreeMap::new();

    let g = build_dag_data("build", &all_units, &explicit, &cms);

    // Across whatever wave layout the grouper picks, no `file:a.o` ever
    // appears — a.o is a unit output, not a source file.
    {
        assert!(
            !g.nodes.iter().any(|n| n.id == "file:a.o"),
            "a.o is a unit output and must not be emitted as a file node",
        );
    }
}

/// The on-disk cache index is written under the unit's Cookfile-local
/// `CacheMeta.recipe_name` ("build"), not the (import-qualified) workspace key
/// the recipe is registered under ("rust.build"). Loading the index under the
/// qualified key finds nothing, and every file node then renders as modified
/// even when a fresh, matching cache entry exists. This pins that the index
/// lookup follows cache_meta.
///
/// CS-0171 note: this used to assert through the per-unit `cached` verdict,
/// which is gone — the cache verdict now comes from `cook_engine::why`, which
/// has no second index-name derivation to get wrong. The file-staleness flag
/// is the remaining consumer of the recipe cache in this crate, so it is what
/// pins the lookup now.
#[test]
fn cache_lookup_uses_cache_meta_recipe_name_not_qualified_key() {
    let tmp = TempDir::new().unwrap();
    let wd = tmp.path().to_path_buf();
    touch(&wd, &["source.cpp", "output.o"]);

    let source_record = cook_fingerprint::FileRecord {
        path: "source.cpp".into(),
        mtime: stat_mtime(&wd.join("source.cpp")).unwrap(),
        hash: hash_file(&wd.join("source.cpp")).unwrap(),
    };
    let output_record = cook_fingerprint::FileRecord {
        path: "output.o".into(),
        mtime: stat_mtime(&wd.join("output.o")).unwrap(),
        hash: hash_file(&wd.join("output.o")).unwrap(),
    };

    let cache_meta = cook_contracts::CacheMeta {
        // Cookfile-local name: differs from the qualified workspace key
        // ("rust.build") the recipe is registered under below.
        recipe_name: "build".into(),
        project_id: "p".into(),
        cookfile_path: "Cookfile".into(),
        cache_key: "k1".into(),
        inputs: vec!["source.cpp".into()],
        consumes: Vec::new(),
        member_keyed: false,
        output_paths: vec!["output.o".into()],
        command_hash: 0,
        env_contribution: 0,
        consulted_env: BTreeMap::new(),
        discovered_inputs: None,
        seal_keys: Default::default(),
        sharing: Default::default(),
        record: false,
    };
    let unit = CapturedUnit {
        payload: WorkPayload::Shell {
            cmd: "clang++ -c source.cpp -o output.o".into(),
            line: 1,
        },
        cache_meta: Some(cache_meta),
        dep_kind: DepKind::Sequential,
        probes: vec![],
        unit_env_vars: Default::default(),
        member: None,
        output_paths: Vec::new(),
            test_name: None,
    };
    let ru = RecipeUnits {
        // Qualified workspace key (import-aliased), used both as the
        // map key below and as `target`.
        recipe_name: "rust.build".into(),
        deps: vec![],
        units: vec![unit],
        step_groups: vec![],
        working_dir: wd,
        env_vars: BTreeMap::new(),
        terminal_outputs: vec!["output.o".into()],
        dep_edges: vec![],
        probes: vec![],
    };

    let mgr = cook_cache::ThreadSafeCacheManager::new(tmp.path().join(".cook/cache"));
    // Populate the in-memory cache under the *local* name only — mirrors
    // what the executor writes on disk (`recipe_cache_index_name`).
    mgr.update_step(
        "build",
        "k1",
        cook_fingerprint::StepEntry {
            inputs: vec![source_record],
            outputs: vec![output_record],
            command_hash: 0,
            env_contribution: 0,
            seal_contribution: 0,
        observed: None,
        },
    );

    let all_units = vec![("rust.build".into(), ru)];
    let explicit: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut cms: BTreeMap<String, Arc<cook_cache::ThreadSafeCacheManager>> = BTreeMap::new();
    cms.insert("rust.build".into(), Arc::new(mgr));

    let g = build_dag_data("rust.build", &all_units, &explicit, &cms);

    let file = g
        .nodes
        .iter()
        .find(|n| n.id == "file:source.cpp")
        .expect("declared input file node missing");
    assert_eq!(
        file.modified,
        Some(false),
        "cache index must be looked up under cache_meta.recipe_name (\"build\"), \
         not the qualified workspace key (\"rust.build\"); a missed lookup has \
         no recorded mtime/hash to compare against and reports the input as \
         modified",
    );

    // And the unit node carries the join key the verdict will arrive on.
    let unit = g
        .nodes
        .iter()
        .find(|n| n.id == "unit:rust.build:0")
        .expect("unit node missing");
    assert_eq!(unit.cache_key.as_deref(), Some("k1"));
}
