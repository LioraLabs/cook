//! Read-side counterpart to `LogStore`: parse `.cook/logs/<build-id>/`
//! into an in-memory `BuildView` that the logs TUI can render.

use std::collections::BTreeMap;
use std::io::{self, BufRead};
use std::path::Path;

use crate::event::{NodeId, NodeKind, RecipeId, SkipReason, Stream, PROGRESS_SCHEMA_VERSION};
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

pub fn load(build_dir: &Path) -> io::Result<(BuildView, LoadDiagnostics)> {
    let mut diag = LoadDiagnostics::default();
    let build_id = build_dir
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| io::Error::other("build_dir has no file name"))?
        .to_string();

    let mut view = BuildView {
        build_id: build_id.clone(),
        started_at: String::new(),
        ended_at: None,
        exit_code: None,
        recipes: BTreeMap::new(),
    };

    let manifest_path = build_dir.join("manifest.toml");
    match std::fs::read_to_string(&manifest_path) {
        Ok(text) => {
            if let Ok(value) = text.parse::<toml::Value>() {
                if let Some(s) = value.get("started_at").and_then(|v| v.as_str()) {
                    view.started_at = s.to_string();
                }
                view.ended_at = value
                    .get("ended_at")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());
                view.exit_code = value
                    .get("exit_code")
                    .and_then(|v| v.as_integer())
                    .map(|i| i as i32);
            }
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            diag.manifest_missing = true;
        }
        Err(e) => return Err(e),
    }

    let events_path = build_dir.join("events.jsonl");
    match std::fs::File::open(&events_path) {
        Ok(_) => {
            replay_events_jsonl(&events_path, &mut view, &mut diag)?;
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            diag.events_jsonl_missing = true;
        }
        Err(e) => return Err(e),
    }

    if diag.events_jsonl_missing {
        populate_from_log_files(build_dir, &mut view)?;
    }

    Ok((view, diag))
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

fn populate_from_log_files(build_dir: &Path, view: &mut BuildView) -> io::Result<()> {
    let nodes_root = build_dir.join("nodes");
    if !nodes_root.exists() {
        return Ok(());
    }
    let mut next_recipe: u32 = 0;
    let mut next_node: u32 = 0;
    for recipe_entry in std::fs::read_dir(&nodes_root)? {
        let recipe_entry = recipe_entry?;
        if !recipe_entry.file_type()?.is_dir() {
            continue;
        }
        let r_name = recipe_entry.file_name().to_string_lossy().to_string();
        let rid = RecipeId::new(next_recipe);
        next_recipe += 1;
        let mut recipe = RecipeView {
            name: r_name.clone(),
            status: Status::Completed, // unknown; default benign
            nodes: BTreeMap::new(),
        };
        for node_entry in std::fs::read_dir(recipe_entry.path())? {
            let node_entry = node_entry?;
            let path = node_entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("log") {
                continue;
            }
            let stem = path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("")
                .to_string();
            let nid = NodeId::new(next_node);
            next_node += 1;
            let mut lines = Vec::new();
            let text = std::fs::read_to_string(&path)?;
            for raw in text.lines() {
                if let Some(rest) = raw.strip_prefix("[out] ") {
                    lines.push(LogLine {
                        stream: Stream::Stdout,
                        ts: None,
                        text: rest.to_string(),
                    });
                } else if let Some(rest) = raw.strip_prefix("[err] ") {
                    lines.push(LogLine {
                        stream: Stream::Stderr,
                        ts: None,
                        text: rest.to_string(),
                    });
                } else {
                    lines.push(LogLine {
                        stream: Stream::Stdout,
                        ts: None,
                        text: raw.to_string(),
                    });
                }
            }
            recipe.nodes.insert(
                nid,
                NodeView {
                    name: stem,
                    status: NodeStatus::Unknown,
                    kind: NodeKind::Cooked,
                    started_at: None,
                    ended_at: None,
                    elapsed_ms: None,
                    skip_reason: None,
                    lines,
                },
            );
        }
        view.recipes.insert(rid, recipe);
    }
    Ok(())
}

fn replay_events_jsonl(
    path: &Path,
    view: &mut BuildView,
    diag: &mut LoadDiagnostics,
) -> io::Result<()> {
    let f = std::fs::File::open(path)?;
    let reader = std::io::BufReader::new(f);

    // Name → minted id, in insertion order.
    let mut recipe_ids: BTreeMap<String, RecipeId> = BTreeMap::new();
    let mut node_ids: BTreeMap<(RecipeId, String), NodeId> = BTreeMap::new();
    let mut next_recipe: u32 = 0;
    let mut next_node: u32 = 0;

    for line in reader.lines() {
        let line = match line {
            Ok(l) => l,
            Err(_) => { diag.skipped_jsonl_lines += 1; continue; }
        };
        if line.trim().is_empty() { continue; }
        let value: serde_json::Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(_) => { diag.skipped_jsonl_lines += 1; continue; }
        };
        let obj = match value.as_object() {
            Some(o) => o,
            None => { diag.skipped_jsonl_lines += 1; continue; }
        };
        let v_ok = obj.get("v")
            .and_then(|v| v.as_u64())
            .map(|n| n as u32 <= PROGRESS_SCHEMA_VERSION)
            .unwrap_or(false);
        if !v_ok {
            diag.skipped_jsonl_lines += 1;
            continue;
        }
        let ty = match obj.get("type").and_then(|v| v.as_str()) {
            Some(t) => t,
            None => { diag.skipped_jsonl_lines += 1; continue; }
        };
        let ts = obj.get("ts").and_then(|v| v.as_str()).map(|s| s.to_string());

        match ty {
            "build-started" => { /* recipes appear on node-started/recipe-started */ }
            "recipe-started" => {
                if let Some(name) = obj.get("recipe").and_then(|v| v.as_str()) {
                    let rid = *recipe_ids.entry(name.to_string()).or_insert_with(|| {
                        let id = RecipeId::new(next_recipe); next_recipe += 1; id
                    });
                    view.recipes.entry(rid).or_insert_with(|| RecipeView {
                        name: name.to_string(),
                        status: Status::Running,
                        nodes: BTreeMap::new(),
                    });
                }
            }
            "recipe-completed" | "recipe-failed" => {
                if let Some(name) = obj.get("recipe").and_then(|v| v.as_str()) {
                    let status = if ty == "recipe-completed" { Status::Completed } else { Status::Failed };
                    if let Some(rid) = recipe_ids.get(name).copied() {
                        if let Some(r) = view.recipes.get_mut(&rid) {
                            r.status = status;
                        }
                    }
                }
            }
            "node-started" => {
                let r_name = obj.get("recipe").and_then(|v| v.as_str()).unwrap_or("");
                let n_name = obj.get("node").and_then(|v| v.as_str()).unwrap_or("").to_string();
                let kind = match obj.get("kind").and_then(|v| v.as_str()) {
                    Some("compile") => NodeKind::Compile,
                    Some("link") => NodeKind::Link,
                    Some("resolve") => NodeKind::Resolve,
                    Some("generate") => NodeKind::Generate,
                    Some("write") => NodeKind::Write,
                    Some("test") => NodeKind::Test,
                    _ => NodeKind::Cooked,
                };
                let rid = *recipe_ids.entry(r_name.to_string()).or_insert_with(|| {
                    let id = RecipeId::new(next_recipe); next_recipe += 1; id
                });
                let recipe = view.recipes.entry(rid).or_insert_with(|| RecipeView {
                    name: r_name.to_string(),
                    status: Status::Running,
                    nodes: BTreeMap::new(),
                });
                let nid = *node_ids.entry((rid, n_name.clone())).or_insert_with(|| {
                    let id = NodeId::new(next_node); next_node += 1; id
                });
                recipe.nodes.entry(nid).or_insert(NodeView {
                    name: n_name,
                    status: NodeStatus::Running,
                    kind,
                    started_at: ts.clone(),
                    ended_at: None,
                    elapsed_ms: None,
                    skip_reason: None,
                    lines: Vec::new(),
                });
            }
            "node-completed" | "node-failed" | "node-cache-hit" | "node-skipped" => {
                let r_name = obj.get("recipe").and_then(|v| v.as_str()).unwrap_or("");
                let n_name = obj.get("node").and_then(|v| v.as_str()).unwrap_or("");
                let rid = match recipe_ids.get(r_name).copied() { Some(r) => r, None => continue };
                let nid = match node_ids.get(&(rid, n_name.to_string())).copied() { Some(n) => n, None => continue };
                let Some(recipe) = view.recipes.get_mut(&rid) else { continue };
                let Some(node) = recipe.nodes.get_mut(&nid) else { continue };
                node.ended_at = ts.clone();
                node.elapsed_ms = obj.get("elapsed_ms").and_then(|v| v.as_u64());
                node.status = match ty {
                    "node-completed" => NodeStatus::Completed,
                    "node-failed" => NodeStatus::Failed,
                    "node-cache-hit" => NodeStatus::Completed,
                    "node-skipped" => {
                        node.skip_reason = obj.get("reason").and_then(|v| v.as_str())
                            .and_then(parse_skip_reason);
                        NodeStatus::Skipped
                    }
                    _ => node.status,
                };
            }
            "node-output" => {
                let r_name = obj.get("recipe").and_then(|v| v.as_str()).unwrap_or("");
                let n_name = obj.get("node").and_then(|v| v.as_str()).unwrap_or("");
                let stream = match obj.get("stream").and_then(|v| v.as_str()) {
                    Some("stderr") => Stream::Stderr,
                    _ => Stream::Stdout,
                };
                let text = obj.get("line").and_then(|v| v.as_str()).unwrap_or("").to_string();
                let rid = match recipe_ids.get(r_name).copied() { Some(r) => r, None => continue };
                let nid = match node_ids.get(&(rid, n_name.to_string())).copied() { Some(n) => n, None => continue };
                if let Some(recipe) = view.recipes.get_mut(&rid) {
                    if let Some(node) = recipe.nodes.get_mut(&nid) {
                        node.lines.push(LogLine { stream, ts: ts.clone(), text });
                    }
                }
            }
            _ => { /* ignore interactive-*, finished — postmortem doesn't need them */ }
        }
    }
    Ok(())
}

fn parse_skip_reason(s: &str) -> Option<SkipReason> {
    match s {
        "upstream-failed" => Some(SkipReason::UpstreamFailed),
        "condition-false" => Some(SkipReason::ConditionFalse),
        "disabled" => Some(SkipReason::Disabled),
        _ => None,
    }
}

#[cfg(test)]
mod tests_load_events {
    use super::*;
    use crate::event::{NodeId, NodeKind, ProgressEvent, RecipeId, RecipeTopo, Stream};
    use crate::log_store::{LogConfig, LogStore};
    use crate::model::build::BuildState;
    use std::time::Duration;

    fn drive_minimal_build(tmp: &Path) -> String {
        let mut store = LogStore::open(tmp, LogConfig::default()).unwrap();
        let mut state = BuildState::new();
        let bs = ProgressEvent::BuildStarted {
            recipes: vec![RecipeTopo {
                id: RecipeId::new(0),
                name: "lib".into(),
                deps: vec![],
                expected_nodes: 1,
            }],
            total_nodes: 1,
        };
        state.apply(&bs);
        store.record(&state, &bs).unwrap();

        let ns = ProgressEvent::NodeStarted {
            recipe: RecipeId::new(0),
            node: NodeId::new(0),
            name: "parser.c".into(),
            artifact: None,
            fallback_label: "parser.c".into(),
            kind: NodeKind::Cooked,
        };
        state.apply(&ns);
        store.record(&state, &ns).unwrap();

        let no = ProgressEvent::NodeOutput {
            recipe: RecipeId::new(0),
            node: NodeId::new(0),
            line: "hello world".into(),
            stream: Stream::Stdout,
        };
        state.apply(&no);
        store.record(&state, &no).unwrap();

        let nf = ProgressEvent::NodeFailed {
            recipe: RecipeId::new(0),
            node: NodeId::new(0),
            elapsed: Duration::from_millis(123),
            error: "boom".into(),
        };
        state.apply(&nf);
        store.record(&state, &nf).unwrap();

        store.close(false).unwrap();
        store.build_id().to_string()
    }

    #[test]
    fn load_replays_events_into_buildview() {
        let tmp = tempfile::tempdir().unwrap();
        let build_id = drive_minimal_build(tmp.path());
        let build_dir = tmp.path().join(".cook").join("logs").join(&build_id);

        let (view, diag) = load(&build_dir).unwrap();
        assert!(!diag.events_jsonl_missing);

        assert_eq!(view.recipes.len(), 1);
        let (_, recipe) = view.recipes.iter().next().unwrap();
        assert_eq!(recipe.name, "lib");
        assert_eq!(recipe.nodes.len(), 1);
        let (_, node) = recipe.nodes.iter().next().unwrap();
        assert_eq!(node.name, "parser.c");
        assert_eq!(node.status, NodeStatus::Failed);
        assert_eq!(node.elapsed_ms, Some(123));
        assert_eq!(node.lines.len(), 1);
        assert_eq!(node.lines[0].text, "hello world");
        assert_eq!(node.lines[0].stream, Stream::Stdout);
    }

    #[test]
    fn load_skips_corrupt_jsonl_lines_and_counts_them() {
        let tmp = tempfile::tempdir().unwrap();
        let build_id = drive_minimal_build(tmp.path());
        let build_dir = tmp.path().join(".cook").join("logs").join(&build_id);
        let events_path = build_dir.join("events.jsonl");
        let mut text = std::fs::read_to_string(&events_path).unwrap();
        text.push_str("not json at all\n");
        text.push_str("{\"missing\":\"v\"}\n");
        std::fs::write(&events_path, text).unwrap();

        let (_view, diag) = load(&build_dir).unwrap();
        assert_eq!(diag.skipped_jsonl_lines, 2);
    }
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

#[cfg(test)]
mod tests_load_manifest {
    use super::*;
    use std::fs;

    #[test]
    fn load_reads_manifest_metadata_into_buildview() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("2026-05-10-aaa");
        fs::create_dir_all(dir.join("nodes")).unwrap();
        fs::write(
            dir.join("manifest.toml"),
            "schema_version = 1\n\
             build_id = \"2026-05-10-aaa\"\n\
             started_at = \"2026-05-10T10:00:00Z\"\n\
             ended_at = \"2026-05-10T10:00:05Z\"\n\
             exit_code = 0\n",
        )
        .unwrap();

        let (view, diag) = load(&dir).unwrap();
        assert_eq!(view.build_id, "2026-05-10-aaa");
        assert_eq!(view.started_at, "2026-05-10T10:00:00Z");
        assert_eq!(view.ended_at.as_deref(), Some("2026-05-10T10:00:05Z"));
        assert_eq!(view.exit_code, Some(0));
        assert!(!diag.manifest_missing);
    }

    #[test]
    fn load_tolerates_missing_manifest() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("2026-05-10-bbb");
        fs::create_dir_all(dir.join("nodes")).unwrap();

        let (view, diag) = load(&dir).unwrap();
        assert!(diag.manifest_missing);
        assert_eq!(view.build_id, "2026-05-10-bbb");
        assert!(view.exit_code.is_none());
    }
}

#[cfg(test)]
mod tests_log_fallback {
    use super::*;
    use std::fs;

    #[test]
    fn load_falls_back_to_log_files_when_events_jsonl_absent() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("2026-05-10-zzz");
        let nodes = dir.join("nodes").join("lib");
        fs::create_dir_all(&nodes).unwrap();
        fs::write(
            dir.join("manifest.toml"),
            "schema_version = 1\n\
             build_id = \"2026-05-10-zzz\"\n\
             started_at = \"2026-05-10T10:00:00Z\"\n\
             ended_at = \"2026-05-10T10:00:01Z\"\n\
             exit_code = 0\n",
        )
        .unwrap();
        fs::write(
            nodes.join("parser.c.log"),
            "[out] hello\n[err] oops\n",
        )
        .unwrap();

        let (view, diag) = load(&dir).unwrap();
        assert!(diag.events_jsonl_missing);
        assert_eq!(view.recipes.len(), 1);
        let (_, recipe) = view.recipes.iter().next().unwrap();
        assert_eq!(recipe.name, "lib");
        let (_, node) = recipe.nodes.iter().next().unwrap();
        assert_eq!(node.name, "parser.c");
        assert_eq!(node.status, NodeStatus::Unknown);
        assert_eq!(node.lines.len(), 2);
        assert_eq!(node.lines[0].stream, Stream::Stdout);
        assert_eq!(node.lines[0].text, "hello");
        assert_eq!(node.lines[1].stream, Stream::Stderr);
        assert_eq!(node.lines[1].text, "oops");
    }
}
