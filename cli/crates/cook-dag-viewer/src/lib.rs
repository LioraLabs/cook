//! Browser-based DAG viewer for the Cook build system.
//!
//! Serves a small embedded HTML/JS viewer over a local HTTP server and feeds it
//! a JSON snapshot of the wave-grouped recipe DAG. Consumers (the `cook` CLI)
//! prepare the recipe units, edge maps, and cache managers; this crate handles
//! data shaping, JSON serialization, and the local viewer server.
//!
//! See `standard/src/content/docs/appendix/D-changes.mdx` (CS-0047) for the
//! split-out and feature-gate rationale, and CS-0048 for the
//! `schema_version` field carried on the JSON payload.

/// Wire-format schema version for the DAG-viewer JSON payload (CS-0048).
///
/// Writers emit `schema_version = VIEWER_SCHEMA_VERSION`; the embedded JS
/// viewer refuses payloads whose `schema_version` exceeds the highest
/// version it recognises (today, 1). Evolution is additive-only — new
/// fields are introduced without bumping the version; an incompatible
/// structural change bumps `VIEWER_SCHEMA_VERSION` (and the `if` guard
/// in `dag_viewer.html`).
pub const VIEWER_SCHEMA_VERSION: u32 = 1;

mod dag_data;
mod dag_server;

use std::collections::BTreeMap;
use std::sync::Arc;

use cook_cache::ThreadSafeCacheManager;
use cook_contracts::RecipeUnits;

pub use dag_data::{build_wave_dag_data, EdgeData, NodeData, WaveData, WaveDagData};

/// Errors raised by the viewer entry point.
#[derive(Debug, thiserror::Error)]
pub enum ViewerError {
    #[error("failed to start DAG viewer server: {0}")]
    ServerStart(String),
    #[error("failed to serialize DAG: {0}")]
    Serialize(String),
}

/// Inputs the viewer needs to render a DAG. Callers (the `cook` CLI) prepare
/// these by running the same register-only walk that `cook_engine::run::run`
/// would otherwise execute.
pub struct DagViewerInputs<'a> {
    pub target: &'a str,
    pub all_units: &'a [(String, RecipeUnits)],
    pub explicit_edges: &'a BTreeMap<String, Vec<String>>,
    pub inferred_deps: &'a BTreeMap<String, Vec<String>>,
    pub cache_managers: &'a BTreeMap<String, Arc<ThreadSafeCacheManager>>,
}

/// Build the wave-grouped DAG JSON for `inputs`, then start a local HTTP
/// server that serves the embedded viewer with that JSON spliced in.
///
/// Blocks until the server is killed (Ctrl+C). Returns `Ok(())` on a clean
/// shutdown and a `ViewerError` if the server cannot be started or the JSON
/// cannot be produced.
pub fn cmd_dag(inputs: &DagViewerInputs<'_>) -> Result<(), ViewerError> {
    let dag_data = dag_data::build_wave_dag_data(
        inputs.target,
        inputs.all_units,
        inputs.explicit_edges,
        inputs.inferred_deps,
        inputs.cache_managers,
    );

    let json =
        serde_json::to_string(&dag_data).map_err(|e| ViewerError::Serialize(e.to_string()))?;

    dag_server::serve_dag(&json)
}
