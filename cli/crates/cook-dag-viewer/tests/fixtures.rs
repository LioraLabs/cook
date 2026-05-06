//! Shared `WaveDagData` fixtures for renderer / state tests.
//!
//! Each builder returns a `WaveDagData` with a known structure so tests can
//! make precise assertions. Keep these small and hand-rolled — they are the
//! ground truth, not generated.

#![allow(dead_code)] // each test binary uses a different subset.

use cook_dag_viewer::{EdgeData, NodeData, WaveData, WaveDagData};

pub fn three_wave_dag() -> WaveDagData {
    WaveDagData {
        schema_version: cook_dag_viewer::VIEWER_SCHEMA_VERSION,
        target: "build".into(),
        waves: vec![
            WaveData {
                recipes: vec!["cpp.compile".into()],
                nodes: vec![
                    NodeData {
                        id: "file:bar.cpp".into(),
                        kind: "file".into(),
                        label: "bar.cpp".into(),
                        recipe: None,
                        command: None,
                        output: None,
                        cached: None,
                        dep_kind: None,
                        group_index: None,
                        modified: Some(false),
                    },
                    NodeData {
                        id: "unit:cpp.compile:0".into(),
                        kind: "unit".into(),
                        label: "bar.o".into(),
                        recipe: Some("cpp.compile".into()),
                        command: Some("clang++ -c bar.cpp -o bar.o".into()),
                        output: Some("bar.o".into()),
                        cached: Some(true),
                        dep_kind: Some("sequential".into()),
                        group_index: None,
                        modified: None,
                    },
                ],
                edges: vec![EdgeData {
                    from: "file:bar.cpp".into(),
                    to: "unit:cpp.compile:0".into(),
                }],
            },
            WaveData {
                recipes: vec!["cpp.link".into()],
                nodes: vec![NodeData {
                    id: "unit:cpp.link:0".into(),
                    kind: "unit".into(),
                    label: "libfoo.a".into(),
                    recipe: Some("cpp.link".into()),
                    command: Some("ar rcs libfoo.a bar.o".into()),
                    output: Some("libfoo.a".into()),
                    cached: Some(false),
                    dep_kind: Some("sequential".into()),
                    group_index: None,
                    modified: None,
                }],
                edges: vec![],
            },
            WaveData { recipes: vec![], nodes: vec![], edges: vec![] },
        ],
        inter_wave_edges: vec![EdgeData {
            from: "unit:cpp.compile:0".into(),
            to: "unit:cpp.link:0".into(),
        }],
    }
}
