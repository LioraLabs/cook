//! Error types for the logs viewer.

use std::io;

#[derive(Debug, thiserror::Error)]
pub enum ViewerError {
    #[error("failed to start logs viewer terminal: {0}")]
    TerminalInit(String),
    #[error("failed to load build {build_id}: {source}")]
    Load { build_id: String, source: io::Error },
    #[error("layout failure: {0}")]
    Layout(String),
    #[error("build {requested} not found. Recent builds: {}", nearby.join(", "))]
    BuildNotFound { requested: String, nearby: Vec<String> },
    #[error("failed to list builds: {0}")]
    IoListing(io::Error),
}

impl ViewerError {
    pub fn io_listing(e: io::Error) -> Self { Self::IoListing(e) }
}
