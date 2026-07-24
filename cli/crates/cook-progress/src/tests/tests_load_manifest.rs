use super::*;
use std::fs;

#[test]
fn load_reads_manifest_metadata_into_buildview() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("2026-05-10-aaa");
    fs::create_dir_all(dir.join("nodes")).unwrap();
    fs::write(
        dir.join("manifest.toml"),
        "schema_version = 1\n\
         build_id = \"2026-05-10-aaa\"\n\
         started_at = \"2026-05-10T10:00:00Z\"\n\
         ended_at = \"2026-05-10T10:00:05Z\"\n\
         exit_code = 0\n",
    )
    .unwrap();

    let (view, diag) = load(&dir).unwrap();
    assert_eq!(view.build_id, "2026-05-10-aaa");
    assert_eq!(view.started_at, "2026-05-10T10:00:00Z");
    assert_eq!(view.ended_at.as_deref(), Some("2026-05-10T10:00:05Z"));
    assert_eq!(view.exit_code, Some(0));
    assert!(!diag.manifest_missing);
}

#[test]
fn load_tolerates_missing_manifest() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("2026-05-10-bbb");
    fs::create_dir_all(dir.join("nodes")).unwrap();

    let (view, diag) = load(&dir).unwrap();
    assert!(diag.manifest_missing);
    assert_eq!(view.build_id, "2026-05-10-bbb");
    assert!(view.exit_code.is_none());
}
