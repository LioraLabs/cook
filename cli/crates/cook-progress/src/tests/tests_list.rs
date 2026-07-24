use super::*;
use std::fs;

#[test]
fn list_builds_returns_newest_first_with_parsed_metadata() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();

    for (id, exit) in [("2026-05-10-aaa", 0), ("2026-05-10-bbb", 1)] {
        let dir = root.join(id);
        fs::create_dir_all(&dir).unwrap();
        let manifest = format!(
            "schema_version = 1\n\
             build_id = \"{id}\"\n\
             started_at = \"2026-05-10T10:00:00Z\"\n\
             ended_at = \"2026-05-10T10:00:05Z\"\n\
             exit_code = {exit}\n"
        );
        fs::write(dir.join("manifest.toml"), manifest).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(10));
    }

    let builds = list_builds(root).unwrap();
    assert_eq!(builds.len(), 2);
    assert_eq!(builds[0].build_id, "2026-05-10-bbb"); // newest first
    assert_eq!(builds[0].exit_code, Some(1));
    assert_eq!(builds[1].exit_code, Some(0));
}

#[test]
fn list_builds_counts_recipes_and_failed_nodes() {
    use std::fs;
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let dir = root.join("2026-05-10-aaa");
    fs::create_dir_all(dir.join("nodes").join("lib")).unwrap();
    fs::create_dir_all(dir.join("nodes").join("vm")).unwrap();
    fs::write(
        dir.join("manifest.toml"),
        "schema_version = 1\nbuild_id = \"2026-05-10-aaa\"\nstarted_at = \"2026-05-10T10:00:00Z\"\nended_at = \"2026-05-10T10:00:01Z\"\nexit_code = 1\n",
    )
    .unwrap();
    fs::write(
        dir.join("events.jsonl"),
        "{\"v\":1,\"ts\":\"2026-05-10T10:00:00Z\",\"type\":\"node-failed\",\"recipe\":\"vm\",\"node\":\"lvm.c\",\"elapsed_ms\":100,\"error\":\"x\"}\n\
         {\"v\":1,\"ts\":\"2026-05-10T10:00:01Z\",\"type\":\"node-completed\",\"recipe\":\"lib\",\"node\":\"parser.c\",\"elapsed_ms\":50,\"kind\":\"cooked\"}\n",
    )
    .unwrap();

    let builds = list_builds(root).unwrap();
    assert_eq!(builds.len(), 1);
    assert_eq!(builds[0].recipe_count, 2);
    assert_eq!(builds[0].failed_count, 1);
}

#[test]
fn list_builds_skips_non_dir_entries_and_missing_manifests() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    fs::write(root.join("stray.txt"), "noise").unwrap();
    let dir = root.join("2026-05-10-aaa");
    fs::create_dir_all(&dir).unwrap();
    // no manifest

    let builds = list_builds(root).unwrap();
    assert_eq!(builds.len(), 1);
    assert_eq!(builds[0].build_id, "2026-05-10-aaa");
    assert_eq!(builds[0].exit_code, None);
}
