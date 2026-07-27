//! Unit cache records: what one run left behind, and the pure rules that
//! decide whether a record may be replayed.
//!
//! COOK-360 prototype. Nothing wires to this yet — `cook-fingerprint`'s
//! `StepEntry` and `cook-cache`'s `TestCacheEntry` are still the live types.
//! It exists so the shape can be read before any call site moves.
//!
//! The thesis it encodes: a unit that declares outputs and a unit that
//! declares none are the same kind of thing, cached by the same rules. They
//! differ in what a replay GIVES BACK — bytes for one, a verdict for the
//! other — and that difference is derived from the declaration, never stored.
//!
//! Fields are private throughout. That is not ceremony: [`UnitRecord::effect_kind`]
//! reads the record's own output list, and it may only be trusted because
//! [`UnitRecord::record`] is the sole constructor and checks that list against
//! the declaration. With public fields the accessor would be a guess.

use std::sync::Arc;

use super::CacheMeta;

/// One file as the cache observed it.
///
/// `path` is an `Arc<str>` for the same reason `cook_fingerprint`'s record is:
/// on a large C++ graph one header appears in hundreds of input sets, so the
/// decoder allocates once per distinct path and every record naming it clones
/// a pointer. Cloning a `FileRecord` is a refcount bump and two `u64` copies.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileRecord {
    path: Arc<str>,
    mtime: u64,
    hash: u64,
}

impl FileRecord {
    /// Build a record for `path`. Returns `None` for an empty path: a record
    /// is identified by its path, so a nameless one cannot be looked up,
    /// compared, or restored.
    pub fn new(path: impl Into<Arc<str>>, mtime: u64, hash: u64) -> Option<Self> {
        let path = path.into();
        if path.is_empty() {
            return None;
        }
        Some(Self { path, mtime, hash })
    }

    pub fn path(&self) -> &str {
        &self.path
    }

    pub fn mtime(&self) -> u64 {
        self.mtime
    }

    pub fn hash(&self) -> u64 {
        self.hash
    }
}

/// Whether a unit's cache hit restores bytes or replays a verdict.
///
/// Derived from the declaration (see [`effect_kind`]), never stored on a
/// record. Storing it would let the evidence contradict the declaration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EffectKind {
    /// The unit declares output paths; a hit restores their bytes.
    Produced,
    /// The unit declares no output paths; a hit replays its observation.
    Observed,
}

/// The declaration decides the effect kind. Pure, total, no I/O.
///
/// This is not a test-vs-cook distinction. A cacheable unit with no declared
/// outputs already exists outside tests — `build_local_cache_key` has carried
/// an empty-output branch since long before this prototype — so the rule
/// generalises an existing case rather than special-casing a step kind.
pub fn effect_kind(meta: &CacheMeta) -> EffectKind {
    if meta.output_paths.is_empty() {
        EffectKind::Observed
    } else {
        EffectKind::Produced
    }
}

/// How a unit's run ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnitOutcome {
    Passed,
    Failed,
}

/// What running a unit was observed to do, beyond the bytes it wrote.
///
/// Recorded for every unit, not only output-less ones: `duration_secs` and
/// `outcome` are meaningful for a producing unit too, and holding them here
/// gives `cook why`'s observed history a first-class home instead of
/// best-effort recovery from retained build logs.
///
/// `recorded_at` is passed in rather than read: this crate owns no clock.
#[derive(Debug, Clone, PartialEq)]
pub struct Observation {
    outcome: UnitOutcome,
    stdout: String,
    stderr: String,
    duration_secs: f64,
    recorded_at: String,
}

impl Observation {
    pub fn new(
        outcome: UnitOutcome,
        stdout: String,
        stderr: String,
        duration_secs: f64,
        recorded_at: String,
    ) -> Self {
        Self { outcome, stdout, stderr, duration_secs, recorded_at }
    }

    pub fn outcome(&self) -> UnitOutcome {
        self.outcome
    }

    pub fn stdout(&self) -> &str {
        &self.stdout
    }

    pub fn stderr(&self) -> &str {
        &self.stderr
    }

    pub fn duration_secs(&self) -> f64 {
        self.duration_secs
    }

    pub fn recorded_at(&self) -> &str {
        &self.recorded_at
    }
}

/// Why a record could not be built for a declaration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecordMismatch {
    /// The declaration names no output paths — an observing unit — but the
    /// record carries output records. The evidence contradicts the
    /// declaration, and the declaration is the truth.
    ObservingUnitHasOutputs { found: usize },
    /// A supplied file record had an empty path.
    EmptyPath,
}

/// A unit's cache record: evidence of one run.
///
/// Deliberately absent: any tag saying what KIND of unit this is. Ask
/// [`Self::effect_kind`], or [`effect_kind`] of the declaration.
///
/// Deliberately present: `outputs`, which is empty for an observing unit.
/// Emptiness is the whole difference.
#[derive(Debug, Clone, PartialEq)]
pub struct UnitRecord {
    key: String,
    inputs: Vec<FileRecord>,
    outputs: Vec<FileRecord>,
    command_hash: u64,
    env_contribution: u64,
    seal_contribution: u64,
    observation: Observation,
}

impl UnitRecord {
    /// The sole constructor, and therefore the sole place the record/declaration
    /// agreement is established.
    ///
    /// The checked invariant is one-directional: a declaration with no outputs
    /// MUST record none. The converse is deliberately NOT checked, because a
    /// declared terminal output (`dist/**`) may legitimately resolve to zero
    /// files, so "declares outputs but recorded none" is a legal state rather
    /// than a contradiction.
    pub fn record(
        meta: &CacheMeta,
        inputs: Vec<FileRecord>,
        outputs: Vec<FileRecord>,
        seal_contribution: u64,
        observation: Observation,
    ) -> Result<Self, RecordMismatch> {
        if effect_kind(meta) == EffectKind::Observed && !outputs.is_empty() {
            return Err(RecordMismatch::ObservingUnitHasOutputs { found: outputs.len() });
        }
        Ok(Self {
            key: meta.cache_key.clone(),
            inputs,
            outputs,
            command_hash: meta.command_hash,
            env_contribution: meta.env_contribution,
            seal_contribution,
            observation,
        })
    }

    pub fn key(&self) -> &str {
        &self.key
    }

    pub fn inputs(&self) -> &[FileRecord] {
        &self.inputs
    }

    pub fn outputs(&self) -> &[FileRecord] {
        &self.outputs
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

    pub fn observation(&self) -> &Observation {
        &self.observation
    }

    /// Trustworthy because [`Self::record`] is the only way in.
    pub fn effect_kind(&self) -> EffectKind {
        if self.outputs.is_empty() {
            EffectKind::Observed
        } else {
            EffectKind::Produced
        }
    }

    /// The one state transition: same determinants, same outputs, same
    /// observation, freshly observed input records.
    ///
    /// Takes `self` by value so every carried-over field moves — including
    /// `observation`, whose captured streams are unbounded. Takes an owned
    /// `Vec` because the caller that gathers these already holds one; a slice
    /// would add an allocation per refresh.
    ///
    /// Infallible by construction: the checked invariant constrains `outputs`,
    /// which this does not touch.
    pub fn with_refreshed_inputs(self, inputs: Vec<FileRecord>) -> Self {
        Self { inputs, ..self }
    }
}

/// The current values a record is judged against.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Determinants<'a> {
    key: &'a str,
    command_hash: u64,
    env_contribution: u64,
    seal_contribution: u64,
}

impl<'a> Determinants<'a> {
    pub fn new(
        key: &'a str,
        command_hash: u64,
        env_contribution: u64,
        seal_contribution: u64,
    ) -> Self {
        Self { key, command_hash, env_contribution, seal_contribution }
    }
}

/// Which determinant moved out from under a record.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeterminantDrift {
    /// The record was filed under a different key than the one being asked
    /// for. A store that addresses entries by content must reject this rather
    /// than trust placement.
    Key,
    CommandHash,
    Env,
    Seal,
}

/// The rule both cache stores share, as a pure function.
///
/// Returns the first determinant that moved, or `None` when the record is
/// still addressed by its determinants and may be replayed. Says nothing about
/// files on disk: that is an observation, and observation belongs to the
/// crates that own I/O.
///
/// Order matters and matches the live implementation — key, command, env,
/// seal — so that the reported cause stays stable as this replaces the
/// hand-rolled predicate chains.
pub fn determinant_drift(
    record: &UnitRecord,
    current: &Determinants<'_>,
) -> Option<DeterminantDrift> {
    if record.key() != current.key {
        return Some(DeterminantDrift::Key);
    }
    if record.command_hash() != current.command_hash {
        return Some(DeterminantDrift::CommandHash);
    }
    if record.env_contribution() != current.env_contribution {
        return Some(DeterminantDrift::Env);
    }
    if record.seal_contribution() != current.seal_contribution {
        return Some(DeterminantDrift::Seal);
    }
    None
}

#[cfg(test)]
#[path = "tests/record_tests.rs"]
mod tests;
