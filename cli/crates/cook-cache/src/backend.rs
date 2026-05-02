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

use std::path::PathBuf;

pub struct LocalBackend {
    root: PathBuf,
}

impl LocalBackend {
    pub fn new(root: PathBuf) -> Self {
        // Ensure root exists; ignore "already exists" errors.
        let _ = std::fs::create_dir_all(&root);
        Self { root }
    }

    /// Compute the on-disk path for a CloudKey:
    ///   {root}/{first_2_hex_chars}/{remaining_62_hex_chars}
    pub(crate) fn path_for(&self, key: &CloudKey) -> PathBuf {
        let hex = hex::encode(key);
        self.root.join(&hex[..2]).join(&hex[2..])
    }
}

impl CacheBackend for LocalBackend {
    fn batch_query(&self, keys: &[CloudKey]) -> BackendResult<BTreeSet<CloudKey>> {
        let mut hits = BTreeSet::new();
        for k in keys {
            if self.path_for(k).exists() {
                hits.insert(*k);
            }
        }
        Ok(hits)
    }

    fn get(&self, key: &CloudKey) -> BackendResult<Option<Vec<u8>>> {
        let path = self.path_for(key);
        match std::fs::read(&path) {
            Ok(bytes) => Ok(Some(bytes)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(BackendError::Other(format!("read {}: {e}", path.display()))),
        }
    }

    fn put(&self, key: &CloudKey, bytes: &[u8], meta: &ArtifactMeta) -> BackendResult<()> {
        let path = self.path_for(key);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| BackendError::Other(format!("mkdir {}: {e}", parent.display())))?;
        }
        // Atomic write via tmp + rename.
        let tmp = path.with_extension("tmp");
        std::fs::write(&tmp, bytes)
            .map_err(|e| BackendError::Other(format!("write {}: {e}", tmp.display())))?;
        std::fs::rename(&tmp, &path)
            .map_err(|e| BackendError::Other(format!("rename {}: {e}", path.display())))?;

        // Sidecar metadata.
        let meta_path = path.with_extension("meta.json");
        let meta_bytes = serde_json::to_vec(meta)
            .map_err(|e| BackendError::Other(format!("serialize meta: {e}")))?;
        std::fs::write(&meta_path, &meta_bytes)
            .map_err(|e| BackendError::Other(format!("write meta {}: {e}", meta_path.display())))?;
        Ok(())
    }

    fn delete(&self, key: &CloudKey) -> BackendResult<()> {
        let path = self.path_for(key);
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(path.with_extension("meta.json"));
        Ok(())
    }

    fn health(&self) -> BackendResult<()> {
        std::fs::metadata(&self.root)
            .map(|_| ())
            .map_err(|e| BackendError::Other(format!("root {}: {e}", self.root.display())))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_meta() -> ArtifactMeta {
        ArtifactMeta {
            recipe_namespace: "cook/Cookfile::build".into(),
            command_hash: 0xdead_beef,
            context_hash: 0x1111_2222,
            env_contribution: 0x3333_4444,
            schema_version: 3,
            size_bytes: 5,
            tags: BTreeSet::new(),
            consulted_env_keys: BTreeSet::new(),
        }
    }

    fn key(byte: u8) -> CloudKey {
        let mut k = [0u8; 32];
        k[0] = byte;
        k
    }

    #[test]
    fn local_backend_health_ok_on_existing_root() {
        let dir = tempfile::tempdir().expect("tempdir");
        let backend = LocalBackend::new(dir.path().to_path_buf());
        backend.health().expect("health ok");
    }

    #[test]
    fn local_backend_get_miss_returns_none() {
        let dir = tempfile::tempdir().expect("tempdir");
        let backend = LocalBackend::new(dir.path().to_path_buf());
        let k = key(0xAB);
        assert!(backend.get(&k).expect("get").is_none());
    }

    #[test]
    fn local_backend_put_get_round_trip() {
        let dir = tempfile::tempdir().expect("tempdir");
        let backend = LocalBackend::new(dir.path().to_path_buf());
        let k = key(0x01);
        backend.put(&k, b"hello", &sample_meta()).expect("put");
        let got = backend.get(&k).expect("get").expect("hit");
        assert_eq!(got, b"hello");
    }

    #[test]
    fn local_backend_put_idempotent() {
        let dir = tempfile::tempdir().expect("tempdir");
        let backend = LocalBackend::new(dir.path().to_path_buf());
        let k = key(0x02);
        backend.put(&k, b"data", &sample_meta()).expect("put 1");
        backend.put(&k, b"data", &sample_meta()).expect("put 2");
        let got = backend.get(&k).expect("get").expect("hit");
        assert_eq!(got, b"data");
    }

    #[test]
    fn local_backend_batch_query_returns_hits_subset() {
        let dir = tempfile::tempdir().expect("tempdir");
        let backend = LocalBackend::new(dir.path().to_path_buf());
        let k1 = key(0x10);
        let k2 = key(0x20);
        let k3 = key(0x30);
        backend.put(&k1, b"a", &sample_meta()).expect("put1");
        backend.put(&k3, b"c", &sample_meta()).expect("put3");
        let hits = backend.batch_query(&[k1, k2, k3]).expect("query");
        assert!(hits.contains(&k1));
        assert!(!hits.contains(&k2));
        assert!(hits.contains(&k3));
    }

    #[test]
    fn local_backend_delete_idempotent() {
        let dir = tempfile::tempdir().expect("tempdir");
        let backend = LocalBackend::new(dir.path().to_path_buf());
        let k = key(0xFF);
        backend.delete(&k).expect("delete missing ok"); // never existed
        backend.put(&k, b"x", &sample_meta()).expect("put");
        backend.delete(&k).expect("delete existing ok");
        backend.delete(&k).expect("delete missing again ok");
        assert!(backend.get(&k).expect("get").is_none());
    }

    #[test]
    fn local_backend_meta_sidecar_persisted() {
        let dir = tempfile::tempdir().expect("tempdir");
        let backend = LocalBackend::new(dir.path().to_path_buf());
        let k = key(0x55);
        let mut meta = sample_meta();
        meta.tags.insert("ci".into());
        meta.tags.insert("release:v0.5".into());
        backend.put(&k, b"x", &meta).expect("put");

        // Read the sidecar file directly to verify structure.
        let path = backend.path_for(&k);
        let meta_path = path.with_extension("meta.json");
        let bytes = std::fs::read(&meta_path).expect("read sidecar");
        let restored: ArtifactMeta = serde_json::from_slice(&bytes).expect("deserialize");
        assert_eq!(restored.tags, meta.tags);
        assert_eq!(restored.recipe_namespace, meta.recipe_namespace);
    }

    #[test]
    fn local_backend_path_for_fans_out_by_first_byte() {
        let dir = tempfile::tempdir().expect("tempdir");
        let backend = LocalBackend::new(dir.path().to_path_buf());
        let k = key(0xAB);
        let path = backend.path_for(&k);
        // First two hex chars are the parent directory; remaining 62 are the file name.
        let parent = path.parent().unwrap().file_name().unwrap().to_string_lossy();
        assert_eq!(parent, "ab");
        let file_name = path.file_name().unwrap().to_string_lossy();
        assert_eq!(file_name.len(), 62);
    }
}
