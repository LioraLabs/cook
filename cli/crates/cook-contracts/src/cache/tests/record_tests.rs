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
// Cacheability keeps three states apart that were long expressed as two
// -------------------------------------------------------------------------

#[test]
fn no_declaration_means_never_cached() {
    assert_eq!(cacheability(None), Cacheability::Uncacheable);
}

/// The state the implementation had no way to say: cacheable, but with
/// nothing in the artifact store. Tests today; any other output-less step
/// kind later, without further change here.
#[test]
fn a_declaration_without_outputs_is_cached_by_result() {
    assert_eq!(cacheability(Some(&meta("k", &[]))), Cacheability::ResultOnly);
}

#[test]
fn a_declaration_with_outputs_is_cached_by_artifact() {
    assert_eq!(cacheability(Some(&meta("k", &["out.o"]))), Cacheability::Artifacts);
}

/// Absence of a declaration is the ONLY thing cacheability adds over
/// effect_kind — the two agree wherever both are defined.
#[test]
fn cacheability_agrees_with_effect_kind_wherever_both_apply() {
    for outputs in [&[][..], &["out.o"][..]] {
        let m = meta("k", outputs);
        let expected = match effect_kind(&m) {
            EffectKind::Observed => Cacheability::ResultOnly,
            EffectKind::Produced => Cacheability::Artifacts,
        };
        assert_eq!(cacheability(Some(&m)), expected);
    }
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
    assert_eq!(determinant_drift(&r.determinants(), &Determinants::new(0xbeef, 0, 0x5ea1)), None);
}

/// Store integrity is a different question from invalidation, and is asked
/// separately: a record whose determinants are untouched can still be the
/// wrong record to have been handed back.
#[test]
fn a_record_knows_which_key_it_was_filed_under() {
    let r = producing_record();
    assert!(r.is_addressed_by("k"));
    assert!(!r.is_addressed_by("other"));
    // Its determinants are meanwhile unmoved — the two questions are independent.
    assert_eq!(determinant_drift(&r.determinants(), &Determinants::new(0xbeef, 0, 0x5ea1)), None);
}

#[test]
fn each_determinant_is_reported_in_order() {
    let r = producing_record();
    assert_eq!(
        determinant_drift(&r.determinants(), &Determinants::new(0xffff, 0, 0x5ea1)),
        Some(DeterminantDrift::CommandHash)
    );
    assert_eq!(
        determinant_drift(&r.determinants(), &Determinants::new(0xbeef, 0xffff, 0x5ea1)),
        Some(DeterminantDrift::Env)
    );
    assert_eq!(
        determinant_drift(&r.determinants(), &Determinants::new(0xbeef, 0, 0xffff)),
        Some(DeterminantDrift::Seal)
    );
}

// -------------------------------------------------------------------------
// Wire format
// -------------------------------------------------------------------------

#[test]
fn a_record_survives_a_wire_round_trip_unchanged() {
    let before = producing_record();
    let after = UnitRecord::from_wire(before.clone().into_wire()).expect("round trips");
    assert_eq!(after, before);
}

#[test]
fn the_wire_round_trips_through_json() {
    let wire = producing_record().into_wire();
    let json = serde_json::to_string(&wire).expect("serialize");
    let back: UnitRecordWire = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(back, wire);
    assert_eq!(UnitRecord::from_wire(back).expect("valid"), producing_record());
}

#[test]
fn a_wire_record_is_stamped_with_the_current_schema_version() {
    assert_eq!(producing_record().into_wire().schema_version, RECORD_SCHEMA_VERSION);
}

#[test]
fn a_superseded_schema_is_rejected_rather_than_migrated() {
    let mut wire = producing_record().into_wire();
    wire.schema_version = RECORD_SCHEMA_VERSION - 1;
    assert_eq!(
        UnitRecord::from_wire(wire).expect_err("must not be read"),
        WireError::SchemaVersion {
            found: RECORD_SCHEMA_VERSION - 1,
            expected: RECORD_SCHEMA_VERSION
        }
    );
}

#[test]
fn a_stored_record_naming_no_path_is_rejected() {
    let mut wire = producing_record().into_wire();
    wire.inputs[0].path = "".into();
    assert_eq!(UnitRecord::from_wire(wire).expect_err("no path"), WireError::EmptyPath);
}

/// The reason the wire carries `Arc<str>` rather than `String`: the binary
/// index builds one allocation per distinct path and clones pointers into
/// every record naming it. Decoding must not quietly undo that.
#[test]
fn decoding_preserves_a_shared_path_allocation() {
    let shared: std::sync::Arc<str> = std::sync::Arc::from("shared/header.h");
    let mut wire = producing_record().into_wire();
    wire.inputs = vec![
        FileRecordWire { path: shared.clone(), mtime: 1, hash: 0x11 },
        FileRecordWire { path: shared.clone(), mtime: 2, hash: 0x22 },
    ];

    let decoded = UnitRecord::from_wire(wire).expect("valid");

    assert_eq!(decoded.inputs().len(), 2);
    assert_eq!(decoded.inputs()[0].path(), "shared/header.h");
    // Same allocation, not merely equal text.
    assert_eq!(
        decoded.inputs()[0].path().as_ptr(),
        decoded.inputs()[1].path().as_ptr(),
        "decoding allocated a second copy of an interned path"
    );
}

/// A decoded record keeps the determinants it was STORED with. If `from_wire`
/// rebuilt them from a declaration, drift could never be observed — the record
/// would always agree with whatever it had just been rebuilt from.
#[test]
fn a_decoded_record_keeps_its_stored_determinants_so_drift_stays_visible() {
    let mut wire = producing_record().into_wire();
    wire.command_hash = 0x01d;

    let decoded = UnitRecord::from_wire(wire).expect("valid");

    assert_eq!(decoded.command_hash(), 0x01d);
    assert_eq!(
        determinant_drift(&decoded.determinants(), &Determinants::new(0xbeef, 0, 0x5ea1)),
        Some(DeterminantDrift::CommandHash)
    );
}

/// A declaration can change under a stored record. Agreement is asked again on
/// the way back, not assumed from the fact that it once held.
#[test]
fn a_stored_record_is_re_checked_against_the_declaration_it_comes_back_to() {
    let decoded = UnitRecord::from_wire(producing_record().into_wire()).expect("valid");

    assert!(decoded.agrees_with(&meta("k", &["out.o"])).is_ok());
    assert_eq!(
        decoded.agrees_with(&meta("k", &[])).expect_err("now declares no outputs"),
        RecordMismatch::ObservingUnitHasOutputs { found: 1 }
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
    assert_eq!(determinant_drift(&r.determinants(), &Determinants::new(0xbeef, 0, 0x5ea1)), None);
    assert_eq!(
        determinant_drift(&r.determinants(), &Determinants::new(0xffff, 0, 0x5ea1)),
        Some(DeterminantDrift::CommandHash)
    );
}
