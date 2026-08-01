//! Content-addressed store law: keys, artifact metadata, determinant
//! manifests, and the magic indices that name the store's reserved artifacts
//! (COOK-418).
//!
//! Everything here has two ends. A key is composed by a publisher and
//! recomposed by a consumer; `OBSERVATION_PATH` is written by one side and
//! looked up by the other; a `DeterminantManifest` is serialised by the
//! publish path and deserialised by `cook why`. That is the admission bar,
//! and it is why these lived in the wrong crate: they were held out for
//! "needing sha2", which is a dependency rather than an effect.
//!
//! The `CacheBackend` trait is deliberately NOT here. A trait definition would
//! pass `layout.rs`, but it is the port to the outside world and its home is
//! with the implementations, in `cook-cache`.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// 32-byte SHA-256 cloud cache key.
pub type CloudKey = [u8; 32];

/// User-overrideable backend tunables (CS-0057). Threaded into every
/// `CacheBackend` constructor; the future `CloudBackend` will honour
/// `timeout`, `max_retries`, `backoff_initial`, and `backoff_max` for HTTP
/// calls, while every backend (local or cloud) MUST honour
/// `max_artifact_bytes` at `put` time.
///
/// Metadata describing one artifact, written alongside the bytes for backend
/// introspection and eviction policy. Values of consulted env are NEVER stored
/// here — only the keys, for diagnostic use.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ArtifactMeta {
    pub recipe_namespace: String,
    pub command_hash: u64,
    pub env_contribution: u64,
    /// COOK-161 / CS-0107: the unit's effective-seal-set value fold. Diagnostic
    /// + key-consistency; defaults to 0 for legacy sidecars.
    #[serde(default)]
    pub seal_contribution: u64,
    pub schema_version: u32,
    pub size_bytes: u64,
    pub tags: BTreeSet<String>,
    pub consulted_env_keys: BTreeSet<String>,
    /// Which output index this artifact represents (0-based).
    pub output_index: u32,
    /// Workspace-relative output path. Diagnostic only; not part of equality.
    pub output_path: String,
    /// SHA-256 of the artifact bytes. Computed and stamped by the backend
    /// in `CacheBackend::put`; verified against the on-disk bytes by
    /// `CacheBackend::get`. Callers SHOULD pass the all-zero sentinel
    /// `[0u8; 32]` at construction time — `put` overwrites it before
    /// persisting the sidecar. This is the soundness primitive for shared
    /// (multi-tenant) backends where the artifact bytes may be tampered
    /// with by parties other than the local build; cf. Cook Standard
    /// §{exec.cache.integrity}. Cryptographic strength here defends against
    /// byte-only tampering; an adversary capable of consistently rewriting
    /// both bytes and meta is out of scope (see CS-0054 spec §2).
    #[serde(default = "ArtifactMeta::zero_content_hash")]
    pub content_hash: [u8; 32],
    /// Disambiguates the artifact body kind. `None` (or the default) is the
    /// legacy "file artifact" case. `Some("probe_value")` is the
    /// canonical-JSON probe-output artifact (CS-0074, encoding revised by
    /// CS-0102). `Some("symlink")` — target carried in `target`, no body.
    /// `Some("dir")` — empty directory, no body. `Some("discovered_inputs")`
    /// — discovered-inputs manifest artifact whose body is a JSON path list,
    /// keyed by the unit's declared-inputs-only cloud key (cold cc sharing).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    /// Unix file mode of the stored output (e.g. `0o755`). Defaults to
    /// `0o644` for legacy sidecars and on Windows (mode-0755 parity handled
    /// at restore). Applies to `File` and `Dir` kinds.
    #[serde(default = "ArtifactMeta::default_mode")]
    pub mode: u32,
    /// Symlink target (workspace-relative), set only when `kind == "symlink"`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
}

impl ArtifactMeta {
    /// Sentinel placeholder for `content_hash` at construction time;
    /// overwritten by `CacheBackend::put`. Also the serde default for
    /// pre-CS-0054 sidecars that lack the field.
    pub fn zero_content_hash() -> [u8; 32] {
        [0u8; 32]
    }

    /// Serde default for `mode`: regular-file 0644.
    pub fn default_mode() -> u32 {
        0o644
    }

    /// Convenience: construct a probe-value artifact meta with `kind = Some("probe_value")`.
    /// All other fields must be filled in by the caller.
    pub fn as_probe_value(mut self) -> Self {
        self.kind = Some("probe_value".into());
        self
    }
}

/// One blob discovered by `LocalBackend::enumerate()` (COOK-232): everything
/// a CAS-hygiene consumer (`cook cache du`, `cook cache gc` / COOK-234) needs
/// to size up and evict a single artifact, WITHOUT trusting anything the
/// original caller claimed about it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EvictCandidate {
    /// The artifact's 32-byte content-addressed key.
    pub key: CloudKey,
    /// On-disk blob size from `fs::metadata().len()`. NOT `ArtifactMeta.size_bytes`,
    /// which is caller-set and untrusted (see `LocalBackend::put`'s handling of
    /// `size_bytes` in the `cook-cache` crate: it's left as whatever the caller
    /// passed in, not recomputed from the written bytes).
    pub size: u64,
    /// Blob mtime as Unix seconds. Touch-on-read (COOK-233) keeps this current;
    /// 0 when the platform mtime is unavailable.
    pub last_access: u64,
    /// `ArtifactMeta.kind` verbatim. `None` = legacy file artifact, or an orphan
    /// blob with no readable sidecar. Both are evictable, so the conflation is safe.
    pub kind: Option<String>,
    /// `ArtifactMeta.recipe_namespace` verbatim, `""` when no sidecar was readable.
    /// Reporting only (milestone D3: attribution is a label, never a deletion key).
    pub recipe_namespace: String,
}

/// COOK-166 / CS-0110: the producer **determinant manifest** persisted
/// alongside a shared artifact. It records the *resolved values* that formed
/// the unit's single cache key K (§{exec.cache.single-key}) — not the artifact
/// bytes, and NOT an attestation of which producer ran (deferred to M2). It
/// powers `cook why`-on-miss and the shadow-divergence verifier: a consumer
/// that recomputes a different K can diff its determinants against this
/// manifest to attribute the miss to a specific input, env value, or probe.
///
/// All collections are ordered (`BTreeMap`) so the same K yields byte-identical
/// manifest bytes — the determinism invariant the verifier relies on. `u64`
/// hashes serialize as zero-padded lowercase hex strings (the `hex_u64`
/// convention of `record.rs`) so a high-bit value round-trips through JSON.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DeterminantManifest {
    pub schema_version: u32,
    pub recipe_namespace: String,
    /// Hex of the unit's `cloud_key` (K). Self-identifying; the verifier
    /// confirms the recorded determinants recompose to this key.
    pub key: String,
    #[serde(with = "crate::cache::step::hex_u64")]
    pub command_hash: u64,
    #[serde(with = "crate::cache::step::hex_u64")]
    pub env_contribution: u64,
    #[serde(with = "crate::cache::step::hex_u64")]
    pub seal_contribution: u64,
    /// Declared input workspace-path → content hash. Resolved form of
    /// `CloudKeyInputs::sorted_input_content_hashes`.
    #[serde(with = "hex_u64_map")]
    pub inputs: BTreeMap<String, u64>,
    /// Resolved (glob-expanded) declared output paths.
    pub output_paths: Vec<String>,
    /// COOK-278: empty directories recorded as trailing implicit outputs
    /// (COOK-180). A manifest-driven restore (`fetch_by_key`) recreates these
    /// after the file outputs so a fetch hit is byte-identical to a fresh
    /// build. Absent (`[]`) on pre-COOK-278 manifests — those restore files
    /// only, exactly as before.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub empty_dir_outputs: Vec<String>,
    /// Post-denylist consulted env key → value. Resolved form of
    /// `env_contribution`.
    pub consulted_env: BTreeMap<String, String>,
    /// Effective-seal-set probe key → canonical-JSON value bytes (UTF-8).
    /// Resolved form of `seal_contribution`.
    pub sealed_probes: BTreeMap<String, String>,
    /// Scalar half of the last successful execution, paired with the
    /// observation artifact under this manifest's key.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observation: Option<crate::cache::observation::Observation>,
}

/// `hex_u64` (see [`crate::cache::step::hex_u64`]) for the *values* of a
/// `BTreeMap<String, u64>`.
mod hex_u64_map {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    use std::collections::BTreeMap;
    pub fn serialize<S: Serializer>(
        m: &BTreeMap<String, u64>,
        s: S,
    ) -> Result<S::Ok, S::Error> {
        let rendered: BTreeMap<&String, String> =
            m.iter().map(|(k, v)| (k, format!("{v:016x}"))).collect();
        rendered.serialize(s)
    }
    pub fn deserialize<'de, D: Deserializer<'de>>(
        d: D,
    ) -> Result<BTreeMap<String, u64>, D::Error> {
        let raw: BTreeMap<String, String> = BTreeMap::deserialize(d)?;
        raw.into_iter()
            .map(|(k, v)| {
                u64::from_str_radix(&v, 16)
                    .map(|n| (k, n))
                    .map_err(serde::de::Error::custom)
            })
            .collect()
    }
}

/// Inputs to `cloud_key()`. The struct is `Copy` so callers can build it once
/// and pass it around; lifetimes track the borrowed namespace and inputs slice.
#[derive(Clone, Copy)]
pub struct CloudKeyInputs<'a> {
    pub schema_version: u32,
    pub recipe_namespace: &'a str,
    pub command_hash: u64,
    pub env_contribution: u64,
    /// COOK-161 / CS-0107: the unit's effective-seal-set value fold (see
    /// `StepEntry.seal_contribution`). Zero for an unsealed unit.
    pub seal_contribution: u64,
    /// Caller MUST sort by path before passing. The slice is hashed in given
    /// order; sorting is the caller's responsibility (cf. spec §5.3).
    pub sorted_input_content_hashes: &'a [u64],
}

/// Compose the canonical `recipe_namespace` string for a unit:
/// `"<project_id>/<cookfile_path>::<recipe>"`. This is the SINGLE source of
/// that composition — every cloud_key, ArtifactMeta, and DeterminantManifest
/// namespace MUST come from here so the three sites cannot drift (spec §5.3).
pub fn recipe_namespace(project_id: &str, cookfile_path: &str, recipe: &str) -> String {
    format!("{project_id}/{cookfile_path}::{recipe}")
}

/// Reserved output index for the COOK-177 discovered-inputs manifest, keyed
/// under a unit's DECLARED-inputs-only cloud key. `u32::MAX` cannot collide
/// with a real output index (no unit declares u32::MAX outputs).
pub const DISCOVERED_INPUTS_MANIFEST_INDEX: u32 = u32::MAX;
/// Reserved output path for the discovered-inputs manifest artifact.
pub const DISCOVERED_INPUTS_MANIFEST_PATH: &str = "__cook_discovered_inputs__";

/// COOK-278: reserved output index for the multi-entry discovered-input SETS
/// manifest, keyed under the same DECLARED-inputs-only cloud key. Unlike the
/// single-set COOK-177 manifest above (last-writer-wins, which loses older
/// input sets the moment an edit changes what the depfile discovers), this
/// artifact accumulates every distinct discovered-path set seen for the
/// declared key, so a revert can recompose the ORIGINAL full key even after
/// an intervening build discovered a different set.
pub const DISCOVERED_INPUT_SETS_INDEX: u32 = u32::MAX - 1;
/// Reserved output path for the discovered-input sets manifest artifact.
pub const DISCOVERED_INPUT_SETS_PATH: &str = "__cook_discovered_input_sets__";

/// Reserved artifact carrying a unit's captured output log.
pub const OBSERVATION_INDEX: u32 = u32::MAX - 2;
pub const OBSERVATION_PATH: &str = "__cook_observation__";

/// Cap on retained discovered-path sets per declared key. Oldest sets fall
/// off; a fallen-off set degrades to a safe re-execute, never a wrong hit.
pub const DISCOVERED_INPUT_SETS_CAP: usize = 64;

/// Derive an output-scoped artifact key from a cache entry's cloud_key.
///
/// One logical cache entry can produce multiple output artifacts. Each
/// artifact is independently addressable in the backend via
/// `SHA-256(cloud_key || u32_le(output_index) || output_path_bytes)`.
/// See 2026-05-02 addendum spec §4.1.
pub fn artifact_key(
    cloud_key: &CloudKey,
    output_index: u32,
    output_path: &str,
) -> CloudKey {
    let mut h = Sha256::new();
    h.update(cloud_key);
    h.update(output_index.to_le_bytes());
    h.update(output_path.as_bytes());
    h.finalize().into()
}

/// Compose the SHA-256 cloud key for an artifact.
/// See spec §5.3 for the composition; the 0x00 delimiter prevents
/// string-injection collisions between the namespace and hash bytes.
pub fn cloud_key(inputs: &CloudKeyInputs<'_>) -> CloudKey {
    let mut h = Sha256::new();
    h.update(inputs.schema_version.to_le_bytes());
    h.update(inputs.recipe_namespace.as_bytes());
    h.update([0x00]); // delimiter
    h.update(inputs.command_hash.to_le_bytes());
    h.update(inputs.env_contribution.to_le_bytes());
    h.update(inputs.seal_contribution.to_le_bytes());
    for hash in inputs.sorted_input_content_hashes {
        h.update(hash.to_le_bytes());
    }
    h.finalize().into()
}

#[cfg(test)]
#[path = "tests/cas_tests.rs"]
mod tests;
