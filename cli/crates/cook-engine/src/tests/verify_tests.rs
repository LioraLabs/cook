use super::*;

#[test]
fn verify_cache_entry_exists() {
    let _f: fn(
        &std::path::Path,
        &crate::RegisteredWorkspace,
        &std::collections::BTreeMap<String, Vec<String>>,
        &std::collections::BTreeSet<String>,
        usize,
    ) -> Result<VerifyReport, String> = verify_cache;
    let _ = _f;
}

#[test]
fn rerun_in_sandbox_hashes_declared_outputs() {
    let dir = tempfile::tempdir().unwrap();
    let env = std::collections::BTreeMap::new();
    let res = rerun_outputs_in_sandbox(
        "printf 'hello' > out.txt",
        dir.path(),
        &env,
        &["out.txt".to_string()],
    )
    .expect("rerun should succeed");
    assert_eq!(res.len(), 1);
    let h = res.get("out.txt").copied().expect("out.txt hashed");
    let res2 = rerun_outputs_in_sandbox(
        "printf 'hello' > out.txt",
        dir.path(),
        &env,
        &["out.txt".to_string()],
    )
    .unwrap();
    assert_eq!(res2.get("out.txt").copied(), Some(h));
}

#[test]
fn rerun_nondeterministic_producer_changes_hash() {
    let dir = tempfile::tempdir().unwrap();
    let env = std::collections::BTreeMap::new();
    let cmd = "date +%s%N > out.txt";
    let a = rerun_outputs_in_sandbox(cmd, dir.path(), &env, &["out.txt".to_string()]).unwrap();
    std::thread::sleep(std::time::Duration::from_millis(5));
    let b = rerun_outputs_in_sandbox(cmd, dir.path(), &env, &["out.txt".to_string()]).unwrap();
    assert_ne!(a.get("out.txt"), b.get("out.txt"), "nondeterministic producer must differ");
    }

    #[test]
    fn rerun_failed_command_is_err() {
        let dir = tempfile::tempdir().unwrap();
        let env = std::collections::BTreeMap::new();
        let r = rerun_outputs_in_sandbox("exit 7", dir.path(), &env, &["out.txt".to_string()]);
    assert!(r.is_err());
}

#[test]
fn verdict_pass_is_ok_and_record_exempt_is_ok() {
    assert!(UnitVerdict::Pass.is_ok());
    assert!(UnitVerdict::RecordExempt.is_ok());
    assert!(!UnitVerdict::Divergence { detail: "x".into() }.is_ok());
    assert!(!UnitVerdict::Error { detail: "y".into() }.is_ok());
}

#[test]
fn report_exit_code_zero_iff_all_ok() {
    let mut r = VerifyReport::default();
    r.units.push(UnitReport { recipe: "build".into(), unit: "a.o".into(), key: "k".into(), verdict: UnitVerdict::Pass });
    assert_eq!(r.exit_code(), 0);
    r.units.push(UnitReport { recipe: "build".into(), unit: "b.o".into(), key: "k2".into(), verdict: UnitVerdict::Divergence { detail: "bytes differ".into() } });
        assert_ne!(r.exit_code(), 0);
    }

    #[test]
    fn matching_bytes_pass() {
        let recorded: BTreeMap<String, u64> = [("a.o".to_string(), 42u64)].into();
    let rerun: BTreeMap<String, u64> = [("a.o".to_string(), 42u64)].into();
    assert_eq!(classify(false, &recorded, &rerun), UnitVerdict::Pass);
}

#[test]
fn differing_bytes_diverge_for_non_record() {
    let recorded: BTreeMap<String, u64> = [("a.o".to_string(), 42u64)].into();
    let rerun: BTreeMap<String, u64> = [("a.o".to_string(), 99u64)].into();
    match classify(false, &recorded, &rerun) {
        UnitVerdict::Divergence { detail } => assert!(detail.contains("a.o")),
        v => panic!("expected divergence, got {v:?}"),
    }
}

#[test]
fn record_unit_byte_difference_is_exempt() {
    let recorded: BTreeMap<String, u64> = [("img.png".to_string(), 42u64)].into();
    let rerun: BTreeMap<String, u64> = [("img.png".to_string(), 99u64)].into();
    assert_eq!(classify(true, &recorded, &rerun), UnitVerdict::RecordExempt);
}

#[test]
fn record_unit_missing_output_is_error() {
    let recorded: BTreeMap<String, u64> = [("img.png".to_string(), 42u64)].into();
    let rerun: BTreeMap<String, u64> = BTreeMap::new();
    match classify(true, &recorded, &rerun) {
        UnitVerdict::Error { .. } => {}
        v => panic!("expected error, got {v:?}"),
    }
}

#[test]
fn non_record_missing_output_is_divergence() {
    let recorded: BTreeMap<String, u64> = [("a.o".to_string(), 42u64)].into();
    let rerun: BTreeMap<String, u64> = BTreeMap::new();
    match classify(false, &recorded, &rerun) {
        UnitVerdict::Divergence { .. } => {}
        v => panic!("expected divergence, got {v:?}"),
    }
}
