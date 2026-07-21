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
mod tests {
    use super::*;

    #[test]
    fn display_uses_full_artifact_path() {
        let n = NodeState::new(
            NodeId::new(0),
            "lvm.c".into(),
            Some("build/obj/liblua/lvm.o".into()),
            "clang -c lvm.c".into(),
        );
        assert_eq!(n.display(), "build/obj/liblua/lvm.o");
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
    fn display_uses_synthesised_name_for_test_nodes() {
        let mut n = NodeState::new(NodeId::new(5), "rust-test_test1".into(), None, "".into());
        n.kind = NodeKind::Test;
        assert_eq!(n.display(), "rust-test_test1");
    }

    #[test]
    fn display_uses_full_artifact_path_nested() {
        let n = NodeState::new(
            NodeId::new(3),
            "alpha".into(),
            Some("build/counts/alpha.count".into()),
            "wc -w < a.txt > alpha.count".into(),
        );
        assert_eq!(n.display(), "build/counts/alpha.count");
    }

    #[test]
    fn display_preserves_distinguishing_directory_segment() {
        // Real sighting: two units differ only by a leading directory
        // segment (e.g. one lives under packages/, the other doesn't).
        // Basename-only display collided them into the same label.
        let n = NodeState::new(
            NodeId::new(5),
            "app".into(),
            Some("packages/app.stamp".into()),
            "touch packages/app.stamp".into(),
        );
        assert_eq!(n.display(), "packages/app.stamp");
    }

    #[test]
    fn display_probe_fallback_keeps_probe_key_style() {
        // Probes have no declared outputs (artifact is always None), so
        // display() falls back to the label — which must render as
        // `probe:<key>`, not `$probe:<key>` (the raw-command-token marker
        // is only for actual shell text, not probe identifiers).
        let n = NodeState::new(
            NodeId::new(6),
            "probe:sys:os".into(),
            None,
            "probe:sys:os".into(),
        );
        assert_eq!(n.display(), "probe:sys:os");
    }

    #[test]
    fn display_falls_back_to_clean_nonempty_label_without_artifact() {
        let n = NodeState::new(
            NodeId::new(4),
            "count".into(),
            None,
            "wc -w < a.txt > b.count".into(),
        );
        let d = n.display();
        assert!(!d.is_empty());
        assert!(!d.contains("set -e"));
        assert!(!d.starts_with('@'));
    }

    #[test]
    fn label_returns_raw_name() {
        let n = NodeState::new(
            NodeId::new(0),
            "lvm.c".into(),
            Some("build/obj/lvm.o".into()),
            "clang -c lvm.c".into(),
        );
        assert_eq!(n.label(), "lvm.c");
        // display() picks the unit's own full output path, label() the raw name — they differ on purpose.
        assert_eq!(n.display(), "build/obj/lvm.o");
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
