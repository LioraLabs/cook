//! Terminal UI viewer for the Cook build DAG.
//!
//! See `docs/superpowers/specs/2026-05-05-dag-tui-viewer-design.md`.

use std::collections::BTreeMap;
use std::sync::Arc;

use cook_cache::ThreadSafeCacheManager;
use cook_contracts::RecipeUnits;

pub mod dag_data;
pub mod frame;
pub mod input;
pub mod render;
pub mod state;
pub mod theme;
pub mod tui;
// Viewer-local copy of the legacy `wave_grouper` module that cook-engine
// shipped before SHI-222 Phase 4. The engine no longer waves at runtime;
// the viewer groups recipes into waves purely for display.
mod wave_grouper;

pub use dag_data::{build_wave_dag_data, EdgeData, NodeData, WaveData, WaveDagData};
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

pub struct DagViewerInputs<'a> {
    pub target: &'a str,
    pub all_units: &'a [(String, RecipeUnits)],
    pub explicit_edges: &'a BTreeMap<String, Vec<String>>,
    pub inferred_deps: &'a BTreeMap<String, Vec<String>>,
    pub cache_managers: &'a BTreeMap<String, Arc<ThreadSafeCacheManager>>,
    pub theme: theme::Theme,
}

/// Entry point. Builds the snapshot and launches the TUI.
pub fn cmd_dag(inputs: &DagViewerInputs<'_>) -> Result<(), ViewerError> {
    let dag = dag_data::build_wave_dag_data(
        inputs.target,
        inputs.all_units,
        inputs.explicit_edges,
        inputs.inferred_deps,
        inputs.cache_managers,
    );
    tui::run_with_theme(SnapshotFrame::new(dag), inputs.theme)
}
