//! Pure eviction-planning policy for the CAS artifact store (`cook cache gc`,
//! CAS-hygiene milestone).
//!
//! `plan_eviction` decides WHAT to evict; it never touches a filesystem,
//! never enumerates a store, and never reads the clock — `now` is a
//! parameter precisely so Cook Cloud can reuse this exact policy
//! server-side, where a client is not permitted to enumerate or delete
//! (milestone D2). Enumeration (`LocalBackend::enumerate`) and execution
//! (`LocalBackend::apply_eviction`) stay backend-specific; only the policy
//! is shared.
//!
//! ## Two independent passes
//!
//! 1. **Age pass** (`older_than`): applies to EVERY kind, including the
//!    size-sweep-exempt kinds below. A candidate is an age victim when
//!    `last_access < cutoff`, where `cutoff = now.saturating_sub(older_than)`
//!    (strictly older; exactly-at-cutoff survives). This is the only knob
//!    that bounds the exempt set, since the size pass never touches it.
//!
//!    `last_access == 0` is never an age victim: zero is `enumerate`'s
//!    "platform mtime unavailable" sentinel, not a timestamp, and the age
//!    sweep is asserting an object is PROVABLY old — it cannot make that
//!    assertion for an unknown last-access time. Such an object remains
//!    fully eligible for the size sweep, where it naturally sorts first
//!    (ascending by `last_access`) — safe by construction, since a wrong
//!    eviction costs one rebuild.
//!
//! 2. **Size pass** (`max_size`): runs second, over whatever the age pass
//!    did not already take, and evicts non-exempt candidates only (`kind:
//!    None` is the legacy file case, per `ArtifactMeta::kind`'s doc, and is
//!    NOT exempt — nor is any unrecognised kind string). Candidates are
//!    evicted ascending by `last_access` (ties broken by `key`, so the plan
//!    is deterministic regardless of the candidates' enumeration order)
//!    until the running total is `<= target`, where
//!    `target = max_size as f64 * low_water`. The running total starts at
//!    `total_before` MINUS whatever the age pass already freed, so a
//!    combined run whose age pass already cleared the budget evicts nothing
//!    further. If the exempt bytes alone exceed the target, every eligible
//!    (non-exempt) candidate is evicted and the sweep stops there — no
//!    panic, no infinite loop; `total_after` honestly reports a value still
//!    above `max_size`.
//!
//! ## Why kinds are exempt from the size sweep (milestone D1)
//!
//! Depfile units use a two-level fetch (`check.rs`) where a manifest
//! (`discovered_input_sets` / `discovered_inputs`) is the only route to its
//! artifacts' full keys. Evicting the manifest corrupts nothing but
//! STRANDS every artifact reachable only through it: the bytes stay on
//! disk, unreachable, until a rebuild republishes the manifest. Since
//! manifests average ~9 KB and artifacts average ~575 KB, a byte-hungry
//! LRU is exactly the policy that causes this. On the measured 11 GB store
//! the exempt kinds are 36% of objects but only 0.9% of bytes, so exempting
//! them from the size sweep is nearly free. `older_than` is what keeps the
//! exempt set bounded, since the age pass applies to every kind (above).
//!
//! ## Carry-forward note from COOK-233
//!
//! `LocalBackend::get_manifest` does not touch, so an artifact consulted
//! only through provenance lookups (`cook why`) still ages out as cold.
//! That is correct for gc, not a design gap: a provenance read is a
//! diagnostic, not a cache hit — the bytes were never reused, so evicting
//! them costs one rebuild on next real use, exactly the LRU bargain.
//! `delete` removes the `.provenance.json` sidecar alongside the blob, so
//! `cook why` loses provenance for an object whose bytes are gone anyway —
//! coherent, not a new gap.

use std::collections::HashSet;
use std::time::Duration;

use crate::backend::{CloudKey, EvictCandidate};

/// Kinds the size-driven sweep must never evict (milestone D1).
pub const SIZE_SWEEP_EXEMPT_KINDS: &[&str] = &[
    "discovered_input_sets",
    "discovered_inputs",
    "probe_value",
    "symlink",
    "dir",
];

/// Low-water fraction COOK-235's `auto_gc` sweeps to. Manual `cook cache gc
/// --max-size N` uses 1.0 (evict to exactly N, per the ticket).
pub const DEFAULT_LOW_WATER: f64 = 0.8;

/// Is `kind` one of [`SIZE_SWEEP_EXEMPT_KINDS`]? `None` (the legacy file
/// case) is NOT exempt. Comparison is exact and case-sensitive; an
/// unrecognised kind string is not exempt either.
pub fn is_size_sweep_exempt(kind: Option<&str>) -> bool {
    match kind {
        Some(k) => SIZE_SWEEP_EXEMPT_KINDS.contains(&k),
        None => false,
    }
}

/// Eviction policy knobs. Pure data; [`plan_eviction`] is the only
/// consumer.
#[derive(Debug, Clone)]
pub struct EvictPolicy {
    /// Sweep the store down to (a fraction of) this many bytes. `None`
    /// disables the size pass entirely.
    pub max_size: Option<u64>,
    /// Evict anything with `last_access` strictly older than
    /// `now - older_than` (`last_access == 0` excepted; see module doc).
    /// `None` disables the age pass entirely.
    pub older_than: Option<Duration>,
    /// Fraction of `max_size` the size pass sweeps down to. `1.0` evicts to
    /// exactly `max_size`. Values below `1.0` (e.g. [`DEFAULT_LOW_WATER`])
    /// sweep past `max_size`, leaving headroom for COOK-235's `auto_gc`.
    ///
    /// Expected range is `0.0..=1.0`; `plan_eviction` `debug_assert!`s this
    /// in debug builds. In a release build an out-of-range value is not
    /// validated: NaN or negative saturate to a `0` target (evict everything
    /// eligible), and a value above `1.0` under-sweeps (targets above
    /// `max_size`). No panic, no UB either way.
    pub low_water: f64,
}

impl EvictPolicy {
    /// Manual `cook cache gc`: sweep to exactly `max_size` (`low_water = 1.0`).
    pub fn manual(max_size: Option<u64>, older_than: Option<Duration>) -> Self {
        Self {
            max_size,
            older_than,
            low_water: 1.0,
        }
    }

    /// Automatic end-of-run sweep: sweep to [`DEFAULT_LOW_WATER`] (0.8) of
    /// `max_size`, leaving headroom so the next few publishing runs do not
    /// re-trigger a sweep. Age is not part of the automatic sweep
    /// (`older_than: None`); `--older-than` stays a deliberate manual act.
    ///
    /// `max_size` is a bare `u64`, not `Option<u64>`: the automatic sweep
    /// only ever runs when a budget is configured, so there is no "no
    /// budget" case to represent here.
    pub fn auto(max_size: u64) -> Self {
        Self {
            max_size: Some(max_size),
            older_than: None,
            low_water: DEFAULT_LOW_WATER,
        }
    }
}

/// The result of applying an [`EvictPolicy`] to a candidate list. Pure data;
/// actually deleting `victims` is the caller's job (`LocalBackend::apply_eviction`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EvictPlan {
    /// The objects to drop, in the order the policy chose them: age victims
    /// first, then size/LRU victims — both groups ordered ascending by
    /// `last_access` (ties broken by `key`) so the plan is deterministic
    /// regardless of the input candidates' enumeration order.
    pub victims: Vec<EvictCandidate>,
    /// Sum of `victims[..].size`.
    pub freed_bytes: u64,
    /// Sum of ALL candidates' sizes, exempt included: the budget is store
    /// size, not evictable-store size.
    pub total_before: u64,
    /// `total_before - freed_bytes` (saturating).
    pub total_after: u64,
    /// Count of all candidates, exempt included.
    pub count_before: usize,
}

/// Sort key shared by both passes: ascending `last_access`, ties broken by
/// `key` ascending. This is what makes the plan deterministic regardless of
/// the order `candidates` arrives in.
fn eviction_order(c: &EvictCandidate) -> (u64, CloudKey) {
    (c.last_access, c.key)
}

/// Pure: no I/O, no filesystem, no clock. `now` is Unix seconds, supplied by
/// the caller so the policy is testable and reusable server-side (milestone
/// D2). See the module doc for the two-pass algorithm.
pub fn plan_eviction(candidates: &[EvictCandidate], policy: &EvictPolicy, now: u64) -> EvictPlan {
    debug_assert!(
        (0.0..=1.0).contains(&policy.low_water),
        "EvictPolicy::low_water must be in 0.0..=1.0, got {}",
        policy.low_water
    );

    let total_before: u64 = candidates.iter().fold(0u64, |a, c| a.saturating_add(c.size));
    let count_before = candidates.len();

    let mut victims: Vec<EvictCandidate> = Vec::new();
    let mut evicted: HashSet<CloudKey> = HashSet::new();

    // --- Age pass: every kind, exempt included ----------------------------
    if let Some(older_than) = policy.older_than {
        let cutoff = now.saturating_sub(older_than.as_secs());
        let mut age_victims: Vec<&EvictCandidate> = candidates
            .iter()
            .filter(|c| c.last_access != 0 && c.last_access < cutoff)
            .collect();
        age_victims.sort_by_key(|c| eviction_order(c));
        for c in age_victims {
            evicted.insert(c.key);
            victims.push(c.clone());
        }
    }

    let freed_by_age: u64 = victims.iter().fold(0u64, |a, c| a.saturating_add(c.size));
    let mut running_total = total_before.saturating_sub(freed_by_age);

    // --- Size pass: non-exempt candidates the age pass didn't already take
    if let Some(max_size) = policy.max_size {
        let target = (max_size as f64 * policy.low_water) as u64;
        let mut eligible: Vec<&EvictCandidate> = candidates
            .iter()
            .filter(|c| !evicted.contains(&c.key) && !is_size_sweep_exempt(c.kind.as_deref()))
            .collect();
        eligible.sort_by_key(|c| eviction_order(c));
        for c in eligible {
            if running_total <= target {
                break;
            }
            running_total = running_total.saturating_sub(c.size);
            victims.push(c.clone());
        }
    }

    let freed_bytes: u64 = victims.iter().fold(0u64, |a, c| a.saturating_add(c.size));
    let total_after = total_before.saturating_sub(freed_bytes);

    EvictPlan {
        victims,
        freed_bytes,
        total_before,
        total_after,
        count_before,
    }
}

#[cfg(test)]
#[path = "tests/evict_tests.rs"]
mod tests;
