//! `LocalBackend` — the v3 filesystem implementation of `CacheBackend`.
//!
//! The trait, key types, and key composition (`cloud_key`, `artifact_key`)
//! live in `cook-fingerprint::backend`; this module is the persistence side.
//! For back-compat we re-export the trait/key types here so existing callers
//! that say `cook_cache::backend::*` continue to compile.

use std::path::PathBuf;

use sha2::{Digest, Sha256};

pub use cook_fingerprint::backend::{
    artifact_key, cloud_key, ArtifactMeta, BackendError, BackendResult, CacheBackend, CloudKey,
    CloudKeyInputs,
};

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
    fn batch_query(&self, keys: &[CloudKey]) -> BackendResult<std::collections::BTreeSet<CloudKey>> {
        let mut hits = std::collections::BTreeSet::new();
        for k in keys {
            if self.path_for(k).exists() {
                hits.insert(*k);
            }
        }
        Ok(hits)
    }

    fn get(&self, key: &CloudKey) -> BackendResult<Option<Vec<u8>>> {
        let path = self.path_for(key);
        let bytes = match std::fs::read(&path) {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(BackendError::Other(format!("read {}: {e}", path.display()))),
        };

        // CS-0054: self-verify content integrity before returning bytes.
        // The sidecar carries a SHA-256 stamped at `put` time; if the
        // bytes-on-disk no longer hash to that value (bit-flip, opportunistic
        // tampering of bytes-only in a shared store, or a tampered sidecar
        // pointing at the wrong hash), we fail closed by treating the read
        // as a miss. The engine then falls through to the rebuild path
        // (Cook Standard §{exec.cache.integrity}). A missing or unreadable
        // sidecar is treated identically — without a recorded hash we have
        // no integrity proof and MUST NOT install the bytes.
        let meta_path = path.with_extension("meta.json");
        let meta_bytes = match std::fs::read(&meta_path) {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                tracing::warn!(
                    "cache integrity: missing sidecar for {}; treating as miss",
                    path.display()
                );
                return Ok(None);
            }
            Err(e) => {
                return Err(BackendError::Other(format!(
                    "read meta {}: {e}",
                    meta_path.display()
                )))
            }
        };
        let meta: ArtifactMeta = match serde_json::from_slice(&meta_bytes) {
            Ok(m) => m,
            Err(e) => {
                tracing::warn!(
                    "cache integrity: malformed sidecar at {} ({e}); treating as miss",
                    meta_path.display()
                );
                return Ok(None);
            }
        };
        let mut hasher = Sha256::new();
        hasher.update(&bytes);
        let actual: [u8; 32] = hasher.finalize().into();
        if actual != meta.content_hash {
            tracing::warn!(
                "cache integrity: content_hash mismatch for {} (sidecar={}, actual={}); failing closed as miss",
                path.display(),
                hex::encode(meta.content_hash),
                hex::encode(actual),
            );
            return Ok(None);
        }
        Ok(Some(bytes))
    }

    fn put(&self, key: &CloudKey, bytes: &[u8], meta: &ArtifactMeta) -> BackendResult<()> {
        let path = self.path_for(key);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| BackendError::Other(format!("mkdir {}: {e}", parent.display())))?;
        }

        // CS-0054: compute the SHA-256 of the artifact bytes once. Used by
        // CS-0055 below to detect content drift cheaply, and stamped into
        // the sidecar as the integrity primitive `get` verifies against.
        let mut hasher = Sha256::new();
        hasher.update(bytes);
        let content_hash: [u8; 32] = hasher.finalize().into();

        // CS-0055: idempotency contract. Before writing, check whether an
        // artifact already exists at this key. If yes and its recorded
        // content_hash matches SHA-256(bytes), this is an idempotent re-put
        // and we return Ok(()) without rewriting (a correct rebuild
        // deterministically produced the same bytes — common case). If yes
        // and the hashes differ, this is a key collision: two distinct byte
        // sequences claiming the same key. We refuse to overwrite and return
        // BackendError::Other with a diagnostic. A missing or unreadable
        // sidecar is treated as "no existing artifact" and we write through
        // (partial-write recovery from CS-0054 §3.2).
        let meta_path = path.with_extension("meta.json");
        let path_exists = path.exists();
        if path_exists {
            match std::fs::read(&meta_path) {
                Ok(existing_meta_bytes) => {
                    match serde_json::from_slice::<ArtifactMeta>(&existing_meta_bytes) {
                        Ok(existing) => {
                            // Pre-CS-0054 sidecars deserialize with the zero
                            // sentinel for content_hash. Treat that as "no
                            // recorded hash" and write through — same upgrade
                            // boundary the read-side fail-closed already
                            // covers (cf. CS-0055 spec §7).
                            if existing.content_hash == ArtifactMeta::zero_content_hash() {
                                tracing::warn!(
                                    "cache idempotency: pre-CS-0054 sentinel content_hash at {}; treating as no prior artifact",
                                    meta_path.display(),
                                );
                            } else if existing.content_hash == content_hash {
                                // Idempotent re-put — same bytes. No-op.
                                return Ok(());
                            } else {
                                let key_hex = hex::encode(key);
                                return Err(BackendError::Other(format!(
                                    "artifact key conflict at {key_hex}: existing content_hash differs from new bytes \
                                     (existing={}, new={})",
                                    hex::encode(existing.content_hash),
                                    hex::encode(content_hash),
                                )));
                            }
                        }
                        Err(e) => {
                            // Malformed sidecar — no recoverable hash to
                            // compare against. Fall through to write path.
                            tracing::warn!(
                                "cache idempotency: malformed sidecar at {} ({e}); treating as no prior artifact",
                                meta_path.display(),
                            );
                        }
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    // Missing sidecar — partial write recovery. Fall through.
                    tracing::warn!(
                        "cache idempotency: missing sidecar for {}; treating as no prior artifact",
                        path.display(),
                    );
                }
                Err(e) => {
                    return Err(BackendError::Other(format!(
                        "read meta {}: {e}",
                        meta_path.display()
                    )));
                }
            }
        }

        // Atomic write via tmp + rename.
        let tmp = path.with_extension("tmp");
        std::fs::write(&tmp, bytes)
            .map_err(|e| BackendError::Other(format!("write {}: {e}", tmp.display())))?;
        std::fs::rename(&tmp, &path)
            .map_err(|e| BackendError::Other(format!("rename {}: {e}", path.display())))?;

        // CS-0054: stamp the SHA-256 of the artifact bytes into the sidecar.
        // The caller's `meta.content_hash` is treated as a placeholder
        // (typically the zero sentinel) and overwritten here — the contract
        // is that `put` is the sole authority on the persisted hash. `get`
        // verifies against this value on every restore.
        let stamped = ArtifactMeta {
            content_hash,
            ..meta.clone()
        };

        // Sidecar metadata. Atomic write via tmp + rename so a partially
        // written sidecar can never be observed alongside fully-written bytes.
        let meta_tmp = path.with_extension("meta.json.tmp");
        let meta_bytes = serde_json::to_vec(&stamped)
            .map_err(|e| BackendError::Other(format!("serialize meta: {e}")))?;
        std::fs::write(&meta_tmp, &meta_bytes)
            .map_err(|e| BackendError::Other(format!("write meta {}: {e}", meta_tmp.display())))?;
        std::fs::rename(&meta_tmp, &meta_path)
            .map_err(|e| BackendError::Other(format!("rename meta {}: {e}", meta_path.display())))?;
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
    use std::collections::BTreeSet;

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
            content_hash: ArtifactMeta::zero_content_hash(),
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

    // ─── CS-0054: SHA-256 content_hash integrity ────────────────────────────

    /// `put` MUST stamp `meta.content_hash` with the SHA-256 of the bytes,
    /// overwriting whatever placeholder the caller passed. The persisted
    /// sidecar reflects the stamped value.
    #[test]
    fn put_computes_sha256_in_meta() {
        let dir = tempfile::tempdir().expect("tempdir");
        let backend = LocalBackend::new(dir.path().to_path_buf());
        let k = key(0xC5);
        let bytes = b"hello cs-0054";
        // Caller passes the zero sentinel; put must overwrite.
        let mut meta = sample_meta();
        meta.content_hash = [0u8; 32];
        backend.put(&k, bytes, &meta).expect("put");

        let path = backend.path_for(&k);
        let meta_path = path.with_extension("meta.json");
        let restored: ArtifactMeta =
            serde_json::from_slice(&std::fs::read(&meta_path).expect("read meta"))
                .expect("deserialize");

        // Known-answer check against an independent SHA-256.
        let expected = <Sha256 as Digest>::digest(bytes);
        let expected_arr: [u8; 32] = expected.into();
        assert_eq!(
            restored.content_hash, expected_arr,
            "put must stamp content_hash with SHA-256(bytes)"
        );
        assert_ne!(
            restored.content_hash,
            [0u8; 32],
            "put must overwrite the caller's zero sentinel"
        );
    }

    /// Happy round-trip: put bytes, get bytes, content_hash verifies, bytes
    /// returned are byte-identical to bytes put.
    #[test]
    fn get_succeeds_when_bytes_match_meta() {
        let dir = tempfile::tempdir().expect("tempdir");
        let backend = LocalBackend::new(dir.path().to_path_buf());
        let k = key(0xC6);
        let bytes = b"round-trip payload";
        backend.put(&k, bytes, &sample_meta()).expect("put");

        let got = backend.get(&k).expect("get").expect("hit");
        assert_eq!(got, bytes, "get returns the bytes put under this key");
    }

    /// Tampering the bytes-on-disk after `put` MUST cause `get` to return
    /// `None`. The threat model is bit-flip / disk corruption locally and
    /// opportunistic byte-only tampering on a shared backend; we MUST NOT
    /// hand the tampered bytes to the engine's restore path.
    #[test]
    fn get_fails_closed_on_byte_tamper() {
        let dir = tempfile::tempdir().expect("tempdir");
        let backend = LocalBackend::new(dir.path().to_path_buf());
        let k = key(0xC7);
        let bytes = b"original bytes";
        backend.put(&k, bytes, &sample_meta()).expect("put");

        // Mutate the on-disk artifact bytes — sidecar is left intact, so
        // the recorded SHA-256 no longer matches.
        let path = backend.path_for(&k);
        std::fs::write(&path, b"TAMPERED bytes").expect("tamper write");

        let got = backend.get(&k).expect("get");
        assert!(
            got.is_none(),
            "byte tamper must surface as a miss (fail closed); got Some"
        );
    }

    /// Tampering the sidecar (e.g., flipping `content_hash` to the SHA-256
    /// of bytes the attacker wants the engine to install) MUST also cause
    /// `get` to return `None`. The attacker would need to consistently
    /// rewrite both bytes AND the sidecar's hash field — that case is
    /// out of scope (deferred to signed-artifact / SLSA work) — but
    /// rewriting only one side MUST fail closed.
    #[test]
    fn get_fails_closed_on_meta_tamper() {
        let dir = tempfile::tempdir().expect("tempdir");
        let backend = LocalBackend::new(dir.path().to_path_buf());
        let k = key(0xC8);
        let bytes = b"original bytes 2";
        backend.put(&k, bytes, &sample_meta()).expect("put");

        // Read the sidecar, flip content_hash to a different value
        // (here we use the SHA-256 of *different* bytes), write back.
        let path = backend.path_for(&k);
        let meta_path = path.with_extension("meta.json");
        let mut meta: ArtifactMeta =
            serde_json::from_slice(&std::fs::read(&meta_path).expect("read meta"))
                .expect("deserialize");
        let bogus = <Sha256 as Digest>::digest(b"not the real bytes");
        meta.content_hash = bogus.into();
        std::fs::write(&meta_path, serde_json::to_vec(&meta).expect("serialize")).expect("rewrite");

        let got = backend.get(&k).expect("get");
        assert!(
            got.is_none(),
            "meta tamper must surface as a miss (fail closed); got Some"
        );
    }

    /// A sidecar that is missing entirely (for whatever reason — partial
    /// delete, partial replication on a remote backend, manual interference)
    /// MUST be treated as a miss: without a recorded hash, the implementation
    /// has no integrity proof and MUST NOT install the bytes.
    #[test]
    fn get_fails_closed_on_missing_meta() {
        let dir = tempfile::tempdir().expect("tempdir");
        let backend = LocalBackend::new(dir.path().to_path_buf());
        let k = key(0xC9);
        backend.put(&k, b"x", &sample_meta()).expect("put");

        let path = backend.path_for(&k);
        let meta_path = path.with_extension("meta.json");
        std::fs::remove_file(&meta_path).expect("remove sidecar");

        let got = backend.get(&k).expect("get");
        assert!(
            got.is_none(),
            "missing sidecar must surface as a miss; got Some"
        );
    }

    // ─── CS-0055: backend trait idempotency contract ────────────────────────

    /// `put` of identical bytes to an existing key MUST succeed (idempotent
    /// re-put). A correct rebuild that deterministically produces the same
    /// bytes is the common case; the second `put` is a no-op.
    #[test]
    fn put_idempotent_on_same_bytes() {
        let dir = tempfile::tempdir().expect("tempdir");
        let backend = LocalBackend::new(dir.path().to_path_buf());
        let k = key(0x55);
        let bytes = b"identical payload";
        backend.put(&k, bytes, &sample_meta()).expect("first put");
        backend
            .put(&k, bytes, &sample_meta())
            .expect("re-put with identical bytes must succeed");

        // Bytes still readable round-trip.
        let got = backend.get(&k).expect("get").expect("hit");
        assert_eq!(got, bytes);
    }

    /// `put` of conflicting bytes to an existing key MUST be rejected with
    /// `BackendError::Other`. The diagnostic MUST name the key in hex and
    /// describe the conflict. The implementation MUST NOT overwrite the
    /// prior bytes.
    #[test]
    fn put_rejects_conflict() {
        let dir = tempfile::tempdir().expect("tempdir");
        let backend = LocalBackend::new(dir.path().to_path_buf());
        let k = key(0x56);
        let bytes_a = b"payload alpha";
        let bytes_b = b"payload bravo";
        backend.put(&k, bytes_a, &sample_meta()).expect("put a");

        let err = backend
            .put(&k, bytes_b, &sample_meta())
            .expect_err("conflicting put must error");
        let err_msg = err.to_string();
        assert!(
            err_msg.contains("conflict"),
            "diagnostic must mention 'conflict'; got: {err_msg}"
        );
        let key_hex = hex::encode(k);
        assert!(
            err_msg.contains(&key_hex),
            "diagnostic must name the key in hex ({key_hex}); got: {err_msg}"
        );
        match err {
            BackendError::Other(_) => {}
            other => panic!("expected BackendError::Other, got {other:?}"),
        }

        // Prior bytes must still be on disk and readable.
        let got = backend.get(&k).expect("get").expect("hit");
        assert_eq!(
            got, bytes_a,
            "conflicting put must NOT overwrite prior bytes"
        );
    }

    /// If the meta sidecar is missing (partial-write recovery case), `put`
    /// MUST treat the entry as if no prior artifact existed and write
    /// through. This restores a healthy entry from a half-written state.
    #[test]
    fn put_recovers_from_missing_meta() {
        let dir = tempfile::tempdir().expect("tempdir");
        let backend = LocalBackend::new(dir.path().to_path_buf());
        let k = key(0x57);
        let bytes = b"recovery payload";
        backend.put(&k, bytes, &sample_meta()).expect("put 1");

        // Manually delete the sidecar — simulates partial replication or
        // interrupted write between bytes-rename and sidecar-rename.
        let path = backend.path_for(&k);
        let meta_path = path.with_extension("meta.json");
        std::fs::remove_file(&meta_path).expect("remove sidecar");

        // Re-put MUST succeed (write-through), restoring a complete entry.
        backend
            .put(&k, bytes, &sample_meta())
            .expect("re-put after missing sidecar must succeed");

        // Round-trip works again (sidecar restored, bytes match).
        let got = backend.get(&k).expect("get").expect("hit");
        assert_eq!(got, bytes);
    }

    /// If the meta sidecar is present but unparseable (corrupt), `put` MUST
    /// treat the entry as if no prior artifact existed and write through.
    /// A single corrupt sidecar must not permanently brick a key.
    #[test]
    fn put_recovers_from_corrupt_meta() {
        let dir = tempfile::tempdir().expect("tempdir");
        let backend = LocalBackend::new(dir.path().to_path_buf());
        let k = key(0x58);
        let bytes = b"corrupt-meta payload";
        backend.put(&k, bytes, &sample_meta()).expect("put 1");

        // Manually corrupt the sidecar with non-JSON garbage.
        let path = backend.path_for(&k);
        let meta_path = path.with_extension("meta.json");
        std::fs::write(&meta_path, b"this is not JSON {{{ ::: garbage")
            .expect("corrupt sidecar");

        // Re-put MUST succeed (write-through), restoring a parseable sidecar.
        backend
            .put(&k, bytes, &sample_meta())
            .expect("re-put after corrupt sidecar must succeed");

        // Round-trip works again.
        let got = backend.get(&k).expect("get").expect("hit");
        assert_eq!(got, bytes);
    }
}
