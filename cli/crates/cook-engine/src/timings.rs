//! CS-0171: last-observed unit wall times, recovered from retained build logs.
//!
//! `cook why` reports what a run *would* do. This module supplies the other
//! half of the sentence: roughly what it cost the last time it actually
//! happened. Nothing here measures anything — it reads what the progress log
//! store already wrote.
//!
//! The data is deliberately thin, and §17.1.6.4 constrains how it may be
//! presented because of that:
//!
//! * A cache hit emits `node-cache-hit`, which carries no `elapsed_ms`. Warm
//!   units therefore contribute no history at all, which is fine — nobody
//!   needs to know how long a hit took.
//! * Retention is `LogConfig::keep_builds` (20 by default) and the logs are
//!   machine-local, so a unit that is fleet-warm but never ran here has no
//!   observation. That is reported as absence, never as zero.
//! * The join key is the unit's recipe-local cache key, stamped onto
//!   `node-completed` by CS-0171. The `node` field is a display name and
//!   collides across distinct units, so it cannot serve.
//!
//! The result is an observation of the past, not a prediction of the run being
//! explained. Callers must render it that way.

use std::collections::BTreeMap;
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::Path;
use std::time::SystemTime;

/// One unit's most recent retained timing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Observation {
    /// Wall time the unit took on that run.
    pub elapsed_ms: u64,
    /// How many retained builds back the observation came from. `0` is the
    /// most recent retained build.
    pub builds_ago: usize,
}

/// Last-observed wall times, keyed by `(recipe, cache_key)`.
#[derive(Debug, Default, Clone)]
pub struct Timings {
    by_unit: BTreeMap<(String, String), Observation>,
}

impl Timings {
    /// Read every retained build log under `<project_root>/.cook/logs`,
    /// newest first, keeping the first (therefore most recent) observation for
    /// each unit.
    ///
    /// Best-effort throughout: an unreadable directory, a truncated
    /// `events.jsonl`, or a malformed line yields less history, never an
    /// error. `cook why` must still answer when the log store is a mess.
    pub fn load(project_root: &Path) -> Self {
        let root = project_root.join(".cook").join("logs");
        let mut builds = build_dirs_newest_first(&root);
        // Guard against an unbounded scan if retention ever fails to prune:
        // beyond the retention window the observations are too stale to be
        // worth the read.
        builds.truncate(MAX_BUILDS_SCANNED);

        let mut by_unit: BTreeMap<(String, String), Observation> = BTreeMap::new();
        for (builds_ago, dir) in builds.into_iter().enumerate() {
            harvest_build(&dir.join("events.jsonl"), builds_ago, &mut by_unit);
        }
        Self { by_unit }
    }

    pub fn get(&self, recipe: &str, cache_key: &str) -> Option<&Observation> {
        // BTreeMap keyed by owned Strings: the borrowed-tuple lookup that would
        // avoid these clones needs an `Ord`-compatible borrowed key type, which
        // a tuple of `&str` is not. The map is small (bounded by units × 20
        // builds) and this runs once per rendered node, so the clone is not
        // worth designing around.
        self.by_unit.get(&(recipe.to_string(), cache_key.to_string()))
    }

    pub fn is_empty(&self) -> bool {
        self.by_unit.is_empty()
    }
}

/// Retention is 20 builds; scanning meaningfully past that means retention has
/// failed, and the extra reads buy nothing but stale numbers.
const MAX_BUILDS_SCANNED: usize = 20;

/// Build directories under `root`, most recently written first.
///
/// Ordered by directory mtime rather than by build id: the id is
/// `<date>-<low 12 bits of the nanosecond clock, hex>`, so it does not sort by
/// recency within a day. mtime is also exactly the signal `LogStore::rotate`
/// uses to choose what to delete, so reader and pruner agree on which build is
/// oldest.
fn build_dirs_newest_first(root: &Path) -> Vec<std::path::PathBuf> {
    let Ok(rd) = fs::read_dir(root) else {
        return Vec::new();
    };
    let mut dirs: Vec<(std::path::PathBuf, SystemTime)> = rd
        .filter_map(|r| r.ok())
        .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
        .filter_map(|e| {
            e.metadata()
                .ok()
                .and_then(|m| m.modified().ok())
                .map(|t| (e.path(), t))
        })
        .collect();
    dirs.sort_by(|a, b| b.1.cmp(&a.1));
    dirs.into_iter().map(|(p, _)| p).collect()
}

/// Scan one build's event stream, recording each unit's time unless a newer
/// build already supplied one.
fn harvest_build(
    events: &Path,
    builds_ago: usize,
    out: &mut BTreeMap<(String, String), Observation>,
) {
    let Ok(f) = fs::File::open(events) else {
        return;
    };
    for line in BufReader::new(f).lines().map_while(Result::ok) {
        // A build's event stream is dominated by `node-output` lines — one per
        // line of every unit's stdout. Rejecting on a substring before handing
        // the line to serde keeps this a scan rather than a parse of the whole
        // build's console output.
        if !line.contains("\"node-completed\"") {
            continue;
        }
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&line) else {
            continue;
        };
        if v.get("type").and_then(|t| t.as_str()) != Some("node-completed") {
            continue;
        }
        let (Some(recipe), Some(key), Some(ms)) = (
            v.get("recipe").and_then(|r| r.as_str()),
            v.get("cache_key").and_then(|k| k.as_str()),
            v.get("elapsed_ms").and_then(|e| e.as_u64()),
        ) else {
            // `cache_key` is null for a non-cacheable node, and absent entirely
            // in a log written before CS-0171. Both mean "no timing to recover
            // for a unit `cook why` can name", so both are skipped.
            continue;
        };
        out.entry((recipe.to_string(), key.to_string()))
            .or_insert(Observation { elapsed_ms: ms, builds_ago });
    }
}

/// Human duration. Sub-second work is the common case for a single unit, and
/// whole-tree sums run to minutes, so both ends need to read cleanly.
pub fn render_ms(ms: u64) -> String {
    if ms < 1000 {
        format!("{ms}ms")
    } else if ms < 60_000 {
        format!("{:.1}s", ms as f64 / 1000.0)
    } else {
        format!("{}m{:02}s", ms / 60_000, (ms % 60_000) / 1000)
    }
}

#[cfg(test)]
#[path = "tests/timings_tests.rs"]
mod tests;
