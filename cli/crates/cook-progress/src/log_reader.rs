//! Read-side counterpart to `LogStore`: parse `.cook/logs/<build-id>/`
//! into an in-memory `BuildView` that the logs TUI can render.

use std::collections::BTreeMap;
use std::io;
use std::path::Path;

use crate::event::{NodeId, NodeKind, RecipeId, SkipReason, Stream};
use crate::model::{NodeStatus, Status};

#[derive(Debug, Clone)]
pub struct BuildView {
    pub build_id: String,
    pub started_at: String,
    pub ended_at: Option<String>,
    pub exit_code: Option<i32>,
    pub recipes: BTreeMap<RecipeId, RecipeView>,
}

#[derive(Debug, Clone)]
pub struct RecipeView {
    pub name: String,
    pub status: Status,
    pub nodes: BTreeMap<NodeId, NodeView>,
}

#[derive(Debug, Clone)]
pub struct NodeView {
    pub name: String,
    pub status: NodeStatus,
    pub kind: NodeKind,
    pub started_at: Option<String>,
    pub ended_at: Option<String>,
    pub elapsed_ms: Option<u64>,
    pub skip_reason: Option<SkipReason>,
    pub lines: Vec<LogLine>,
}

#[derive(Debug, Clone)]
pub struct LogLine {
    pub stream: Stream,
    pub ts: Option<String>,
    pub text: String,
}

#[derive(Debug, Clone)]
pub struct BuildSummary {
    pub build_id: String,
    pub started_at: String,
    pub ended_at: Option<String>,
    pub exit_code: Option<i32>,
    pub recipe_count: usize,
    pub failed_count: usize,
}

#[derive(Debug, Default, Clone)]
pub struct LoadDiagnostics {
    pub skipped_jsonl_lines: usize,
    pub manifest_missing: bool,
    pub events_jsonl_missing: bool,
}

pub fn load(_build_dir: &Path) -> io::Result<(BuildView, LoadDiagnostics)> {
    unimplemented!("filled in by later tasks")
}

pub fn list_builds(_logs_root: &Path) -> io::Result<Vec<BuildSummary>> {
    unimplemented!("filled in by later tasks")
}
