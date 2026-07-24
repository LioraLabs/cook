use super::*;
use crate::event::RecipeTopo;

#[test]
fn open_creates_build_directory() {
    let tmp = tempfile::tempdir().unwrap();
    let store = LogStore::open(tmp.path(), LogConfig::default()).unwrap();
    let build_dir = tmp.path().join(".cook").join("logs").join(store.build_id());
    assert!(build_dir.exists());
    assert!(build_dir.join("nodes").exists());
}

#[test]
fn node_output_is_written_with_stream_tag() {
    let tmp = tempfile::tempdir().unwrap();
    let mut store = LogStore::open(tmp.path(), LogConfig::default()).unwrap();
    let mut state = BuildState::new();
    state.apply(&ProgressEvent::BuildStarted {
        recipes: vec![RecipeTopo {
            id: RecipeId::new(0), name: "lib".into(),
            deps: vec![], expected_nodes: 1,
        }],
        total_nodes: 1,
    });
    state.apply(&ProgressEvent::NodeStarted {
        recipe: RecipeId::new(0), node: NodeId::new(0),
        name: "lvm.c".into(), artifact: None, fallback_label: "x".into(),
        kind: crate::event::NodeKind::Cooked,
            cause: None,
        });
    store.record(&state, &ProgressEvent::NodeOutput {
        recipe: RecipeId::new(0), node: NodeId::new(0),
        line: "warning".into(), stream: Stream::Stderr,
    }).unwrap();
    store.close(true).unwrap();

    let log = fs::read_to_string(tmp.path()
        .join(".cook").join("logs").join(store.build_id())
        .join("nodes").join("lib").join("lvm.c.log")).unwrap();
    assert!(log.contains("[err] warning"), "got: {log}");
}

#[test]
fn rotate_removes_oldest_when_over_keep_builds() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join(".cook").join("logs");
    fs::create_dir_all(&root).unwrap();
    for i in 0..5 {
        let d = root.join(format!("build-{i}"));
        fs::create_dir_all(&d).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    rotate(&root, 2, u64::MAX).unwrap();
    let remaining = fs::read_dir(&root).unwrap().count();
    assert_eq!(remaining, 2);
}

#[test]
fn events_jsonl_is_written_in_spec_shape() {
    let tmp = tempfile::tempdir().unwrap();
    let mut store = LogStore::open(tmp.path(), LogConfig::default()).unwrap();
    let mut state = BuildState::new();
    let ev = ProgressEvent::BuildStarted {
        recipes: vec![RecipeTopo {
            id: RecipeId::new(0), name: "deps".into(),
            deps: vec![], expected_nodes: 2,
        }],
        total_nodes: 2,
    };
    state.apply(&ev);
    store.record(&state, &ev).unwrap();
    store.close(true).unwrap();

    let events_path = tmp.path()
        .join(".cook").join("logs").join(store.build_id())
        .join("events.jsonl");
    let data = fs::read_to_string(events_path).unwrap();
    assert!(data.contains("\"type\":\"build-started\""), "got: {data}");
    assert!(data.contains("\"v\":1"), "got: {data}");
    assert!(data.contains("\"ts\":"), "got: {data}");
}

#[test]
fn manifest_toml_written_on_close() {
    let tmp = tempfile::tempdir().unwrap();
    let mut store = LogStore::open(tmp.path(), LogConfig::default()).unwrap();
    store.close(true).unwrap();

    let manifest_path = tmp.path()
        .join(".cook").join("logs").join(store.build_id())
        .join("manifest.toml");
    let data = fs::read_to_string(manifest_path).unwrap();
    assert!(data.contains("schema_version = 1"));
    assert!(data.contains("exit_code = 0"));
    assert!(data.contains(&format!("build_id = \"{}\"", store.build_id())));
}

#[test]
fn recipe_and_node_names_are_sanitized_into_paths() {
    let tmp = tempfile::tempdir().unwrap();
    let mut store = LogStore::open(tmp.path(), LogConfig::default()).unwrap();
    let mut state = BuildState::new();
    state.apply(&ProgressEvent::BuildStarted {
        recipes: vec![RecipeTopo {
            id: RecipeId::new(0),
            name: "../../etc/passwd".into(),
            deps: vec![],
            expected_nodes: 1,
        }],
        total_nodes: 1,
    });
    state.apply(&ProgressEvent::NodeStarted {
        recipe: RecipeId::new(0), node: NodeId::new(0),
        name: "../../root".into(), artifact: None, fallback_label: "x".into(),
        kind: crate::event::NodeKind::Cooked,
            cause: None,
        });
    store.record(&state, &ProgressEvent::NodeOutput {
        recipe: RecipeId::new(0), node: NodeId::new(0),
        line: "hi".into(), stream: Stream::Stdout,
    }).unwrap();
    store.close(true).unwrap();

    // Nothing was written outside the build directory.
    let build_dir = tmp.path().join(".cook").join("logs").join(store.build_id());
    let nodes_dir = build_dir.join("nodes");
    let sanitized_rname = nodes_dir.join(".._.._etc_passwd");
    assert!(sanitized_rname.exists(), "sanitized recipe dir should exist: {sanitized_rname:?}");
    let sanitized_file = sanitized_rname.join(".._.._root.log");
    assert!(sanitized_file.exists(), "sanitized node file should exist: {sanitized_file:?}");

    // No traversal happened: there is no 'etc' directory outside the build.
    assert!(!tmp.path().join("etc").exists());
}
