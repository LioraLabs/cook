//! The pure rules a cache record is judged by: what a unit's declaration
//! admits, and which of its determinants moved.
//!
//! Everything here is a total function of its arguments. No filesystem, no
//! clock, no backend. That is the whole reason these rules live in this crate
//! rather than beside the code that stats and hashes: the judgment is shared
//! by every store, while observing the world and repairing it are not.
//!
//! Deliberately absent: the record itself. `cook_fingerprint::StepEntry` is
//! the recorded shape and `cook_cache` encodes it, because a record is only
//! meaningful next to the I/O that writes and reads it. This module held a
//! prototype `UnitRecord` and a wire format for it (COOK-360); CS-0186
//! withdrew the observation that was to be their reason for existing, at which
//! point the prototype was `StepEntry` with a key field, and it is deleted
//! rather than left as a second answer to a question already answered.
//!
//! The one invariant that prototype enforced — an observing unit must not have
//! recorded outputs — is not lost with it. It is enforced where a record is
//! read back, by the output-count comparison in
//! `cook_fingerprint::needs_rebuild_cook`: a declaration with no outputs meets
//! a record carrying some, the counts differ, and the unit rebuilds instead of
//! replaying. A named function asserting the same thing would have no caller.

use super::CacheMeta;

/// Whether a unit's cache hit restores bytes or replays a verdict.
///
/// Derived from the declaration (see [`effect_kind`]), never stored on a
/// record. Storing it would let the evidence contradict the declaration, and
/// there would then be no ground on which to prefer either (§17.1.1.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EffectKind {
    /// The unit declares output paths; a hit restores their bytes.
    Produced,
    /// The unit declares no output paths; a hit replays its verdict.
    Observed,
}

/// The declaration decides the effect kind. Pure, total, no I/O.
///
/// This is not a test-vs-cook distinction (CS-0186). A cacheable unit with no
/// declared outputs already exists outside tests — `build_local_cache_key` has
/// carried an empty-output branch since long before this rule was written down
/// — so the rule generalises an existing case rather than special-casing a
/// step kind.
pub fn effect_kind(meta: &CacheMeta) -> EffectKind {
    if meta.output_paths.is_empty() {
        EffectKind::Observed
    } else {
        EffectKind::Produced
    }
}

/// What kind of cache participation a unit's declaration admits.
///
/// Three states that the reference implementation long expressed as two, by
/// letting a missing [`CacheMeta`] stand for both "not cacheable" and "has no
/// outputs". Those are different claims: a chore body may never be cached at
/// all (§7.4), while a unit that declares no outputs is perfectly cacheable —
/// its hit simply replays a recorded verdict instead of restoring bytes.
///
/// Keeping them apart is also what lets a step kind be added later whose units
/// are output-less WITHOUT being tests: it is [`crate::StepKind`], not the
/// output list, that says how a unit is reported (§8.6, CS-0186).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cacheability {
    /// Never cached. Chore bodies and interactive units.
    Uncacheable,
    /// Cached, with nothing in the artifact store: a hit replays the recorded
    /// verdict. Every unit declaring no outputs.
    ResultOnly,
    /// Cached, with artifacts to publish and restore.
    Artifacts,
}

/// The three-state rule, as a pure function over the declaration.
///
/// Defined in terms of [`effect_kind`] rather than repeating it: absence of a
/// declaration is the only thing this adds.
pub fn cacheability(meta: Option<&CacheMeta>) -> Cacheability {
    match meta {
        None => Cacheability::Uncacheable,
        Some(m) => match effect_kind(m) {
            EffectKind::Observed => Cacheability::ResultOnly,
            EffectKind::Produced => Cacheability::Artifacts,
        },
    }
}

/// The three values whose movement invalidates a record, independent of any
/// file on disk.
///
/// Deliberately does NOT carry the cache key. Whether a store handed back the
/// record that was asked for is a question about the STORE, and it is a
/// different question with a different meaning when it fails. Conflating them
/// was tempting because both are cheap comparisons, but Cook's local index
/// keys on unit IDENTITY (§17.1.1.1) rather than on content, so a key match
/// there implies nothing about determinants and a determinant match implies
/// nothing about the key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Determinants {
    command_hash: u64,
    env_contribution: u64,
    seal_contribution: u64,
}

impl Determinants {
    pub fn new(command_hash: u64, env_contribution: u64, seal_contribution: u64) -> Self {
        Self { command_hash, env_contribution, seal_contribution }
    }

    pub fn command_hash(&self) -> u64 {
        self.command_hash
    }

    pub fn env_contribution(&self) -> u64 {
        self.env_contribution
    }

    pub fn seal_contribution(&self) -> u64 {
        self.seal_contribution
    }
}

/// Which determinant moved out from under a record.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeterminantDrift {
    CommandHash,
    Env,
    Seal,
}

/// The judgment every store shares, as a pure function.
///
/// Returns the first determinant that moved, or `None` when nothing did. Says
/// nothing whatever about files on disk: that is an OBSERVATION, and
/// observation belongs to the crates that own I/O. This is the judge half of
/// observe / judge / repair.
///
/// Order is command, env, seal — the live implementation's order — so the
/// reported cause stays stable now that this has replaced the hand-rolled
/// chains.
pub fn determinant_drift(
    stored: &Determinants,
    current: &Determinants,
) -> Option<DeterminantDrift> {
    if stored.command_hash != current.command_hash {
        return Some(DeterminantDrift::CommandHash);
    }
    if stored.env_contribution != current.env_contribution {
        return Some(DeterminantDrift::Env);
    }
    if stored.seal_contribution != current.seal_contribution {
        return Some(DeterminantDrift::Seal);
    }
    None
}

/// Bumped whenever the recorded shape or its meaning changes.
///
/// Continues `cook_fingerprint::record::CACHE_VERSION` rather than starting a
/// fresh counter, so the existing invalidate-on-bump path keeps working and a
/// new number cannot collide with an index already on disk. Superseded records
/// are DELETED, not migrated (CS-0166); nothing reads an older shape.
///
/// There were two of these. This counter versioned the step index while the
/// removed test-result store carried its own, so two stores holding the same
/// kind of thing were free to disagree about what shape it had, and a change
/// to one could not invalidate the other. One store now, and one version.
pub const RECORD_SCHEMA_VERSION: u32 = 8;

#[cfg(test)]
#[path = "tests/record_tests.rs"]
mod tests;
