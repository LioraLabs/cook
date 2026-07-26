//! End-of-run CAS store-budget check (COOK-235, milestones D4/D5).
//!
//! This is where the four inputs the rest of the ticket built meet: the
//! `[cache] max_size` / `auto_gc` config knobs, the reusable
//! `enumerate_store` / `sweep` seam `cook cache gc` is built on,
//! `EvictPolicy::auto`'s low-water sweep, and the engine's per-run CAS
//! publish count. A publishing run that leaves the local store over its
//! configured budget either warns or (opt-in) sweeps.
//!
//! Three properties hold the design together:
//!
//! 1. **It never fails the build.** Every failure mode below — an unreadable
//!    config, a typo'd budget literal, an unwalkable store, a sweep that
//!    errors — degrades to a `cook: warning:` line and returns. The run
//!    already succeeded; cache hygiene is not allowed to retroactively fail
//!    it.
//! 2. **It costs a settled no-op build nothing.** The `published_count == 0`
//!    short-circuit comes before the config load and before the store walk,
//!    so the overwhelmingly common "nothing to do" build does no extra I/O
//!    at all.
//! 3. **It is stateless** (milestone D5): no stamp file, no rate limiting.
//!    Every over-budget publishing run reports. A budget the user set and
//!    then blew past should not go quiet just because it was mentioned once.
//!
//! `render_budget_warning`, `render_auto_gc_report`, and `largest_namespace`
//! are pure — no filesystem, no clock — so the output is unit-testable
//! directly, exactly as in `cache_du` and `cache_gc`.
//! `check_budget_after_run` is the thin I/O shell around them, and it is the
//! only place `SystemTime::now()` is read, keeping `plan_eviction` pure.

use std::collections::BTreeMap;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use cook_engine::cook_cache::backend::{EvictCandidate, EvictOutcome, EvictPolicy};
use cook_engine::cook_cache::CloudConfig;

use crate::cache_du::{human_size, percent_used};
use crate::cache_gc::{enumerate_store, sweep, Sweep};

/// The end-of-run budget check. Called from `run_with_progress` and
/// `cmd_test` after the progress renderer has released the terminal, so
/// these lines cannot fight the progress display.
///
/// Never returns an error and never panics-by-design: this runs after a
/// successful build, and a cache-hygiene problem must not turn a green run
/// red. Every early return below is silent unless its comment says
/// otherwise.
///
/// `no_auto_gc` is `pipeline::no_auto_gc_enabled(globals)` — the
/// `--no-auto-gc` / `COOK_NO_AUTO_GC` escape hatch. It suppresses only the
/// deletion, never the warning, so a CI job can stay deterministic while its
/// operator still learns the store needs a sweep.
pub(crate) fn check_budget_after_run(project_root: &Path, published_count: u64, no_auto_gc: bool) {
    // 1. This run published no outputs, so there is nothing new to account
    //    for. Deliberately BEFORE the config load and before the store walk:
    //    that ordering is the requirement, not an implementation detail — it
    //    is what keeps a settled no-op build at zero added cost.
    //
    //    Note the gate is "published no outputs", not "the store did not
    //    grow": pre-pass probe values are written outside the publish
    //    counter (COOK-339), so a `published_count == 0` run can still add a
    //    handful of small objects.
    //
    //    Be precise about what that costs, because the honest bound is
    //    weaker than "one build late". The check is stateless by design
    //    (step 9's comment, milestone D5): only a publishing run reports. A
    //    store already over budget therefore stays quiet for as long as
    //    subsequent runs publish nothing, which on a settled tree is
    //    indefinitely. That is the accepted trade for never walking the
    //    store on a build that did nothing; `cook cache du` is the
    //    on-demand way to ask regardless. See `RunResult::published_count`,
    //    which states the same bound.
    if published_count == 0 {
        return;
    }

    // 2. The config already parsed once during this run, so a failure here
    //    is a transient read error at worst — and whatever the engine said
    //    about it, it said first. Do not double-report.
    let Ok(cloud) = CloudConfig::load_or_default(project_root) else {
        return;
    };

    // 3. LocalBackend only. A server-managed cloud cache has its own
    //    retention, and this process has no authority over it — same early
    //    return `cache_du.rs` and `cache_gc.rs` make.
    if cloud.cloud.enabled {
        return;
    }

    let budget = match cloud.max_size_bytes() {
        // 4. A typo must never silently mean "no budget". `BadMaxSize`'s
        //    `Display` already embeds `SIZE_LITERAL_HELP`, so the error's own
        //    message is the whole diagnostic — appending the help text again
        //    would print the accepted-forms list twice.
        Err(e) => {
            eprintln!("cook: warning: {e}");
            return;
        }
        // 5. No budget configured is today's behaviour: no walk, no warning.
        //    The whole feature is opt-in on this one field.
        Ok(None) => return,
        Ok(Some(budget)) => budget,
    };

    // 6. The same resolution `build_cache_ctx`, `du`, and `gc` use, so the
    //    check can never measure a different directory than the one this
    //    build just wrote to.
    let store = cloud.resolved_cache_dir();
    let candidates = match enumerate_store(&store) {
        Ok(Some(candidates)) => candidates,
        // A missing store is zero work, not an error (and `enumerate_store`
        // deliberately does not create it).
        Ok(None) => return,
        Err(e) => {
            eprintln!("cook: warning: cache budget check failed: {e}");
            return;
        }
    };

    // 7. The budget is store size, not evictable-store size: exempt kinds
    //    count toward the total exactly as they do in `EvictPlan.total_before`
    //    and in `cook cache du`. Strictly over, per the ticket — a store
    //    sitting exactly at its budget is within it.
    let total = candidates.iter().fold(0u64, |a, c| a.saturating_add(c.size));
    if total <= budget {
        return;
    }

    // The literal the user typed, echoed verbatim into the remediation
    // command so the line is paste-able. `max_size_literal()` is `Some`
    // whenever `max_size_bytes()` returned `Ok(Some(_))` — the fallback is
    // unreachable, and renders the budget as a bare byte count, which
    // `parse_size` accepts, rather than a `human_size` string containing a
    // space that would need quoting.
    let literal = cloud
        .max_size_literal()
        .map_or_else(|| budget.to_string(), str::to_string);

    // 8. Over budget with `auto_gc` opted in (and not overridden for this
    //    run): sweep to the low-water mark instead of only complaining.
    if cloud.auto_gc() && !no_auto_gc {
        // The only clock read on this path, kept in the I/O shell so
        // `plan_eviction` stays pure — the same discipline `cmd_cache_gc`
        // follows.
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);

        match sweep(&store, &candidates, &EvictPolicy::auto(budget), now, false) {
            // A gc failure must never fail the build: report and move on.
            Err(e) => {
                eprintln!("cook: warning: cache gc failed: {e}");
                return;
            }
            Ok(Sweep {
                outcome: Some(outcome),
                ..
            }) => {
                eprint!("{}", render_auto_gc_report(total, budget, &outcome));
                return;
            }
            // The plan chose no victims, so nothing was swept and the store
            // is still over budget. Reachable, not theoretical: a store whose
            // bytes are all `SIZE_SWEEP_EXEMPT_KINDS` plans zero victims by
            // design, and the automatic sweep simply cannot help it. Fall
            // through to the warning below — staying silent here would leave
            // such a store over budget forever with no signal at all.
            Ok(Sweep { outcome: None, .. }) => {}
        }
    }

    // 9. Over budget and nothing swept it: either `auto_gc` is off (or
    //    overridden for this run), or the sweep above found no victims.
    eprint!(
        "{}",
        render_budget_warning(total, budget, &literal, largest_namespace(&candidates))
    );
}

/// The over-budget warning block. Pure — no filesystem, no config.
///
/// Every line carries cook's `cook: ` stderr prefix and the continuation
/// lines are indented to align under it: every other cook warning is
/// prefixed and line-oriented, and a mixed-prefix multi-line block reads
/// worse than a consistent one.
///
/// `total` and `budget` both go through `cache_du::human_size` and the
/// percentage through `cache_du::percent_used` — one shared vocabulary for
/// CAS sizes across `du`, `gc`, and this check, with no second formatter and
/// no second rounding rule.
///
/// The remediation command echoes `max_size_literal` verbatim rather than a
/// re-rendered `human_size(budget)`: the user typed `20GB` and must be able
/// to paste the line as printed. The reclaim figure is `total - budget`,
/// which is exactly what that command frees (`EvictPolicy::manual` sweeps at
/// `low_water = 1.0`) — NOT the larger figure the automatic low-water sweep
/// would free. The printed command and the printed number have to agree.
pub(crate) fn render_budget_warning(
    total: u64,
    budget: u64,
    max_size_literal: &str,
    largest: Option<(&str, u64)>,
) -> String {
    let mut out = format!(
        "cook: warning: cache store is {}, over the {} budget ({}%)\n",
        human_size(total),
        human_size(budget),
        percent_used(total, budget),
    );
    out.push_str(&format!(
        "cook:          run `cook cache gc --max-size {max_size_literal}` to reclaim {}\n",
        human_size(total.saturating_sub(budget)),
    ));
    if let Some((namespace, bytes)) = largest {
        // `recipe_namespace` is `""` when no sidecar was readable, which is
        // the common case on a real store rather than an edge case, so it
        // gets a label of its own instead of rendering as a bare size.
        let label = if namespace.is_empty() {
            "(unattributed)"
        } else {
            namespace
        };
        out.push_str(&format!(
            "cook:          largest: {label} {} ({}%)\n",
            human_size(bytes),
            // Share of the store, not of the budget: this line answers
            // "what is filling it up", and a percentage of the budget would
            // be a second, easily-confused reading of the same bytes.
            percent_used(bytes, total),
        ));
    }
    out
}

/// The post-sweep one-liner. Pure.
///
/// `before` is the enumeration total; the freed figure and therefore the
/// after figure both come from `outcome` — ground truth as of the actual
/// filesystem walk — rather than from the plan's projection, the same
/// discipline `cache_gc::render_applied` follows. A victim removed by a
/// concurrent sweep is planned but not counted, and this must report what
/// happened, not what was intended.
///
/// One line, not a block: a sweep is a completed action, and the user's only
/// question is how much went away.
pub(crate) fn render_auto_gc_report(before: u64, budget: u64, outcome: &EvictOutcome) -> String {
    format!(
        "cook: cache store was {}, over the {} budget — swept {} ({} objects), now {}\n",
        human_size(before),
        human_size(budget),
        human_size(outcome.bytes),
        outcome.objects,
        human_size(before.saturating_sub(outcome.bytes)),
    )
}

/// The single `recipe_namespace` accounting for the most bytes, folded over
/// the candidates already in hand. Pure.
///
/// A fold, not `cache_du::summarize`: the warning needs one row, and
/// building the full `du` report (by-kind, by-namespace, oldest/newest, the
/// top-20 cap) to throw all but one line of it away would be waste on a path
/// that runs at the end of every publishing build.
///
/// Ties break on the namespace name ascending — the same tie-break `du`'s
/// sort uses — so the printed line is reproducible regardless of the order
/// `LocalBackend::enumerate` happened to walk the store in.
fn largest_namespace(candidates: &[EvictCandidate]) -> Option<(&str, u64)> {
    let mut by_namespace: BTreeMap<&str, u64> = BTreeMap::new();
    for c in candidates {
        let entry = by_namespace.entry(c.recipe_namespace.as_str()).or_insert(0);
        *entry = entry.saturating_add(c.size);
    }
    by_namespace
        .into_iter()
        // Bytes ascending, then name DESCENDING, so that among equal byte
        // counts the alphabetically-first name compares greatest and is the
        // one `max_by` returns.
        .max_by(|a, b| a.1.cmp(&b.1).then_with(|| b.0.cmp(a.0)))
}

#[cfg(test)]
#[path = "tests/cache_budget_tests.rs"]
mod tests;
