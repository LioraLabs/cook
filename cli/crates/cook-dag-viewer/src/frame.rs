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

    fn status_of(&self, _node_id: &str) -> NodeStatus {
        // Filled in by Task 2.
        NodeStatus::Pending
    }

    fn poll_event(&mut self, _timeout: Duration) -> Option<FrameEvent> {
        None
    }
}
