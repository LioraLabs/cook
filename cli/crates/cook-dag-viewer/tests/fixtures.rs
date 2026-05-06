//! Shared `WaveDagData` fixtures for renderer / state tests.
//!
//! Each builder returns a `WaveDagData` with a known structure so tests can
//! make precise assertions. Keep these small and hand-rolled — they are the
//! ground truth, not generated.

#![allow(dead_code)] // each test binary uses a different subset.

use cook_dag_viewer::{EdgeData, NodeData, WaveData, WaveDagData};

fn cached_unit_node(id: &str, recipe: &str, label: &str) -> NodeData {
    NodeData {
        id: id.into(),
        kind: "unit".into(),
        label: label.into(),
        recipe: Some(recipe.into()),
        command: Some("c".into()),
        output: None,
        cached: Some(true),
        dep_kind: Some("sequential".into()),
        group_index: None,
        modified: None,
        discovered: None,
    }
}

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
                        discovered: None,
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
                        discovered: None,
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
                    discovered: None,
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

pub fn small_dag() -> WaveDagData {
    // 3 waves × 2 units = 6 unit nodes, plus 2 inter-wave edges.
    let waves = (0..3)
        .map(|w| WaveData {
            recipes: vec![format!("r{}", w)],
            nodes: (0..2)
                .map(|i| cached_unit_node(
                    &format!("u:{}-{}", w, i),
                    &format!("r{}", w),
                    &format!("u{}{}", w, i),
                ))
                .collect(),
            edges: vec![],
        })
        .collect();
    let inter_wave_edges = (0..2)
        .map(|w| EdgeData {
            from: format!("u:{}-1", w),
            to: format!("u:{}-0", w + 1),
        })
        .collect();
    WaveDagData {
        schema_version: cook_dag_viewer::VIEWER_SCHEMA_VERSION,
        target: "build".into(),
        waves,
        inter_wave_edges,
    }
}

pub fn medium_dag() -> WaveDagData {
    // 6 waves × 5 units = 30 unit nodes; ~3 cross-wave edges per wave gap.
    let waves = (0..6)
        .map(|w| WaveData {
            recipes: vec![format!("r{}", w)],
            nodes: (0..5)
                .map(|i| cached_unit_node(
                    &format!("u:{}-{}", w, i),
                    &format!("r{}", w),
                    &format!("u{}{}", w, i),
                ))
                .collect(),
            edges: vec![],
        })
        .collect();
    let inter_wave_edges = (0..5)
        .flat_map(|w| {
            (0..3).map(move |i| EdgeData {
                from: format!("u:{}-{}", w, i),
                to: format!("u:{}-{}", w + 1, i),
            })
        })
        .collect();
    WaveDagData {
        schema_version: cook_dag_viewer::VIEWER_SCHEMA_VERSION,
        target: "build".into(),
        waves,
        inter_wave_edges,
    }
}

pub fn wide_dag() -> WaveDagData {
    // 8 waves × 10 units = 80 unit nodes.
    let waves = (0..8)
        .map(|w| WaveData {
            recipes: vec![format!("r{}", w)],
            nodes: (0..10)
                .map(|i| cached_unit_node(
                    &format!("u:{}-{}", w, i),
                    &format!("r{}", w),
                    &format!("u{}{}", w, i),
                ))
                .collect(),
            edges: vec![],
        })
        .collect();
    let inter_wave_edges = (0..7)
        .flat_map(|w| {
            (0..5).map(move |i| EdgeData {
                from: format!("u:{}-{}", w, i),
                to: format!("u:{}-{}", w + 1, i),
            })
        })
        .collect();
    WaveDagData {
        schema_version: cook_dag_viewer::VIEWER_SCHEMA_VERSION,
        target: "build".into(),
        waves,
        inter_wave_edges,
    }
}
