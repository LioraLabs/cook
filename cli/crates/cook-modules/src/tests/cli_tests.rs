use super::*;

#[test]
fn parse_name_at_version_separates_name_and_constraint() {
    assert_eq!(
        parse_name_at_version("cook_smoke@0.1.0-1"),
        ("cook_smoke".into(), "0.1.0-1".into())
    );
    assert_eq!(
        parse_name_at_version("cook_smoke"),
        ("cook_smoke".into(), "*".into())
    );
}

#[test]
fn validate_lockfile_consistent_passes_on_match() {
    let mut manifest = ManifestModules::default();
    manifest.modules.insert("cook_smoke".into(), "*".into());
    let lock = Lockfile::new(vec![lockfile::LockedModule {
        name: "cook_smoke".into(),
        version: "0.1.0-1".into(),
        source: "https://rocks.usecook.com/cook_smoke-0.1.0-1.src.rock".into(),
        integrity: "sha256-x".into(),
        direct: true,
    }]);
    validate_lockfile_consistent(&lock, &manifest).expect("ok");
}

#[test]
fn validate_lockfile_consistent_errors_on_drift() {
    let mut manifest = ManifestModules::default();
    manifest.modules.insert("cook_smoke".into(), "*".into());
    let lock = Lockfile::new(Vec::new());
    let err = validate_lockfile_consistent(&lock, &manifest).expect_err("must fail");
    assert!(format!("{:#}", err).contains("disagree"));
}
