use super::*;
use crate::dag_data::{NodeData, WaveData, WaveDagData};

fn dag() -> WaveDagData {
    WaveDagData {
        schema_version: crate::VIEWER_SCHEMA_VERSION,
        target: "build".into(),
        waves: vec![WaveData {
            recipes: vec!["cpp.compile".into()],
            nodes: vec![
                NodeData {
                    id: "unit:cpp.compile:0".into(),
                    kind: "unit".into(),
                    label: "foo.o".into(),
                    recipe: Some("cpp.compile".into()),
                    command: Some("clang -c foo.cpp".into()),
                    output: None,
                    cached: Some(true),
                    dep_kind: Some("sequential".into()),
                    group_index: None,
                    modified: None,
                    discovered: None,
                },
                NodeData {
                    id: "unit:cpp.compile:1".into(),
                    kind: "unit".into(),
                    label: "bar.o".into(),
                    recipe: Some("cpp.compile".into()),
                    command: Some("clang -c bar.cpp".into()),
                    output: None,
                    cached: Some(false),
                    dep_kind: Some("sequential".into()),
                    group_index: None,
                    modified: None,
                    discovered: None,
                },
            ],
            edges: vec![],
        }],
        inter_wave_edges: vec![],
    }
}

#[test]
fn fuzzy_match_finds_substring() {
    let g = dag();
    let mut s = SearchState::default();
    s.query = "bar".into();
        s.update(&g);
        assert!(s.matches.contains(&"unit:cpp.compile:1".to_string()));
}

#[test]
fn empty_query_returns_no_matches() {
    let g = dag();
    let mut s = SearchState::default();
    s.update(&g);
    assert!(s.matches.is_empty());
}
