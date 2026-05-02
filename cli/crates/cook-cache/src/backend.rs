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
    /// Which output index this artifact represents (0-based).
    pub output_index: u32,
    /// Workspace-relative output path. Diagnostic only; not part of equality.
    pub output_path: String,
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

use sha2::{Digest, Sha256};

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
            output_index: 0,
            output_path: "build/foo.o".into(),
        }
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
