//! Cache metadata and sharing disposition.

pub mod observation;
pub mod record;
pub mod step;

/// How one declared input entry is read (§17.1.1.2, CS-0186).
///
/// Two entries, two meanings, and the difference is NOT recoverable from the
/// string. `*`, `?` and `[` are ordinary filename characters on every platform
/// Cook supports: `app/[slug]/page.tsx` is a file — the Next.js routing
/// convention — and reads as a character class to any matcher. An entry
/// classified at match time by scanning its characters therefore expands that
/// name, matches nothing, and drops out of the unit's input set on the check
/// side and the record side alike, so the two agree and the unit reports a hit
/// over a file that has changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputKind {
    /// One file, named outright. Used as written; never expanded.
    Path,
    /// A glob or build-owned directory entry whose EXPANSION the unit reads.
    /// Resolved when the unit is ready (§18), because a consumed output
    /// declared `dist/**` names nothing until its producer has run.
    Pattern,
}

/// One entry of a unit's declared input set, with the classification made
/// where the entry entered the declaration (§17.1.1.2, CS-0186).
///
/// The kind travels with the path because the register phase is the only place
/// that knows it: it saw the tree the entry came out of. Recomputing it later
/// is what §17.1.1.2 forbids.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeclaredInput {
    pub path: String,
    pub kind: InputKind,
}

impl DeclaredInput {
    /// A file the unit reads.
    pub fn path(p: impl Into<String>) -> Self {
        Self { path: p.into(), kind: InputKind::Path }
    }

    /// A pattern whose expansion the unit reads.
    pub fn pattern(p: impl Into<String>) -> Self {
        Self { path: p.into(), kind: InputKind::Pattern }
    }

    pub fn is_pattern(&self) -> bool {
        matches!(self.kind, InputKind::Pattern)
    }
}

/// A bare string is a path. Patterns are never incidental — every one of them
/// is a consumed output or an author's glob, and both say so at their
/// declaration site — so the conversion that costs nothing to write is the one
/// that cannot under-key a unit by accident.
impl From<String> for DeclaredInput {
    fn from(p: String) -> Self {
        Self::path(p)
    }
}

impl From<&str> for DeclaredInput {
    fn from(p: &str) -> Self {
        Self::path(p)
    }
}

/// Declarative description of post-execution input discovery for a unit.
#[derive(Debug, Clone, PartialEq)]
pub struct DiscoveredInputs {
    pub from: String,
    pub format: String,
}

/// A unit's sharing disposition.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Sharing {
    #[default]
    Shared,
    Local,
    Pinned,
}

impl Sharing {
    pub fn is_local(self) -> bool {
        matches!(self, Sharing::Local)
    }

    pub fn is_pinned(self) -> bool {
        matches!(self, Sharing::Pinned)
    }

    pub fn as_wire_str(self) -> Option<&'static str> {
        match self {
            Sharing::Shared => None,
            Sharing::Local => Some("local"),
            Sharing::Pinned => Some("pinned"),
        }
    }

    pub fn from_wire_str(s: &str) -> Self {
        match s {
            "local" => Sharing::Local,
            "pinned" => Sharing::Pinned,
            _ => Sharing::Shared,
        }
    }
}

/// Metadata used by the caching subsystem to determine whether a unit can be skipped.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct CacheMeta {
    pub recipe_name: String,
    pub project_id: String,
    pub cookfile_path: String,
    pub cache_key: String,
    /// What this unit reads, entry by entry, each carrying whether it is a
    /// path or a pattern (§17.1.1.2, CS-0186). A pattern entry is resolved
    /// when the unit becomes ready, because a consumed output declared
    /// `dist/**` names no file at registration; a path entry is used as
    /// written, whatever characters are in it.
    pub inputs: Vec<DeclaredInput>,
    /// An allowlist narrowing the PATTERN-derived half of [`Self::inputs`]
    /// after resolution (§17.1.1.2, CS-0175 as amended by CS-0186). Empty means
    /// no narrowing. Two things it cannot do, both because narrowing errs
    /// toward the under-keyed direction where a stale hit replays against
    /// inputs that have moved: a filter matching nothing is inert rather than
    /// narrowing to nothing, and no filter ever removes a path the unit
    /// declared outright.
    ///
    /// On the declaration rather than on a payload: it is a fact about what the
    /// unit reads, and nothing that consults it needs to know which step kind
    /// produced the unit.
    pub consumes: Vec<String>,
    /// Whether this unit's key folds a materialised data member (§17.1
    /// observable 5). The member itself reaches the key through
    /// [`Self::command_hash`] and is deliberately not repeated here; what the
    /// cache needs from it is the one fact §17.4 rule 1 asks for — that there
    /// IS something to key on when the unit declares no file and no output.
    pub member_keyed: bool,
    pub output_paths: Vec<String>,
    pub command_hash: u64,
    pub env_contribution: u64,
    pub consulted_env: std::collections::BTreeMap<String, String>,
    pub discovered_inputs: Option<DiscoveredInputs>,
    pub seal_keys: std::collections::BTreeSet<String>,
    pub sharing: Sharing,
    pub record: bool,
}
