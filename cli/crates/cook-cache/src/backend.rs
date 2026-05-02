//! The CacheBackend trait — the seam Cook Cloud's R2/D1 backend implements
//! against. v3 ships LocalBackend (file-system); SHI-24 will add CloudBackend.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

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
}

pub trait CacheBackend: Send + Sync {
    /// Batch existence check. Returns the subset of inputs that are hits.
    /// Implementations MAY ignore order; the engine sorts before calling.
    fn batch_query(&self, keys: &[CloudKey]) -> BackendResult<BTreeSet<CloudKey>>;

    /// Fetch artifact bytes. Returns Ok(None) on miss (NOT an error).
    fn get(&self, key: &CloudKey) -> BackendResult<Option<Vec<u8>>>;

    /// Upload artifact bytes with metadata. Idempotent on (key, bytes):
    /// re-putting the same pair MUST succeed.
    fn put(&self, key: &CloudKey, bytes: &[u8], meta: &ArtifactMeta) -> BackendResult<()>;

    /// Explicit deletion. Idempotent: returns Ok(()) for both
    /// "deleted" and "didn't exist".
    fn delete(&self, key: &CloudKey) -> BackendResult<()>;

    /// Lightweight health check. Engine calls once at build start.
    fn health(&self) -> BackendResult<()>;
}
