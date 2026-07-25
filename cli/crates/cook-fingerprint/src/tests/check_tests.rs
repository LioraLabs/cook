use super::*;
use crate::record::FileRecord;

// -------------------------------------------------------------------------
// Task 4: hashing / mtime utilities
// -------------------------------------------------------------------------

#[test]
fn test_hash_file_deterministic() {
    let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("file.txt");
    std::fs::write(&path, b"hello world").expect("write");

    let h1 = hash_file(&path).expect("hash");
    let h2 = hash_file(&path).expect("hash");
    assert_eq!(h1, h2);
}

#[test]
fn test_hash_file_differs_on_content() {
    let dir = tempfile::tempdir().expect("tempdir");
        let p1 = dir.path().join("a.txt");
    let p2 = dir.path().join("b.txt");
    std::fs::write(&p1, b"hello").expect("write");
    std::fs::write(&p2, b"world").expect("write");

    let h1 = hash_file(&p1).expect("hash");
    let h2 = hash_file(&p2).expect("hash");
    assert_ne!(h1, h2);
}

#[test]
fn test_hash_file_missing_returns_none() {
    let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("nonexistent.txt");
    assert!(hash_file(&path).is_none());
}

#[test]
fn test_stat_mtime_returns_positive() {
    let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("file.txt");
    std::fs::write(&path, b"data").expect("write");

    let mtime = stat_mtime(&path).expect("mtime");
    assert!(mtime > 0);
}

#[test]
fn test_stat_mtime_missing_returns_none() {
    let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("nonexistent.txt");
    assert!(stat_mtime(&path).is_none());
}

#[test]
fn test_hash_str_deterministic() {
    let h1 = hash_str("gcc -O2 -c $in -o $out");
    let h2 = hash_str("gcc -O2 -c $in -o $out");
    assert_eq!(h1, h2);
}

#[test]
fn test_hash_str_differs() {
    let h1 = hash_str("gcc -O2 -c $in -o $out");
    let h2 = hash_str("clang -O2 -c $in -o $out");
    assert_ne!(h1, h2);
}

// -------------------------------------------------------------------------
// COOK-276: warm re-run attribution — full diff + cause summaries
// -------------------------------------------------------------------------

#[test]
fn check_inputs_collects_every_changed_path_not_just_first() {
    let dir = tempfile::tempdir().expect("tempdir");
        let wd = dir.path();
        for f in ["a.txt", "b.txt", "c.txt"] {
        std::fs::write(wd.join(f), format!("old {f}")).expect("write");
    }
    let cached: Vec<FileRecord> = ["a.txt", "b.txt", "c.txt"]
        .iter()
        .map(|f| FileRecord {
            path: (*f).into(),
            mtime: 0, // force the hash comparison
            hash: hash_file(&wd.join(f)).expect("hash"),
        })
        .collect();
    // Change two of the three.
    std::fs::write(wd.join("a.txt"), b"new a").expect("write");
    std::fs::write(wd.join("c.txt"), b"new c").expect("write");

    let err = check_inputs(&cached, &["a.txt", "b.txt", "c.txt"], wd).unwrap_err();
    assert_eq!(
        err,
        RebuildReason::InputsChanged {
            changed: vec!["a.txt".into(), "c.txt".into()],
            added: vec![],
            removed: vec![],
        }
    );
}

#[test]
fn check_inputs_names_added_and_removed_paths() {
    let dir = tempfile::tempdir().expect("tempdir");
        let wd = dir.path();
        std::fs::write(wd.join("keep.txt"), b"x").expect("write");
    let cached = vec![
        make_file_record("keep.txt", wd),
        FileRecord { path: "gone.txt".into(), mtime: 0, hash: 1 },
    ];
    let err = check_inputs(&cached, &["keep.txt", "new.txt"], wd).unwrap_err();
    assert_eq!(
        err,
        RebuildReason::InputsChanged {
            changed: vec![],
            added: vec!["new.txt".into()],
            removed: vec!["gone.txt".into()],
        }
    );
}

#[test]
fn cause_summary_formats_and_caps() {
    let r = RebuildReason::InputsChanged {
        changed: vec!["apps/web/manifest.json".into(), "b".into(), "c".into()],
        added: vec![],
        removed: vec![],
    };
    assert_eq!(
        r.cause_summary().unwrap(),
        "input changed: apps/web/manifest.json (+2 more)"
    );

    let single = RebuildReason::InputsChanged {
        changed: vec![],
        added: vec!["new.txt".into()],
        removed: vec![],
    };
    assert_eq!(single.cause_summary().unwrap(), "input added: new.txt");

    let mixed = RebuildReason::InputsChanged {
        changed: vec!["m.json".into()],
        added: vec!["a".into()],
        removed: vec!["r".into()],
        };
        assert_eq!(mixed.cause_summary().unwrap(), "input changed: m.json (+2 more)");

    assert_eq!(RebuildReason::EnvChanged.cause_summary().unwrap(), "env changed");
    assert_eq!(RebuildReason::CommandHashChanged.cause_summary().unwrap(), "command changed");
    assert_eq!(RebuildReason::NoCacheEntry.cause_summary(), None, "cold is not attributed");

    let reorder = RebuildReason::InputsChanged { changed: vec![], added: vec![], removed: vec![] };
    assert_eq!(reorder.cause_summary().unwrap(), "input set reordered");
}

// -------------------------------------------------------------------------
// Task 5: rebuild-check algorithm
// -------------------------------------------------------------------------

fn make_file_record(rel_path: &str, working_dir: &Path) -> FileRecord {
    let abs = working_dir.join(rel_path);
    FileRecord {
        path: rel_path.into(),
        mtime: stat_mtime(&abs).expect("mtime"),
        hash: hash_file(&abs).expect("hash"),
    }
}

#[test]
fn test_no_cache_entry_rebuilds() {
    let dir = tempfile::tempdir().expect("tempdir");
        let (result, updated) =
            needs_rebuild_cook(None, &["in.c"], &["out.o"], 0xdead, 0, 0, dir.path(), None, None, false);
    assert_eq!(result, RebuildResult::Rebuild(RebuildReason::NoCacheEntry));
    assert!(updated.is_none());
}

#[test]
fn test_command_hash_changed_rebuilds() {
    let dir = tempfile::tempdir().expect("tempdir");
        let wd = dir.path();
        std::fs::write(wd.join("in.c"), b"int main(){}").expect("write");
    std::fs::write(wd.join("out.o"), b"binary").expect("write");

    let in_record = make_file_record("in.c", wd);
    let out_record = make_file_record("out.o", wd);

    let entry = StepEntry {
        inputs: vec![in_record],
        outputs: vec![out_record],
        command_hash: 0x1111,
        env_contribution: 0,
        seal_contribution: 0,
    };

    let (result, updated) =
        needs_rebuild_cook(Some(&entry), &["in.c"], &["out.o"], 0x2222, 0, 0, wd, None, None, false);
    assert_eq!(
        result,
        RebuildResult::Rebuild(RebuildReason::CommandHashChanged)
    );
    assert!(updated.is_none());
}

#[test]
fn test_output_missing_rebuilds() {
    let dir = tempfile::tempdir().expect("tempdir");
        let wd = dir.path();
        std::fs::write(wd.join("in.c"), b"int main(){}").expect("write");
    // out.o is intentionally NOT created

    let in_record = make_file_record("in.c", wd);

    let entry = StepEntry {
        inputs: vec![in_record],
        outputs: vec![],
        command_hash: 0xbeef,
        env_contribution: 0,
        seal_contribution: 0,
    };

    let (result, updated) =
        needs_rebuild_cook(Some(&entry), &["in.c"], &["out.o"], 0xbeef, 0, 0, wd, None, None, false);
    assert_eq!(result, RebuildResult::Rebuild(RebuildReason::OutputMissing));
    assert!(updated.is_none());
}

#[test]
fn test_nothing_changed_skips() {
    let dir = tempfile::tempdir().expect("tempdir");
        let wd = dir.path();
        std::fs::write(wd.join("in.c"), b"int main(){}").expect("write");
    std::fs::write(wd.join("out.o"), b"binary").expect("write");

    let in_record = make_file_record("in.c", wd);
    let out_record = make_file_record("out.o", wd);

    let entry = StepEntry {
        inputs: vec![in_record],
        outputs: vec![out_record],
        command_hash: 0xbeef,
        env_contribution: 0,
        seal_contribution: 0,
    };

    let (result, updated) =
        needs_rebuild_cook(Some(&entry), &["in.c"], &["out.o"], 0xbeef, 0, 0, wd, None, None, false);
    assert_eq!(result, RebuildResult::Skip);
    assert!(updated.is_some());
}

#[test]
fn test_input_content_changed_rebuilds() {
    let dir = tempfile::tempdir().expect("tempdir");
        let wd = dir.path();
        std::fs::write(wd.join("in.c"), b"int main(){}").expect("write");
    std::fs::write(wd.join("out.o"), b"binary").expect("write");

    let out_record = make_file_record("out.o", wd);

    // Build a cache entry whose input mtime is stale (0) and whose hash
    // matches the OLD content.  The disk file already has different content
    // ("void foo(){}"), so when the mtime fast-path fires (0 != real mtime)
    // the hash comparison will also differ, triggering InputChanged.
    let old_hash = xxhash_rust::xxh3::xxh3_64(b"int main(){}");
    let in_record = FileRecord {
        path: "in.c".into(),
        mtime: 0, // guaranteed to differ from any real mtime
        hash: old_hash,
    };

    // Overwrite the input with different content.
    std::fs::write(wd.join("in.c"), b"void foo(){}").expect("write");

    let entry = StepEntry {
        inputs: vec![in_record],
        outputs: vec![out_record],
        command_hash: 0xbeef,
        env_contribution: 0,
        seal_contribution: 0,
    };

    let (result, updated) =
        needs_rebuild_cook(Some(&entry), &["in.c"], &["out.o"], 0xbeef, 0, 0, wd, None, None, false);
    assert_eq!(
        result,
        RebuildResult::Rebuild(RebuildReason::InputsChanged {
            changed: vec!["in.c".to_string()],
            added: vec![],
            removed: vec![],
        })
    );
    assert!(updated.is_none());
}

// -------------------------------------------------------------------------
// COOK-163: record disposition waives output-drift rebuild
// -------------------------------------------------------------------------

#[test]
fn record_unit_with_drifted_present_output_skips() {
    let dir = tempfile::tempdir().expect("tempdir");
        let wd = dir.path();
        std::fs::write(wd.join("in.c"), b"int main(){}").expect("write");
    std::fs::write(wd.join("out.o"), b"binary").expect("write");

    let in_record = make_file_record("in.c", wd);

    // Recorded output hash deliberately does NOT match the on-disk content,
    // and the mtime is stale (0) so the drift fast-path fires.
    let out_record = FileRecord {
        path: "out.o".into(),
        mtime: 0, // guaranteed to differ from any real mtime
        hash: xxhash_rust::xxh3::xxh3_64(b"different recorded bytes"),
    };

    let entry = StepEntry {
        inputs: vec![in_record],
        outputs: vec![out_record],
        command_hash: 0xbeef,
        env_contribution: 0,
        seal_contribution: 0,
    };

    // Control: a non-record unit with a drifted present output and no
    // restore_ctx falls through to OutputChanged rebuild.
    let (control, _) = needs_rebuild_cook(
        Some(&entry),
        &["in.c"],
        &["out.o"],
        0xbeef,
        0,
        0,
        wd,
        None,
        None,
        false,
    );
    assert!(matches!(control, RebuildResult::Rebuild(_)));

    // Waiver: a record unit treats the present-but-drifted output as
    // authoritative — Skip, with an updated entry.
    let (result, updated) = needs_rebuild_cook(
        Some(&entry),
        &["in.c"],
        &["out.o"],
        0xbeef,
        0,
        0,
        wd,
        None,
        None,
        true,
    );
    assert_eq!(result, RebuildResult::Skip);
    assert!(updated.is_some());
}

#[test]
fn record_unit_with_missing_output_still_rebuilds_without_restore() {
    let dir = tempfile::tempdir().expect("tempdir");
        let wd = dir.path();
        std::fs::write(wd.join("in.c"), b"int main(){}").expect("write");
    // out.o is intentionally NOT created — genuinely missing.

    let in_record = make_file_record("in.c", wd);
    let out_record = FileRecord {
        path: "out.o".into(),
        mtime: 0,
        hash: xxhash_rust::xxh3::xxh3_64(b"recorded bytes"),
    };

    let entry = StepEntry {
        inputs: vec![in_record],
        outputs: vec![out_record],
        command_hash: 0xbeef,
        env_contribution: 0,
        seal_contribution: 0,
    };

    // record cannot conjure bytes without a backend: a genuinely missing
    // output still restores/rebuilds.
    let (result, updated) = needs_rebuild_cook(
        Some(&entry),
        &["in.c"],
        &["out.o"],
        0xbeef,
        0,
        0,
        wd,
        None,
        None,
        true,
    );
    assert_eq!(result, RebuildResult::Rebuild(RebuildReason::OutputMissing));
    assert!(updated.is_none());
}

#[test]
fn test_plate_no_cache_entry_runs() {
    let dir = tempfile::tempdir().expect("tempdir");
        let (result, updated) = needs_rebuild_plate(None, &["in.c"], 0xdead, 0, 0, dir.path());
    assert_eq!(result, RebuildResult::Rebuild(RebuildReason::NoCacheEntry));
    assert!(updated.is_none());
}

#[test]
fn test_plate_nothing_changed_skips() {
    let dir = tempfile::tempdir().expect("tempdir");
        let wd = dir.path();
        std::fs::write(wd.join("in.c"), b"int main(){}").expect("write");

    let in_record = make_file_record("in.c", wd);

    let entry = StepEntry {
        inputs: vec![in_record],
        outputs: vec![],
        command_hash: 0xbeef,
        env_contribution: 0,
        seal_contribution: 0,
    };

    let (result, updated) = needs_rebuild_plate(Some(&entry), &["in.c"], 0xbeef, 0, 0, wd);
    assert_eq!(result, RebuildResult::Skip);
    let updated = updated.expect("should have updated entry");
    assert!(updated.outputs.is_empty());
}

// -------------------------------------------------------------------------
// Task 8: hash_env
// -------------------------------------------------------------------------

#[test]
fn test_hash_env_deterministic() {
    let mut env = BTreeMap::new();
    env.insert("FOO".to_string(), "bar".to_string());
        env.insert("BAZ".to_string(), "qux".to_string());

    let h1 = hash_env(&env);
    let h2 = hash_env(&env);
    assert_eq!(h1, h2);
}

#[test]
fn test_hash_env_order_independent() {
    let mut env1 = BTreeMap::new();
    env1.insert("A".to_string(), "1".to_string());
    env1.insert("B".to_string(), "2".to_string());

    let mut env2 = BTreeMap::new();
    env2.insert("B".to_string(), "2".to_string());
    env2.insert("A".to_string(), "1".to_string());

    assert_eq!(hash_env(&env1), hash_env(&env2));
}

#[test]
fn test_hash_env_differs_on_value_change() {
    let mut env1 = BTreeMap::new();
    env1.insert("KEY".to_string(), "value1".to_string());

    let mut env2 = BTreeMap::new();
    env2.insert("KEY".to_string(), "value2".to_string());

    assert_ne!(hash_env(&env1), hash_env(&env2));
}

// -------------------------------------------------------------------------
// New tests for env rebuild reason
// -------------------------------------------------------------------------

#[test]
fn env_contribution_changed_rebuilds() {
    let dir = tempfile::tempdir().expect("tempdir");
        let wd = dir.path();
        std::fs::write(wd.join("in.c"), b"int main(){}").expect("write");
    std::fs::write(wd.join("out.o"), b"binary").expect("write");

    let in_record = make_file_record("in.c", wd);
    let out_record = make_file_record("out.o", wd);

    let entry = StepEntry {
        inputs: vec![in_record],
        outputs: vec![out_record],
        command_hash: 0xbeef,
        env_contribution: 0x1111,
        seal_contribution: 0,
    };

    let (result, updated) = needs_rebuild_cook(Some(&entry), &["in.c"], &["out.o"], 0xbeef, 0x9999, 0, wd, None, None, false);
    assert_eq!(result, RebuildResult::Rebuild(RebuildReason::EnvChanged));
    assert!(updated.is_none());
}

#[test]
fn seal_contribution_changed_rebuilds() {
    let dir = tempfile::tempdir().expect("tempdir");
        let wd = dir.path();
        std::fs::write(wd.join("in.c"), b"int main(){}").expect("write");
    std::fs::write(wd.join("out.o"), b"binary").expect("write");

    let in_record = make_file_record("in.c", wd);
    let out_record = make_file_record("out.o", wd);

    let entry = StepEntry {
        inputs: vec![in_record],
        outputs: vec![out_record],
        command_hash: 0xbeef,
        env_contribution: 0,
        seal_contribution: 0x1111,
    };

    // Same command/env/inputs/outputs, different seal value -> SealChanged.
    let (result, updated) = needs_rebuild_cook(
        Some(&entry), &["in.c"], &["out.o"], 0xbeef, 0, 0x9999, wd, None, None, false,
    );
    assert_eq!(result, RebuildResult::Rebuild(RebuildReason::SealChanged));
    assert!(updated.is_none());
}

#[test]
fn augments_current_inputs_from_depfile_and_skips() {
    use cook_contracts::DiscoveredInputs;

    let dir = tempfile::tempdir().expect("tempdir");
        let wd = dir.path();
        // Lay out source, header, and a depfile that references both.
        std::fs::write(wd.join("src.c"), b"src").expect("src");
    std::fs::write(wd.join("hdr.h"), b"hdr").expect("hdr");
        std::fs::create_dir_all(wd.join(".cook/deps")).expect("mkdir");
    std::fs::write(
        wd.join(".cook/deps/src.d"),
        b"build/src.o: src.c hdr.h\n",
    ).expect("d");
    std::fs::write(wd.join("out.o"), b"obj").expect("out");

    // Build a stored entry that already has the fat input set.
    let src_hash = hash_file(&wd.join("src.c")).unwrap();
    let hdr_hash = hash_file(&wd.join("hdr.h")).unwrap();
    let out_hash = hash_file(&wd.join("out.o")).unwrap();

    let entry = StepEntry {
        inputs: vec![
            FileRecord { path: "src.c".into(), mtime: 0, hash: src_hash },
            FileRecord { path: "hdr.h".into(), mtime: 0, hash: hdr_hash },
        ],
        outputs: vec![FileRecord {
            path: "out.o".into(),
            mtime: stat_mtime(&wd.join("out.o")).unwrap_or(0),
            hash: out_hash,
        }],
        command_hash: 0xc0de,
        env_contribution: 0,
        seal_contribution: 0,
    };

    let di = DiscoveredInputs {
        from: ".cook/deps/src.d".into(),
        format: "make".into(),
    };


    // Caller passes only the declared input.
    let (result, _updated) = needs_rebuild_cook(
        Some(&entry),
        &["src.c"],
        &["out.o"],
        0xc0de,
        0,
        0,
        wd,
        None,
        Some(&di),
        false,
    );

    assert!(matches!(result, RebuildResult::Skip),
        "augmented current_inputs (declared + discovered) should match the fat entry");
}

#[test]
fn missing_depfile_does_not_force_a_rebuild() {
    // COOK-313 (absorb-and-forget), replacing the pre-COOK-313 contract that
    // this same fixture used to assert: a missing `.d` used to no-op the
    // augmentation, leaving current=[src.c] against a fat entry=[src.c,hdr.h]
    // and producing InputsChanged{removed:[hdr.h]}.
    //
    // The check no longer consults the depfile at all, so the recorded set IS
    // the current set. Every recorded input is still verified by content, and
    // here both are unchanged on disk, so this is a legitimate skip. Nothing
    // is lost: the depfile is an implicit OUTPUT, so when a real unit records
    // it as one, a missing `.d` is caught by the output walk instead — see
    // `missing_depfile_recorded_as_output_still_self_heals`.
    use cook_contracts::DiscoveredInputs;

    let dir = tempfile::tempdir().expect("tempdir");
    let wd = dir.path();
    std::fs::write(wd.join("src.c"), b"src").expect("src");
    std::fs::write(wd.join("hdr.h"), b"hdr").expect("hdr");
    std::fs::write(wd.join("out.o"), b"obj").expect("out");

    let entry = fat_entry(wd);
    let di = DiscoveredInputs {
        from: ".cook/deps/src.d".into(), // does not exist
        format: "make".into(),
    };

    let (result, _) = needs_rebuild_cook(
        Some(&entry), &["src.c"], &["out.o"], 0xc0de, 0, 0, wd, None, Some(&di), false,
    );

    assert!(matches!(result, RebuildResult::Skip), "got: {result:?}");
}

#[test]
fn settled_check_never_reads_the_depfile() {
    // The load-bearing test for COOK-313. A settled unit must skip even with
    // its depfile deleted, which is only possible if the check does not read
    // it. Before the change this same setup produced an InputsChanged
    // rebuild; the depfile's absence is now simply not consulted.
    use cook_contracts::DiscoveredInputs;

    let dir = tempfile::tempdir().expect("tempdir");
    let wd = dir.path();
    std::fs::write(wd.join("src.c"), b"src").expect("src");
    std::fs::write(wd.join("hdr.h"), b"hdr").expect("hdr");
    std::fs::write(wd.join("out.o"), b"obj").expect("out");
    std::fs::create_dir_all(wd.join(".cook/deps")).expect("mkdir");
    std::fs::write(wd.join(".cook/deps/src.d"), b"build/src.o: src.c hdr.h\n").expect("d");

    let entry = fat_entry(wd);
    let di = DiscoveredInputs { from: ".cook/deps/src.d".into(), format: "make".into() };

    // With the depfile present: skip.
    let (before, _) = needs_rebuild_cook(
        Some(&entry), &["src.c"], &["out.o"], 0xc0de, 0, 0, wd, None, Some(&di), false,
    );
    assert!(matches!(before, RebuildResult::Skip), "got: {before:?}");

    // Delete it and re-check. Identical decision — the file is not an input.
    std::fs::remove_file(wd.join(".cook/deps/src.d")).expect("rm");
    let (after, _) = needs_rebuild_cook(
        Some(&entry), &["src.c"], &["out.o"], 0xc0de, 0, 0, wd, None, Some(&di), false,
    );
    assert_eq!(before, after, "the depfile's presence must not change the decision");
}

#[test]
fn deleted_discovered_header_rebuilds() {
    // The case whose code path MOVED. It used to surface as the header
    // vanishing from the re-parsed depfile (an input-set diff); it now
    // surfaces from the content walk, because stat of a deleted file fails.
    // Same rebuild, and the reason must still name the header.
    use cook_contracts::DiscoveredInputs;

    let dir = tempfile::tempdir().expect("tempdir");
    let wd = dir.path();
    std::fs::write(wd.join("src.c"), b"src").expect("src");
    std::fs::write(wd.join("hdr.h"), b"hdr").expect("hdr");
    std::fs::write(wd.join("out.o"), b"obj").expect("out");

    let entry = fat_entry(wd);
    let di = DiscoveredInputs { from: ".cook/deps/src.d".into(), format: "make".into() };

    std::fs::remove_file(wd.join("hdr.h")).expect("rm header");

    let (result, _) = needs_rebuild_cook(
        Some(&entry), &["src.c"], &["out.o"], 0xc0de, 0, 0, wd, None, Some(&di), false,
    );

    assert!(matches!(
        &result,
        RebuildResult::Rebuild(RebuildReason::InputsChanged { changed, .. })
            if changed == &vec!["hdr.h".to_string()]
    ), "a deleted discovered header must rebuild and be named; got: {result:?}");
}

#[test]
fn changed_discovered_header_rebuilds() {
    // The ordinary incremental case: edit a header, the object rebuilds.
    use cook_contracts::DiscoveredInputs;

    let dir = tempfile::tempdir().expect("tempdir");
    let wd = dir.path();
    std::fs::write(wd.join("src.c"), b"src").expect("src");
    std::fs::write(wd.join("hdr.h"), b"hdr").expect("hdr");
    std::fs::write(wd.join("out.o"), b"obj").expect("out");

    let entry = fat_entry(wd);
    let di = DiscoveredInputs { from: ".cook/deps/src.d".into(), format: "make".into() };

    // The recorded mtime is 0, so any real on-disk mtime already forces the
    // content hash — no mtime manipulation needed to exercise the walk.
    std::fs::write(wd.join("hdr.h"), b"hdr-EDITED").expect("edit header");

    let (result, _) = needs_rebuild_cook(
        Some(&entry), &["src.c"], &["out.o"], 0xc0de, 0, 0, wd, None, Some(&di), false,
    );

    assert!(matches!(
        &result,
        RebuildResult::Rebuild(RebuildReason::InputsChanged { changed, .. })
            if changed == &vec!["hdr.h".to_string()]
    ), "got: {result:?}");
}

#[test]
fn missing_depfile_recorded_as_output_still_self_heals() {
    // The safety net for absorb-and-forget. In the real pipeline
    // record_completion appends the `.d` as an implicit OUTPUT, so a wiped
    // depfile is caught by the output walk rather than the input walk. This
    // is where the missing-depfile recovery the old input-side fallback
    // provided actually lives now.
    use cook_contracts::DiscoveredInputs;

    let dir = tempfile::tempdir().expect("tempdir");
    let wd = dir.path();
    std::fs::write(wd.join("src.c"), b"src").expect("src");
    std::fs::write(wd.join("hdr.h"), b"hdr").expect("hdr");
    std::fs::write(wd.join("out.o"), b"obj").expect("out");
    std::fs::create_dir_all(wd.join(".cook/deps")).expect("mkdir");
    std::fs::write(wd.join(".cook/deps/src.d"), b"build/src.o: src.c hdr.h\n").expect("d");

    let mut entry = fat_entry(wd);
    // Record the depfile as the implicit extra output, as the engine does.
    let d_rel = ".cook/deps/src.d";
    entry.outputs.push(FileRecord {
        path: d_rel.into(),
        mtime: stat_mtime(&wd.join(d_rel)).unwrap_or(0),
        hash: hash_file(&wd.join(d_rel)).unwrap(),
    });

    let di = DiscoveredInputs { from: d_rel.into(), format: "make".into() };

    // Settled: skip.
    let (before, _) = needs_rebuild_cook(
        Some(&entry), &["src.c"], &["out.o"], 0xc0de, 0, 0, wd, None, Some(&di), false,
    );
    assert!(matches!(before, RebuildResult::Skip), "got: {before:?}");

    // Wipe just the depfile. Without a restore_ctx there is nothing to fetch,
    // so the output walk must force a rebuild rather than silently skipping.
    std::fs::remove_file(wd.join(d_rel)).expect("rm depfile");
    let (after, _) = needs_rebuild_cook(
        Some(&entry), &["src.c"], &["out.o"], 0xc0de, 0, 0, wd, None, Some(&di), false,
    );
    assert!(
        matches!(after, RebuildResult::Rebuild(_)),
        "a wiped depfile recorded as an output must self-heal; got: {after:?}"
    );
}

/// The fat entry a `discovered_inputs` unit records: declared input plus the
/// header its depfile named, against `out.o`.
fn fat_entry(wd: &std::path::Path) -> StepEntry {
    StepEntry {
        inputs: vec![
            FileRecord { path: "src.c".into(), mtime: 0, hash: hash_file(&wd.join("src.c")).unwrap() },
            FileRecord { path: "hdr.h".into(), mtime: 0, hash: hash_file(&wd.join("hdr.h")).unwrap() },
        ],
        outputs: vec![FileRecord {
            path: "out.o".into(),
            mtime: stat_mtime(&wd.join("out.o")).unwrap_or(0),
            hash: hash_file(&wd.join("out.o")).unwrap(),
        }],
        command_hash: 0xc0de,
        env_contribution: 0,
        seal_contribution: 0,
    }
}

// -------------------------------------------------------------------------
// COOK-180: restore_one kind-dispatch + symlink-last ordering
// -------------------------------------------------------------------------

/// In-crate fake `CacheBackend` for unit-testing `restore_one`. A real
/// `LocalBackend` lives in `cook-cache`, but cook-cache dev-depends on
/// cook-fingerprint which produces two distinct crate instances in the
/// test dependency graph — so `cook_cache::LocalBackend` implements a
/// *different* `CacheBackend` trait than `crate::backend::CacheBackend`.
/// This minimal in-memory fake speaks the in-crate trait. Integrity
/// (VerifyingReader) is exercised end-to-end by cook-cache's integration
/// restore tests; here we only need faithful kind/mode/target dispatch.
#[derive(Default)]
struct FakeBackend {
    store: std::sync::Mutex<
        std::collections::HashMap<crate::backend::CloudKey, (Vec<u8>, crate::backend::ArtifactMeta)>,
    >,
}

impl FakeBackend {
    fn insert(&self, key: crate::backend::CloudKey, bytes: Vec<u8>, meta: crate::backend::ArtifactMeta) {
        self.store.lock().unwrap().insert(key, (bytes, meta));
    }
}

impl crate::backend::CacheBackend for FakeBackend {
    fn batch_query(
        &self,
        keys: &[crate::backend::CloudKey],
    ) -> crate::backend::BackendResult<std::collections::BTreeSet<crate::backend::CloudKey>> {
        let store = self.store.lock().unwrap();
        Ok(keys.iter().filter(|k| store.contains_key(*k)).copied().collect())
    }
    fn get(
        &self,
        key: &crate::backend::CloudKey,
    ) -> crate::backend::BackendResult<Option<Box<dyn std::io::Read + Send>>> {
        Ok(self.get_with_meta(key)?.map(|(r, _)| r))
    }
    fn get_with_meta(
        &self,
        key: &crate::backend::CloudKey,
    ) -> crate::backend::BackendResult<Option<(Box<dyn std::io::Read + Send>, crate::backend::ArtifactMeta)>>
    {
        Ok(self
            .store
            .lock()
            .unwrap()
            .get(key)
            .map(|(b, m)| {
                let r: Box<dyn std::io::Read + Send> = Box::new(std::io::Cursor::new(b.clone()));
                (r, m.clone())
            }))
    }
    fn put(
        &self,
        _key: &crate::backend::CloudKey,
        _reader: &mut dyn std::io::Read,
        _meta: &mut crate::backend::ArtifactMeta,
    ) -> crate::backend::BackendResult<()> {
        Ok(())
    }
    fn delete(&self, _key: &crate::backend::CloudKey) -> crate::backend::BackendResult<()> {
        Ok(())
    }
    fn health(&self) -> crate::backend::BackendResult<()> {
        Ok(())
    }
    fn put_manifest(
        &self,
        _key: &crate::backend::CloudKey,
        _manifest: &crate::backend::DeterminantManifest,
    ) -> crate::backend::BackendResult<()> {
        Ok(())
    }
    fn get_manifest(
        &self,
        _key: &crate::backend::CloudKey,
    ) -> crate::backend::BackendResult<Option<crate::backend::DeterminantManifest>> {
        Ok(None)
    }
}

fn fake_meta(
    kind: Option<String>,
    mode: u32,
    target: Option<String>,
    bytes: &[u8],
) -> crate::backend::ArtifactMeta {
    use crate::backend::ArtifactMeta;
    use std::collections::BTreeSet;
    let content_hash = {
        use sha2::{Digest, Sha256};
        let mut h = Sha256::new();
        h.update(bytes);
        h.finalize().into()
    };
    ArtifactMeta {
        recipe_namespace: "t".into(),
        command_hash: 0,
        env_contribution: 0,
        seal_contribution: 0,
        schema_version: CACHE_VERSION,
        size_bytes: bytes.len() as u64,
        tags: BTreeSet::new(),
        consulted_env_keys: BTreeSet::new(),
        output_index: 0,
        output_path: String::new(),
        content_hash,
        kind,
        mode,
        target,
    }
}

#[test]
fn restore_one_materialises_file_and_symlink() {
    use crate::backend::ArtifactMeta;

    let backend = FakeBackend::default();

    let file_key: crate::backend::CloudKey = [1u8; 32];
    let symlink_key: crate::backend::CloudKey = [2u8; 32];

    let body = b"#!/bin/sh\n";
    backend.insert(file_key, body.to_vec(), fake_meta(None, 0o755, None, body));
    backend.insert(
        symlink_key,
        Vec::new(),
        fake_meta(Some("symlink".into()), ArtifactMeta::default_mode(), Some("run".into()), b""),
    );

    let tmp = tempfile::tempdir().unwrap();
    let wd = tmp.path().join("wd");
    std::fs::create_dir_all(&wd).unwrap();

    assert!(restore_one(&backend, &file_key, &wd.join("bin/run"), &wd, None));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let m = std::fs::metadata(wd.join("bin/run")).unwrap();
        assert_eq!(m.permissions().mode() & 0o777, 0o755);
    }

    #[cfg(unix)]
    {
        assert!(restore_one(&backend, &symlink_key, &wd.join("bin/run-link"), &wd, None));
        assert!(std::fs::symlink_metadata(wd.join("bin/run-link"))
            .unwrap()
            .file_type()
            .is_symlink());
    }
}

// -------------------------------------------------------------------------
// Task 5 (COOK-180): symlink restore-time path hardening
// -------------------------------------------------------------------------

#[test]
#[cfg(unix)]
fn symlink_hardening_rejects_escapes() {
    let tmp = tempfile::tempdir().unwrap();
    let anchor = tmp.path();
    let link = anchor.join("sub/link");
    std::fs::create_dir_all(link.parent().unwrap()).unwrap();
    // absolute target rejected
    assert!(!restore_symlink_checked(anchor, &link, "/etc/passwd"));
    assert!(!link.exists());
    // parent-escape rejected
    assert!(!restore_symlink_checked(anchor, &link, "../../etc/passwd"));
    assert!(!std::fs::symlink_metadata(&link).map(|m| m.file_type().is_symlink()).unwrap_or(false));
    // sibling within anchor accepted
    assert!(restore_symlink_checked(anchor, &link, "sib"));
    assert!(std::fs::symlink_metadata(&link).unwrap().file_type().is_symlink());
}

#[test]
#[cfg(unix)]
fn symlink_hardening_allows_reentrant_within_anchor() {
    let tmp = tempfile::tempdir().unwrap();
    let anchor = tmp.path();
    let link = anchor.join("sub/link");
    std::fs::create_dir_all(link.parent().unwrap()).unwrap();
    std::fs::create_dir_all(anchor.join("sub2")).unwrap();
    // target `../sub2/x` from link-parent `sub/` resolves to `sub2/x` under anchor
    assert!(restore_symlink_checked(anchor, &link, "../sub2/x"));
    assert!(std::fs::symlink_metadata(&link).unwrap().file_type().is_symlink());
}
