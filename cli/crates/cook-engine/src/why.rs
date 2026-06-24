//! Read-only `cook why` determinant explanation (COOK-165, §17.1.6).
//!
//! Builds, per cacheable unit, the COMPLETE attributed cache key K and a
//! hit/miss classification; on a shared miss it diffs the consumer's resolved
//! determinants against the producer determinant manifest (COOK-166) fetched by
//! K, naming the differing determinant(s). Executes nothing.

use std::collections::BTreeMap;

use cook_fingerprint::backend::DeterminantManifest;

/// How a unit's cache lookup resolved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CacheStatus {
    LocalHit,
    SharedHit,
    SharedMiss,
    LocalOnlyMiss,
    PinnedColdMiss,
}

/// One determinant difference found when diffing consumer determinants against a
/// producer manifest on a shared miss.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeterminantDiff {
    CommandHash { ours: u64, theirs: u64 },
    EnvContribution { ours: u64, theirs: u64 },
    SealContribution { ours: u64, theirs: u64 },
    Input { path: String, ours: Option<u64>, theirs: Option<u64> },
    Env { key: String, ours: Option<String>, theirs: Option<String> },
    Probe { key: String, ours: Option<String>, theirs: Option<String> },
    OutputPaths { ours: Vec<String>, theirs: Vec<String> },
}

/// The consumer-side resolved determinants for one unit (the data behind K).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnitDeterminants {
    pub command_hash: u64,
    pub env_contribution: u64,
    pub seal_contribution: u64,
    pub inputs: BTreeMap<String, u64>,
    pub output_paths: Vec<String>,
    pub consulted_env: BTreeMap<String, String>,
    pub sealed_probes: BTreeMap<String, String>,
}

/// Diff the consumer determinants against a producer manifest, in a stable order
/// (the variant order above, keys sorted within).
pub fn diff_against_manifest(
    ours: &UnitDeterminants,
    theirs: &DeterminantManifest,
) -> Vec<DeterminantDiff> {
    let mut out = Vec::new();
    if ours.command_hash != theirs.command_hash {
        out.push(DeterminantDiff::CommandHash { ours: ours.command_hash, theirs: theirs.command_hash });
    }
    if ours.env_contribution != theirs.env_contribution {
        out.push(DeterminantDiff::EnvContribution { ours: ours.env_contribution, theirs: theirs.env_contribution });
    }
    if ours.seal_contribution != theirs.seal_contribution {
        out.push(DeterminantDiff::SealContribution { ours: ours.seal_contribution, theirs: theirs.seal_contribution });
    }
    diff_map_u64(&ours.inputs, &theirs.inputs, |path, o, t| {
        out.push(DeterminantDiff::Input { path, ours: o, theirs: t });
    });
    diff_map_str(&ours.consulted_env, &theirs.consulted_env, |key, o, t| {
        out.push(DeterminantDiff::Env { key, ours: o, theirs: t });
    });
    diff_map_str(&ours.sealed_probes, &theirs.sealed_probes, |key, o, t| {
        out.push(DeterminantDiff::Probe { key, ours: o, theirs: t });
    });
    if ours.output_paths != theirs.output_paths {
        out.push(DeterminantDiff::OutputPaths {
            ours: ours.output_paths.clone(),
            theirs: theirs.output_paths.clone(),
        });
    }
    out
}

fn diff_map_u64(
    ours: &BTreeMap<String, u64>,
    theirs: &BTreeMap<String, u64>,
    mut emit: impl FnMut(String, Option<u64>, Option<u64>),
) {
    let keys: std::collections::BTreeSet<&String> = ours.keys().chain(theirs.keys()).collect();
    for k in keys {
        let (o, t) = (ours.get(k).copied(), theirs.get(k).copied());
        if o != t { emit(k.clone(), o, t); }
    }
}

fn diff_map_str(
    ours: &BTreeMap<String, String>,
    theirs: &BTreeMap<String, String>,
    mut emit: impl FnMut(String, Option<String>, Option<String>),
) {
    let keys: std::collections::BTreeSet<&String> = ours.keys().chain(theirs.keys()).collect();
    for k in keys {
        let (o, t) = (ours.get(k).cloned(), theirs.get(k).cloned());
        if o != t { emit(k.clone(), o, t); }
    }
}

#[cfg(test)]
mod tests {
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
}
