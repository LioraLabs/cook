//! The declaration rules and the shared determinant rule.

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
        consumes: Vec::new(),
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

/// A declared terminal output may legitimately resolve to zero files on disk,
/// but the DECLARATION still names one, and it is the declaration this reads.
/// A unit is observing because its author declared no output, never because
/// the tree happens to hold none.
#[test]
fn a_glob_output_is_a_declaration_even_before_it_resolves() {
    assert_eq!(effect_kind(&meta("k", &["dist/**"])), EffectKind::Produced);
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
    for outputs in [&[][..], &["out.o"][..], &["dist/**"][..]] {
        let m = meta("k", outputs);
        let expected = match effect_kind(&m) {
            EffectKind::Observed => Cacheability::ResultOnly,
            EffectKind::Produced => Cacheability::Artifacts,
        };
        assert_eq!(cacheability(Some(&m)), expected);
    }
}

// -------------------------------------------------------------------------
// The rule every store shares
// -------------------------------------------------------------------------

fn stored() -> Determinants {
    Determinants::new(0xbeef, 0, 0x5ea1)
}

#[test]
fn unmoved_determinants_permit_a_replay() {
    assert_eq!(determinant_drift(&stored(), &Determinants::new(0xbeef, 0, 0x5ea1)), None);
}

#[test]
fn each_determinant_is_reported_in_order() {
    assert_eq!(
        determinant_drift(&stored(), &Determinants::new(0xffff, 0, 0x5ea1)),
        Some(DeterminantDrift::CommandHash)
    );
    assert_eq!(
        determinant_drift(&stored(), &Determinants::new(0xbeef, 0xffff, 0x5ea1)),
        Some(DeterminantDrift::Env)
    );
    assert_eq!(
        determinant_drift(&stored(), &Determinants::new(0xbeef, 0, 0xffff)),
        Some(DeterminantDrift::Seal)
    );
}

/// Command is reported ahead of env, and env ahead of seal, when more than one
/// has moved. The order is the reported CAUSE, so it is behaviour and not an
/// implementation detail.
#[test]
fn the_first_moved_determinant_wins_when_several_moved() {
    assert_eq!(
        determinant_drift(&stored(), &Determinants::new(0xffff, 0xffff, 0xffff)),
        Some(DeterminantDrift::CommandHash)
    );
    assert_eq!(
        determinant_drift(&stored(), &Determinants::new(0xbeef, 0xffff, 0xffff)),
        Some(DeterminantDrift::Env)
    );
}

/// The rule is indifferent to effect kind — that is the whole point. An
/// observing unit is invalidated by exactly what invalidates a producing one.
#[test]
fn an_observing_unit_obeys_the_same_determinant_rule() {
    let m = meta("k", &[]);
    assert_eq!(effect_kind(&m), EffectKind::Observed);
    let d = Determinants::new(m.command_hash, m.env_contribution, 0x5ea1);
    assert_eq!(determinant_drift(&d, &Determinants::new(0xbeef, 0, 0x5ea1)), None);
    assert_eq!(
        determinant_drift(&d, &Determinants::new(0xffff, 0, 0x5ea1)),
        Some(DeterminantDrift::CommandHash)
    );
}
