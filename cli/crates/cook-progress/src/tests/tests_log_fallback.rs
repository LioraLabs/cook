use super::*;
use std::fs;

#[test]
fn load_falls_back_to_log_files_when_events_jsonl_absent() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("2026-05-10-zzz");
    let nodes = dir.join("nodes").join("lib");
    fs::create_dir_all(&nodes).unwrap();
    fs::write(
        dir.join("manifest.toml"),
        "schema_version = 1\n\
         build_id = \"2026-05-10-zzz\"\n\
         started_at = \"2026-05-10T10:00:00Z\"\n\
         ended_at = \"2026-05-10T10:00:01Z\"\n\
         exit_code = 0\n",
    )
    .unwrap();
    fs::write(
        nodes.join("parser.c.log"),
        "[out] hello\n[err] oops\n",
    )
    .unwrap();

    let (view, diag) = load(&dir).unwrap();
    assert!(diag.events_jsonl_missing);
    assert_eq!(view.recipes.len(), 1);
    let (_, recipe) = view.recipes.iter().next().unwrap();
    assert_eq!(recipe.name, "lib");
    let (_, node) = recipe.nodes.iter().next().unwrap();
    assert_eq!(node.name, "parser.c");
    assert_eq!(node.status, NodeStatus::Unknown);
    assert_eq!(node.lines.len(), 2);
    assert_eq!(node.lines[0].stream, Stream::Stdout);
    assert_eq!(node.lines[0].text, "hello");
    assert_eq!(node.lines[1].stream, Stream::Stderr);
    assert_eq!(node.lines[1].text, "oops");
}
