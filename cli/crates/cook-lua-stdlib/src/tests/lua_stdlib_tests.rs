use super::*;

#[test]
fn static_returns_captured_path() {
    let src = WorkingDirSource::Static(PathBuf::from("/tmp/static"));
    assert_eq!(src.resolve(), PathBuf::from("/tmp/static"));
    // Repeated calls return the same path.
    assert_eq!(src.resolve(), PathBuf::from("/tmp/static"));
}

#[test]
fn live_reflects_post_registration_mutations() {
    let slot = Arc::new(Mutex::new(PathBuf::from("/tmp/initial")));
    let src = WorkingDirSource::Live(Arc::clone(&slot));
    assert_eq!(src.resolve(), PathBuf::from("/tmp/initial"));

    // Simulate a new work item updating the slot.
    *slot.lock().unwrap() = PathBuf::from("/tmp/updated");
    assert_eq!(
        src.resolve(),
        PathBuf::from("/tmp/updated"),
        "Live source must observe slot mutations made after registration"
    );
}

#[test]
fn live_clone_shares_slot() {
    let slot = Arc::new(Mutex::new(PathBuf::from("/tmp/a")));
    let src1 = WorkingDirSource::Live(Arc::clone(&slot));
    let src2 = src1.clone();

    *slot.lock().unwrap() = PathBuf::from("/tmp/b");
    assert_eq!(src1.resolve(), PathBuf::from("/tmp/b"));
    assert_eq!(src2.resolve(), PathBuf::from("/tmp/b"));
}
