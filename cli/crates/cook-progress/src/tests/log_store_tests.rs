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
            cache_key: None,
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
            cache_key: None,
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

// ---------------------------------------------------------------------------
// The composer and its inverse (COOK-421)
// ---------------------------------------------------------------------------

/// This crate WRITES `started_at` / `ended_at` into `.cook/logs` with the
/// `time` crate's `Rfc3339`; `cook-logs` READS them back with
/// `cook_contracts::timestamp::parse_rfc3339_ms` to show how long a build
/// took. The two ends are in different crates, and only this one can see both
/// the formatter and the parser, so the agreement test belongs here.
///
/// It is not a copy of the parser's own round-trip test. That one proves the
/// parser is the inverse of the CONTRACTS formatter; this proves it is the
/// inverse of the formatter this crate actually calls — the `time` crate's,
/// whose exact output (how many fractional digits, whether the zone is `Z` or
/// `+00:00`) is not ours to choose.
#[test]
fn every_timestamp_this_crate_writes_is_one_cook_logs_can_read_back() {
    use time::format_description::well_known::Rfc3339;
    use time::OffsetDateTime;

    // Fixed instants, so this cannot flake on a clock: the epoch, a leap day,
    // a whole second, a sub-second, and an end-of-year rollover.
    for nanos in [
        0i128,
        951_782_400_000_000_000,             // 2000-02-29T00:00:00Z
        951_782_400_500_000_000,             // …with half a second
        1_772_323_199_123_456_789,           // 2026-02-28T23:59:59.123456789Z
        1_772_323_200_000_000_000,           // 2026-03-01T00:00:00Z
    ] {
        let instant = OffsetDateTime::from_unix_timestamp_nanos(nanos).expect("in range");
        let written = instant.format(&Rfc3339).expect("format");
        let read_back = cook_contracts::timestamp::parse_rfc3339_ms(&written)
            .unwrap_or_else(|| panic!("cook-logs cannot read what this crate wrote: {written}"));
        assert_eq!(
            read_back,
            (nanos / 1_000_000) as i64,
            "{written} parsed to the wrong instant"
        );
    }
}

/// The live path, one instant, no assertion about WHICH instant: whatever the
/// clock says, the string this crate stores must survive the trip.
#[test]
fn the_timestamp_actually_stored_survives_the_trip() {
    let stamped = super::current_rfc3339();
    assert!(
        cook_contracts::timestamp::parse_rfc3339_ms(&stamped).is_some(),
        "cook-logs cannot read the stamp this crate writes: {stamped}"
    );
}
