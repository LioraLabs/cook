use super::*;
use cook_contracts::probe_key::LocalProbeKey;
use cook_probe::store::ProbeValueStore;

#[test]
fn verify_cache_entry_exists() {
    let _f: fn(
        &std::path::Path,
        &crate::RegisteredWorkspace,
        &std::collections::BTreeMap<String, Vec<String>>,
        &std::collections::BTreeSet<String>,
        usize,
        std::sync::Arc<cook_cache::cache_ctx::FailedPuts>,
    ) -> Result<VerifyReport, String> = verify_cache;
    let _ = _f;
}

#[test]
fn rerun_in_sandbox_hashes_declared_outputs() {
    let dir = tempfile::tempdir().unwrap();
    let env = std::collections::BTreeMap::new();
    let res = rerun_outputs_in_sandbox(
        "printf 'hello' > out.txt",
        &ProbeValueStore::new(), "r",
        dir.path(),
        &env,
        &["out.txt".to_string()],
    )
    .expect("rerun should succeed");
    assert_eq!(res.len(), 1);
    let h = res.get("out.txt").copied().expect("out.txt hashed");
    let res2 = rerun_outputs_in_sandbox(
        "printf 'hello' > out.txt",
        &ProbeValueStore::new(), "r",
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
    let a = rerun_outputs_in_sandbox(cmd, &ProbeValueStore::new(), "r", dir.path(), &env, &["out.txt".to_string()]).unwrap();
    std::thread::sleep(std::time::Duration::from_millis(5));
    let b = rerun_outputs_in_sandbox(cmd, &ProbeValueStore::new(), "r", dir.path(), &env, &["out.txt".to_string()]).unwrap();
    assert_ne!(a.get("out.txt"), b.get("out.txt"), "nondeterministic producer must differ");
    }

    #[test]
    fn rerun_failed_command_is_err() {
        let dir = tempfile::tempdir().unwrap();
        let env = std::collections::BTreeMap::new();
        let r = rerun_outputs_in_sandbox("exit 7", &ProbeValueStore::new(), "r", dir.path(), &env, &["out.txt".to_string()]);
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

#[test]
fn rerun_substitutes_probe_sigils_before_the_shell() {
    let dir = tempfile::tempdir().unwrap();
    let store = ProbeValueStore::new();
    store.for_recipe("r").insert(&LocalProbeKey::new("greet:who"), b"\"world\"".to_vec());
    let res = rerun_outputs_in_sandbox(
        "echo $<greet:who> > out.txt",
        &store,
        "r",
        dir.path(),
        &std::collections::BTreeMap::new(),
        &["out.txt".to_string()],
    )
    .expect("sigil must be resolved, not left for the shell");
    let expected = dir.path().join("expected.txt");
    std::fs::write(&expected, "world\n").unwrap();
    assert_eq!(res.get("out.txt").copied(), cook_cache::hash_file(&expected));
}

#[cfg(unix)]
#[test]
fn rerun_keeps_relative_symlinks_and_sandboxes_outside_the_project() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join("real")).unwrap();
    std::fs::write(dir.path().join("real/data.txt"), "through the link").unwrap();
    std::os::unix::fs::symlink("real", dir.path().join("link")).unwrap();
    let cmd = format!(
        "case \"$PWD\" in {}/*) exit 9;; */cook/verify/*) cat link/data.txt > out.txt;; *) exit 8;; esac",
        dir.path().display()
    );
    let res = rerun_outputs_in_sandbox(
        &cmd,
        &ProbeValueStore::new(), "r",
        dir.path(),
        &std::collections::BTreeMap::new(),
        &["out.txt".to_string()],
    )
    .expect("rerun should succeed");
    let expected = dir.path().join("expected.txt");
    std::fs::write(&expected, "through the link").unwrap();
    assert_eq!(res.get("out.txt").copied(), cook_cache::hash_file(&expected));
}

/// A module may declare outputs as absolute paths under the unit's working
/// directory (cook_pnpm does: `<root>/././dist/**`). The re-run must read them
/// from the sandbox it built in, under the same names the real run recorded —
/// never from the real tree, which would compare the cache against itself.
#[test]
fn absolute_outputs_under_the_working_dir_are_read_from_the_sandbox() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_str().unwrap();
    std::fs::create_dir(dir.path().join("dist")).unwrap();
    std::fs::write(dir.path().join("dist/a.js"), "stale real tree").unwrap();
    std::fs::write(dir.path().join("lit.txt"), "stale real tree").unwrap();
    let declared = vec![format!("{root}/././dist/**"), format!("{root}/lit.txt")];
    let recorded_names = crate::executor::resolve_output_paths(&declared, dir.path());
    let res = rerun_outputs_in_sandbox(
        "printf fresh > dist/a.js && printf fresh > lit.txt",
        &ProbeValueStore::new(), "r",
        dir.path(),
        &std::collections::BTreeMap::new(),
        &declared,
    )
    .unwrap();
    let names: Vec<_> = res.keys().cloned().collect();
    let mut want = recorded_names.clone();
    want.sort();
    assert_eq!(names, want, "re-run must name outputs as the real run recorded them");
    let expected = dir.path().join("expected.txt");
    std::fs::write(&expected, "fresh").unwrap();
    let want_hash = cook_cache::hash_file(&expected).unwrap();
    for h in res.values() {
        assert_eq!(*h, want_hash, "hashed the real tree, not the sandbox");
    }
}

#[test]
fn trailing_slash_working_dir_still_rebases() {
    let dir = tempfile::tempdir().unwrap();
    let root = format!("{}/", dir.path().to_str().unwrap());
    std::fs::write(dir.path().join("lit.txt"), "stale").unwrap();
    let declared = vec![format!("{}lit.txt", root)];
    let res = rerun_outputs_in_sandbox(
        "printf fresh > lit.txt",
        &ProbeValueStore::new(),
        "r",
        Path::new(&root),
        &std::collections::BTreeMap::new(),
        &declared,
    )
    .unwrap();
    let expected = dir.path().join("expected.txt");
    std::fs::write(&expected, "fresh").unwrap();
    assert_eq!(res.values().copied().next(), cook_cache::hash_file(&expected));
}

#[test]
fn absolute_output_outside_the_working_dir_is_an_error() {
    let dir = tempfile::tempdir().unwrap();
    let other = tempfile::tempdir().unwrap();
    std::fs::write(other.path().join("x.txt"), "real").unwrap();
    let declared = vec![other.path().join("x.txt").to_str().unwrap().to_string()];
    let r = rerun_outputs_in_sandbox(
        "true",
        &ProbeValueStore::new(),
        "r",
        dir.path(),
        &std::collections::BTreeMap::new(),
        &declared,
    );
    assert!(r.unwrap_err().contains("outside the unit's working directory"));
}

#[cfg(unix)]
#[test]
fn absolute_symlink_into_the_working_dir_does_not_reach_the_real_tree() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join("real")).unwrap();
    std::fs::write(dir.path().join("real/data.txt"), "d").unwrap();
    std::os::unix::fs::symlink(dir.path().join("real"), dir.path().join("link")).unwrap();
    rerun_outputs_in_sandbox(
        "printf x > link/new.txt",
        &ProbeValueStore::new(),
        "r",
        dir.path(),
        &std::collections::BTreeMap::new(),
        &[],
    )
    .expect("rerun should succeed");
    assert!(!dir.path().join("real/new.txt").exists(), "re-run wrote through the link into the real tree");
}

#[test]
fn traced_rerun_names_the_real_working_dir() {
    if cook_cache::resolve_tool_path("strace").is_none() {
        eprintln!("skip: strace not on PATH");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("cfg.txt"), "x").unwrap();
    let r = rerun_in_sandbox(
        "cat cfg.txt > out.txt",
        &ProbeValueStore::new(), "r",
        dir.path(),
        &std::collections::BTreeMap::new(),
        &["out.txt".to_string()],
        &std::collections::BTreeSet::new(),
        true,
    )
    .expect("traced rerun");
    let trace = r.trace.expect("trace returned");
    let real = dir.path().to_string_lossy().into_owned();
    assert!(trace.contains(&format!("{real}/cfg.txt")), "trace: {trace}");
    assert!(!trace.contains("/verify/"), "sandbox path leaked: {trace}");
    assert!(r.outputs.contains_key("out.txt"));
}

#[test]
fn declared_output_escaping_via_dotdot_is_err() {
    let dir = tempfile::tempdir().unwrap();
    let out = format!("{}/../escape.txt", dir.path().display());
    let r = rerun_outputs_in_sandbox(
        "true",
        &ProbeValueStore::new(), "r",
        dir.path(),
        &std::collections::BTreeMap::new(),
        &[out],
    );
    assert!(r.unwrap_err().contains("outside"));
}

#[test]
fn isolated_root_needs_an_overlay_mount() {
    let dir = tempfile::tempdir().unwrap();
    std::env::set_var(crate::sandbox::ROOT_ENV, dir.path());
    let plain = crate::sandbox::isolated_root();
    std::env::remove_var(crate::sandbox::ROOT_ENV);
    assert_eq!(plain, None);
    assert_eq!(crate::sandbox::isolated_root(), None);
}

#[test]
fn escaping_output_is_refused_before_the_command_runs() {
    let dir = tempfile::tempdir().unwrap();
    let side = tempfile::tempdir().unwrap();
    let marker = side.path().join("ran");
    let r = rerun_outputs_in_sandbox(
        &format!("touch {}", marker.display()),
        &ProbeValueStore::new(),
        "r",
        dir.path(),
        &std::collections::BTreeMap::new(),
        &[format!("{}/../escape.txt", dir.path().display())],
    );
    assert!(r.unwrap_err().contains("outside the unit's working directory"));
    assert!(!marker.exists(), "the command ran before the output check");
}

/// The delete-declared-outputs step belongs to the in-place (overlay) branch
/// only; a copy-mode re-run starts from the copy as it is. (The in-place
/// branch needs a mounted overlay and has no unit test.)
#[test]
fn copy_mode_does_not_delete_declared_outputs() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("out.txt"), "stale").unwrap();
    let r = rerun_outputs_in_sandbox(
        "test -f out.txt",
        &ProbeValueStore::new(),
        "r",
        dir.path(),
        &std::collections::BTreeMap::new(),
        &["out.txt".to_string()],
    );
    assert!(r.is_ok(), "{r:?}");
    assert_eq!(std::fs::read_to_string(dir.path().join("out.txt")).unwrap(), "stale");
}
