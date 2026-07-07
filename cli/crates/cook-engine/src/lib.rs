//! cook-engine — build pipeline orchestrator for the Cook build system.
//!
//! Drives the recipe DAG, calls cook-register per wave, evaluates cache,
//! builds work-unit DAGs, and feeds ready nodes to cook-luaotp.

pub mod affected;
pub mod analyzer;
pub mod dag_builder;
pub mod executor;
pub mod id;
pub mod pipeline;
pub mod recipe_dag;
pub mod reconcile;
pub mod registered_workspace;
pub mod run;
mod seal;
pub mod verify;
pub mod why;

pub use registered_workspace::RegisteredWorkspace;
pub use run::{
    build_cache_ctx_for_cli, cache_managers_for_cli, run, RunResult, TestScope,
};
// `cook why` types are consumed through the `cook_engine::why::` module path
// (CLI renderers, E2E); no flat re-export — one public name per type.

// Re-export the registration-phase public types so consumers can build a
// `RegisteredWorkspace` without taking a direct `cook-register` dependency.
//
// Note: `cook_register::RecipeKind` is intentionally NOT re-exported at the
// engine root — it would collide with the engine's own `RecipeKind` (the
// progress-event mirror enum a few lines below). Consumers that need the
// registration-phase kind reach it through the re-exported `cook_register`
// module path: `cook_engine::cook_register::RecipeKind`.
pub use cook_register::{RegisteredCookfile, RegisteredRecipePub};

// Re-export the `cook_register` crate as a module so consumers (notably
// `cook-cli`) can name `cook_engine::cook_register::RecipeKind` and other
// registration-phase types without taking a direct `cook-register`
// dependency. The collision-avoidance rule above still applies to the
// engine's own root namespace.
pub use cook_register;

// Re-export `cook_contracts` and `cook_cache` as modules so consumers
// (notably `cook-cli`) can name `cook_engine::cook_contracts::RecipeUnits`
// and `cook_engine::cook_cache::ThreadSafeCacheManager` without taking
// direct dependencies on those crates. The Cargo.toml comment in cook-cli
// explicitly calls out that contracts/cache/etc. are reached transitively
// through the engine's public API; these re-exports honour that contract.
pub use cook_cache;
pub use cook_contracts;

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Duration;

use thiserror::Error;

use cook_contracts::{CacheMeta, OutputStream, WorkPayload};

// Re-export OutputStream so consumers using EngineEvent::OutputLine don't
// need a direct cook-contracts dependency just to name the stream variant.
pub use cook_contracts::OutputStream as Stream;

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
// NodeKind — engine-side mirror of cook_progress::NodeKind
// ---------------------------------------------------------------------------

/// Kind of work a node is doing — engine-side enum, isomorphic to
/// `cook_progress::NodeKind`. The CLI translates between the two so that
/// `cook-engine` does not depend on `cook-progress` (the renderer is one
/// of several possible event consumers).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum NodeKind {
    Compile,
    Link,
    Resolve,
    Generate,
    Write,
    Test,
    #[default]
    Cooked,
}

// ---------------------------------------------------------------------------
// RecipeKind — engine-side mirror of cook_progress::event::RecipeKind
// ---------------------------------------------------------------------------

/// Whether a completed recipe was a normal recipe or a chore — engine-side
/// enum, isomorphic to `cook_progress::event::RecipeKind`. The CLI
/// translates between the two so that `cook-engine` does not depend on
/// `cook-progress`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RecipeKind {
    #[default]
    Recipe,
    Chore,
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
        kind: RecipeKind,
    },
    /// A recipe did not complete because one or more nodes were skipped.
    RecipeSkipped {
        name: String,
        elapsed: Duration,
        skipped_nodes: usize,
        completed_nodes: usize,
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
        /// What sort of work this node is doing. Drives the verb the renderer
        /// prints on completion (`Tested`, `Compiled`, `Cooked`, …). Defaults
        /// to `Cooked` for shell/cook/lua steps; test-step nodes emit `Test`.
        kind: NodeKind,
    },
    /// A work node completed successfully.
    NodeCompleted {
        recipe: String,
        node_name: String,
        elapsed: Duration,
        /// Mirrors the `kind` from the matching `NodeStarted` so the renderer
        /// can pick the right verb without remembering per-node state.
        kind: NodeKind,
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
        /// Number of body steps in this chore window. 0 for non-chore
        /// (legacy single-line) interactives.
        chore_step_count: usize,
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
        /// 1-indexed step number that failed inside the chore window;
        /// `None` on success or for non-chore interactives.
        failed_step: Option<usize>,
    },
    /// A line of output from a work node.
    ///
    /// `stream` distinguishes stdout vs stderr (CS-0035).  Pre-CS-0035 this
    /// variant carried `is_stderr: bool` that was hardcoded to `false` at the
    /// call sites in `executor.rs`, so the wire-format `stream` field in
    /// `events.jsonl` was always `"stdout"` — a lie about the `Stream::Stderr`
    /// variant that was never reachable.
    OutputLine {
        recipe: String,
        line: String,
        stream: OutputStream,
    },
    /// The entire engine run has finished.
    Finished {
        elapsed: Duration,
        success: bool,
    },
    /// A test unit has started executing.
    TestStarted {
        id: TestId,
        recipe: String,
        name: String,
        line: u32,
        iteration_item: Option<String>,
    },
    /// A test unit passed.
    TestPassed {
        id: TestId,
        duration: std::time::Duration,
        cached: bool,
        should_fail: bool,
        stdout: String,
        stderr: String,
        line: u32,
    },
    /// A test unit failed.
    TestFailed {
        id: TestId,
        duration: std::time::Duration,
        stdout: String,
        stderr: String,
        reason: TestFailureReason,
        line: u32,
    },
    /// A test unit was blocked because an upstream cook step failed.
    TestBlocked {
        id: TestId,
        upstream: String,
        line: u32,
    },
    /// A test unit timed out.
    TestTimedOut {
        id: TestId,
        timeout: std::time::Duration,
        stdout: String,
        stderr: String,
        line: u32,
    },
}

// ---------------------------------------------------------------------------
// TestId, TestOutcome, TestFailureReason, TestResult
// ---------------------------------------------------------------------------

/// Stable identity for one test unit. Format: `<namespace>.<recipe>:<name>[<discriminator>]`.
/// The discriminator is empty for one-shot tests; for iteration modes it is the
/// iteration item (typically the input filename's basename).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Ord, PartialOrd)]
pub struct TestId(pub String);

impl std::fmt::Display for TestId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Outcome of one test unit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TestOutcome {
    Passed,
    Failed,
    Blocked,
    TimedOut,
}

/// Reason for a test failure (when outcome is Failed).
#[derive(Debug, Clone)]
pub enum TestFailureReason {
    ExitStatusMismatch {
        expected_success: bool,
        observed_success: bool,
        exit_code: Option<i32>,
    },
    SignalKilled(i32),
    SpawnError(String),
}

/// One row in `RunResult.test_results`.
#[derive(Debug, Clone)]
pub struct TestResult {
    pub id: TestId,
    pub namespace: String,
    pub recipe: String,
    pub name: String,
    pub suite: String,
    pub iteration_item: Option<String>,
    pub outcome: TestOutcome,
    pub duration: std::time::Duration,
    pub from_cache: bool,
    pub stdout: String,
    pub stderr: String,
    pub fingerprint: Option<String>,
    pub blocked_by: Option<String>,
    pub should_fail: bool,
    pub timed_out: bool,
    pub line: u32,
    pub exit_code: Option<i32>,
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
        /// Test results accumulated before the failure (includes Blocked rows for
        /// any test nodes that were cancelled as a consequence of cook-step failures).
        /// `run_for_test_inner` extracts these so it can return Ok instead of Err.
        partial_test_results: Vec<TestResult>,
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

    /// §22.1.2 terminal-output rule violation: a downstream recipe declares
    /// a literal `inputs[]` path that is matched by an upstream recipe's
    /// glob `outputs[]` pattern. Detected at register time (syntactic check,
    /// no filesystem access). The author should either narrow the upstream
    /// outputs[] to specific files or use a `requires` edge.
    #[error("cross-recipe globbed-output edge: recipe '{downstream}' reads input '{input}' which matches recipe '{upstream}' output pattern '{pattern}'")]
    GlobbedOutputCrossRecipeEdge {
        upstream: String,
        downstream: String,
        input: String,
        pattern: String,
    },
}

// Map `cook_register::RegisterError` onto `EngineError` so callers that
// drive the register-phase via this crate can propagate failures with `?`
// without reaching into `cook-register` directly.
//
// `RecipeCollision` is mapped onto `RegistrationFailed` here; the CLI lifts
// it to a structured `CookError::RecipeCollision` in Phase 5 Task 5.6.
impl From<cook_register::RegisterError> for EngineError {
    fn from(e: cook_register::RegisterError) -> Self {
        match e {
            cook_register::RegisterError::DependencyCycle { recipes } => {
                EngineError::CycleDetected(format!(
                    "recipe cycle: {}",
                    recipes.join(" -> ")
                ))
            }
            cook_register::RegisterError::RecipeCollision { name, sites } => {
                let sites_str = sites
                    .iter()
                    .map(|s| {
                        let kind = match s.kind {
                            cook_register::RegistrationSiteKind::SurfaceRecipe => {
                                "surface recipe"
                            }
                            cook_register::RegistrationSiteKind::SurfaceChore => {
                                "surface chore"
                            }
                            cook_register::RegistrationSiteKind::Dynamic => {
                                "cook.recipe call"
                            }
                        };
                        format!("{} at line {}", kind, s.line)
                    })
                    .collect::<Vec<_>>()
                    .join(", ");
                EngineError::RegistrationFailed {
                    recipe: name.clone(),
                    message: format!(
                        "recipe '{name}' is registered more than once: {sites_str}"
                    ),
                }
            }
            cook_register::RegisterError::Lua(le) => EngineError::RegistrationFailed {
                recipe: String::new(),
                message: le.to_string(),
            },
            cook_register::RegisterError::CommandFailed {
                command,
                line,
                code,
            } => EngineError::RegistrationFailed {
                recipe: String::new(),
                message: format!(
                    "Cookfile:{line}: command failed (exit {code}): {command}"
                ),
            },
            cook_register::RegisterError::RecipeNotFound(name) => {
                EngineError::UnknownRecipe(name)
            }
            // COOK-36 Task 4: argv-binding diagnostics surface as
            // RegistrationFailed so the CLI can render the message and exit.
            // The variants below carry the user-visible string in their
            // `#[error(...)]` attribute (`cook-register/src/lib.rs`); pull
            // it via `to_string()` so the wording lives in one place.
            ref e @ cook_register::RegisterError::ChoreParamMissing { ref chore, .. } => {
                EngineError::RegistrationFailed {
                    recipe: chore.clone(),
                    message: e.to_string(),
                }
            }
            ref e @ cook_register::RegisterError::ChoreTooManyArgv {
                ref chore,
                declared,
                supplied,
                ref first_unmatched,
            } => {
                let mut msg = e.to_string();
                if declared == 0 && supplied == 1 && !first_unmatched.is_empty()
                    && first_unmatched.chars().all(|c| {
                        c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.'
                    })
                {
                    msg.push_str(&format!(
                        ". Did you mean a config preset? Use 'cook {chore} @{first_unmatched}' \
                         or 'cook {chore} --config {first_unmatched}'."
                    ));
                }
                EngineError::RegistrationFailed {
                    recipe: chore.clone(),
                    message: msg,
                }
            }
            ref e @ cook_register::RegisterError::RecipeWithArgv { ref name, .. } => {
                EngineError::RegistrationFailed {
                    recipe: name.clone(),
                    message: e.to_string(),
                }
            }
            ref e @ cook_register::RegisterError::ChoreVariadicEmpty { ref chore, .. } => {
                EngineError::RegistrationFailed {
                    recipe: chore.clone(),
                    message: e.to_string(),
                }
            }
            ref e @ cook_register::RegisterError::ChoreParamDefaultLuaError {
                ref chore, ..
            } => {
                EngineError::RegistrationFailed {
                    recipe: chore.clone(),
                    message: e.to_string(),
                }
            }
            ref e @ cook_register::RegisterError::ChoreParamDefaultLuaNonString {
                ref chore, ..
            } => {
                EngineError::RegistrationFailed {
                    recipe: chore.clone(),
                    message: e.to_string(),
                }
            }
            // COOK-64 §22.5.9 — `for_each` pre-pass diagnostics.
            ref e @ cook_register::RegisterError::ForEachProbeUndeclared {
                ref recipe, ..
            } => EngineError::RegistrationFailed {
                recipe: recipe.clone(),
                message: e.to_string(),
            },
            ref e @ (cook_register::RegisterError::ForEachProbeProduceFailed { .. }
            | cook_register::RegisterError::ForEachNotArray { .. }
            | cook_register::RegisterError::ForEachProbeArtifactDep { .. }) => {
                EngineError::RegistrationFailed {
                    recipe: String::new(),
                    message: e.to_string(),
                }
            }
        }
    }
}

#[cfg(test)]
mod test_result_tests {
    use super::*;
    #[test]
    fn test_result_carries_line() {
        let r = TestResult {
            id: TestId("r:t".into()),
            namespace: String::new(),
            recipe: "r".into(),
            name: "t".into(),
            suite: String::new(),
            iteration_item: None,
            outcome: TestOutcome::Passed,
            duration: std::time::Duration::ZERO,
            from_cache: false,
            stdout: String::new(),
            stderr: String::new(),
            fingerprint: None,
            blocked_by: None,
            should_fail: false,
            timed_out: false,
            line: 42,
            exit_code: None,
        };
        assert_eq!(r.line, 42);
    }

    #[test]
    fn test_started_event_carries_line() {
        let evt = EngineEvent::TestStarted {
            id: TestId("r:t".into()),
            recipe: "r".into(),
            name: "t".into(),
            line: 7,
            iteration_item: None,
        };
        if let EngineEvent::TestStarted { line, .. } = evt {
            assert_eq!(line, 7);
        } else {
            panic!("wrong variant");
        }
    }
}
