use super::*;

fn key(byte: u8) -> CloudKey {
    let mut k = [0u8; 32];
    k[0] = byte;
    k
}

#[test]
fn artifact_key_deterministic() {
    let cloud_k = key(0xAB);
    let a = artifact_key(&cloud_k, 0, "build/foo.o");
    let b = artifact_key(&cloud_k, 0, "build/foo.o");
    assert_eq!(a, b);
}

#[test]
fn artifact_key_differs_on_index() {
    let cloud_k = key(0xAB);
    let a = artifact_key(&cloud_k, 0, "build/foo.o");
    let b = artifact_key(&cloud_k, 1, "build/foo.o");
    assert_ne!(a, b);
}

#[test]
fn artifact_key_differs_on_path() {
    let cloud_k = key(0xAB);
    let a = artifact_key(&cloud_k, 0, "build/foo.o");
    let b = artifact_key(&cloud_k, 0, "build/bar.o");
    assert_ne!(a, b);
}

#[test]
fn artifact_key_differs_on_cloud_key() {
    let a = artifact_key(&key(0x01), 0, "build/foo.o");
    let b = artifact_key(&key(0x02), 0, "build/foo.o");
    assert_ne!(a, b);
}

#[test]
fn discovered_inputs_manifest_key_is_distinct() {
    let base = [3u8; 32];
    let manifest = artifact_key(
        &base,
        DISCOVERED_INPUTS_MANIFEST_INDEX,
        DISCOVERED_INPUTS_MANIFEST_PATH,
    );
    let out0 = artifact_key(&base, 0, "out");
    assert_ne!(manifest, out0);
}

#[test]
fn observation_key_is_outside_the_output_and_manifest_ranges() {
    let base = [3u8; 32];
    let observation = artifact_key(&base, OBSERVATION_INDEX, OBSERVATION_PATH);
    assert_ne!(observation, artifact_key(&base, 0, "out"));
    assert_ne!(
        observation,
        artifact_key(
            &base,
            DISCOVERED_INPUTS_MANIFEST_INDEX,
            DISCOVERED_INPUTS_MANIFEST_PATH,
        )
    );
    assert_ne!(
        observation,
        artifact_key(
            &base,
            DISCOVERED_INPUT_SETS_INDEX,
            DISCOVERED_INPUT_SETS_PATH
        )
    );
}

// ─── cloud_key composition tests ────────────────────────────────────────

fn make_key_inputs() -> CloudKeyInputs<'static> {
    CloudKeyInputs {
        schema_version: 3,
        recipe_namespace: "cook/Cookfile::build",
        command_hash: 0xAAAA,
        env_contribution: 0xCCCC,
        seal_contribution: 0xDDDD,
        sorted_input_content_hashes: &[0x1111, 0x2222, 0x3333],
    }
}

#[test]
fn cloud_key_deterministic() {
    let inputs = make_key_inputs();
    let k1 = cloud_key(&inputs);
    let k2 = cloud_key(&inputs);
    assert_eq!(k1, k2);
}

#[test]
fn cloud_key_changes_on_command_hash_change() {
    let a = make_key_inputs();
    let mut b = a;
    b.command_hash = 0xFFFF;
    assert_ne!(cloud_key(&a), cloud_key(&b));
}

#[test]
fn cloud_key_changes_on_env_contribution_change() {
    let a = make_key_inputs();
    let mut b = a;
    b.env_contribution = 0xFFFF;
    assert_ne!(cloud_key(&a), cloud_key(&b));
}

#[test]
fn cloud_key_changes_on_schema_version_change() {
    let a = make_key_inputs();
    let mut b = a;
    b.schema_version = 4;
    assert_ne!(cloud_key(&a), cloud_key(&b));
}

#[test]
fn cloud_key_changes_on_namespace_change() {
    let a = make_key_inputs();
    let mut b = a;
    b.recipe_namespace = "cook/Cookfile::test";
    assert_ne!(cloud_key(&a), cloud_key(&b));
}

#[test]
fn cloud_key_changes_on_input_content_change() {
    let a = make_key_inputs();
    let alt_inputs = [0x1111, 0x2222, 0x9999]; // last hash differs
    let b = CloudKeyInputs { sorted_input_content_hashes: &alt_inputs, ..a };
    assert_ne!(cloud_key(&a), cloud_key(&b));
}

#[test]
fn cloud_key_caller_must_sort_inputs() {
    // The function trusts its caller's sort. A caller-sorted slice produces
    // a stable hash; an unsorted slice produces a different (but stable) one.
    // This test documents that the sort is the caller's responsibility.
    let sorted = [0x1111u64, 0x2222, 0x3333];
    let unsorted = [0x3333u64, 0x1111, 0x2222];
    let a = make_key_inputs();
    let b = CloudKeyInputs { sorted_input_content_hashes: &sorted, ..a };
    let c = CloudKeyInputs { sorted_input_content_hashes: &unsorted, ..a };
    assert_ne!(cloud_key(&b), cloud_key(&c),
        "the function does not internally sort; caller responsibility");
}

#[test]
fn cloud_key_returns_32_bytes() {
    let k = cloud_key(&make_key_inputs());
    assert_eq!(k.len(), 32);
}

#[test]
fn cloud_key_changes_on_seal_contribution_change() {
    let a = make_key_inputs();
    let mut b = a;
    b.seal_contribution = 0xFFFF;
    assert_ne!(cloud_key(&a), cloud_key(&b));
}

#[test]
fn cloud_key_zero_seal_is_stable() {
    let a = make_key_inputs();
    let b = a;
    assert_eq!(cloud_key(&a), cloud_key(&b));
}

// ---- CS-0074 ArtifactMeta.kind tests ----

fn minimal_meta_json(extra: &str) -> String {
    format!(
        r#"{{
                "recipe_namespace": "ns",
                "command_hash": 0,
                "env_contribution": 0,
                "schema_version": 1,
                "size_bytes": 0,
                "tags": [],
                "consulted_env_keys": [],
                "output_index": 0,
                "output_path": "a.o"
                {}
            }}"#,
        extra
    )
}

#[test]
fn artifact_meta_kind_defaults_to_none_for_legacy_sidecars() {
    let json = minimal_meta_json("");
    let meta: ArtifactMeta = serde_json::from_str(&json).unwrap();
    assert!(meta.kind.is_none());
}

#[test]
fn artifact_meta_kind_round_trips_when_set() {
    let json = minimal_meta_json(r#", "kind": "probe_value""#);
    let meta: ArtifactMeta = serde_json::from_str(&json).unwrap();
    assert_eq!(meta.kind.as_deref(), Some("probe_value"));
    // Round-trip through serde_json
    let s = serde_json::to_string(&meta).unwrap();
    let back: ArtifactMeta = serde_json::from_str(&s).unwrap();
    assert_eq!(back.kind.as_deref(), Some("probe_value"));
}

#[test]
fn artifact_meta_as_probe_value_sets_kind() {
    let meta = ArtifactMeta {
        recipe_namespace: "ns".into(),
        command_hash: 0,
        env_contribution: 0,
        seal_contribution: 0,
        schema_version: 1,
        size_bytes: 0,
        tags: BTreeSet::new(),
        consulted_env_keys: BTreeSet::new(),
        output_index: 0,
        output_path: "probe.bin".into(),
        content_hash: ArtifactMeta::zero_content_hash(),
        kind: None,
        mode: ArtifactMeta::default_mode(),
        target: None,
    }
    .as_probe_value();
    assert_eq!(meta.kind.as_deref(), Some("probe_value"));
}

#[test]
fn artifact_meta_kind_none_not_serialised() {
    let meta = ArtifactMeta {
        recipe_namespace: "ns".into(),
        command_hash: 0,
        env_contribution: 0,
        seal_contribution: 0,
        schema_version: 1,
        size_bytes: 0,
        tags: BTreeSet::new(),
        consulted_env_keys: BTreeSet::new(),
        output_index: 0,
        output_path: "a.o".into(),
        content_hash: ArtifactMeta::zero_content_hash(),
        kind: None,
        mode: ArtifactMeta::default_mode(),
        target: None,
    };
    let s = serde_json::to_string(&meta).unwrap();
    assert!(!s.contains("kind"), "kind: None MUST be omitted from JSON: {s}");
}

// ---- end CS-0074 ----

// ---- COOK-180: mode + target fidelity tests ----

#[test]
fn artifact_meta_mode_and_symlink_target_round_trip() {
    let json = minimal_meta_json(
        r#", "mode": 493, "kind": "symlink", "target": "../sib""#,
    );
    let meta: ArtifactMeta = serde_json::from_str(&json).expect("parse");
    assert_eq!(meta.mode, 0o755);
    assert_eq!(meta.kind.as_deref(), Some("symlink"));
    assert_eq!(meta.target.as_deref(), Some("../sib"));
    let s = serde_json::to_string(&meta).expect("serialize");
    let back: ArtifactMeta = serde_json::from_str(&s).expect("reparse");
    assert_eq!(meta, back);
}

#[test]
fn artifact_meta_legacy_sidecar_defaults_mode_and_target() {
    // A pre-fidelity sidecar lacks mode/target entirely.
    let meta: ArtifactMeta = serde_json::from_str(&minimal_meta_json("")).expect("parse");
    assert_eq!(meta.mode, 0o644);
    assert!(meta.target.is_none());
}

// ---- end COOK-180 ----

#[test]
fn determinant_manifest_serializes_deterministically() {
    use std::collections::BTreeMap;
    let mut inputs = BTreeMap::new();
    inputs.insert("src/a.c".to_string(), 0xAABB_u64);
    inputs.insert("src/b.c".to_string(), 0xFFFF_FFFF_FFFF_FFFF_u64);
    let mut env = BTreeMap::new();
    env.insert("CC".to_string(), "clang".to_string());
    let mut probes = BTreeMap::new();
    probes.insert("host".to_string(), "\"x86_64-linux\"".to_string());
    let m = DeterminantManifest {
        schema_version: 5,
        recipe_namespace: "cook/Cookfile::build".into(),
        key: "ab".repeat(32),
        command_hash: 0x1234,
        env_contribution: 0x5678,
        seal_contribution: 0x9abc,
        inputs,
        output_paths: vec!["build/a.o".into()],
        empty_dir_outputs: Vec::new(),
        consulted_env: env,
        sealed_probes: probes,
        observation: None,
    };
    let a = serde_json::to_vec(&m).unwrap();
    let b = serde_json::to_vec(&m.clone()).unwrap();
    assert_eq!(a, b, "same manifest must serialize to identical bytes");
    let back: DeterminantManifest = serde_json::from_slice(&a).unwrap();
    assert_eq!(back.inputs["src/b.c"], 0xFFFF_FFFF_FFFF_FFFF_u64);
    assert_eq!(back, m);
}

// ---------------------------------------------------------------------------
// CS-0204: the extra-input path-set manifest
// ---------------------------------------------------------------------------

/// Newest first, deduplicated, capped. The dedup is what stops a settled
/// project from rewriting a growing manifest on every run.
#[test]
fn merging_a_path_set_puts_it_first_and_deduplicates() {
    let old = vec![vec!["a.lua".to_string()], vec!["b.lua".to_string()]];
    let merged = merge_path_set(&old, &["b.lua".to_string()]);
    assert_eq!(merged, vec![vec!["b.lua".to_string()], vec!["a.lua".to_string()]]);
}

#[test]
fn merging_caps_the_retained_sets() {
    let mut existing: Vec<Vec<String>> = Vec::new();
    for i in 0..(MODULE_SET_CAP + 5) {
        existing = merge_path_set(&existing, &[format!("m{i}.lua")]);
    }
    assert_eq!(existing.len(), MODULE_SET_CAP);
    assert_eq!(existing[0], vec![format!("m{}.lua", MODULE_SET_CAP + 4)]);
}

#[test]
fn path_sets_round_trip_through_the_wire_form() {
    let sets = vec![
        vec![
            ".cook/modules/share/lua/5.4/a.lua".to_string(),
            ".cook/modules/share/lua/5.4/b.lua".to_string(),
        ],
        vec![],
    ];
    assert_eq!(decode_path_sets(&encode_path_sets(&sets)), sets);
}

/// Total: a corrupt manifest decodes to nothing, which every caller reads as a
/// safe miss rather than as a reason to panic.
#[test]
fn malformed_manifest_bytes_decode_to_no_sets() {
    assert!(decode_path_sets(b"not json at all").is_empty());
    assert!(decode_path_sets(b"[\"a string, not a list of lists\"]").is_empty());
    assert!(decode_path_sets(&[]).is_empty());
}
