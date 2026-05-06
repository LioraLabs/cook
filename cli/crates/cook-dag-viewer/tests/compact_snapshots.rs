//! Compact-mode rendering snapshots — verifies the focus filter survives
//! through the Sugiyama layout + bracketed-label canvas pipeline.

use cook_dag_viewer::frame::SnapshotFrame;
use cook_dag_viewer::render::canvas;
use cook_dag_viewer::render::pick_layout;
use cook_dag_viewer::state::{AppState, DensityMode, Selection};

mod fixtures;

#[test]
fn compact_unit_focus_canvas_excludes_distant_nodes() {
    // small_dag: u:0-0, u:0-1 in wave 0; u:1-0, u:1-1 in wave 1; …; with
    // inter-wave edges u:0-1→u:1-0 and u:1-1→u:2-0. Unit-level focus on
    // u:0-1 brings in u:1-0 (1-hop), nothing further.
    let g = fixtures::small_dag();
    let mut app = AppState::new(&g);
    app.density = DensityMode::Compact;
    // Recipe r0 has units u:0-0 (index 0), u:0-1 (index 1). Pick u:0-1.
    app.tree.waves[0].recipes[0].expanded = true;
    app.selection = Selection::unit(0, 0, 1);

    let frame = SnapshotFrame::new(g.clone());
    let layout = pick_layout(&app, &g);
    let buf = canvas::render(&layout, &app, &frame);

    let dump: String = (0..buf.area().height)
        .map(|y| {
            (0..buf.area().width)
                .map(|x| buf.cell((x, y)).unwrap().symbol().to_string())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n");

    // Selected unit's label should appear.
    assert!(
        dump.contains("u01"),
        "selected unit label `u01` should appear in canvas:\n{dump}",
    );
    // 1-hop downstream neighbor should appear.
    assert!(
        dump.contains("u10"),
        "1-hop downstream label `u10` should appear in canvas:\n{dump}",
    );
    // 2-hop and further nodes should NOT appear.
    assert!(
        !dump.contains("u20"),
        "wave-2 label `u20` should NOT appear in compact-focus canvas:\n{dump}",
    );
    assert!(
        !dump.contains("u21"),
        "wave-2 label `u21` should NOT appear in compact-focus canvas:\n{dump}",
    );
    assert!(
        !dump.contains("u11"),
        "wave-1 sibling `u11` is not 1-hop from u01 and should NOT appear:\n{dump}",
    );
}
