//! The Cook build graph: model, aggregation, and renderers for `cook dag`.
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

pub mod dag_data;
pub mod emit;

pub use dag_data::{build_dag_data, DagData, EdgeData, EdgeKind, NodeData};

/// Wire-format schema version for the DAG payload (CS-0048).
///
/// 3 since the wave structure was removed: `{waves, inter_wave_edges}` became
/// `{recipes, nodes, edges}`, which is an incompatible structural change and
/// so requires a bump under CS-0048's evolution policy.
pub const DAG_SCHEMA_VERSION: u32 = 3;

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
