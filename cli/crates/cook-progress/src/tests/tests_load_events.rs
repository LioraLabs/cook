use super::*;
use crate::event::{NodeId, NodeKind, ProgressEvent, RecipeId, RecipeTopo, Stream};
use crate::log_store::{LogConfig, LogStore};
use crate::model::build::BuildState;
use std::time::Duration;

fn drive_minimal_build(tmp: &Path) -> String {
    let mut store = LogStore::open(tmp, LogConfig::default()).unwrap();
    let mut state = BuildState::new();
    let bs = ProgressEvent::BuildStarted {
        recipes: vec![RecipeTopo {
            id: RecipeId::new(0),
            name: "lib".into(),
            deps: vec![],
            expected_nodes: 1,
        }],
        total_nodes: 1,
    };
    state.apply(&bs);
    store.record(&state, &bs).unwrap();

    let ns = ProgressEvent::NodeStarted {
        recipe: RecipeId::new(0),
        node: NodeId::new(0),
        name: "parser.c".into(),
        artifact: None,
        fallback_label: "parser.c".into(),
        kind: NodeKind::Cooked,
            cause: None,
            cache_key: None,
        };
    state.apply(&ns);
    store.record(&state, &ns).unwrap();

    let no = ProgressEvent::NodeOutput {
        recipe: RecipeId::new(0),
        node: NodeId::new(0),
        line: "hello world".into(),
        stream: Stream::Stdout,
    };
    state.apply(&no);
    store.record(&state, &no).unwrap();

    let nf = ProgressEvent::NodeFailed {
        recipe: RecipeId::new(0),
        node: NodeId::new(0),
        elapsed: Duration::from_millis(123),
        error: "boom".into(),
    };
    state.apply(&nf);
    store.record(&state, &nf).unwrap();

    store.close(false).unwrap();
    store.build_id().to_string()
}

#[test]
fn load_replays_events_into_buildview() {
    let tmp = tempfile::tempdir().unwrap();
    let build_id = drive_minimal_build(tmp.path());
    let build_dir = tmp.path().join(".cook").join("logs").join(&build_id);

    let (view, diag) = load(&build_dir).unwrap();
    assert!(!diag.events_jsonl_missing);

    assert_eq!(view.recipes.len(), 1);
    let (_, recipe) = view.recipes.iter().next().unwrap();
    assert_eq!(recipe.name, "lib");
    assert_eq!(recipe.nodes.len(), 1);
    let (_, node) = recipe.nodes.iter().next().unwrap();
    assert_eq!(node.name, "parser.c");
    assert_eq!(node.status, NodeStatus::Failed);
    assert_eq!(node.elapsed_ms, Some(123));
    assert_eq!(node.lines.len(), 1);
    assert_eq!(node.lines[0].text, "hello world");
    assert_eq!(node.lines[0].stream, Stream::Stdout);
}

#[test]
fn load_skips_corrupt_jsonl_lines_and_counts_them() {
    let tmp = tempfile::tempdir().unwrap();
    let build_id = drive_minimal_build(tmp.path());
    let build_dir = tmp.path().join(".cook").join("logs").join(&build_id);
    let events_path = build_dir.join("events.jsonl");
    let mut text = std::fs::read_to_string(&events_path).unwrap();
    text.push_str("not json at all\n");
    text.push_str("{\"missing\":\"v\"}\n");
    std::fs::write(&events_path, text).unwrap();

    let (_view, diag) = load(&build_dir).unwrap();
    assert_eq!(diag.skipped_jsonl_lines, 2);
}

/// COOK-410: `events.jsonl` had two read policies. `load`'s replay gated on a
/// locally re-derived envelope, while `summarize_build_dir`'s failed-count scan
/// had no version gate at all, so a `node-failed` line written by a future
/// schema was counted in the build summary that the full replay then refused
/// to read. One format, one policy: both now call `check_schema_version`.
#[test]
fn a_future_schema_line_is_refused_by_the_summary_and_the_replay_alike() {
    let tmp = tempfile::tempdir().unwrap();
    let build_id = drive_minimal_build(tmp.path());
    let build_dir = tmp.path().join(".cook").join("logs").join(&build_id);
    let events = build_dir.join("events.jsonl");

    let baseline_summary = crate::log_reader::list_builds(
        &tmp.path().join(".cook").join("logs"),
    )
    .unwrap()
    .into_iter()
    .find(|b| b.build_id == build_id)
    .expect("build present")
    .failed_count;

    let (baseline_view, _) = crate::log_reader::load(&build_dir).unwrap();
    let baseline_nodes: usize =
        baseline_view.recipes.values().map(|r| r.nodes.len()).sum();

    // A node-failed line from a writer one major schema ahead of us.
    let future = crate::event::PROGRESS_SCHEMA_VERSION + 1;
    let mut text = std::fs::read_to_string(&events).unwrap();
    text.push_str(&format!(
        "{{\"v\":{future},\"ts\":\"2026-08-01T00:00:00Z\",\"type\":\"node-failed\",\
         \"recipe\":\"lib\",\"node\":\"a\",\"elapsed_ms\":5,\"error\":\"boom\"}}\n"
    ));
    std::fs::write(&events, text).unwrap();

    let after_summary = crate::log_reader::list_builds(
        &tmp.path().join(".cook").join("logs"),
    )
    .unwrap()
    .into_iter()
    .find(|b| b.build_id == build_id)
    .expect("build present")
    .failed_count;

    let (after_view, _) = crate::log_reader::load(&build_dir).unwrap();
    let after_nodes: usize = after_view.recipes.values().map(|r| r.nodes.len()).sum();

    assert_eq!(
        after_summary, baseline_summary,
        "the failed-count scan must refuse a v{future} line; the replay does, \
         and a summary that disagrees with the replay is the bug"
    );
    assert_eq!(
        after_nodes, baseline_nodes,
        "the replay must refuse it too (regression guard on the shared gate)"
    );
}
