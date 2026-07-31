use super::*;
use cook_progress::event::{NodeId, NodeKind, RecipeId};
use cook_progress::log_reader::{NodeView, RecipeView};
use cook_progress::model::{NodeStatus, Status};
use ratatui::backend::TestBackend;
use std::collections::BTreeMap;

fn one_failed_build() -> BuildView {
    let mut nodes = BTreeMap::new();
    nodes.insert(
        NodeId::new(0),
        NodeView {
            name: "lvm.c".into(),
            status: NodeStatus::Failed,
            kind: NodeKind::Cooked,
            started_at: None,
            ended_at: None,
            elapsed_ms: Some(1100),
            skip_reason: None,
            lines: vec![cook_progress::log_reader::LogLine {
                stream: cook_progress::event::Stream::Stderr,
                ts: None,
                text: "error: undeclared 'foo'".into(),
            }],
        },
    );
    let mut recipes = BTreeMap::new();
    recipes.insert(
        RecipeId::new(0),
        RecipeView {
            name: "vm".into(),
            status: Status::Failed,
            nodes,
        },
    );
    BuildView {
        build_id: "2026-05-10-abc".into(),
        started_at: "2026-05-10T10:00:00Z".into(),
        ended_at: Some("2026-05-10T10:00:12Z".into()),
        exit_code: Some(1),
        recipes,
    }
}

#[test]
fn renders_one_frame_with_failed_node_visible() {
    let backend = TestBackend::new(80, 24);
    let mut terminal = Terminal::new(backend).unwrap();
    let mut state = UiState::new(one_failed_build(), LoadDiagnostics::default());
    let frame = terminal
        .draw(|f| draw_frame(f, &mut state, &Theme::default()))
        .unwrap();
    let content: String = frame
        .buffer
        .content()
        .iter()
        .map(|c| c.symbol())
        .collect();
    assert!(content.contains("lvm.c"), "tree pane should show node name");
    assert!(
        content.contains("error: undeclared"),
        "output pane should show log line"
    );
    assert!(
        content.contains("2026-05-10-abc"),
        "header should show build id"
    );
}

#[test]
fn print_logs_fallback_renders_build_recipe_node_and_lines() {
    let view = one_failed_build();
    let mut buf: Vec<u8> = Vec::new();
    write_logs_fallback(&view, &mut buf).unwrap();
    let out = String::from_utf8(buf).unwrap();

    assert!(out.contains("2026-05-10-abc"), "should show build id");
    assert!(out.contains("exit Some(1)"), "should show exit code");
    assert!(out.contains("vm"), "should show recipe name");
    assert!(out.contains("Failed"), "should show recipe/node status");
    assert!(out.contains("lvm.c"), "should show node name");
    // COOK-392: elapsed renders under the one duration law (1100ms → 1.1s;
    // the raw "{ms}ms" spelling was the sixth independent renderer).
    assert!(out.contains("1.1s"), "should show node elapsed time");
    assert!(
        out.contains("error: undeclared 'foo'"),
        "should show log line text"
    );
}
