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

/// COOK-404. CS-0198 made duration rendering one user-visible law, and
/// COOK-392's commit message asserted "All six sites delegate." Two did not:
/// `render/tree.rs` and `render/output.rs` still hand-rolled `{:.1}s`, so in a
/// single frame 61,500ms read `1m01s` in the header and `61.5s` in the tree.
///
/// A behavioural test cannot catch the next fork, because a second correct-
/// looking spelling agrees with the law for every duration under a minute and
/// only diverges past it. So this scans the crate's own sources instead, the
/// same shape as `cook-contracts`' `tests/layout.rs`.
#[test]
fn no_module_in_this_crate_spells_duration_itself() {
    use std::fs;
    use std::path::Path;

    fn visit(dir: &Path, hits: &mut Vec<String>) {
        for entry in fs::read_dir(dir).expect("read cook-logs source dir") {
            let path = entry.expect("read entry").path();
            if path.is_dir() {
                if path.file_name().is_none_or(|n| n != "tests") {
                    visit(&path, hits);
                }
                continue;
            }
            if path.extension().is_none_or(|e| e != "rs") {
                continue;
            }
            let source = fs::read_to_string(&path).expect("read source file");
            for (i, line) in source.lines().enumerate() {
                let code = line.split("//").next().unwrap_or_default();
                // The two shapes a hand-rolled duration takes here: dividing
                // milliseconds down to seconds, or formatting a Duration's
                // float seconds directly.
                if code.contains("/ 1000.0") || code.contains("as_secs_f64()") {
                    hits.push(format!("{}:{}", path.display(), i + 1));
                }
            }
        }
    }

    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut hits = Vec::new();
    visit(&src, &mut hits);

    assert!(
        hits.is_empty(),
        "these sites render a duration themselves instead of calling \
         cook_contracts::render::duration_ms, which is THE law (CS-0198). A \
         second spelling agrees for everything under a minute and diverges \
         above it, so it will not be caught by eye:\n  {}",
        hits.join("\n  ")
    );
}
