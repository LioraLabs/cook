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
