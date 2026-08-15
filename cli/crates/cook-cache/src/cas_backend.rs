//! The `CacheBackend` trait: the seam between cache identity and cache
//! persistence. Cook Cloud's R2/D1 backend implements it; `LocalBackend` is
//! the v3 filesystem implementation.
//!
//! COOK-418: the keys, artifact metadata, determinant manifests and reserved
//! artifact indices this file used to also hold are now
//! `cook_contracts::cache::cas`. They are data with two ends and belong with
//! the law; the trait is the port to the outside world and belongs with the
//! implementations. This file moves to `cook-cache` in stage 3.

use std::collections::BTreeSet;
use std::io::Read;
use std::time::Duration;

pub use cook_contracts::cache::cas::*;

/// Defaults are tuned for cloud-grade workloads: 30s per-call timeout,
/// 3 retries with exponential backoff from 100ms to 5s, and a 1 GiB cap
/// on a single artifact's size. Users override via `[cloud]` knobs in
/// `.cook/cloud.toml`.
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

