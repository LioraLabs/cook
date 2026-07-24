use super::*;

fn write(p: &Path, contents: &str) {
    std::fs::write(p, contents).unwrap();
}

#[test]
fn sweeps_unmodified_orphan() {
    let dir = tempfile::tempdir().unwrap();
    let orphan = dir.path().join("jack.txt");
    write(&orphan, "Jack of Diamonds\n");
    let hash = hash_file(&orphan).unwrap();

    let mut prior = BTreeMap::new();
    prior.insert(orphan.clone(), hash);
    let live = BTreeSet::new(); // no longer declared

    let report = sweep(&prior, &live);
    assert_eq!(report.swept(), &[orphan.clone()]);
    assert!(report.kept_modified().is_empty());
    assert!(!orphan.exists(), "unmodified orphan must be removed");
}

#[test]
fn keeps_modified_orphan() {
    let dir = tempfile::tempdir().unwrap();
    let orphan = dir.path().join("jack.txt");
    write(&orphan, "Jack of Diamonds\n");
    let recorded = hash_file(&orphan).unwrap();
    // User edits the file after Cook wrote it.
    write(&orphan, "HAND EDITED\n");

    let mut prior = BTreeMap::new();
    prior.insert(orphan.clone(), recorded);
    let report = sweep(&prior, &BTreeSet::new());

    assert!(report.swept().is_empty());
    assert_eq!(report.kept_modified(), &[orphan.clone()]);
    assert!(orphan.exists(), "modified orphan must be kept");
}

#[test]
fn live_output_is_not_swept() {
    let dir = tempfile::tempdir().unwrap();
    let still_here = dir.path().join("ace.txt");
    write(&still_here, "Ace of Spades\n");
    let hash = hash_file(&still_here).unwrap();

    let mut prior = BTreeMap::new();
    prior.insert(still_here.clone(), hash);
    let mut live = BTreeSet::new();
    live.insert(still_here.clone()); // still declared this run

    let report = sweep(&prior, &live);
    assert!(report.is_empty());
    assert!(still_here.exists());
}

#[test]
fn absent_orphan_is_noop() {
    let dir = tempfile::tempdir().unwrap();
    let gone = dir.path().join("gone.txt");
    let mut prior = BTreeMap::new();
    prior.insert(gone.clone(), 12345);
    let report = sweep(&prior, &BTreeSet::new());
    assert!(report.is_empty());
}

#[test]
fn directory_orphan_is_left_in_place() {
    // Files only (§17.7): a directory matching a prior path is never swept.
    let dir = tempfile::tempdir().unwrap();
    let subdir = dir.path().join("build");
    std::fs::create_dir(&subdir).unwrap();
    let mut prior = BTreeMap::new();
    prior.insert(subdir.clone(), 0);
    let report = sweep(&prior, &BTreeSet::new());
    assert!(report.is_empty());
    assert!(subdir.exists());
}
