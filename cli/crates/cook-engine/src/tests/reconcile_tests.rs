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

    let report = sweep(&prior, &live, dir.path());
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
    let report = sweep(&prior, &BTreeSet::new(), dir.path());

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

    let report = sweep(&prior, &live, dir.path());
    assert!(report.is_empty());
    assert!(still_here.exists());
}

#[test]
fn absent_orphan_is_noop() {
    let dir = tempfile::tempdir().unwrap();
    let gone = dir.path().join("gone.txt");
    let mut prior = BTreeMap::new();
    prior.insert(gone.clone(), 12345);
    let report = sweep(&prior, &BTreeSet::new(), dir.path());
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
    let report = sweep(&prior, &BTreeSet::new(), dir.path());
    assert!(report.is_empty());
    assert!(subdir.exists());
}

/// A record made while output walks still entered links names a file through
/// `dist/assets -> ../public`. Sweeping it would delete the user's source.
#[test]
#[cfg(unix)]
fn never_sweeps_through_a_link() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("public")).unwrap();
    std::fs::create_dir_all(dir.path().join("dist")).unwrap();
    std::os::unix::fs::symlink("../public", dir.path().join("dist/assets")).unwrap();
    let logo = dir.path().join("public/logo.png");
    write(&logo, "png\n");
    let mut prior = BTreeMap::new();
    prior.insert(dir.path().join("dist/assets/logo.png"), hash_file(&logo).unwrap());
    let report = sweep(&prior, &BTreeSet::new(), dir.path());
    assert!(report.swept().is_empty());
    assert!(logo.exists(), "a file reached through a link was swept");
}
