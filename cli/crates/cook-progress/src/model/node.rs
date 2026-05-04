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

    /// Basename of `artifact` if set; otherwise the first whitespace-separated token
    /// of `fallback_label` (stripped of a leading `$ `). An `@N` token (an
    /// interactive step keyed by source line) is returned as-is.
    pub fn display(&self) -> String {
        if let Some(artifact) = &self.artifact {
            if let Some(base) = artifact.file_name().and_then(|s| s.to_str()) {
                return base.to_string();
            }
        }
        let stripped = self.fallback_label.trim_start_matches("$ ").trim_start();
        let first = stripped.split_whitespace().next().unwrap_or("?");
        if first.starts_with('@') {
            first.to_string()
        } else {
            format!("${first}")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_uses_artifact_basename() {
        let n = NodeState::new(
            NodeId::new(0),
            "lvm.c".into(),
            Some("build/obj/liblua/lvm.o".into()),
            "clang -c lvm.c".into(),
        );
        assert_eq!(n.display(), "lvm.o");
    }

    #[test]
    fn display_falls_back_to_command_token() {
        let n = NodeState::new(
            NodeId::new(1),
            "archive".into(),
            None,
            "$ ar rcs libliblua.a lapi.o".into(),
        );
        assert_eq!(n.display(), "$ar");
    }

    #[test]
    fn display_handles_empty_fallback() {
        let n = NodeState::new(NodeId::new(2), "x".into(), None, "".into());
        assert_eq!(n.display(), "$?");
    }

    #[test]
    fn new_default_kind_is_cooked() {
        let n = NodeState::new(
            NodeId::new(0),
            "x".into(),
            None,
            "".into(),
        );
        assert_eq!(n.kind, crate::event::NodeKind::Cooked);
    }
}
