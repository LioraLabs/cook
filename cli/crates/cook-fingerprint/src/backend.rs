//! The CacheBackend trait — the seam between fingerprint computation and the
//! cache persistence layer. Cook Cloud's R2/D1 backend implements this trait;
//! `cook-cache::backend::LocalBackend` is the v3 filesystem implementation.

use std::collections::{BTreeMap, BTreeSet};
use std::io::Read;
use std::time::Duration;

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
/// Defaults are tuned for cloud-grade workloads: 30s per-call timeout,
/// 3 retries with exponential backoff from 100ms to 5s, and a 1 GiB cap
/// on a single artifact's size. Users override via `[cloud]` knobs in
/// `.cook/cloud.toml` (cf. design spec
/// `standard/specs/2026-05-04-cache-backend-config-design.md`).
#[derive(Debug, Clone)]
pub struct BackendConfig {
    /// Per-network-call timeout. Honored by network backends; ignored by
    /// `LocalBackend` (disk I/O does not time out in the cooperative-cancel
    /// sense). Default: 30s.
    pub timeout: Duration,
    /// Maximum number of retry attempts for transient failures (e.g.,
    /// network errors mapped to `BackendError::Transient`). Default: 3.
    pub max_retries: u32,
    /// Initial backoff delay before the first retry. Default: 100ms.
    pub backoff_initial: Duration,
    /// Cap on backoff delay between retries. Default: 5s.
    pub backoff_max: Duration,
    /// Maximum bytes a single artifact may have at put time. Default: 1 GiB.
    /// Both `LocalBackend` and `CloudBackend` MUST refuse `put` calls whose
    /// streamed bytes exceed this limit, returning `BackendError::Other`
    /// with a message naming the limit. The check happens during streaming,
    /// not pre-flight (the caller may not know the size up front).
    pub max_artifact_bytes: u64,
}

impl Default for BackendConfig {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(30),
            max_retries: 3,
            backoff_initial: Duration::from_millis(100),
            backoff_max: Duration::from_secs(5),
            max_artifact_bytes: 1024 * 1024 * 1024, // 1 GiB
        }
    }
}

#[derive(Debug, Clone)]
pub enum BackendError {
    /// Network/transport failure. Engine treats as miss and proceeds.
    Transient(String),
    /// Authentication/permission failure. Engine logs once, disables backend for build.
    Unauthorized(String),
    /// Quota exceeded. CS-0059: carries an optional `Retry-After` hint
    /// parsed from the server response. `None` is the terminal "drop &
    /// continue" CS-0058 behaviour (server gave no timing); `Some(d)` is
    /// retryable — the retry shell sleeps `d` (clamped to
    /// `[backoff_initial, backoff_max]`) and tries again, still bounded by
    /// `BackendConfig::max_retries`.
    QuotaExceeded(Option<std::time::Duration>),
    /// Unexpected backend state (corrupted response, etc.). Logged; treated as miss.
    Other(String),
}

impl std::fmt::Display for BackendError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BackendError::Transient(s) => write!(f, "transient backend error: {s}"),
            BackendError::Unauthorized(s) => write!(f, "backend unauthorized: {s}"),
            BackendError::QuotaExceeded(Some(d)) => {
                write!(f, "backend quota exceeded; retry after {d:?}")
            }
            BackendError::QuotaExceeded(None) => write!(f, "backend quota exceeded"),
            BackendError::Other(s) => write!(f, "backend error: {s}"),
        }
    }
}

impl std::error::Error for BackendError {}

pub type BackendResult<T> = Result<T, BackendError>;

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
    /// — manifest artifact whose body is a JSON path list (a later task).
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
    #[serde(with = "crate::record::hex_u64")]
    pub command_hash: u64,
    #[serde(with = "crate::record::hex_u64")]
    pub env_contribution: u64,
    #[serde(with = "crate::record::hex_u64")]
    pub seal_contribution: u64,
    /// Declared input workspace-path → content hash. Resolved form of
    /// `CloudKeyInputs::sorted_input_content_hashes`.
    #[serde(with = "hex_u64_map")]
    pub inputs: BTreeMap<String, u64>,
    /// Resolved (glob-expanded) declared output paths.
    pub output_paths: Vec<String>,
    /// Post-denylist consulted env key → value. Resolved form of
    /// `env_contribution`.
    pub consulted_env: BTreeMap<String, String>,
    /// Effective-seal-set probe key → canonical-JSON value bytes (UTF-8).
    /// Resolved form of `seal_contribution`.
    pub sealed_probes: BTreeMap<String, String>,
}

/// `hex_u64` (see [`crate::record::hex_u64`]) for the *values* of a
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

pub trait CacheBackend: Send + Sync {
    /// Batch existence check. Returns the subset of inputs that are hits.
    /// Implementations MAY ignore order; the engine sorts before calling.
    fn batch_query(&self, keys: &[CloudKey]) -> BackendResult<BTreeSet<CloudKey>>;

    /// Fetch artifact bytes as a streaming reader. Returns `Ok(None)` on
    /// miss (NOT an error).
    ///
    /// Implementations MUST self-verify content integrity such that the
    /// bytes ultimately delivered through the returned reader are
    /// byte-identical to the bytes most recently `put` under this key —
    /// otherwise the implementation MUST surface the failure either as
    /// `Ok(None)` (the integrity proof was unrecoverable before any bytes
    /// flowed: missing sidecar, malformed sidecar, zero-sentinel
    /// `content_hash` — pre-CS-0054 orphan) or as an `io::Error` of
    /// `ErrorKind::InvalidData` raised by the returned reader at
    /// end-of-stream (the bytes flowed but their hash did not match the
    /// sidecar's `content_hash`). The reference contract is SHA-256 of
    /// bytes-as-they-stream equal to `ArtifactMeta::content_hash` from the
    /// sidecar; alternative cryptographic schemes are permitted so long as
    /// the byte-identity property holds. Streaming verification (a
    /// `VerifyingReader`-style wrapper that tees bytes through a hasher
    /// and surfaces failure at EOF) is the recommended shape — it
    /// generalises cleanly to a future `CloudBackend` whose body is an
    /// HTTP response stream — but an implementation MAY also buffer the
    /// full bytes, verify, and return a `Cursor`. This is the soundness
    /// primitive the Standard §{exec.cache.integrity} relies on; it MUST
    /// hold whether the backend is local-filesystem or a multi-tenant
    /// shared store.
    fn get(&self, key: &CloudKey) -> BackendResult<Option<Box<dyn Read + Send>>>;

    /// Like `get`, but also returns the artifact's `ArtifactMeta`. Restore
    /// needs the `kind`/`mode`/`target` BEFORE deciding how to materialise the
    /// output (a symlink/dir has no usable body). Default impl is unsupported;
    /// concrete backends override.
    fn get_with_meta(
        &self,
        _key: &CloudKey,
    ) -> BackendResult<Option<(Box<dyn Read + Send>, ArtifactMeta)>> {
        Err(BackendError::Other("get_with_meta unsupported".into()))
    }

    /// Upload artifact bytes with metadata, streaming from `reader`.
    ///
    /// Implementations MUST stream the bytes from `reader` to a temporary
    /// location (without materialising the full artifact in memory),
    /// computing SHA-256 (or an equivalent cryptographic digest) of the
    /// bytes as they flow, and finalize the hash on EOF. The contract for
    /// `meta.content_hash` is:
    ///
    /// 1. If the caller's `meta.content_hash` is the zero sentinel
    ///    (`[0u8; 32]`), the implementation MUST stamp the computed hash
    ///    into `meta` (in-place) before returning `Ok(())`, and MUST
    ///    persist the stamped hash in the sidecar. This is the common
    ///    case: callers initialise with the sentinel and let `put` be the
    ///    sole authority on the persisted hash.
    /// 2. If the caller's `meta.content_hash` is non-zero and equal to the
    ///    computed hash, the implementation MUST persist `meta` as-is and
    ///    return `Ok(())` (caller-claimed hash matched).
    /// 3. If the caller's `meta.content_hash` is non-zero and differs from
    ///    the computed hash, the implementation MUST return
    ///    `BackendError::Other("caller-claimed content_hash differs from
    ///    streamed bytes")` (or a diagnostic of equivalent specificity)
    ///    — defence against caller bugs that would persist a sidecar
    ///    inconsistent with the bytes.
    ///
    /// **Idempotency contract (CS-0055).** Conflict detection MUST happen
    /// after the bytes have streamed and the SHA-256 has been finalized
    /// (the temporary file is the implementation's scratch space). A `put`
    /// to a key that already holds an artifact MUST distinguish two cases
    /// by comparing the computed hash against the recorded `content_hash`
    /// of the existing artifact:
    ///
    /// 1. **Identical bytes** (`computed == existing.content_hash`): the
    ///    `put` MUST discard the temporary and succeed as a no-op (or as
    ///    an idempotent re-stamp); `Ok(())` MUST be returned. This is the
    ///    common case: a correct rebuild deterministically produced the
    ///    same bytes.
    /// 2. **Conflicting bytes** (`computed != existing.content_hash`): the
    ///    `put` MUST discard the temporary and return
    ///    `BackendError::Other(...)` with a diagnostic message that names
    ///    the key in hex and describes the conflict. The implementation
    ///    MUST NOT overwrite the prior bytes or sidecar.
    ///
    /// This is the write-side analogue of the `get` integrity check: it
    /// guarantees that a key in the artifact store maps to one and only one
    /// byte sequence over its lifetime, which is the invariant the read-side
    /// verification relies upon. On a multi-tenant shared backend this also
    /// prevents one client (e.g., one running a poisoned toolchain that
    /// produced different bytes) from silently corrupting another client's
    /// artifact through a key collision.
    ///
    /// If the existing meta sidecar is missing, unreadable, malformed, or
    /// carries the zero-sentinel `content_hash` (i.e., no recorded hash is
    /// recoverable), the implementation MUST treat the entry as if no prior
    /// artifact existed and write through — this is the partial-write
    /// recovery path established by the atomic sidecar contract (cf.
    /// CS-0054 §3.2 and CS-0055 §7).
    fn put(
        &self,
        key: &CloudKey,
        reader: &mut dyn Read,
        meta: &mut ArtifactMeta,
    ) -> BackendResult<()>;

    /// Explicit deletion. Idempotent: returns Ok(()) for both
    /// "deleted" and "didn't exist".
    fn delete(&self, key: &CloudKey) -> BackendResult<()>;

    /// Lightweight health check. Engine calls once at build start.
    fn health(&self) -> BackendResult<()>;

    /// COOK-166 / CS-0110: persist the producer determinant manifest for the
    /// unit addressed by `key` (the unit's `cloud_key` K). Retrievable by the
    /// same key via [`CacheBackend::get_manifest`]. Diagnostic/verification
    /// data — NOT integrity-critical; a correct rebuild writes byte-identical
    /// content, so this is idempotent (last write wins on identical bytes).
    fn put_manifest(
        &self,
        key: &CloudKey,
        manifest: &DeterminantManifest,
    ) -> BackendResult<()>;

    /// Fetch the determinant manifest for `key`. `Ok(None)` on miss or on a
    /// malformed/legacy sidecar (the manifest is best-effort diagnostic data;
    /// a missing manifest is never an error).
    fn get_manifest(&self, key: &CloudKey) -> BackendResult<Option<DeterminantManifest>>;
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
mod tests {
    use super::*;

    fn key(byte: u8) -> CloudKey {
        let mut k = [0u8; 32];
        k[0] = byte;
        k
    }

    #[test]
    fn artifact_key_deterministic() {
        let cloud_k = key(0xAB);
        let a = artifact_key(&cloud_k, 0, "build/foo.o");
        let b = artifact_key(&cloud_k, 0, "build/foo.o");
        assert_eq!(a, b);
    }

    #[test]
    fn artifact_key_differs_on_index() {
        let cloud_k = key(0xAB);
        let a = artifact_key(&cloud_k, 0, "build/foo.o");
        let b = artifact_key(&cloud_k, 1, "build/foo.o");
        assert_ne!(a, b);
    }

    #[test]
    fn artifact_key_differs_on_path() {
        let cloud_k = key(0xAB);
        let a = artifact_key(&cloud_k, 0, "build/foo.o");
        let b = artifact_key(&cloud_k, 0, "build/bar.o");
        assert_ne!(a, b);
    }

    #[test]
    fn artifact_key_differs_on_cloud_key() {
        let a = artifact_key(&key(0x01), 0, "build/foo.o");
        let b = artifact_key(&key(0x02), 0, "build/foo.o");
        assert_ne!(a, b);
    }

    #[test]
    fn discovered_inputs_manifest_key_is_distinct() {
        let base = [3u8; 32];
        let manifest = artifact_key(
            &base,
            DISCOVERED_INPUTS_MANIFEST_INDEX,
            DISCOVERED_INPUTS_MANIFEST_PATH,
        );
        let out0 = artifact_key(&base, 0, "out");
        assert_ne!(manifest, out0);
    }

    // ─── cloud_key composition tests ────────────────────────────────────────

    fn make_key_inputs() -> CloudKeyInputs<'static> {
        CloudKeyInputs {
            schema_version: 3,
            recipe_namespace: "cook/Cookfile::build",
            command_hash: 0xAAAA,
            env_contribution: 0xCCCC,
            seal_contribution: 0xDDDD,
            sorted_input_content_hashes: &[0x1111, 0x2222, 0x3333],
        }
    }

    #[test]
    fn cloud_key_deterministic() {
        let inputs = make_key_inputs();
        let k1 = cloud_key(&inputs);
        let k2 = cloud_key(&inputs);
        assert_eq!(k1, k2);
    }

    #[test]
    fn cloud_key_changes_on_command_hash_change() {
        let a = make_key_inputs();
        let mut b = a;
        b.command_hash = 0xFFFF;
        assert_ne!(cloud_key(&a), cloud_key(&b));
    }

    #[test]
    fn cloud_key_changes_on_env_contribution_change() {
        let a = make_key_inputs();
        let mut b = a;
        b.env_contribution = 0xFFFF;
        assert_ne!(cloud_key(&a), cloud_key(&b));
    }

    #[test]
    fn cloud_key_changes_on_schema_version_change() {
        let a = make_key_inputs();
        let mut b = a;
        b.schema_version = 4;
        assert_ne!(cloud_key(&a), cloud_key(&b));
    }

    #[test]
    fn cloud_key_changes_on_namespace_change() {
        let a = make_key_inputs();
        let mut b = a;
        b.recipe_namespace = "cook/Cookfile::test";
        assert_ne!(cloud_key(&a), cloud_key(&b));
    }

    #[test]
    fn cloud_key_changes_on_input_content_change() {
        let a = make_key_inputs();
        let alt_inputs = [0x1111, 0x2222, 0x9999]; // last hash differs
        let b = CloudKeyInputs { sorted_input_content_hashes: &alt_inputs, ..a };
        assert_ne!(cloud_key(&a), cloud_key(&b));
    }

    #[test]
    fn cloud_key_caller_must_sort_inputs() {
        // The function trusts its caller's sort. A caller-sorted slice produces
        // a stable hash; an unsorted slice produces a different (but stable) one.
        // This test documents that the sort is the caller's responsibility.
        let sorted = [0x1111u64, 0x2222, 0x3333];
        let unsorted = [0x3333u64, 0x1111, 0x2222];
        let a = make_key_inputs();
        let b = CloudKeyInputs { sorted_input_content_hashes: &sorted, ..a };
        let c = CloudKeyInputs { sorted_input_content_hashes: &unsorted, ..a };
        assert_ne!(cloud_key(&b), cloud_key(&c),
            "the function does not internally sort; caller responsibility");
    }

    #[test]
    fn cloud_key_returns_32_bytes() {
        let k = cloud_key(&make_key_inputs());
        assert_eq!(k.len(), 32);
    }

    #[test]
    fn cloud_key_changes_on_seal_contribution_change() {
        let a = make_key_inputs();
        let mut b = a;
        b.seal_contribution = 0xFFFF;
        assert_ne!(cloud_key(&a), cloud_key(&b));
    }

    #[test]
    fn cloud_key_zero_seal_is_stable() {
        let a = make_key_inputs();
        let b = a;
        assert_eq!(cloud_key(&a), cloud_key(&b));
    }

    // ---- CS-0074 ArtifactMeta.kind tests ----

    fn minimal_meta_json(extra: &str) -> String {
        format!(
            r#"{{
                "recipe_namespace": "ns",
                "command_hash": 0,
                "env_contribution": 0,
                "schema_version": 1,
                "size_bytes": 0,
                "tags": [],
                "consulted_env_keys": [],
                "output_index": 0,
                "output_path": "a.o"
                {}
            }}"#,
            extra
        )
    }

    #[test]
    fn artifact_meta_kind_defaults_to_none_for_legacy_sidecars() {
        let json = minimal_meta_json("");
        let meta: ArtifactMeta = serde_json::from_str(&json).unwrap();
        assert!(meta.kind.is_none());
    }

    #[test]
    fn artifact_meta_kind_round_trips_when_set() {
        let json = minimal_meta_json(r#", "kind": "probe_value""#);
        let meta: ArtifactMeta = serde_json::from_str(&json).unwrap();
        assert_eq!(meta.kind.as_deref(), Some("probe_value"));
        // Round-trip through serde_json
        let s = serde_json::to_string(&meta).unwrap();
        let back: ArtifactMeta = serde_json::from_str(&s).unwrap();
        assert_eq!(back.kind.as_deref(), Some("probe_value"));
    }

    #[test]
    fn artifact_meta_as_probe_value_sets_kind() {
        let meta = ArtifactMeta {
            recipe_namespace: "ns".into(),
            command_hash: 0,
            env_contribution: 0,
            seal_contribution: 0,
            schema_version: 1,
            size_bytes: 0,
            tags: BTreeSet::new(),
            consulted_env_keys: BTreeSet::new(),
            output_index: 0,
            output_path: "probe.bin".into(),
            content_hash: ArtifactMeta::zero_content_hash(),
            kind: None,
            mode: ArtifactMeta::default_mode(),
            target: None,
        }
        .as_probe_value();
        assert_eq!(meta.kind.as_deref(), Some("probe_value"));
    }

    #[test]
    fn artifact_meta_kind_none_not_serialised() {
        let meta = ArtifactMeta {
            recipe_namespace: "ns".into(),
            command_hash: 0,
            env_contribution: 0,
            seal_contribution: 0,
            schema_version: 1,
            size_bytes: 0,
            tags: BTreeSet::new(),
            consulted_env_keys: BTreeSet::new(),
            output_index: 0,
            output_path: "a.o".into(),
            content_hash: ArtifactMeta::zero_content_hash(),
            kind: None,
            mode: ArtifactMeta::default_mode(),
            target: None,
        };
        let s = serde_json::to_string(&meta).unwrap();
        assert!(!s.contains("kind"), "kind: None MUST be omitted from JSON: {s}");
    }

    // ---- end CS-0074 ----

    // ---- COOK-180: mode + target fidelity tests ----

    #[test]
    fn artifact_meta_mode_and_symlink_target_round_trip() {
        let json = minimal_meta_json(
            r#", "mode": 493, "kind": "symlink", "target": "../sib""#,
        );
        let meta: ArtifactMeta = serde_json::from_str(&json).expect("parse");
        assert_eq!(meta.mode, 0o755);
        assert_eq!(meta.kind.as_deref(), Some("symlink"));
        assert_eq!(meta.target.as_deref(), Some("../sib"));
        let s = serde_json::to_string(&meta).expect("serialize");
        let back: ArtifactMeta = serde_json::from_str(&s).expect("reparse");
        assert_eq!(meta, back);
    }

    #[test]
    fn artifact_meta_legacy_sidecar_defaults_mode_and_target() {
        // A pre-fidelity sidecar lacks mode/target entirely.
        let meta: ArtifactMeta = serde_json::from_str(&minimal_meta_json("")).expect("parse");
        assert_eq!(meta.mode, 0o644);
        assert!(meta.target.is_none());
    }

    // ---- end COOK-180 ----

    #[test]
    fn determinant_manifest_serializes_deterministically() {
        use std::collections::BTreeMap;
        let mut inputs = BTreeMap::new();
        inputs.insert("src/a.c".to_string(), 0xAABB_u64);
        inputs.insert("src/b.c".to_string(), 0xFFFF_FFFF_FFFF_FFFF_u64);
        let mut env = BTreeMap::new();
        env.insert("CC".to_string(), "clang".to_string());
        let mut probes = BTreeMap::new();
        probes.insert("host".to_string(), "\"x86_64-linux\"".to_string());
        let m = DeterminantManifest {
            schema_version: 5,
            recipe_namespace: "cook/Cookfile::build".into(),
            key: "ab".repeat(32),
            command_hash: 0x1234,
            env_contribution: 0x5678,
            seal_contribution: 0x9abc,
            inputs,
            output_paths: vec!["build/a.o".into()],
            consulted_env: env,
            sealed_probes: probes,
        };
        let a = serde_json::to_vec(&m).unwrap();
        let b = serde_json::to_vec(&m.clone()).unwrap();
        assert_eq!(a, b, "same manifest must serialize to identical bytes");
        let back: DeterminantManifest = serde_json::from_slice(&a).unwrap();
        assert_eq!(back.inputs["src/b.c"], 0xFFFF_FFFF_FFFF_FFFF_u64);
        assert_eq!(back, m);
    }
}
