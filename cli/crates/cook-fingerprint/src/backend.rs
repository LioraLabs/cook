//! The CacheBackend trait — the seam between fingerprint computation and the
//! cache persistence layer. Cook Cloud's R2/D1 backend implements this trait;
//! `cook-cache::backend::LocalBackend` is the v3 filesystem implementation.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// 32-byte SHA-256 cloud cache key.
pub type CloudKey = [u8; 32];

#[derive(Debug, Clone)]
pub enum BackendError {
    /// Network/transport failure. Engine treats as miss and proceeds.
    Transient(String),
    /// Authentication/permission failure. Engine logs once, disables backend for build.
    Unauthorized(String),
    /// Quota exceeded on put. Engine logs, drops the put, build continues.
    QuotaExceeded,
    /// Unexpected backend state (corrupted response, etc.). Logged; treated as miss.
    Other(String),
}

impl std::fmt::Display for BackendError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BackendError::Transient(s) => write!(f, "transient backend error: {s}"),
            BackendError::Unauthorized(s) => write!(f, "backend unauthorized: {s}"),
            BackendError::QuotaExceeded => write!(f, "backend quota exceeded"),
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
    pub context_hash: u64,
    pub env_contribution: u64,
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
}

impl ArtifactMeta {
    /// Sentinel placeholder for `content_hash` at construction time;
    /// overwritten by `CacheBackend::put`. Also the serde default for
    /// pre-CS-0054 sidecars that lack the field.
    pub fn zero_content_hash() -> [u8; 32] {
        [0u8; 32]
    }
}

pub trait CacheBackend: Send + Sync {
    /// Batch existence check. Returns the subset of inputs that are hits.
    /// Implementations MAY ignore order; the engine sorts before calling.
    fn batch_query(&self, keys: &[CloudKey]) -> BackendResult<BTreeSet<CloudKey>>;

    /// Fetch artifact bytes. Returns Ok(None) on miss (NOT an error).
    ///
    /// Implementations MUST self-verify content integrity before returning
    /// bytes. Concretely: the bytes returned MUST be byte-identical to the
    /// bytes most recently `put` under this key — otherwise `Ok(None)` MUST
    /// be returned (treat as miss, fail closed). The reference contract is
    /// SHA-256 of bytes-on-disk equal to `ArtifactMeta::content_hash` from
    /// the sidecar; alternative cryptographic schemes are permitted so long
    /// as the byte-identity property holds. This is the soundness primitive
    /// the Standard §{exec.cache.integrity} relies on; it MUST hold whether
    /// the backend is local-filesystem or a multi-tenant shared store.
    fn get(&self, key: &CloudKey) -> BackendResult<Option<Vec<u8>>>;

    /// Upload artifact bytes with metadata.
    ///
    /// Implementations MUST stamp `meta.content_hash` with the SHA-256 of
    /// `bytes` (or an equivalent cryptographic digest) before persisting
    /// the sidecar; callers pass the zero sentinel and treat the overwrite
    /// as authoritative. The persisted hash is the value `get` will check
    /// the bytes-on-disk against on a subsequent restore.
    ///
    /// **Idempotency contract (CS-0055).** A `put` to a key that already
    /// holds an artifact MUST distinguish two cases by comparing the SHA-256
    /// of the new `bytes` against the recorded `content_hash` of the existing
    /// artifact:
    ///
    /// 1. **Identical bytes** (`SHA-256(new_bytes) == existing.content_hash`):
    ///    the `put` MUST succeed as a no-op (or as an idempotent re-stamp);
    ///    `Ok(())` MUST be returned. This is the common case: a correct
    ///    rebuild deterministically produced the same bytes.
    /// 2. **Conflicting bytes** (`SHA-256(new_bytes) != existing.content_hash`):
    ///    the `put` MUST return `BackendError::Other(...)` with a diagnostic
    ///    message that names the key in hex and describes the conflict. The
    ///    implementation MUST NOT overwrite the prior bytes or sidecar.
    ///
    /// This is the write-side analogue of the `get` integrity check: it
    /// guarantees that a key in the artifact store maps to one and only one
    /// byte sequence over its lifetime, which is the invariant the read-side
    /// verification relies upon. On a multi-tenant shared backend this also
    /// prevents one client (e.g., one running a poisoned toolchain that
    /// produced different bytes) from silently corrupting another client's
    /// artifact through a key collision.
    ///
    /// If the existing meta sidecar is missing, unreadable, or malformed
    /// (i.e., no recorded `content_hash` is recoverable), the implementation
    /// MUST treat the entry as if no prior artifact existed and write through
    /// — this is the partial-write recovery path established by the atomic
    /// sidecar contract (cf. CS-0054 §3.2).
    fn put(&self, key: &CloudKey, bytes: &[u8], meta: &ArtifactMeta) -> BackendResult<()>;

    /// Explicit deletion. Idempotent: returns Ok(()) for both
    /// "deleted" and "didn't exist".
    fn delete(&self, key: &CloudKey) -> BackendResult<()>;

    /// Lightweight health check. Engine calls once at build start.
    fn health(&self) -> BackendResult<()>;
}

/// Inputs to `cloud_key()`. The struct is `Copy` so callers can build it once
/// and pass it around; lifetimes track the borrowed namespace and inputs slice.
#[derive(Clone, Copy)]
pub struct CloudKeyInputs<'a> {
    pub schema_version: u32,
    pub recipe_namespace: &'a str,
    pub command_hash: u64,
    pub context_hash: u64,
    pub env_contribution: u64,
    /// Caller MUST sort by path before passing. The slice is hashed in given
    /// order; sorting is the caller's responsibility (cf. spec §5.3).
    pub sorted_input_content_hashes: &'a [u64],
}

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
    h.update(inputs.context_hash.to_le_bytes());
    h.update(inputs.env_contribution.to_le_bytes());
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

    // ─── cloud_key composition tests ────────────────────────────────────────

    fn make_key_inputs() -> CloudKeyInputs<'static> {
        CloudKeyInputs {
            schema_version: 3,
            recipe_namespace: "cook/Cookfile::build",
            command_hash: 0xAAAA,
            context_hash: 0xBBBB,
            env_contribution: 0xCCCC,
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
    fn cloud_key_changes_on_context_hash_change() {
        let a = make_key_inputs();
        let mut b = a;
        b.context_hash = 0xFFFF;
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
}
