//! cook-engine — build pipeline orchestrator for the Cook build system.
//!
//! Drives the recipe DAG, calls cook-register per wave, evaluates cache,
//! builds work-unit DAGs, and feeds ready nodes to cook-luaotp.

pub mod analyzer;
pub mod dag_builder;
pub mod executor;
pub mod recipe_dag;
pub mod registry_entry;
pub mod run;
pub mod wave_grouper;

pub use registry_entry::RegistryEntry;

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Duration;

use thiserror::Error;

use cook_contracts::{CacheMeta, WorkPayload};

// ---------------------------------------------------------------------------
// RecipeTopology — per-recipe topology snapshot for BuildStarted
// ---------------------------------------------------------------------------

/// Per-recipe topology snapshot, part of `EngineEvent::BuildStarted`.
#[derive(Debug, Clone)]
pub struct RecipeTopology {
    pub name: String,
    pub deps: Vec<String>,
    pub expected_nodes: usize,
}

// ---------------------------------------------------------------------------
// ExportStore — cross-recipe data sharing state owned by engine
// ---------------------------------------------------------------------------

/// Cross-recipe data sharing state owned by the engine.
///
/// Recipes export structured data (via `cook.export()`) that other recipes can
/// import. The engine owns this map and passes it into each registration call.
pub type ExportStore = BTreeMap<String, serde_json::Value>;

// ---------------------------------------------------------------------------
// WorkNode — bridge between generic Dag<T> and Cook-specific data
// ---------------------------------------------------------------------------

/// A node in the work DAG that bridges the generic `Dag<T>` with Cook-specific data.
///
/// When `payload` is `None` the node is pre-satisfied (cached) and will be
/// completed immediately without submitting work to the pool.
#[derive(Debug, Clone)]
pub struct WorkNode {
    /// The work to execute. `None` means pre-satisfied (cached).
    pub payload: Option<WorkPayload>,
    /// Name of the recipe this node belongs to.
    pub recipe_name: String,
    /// Cache metadata for recording completion.
    pub cache_meta: Option<CacheMeta>,
    /// Working directory for command execution.
    pub working_dir: PathBuf,
    /// Environment variables to set for the child process.
    pub env_vars: BTreeMap<String, String>,
}

// ---------------------------------------------------------------------------
// EngineEvent — progress / observability events emitted during execution
// ---------------------------------------------------------------------------

/// Events emitted by the engine during execution for progress reporting
/// and observability.
#[derive(Debug, Clone)]
pub enum EngineEvent {
    /// Complete DAG topology, emitted once before any recipe starts.
    BuildStarted {
        recipes: Vec<RecipeTopology>,
        total_nodes: usize,
    },
    /// A recipe has been queued for registration.
    RecipeQueued {
        name: String,
    },
    /// A recipe has started executing its work nodes.
    RecipeStarted {
        name: String,
        total_nodes: usize,
    },
    /// A recipe completed all its work nodes successfully.
    RecipeCompleted {
        name: String,
        elapsed: Duration,
        cached_nodes: usize,
        total_nodes: usize,
    },
    /// A recipe failed due to one or more node failures.
    RecipeFailed {
        name: String,
        elapsed: Duration,
        completed_nodes: usize,
        total_nodes: usize,
    },
    /// A work node has started executing.
    NodeStarted {
        recipe: String,
        node_name: String,
        artifact: Option<std::path::PathBuf>,
        fallback_label: String,
    },
    /// A work node completed successfully.
    NodeCompleted {
        recipe: String,
        node_name: String,
        elapsed: Duration,
    },
    /// A work node failed.
    NodeFailed {
        recipe: String,
        node_name: String,
        elapsed: Duration,
        error: String,
    },
    /// A work node was satisfied from cache (no execution needed).
    NodeCacheHit {
        recipe: String,
        node_name: String,
        artifact: Option<std::path::PathBuf>,
    },
    /// A work node was skipped because an upstream dependency failed.
    NodeSkipped {
        recipe: String,
        node_name: String,
    },
    /// An interactive command is about to run on the main thread.
    InteractiveStart {
        recipe: String,
        node_name: String,
    },
    /// An interactive command finished.
    InteractiveEnd {
        recipe: String,
        node_name: String,
        elapsed: Duration,
        success: bool,
        /// True when no further work will run after this node — the renderer
        /// uses this to leave progress bars frozen instead of resuming them.
        is_terminal: bool,
    },
    /// A line of output from a work node.
    OutputLine {
        recipe: String,
        line: String,
        is_stderr: bool,
    },
    /// The entire engine run has finished.
    Finished {
        elapsed: Duration,
        success: bool,
    },
}

// ---------------------------------------------------------------------------
// EngineError
// ---------------------------------------------------------------------------

/// Errors that can occur during engine execution.
#[derive(Error, Debug)]
pub enum EngineError {
    /// One or more work nodes failed during DAG execution.
    #[error("engine: {count} task(s) failed")]
    TaskFailures {
        count: usize,
        /// (node_id, recipe_name, error_message)
        failures: Vec<(usize, String, String)>,
    },

    /// Dependency resolution found a cycle.
    #[error("dependency cycle detected: {0}")]
    CycleDetected(String),

    /// A referenced recipe does not exist.
    #[error("unknown recipe: {0}")]
    UnknownRecipe(String),

    /// Registration (capture-mode Lua execution) failed.
    #[error("registration failed for recipe '{recipe}': {message}")]
    RegistrationFailed {
        recipe: String,
        message: String,
    },

    /// Cache I/O error.
    #[error("cache error: {0}")]
    CacheError(String),

    /// Two or more non-dep-related recipes declare the same canonical output
    /// path. Detected at plan time, before any work runs, to prevent silent
    /// races under `--jobs > 1`.
    #[error("output collision: {recipes:?} all declare output {path:?} with no dependency edge between them")]
    OutputCollision {
        path: PathBuf,
        recipes: Vec<String>,
    },
}
