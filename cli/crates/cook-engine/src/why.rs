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
    /// A declared input is absent on disk: the unit cannot be a clean hit and no
    /// key is computed (mirrors `hash_input_paths` returning None). Rendered as
    /// `MISS (input '<path>' missing)`.
    MissingInput { path: String },
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

// ---------------------------------------------------------------------------
// Read-only `explain()` walk (COOK-165 Task 3)
// ---------------------------------------------------------------------------

use std::collections::BTreeSet;
use std::path::Path;
use std::sync::Arc;

use cook_cache::cache_ctx::CacheContext;
use cook_cache::ThreadSafeCacheManager;
use cook_contracts::WorkPayload;

use crate::{dag_builder, RegisteredWorkspace, WorkNode};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Disposition {
    Unannotated,
    Local,
    Pinned,
}

#[derive(Debug, Clone)]
pub struct WhyUnit {
    pub recipe_name: String,
    pub cache_key: String,
    pub key_hex: String,
    pub disposition: Disposition,
    pub status: CacheStatus,
    pub determinants: UnitDeterminants,
    pub line: u32,
    /// On a shared miss: Some(diffs) if a producer manifest was found (empty ⇒
    /// determinants identical to ours), None if no manifest exists for K.
    pub manifest_diff: Option<Vec<DeterminantDiff>>,
}

#[derive(Debug, Clone)]
pub struct WhyReport {
    pub recipe: String,
    pub units: Vec<WhyUnit>,
}

/// Build a read-only determinant explanation for `target`'s reachable closure.
/// Executes nothing. `probes_dir` is `<workspace>/.cook/probes`; sealed probe
/// values are read through from there (materialised on a prior run).
#[allow(clippy::too_many_arguments)]
pub fn explain(
    target: &str,
    registered_workspace: &RegisteredWorkspace,
    edges: &BTreeMap<String, Vec<String>>,
    reachable: &BTreeSet<String>,
    cache_ctx: &CacheContext,
    cache_managers: &BTreeMap<String, Arc<ThreadSafeCacheManager>>,
    probes_dir: &Path,
) -> Result<WhyReport, crate::EngineError> {
    let topo = crate::run::toposort_reachable_pub(edges, reachable)?;
    let mut all_units = Vec::with_capacity(topo.len());
    for name in &topo {
        let units = registered_workspace
            .units_by_recipe
            .get(name)
            .ok_or_else(|| crate::EngineError::UnknownRecipe(name.clone()))?;
        let mut u = units.clone();
        if let Some(deps) = edges.get(name) {
            u.deps = deps.clone();
        }
        all_units.push(u);
    }
    let dag = dag_builder::build_dag(all_units)?;

    let probe_store = cook_luaotp::ProbeValueStore::new();
    if probes_dir.exists() {
        probe_store.attach_dir(probes_dir.to_path_buf());
    }

    let mut units = Vec::new();
    for idx in 0..dag.len() {
        let node = dag.node(idx).payload();
        let Some(meta) = &node.cache_meta else {
            continue;
        };
        if meta.output_paths.is_empty() {
            continue;
        }
        let det = resolve_unit_determinants(node, meta, &probe_store);
        let key_hex = unit_key_hex(meta, &det);
        let (status, manifest_diff) =
            classify(node, meta, cache_ctx, cache_managers, &det, &key_hex);
        let disposition = match meta.sharing {
            cook_contracts::Sharing::Local => Disposition::Local,
            cook_contracts::Sharing::Pinned => Disposition::Pinned,
            cook_contracts::Sharing::Shared => Disposition::Unannotated,
        };
        units.push(WhyUnit {
            recipe_name: meta.recipe_name.clone(),
            cache_key: meta.cache_key.clone(),
            key_hex,
            disposition,
            status,
            determinants: det,
            line: node_line(node),
            manifest_diff,
        });
    }
    Ok(WhyReport {
        recipe: target.to_string(),
        units,
    })
}

fn node_line(node: &WorkNode) -> u32 {
    match &node.payload {
        Some(WorkPayload::Shell { line, .. }) => *line as u32,
        Some(WorkPayload::Test { line, .. }) => *line as u32,
        Some(WorkPayload::Probe { line, .. }) => *line as u32,
        _ => 0,
    }
}

pub(crate) fn resolve_unit_determinants(
    node: &WorkNode,
    meta: &cook_contracts::CacheMeta,
    probe_store: &cook_luaotp::ProbeValueStore,
) -> UnitDeterminants {
    let mut inputs = BTreeMap::new();
    for p in &meta.input_paths {
        let h = cook_fingerprint::hash_file(&node.working_dir.join(p)).unwrap_or(0);
        inputs.insert(p.clone(), h);
    }
    let seal_contribution = crate::seal::seal_contribution(&meta.seal_keys, probe_store);
    // C2: share the producer's sealed-probe resolution (absent → empty string)
    // so a shared-miss diff doesn't falsely report a probe difference against a
    // manifest that persisted the empty-string encoding.
    let sealed_probes = crate::seal::resolve_sealed_probes(&meta.seal_keys, probe_store);
    UnitDeterminants {
        command_hash: meta.command_hash,
        env_contribution: meta.env_contribution,
        seal_contribution,
        inputs,
        output_paths: meta.output_paths.clone(),
        consulted_env: meta.consulted_env.clone(),
        sealed_probes,
    }
}

fn unit_key_hex(meta: &cook_contracts::CacheMeta, det: &UnitDeterminants) -> String {
    let mut sorted: Vec<u64> = det.inputs.values().copied().collect();
    sorted.sort();
    let recipe_namespace = cook_fingerprint::recipe_namespace(
        &meta.project_id,
        &meta.cookfile_path,
        &meta.recipe_name,
    );
    let k = cook_fingerprint::cloud_key(&cook_fingerprint::CloudKeyInputs {
        schema_version: crate::executor::cache_version(),
        recipe_namespace: &recipe_namespace,
        command_hash: det.command_hash,
        env_contribution: det.env_contribution,
        seal_contribution: det.seal_contribution,
        sorted_input_content_hashes: &sorted,
    });
    hex::encode(k)
}

#[allow(clippy::too_many_arguments)]
fn classify(
    node: &WorkNode,
    meta: &cook_contracts::CacheMeta,
    cache_ctx: &CacheContext,
    cache_managers: &BTreeMap<String, Arc<ThreadSafeCacheManager>>,
    det: &UnitDeterminants,
    key_hex: &str,
) -> (CacheStatus, Option<Vec<DeterminantDiff>>) {
    // I1: a declared input absent on disk means the unit cannot be a clean hit
    // and no real key exists (mirrors `hash_input_paths` returning None at
    // executor.rs:710). Attribute the miss to that input rather than fabricating
    // a `0`-hash key that can never match a real manifest.
    if let Some(p) = first_missing_input(meta, &node.working_dir) {
        return (CacheStatus::MissingInput { path: p }, None);
    }
    if local_step_hit(node, meta, det, cache_managers) {
        return (CacheStatus::LocalHit, None);
    }
    if meta.sharing.is_local() {
        return (CacheStatus::LocalOnlyMiss, None);
    }
    // C1: read-only shared-store probe — recompute artifact keys and confirm the
    // backend holds every output, but NEVER write to the working tree (unlike
    // `fetch_by_key`, which restores). `cook why` is strictly read-only.
    if shared_artifacts_present(cache_ctx, key_hex, meta) {
        return (CacheStatus::SharedHit, None);
    }
    if meta.sharing.is_pinned() {
        return (
            CacheStatus::PinnedColdMiss,
            manifest_diff(cache_ctx, key_hex, det),
        );
    }
    (CacheStatus::SharedMiss, manifest_diff(cache_ctx, key_hex, det))
}

fn first_missing_input(
    meta: &cook_contracts::CacheMeta,
    working_dir: &Path,
) -> Option<String> {
    meta.input_paths
        .iter()
        .find(|p| cook_fingerprint::hash_file(&working_dir.join(p)).is_none())
        .cloned()
}

/// Read-only shared-store probe: recompute the artifact keys and check the
/// backend has every output, draining each reader for integrity verification
/// (CS-0054) but NEVER writing to the working tree. `cook why` is read-only.
fn shared_artifacts_present(
    cache_ctx: &CacheContext,
    key_hex: &str,
    meta: &cook_contracts::CacheMeta,
) -> bool {
    let Some(cloud_k) = decode_key_hex(key_hex) else {
        return false;
    };
    if meta.output_paths.is_empty() {
        return false;
    }
    for (idx, path) in meta.output_paths.iter().enumerate() {
        let artifact_k = cook_fingerprint::artifact_key(&cloud_k, idx as u32, path);
        match cache_ctx.backend.get(&artifact_k) {
            Ok(Some(mut reader)) => {
                // Drain to trigger streaming verify-on-restore; discard bytes.
                let mut sink = std::io::sink();
                if std::io::copy(&mut reader, &mut sink).is_err() {
                    return false;
                }
            }
            _ => return false,
        }
    }
    true
}

/// Decode a 64-char lowercase-hex string into a 32-byte cloud key. Returns None
/// on any length or non-hex error. C2: uses `hex::decode` (no hand-rolled hex).
fn decode_key_hex(key_hex: &str) -> Option<[u8; 32]> {
    hex::decode(key_hex).ok()?.try_into().ok()
}

fn local_step_hit(
    node: &WorkNode,
    meta: &cook_contracts::CacheMeta,
    det: &UnitDeterminants,
    cache_managers: &BTreeMap<String, Arc<ThreadSafeCacheManager>>,
) -> bool {
    let Some(cm) = cache_managers.get(&node.recipe_name) else {
        return false;
    };
    let cache = cm.get_or_load(&meta.recipe_name);
    let Some(entry) = cache.steps.get(&meta.cache_key) else {
        return false;
    };
    let input_refs: Vec<&str> = meta.input_paths.iter().map(|s| s.as_str()).collect();
    // I2: for glob outputs the raw pattern strings don't exist on disk; passing
    // them to needs_rebuild_cook would trip OutputMissing → spurious miss. Mirror
    // check_node_cache (executor.rs:654-664) by substituting the StepEntry's
    // recorded concrete output paths when any declared output is a glob.
    let any_glob = meta.output_paths.iter().any(|s| cook_fingerprint::is_terminal_output(s));
    let current_outputs_storage: Vec<String> = if any_glob {
        entry.outputs.iter().map(|f| f.path.clone()).collect()
    } else {
        meta.output_paths.clone()
    };
    let outs: Vec<&str> = current_outputs_storage.iter().map(|s| s.as_str()).collect();
    let (result, _updated) = cook_fingerprint::needs_rebuild_cook(
        Some(entry),
        &input_refs,
        &outs,
        det.command_hash,
        det.env_contribution,
        det.seal_contribution,
        &node.working_dir,
        None,
        meta.discovered_inputs.as_ref(),
        meta.record,
    );
    matches!(result, cook_fingerprint::RebuildResult::Skip)
}

fn manifest_diff(
    cache_ctx: &CacheContext,
    key_hex: &str,
    det: &UnitDeterminants,
) -> Option<Vec<DeterminantDiff>> {
    let k = decode_key_hex(key_hex)?;
    let manifest = cache_ctx.backend.get_manifest(&k).ok().flatten()?;
    Some(diff_against_manifest(det, &manifest))
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
