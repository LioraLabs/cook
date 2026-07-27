//! COOK-360 prototype tests: the record/declaration agreement and the shared
//! determinant rule.

use super::*;
use crate::cache::{CacheMeta, Sharing};

/// A declaration with the given output paths. Everything else is inert — these
/// tests are about the output list and the determinants, nothing else.
fn meta(cache_key: &str, output_paths: &[&str]) -> CacheMeta {
    CacheMeta {
        recipe_name: "r".to_string(),
        project_id: "p".to_string(),
        cookfile_path: "Cookfile".to_string(),
        cache_key: cache_key.to_string(),
        input_paths: vec![],
        output_paths: output_paths.iter().map(|s| s.to_string()).collect(),
        command_hash: 0xbeef,
        env_contribution: 0,
        consulted_env: std::collections::BTreeMap::new(),
        discovered_inputs: None,
        seal_keys: std::collections::BTreeSet::new(),
        sharing: Sharing::Shared,
        record: false,
    }
}

fn rec(path: &str, mtime: u64, hash: u64) -> FileRecord {
    FileRecord::new(path, mtime, hash).expect("non-empty path")
}

fn observation() -> Observation {
    Observation::new(
        UnitOutcome::Passed,
        "out".to_string(),
        "err".to_string(),
        1.5,
        "2026-07-27T00:00:00Z".to_string(),
    )
}

// -------------------------------------------------------------------------
// effect_kind is derived from the declaration
// -------------------------------------------------------------------------

#[test]
fn no_declared_outputs_is_an_observing_unit() {
    assert_eq!(effect_kind(&meta("k", &[])), EffectKind::Observed);
}

#[test]
fn declared_outputs_make_a_producing_unit() {
    assert_eq!(effect_kind(&meta("k", &["a.o"])), EffectKind::Produced);
}

// -------------------------------------------------------------------------
// The constructor is the only way in, which is what makes the derived
// accessor on the record trustworthy.
// -------------------------------------------------------------------------

#[test]
fn an_observing_unit_may_not_record_outputs() {
    let err = UnitRecord::record(
        &meta("k", &[]),
        vec![rec("in.c", 1, 0x11)],
        vec![rec("out.o", 2, 0x22)],
        0,
        observation(),
    )
    .expect_err("evidence contradicts the declaration");
    assert_eq!(err, RecordMismatch::ObservingUnitHasOutputs { found: 1 });
}

/// A declared terminal output (`dist/**`) may legitimately resolve to nothing,
/// so this direction is a legal state, not a contradiction.
#[test]
fn a_producing_unit_may_record_zero_outputs() {
    let r = UnitRecord::record(&meta("k", &["dist/**"]), vec![], vec![], 0, observation())
        .expect("a glob may resolve empty");
    assert_eq!(r.effect_kind(), EffectKind::Observed);
}

#[test]
fn a_records_effect_kind_agrees_with_its_declaration() {
    let m = meta("k", &["out.o"]);
    let r = UnitRecord::record(&m, vec![], vec![rec("out.o", 2, 0x22)], 0, observation())
        .expect("agrees");
    assert_eq!(r.effect_kind(), effect_kind(&m));
    assert_eq!(r.effect_kind(), EffectKind::Produced);
}

#[test]
fn a_record_carries_the_declarations_determinants() {
    let m = meta("k", &["out.o"]);
    let r = UnitRecord::record(&m, vec![], vec![rec("out.o", 2, 0x22)], 0x5ea1, observation())
        .expect("agrees");
    assert_eq!(r.key(), "k");
    assert_eq!(r.command_hash(), 0xbeef);
    assert_eq!(r.env_contribution(), 0);
    assert_eq!(r.seal_contribution(), 0x5ea1);
}

#[test]
fn a_file_record_needs_a_path() {
    assert!(FileRecord::new("", 1, 2).is_none());
}

// -------------------------------------------------------------------------
// The one transition
// -------------------------------------------------------------------------

#[test]
fn refreshing_inputs_replaces_only_the_inputs() {
    let m = meta("k", &["out.o"]);
    let before = UnitRecord::record(
        &m,
        vec![rec("in.c", 1, 0x11)],
        vec![rec("out.o", 2, 0x22)],
        0x5ea1,
        observation(),
    )
    .expect("agrees");

    let after = before.clone().with_refreshed_inputs(vec![rec("in.c", 99, 0x11)]);

    // The input record moved.
    assert_eq!(after.inputs().len(), 1);
    assert_eq!(after.inputs()[0].mtime(), 99);
    assert_eq!(before.inputs()[0].mtime(), 1);

    // Nothing else did.
    assert_eq!(after.key(), before.key());
    assert_eq!(after.outputs(), before.outputs());
    assert_eq!(after.command_hash(), before.command_hash());
    assert_eq!(after.env_contribution(), before.env_contribution());
    assert_eq!(after.seal_contribution(), before.seal_contribution());
    assert_eq!(after.observation(), before.observation());
    assert_eq!(after.effect_kind(), before.effect_kind());
}

// -------------------------------------------------------------------------
// The rule both stores share
// -------------------------------------------------------------------------

fn producing_record() -> UnitRecord {
    UnitRecord::record(
        &meta("k", &["out.o"]),
        vec![rec("in.c", 1, 0x11)],
        vec![rec("out.o", 2, 0x22)],
        0x5ea1,
        observation(),
    )
    .expect("agrees")
}

#[test]
fn unmoved_determinants_permit_a_replay() {
    let r = producing_record();
    assert_eq!(determinant_drift(&r, &Determinants::new("k", 0xbeef, 0, 0x5ea1)), None);
}

#[test]
fn a_key_filed_elsewhere_is_rejected_before_anything_else() {
    let r = producing_record();
    // Every other determinant has ALSO moved; the key is still reported,
    // because placement is never trusted over content.
    assert_eq!(
        determinant_drift(&r, &Determinants::new("other", 0xffff, 0xffff, 0xffff)),
        Some(DeterminantDrift::Key)
    );
}

#[test]
fn each_determinant_is_reported_in_order() {
    let r = producing_record();
    assert_eq!(
        determinant_drift(&r, &Determinants::new("k", 0xffff, 0, 0x5ea1)),
        Some(DeterminantDrift::CommandHash)
    );
    assert_eq!(
        determinant_drift(&r, &Determinants::new("k", 0xbeef, 0xffff, 0x5ea1)),
        Some(DeterminantDrift::Env)
    );
    assert_eq!(
        determinant_drift(&r, &Determinants::new("k", 0xbeef, 0, 0xffff)),
        Some(DeterminantDrift::Seal)
    );
}

/// The rule is indifferent to effect kind — that is the whole point.
#[test]
fn an_observing_unit_obeys_the_same_determinant_rule() {
    let r = UnitRecord::record(
        &meta("k", &[]),
        vec![rec("in.c", 1, 0x11)],
        vec![],
        0x5ea1,
        observation(),
    )
    .expect("agrees");
    assert_eq!(r.effect_kind(), EffectKind::Observed);
    assert_eq!(determinant_drift(&r, &Determinants::new("k", 0xbeef, 0, 0x5ea1)), None);
    assert_eq!(
        determinant_drift(&r, &Determinants::new("k", 0xffff, 0, 0x5ea1)),
        Some(DeterminantDrift::CommandHash)
    );
}
