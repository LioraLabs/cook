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

pub fn list_builds(logs_root: &Path) -> io::Result<Vec<BuildSummary>> {
    if !logs_root.exists() {
        return Ok(Vec::new());
    }
    let mut summaries: Vec<(BuildSummary, std::time::SystemTime)> = Vec::new();
    for entry in std::fs::read_dir(logs_root)? {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let build_dir = entry.path();
        let mtime = entry.metadata()?.modified().unwrap_or(std::time::UNIX_EPOCH);
        let summary = summarize_build_dir(&build_dir)?;
        summaries.push((summary, mtime));
    }
    summaries.sort_by(|a, b| b.1.cmp(&a.1)); // newest first
    Ok(summaries.into_iter().map(|(s, _)| s).collect())
}

fn summarize_build_dir(build_dir: &Path) -> io::Result<BuildSummary> {
    let build_id = build_dir
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("")
        .to_string();
    let manifest_path = build_dir.join("manifest.toml");
    let mut summary = BuildSummary {
        build_id,
        started_at: String::new(),
        ended_at: None,
        exit_code: None,
        recipe_count: 0,
        failed_count: 0,
    };
    if let Ok(text) = std::fs::read_to_string(&manifest_path) {
        if let Ok(value) = text.parse::<toml::Value>() {
            if let Some(s) = value.get("started_at").and_then(|v| v.as_str()) {
                summary.started_at = s.to_string();
            }
            summary.ended_at = value
                .get("ended_at")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            summary.exit_code = value
                .get("exit_code")
                .and_then(|v| v.as_integer())
                .map(|i| i as i32);
        }
    }
    Ok(summary)
}

#[cfg(test)]
mod tests_list {
    use super::*;
    use std::fs;

    #[test]
    fn list_builds_returns_newest_first_with_parsed_metadata() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();

        for (id, exit) in [("2026-05-10-aaa", 0), ("2026-05-10-bbb", 1)] {
            let dir = root.join(id);
            fs::create_dir_all(&dir).unwrap();
            let manifest = format!(
                "schema_version = 1\n\
                 build_id = \"{id}\"\n\
                 started_at = \"2026-05-10T10:00:00Z\"\n\
                 ended_at = \"2026-05-10T10:00:05Z\"\n\
                 exit_code = {exit}\n"
            );
            fs::write(dir.join("manifest.toml"), manifest).unwrap();
            std::thread::sleep(std::time::Duration::from_millis(10));
        }

        let builds = list_builds(root).unwrap();
        assert_eq!(builds.len(), 2);
        assert_eq!(builds[0].build_id, "2026-05-10-bbb"); // newest first
        assert_eq!(builds[0].exit_code, Some(1));
        assert_eq!(builds[1].exit_code, Some(0));
    }

    #[test]
    fn list_builds_skips_non_dir_entries_and_missing_manifests() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        fs::write(root.join("stray.txt"), "noise").unwrap();
        let dir = root.join("2026-05-10-aaa");
        fs::create_dir_all(&dir).unwrap();
        // no manifest

        let builds = list_builds(root).unwrap();
        assert_eq!(builds.len(), 1);
        assert_eq!(builds[0].build_id, "2026-05-10-aaa");
        assert_eq!(builds[0].exit_code, None);
    }
}
