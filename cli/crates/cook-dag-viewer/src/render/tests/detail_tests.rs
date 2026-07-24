use super::*;
use crate::dag_data::{EdgeData, NodeData, WaveData, WaveDagData};
use crate::frame::SnapshotFrame;
use crate::state::{AppState, Selection};

fn graph() -> WaveDagData {
    WaveDagData {
        schema_version: crate::VIEWER_SCHEMA_VERSION,
        target: "build".into(),
        waves: vec![WaveData {
            recipes: vec!["a".into()],
            nodes: vec![
                NodeData {
                    id: "file:foo.cpp".into(),
                    kind: "file".into(),
                    label: "foo.cpp".into(),
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
                    id: "unit:a:0".into(),
                    kind: "unit".into(),
                    label: "foo.o".into(),
                    recipe: Some("a".into()),
                    command: Some("clang -c foo.cpp".into()),
                    output: Some("foo.o".into()),
                    cached: Some(true),
                    dep_kind: Some("sequential".into()),
                    group_index: None,
                    modified: None,
                    discovered: None,
                },
            ],
            edges: vec![EdgeData {
                from: "file:foo.cpp".into(),
                to: "unit:a:0".into(),
            }],
        }],
        inter_wave_edges: vec![],
    }
}

fn first_line(buf: &Buffer, area: Rect) -> String {
    (area.x..area.x + area.width)
        .map(|x| buf.cell((x, area.y)).unwrap().symbol().to_string())
        .collect::<String>()
        .trim_end()
        .to_string()
}

#[test]
fn renders_header_and_inputs() {
    let g = graph();
    let mut app = AppState::new(&g);
    app.tree.waves[0].recipes[0].expanded = true;
    app.selection = Selection::unit(0, 0, 0);
    let frame = SnapshotFrame::new(g);
    let area = Rect::new(0, 0, 80, 6);
    let mut buf = Buffer::empty(area);
    render(area, &mut buf, &app, &frame);

    assert!(first_line(&buf, area).contains("unit:a:0"));
    assert!(first_line(&buf, area).contains("✓ cached"));
    // Row 2 should mention the file input.
    let row2: String = (0..80)
        .map(|x| buf.cell((x, 2)).unwrap().symbol().to_string())
        .collect();
    assert!(row2.contains("file:foo.cpp"));
}

#[test]
fn renders_no_selection_message_when_at_wave_level() {
    let g = graph();
    let app = AppState::new(&g); // first() => wave-level selection
    let frame = SnapshotFrame::new(g);
    let area = Rect::new(0, 0, 40, 6);
    let mut buf = Buffer::empty(area);
    render(area, &mut buf, &app, &frame);
    assert!(first_line(&buf, area).contains("(no selection)"));
}

fn graph_with_discovered() -> WaveDagData {
    WaveDagData {
        schema_version: crate::VIEWER_SCHEMA_VERSION,
        target: "build".into(),
        waves: vec![WaveData {
            recipes: vec!["a".into()],
            nodes: vec![
                NodeData {
                    id: "file:foo.cpp".into(),
                    kind: "file".into(),
                    label: "foo.cpp".into(),
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
                    id: "file:helpers.h".into(),
                    kind: "file".into(),
                    label: "helpers.h".into(),
                    recipe: None,
                    command: None,
                    output: None,
                    cached: None,
                    dep_kind: None,
                    group_index: None,
                    modified: None,
                    discovered: Some(true),
                },
                NodeData {
                    id: "unit:a:0".into(),
                    kind: "unit".into(),
                    label: "foo.o".into(),
                    recipe: Some("a".into()),
                    command: Some("clang -c foo.cpp".into()),
                    output: Some("foo.o".into()),
                    cached: Some(true),
                    dep_kind: Some("sequential".into()),
                    group_index: None,
                    modified: None,
                    discovered: None,
                },
            ],
            edges: vec![
                EdgeData {
                    from: "file:foo.cpp".into(),
                    to: "unit:a:0".into(),
                },
                EdgeData {
                    from: "file:helpers.h".into(),
                    to: "unit:a:0".into(),
                },
            ],
        }],
        inter_wave_edges: vec![],
    }
}

fn read_row(buf: &Buffer, area: Rect, row: u16) -> String {
    (area.x..area.x + area.width)
        .map(|x| buf.cell((x, area.y + row)).unwrap().symbol().to_string())
        .collect::<String>()
        .trim_end()
        .to_string()
}

#[test]
fn detail_pane_lists_discovered_inputs_separately() {
    let g = graph_with_discovered();
    let mut app = AppState::new(&g);
    app.tree.waves[0].recipes[0].expanded = true;
    app.selection = Selection::unit(0, 0, 0);
    let frame = SnapshotFrame::new(g);
    let area = Rect::new(0, 0, 80, 6);
    let mut buf = Buffer::empty(area);
    render(area, &mut buf, &app, &frame);

    // The declared inputs row mentions foo.cpp (existing behaviour).
    assert!(read_row(&buf, area, 2).contains("file:foo.cpp"));

    // A new row mentions the discovered input. Look across rows 3 and
    // 4 — the order is implementation-pinned to row 3 below, but if
    // future styling shifts it the test only cares it appears somewhere
    // visible.
    let combined = format!("{}\n{}", read_row(&buf, area, 3), read_row(&buf, area, 4));
    assert!(
        combined.contains("file:helpers.h") && combined.to_lowercase().contains("discovered"),
        "expected a discovered inputs row mentioning helpers.h, got:\n{combined}",
    );
}

#[test]
fn detail_pane_omits_discovered_row_when_unit_has_none() {
    let g = graph(); // existing fixture: declared input only, no discovered
    let mut app = AppState::new(&g);
    app.tree.waves[0].recipes[0].expanded = true;
    app.selection = Selection::unit(0, 0, 0);
    let frame = SnapshotFrame::new(g);
    let area = Rect::new(0, 0, 80, 6);
    let mut buf = Buffer::empty(area);
    render(area, &mut buf, &app, &frame);

    // Whichever row layout we choose, no row should contain "discovered"
    // when the unit has none.
    for row in 0..area.height {
        let line = read_row(&buf, area, row);
        assert!(
            !line.to_lowercase().contains("discovered"),
            "row {row} should not mention discovered: {line}",
        );
    }
}
