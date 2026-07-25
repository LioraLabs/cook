//! `cook cache gc` — evict least-recently-used or stale artifacts from the
//! local CAS store.
//!
//! Read-only project-root resolution, exactly like `cook cache du`: no
//! workspace registration, so `gc` works even when the Cookfile no longer
//! parses. `render_dry_run` and `render_applied` are pure functions over an
//! `EvictPlan` / `EvictOutcome` — no I/O — so the output is unit-testable
//! without a filesystem. `cmd_cache_gc` is the thin I/O shell: resolve the
//! project root and config, parse the flags, enumerate the local CAS (unless
//! it doesn't exist), plan the sweep, apply it unless `--dry-run`, print.
//!
//! `SystemTime::now()` is read exactly once, here, so `plan_eviction` (in
//! `cook-fingerprint`) stays pure and reusable server-side.

use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use cook_engine::cook_cache::backend::{plan_eviction, EvictOutcome, EvictPlan, EvictPolicy, LocalBackend};
use cook_engine::cook_cache::{parse_size, CloudConfig, SIZE_LITERAL_HELP};

use crate::cache_du::human_size;
use crate::cli::{CacheGcArgs, Globals};
use crate::error::CookError;
use crate::pipeline::resolve_project_root;

/// `cook cache gc`. Resolves the project root exactly like `cmd_cache_du`
/// (no workspace registration), loads `.cook/cloud.toml`, and either defers
/// to the server-managed cloud cache or sweeps the local CAS per the
/// requested policy.
pub fn cmd_cache_gc(globals: &Globals, args: &CacheGcArgs) -> Result<(), CookError> {
    let project_root = resolve_project_root(globals)?;
    let cloud_config = CloudConfig::load_or_default(&project_root)
        .map_err(|e| CookError::Other(format!("invalid .cook/cloud.toml: {e}")))?;

    if cloud_config.cloud.enabled {
        println!("cloud cache is server-managed; gc operates on the local store");
        return Ok(());
    }

    if args.max_size.is_none() && args.older_than.is_none() {
        return Err(usage_error());
    }

    let max_size = args
        .max_size
        .as_deref()
        .map(parse_max_size_flag)
        .transpose()?;
    let older_than = args
        .older_than
        .as_deref()
        .map(parse_older_than_flag)
        .transpose()?;

    // Same resolution `build_cache_ctx` and `du` use, so `gc` can never
    // sweep a different directory than the one the engine actually writes
    // to.
    let store = cloud_config.resolved_cache_dir();

    // The only clock read in the feature; kept here so `plan_eviction`
    // stays pure and reusable server-side.
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    // `--max-size N` evicts to exactly N (low_water 1.0), per the ticket.
    let policy = EvictPolicy::manual(max_size, older_than);

    // `gc` must never create the store as a side effect of merely
    // inspecting it: `LocalBackend::with_config` (which `LocalBackend::new`
    // calls) unconditionally `create_dir_all`s the root, so a missing store
    // is handled here, BEFORE constructing a backend at all — mirroring
    // `du`'s identical early return. A missing store is a zero-work report,
    // not an error.
    if !store.exists() {
        let plan = plan_eviction(&[], &policy, now);
        print_report(&store, &plan, args.dry_run, None);
        return Ok(());
    }

    let backend = LocalBackend::new(store.clone());
    let candidates = backend.enumerate().map_err(|e| {
        CookError::Other(format!(
            "enumerating cache store {}: {e}",
            store.display()
        ))
    })?;

    let plan = plan_eviction(&candidates, &policy, now);

    if args.dry_run || plan.victims.is_empty() {
        // Nothing to apply either because the caller asked for a dry run,
        // or because the plan chose no victims — either way, no delete, so
        // there is no real `EvictOutcome` to report.
        print_report(&store, &plan, args.dry_run, None);
        return Ok(());
    }

    let outcome = backend.apply_eviction(&plan).map_err(|e| {
        CookError::Other(format!(
            "evicting from cache store {}: {e}",
            store.display()
        ))
    })?;
    print_report(&store, &plan, false, Some(&outcome));
    Ok(())
}

/// Print either the dry-run projection or the real-sweep report, per
/// `dry_run`. Kept as a one-line dispatcher so `cmd_cache_gc` doesn't repeat
/// the `if` at each of its call sites.
///
/// `outcome` is `None` whenever nothing was actually applied (dry run, empty
/// plan, or a missing store) — there is no real `EvictOutcome` to fabricate
/// in those cases, so the "nothing happened" report is rendered directly
/// rather than handed a placeholder value that a renderer must know to
/// ignore.
fn print_report(store: &Path, plan: &EvictPlan, dry_run: bool, outcome: Option<&EvictOutcome>) {
    if dry_run {
        print!("{}", render_dry_run(store, plan));
    } else if let Some(outcome) = outcome {
        print!("{}", render_applied(store, plan, outcome));
    } else {
        print!("{}", render_nothing_to_evict(store));
    }
}

/// The neither-`--max-size`-nor-`--older-than` usage error. A pure function
/// (no I/O) so its message is unit-testable directly.
fn usage_error() -> CookError {
    CookError::Other(
        "cook cache gc requires --max-size or --older-than (e.g. `cook cache gc --max-size 10GB` \
         or `cook cache gc --older-than 30d`)"
            .to_string(),
    )
}

/// Parse `--max-size` via the same `parse_size` that backs `.cook/cloud.toml`'s
/// `[cache] max_size` — one accepted-forms vocabulary for a byte budget,
/// whether it's typed in a config file or on this command line.
fn parse_max_size_flag(literal: &str) -> Result<u64, CookError> {
    parse_size(literal).ok_or_else(|| {
        CookError::Other(format!(
            "invalid --max-size {literal:?}: expected {SIZE_LITERAL_HELP}, \
             e.g. \"10GB\" or \"500MB\""
        ))
    })
}

/// Parse `--older-than` with `humantime::parse_duration`, which already
/// accepts the ticket's `30d` / `720h` forms.
fn parse_older_than_flag(literal: &str) -> Result<Duration, CookError> {
    humantime::parse_duration(literal).map_err(|e| {
        CookError::Other(format!(
            "invalid --older-than {literal:?}: {e} (expected a duration like \"30d\" or \"720h\")"
        ))
    })
}

/// The shared "no victims" report: an empty plan means the same thing
/// whether it came from a dry run, a real sweep, or a missing store, so
/// there is exactly one place that spells out its two lines.
pub(crate) fn render_nothing_to_evict(store: &Path) -> String {
    format!("Store: {}\nNothing to evict.\n", store.display())
}

/// Render the `--dry-run` projection: `Would free: …` sourced from
/// `plan.freed_bytes` / `plan.victims.len()`, since nothing has actually been
/// deleted yet — the plan IS the whole story for a dry run.
pub(crate) fn render_dry_run(store: &Path, plan: &EvictPlan) -> String {
    if plan.victims.is_empty() {
        return render_nothing_to_evict(store);
    }
    let mut out = format!("Store: {}\n", store.display());
    out.push_str(&format!(
        "Would free: {} objects, {}\n",
        plan.victims.len(),
        human_size(plan.freed_bytes),
    ));
    out.push_str(&store_transition_line(
        plan.total_before,
        plan.total_after,
        plan.count_before,
        plan.count_before.saturating_sub(plan.victims.len()),
    ));
    out
}

/// Render the post-sweep report: `Freed: …` and the store-transition line are
/// both sourced from `outcome` (ground truth as of the actual filesystem
/// walk), not from `plan.freed_bytes` / `plan.total_after` (the pre-sweep
/// projection) — a victim removed by a concurrent sweep is planned but not
/// counted, and this must report what actually happened, never a
/// consistency check between the two. `plan.total_before` / `count_before`
/// are the enumeration snapshot taken before any deletion and are safe to
/// reuse here: they don't depend on which victims the policy chose.
pub(crate) fn render_applied(store: &Path, plan: &EvictPlan, outcome: &EvictOutcome) -> String {
    if plan.victims.is_empty() {
        return render_nothing_to_evict(store);
    }
    let mut out = format!("Store: {}\n", store.display());
    out.push_str(&format!(
        "Freed: {} objects, {}\n",
        outcome.objects,
        human_size(outcome.bytes),
    ));
    out.push_str(&store_transition_line(
        plan.total_before,
        plan.total_before.saturating_sub(outcome.bytes),
        plan.count_before,
        plan.count_before.saturating_sub(outcome.objects),
    ));
    out
}

fn store_transition_line(
    before_bytes: u64,
    after_bytes: u64,
    before_count: usize,
    after_count: usize,
) -> String {
    format!(
        "Store: {} -> {} ({} objects -> {})\n",
        human_size(before_bytes),
        human_size(after_bytes),
        before_count,
        after_count,
    )
}

#[cfg(test)]
#[path = "tests/cache_gc_tests.rs"]
mod tests;
