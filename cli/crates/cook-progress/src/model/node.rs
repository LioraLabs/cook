//! NodeState — per-node live status inside a recipe.

use std::path::PathBuf;
use std::time::Instant;

use crate::event::{NodeId, NodeKind};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeStatus {
    Waiting,
    Running,
    Completed,
    Failed,
    Skipped,
    /// Status is unknown because the events log is absent; reconstructed from
    /// `.log` files only.
    Unknown,
}

#[derive(Debug, Clone)]
pub struct NodeState {
    pub id: NodeId,
    pub name: String,
    pub artifact: Option<PathBuf>,
    pub fallback_label: String,
    pub status: NodeStatus,
    pub started_at: Option<Instant>,
    pub completed_at: Option<Instant>,
    pub kind: NodeKind,
}

impl NodeState {
    pub fn new(id: NodeId, name: String, artifact: Option<PathBuf>, fallback_label: String) -> Self {
        Self {
            id,
            name,
            artifact,
            fallback_label,
            status: NodeStatus::Waiting,
            started_at: None,
            completed_at: None,
            kind: NodeKind::Cooked,
        }
    }

    /// The unit's own declared output path (relative to the project root,
    /// in full — the distinguishing directory segment, e.g. `packages/` or
    /// `build/`, is never dropped) if `artifact` is set; otherwise a clean
    /// fallback derived from `fallback_label` (stripped of a leading `$ `).
    /// An `@N` token (an interactive step keyed by source line) and a
    /// `probe:<key>` token are returned as-is; any other bare command token
    /// is `$`-prefixed to mark it as raw command text rather than a path.
    pub fn display(&self) -> String {
        if let Some(artifact) = &self.artifact {
            return artifact.to_string_lossy().into_owned();
        }
        // A test unit's name is a synthesised label (`<recipe>_test<N>`,
        // cook-register/src/test_api.rs), not command text — show it bare,
        // never `$`-prefixed.
        if self.kind == NodeKind::Test && !self.name.is_empty() {
            return self.name.clone();
        }
        let stripped = self.fallback_label.trim_start_matches("$ ").trim_start();
        let first = stripped.split_whitespace().next().unwrap_or("?");
        if first.starts_with('@') || first.starts_with("probe:") {
            first.to_string()
        } else {
            format!("${first}")
        }
    }

    /// Raw node name (e.g. "lvm.c"), for log-line prefixes like
    /// `[recipe/<label>] line`. Distinct from `display()`, which prefers the
    /// unit's own full output path and is used for verb lines.
    pub fn label(&self) -> &str {
        &self.name
    }
}

#[cfg(test)]
#[path = "tests/node_tests.rs"]
mod tests;
