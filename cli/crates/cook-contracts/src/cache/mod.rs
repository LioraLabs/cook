//! Cache metadata and sharing disposition.

pub mod record;

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
#[derive(Debug, Clone, PartialEq)]
pub struct CacheMeta {
    pub recipe_name: String,
    pub project_id: String,
    pub cookfile_path: String,
    pub cache_key: String,
    /// The paths this unit reads. May contain a glob or a build-owned
    /// directory entry (§{exec.cache.output-globs}) when the unit declares an
    /// input its producer has not written yet — a consumed output declared
    /// `dist/**` names no file at registration. Such an entry is resolved when
    /// the unit becomes ready (§17.4 rule 1, CS-0186); an entry naming a
    /// literal path is used as written.
    pub input_paths: Vec<String>,
    /// An allowlist narrowing [`Self::input_paths`] after resolution (CS-0175).
    /// Empty means no narrowing. A filter matching nothing is inert rather than
    /// narrowing to nothing, because narrowing errs toward the under-keyed
    /// direction where a stale hit replays against inputs that have moved.
    ///
    /// On the declaration rather than on a payload: it is a fact about what the
    /// unit reads, and nothing that consults it needs to know which step kind
    /// produced the unit.
    pub consumes: Vec<String>,
    pub output_paths: Vec<String>,
    pub command_hash: u64,
    pub env_contribution: u64,
    pub consulted_env: std::collections::BTreeMap<String, String>,
    pub discovered_inputs: Option<DiscoveredInputs>,
    pub seal_keys: std::collections::BTreeSet<String>,
    pub sharing: Sharing,
    pub record: bool,
}
