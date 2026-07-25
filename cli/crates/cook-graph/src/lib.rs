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
pub const DAG_SCHEMA_VERSION: u32 = 4;

#[derive(Debug, thiserror::Error)]
pub enum ViewerError {
    #[error("failed to serialize DAG: {0}")]
    Serialize(String),
}

/// The graph inputs, independent of how the graph is then presented.
pub struct DagInputs<'a> {
    pub target: &'a str,
    pub all_units: &'a [(String, RecipeUnits)],
    pub explicit_edges: &'a BTreeMap<String, Vec<String>>,
    pub cache_managers: &'a BTreeMap<String, Arc<ThreadSafeCacheManager>>,
}

/// Build the unit-level graph. Every presentation path starts here.
pub fn build_dag(inputs: &DagInputs<'_>) -> DagData {
    dag_data::build_dag_data(
        inputs.target,
        inputs.all_units,
        inputs.explicit_edges,
        inputs.cache_managers,
    )
}
