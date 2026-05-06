//! `ViewFrame` trait + `SnapshotFrame`. Live mode plugs in here later.
//! See design spec §Live-Mode Seam.

use std::time::Duration;

use crate::dag_data::WaveDagData;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeStatus {
    Cached,
    Stale,
    Modified,
    Pending,
    Running,
    Done,
    Failed,
}

#[derive(Debug, Clone)]
pub enum FrameEvent {
    Refreshed,
    NodeChanged(String),
}

pub trait ViewFrame {
    fn graph(&self) -> &WaveDagData;
    fn status_of(&self, node_id: &str) -> NodeStatus;
    fn poll_event(&mut self, timeout: Duration) -> Option<FrameEvent>;
}

pub struct SnapshotFrame {
    graph: WaveDagData,
}

impl SnapshotFrame {
    pub fn new(graph: WaveDagData) -> Self {
        Self { graph }
    }
}

impl ViewFrame for SnapshotFrame {
    fn graph(&self) -> &WaveDagData {
        &self.graph
    }

    fn status_of(&self, node_id: &str) -> NodeStatus {
        for wave in &self.graph.waves {
            for n in &wave.nodes {
                if n.id == node_id {
                    return match (n.kind.as_str(), n.cached, n.modified) {
                        ("unit", Some(true), _) => NodeStatus::Cached,
                        ("unit", Some(false), _) => NodeStatus::Stale,
                        ("unit", None, _) => NodeStatus::Pending,
                        ("file", _, Some(true)) => NodeStatus::Modified,
                        ("file", _, Some(false)) => NodeStatus::Done,
                        _ => NodeStatus::Pending,
                    };
                }
            }
        }
        NodeStatus::Pending
    }

    fn poll_event(&mut self, _timeout: Duration) -> Option<FrameEvent> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dag_data::{NodeData, WaveData, WaveDagData};

    fn dag_with(nodes: Vec<NodeData>) -> WaveDagData {
        WaveDagData {
            schema_version: crate::VIEWER_SCHEMA_VERSION,
            target: "build".into(),
            waves: vec![WaveData { recipes: vec![], nodes, edges: vec![] }],
            inter_wave_edges: vec![],
        }
    }

    fn unit(id: &str, cached: Option<bool>) -> NodeData {
        NodeData {
            id: id.into(),
            kind: "unit".into(),
            label: id.into(),
            recipe: Some("r".into()),
            command: Some("cmd".into()),
            output: None,
            cached,
            dep_kind: Some("sequential".into()),
            group_index: None,
            modified: None,
            discovered: None,
        }
    }

    fn file(id: &str, modified: bool) -> NodeData {
        NodeData {
            id: id.into(),
            kind: "file".into(),
            label: id.into(),
            recipe: None,
            command: None,
            output: None,
            cached: None,
            dep_kind: None,
            group_index: None,
            modified: Some(modified),
            discovered: None,
        }
    }

    #[test]
    fn status_of_unit_cached_returns_cached() {
        let frame = SnapshotFrame::new(dag_with(vec![unit("u1", Some(true))]));
        assert_eq!(frame.status_of("u1"), NodeStatus::Cached);
    }

    #[test]
    fn status_of_unit_not_cached_returns_stale() {
        let frame = SnapshotFrame::new(dag_with(vec![unit("u1", Some(false))]));
        assert_eq!(frame.status_of("u1"), NodeStatus::Stale);
    }

    #[test]
    fn status_of_unit_unknown_cache_returns_pending() {
        let frame = SnapshotFrame::new(dag_with(vec![unit("u1", None)]));
        assert_eq!(frame.status_of("u1"), NodeStatus::Pending);
    }

    #[test]
    fn status_of_file_modified_returns_modified() {
        let frame = SnapshotFrame::new(dag_with(vec![file("file:foo", true)]));
        assert_eq!(frame.status_of("file:foo"), NodeStatus::Modified);
    }

    #[test]
    fn status_of_file_unmodified_returns_done() {
        let frame = SnapshotFrame::new(dag_with(vec![file("file:foo", false)]));
        assert_eq!(frame.status_of("file:foo"), NodeStatus::Done);
    }

    #[test]
    fn status_of_unknown_node_returns_pending() {
        let frame = SnapshotFrame::new(dag_with(vec![]));
        assert_eq!(frame.status_of("nope"), NodeStatus::Pending);
    }
}
