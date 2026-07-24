use super::*;
use cook_contracts::WorkPayload;

#[test]
fn empty_dirs_under_reports_only_empty_dirs() {
    let tmp = tempfile::tempdir().unwrap();
    let wd = tmp.path();
    std::fs::create_dir_all(wd.join("out/empty")).unwrap();
    std::fs::create_dir_all(wd.join("out/full")).unwrap();
    std::fs::write(wd.join("out/full/f"), b"x").unwrap();
    let mut got = empty_dirs_under(wd, "out");
    got.sort();
    assert_eq!(got, vec!["out/empty".to_string()]);
}

#[test]
fn test_hash_str_deterministic() {
    let h1 = hash_str("hello");
    let h2 = hash_str("hello");
    assert_eq!(h1, h2);
}

#[test]
fn test_hash_str_differs() {
    let h1 = hash_str("hello");
    let h2 = hash_str("world");
    assert_ne!(h1, h2);
}

fn empty_inputs() -> FingerprintInputs {
    FingerprintInputs::default()
}

// ── CS-0159: sealed-probe fold in the test fingerprint ──────────────

fn sealed(pairs: &[(&str, &str)]) -> FingerprintInputs {
    FingerprintInputs {
        sealed_probes: pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect(),
        ..Default::default()
    }
}

/// A test that seals nothing hashes exactly as it did pre-CS-0159 — the
/// surface is additive, so no existing test-cache entry is invalidated.
#[test]
fn seal_empty_set_leaves_fingerprint_unchanged() {
    let p = make_test_payload("./t", 0, false, "s", "t");
    assert_eq!(
        compute_test_fingerprint(&p, &empty_inputs()),
        compute_test_fingerprint(&p, &sealed(&[]))
    );
}

/// A sealed probe's VALUE is a determinant: same key, different value =>
/// different key. This is the whole point of sealing a test.
#[test]
fn seal_value_change_busts_fingerprint() {
    let p = make_test_payload("./t", 0, false, "s", "t");
    let a = compute_test_fingerprint(&p, &sealed(&[("toolchain", "gcc-13")]));
    let b = compute_test_fingerprint(&p, &sealed(&[("toolchain", "gcc-14")]));
    assert_ne!(a, b);
}

/// Sealing at all changes the key relative to not sealing.
#[test]
fn seal_presence_changes_fingerprint() {
    let p = make_test_payload("./t", 0, false, "s", "t");
    assert_ne!(
        compute_test_fingerprint(&p, &empty_inputs()),
        compute_test_fingerprint(&p, &sealed(&[("toolchain", "gcc-13")]))
    );
}

/// The fold is order-insensitive — the author's declaration order MUST NOT
/// affect the key (the engine passes a BTreeMap, but the hash sorts too).
#[test]
fn seal_fold_is_order_insensitive() {
    let p = make_test_payload("./t", 0, false, "s", "t");
    let a = compute_test_fingerprint(&p, &sealed(&[("a", "1"), ("b", "2")]));
    let b = compute_test_fingerprint(&p, &sealed(&[("b", "2"), ("a", "1")]));
    assert_eq!(a, b);
}

/// Key and value are not interchangeable — swapping them MUST NOT collide.
#[test]
fn seal_key_value_swap_does_not_collide() {
    let p = make_test_payload("./t", 0, false, "s", "t");
    let a = compute_test_fingerprint(&p, &sealed(&[("k", "v")]));
    let b = compute_test_fingerprint(&p, &sealed(&[("v", "k")]));
    assert_ne!(a, b);
}

/// The sealed fold occupies its own slot: a sealed probe MUST NOT be
/// confusable with an env-var contribution of the same key/value.
#[test]
fn seal_does_not_collide_with_env_contribution() {
    let p = make_test_payload("./t", 0, false, "s", "t");
    let as_seal = compute_test_fingerprint(&p, &sealed(&[("K", "V")]));
    let as_env = compute_test_fingerprint(
        &p,
        &FingerprintInputs {
            env_keys: vec![("K".to_string(), "V".to_string())],
            ..Default::default()
        },
    );
    assert_ne!(as_seal, as_env);
}

fn make_test_payload(
    cmd: &str,
    timeout: u64,
    should_fail: bool,
    suite_name: &str,
    test_name: &str,
) -> WorkPayload {
    WorkPayload::Test {
        seal_keys: Default::default(),
        cmd: cmd.into(),
        line: 1,
        timeout,
        should_fail,
        suite_name: suite_name.into(),
        test_name: test_name.into(),
        iteration_item: None,
        lua_code: None,
        input_paths: vec![],
    }
}

#[test]
fn test_unit_fingerprint_includes_timeout() {
    let fp_30 = compute_test_fingerprint(
        &make_test_payload("true", 30, false, "r", "t"),
        &empty_inputs(),
    );
    let fp_60 = compute_test_fingerprint(
        &make_test_payload("true", 60, false, "r", "t"),
        &empty_inputs(),
    );
    assert_ne!(
        fp_30, fp_60,
        "different timeouts must produce different fingerprints"
    );
}

#[test]
fn test_unit_fingerprint_includes_should_fail() {
    let fp_t = compute_test_fingerprint(
        &make_test_payload("true", 30, true, "r", "t"),
        &empty_inputs(),
    );
    let fp_f = compute_test_fingerprint(
        &make_test_payload("true", 30, false, "r", "t"),
        &empty_inputs(),
    );
    assert_ne!(fp_t, fp_f);
}

#[test]
fn test_unit_fingerprint_independent_of_test_name() {
    // Renaming via `as` (the test_name) MUST NOT bust fingerprint per CS-0061 §3.3.
    let fp_a = compute_test_fingerprint(
        &make_test_payload("true", 30, false, "r", "alpha"),
        &empty_inputs(),
    );
    let fp_b = compute_test_fingerprint(
        &make_test_payload("true", 30, false, "r", "beta"),
        &empty_inputs(),
    );
    assert_eq!(fp_a, fp_b, "renaming a test MUST NOT bust its fingerprint");
}

#[test]
fn test_unit_fingerprint_independent_of_suite_name() {
    let fp_a = compute_test_fingerprint(
        &make_test_payload("true", 30, false, "recipe_a", "t"),
        &empty_inputs(),
    );
    let fp_b = compute_test_fingerprint(
        &make_test_payload("true", 30, false, "recipe_b", "t"),
        &empty_inputs(),
    );
    assert_eq!(fp_a, fp_b);
}

#[test]
fn test_unit_fingerprint_deterministic() {
    let payload = make_test_payload("run_tests.sh", 120, false, "suite", "test1");
    let inputs = FingerprintInputs {
        sealed_probes: vec![],
        cook_outputs: vec![("out/lib.a".into(), "sha256:abc".into())],
        dep_outputs: vec![],
        env_keys: vec![("CC".into(), "gcc".into())],
    };
    let fp1 = compute_test_fingerprint(&payload, &inputs);
    let fp2 = compute_test_fingerprint(&payload, &inputs);
    assert_eq!(fp1, fp2);
    assert!(fp1.starts_with("sha256:"));
}

#[test]
fn test_unit_fingerprint_includes_cmd() {
    let fp_a = compute_test_fingerprint(
        &make_test_payload("cmd_a", 30, false, "r", "t"),
        &empty_inputs(),
    );
    let fp_b = compute_test_fingerprint(
        &make_test_payload("cmd_b", 30, false, "r", "t"),
        &empty_inputs(),
    );
    assert_ne!(fp_a, fp_b, "different commands must produce different fingerprints");
}

#[test]
fn glob_meta_literal_paths_return_false() {
    assert!(!has_glob_meta(""));
    assert!(!has_glob_meta("main.c"));
    assert!(!has_glob_meta("build/main.o"));
    assert!(!has_glob_meta("apps/web/.next/BUILD_ID"));
    assert!(!has_glob_meta("a/b/c/d.txt"));
}

#[test]
fn glob_meta_star_returns_true() {
    assert!(has_glob_meta("*"));
    assert!(has_glob_meta("*.c"));
    assert!(has_glob_meta("src/**"));
    assert!(has_glob_meta("src/**/*"));
    assert!(has_glob_meta("apps/web/.next/**"));
}

#[test]
fn glob_meta_question_returns_true() {
    assert!(has_glob_meta("?"));
    assert!(has_glob_meta("file?.txt"));
}

#[test]
fn glob_meta_bracket_returns_true() {
    assert!(has_glob_meta("[abc].txt"));
    assert!(has_glob_meta("src/[ab]/main.c"));
}

#[test]
fn glob_meta_brace_returns_false() {
    // The reference engine's `glob = "0.3"` crate does NOT support
    // brace alternation; `{` is treated as a literal. Per CS-0085
    // the spec excludes `{` from the metacharacter set so that a
    // string like "out/{a,b}.txt" is treated as a LITERAL PATH,
    // not as a glob pattern. Brace expansion may be added in a
    // future CS once the reference engine supports it.
    assert!(!has_glob_meta("{a,b}.txt"));
    assert!(!has_glob_meta("src/{lib,app}/main.c"));
}

#[test]
fn test_unit_fingerprint_cook_outputs_order_independent() {
    let inputs_a = FingerprintInputs {
        sealed_probes: vec![],
        cook_outputs: vec![
            ("a".into(), "hash1".into()),
            ("b".into(), "hash2".into()),
        ],
        ..Default::default()
    };
    let inputs_b = FingerprintInputs {
        sealed_probes: vec![],
        cook_outputs: vec![
            ("b".into(), "hash2".into()),
            ("a".into(), "hash1".into()),
        ],
        ..Default::default()
    };
    let payload = make_test_payload("true", 30, false, "r", "t");
    assert_eq!(
        compute_test_fingerprint(&payload, &inputs_a),
        compute_test_fingerprint(&payload, &inputs_b),
        "cook_outputs insertion order must not affect fingerprint"
    );
}

#[test]
fn reconcile_dir_output_deletes_strays_keeps_set_prunes_empty() {
    let tmp = tempfile::tempdir().unwrap();
    let wd = tmp.path();
    std::fs::create_dir_all(wd.join("pkg/sub")).unwrap();
    std::fs::write(wd.join("pkg/a.js"), b"a").unwrap();        // kept
    std::fs::write(wd.join("pkg/STRAY.txt"), b"x").unwrap();   // delete
    std::fs::write(wd.join("pkg/sub/old.wasm"), b"o").unwrap();// delete -> sub becomes empty

    let kept: std::collections::BTreeSet<String> =
        ["pkg/a.js".to_string()].into_iter().collect();
    reconcile_dir_output(wd, "pkg", &kept);

    assert!(wd.join("pkg/a.js").exists());
    assert!(!wd.join("pkg/STRAY.txt").exists());
    assert!(!wd.join("pkg/sub/old.wasm").exists());
    assert!(!wd.join("pkg/sub").exists());   // pruned empty dir
    assert!(wd.join("pkg").exists());        // root dir preserved
}

#[test]
fn reconcile_preserves_kept_empty_dir_prunes_unkept() {
    let tmp = tempfile::tempdir().unwrap();
    let wd = tmp.path();
    std::fs::create_dir_all(wd.join("out/keep")).unwrap(); // recorded empty dir
    std::fs::create_dir_all(wd.join("out/stray")).unwrap(); // not recorded
    std::fs::write(wd.join("out/f"), b"x").unwrap();
    let mut kept = std::collections::BTreeSet::new();
    kept.insert("out/f".to_string());
    kept.insert("out/keep".to_string());
    reconcile_dir_output(wd, "out", &kept);
    assert!(wd.join("out/keep").is_dir(), "kept empty dir must survive");
    assert!(
        !wd.join("out/stray").exists(),
        "unrecorded empty dir must be pruned"
    );
    assert!(wd.join("out/f").is_file());
}

#[test]
fn reconcile_dir_output_trailing_slash_root_works_identically() {
    // A caller that passes "pkg/" (with trailing slash) must behave
    // identically to "pkg" — stray deleted, kept file preserved, empty
    // subdirectory pruned.
    let tmp = tempfile::tempdir().unwrap();
    let wd = tmp.path();
    std::fs::create_dir_all(wd.join("pkg/sub")).unwrap();
    std::fs::write(wd.join("pkg/a.js"), b"a").unwrap();        // kept
    std::fs::write(wd.join("pkg/STRAY.txt"), b"x").unwrap();   // delete
    std::fs::write(wd.join("pkg/sub/old.wasm"), b"o").unwrap();// delete -> sub becomes empty

    let kept: std::collections::BTreeSet<String> =
        ["pkg/a.js".to_string()].into_iter().collect();
    // Pass root with trailing slash — must behave the same as "pkg".
    reconcile_dir_output(wd, "pkg/", &kept);

    assert!(wd.join("pkg/a.js").exists());
    assert!(!wd.join("pkg/STRAY.txt").exists());
    assert!(!wd.join("pkg/sub/old.wasm").exists());
    assert!(!wd.join("pkg/sub").exists());   // pruned empty dir
    assert!(wd.join("pkg").exists());        // root dir preserved
}

#[test]
fn terminal_output_covers_globs_and_dir_outputs() {
    assert!(is_dir_output("pkg/"));
    assert!(!is_dir_output("pkg"));
    assert!(!is_dir_output("pkg/file.js"));

    assert!(is_terminal_output("pkg/"));        // directory output (CS-0119)
    assert!(is_terminal_output("out/*.o"));     // glob (CS-0085)
    assert!(is_terminal_output("a/**"));        // glob
    assert!(!is_terminal_output("build/app"));  // literal
}
