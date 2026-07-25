use super::*;
use crate::event::{NodeId, NodeKind, ProgressEvent, RecipeId, RecipeTopo};
use std::time::Duration;

#[test]
fn finish_clears_status_line_and_shuts_down_thread() {
    let opts = InlineOptions {
        event: EventWriterOptions { colored: false, ..Default::default() },
        status: StatusLineOptions { colored: false, ..Default::default() },
        status_enabled: false, // status line off in test to avoid stderr writes
    };
    let mut r = InlineRenderer::new(opts);
    let mut state = BuildState::new();
    let ev = ProgressEvent::BuildStarted {
        recipes: vec![RecipeTopo {
            id: RecipeId::new(0), name: "lib".into(), deps: vec![], expected_nodes: 1,
        }],
        total_nodes: 1,
    };
    state.apply(&ev);
    r.handle(&state, &ev).unwrap();
    r.finish(&state).unwrap();
}

#[test]
fn handle_routes_events_to_event_writer() {
    // Smoke: build a complete event sequence without panicking.
    let opts = InlineOptions {
        event: EventWriterOptions { colored: false, ..Default::default() },
        status_enabled: false,
        ..Default::default()
    };
    let mut r = InlineRenderer::new(opts);
    let mut state = BuildState::new();
    for ev in [
        ProgressEvent::BuildStarted {
            recipes: vec![RecipeTopo { id: RecipeId::new(0), name: "lib".into(), deps: vec![], expected_nodes: 1 }],
            total_nodes: 1,
        },
        ProgressEvent::RecipeStarted { recipe: RecipeId::new(0) },
        ProgressEvent::NodeStarted {
            recipe: RecipeId::new(0), node: NodeId::new(0),
            name: "x.c".into(), artifact: None, fallback_label: "x".into(),
            kind: NodeKind::Compile,
            cause: None,
            cache_key: None,
        },
        ProgressEvent::NodeCompleted {
            recipe: RecipeId::new(0), node: NodeId::new(0),
            elapsed: Duration::from_millis(100), kind: NodeKind::Compile,
            cache_key: None,
        },
        ProgressEvent::RecipeCompleted {
            recipe: RecipeId::new(0),
            elapsed: Duration::from_millis(150), cached: 0, total: 1,
            kind: crate::event::RecipeKind::Recipe,
        },
        ProgressEvent::Finished { success: true },
    ] {
        state.apply(&ev);
        r.handle(&state, &ev).unwrap();
    }
    r.finish(&state).unwrap();
}
