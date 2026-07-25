use super::*;
use crate::dag_data::{NodeData, WaveData, WaveDagData};
use crate::frame::SnapshotFrame;
use crate::state::AppState;

fn unit(id: &str, recipe: &str, label: &str, cached: Option<bool>) -> NodeData {
    NodeData {
        id: id.into(),
        kind: "unit".into(),
        label: label.into(),
        recipe: Some(recipe.into()),
        command: Some("cmd".into()),
        output: None,
        cached,
        dep_kind: Some("sequential".into()),
        group_index: None,
        modified: None,
        discovered: None,
    }
}

fn graph() -> WaveDagData {
    WaveDagData {
        schema_version: crate::VIEWER_SCHEMA_VERSION,
        target: "build".into(),
        waves: vec![WaveData {
            recipes: vec!["a".into()],
            nodes: vec![
                unit("unit:a:0", "a", "a0", Some(true)),
                unit("unit:a:1", "a", "a1", Some(false)),
            ],
            edges: vec![],
        }],
        inter_wave_edges: vec![],
    }
}

fn cell_at(buf: &Buffer, x: u16, y: u16) -> char {
    buf.cell((x, y)).unwrap().symbol().chars().next().unwrap_or(' ')
}

#[test]
fn renders_collapsed_wave_only() {
    let g = graph();
    let app = AppState::new(&g);
    let frame = SnapshotFrame::new(g);
    let area = Rect::new(0, 0, 28, 5);
    let mut buf = Buffer::empty(area);
    render(area, &mut buf, &app, &frame);

    // Wave 0 (1 recipes) is at row 0
    assert_eq!(cell_at(&buf, 0, 0), '▼');
}

#[test]
fn renders_expanded_recipe_with_units_and_badges() {
    let g = graph();
    let mut app = AppState::new(&g);
    app.tree.waves[0].recipes[0].expanded = true;
    let frame = SnapshotFrame::new(g);
    let area = Rect::new(0, 0, 28, 5);
    let mut buf = Buffer::empty(area);
    render(area, &mut buf, &app, &frame);

    // Row 1 = recipe `a` at indent 2 (expanded).
    assert_eq!(cell_at(&buf, 2, 1), '▼');
    // Row 2 = unit a0 cached → ✓.
    assert_eq!(cell_at(&buf, 4, 2), '●');
}

fn graph_with_files() -> WaveDagData {
    use crate::dag_data::{EdgeData, EdgeKind};
    WaveDagData {
        schema_version: crate::VIEWER_SCHEMA_VERSION,
        target: "build".into(),
        waves: vec![WaveData {
            recipes: vec!["a".into()],
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
                    modified: Some(true),
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
                    modified: Some(false),
                    discovered: Some(true),
                },
                unit("unit:a:0", "a", "a0", Some(true)),
            ],
            edges: vec![
                EdgeData { from: "file:bar.cpp".into(), to: "unit:a:0".into(), kind: EdgeKind::Data },
                EdgeData { from: "file:helpers.h".into(), to: "unit:a:0".into(), kind: EdgeKind::Data },
            ],
        }],
        inter_wave_edges: vec![],
    }
}

fn row_text(buf: &Buffer, area: Rect, y: u16) -> String {
    (area.x..area.x + area.width)
        .map(|x| buf.cell((x, y)).unwrap().symbol().to_string())
        .collect::<String>()
        .trim_end()
        .to_string()
}

#[test]
fn files_folder_header_renders_with_count() {
    let g = graph_with_files();
    let app = AppState::new(&g); // wave 0 expanded by default; files folder collapsed
    let frame = SnapshotFrame::new(g);
    let area = Rect::new(0, 0, 28, 6);
    let mut buf = Buffer::empty(area);
    render(area, &mut buf, &app, &frame);

    // Row 0: wave header. Row 1: Files folder header (collapsed).
    let line = row_text(&buf, area, 1);
    assert!(line.contains("Files (2)"), "expected `Files (2)` header, got: `{line}`");
    assert!(line.contains('▶'), "collapsed folder uses the right-pointing triangle");
}

#[test]
fn files_folder_expanded_lists_files_alphabetical() {
    let g = graph_with_files();
    let mut app = AppState::new(&g);
    app.tree.waves[0].files_expanded = true;
    let frame = SnapshotFrame::new(g);
    let area = Rect::new(0, 0, 28, 8);
    let mut buf = Buffer::empty(area);
    render(area, &mut buf, &app, &frame);

    // Row 1 = Files folder header (expanded with ▼).
    assert!(row_text(&buf, area, 1).contains('▼'));
    // Row 2 = first file alphabetically: bar.cpp (declared, modified).
    let bar_row = row_text(&buf, area, 2);
    assert!(bar_row.contains("bar.cpp"), "row 2 should contain bar.cpp, got: `{bar_row}`");
    assert!(bar_row.contains('▢'), "declared file uses ▢ glyph");
    assert!(bar_row.contains('⚠'), "modified file uses ⚠ badge");

    // Row 3 = helpers.h (discovered, clean).
    let helpers_row = row_text(&buf, area, 3);
    assert!(helpers_row.contains("helpers.h"));
    assert!(helpers_row.contains('◇'), "discovered file uses ◇ glyph");
    assert!(helpers_row.contains('·'), "clean file uses · badge");
}

#[test]
fn files_folder_hidden_when_wave_has_no_files() {
    let g = graph(); // existing fixture in this mod with one unit, no files
    let app = AppState::new(&g);
    let frame = SnapshotFrame::new(g);
    let area = Rect::new(0, 0, 28, 4);
    let mut buf = Buffer::empty(area);
    render(area, &mut buf, &app, &frame);

    // Row 0 = wave header. Row 1 should be the recipe row (no Files header).
    let line = row_text(&buf, area, 1);
    assert!(!line.contains("Files"), "no Files header for empty-files wave");
}

#[test]
fn files_folder_header_is_reversed_when_selected() {
    let g = graph_with_files();
    let mut app = AppState::new(&g);
    app.selection = Selection::files_folder(0);
    let frame = SnapshotFrame::new(g);
    let area = Rect::new(0, 0, 28, 6);
    let mut buf = Buffer::empty(area);
    render(area, &mut buf, &app, &frame);

    // Row 1 = Files folder header. The first non-blank cell (the ▶/▼ glyph
    // at indent 2) must carry REVERSED.
    let cell = buf.cell((2, 1)).unwrap();
    assert!(
        cell.style().add_modifier.contains(Modifier::REVERSED),
        "expected folder-header glyph to be REVERSED when selected"
    );
}

#[test]
fn scrolled_view_skips_rows_above_the_offset() {
    // Wave 0 expanded with one recipe expanded → many unit rows.
    let mut nodes: Vec<NodeData> = Vec::new();
    for i in 0..10 {
        nodes.push(unit(&format!("unit:a:{i}"), "a", &format!("u{i}"), Some(true)));
    }
    let g = WaveDagData {
        schema_version: crate::VIEWER_SCHEMA_VERSION,
        target: "build".into(),
        waves: vec![WaveData {
            recipes: vec!["a".into()],
            nodes,
            edges: vec![],
        }],
        inter_wave_edges: vec![],
    };
    let mut app = AppState::new(&g);
    app.tree.waves[0].recipes[0].expanded = true;
    // Visible rows: wave_only=0, recipe(0)=1, unit(0..10)=2..11. Total 12.
    // Scroll past the wave + recipe rows → unit u0 should be the first row.
    app.index_scroll = 2;
    let frame = SnapshotFrame::new(g);
    let area = Rect::new(0, 0, 28, 5);
    let mut buf = Buffer::empty(area);
    render(area, &mut buf, &app, &frame);

    // Row 0 must NOT be the wave header.
    let r0 = row_text(&buf, area, 0);
    assert!(!r0.contains("Wave 0"), "row 0 should be scrolled past wave header, got: `{r0}`");
    assert!(r0.contains("u0"), "row 0 should be unit u0, got: `{r0}`");
    // Row 1 should be u1.
    let r1 = row_text(&buf, area, 1);
    assert!(r1.contains("u1"), "row 1 should be unit u1, got: `{r1}`");
}

#[test]
fn files_folder_header_is_not_reversed_when_unselected() {
    let g = graph_with_files();
    let app = AppState::new(&g); // default selection = wave_only(0)
    let frame = SnapshotFrame::new(g);
    let area = Rect::new(0, 0, 28, 6);
    let mut buf = Buffer::empty(area);
    render(area, &mut buf, &app, &frame);

    let cell = buf.cell((2, 1)).unwrap();
    assert!(!cell.style().add_modifier.contains(Modifier::REVERSED));
}

#[test]
fn long_filename_truncates_with_ellipsis_before_badge() {
    use crate::dag_data::{EdgeData, EdgeKind};
    let g = WaveDagData {
        schema_version: crate::VIEWER_SCHEMA_VERSION,
        target: "build".into(),
        waves: vec![WaveData {
            recipes: vec!["a".into()],
            nodes: vec![
                NodeData {
                    id: "file:include/platform/threading.h".into(),
                    kind: "file".into(),
                    label: "include/platform/threading.h".into(),
                    recipe: None,
                    command: None,
                    output: None,
                    cached: None,
                    dep_kind: None,
                    group_index: None,
                    modified: Some(true),
                    discovered: None,
                },
                unit("unit:a:0", "a", "a0", Some(true)),
            ],
            edges: vec![EdgeData {
                from: "file:include/platform/threading.h".into(),
                to: "unit:a:0".into(), kind: EdgeKind::Data }],
        }],
        inter_wave_edges: vec![],
    };
    let mut app = AppState::new(&g);
    app.tree.waves[0].files_expanded = true;
    let frame = SnapshotFrame::new(g);
    let area = Rect::new(0, 0, 28, 6);
    let mut buf = Buffer::empty(area);
    render(area, &mut buf, &app, &frame);

    // Row 2 = the truncated file row. It must end with the badge `⚠` at
    // the right edge, with `…` somewhere in the label rather than the
    // tail of the filename being clobbered.
    let file_row = row_text(&buf, area, 2);
    assert!(
        file_row.contains('…'),
        "long filename must show the ellipsis truncation marker, got: `{file_row}`",
    );
    assert!(
        file_row.contains('⚠'),
        "badge must remain visible at right edge, got: `{file_row}`",
    );
    // The badge cell should not be clobbered by the label — confirm the
    // last non-space character isn't a slash or letter from the filename.
    let trimmed = file_row.trim_end();
    let last_char = trimmed.chars().last().unwrap_or(' ');
    assert_eq!(last_char, '⚠', "rightmost cell must be the badge, got: `{trimmed}`");
}
