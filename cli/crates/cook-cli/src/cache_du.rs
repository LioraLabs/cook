//! `cook cache du` — report local artifact-store disk usage (COOK-232).
//!
//! Read-only, like `cook cache dump`: resolves the workspace root but does
//! NOT register the workspace, so it works even when the Cookfile no longer
//! parses — exactly the moment someone needs to inspect cache state by hand.
//!
//! `summarize` and `render` are pure functions over `Vec<EvictCandidate>` —
//! no I/O — so the aggregation and rendering logic is unit-testable without
//! touching a filesystem. `cmd_cache_du` is the thin I/O shell: resolve the
//! project root and config, enumerate the local CAS, summarize, render,
//! print.

use std::path::Path;

use cook_engine::cook_cache::backend::{EvictCandidate, LocalBackend};
use cook_engine::cook_cache::CloudConfig;

use crate::cli::Globals;
use crate::error::CookError;
use crate::pipeline::resolve_project_root;

/// `cook cache du`. Resolves the project root exactly like `cmd_cache_dump`
/// (no workspace registration), loads `.cook/cloud.toml`, and either defers
/// to the server-managed cloud cache or walks the local CAS and prints a
/// usage report.
pub fn cmd_cache_du(globals: &Globals) -> Result<(), CookError> {
    let project_root = resolve_project_root(globals)?;
    let cloud_config = CloudConfig::load_or_default(&project_root)
        .map_err(|e| CookError::Other(format!("invalid .cook/cloud.toml: {e}")))?;

    if cloud_config.cloud.enabled {
        println!("cloud cache is server-managed; du operates on the local store");
        return Ok(());
    }

    // COOK-232 Addition 1: the SAME resolution `build_cache_ctx` uses, so
    // `du` can never report on a different directory than the one the
    // engine actually writes to.
    let store = cloud_config.resolved_cache_dir();

    // Surface a bad `[cache] max_size` literal as an error rather than
    // silently ignoring it — a typo'd budget must not be swallowed.
    let budget = cloud_config
        .max_size_bytes()
        .map_err(|e| CookError::Other(format!("invalid .cook/cloud.toml: {e}")))?;

    let backend = LocalBackend::new(store.clone());
    let candidates = backend.enumerate().map_err(|e| {
        CookError::Other(format!(
            "enumerating cache store {}: {e}",
            store.display()
        ))
    })?;

    let report = summarize(candidates);
    print!("{}", render(&report, &store, budget));
    Ok(())
}

/// Pure aggregation over `LocalBackend::enumerate` output. No I/O — this is
/// what makes `render`'s output unit-testable without a filesystem.
pub(crate) struct DuReport {
    total_bytes: u64,
    total_count: usize,
    /// `(kind label, total bytes, object count)`, sorted bytes-descending.
    /// `EvictCandidate.kind == None` is folded into the `"file"` label here
    /// (ArtifactMeta documents `None` as the legacy file-artifact case).
    by_kind: Vec<(String, u64, usize)>,
    /// `(recipe_namespace, total bytes, object count)`, sorted
    /// bytes-descending. The namespace string is kept EXACTLY as stored —
    /// no parsing or repair of the empty `project_id` prefix (COOK-333,
    /// explicitly out of scope here). Capping to the top 20 and the
    /// empty-string `"<no sidecar>"` label are both `render`'s job, not
    /// this aggregation's, so the raw data stays inspectable in tests.
    by_namespace: Vec<(String, u64, usize)>,
    /// Min / max `last_access` (blob mtime, Unix seconds). `None` only when
    /// there are no candidates at all.
    oldest: Option<u64>,
    newest: Option<u64>,
}

pub(crate) fn summarize(candidates: Vec<EvictCandidate>) -> DuReport {
    let mut total_bytes: u64 = 0;
    let mut oldest: Option<u64> = None;
    let mut newest: Option<u64> = None;
    let mut by_kind: std::collections::BTreeMap<String, (u64, usize)> =
        std::collections::BTreeMap::new();
    let mut by_namespace: std::collections::BTreeMap<String, (u64, usize)> =
        std::collections::BTreeMap::new();

    for c in &candidates {
        total_bytes = total_bytes.saturating_add(c.size);
        oldest = Some(oldest.map_or(c.last_access, |o| o.min(c.last_access)));
        newest = Some(newest.map_or(c.last_access, |n| n.max(c.last_access)));

        let kind_label = c.kind.clone().unwrap_or_else(|| "file".to_string());
        let kind_entry = by_kind.entry(kind_label).or_insert((0, 0));
        kind_entry.0 = kind_entry.0.saturating_add(c.size);
        kind_entry.1 += 1;

        let ns_entry = by_namespace
            .entry(c.recipe_namespace.clone())
            .or_insert((0, 0));
        ns_entry.0 = ns_entry.0.saturating_add(c.size);
        ns_entry.1 += 1;
    }

    let mut by_kind: Vec<(String, u64, usize)> = by_kind
        .into_iter()
        .map(|(label, (bytes, count))| (label, bytes, count))
        .collect();
    // Bytes-descending; ties broken by label so the order is deterministic.
    by_kind.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));

    let mut by_namespace: Vec<(String, u64, usize)> = by_namespace
        .into_iter()
        .map(|(ns, (bytes, count))| (ns, bytes, count))
        .collect();
    by_namespace.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));

    DuReport {
        total_bytes,
        total_count: candidates.len(),
        by_kind,
        by_namespace,
        oldest,
        newest,
    }
}

/// Cap on the number of `By namespace:` rows printed by name; the rest fold
/// into a single `… and N more` line.
const MAX_NAMESPACE_ROWS: usize = 20;

pub(crate) fn render(report: &DuReport, store: &Path, budget: Option<u64>) -> String {
    let mut out = String::new();

    out.push_str(&format!("Store: {}\n", store.display()));
    out.push_str(&format!(
        "Total: {} ({} bytes) across {} objects\n",
        human_size(report.total_bytes),
        report.total_bytes,
        report.total_count,
    ));

    if report.total_count == 0 {
        // Addition 2: the sidecar-bytes disclosure would be pure noise on
        // an empty store, so it (and the breakdown / oldest-newest
        // sections) is omitted entirely.
        if let Some(budget) = budget {
            out.push_str(&budget_line(0, budget));
        }
        return out;
    }

    // Addition 2: `EvictCandidate.size` is the blob's own byte length —
    // `.meta.json` / `.provenance.json` sidecars are real disk usage this
    // total does not include. Say so, every time there's a nonzero total to
    // qualify.
    out.push_str(
        "(artifact bytes only; .meta.json / .provenance.json sidecars are not counted)\n",
    );

    out.push('\n');
    out.push_str("By kind:\n");
    for (label, bytes, count) in &report.by_kind {
        out.push_str(&format!(
            "  {label}: {count} objects, {}\n",
            human_size(*bytes)
        ));
    }

    out.push('\n');
    out.push_str("By namespace:\n");
    let ns_total = report.by_namespace.len();
    for (ns, bytes, count) in report.by_namespace.iter().take(MAX_NAMESPACE_ROWS) {
        let label = if ns.is_empty() { "<no sidecar>" } else { ns.as_str() };
        out.push_str(&format!(
            "  {label}: {count} objects, {}\n",
            human_size(*bytes)
        ));
    }
    if ns_total > MAX_NAMESPACE_ROWS {
        out.push_str(&format!("  … and {} more\n", ns_total - MAX_NAMESPACE_ROWS));
    }

    out.push('\n');
    if let Some(oldest) = report.oldest {
        out.push_str(&format!("Oldest: {}\n", format_ts(oldest)));
    }
    if let Some(newest) = report.newest {
        out.push_str(&format!("Newest: {}\n", format_ts(newest)));
    }

    if let Some(budget) = budget {
        out.push_str(&budget_line(report.total_bytes, budget));
    }

    out
}

/// `Budget: <used> of <budget> (<pct>% used)`, with an ` — OVER BUDGET by
/// <overage>` suffix when `total` exceeds `budget`.
fn budget_line(total: u64, budget: u64) -> String {
    let pct = percent_used(total, budget);
    let mut line = format!(
        "Budget: {} of {} ({pct}% used)",
        human_size(total),
        human_size(budget),
    );
    if total > budget {
        line.push_str(&format!(" — OVER BUDGET by {}", human_size(total - budget)));
    }
    line.push('\n');
    line
}

fn percent_used(total: u64, budget: u64) -> u64 {
    if budget == 0 {
        return if total == 0 { 0 } else { 100 };
    }
    ((total as f64 / budget as f64) * 100.0).round() as u64
}

/// Decimal-SI (1000-based) human size formatter: B, kB, MB, GB, TB. One
/// shared formatter for every size in the report, so the figures in `Total:`,
/// `By kind:`, `By namespace:`, and `Budget:` are all on the same scale.
fn human_size(bytes: u64) -> String {
    const KB: f64 = 1_000.0;
    const MB: f64 = 1_000_000.0;
    const GB: f64 = 1_000_000_000.0;
    const TB: f64 = 1_000_000_000_000.0;
    if bytes < 1_000 {
        return format!("{bytes} B");
    }
    let b = bytes as f64;
    if b < MB {
        format!("{:.1} kB", b / KB)
    } else if b < GB {
        format!("{:.1} MB", b / MB)
    } else if b < TB {
        format!("{:.1} GB", b / GB)
    } else {
        format!("{:.1} TB", b / TB)
    }
}

/// Render a Unix-seconds timestamp as `YYYY-MM-DDTHH:MM:SSZ`, reusing
/// `crate::iso8601::days_to_ymd` rather than pulling in `chrono`.
fn format_ts(secs: u64) -> String {
    let days = secs / 86_400;
    let rem = secs % 86_400;
    let hour = rem / 3600;
    let min = (rem % 3600) / 60;
    let sec = rem % 60;
    let (year, month, day) = crate::iso8601::days_to_ymd(days as i64);
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{min:02}:{sec:02}Z")
}

#[cfg(test)]
#[path = "tests/cache_du_tests.rs"]
mod tests;
