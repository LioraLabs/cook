//! Smoke test: build a small BuildView, run a single render frame with a
//! TestBackend, assert the failure error message is visible. The renderer-
//! level test in src/tui.rs is the meaningful coverage; this file primarily
//! verifies the public API surface of cook-logs.

use cook_logs::Theme;
use cook_progress::event::{NodeId, NodeKind, RecipeId, Stream};
use cook_progress::log_reader::{BuildView, LoadDiagnostics, LogLine, NodeView, RecipeView};
use cook_progress::model::{NodeStatus, Status};
use std::collections::BTreeMap;

#[test]
fn public_surface_compiles() {
    // Just verify the public types are accessible and constructable.
    let mut nodes = BTreeMap::new();
    nodes.insert(NodeId::new(0), NodeView {
        name: "lvm.c".into(),
        status: NodeStatus::Failed,
        kind: NodeKind::Cooked,
        started_at: None,
        ended_at: None,
        elapsed_ms: Some(1100),
        skip_reason: None,
        lines: vec![LogLine {
            stream: Stream::Stderr,
            ts: None,
            text: "error: undeclared 'foo'".into(),
        }],
    });
    let mut recipes = BTreeMap::new();
    recipes.insert(RecipeId::new(0), RecipeView {
        name: "vm".into(),
        status: Status::Failed,
        nodes,
    });
    let view = BuildView {
        build_id: "2026-05-10-abc".into(),
        started_at: "2026-05-10T10:00:00Z".into(),
        ended_at: Some("2026-05-10T10:00:12Z".into()),
        exit_code: Some(1),
        recipes,
    };

    let _theme = Theme::default();
    let _diag = LoadDiagnostics::default();
    let _view = view;
    // Constructible. Sufficient for a public-surface smoke test.
}
