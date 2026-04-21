//! Persistent build log store — .cook/logs/<build-id>/.

use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::event::{NodeId, ProgressEvent, RecipeId, Stream};
use crate::model::build::BuildState;
use crate::render::json::event_to_value;

#[derive(Debug, Clone)]
pub struct LogConfig {
    pub keep_builds: usize,
    pub max_bytes_per_node: u64,
    pub max_total_bytes: u64,
    pub events_jsonl: bool,
}

impl Default for LogConfig {
    fn default() -> Self {
        Self {
            keep_builds: 20,
            max_bytes_per_node: 2 * 1024 * 1024,
            max_total_bytes: 500 * 1024 * 1024,
            events_jsonl: true,
        }
    }
}

pub struct LogStore {
    root: PathBuf,
    build_id: String,
    events_writer: Option<BufWriter<File>>,
    node_writers: BTreeMap<(RecipeId, NodeId), BufWriter<File>>,
    node_bytes: BTreeMap<(RecipeId, NodeId), u64>,
    config: LogConfig,
    started_at: String,
}

impl LogStore {
    pub fn open(project_root: &Path, config: LogConfig) -> io::Result<Self> {
        let root = project_root.join(".cook").join("logs");
        fs::create_dir_all(&root)?;
        rotate(&root, config.keep_builds, config.max_total_bytes)?;

        let build_id = new_build_id();
        let build_dir = root.join(&build_id);
        fs::create_dir_all(build_dir.join("nodes"))?;

        let events_writer = if config.events_jsonl {
            Some(BufWriter::new(
                OpenOptions::new().create(true).append(true).open(build_dir.join("events.jsonl"))?
            ))
        } else {
            None
        };

        Ok(Self {
            root,
            build_id,
            events_writer,
            node_writers: BTreeMap::new(),
            node_bytes: BTreeMap::new(),
            config,
            started_at: current_rfc3339(),
        })
    }

    pub fn build_id(&self) -> &str { &self.build_id }

    pub fn record(&mut self, state: &BuildState, event: &ProgressEvent) -> io::Result<()> {
        if let Some(w) = self.events_writer.as_mut() {
            let mut payload = event_to_value(state, event);
            let mut obj = serde_json::Map::new();
            obj.insert("ts".into(), serde_json::Value::String(current_rfc3339()));
            obj.insert("v".into(), serde_json::Value::from(1u32));
            if let serde_json::Value::Object(inner) = payload.take() {
                for (k, v) in inner {
                    obj.insert(k, v);
                }
            }
            serde_json::to_writer(&mut *w, &serde_json::Value::Object(obj)).map_err(io::Error::other)?;
            w.write_all(b"\n")?;
        }

        if let ProgressEvent::NodeOutput { recipe, node, line, stream } = event {
            let key = (*recipe, *node);
            let bytes_now = *self.node_bytes.entry(key).or_insert(0);
            if bytes_now >= self.config.max_bytes_per_node {
                return Ok(());
            }

            if !self.node_writers.contains_key(&key) {
                let r = state.recipes.get(recipe);
                let rname = r.map(|x| x.name.as_str()).unwrap_or("unknown");
                let nname = r
                    .and_then(|x| x.nodes.get(node))
                    .map(|n| n.name.clone())
                    .unwrap_or_else(|| format!("node-{}", node.raw()));
                let dir = self.root.join(&self.build_id).join("nodes").join(sanitize(rname));
                fs::create_dir_all(&dir)?;
                let path = dir.join(format!("{}.log", sanitize(&nname)));
                let f = OpenOptions::new().create(true).append(true).open(path)?;
                self.node_writers.insert(key, BufWriter::new(f));
            }

            let tag = match stream { Stream::Stdout => "[out]", Stream::Stderr => "[err]" };
            let record = format!("{tag} {line}\n");
            let writer = self.node_writers.get_mut(&key).unwrap();
            writer.write_all(record.as_bytes())?;
            let new_bytes = bytes_now + record.len() as u64;
            self.node_bytes.insert(key, new_bytes);
            if new_bytes >= self.config.max_bytes_per_node {
                writer.write_all(b"--- truncated ---\n")?;
            }
        }
        Ok(())
    }

    pub fn close(&mut self, success: bool) -> io::Result<()> {
        if let Some(w) = self.events_writer.as_mut() { w.flush()?; }
        for w in self.node_writers.values_mut() { w.flush()?; }

        let manifest = format!(
            "schema_version = 1\nbuild_id = \"{}\"\nstarted_at = \"{}\"\nended_at = \"{}\"\nexit_code = {}\n",
            self.build_id,
            self.started_at,
            current_rfc3339(),
            if success { 0 } else { 1 },
        );
        let path = self.root.join(&self.build_id).join("manifest.toml");
        fs::write(path, manifest)
    }
}

fn new_build_id() -> String {
    let ts = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_nanos();
    let hash = format!("{:x}", ts & 0xfff);
    format!("{}-{hash}", current_date_string())
}

fn current_date_string() -> String {
    let now = time::OffsetDateTime::now_utc();
    format!("{:04}-{:02}-{:02}", now.year(), u8::from(now.month()), now.day())
}

fn current_rfc3339() -> String {
    time::OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_string())
}

fn sanitize(name: &str) -> String {
    name.chars()
        .map(|c| if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_') { c } else { '_' })
        .collect()
}

fn rotate(root: &Path, keep_builds: usize, max_total_bytes: u64) -> io::Result<()> {
    let mut entries: Vec<(PathBuf, SystemTime)> = fs::read_dir(root)?
        .filter_map(|r| r.ok())
        .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
        .filter_map(|e| e.metadata().ok().and_then(|m| m.modified().ok()).map(|t| (e.path(), t)))
        .collect();
    entries.sort_by_key(|(_, t)| *t);

    while entries.len() > keep_builds {
        let (p, _) = entries.remove(0);
        let _ = fs::remove_dir_all(p);
    }

    loop {
        let total: u64 = entries.iter()
            .map(|(p, _)| dir_size(p).unwrap_or(0))
            .sum();
        if total <= max_total_bytes || entries.is_empty() { break; }
        let (p, _) = entries.remove(0);
        let _ = fs::remove_dir_all(p);
    }
    Ok(())
}

fn dir_size(p: &Path) -> io::Result<u64> {
    let mut total = 0;
    for entry in fs::read_dir(p)? {
        let entry = entry?;
        let md = entry.metadata()?;
        if md.is_dir() {
            total += dir_size(&entry.path())?;
        } else {
            total += md.len();
        }
    }
    Ok(total)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::RecipeTopo;

    #[test]
    fn open_creates_build_directory() {
        let tmp = tempfile::tempdir().unwrap();
        let store = LogStore::open(tmp.path(), LogConfig::default()).unwrap();
        let build_dir = tmp.path().join(".cook").join("logs").join(store.build_id());
        assert!(build_dir.exists());
        assert!(build_dir.join("nodes").exists());
    }

    #[test]
    fn node_output_is_written_with_stream_tag() {
        let tmp = tempfile::tempdir().unwrap();
        let mut store = LogStore::open(tmp.path(), LogConfig::default()).unwrap();
        let mut state = BuildState::new();
        state.apply(&ProgressEvent::BuildStarted {
            recipes: vec![RecipeTopo {
                id: RecipeId::new(0), name: "lib".into(),
                deps: vec![], expected_nodes: 1,
            }],
            total_nodes: 1,
        });
        state.apply(&ProgressEvent::NodeStarted {
            recipe: RecipeId::new(0), node: NodeId::new(0),
            name: "lvm.c".into(), artifact: None, fallback_label: "x".into(),
        });
        store.record(&state, &ProgressEvent::NodeOutput {
            recipe: RecipeId::new(0), node: NodeId::new(0),
            line: "warning".into(), stream: Stream::Stderr,
        }).unwrap();
        store.close(true).unwrap();

        let log = fs::read_to_string(tmp.path()
            .join(".cook").join("logs").join(store.build_id())
            .join("nodes").join("lib").join("lvm.c.log")).unwrap();
        assert!(log.contains("[err] warning"), "got: {log}");
    }

    #[test]
    fn rotate_removes_oldest_when_over_keep_builds() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join(".cook").join("logs");
        fs::create_dir_all(&root).unwrap();
        for i in 0..5 {
            let d = root.join(format!("build-{i}"));
            fs::create_dir_all(&d).unwrap();
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        rotate(&root, 2, u64::MAX).unwrap();
        let remaining = fs::read_dir(&root).unwrap().count();
        assert_eq!(remaining, 2);
    }

    #[test]
    fn events_jsonl_is_written_in_spec_shape() {
        let tmp = tempfile::tempdir().unwrap();
        let mut store = LogStore::open(tmp.path(), LogConfig::default()).unwrap();
        let mut state = BuildState::new();
        let ev = ProgressEvent::BuildStarted {
            recipes: vec![RecipeTopo {
                id: RecipeId::new(0), name: "deps".into(),
                deps: vec![], expected_nodes: 2,
            }],
            total_nodes: 2,
        };
        state.apply(&ev);
        store.record(&state, &ev).unwrap();
        store.close(true).unwrap();

        let events_path = tmp.path()
            .join(".cook").join("logs").join(store.build_id())
            .join("events.jsonl");
        let data = fs::read_to_string(events_path).unwrap();
        assert!(data.contains("\"type\":\"build-started\""), "got: {data}");
        assert!(data.contains("\"v\":1"), "got: {data}");
        assert!(data.contains("\"ts\":"), "got: {data}");
    }

    #[test]
    fn manifest_toml_written_on_close() {
        let tmp = tempfile::tempdir().unwrap();
        let mut store = LogStore::open(tmp.path(), LogConfig::default()).unwrap();
        store.close(true).unwrap();

        let manifest_path = tmp.path()
            .join(".cook").join("logs").join(store.build_id())
            .join("manifest.toml");
        let data = fs::read_to_string(manifest_path).unwrap();
        assert!(data.contains("schema_version = 1"));
        assert!(data.contains("exit_code = 0"));
        assert!(data.contains(&format!("build_id = \"{}\"", store.build_id())));
    }

    #[test]
    fn recipe_and_node_names_are_sanitized_into_paths() {
        let tmp = tempfile::tempdir().unwrap();
        let mut store = LogStore::open(tmp.path(), LogConfig::default()).unwrap();
        let mut state = BuildState::new();
        state.apply(&ProgressEvent::BuildStarted {
            recipes: vec![RecipeTopo {
                id: RecipeId::new(0),
                name: "../../etc/passwd".into(),
                deps: vec![],
                expected_nodes: 1,
            }],
            total_nodes: 1,
        });
        state.apply(&ProgressEvent::NodeStarted {
            recipe: RecipeId::new(0), node: NodeId::new(0),
            name: "../../root".into(), artifact: None, fallback_label: "x".into(),
        });
        store.record(&state, &ProgressEvent::NodeOutput {
            recipe: RecipeId::new(0), node: NodeId::new(0),
            line: "hi".into(), stream: Stream::Stdout,
        }).unwrap();
        store.close(true).unwrap();

        // Nothing was written outside the build directory.
        let build_dir = tmp.path().join(".cook").join("logs").join(store.build_id());
        let nodes_dir = build_dir.join("nodes");
        let sanitized_rname = nodes_dir.join(".._.._etc_passwd");
        assert!(sanitized_rname.exists(), "sanitized recipe dir should exist: {sanitized_rname:?}");
        let sanitized_file = sanitized_rname.join(".._.._root.log");
        assert!(sanitized_file.exists(), "sanitized node file should exist: {sanitized_file:?}");

        // No traversal happened: there is no 'etc' directory outside the build.
        assert!(!tmp.path().join("etc").exists());
    }
}
