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
///
/// Derives serde directly rather than getting a parallel wire enum: it is a
/// fieldless enum with no invariant and no private state, so there is no
/// constructor for a deserializer to bypass.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
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
        let record = Self {
            key: meta.cache_key.clone(),
            inputs,
            outputs,
            command_hash: meta.command_hash,
            env_contribution: meta.env_contribution,
            seal_contribution,
            observation,
        };
        // The invariant is stated once, in `agrees_with`, and asked here at
        // birth and again whenever a record comes back from storage.
        record.agrees_with(meta)?;
        Ok(record)
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

// -------------------------------------------------------------------------
// Wire format
//
// The LOGICAL on-disk shape lives here: field set, meaning, version, and the
// validity rules for reading one back. The CONTAINER layout does not — the
// binary index writes a string table and a flat record pool that entries index
// into by `(start, len)`, which is a statement about many records sharing one
// file, not about what a record is. That stays in `cook-cache`, and encodes to
// and from these types so both codecs agree on the shape by construction.
// -------------------------------------------------------------------------

/// Bumped whenever the recorded shape or its meaning changes.
///
/// Continues `cook_fingerprint::record::CACHE_VERSION` (7 at the time of
/// writing) rather than starting a fresh counter, so the existing
/// invalidate-on-bump path keeps working and a new number cannot collide with
/// an index already on disk. Superseded records are DELETED, not migrated
/// (CS-0166); nothing here reads an older shape.
pub const RECORD_SCHEMA_VERSION: u32 = 8;

/// Transport shape for [`FileRecord`].
///
/// `path` is `Arc<str>`, not `String`, so the binary decoder can build one
/// allocation per distinct path and clone pointers into every record naming
/// it. A `String` here would silently undo that interning on every load.
///
/// Note serde does not itself preserve sharing: a JSON round trip allocates
/// per occurrence. The `Arc` is here so the binary codec's interning has
/// somewhere to land, not because serde performs it.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct FileRecordWire {
    pub path: Arc<str>,
    pub mtime: u64,
    pub hash: u64,
}

/// Transport shape for [`Observation`].
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ObservationWire {
    pub outcome: UnitOutcome,
    pub stdout: String,
    pub stderr: String,
    pub duration_secs: f64,
    pub recorded_at: String,
}

/// Transport shape for [`UnitRecord`].
///
/// Public fields, because it has no invariant of its own — it is bytes given a
/// shape. Every invariant is established on the way out of it, by
/// [`UnitRecord::from_wire`].
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct UnitRecordWire {
    pub schema_version: u32,
    pub key: String,
    pub inputs: Vec<FileRecordWire>,
    pub outputs: Vec<FileRecordWire>,
    pub command_hash: u64,
    pub env_contribution: u64,
    pub seal_contribution: u64,
    pub observation: ObservationWire,
}

/// Why stored bytes could not become a record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WireError {
    /// The stored shape is not the one this build reads. Stores discard rather
    /// than migrate (CS-0166).
    SchemaVersion { found: u32, expected: u32 },
    /// A stored file record named no path.
    EmptyPath,
}

impl FileRecord {
    pub fn into_wire(self) -> FileRecordWire {
        FileRecordWire { path: self.path, mtime: self.mtime, hash: self.hash }
    }

    pub fn from_wire(wire: FileRecordWire) -> Result<Self, WireError> {
        Self::new(wire.path, wire.mtime, wire.hash).ok_or(WireError::EmptyPath)
    }
}

impl Observation {
    pub fn into_wire(self) -> ObservationWire {
        ObservationWire {
            outcome: self.outcome,
            stdout: self.stdout,
            stderr: self.stderr,
            duration_secs: self.duration_secs,
            recorded_at: self.recorded_at,
        }
    }

    pub fn from_wire(wire: ObservationWire) -> Self {
        Self {
            outcome: wire.outcome,
            stdout: wire.stdout,
            stderr: wire.stderr,
            duration_secs: wire.duration_secs,
            recorded_at: wire.recorded_at,
        }
    }
}

impl UnitRecord {
    /// Consumes the record so every field moves. `&self` would clone the
    /// captured streams, which are unbounded.
    pub fn into_wire(self) -> UnitRecordWire {
        UnitRecordWire {
            schema_version: RECORD_SCHEMA_VERSION,
            key: self.key,
            inputs: self.inputs.into_iter().map(FileRecord::into_wire).collect(),
            outputs: self.outputs.into_iter().map(FileRecord::into_wire).collect(),
            command_hash: self.command_hash,
            env_contribution: self.env_contribution,
            seal_contribution: self.seal_contribution,
            observation: self.observation.into_wire(),
        }
    }

    /// Read a record back.
    ///
    /// Deliberately takes NO declaration. An earlier design had it rebuild the
    /// determinants from a `CacheMeta`, which would have made
    /// [`determinant_drift`] structurally unable to fire: the record would
    /// always have agreed with whatever it was just rebuilt from. A stored
    /// record must keep the determinants it was STORED with; that is the
    /// entire basis on which drift is detectable.
    ///
    /// Relating a decoded record to a declaration is a separate question, and
    /// a separate pure rule: [`Self::agrees_with`].
    pub fn from_wire(wire: UnitRecordWire) -> Result<Self, WireError> {
        if wire.schema_version != RECORD_SCHEMA_VERSION {
            return Err(WireError::SchemaVersion {
                found: wire.schema_version,
                expected: RECORD_SCHEMA_VERSION,
            });
        }
        // `into_iter().map().collect()` over identically-laid-out element
        // types reuses the source buffer, so decoding costs the allocations
        // the codec already made and no more.
        let inputs = wire
            .inputs
            .into_iter()
            .map(FileRecord::from_wire)
            .collect::<Result<Vec<_>, _>>()?;
        let outputs = wire
            .outputs
            .into_iter()
            .map(FileRecord::from_wire)
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self {
            key: wire.key,
            inputs,
            outputs,
            command_hash: wire.command_hash,
            env_contribution: wire.env_contribution,
            seal_contribution: wire.seal_contribution,
            observation: Observation::from_wire(wire.observation),
        })
    }

    /// Does this record still describe a run of the unit `meta` declares?
    ///
    /// The same one-directional invariant [`Self::record`] establishes at
    /// birth, asked of a record that has come back from storage — where the
    /// declaration may since have changed underneath it.
    pub fn agrees_with(&self, meta: &CacheMeta) -> Result<(), RecordMismatch> {
        if effect_kind(meta) == EffectKind::Observed && !self.outputs.is_empty() {
            return Err(RecordMismatch::ObservingUnitHasOutputs { found: self.outputs.len() });
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "tests/record_tests.rs"]
mod tests;
