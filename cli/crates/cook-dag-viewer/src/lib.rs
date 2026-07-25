//! Terminal UI viewer for the Cook build DAG.

use std::collections::BTreeMap;
use std::sync::Arc;

use cook_cache::ThreadSafeCacheManager;
use cook_contracts::RecipeUnits;

// Always available: the graph model, the aggregation, and the
// text/mermaid/dot/json renderers. None of these carry a terminal
// dependency, so `cook dag` works in a default build.
pub mod dag_data;
pub mod emit;
pub mod frame;

// The ratatui terminal browser, behind `tui`.
#[cfg(feature = "tui")]
pub mod input;
#[cfg(feature = "tui")]
pub mod render;
#[cfg(feature = "tui")]
pub mod state;
#[cfg(feature = "tui")]
pub mod theme;
#[cfg(feature = "tui")]
pub mod tui;
// Viewer-local copy of the legacy `wave_grouper` module that cook-engine
// shipped before SHI-222 Phase 4. The engine no longer waves at runtime;
// the viewer groups recipes into waves purely for display.
mod wave_grouper;

pub use dag_data::{build_wave_dag_data, EdgeData, EdgeKind, NodeData, WaveData, WaveDagData};
pub use frame::{FrameEvent, NodeStatus, SnapshotFrame, ViewFrame};

/// Wire-format schema version for the DAG-viewer JSON payload (CS-0048).
pub const VIEWER_SCHEMA_VERSION: u32 = 2;

#[derive(Debug, thiserror::Error)]
pub enum ViewerError {
    #[error("failed to start DAG viewer terminal: {0}")]
    TerminalInit(String),
    #[error("failed to serialize DAG: {0}")]
    Serialize(String),
    #[error("layout failure: {0}")]
    Layout(String),
}

/// The graph inputs, independent of how the graph is then presented.
pub struct DagInputs<'a> {
    pub target: &'a str,
    pub all_units: &'a [(String, RecipeUnits)],
    pub explicit_edges: &'a BTreeMap<String, Vec<String>>,
    pub inferred_deps: &'a BTreeMap<String, Vec<String>>,
    pub cache_managers: &'a BTreeMap<String, Arc<ThreadSafeCacheManager>>,
}

/// Build the unit-level graph. Every presentation path starts here.
pub fn build_dag(inputs: &DagInputs<'_>) -> WaveDagData {
    dag_data::build_wave_dag_data(
        inputs.target,
        inputs.all_units,
        inputs.explicit_edges,
        inputs.inferred_deps,
        inputs.cache_managers,
    )
}

/// Launch the ratatui browser over the graph.
#[cfg(feature = "tui")]
pub fn run_tui(inputs: &DagInputs<'_>, theme: theme::Theme) -> Result<(), ViewerError> {
    tui::run_with_theme(SnapshotFrame::new(build_dag(inputs)), theme)
}
