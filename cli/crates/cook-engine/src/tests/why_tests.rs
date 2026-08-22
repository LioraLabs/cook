use super::*;

fn manifest(cmd: u64) -> DeterminantManifest {
    DeterminantManifest {
        schema_version: 5,
        recipe_namespace: "p/Cookfile::build".into(),
        key: "00".into(),
        command_hash: cmd,
        env_contribution: 7,
        seal_contribution: 9,
        inputs: BTreeMap::from([("src/a.c".into(), 100u64)]),
        output_paths: vec!["build/a.o".into()],
        empty_dir_outputs: Vec::new(),
        consulted_env: BTreeMap::from([("CC".into(), "gcc".into())]),
        sealed_probes: BTreeMap::from([("host".into(), "\"x86_64\"".into())]),
        observation: None,
    }
}

fn ours() -> UnitDeterminants {
    UnitDeterminants {
        command_hash: 1,
        env_contribution: 7,
        seal_contribution: 9,
        inputs: BTreeMap::from([("src/a.c".into(), 100u64)]),
        output_paths: vec!["build/a.o".into()],
        consulted_env: BTreeMap::from([("CC".into(), "gcc".into())]),
        sealed_probes: BTreeMap::from([("host".into(), "\"x86_64\"".into())]),
        pending_inputs: BTreeMap::new(),
        prior_invocation_probes: BTreeSet::new(),
        probe_lookup_failures: BTreeMap::new(),
    }
}

#[test]
fn diff_names_only_the_command_hash_when_that_is_all_that_differs() {
    let diffs = diff_against_manifest(&ours(), &manifest(2));
    assert_eq!(diffs, vec![DeterminantDiff::CommandHash { ours: 1, theirs: 2 }]);
}

#[test]
fn diff_names_a_sealed_probe_value_difference() {
    let mut o = ours();
    o.command_hash = 2;
    o.sealed_probes.insert("host".into(), "\"aarch64\"".into());
    let diffs = diff_against_manifest(&o, &manifest(2));
    assert_eq!(diffs, vec![DeterminantDiff::Probe {
        key: "host".into(),
        ours: Some("\"aarch64\"".into()),
        theirs: Some("\"x86_64\"".into()),
    }]);
}

#[test]
fn identical_determinants_produce_no_diff() {
    let mut o = ours();
    o.command_hash = 2;
    assert!(diff_against_manifest(&o, &manifest(2)).is_empty());
}

#[test]
fn present_unreadable_member_falls_back_instead_of_becoming_missing() {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir(root.path().join("unreadable")).unwrap();
    let key = cook_contracts::probe_key::qualify_for_recipe(
        "",
        &cook_contracts::probe_key::LocalProbeKey::new("files"),
    );
    let probe = cook_contracts::ProbeUnit {
        key: cook_contracts::probe_key::LocalProbeKey::new("files"),
        produce_source: cook_contracts::probe_value::FILES_MANIFEST_PRODUCE.into(),
        produce_line: 0,
        inputs: cook_contracts::ProbeInputs {
            files: vec!["unreadable".into()],
            ..Default::default()
        },
    };

    assert!(matches!(
        fresh_files_manifest(
            &key,
            &probe,
            root.path(),
            &BTreeMap::new(),
            &BTreeMap::new()
        ),
        FilesManifestFreshness::Failed(message) if message.contains("could not be read")
    ));
}
