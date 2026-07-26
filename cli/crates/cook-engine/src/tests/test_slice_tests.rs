use super::*;
use cook_contracts::{CapturedUnit, DepKind, StepKind, WorkPayload};
use std::collections::BTreeSet;

/// Build a LuaChunk unit that declares the given output paths.
/// Used as the "cook step" stand-in since WorkPayload has no Cook variant;
/// LuaChunk is the payload emitted for declarative cook steps.
fn mk_cook(outputs: &[&str]) -> CapturedUnit {
    CapturedUnit {
        payload: WorkPayload::LuaChunk {
            code: "cook.sh(\"echo > \" .. output)".into(),
            inputs: vec![],
            outputs: outputs.iter().map(|s| s.to_string()).collect(),
            ingredient_groups: vec![],
            step_kind: StepKind::Cook,
            is_chore: false,
            line: 0,
        },
        cache_meta: None,
        dep_kind: DepKind::Sequential,
        probes: vec![],
        unit_env_vars: Default::default(),
        member: None,
        output_paths: Vec::new(),
    }
}

fn mk_test() -> CapturedUnit {
    CapturedUnit {
        payload: WorkPayload::Test {
            seal_keys: Default::default(),
            consumes: Vec::new(),
            cmd: "true".into(),
            line: 1,
            timeout: 30,
            should_fail: false,
            suite_name: "r".into(),
                test_name: "t".into(),
            iteration_item: None,
            lua_code: None,
            input_paths: vec![],
        },
        cache_meta: None,
        dep_kind: DepKind::Sequential,
        probes: vec![],
        unit_env_vars: Default::default(),
        member: None,
        output_paths: Vec::new(),
    }
}

#[test]
fn build_test_slice_excludes_unrelated_cook_units() {
    // Units:
    //   #0: cook produces "needed.bin"  (test #2 depends on this)
    //   #1: cook produces "unrelated.bin" (no test depends)
    //   #2: test depends on "needed.bin" via dep_edges
    //   #3: test (one-shot, no deps)
    let units = vec![
        mk_cook(&["needed.bin"]),
        mk_cook(&["unrelated.bin"]),
        mk_test(),
        mk_test(),
    ];
    let dep_edges = vec![(2usize, "needed.bin".to_string())];

    let slice = build_test_slice(&units, &dep_edges);
    let s: BTreeSet<_> = slice.iter().copied().collect();
    assert!(s.contains(&0), "cook needed by a test must be in slice");
    assert!(s.contains(&2), "test units always in slice");
    assert!(s.contains(&3), "one-shot test always in slice");
    assert!(!s.contains(&1), "unrelated cook must be excluded");
}

#[test]
fn build_test_slice_handles_transitive_deps() {
    // #0 cook produces "a.out"
    // #1 cook produces "b.out", depends on "a.out"
    // #2 test depends on "b.out"
    let units = vec![
        mk_cook(&["a.out"]),
        mk_cook(&["b.out"]),
        mk_test(),
    ];
    let dep_edges = vec![
        (1usize, "a.out".to_string()),
        (2usize, "b.out".to_string()),
    ];
    let slice = build_test_slice(&units, &dep_edges);
    assert_eq!(slice.len(), 3, "transitive cook deps must be included; got: {slice:?}");
}

#[test]
fn build_test_slice_empty_when_no_tests() {
    let units = vec![mk_cook(&["x.out"])];
    let dep_edges = vec![];
    let slice = build_test_slice(&units, &dep_edges);
    assert!(slice.is_empty(), "no test units => empty slice");
}

#[test]
fn build_test_slice_all_tests_no_deps() {
    let units = vec![mk_test(), mk_test(), mk_test()];
    let dep_edges = vec![];
    let slice = build_test_slice(&units, &dep_edges);
    assert_eq!(slice, vec![0, 1, 2], "all test units with no deps");
}
