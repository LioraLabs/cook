use super::watch_dir_for_pattern;
use std::fs;

#[test]
fn watches_existing_glob_prefix_or_nearest_existing_ancestor() {
    let temp = tempfile::tempdir().expect("tempdir");
    let existing = temp.path().join("existing");
    fs::create_dir(&existing).expect("existing prefix");

    assert_eq!(watch_dir_for_pattern(&existing.join("**/*.json")), existing,);
    assert_eq!(
        watch_dir_for_pattern(&temp.path().join("missing/nested/**/*.json")),
        temp.path(),
    );
}
