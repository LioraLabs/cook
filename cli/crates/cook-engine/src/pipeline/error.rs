//! `PipelineError` — error type for pipeline-level orchestration (parse,
//! workspace load, registry assembly).
//!
//! `EngineError` (the parent crate's existing type) covers errors raised
//! during the wave-execution loop in `cook_engine::run`. Pipeline errors
//! happen earlier: while reading and parsing Cookfiles, walking imports,
//! and assembling the registries that `run` consumes. Keeping the two
//! error types separate avoids overloading `EngineError` with concerns
//! that have nothing to do with cache/scheduler/executor failures, and
//! lets the CLI map each type to its preferred user-facing diagnostic.

use thiserror::Error;

/// Errors produced by the pipeline-orchestration layer (Cookfile parse,
/// workspace resolution, registry assembly).
#[derive(Error, Debug)]
pub enum PipelineError {
    /// Cookfile could not be read from disk.
    #[error("cannot read {path}: {source}")]
    Io {
        path: std::path::PathBuf,
        #[source]
        source: std::io::Error,
    },

    /// Cookfile failed to parse.
    #[error("parse error: {0}")]
    Parse(String),

    /// Codegen (cook-luagen) refused the AST — typically a § 5.4 accessor
    /// placement violation or similar surface-syntax constraint.
    #[error("{0}")]
    Codegen(String),

    /// `--config <name>` selected a config that doesn't exist in the
    /// invoked Cookfile. `available` is the list of names that *do* exist
    /// (may be empty if the Cookfile defines no named configs).
    #[error("unknown config '{name}'{}", format_available(available))]
    UnknownConfig {
        name: String,
        available: Vec<String>,
    },

    /// Workspace import resolution failed (missing dir, cycle, parse error
    /// in an imported Cookfile, etc.). The string carries the full diagnostic.
    #[error("{0}")]
    Workspace(String),

    /// `--set KEY=VALUE` flag was malformed.
    #[error("--set value must be KEY=VALUE, got: {0}")]
    InvalidSet(String),

    /// Catch-all for orchestration-layer errors that don't fit a more
    /// specific variant. Mostly used for diagnostic messages where the
    /// CLI just needs to print the string.
    #[error("{0}")]
    Other(String),
}

fn format_available(available: &[String]) -> String {
    if available.is_empty() {
        ": no named configs defined".to_string()
    } else {
        format!(". available: {}", available.join(", "))
    }
}
