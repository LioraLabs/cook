//! The Cook build graph: model, aggregation, and renderers for `cook why`.
//!
//! There is no terminal browser here any more, and no waves. The ratatui
//! viewer navigated by wave, and waves were a display construct the engine
//! stopped scheduling by at SHI-222 Phase 4 — so the browser was a navigation
//! model for a structure that did not exist. Both are gone; what remains is
//! the graph itself and four ways to print it.

use std::collections::BTreeMap;
use std::sync::Arc;

use cook_cache::ThreadSafeCacheManager;
use cook_contracts::RecipeUnits;

pub mod annotate;
pub mod dag_data;
pub mod emit;

pub use annotate::{Annotations, UnitFacts};
pub use dag_data::{build_dag_data, DagData, EdgeData, EdgeKind, NodeData};

/// Wire-format schema version for the `cook why` payload (CS-0048,
/// §{exec.cache.why.formats}).
///
/// 3 when the wave structure was removed: `{waves, inter_wave_edges}` became
/// `{recipes, nodes, edges}`.
///
/// 4 at CS-0171, when the graph payload and the determinant payload merged
/// into one document: a node's `cached` boolean became `hits`/`rebuilds`/
/// `unclassified` tallies plus `forces` and the timing pair, and the former
/// `cook why --json` `units` array joined the same object. Two incompatible
/// structural changes at once, and one bump covers both.
///
/// It versions EVERY machine-readable document the query emits, not only the
/// one assembled here: since CS-0217 the `--unit` selector's document carries
/// this same number. That document is this one at reduced scope — the same
/// `units` array, per-unit objects built by the same encoder — so a change
/// that breaks a consumer of one breaks a consumer of the other, and one
/// number is the only way to say so. The name is historical; the payload it
/// describes has not been only a DAG since 4.
///
/// 5 at CS-0216, when `observed_max_age` left the node object. The model that
/// fed it, an observation's position in a local build history, went at
/// CS-0189, after which the only producer hardcoded `0`; the key survived as a
/// permanent zero telling every reader that every contributing unit was timed
/// in the most recent build. Dropping a key is a structurally incompatible
/// change, which §17.1.6.6 says MUST bump this number.
pub const DAG_SCHEMA_VERSION: u32 = 5;

/// Stamp the wire-format version onto a machine-readable `cook why` document.
///
/// Both documents the query emits go through here (§{exec.cache.why.formats} /
/// CS-0217): this crate's whole-closure document, and `cook-cli`'s
/// selector-scoped one. The key and the number are one decision, so a caller
/// spelling `"schema_version"` for itself would be the second end of a wire
/// format with nothing holding the two ends together — which is the state
/// CS-0217 found, in the milder form of one end having no version at all.
pub fn stamp_schema_version(document: &mut serde_json::Value) {
    document["schema_version"] = DAG_SCHEMA_VERSION.into();
}

/// The graph inputs, independent of how the graph is then presented.
pub struct DagInputs<'a> {
    pub target: &'a str,
    pub all_units: &'a [(String, RecipeUnits)],
    pub explicit_edges: &'a BTreeMap<String, Vec<String>>,
    pub cache_managers: &'a BTreeMap<String, Arc<ThreadSafeCacheManager>>,
}

/// Build the unit-level graph. Every presentation path starts here.
///
/// Fallible since COOK-402: the structural edges come from the shared wiring
/// law (`cook_contracts::unit_graph::plan`), which can reject the closure
/// (dangling `dep_edges`, dependency cycles). On the `cook why` path these
/// were already screened by the engine's own plan of the same closure, so an
/// error here means the two calls were fed different inputs.
pub fn build_dag(
    inputs: &DagInputs<'_>,
) -> Result<DagData, cook_contracts::unit_graph::UnitGraphError> {
    dag_data::build_dag_data(
        inputs.target,
        inputs.all_units,
        inputs.explicit_edges,
        inputs.cache_managers,
    )
}
