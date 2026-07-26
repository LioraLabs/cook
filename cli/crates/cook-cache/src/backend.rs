//! `LocalBackend` — the v3 filesystem implementation of `CacheBackend`.
//!
//! The trait, key types, and key composition (`cloud_key`, `artifact_key`)
//! live in `cook-fingerprint::backend`; this module is the persistence side.
//! For back-compat we re-export the trait/key types here so existing callers
//! that say `cook_cache::backend::*` continue to compile.

use std::fs::File;
use std::io::{self, Cursor, Read, Write};
use std::path::PathBuf;

use sha2::{Digest, Sha256};

pub use cook_fingerprint::backend::{
    artifact_key, cloud_key, ArtifactMeta, BackendConfig, BackendError, BackendResult, CacheBackend,
    CloudKey, CloudKeyInputs, DeterminantManifest, EvictCandidate,
};
pub use cook_fingerprint::evict::{
    is_size_sweep_exempt, plan_eviction, EvictPlan, EvictPolicy, DEFAULT_LOW_WATER,
    SIZE_SWEEP_EXEMPT_KINDS,
};

/// Streaming SHA-256 verifier: wraps an `R: Read`, tees bytes through a
/// hasher, and on EOF compares the finalized hash to `expected`. On
/// mismatch, the EOF read returns `io::Error` of kind `InvalidData`.
///
/// This is the streaming-equivalent of CS-0054's read-side self-verify:
/// without it, a multi-GB cache restore would have to materialise the full
/// artifact into a `Vec<u8>` before verification, which is the OOM path
/// CS-0056 was created to close.
///
/// Generic over `R: Read` so callers can wrap a `File`, an HTTP-body
/// reader, a `Cursor<Vec<u8>>`, etc., with no allocation in the hot path
/// beyond the per-instance `Sha256` state.
pub struct VerifyingReader<R: Read> {
    inner: R,
    hasher: Sha256,
    expected: [u8; 32],
    /// Once we've raised the EOF mismatch error (or matched cleanly), we
    /// don't want to re-finalize on a subsequent read attempt — `Sha256`
    /// is consumed by `finalize()`. We track terminal state explicitly.
    done: bool,
}

impl<R: Read> VerifyingReader<R> {
    pub fn new(inner: R, expected: [u8; 32]) -> Self {
        Self {
            inner,
            hasher: Sha256::new(),
            expected,
            done: false,
        }
    }
}

impl<R: Read> Read for VerifyingReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.done {
            // After a successful EOF (or after a raised mismatch), every
            // subsequent read is EOF. Honest readers stop on the first 0;
            // defensive ones keep calling — keep returning 0 (or the same
            // error if we already raised one would be ideal, but `Sha256`
            // is consumed and we can't recompute. The mismatch was raised
            // on the EOF read; that's the signal callers contract on).
            return Ok(0);
        }
        let n = self.inner.read(buf)?;
        if n == 0 {
            // EOF — finalize and check.
            self.done = true;
            // Take the hasher out so we can call `finalize()` (which
            // consumes self).
            let hasher = std::mem::replace(&mut self.hasher, Sha256::new());
            let actual: [u8; 32] = hasher.finalize().into();
            if actual != self.expected {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!(
                        "cache integrity: content_hash mismatch (expected={}, actual={})",
                        hex::encode(self.expected),
                        hex::encode(actual),
                    ),
                ));
            }
            return Ok(0);
        }
        self.hasher.update(&buf[..n]);
        Ok(n)
    }
}

/// Convenience helper: read the full artifact bytes into a `Vec<u8>`.
/// Wraps `CacheBackend::get` for callers that already need the bytes
/// resident in memory; for streaming callers, prefer `get` directly.
///
/// The streaming verification is enforced inside the returned reader, so
/// `read_to_end` here surfaces any tampering as an `io::Error` (mapped to
/// `BackendError::Other` for the trait's error type).
pub fn get_bytes(
    backend: &dyn CacheBackend,
    key: &CloudKey,
) -> BackendResult<Option<Vec<u8>>> {
    let Some(mut reader) = backend.get(key)? else {
        return Ok(None);
    };
    let mut bytes = Vec::new();
    match reader.read_to_end(&mut bytes) {
        Ok(_) => Ok(Some(bytes)),
        Err(e) if e.kind() == io::ErrorKind::InvalidData => {
            // CS-0054 read-side fail-closed: the streaming verification
            // detected tampering. Surface as a miss (Ok(None)) rather
            // than as a transport error — the engine treats the same way.
            tracing::warn!("cache integrity: streaming verification failed: {e}");
            Ok(None)
        }
        Err(e) => Err(BackendError::Other(format!("read streaming body: {e}"))),
    }
}

/// Convenience helper: write `bytes` to the backend through the streaming
/// `put`. Wraps `CacheBackend::put` for callers that already have the
/// bytes in hand; for streaming callers (genuinely large artifacts that
/// originate on disk or from the network), prefer `put` directly.
pub fn put_bytes(
    backend: &dyn CacheBackend,
    key: &CloudKey,
    bytes: &[u8],
    meta: &mut ArtifactMeta,
) -> BackendResult<()> {
    let mut cursor = Cursor::new(bytes);
    backend.put(key, &mut cursor, meta)
}

pub struct LocalBackend {
    root: PathBuf,
    /// CS-0057 tunables. `LocalBackend` honours `max_artifact_bytes` at
    /// `put` time (streamed-byte counter aborts oversize puts); the
    /// `timeout`, `max_retries`, `backoff_initial`, and `backoff_max`
    /// fields are no-ops for disk I/O — they're documented and threaded
    /// through anyway so the future `CloudBackend` constructor can accept
    /// the same `BackendConfig` shape.
    config: BackendConfig,
}

impl LocalBackend {
    /// Construct a `LocalBackend` rooted at `root` with default
    /// `BackendConfig` tunables. Equivalent to
    /// `LocalBackend::with_config(root, BackendConfig::default())`.
    pub fn new(root: PathBuf) -> Self {
        Self::with_config(root, BackendConfig::default())
    }

    /// Construct a `LocalBackend` rooted at `root` with explicit
    /// `BackendConfig` tunables. The CLI bootstrap calls this with
    /// `cloud.toml`-derived overrides; tests call it to pin specific
    /// `max_artifact_bytes` for the oversize-rejection path.
    pub fn with_config(root: PathBuf, config: BackendConfig) -> Self {
        // Ensure root exists; ignore "already exists" errors.
        let _ = std::fs::create_dir_all(&root);
        Self { root, config }
    }

    /// Borrow the active `BackendConfig`. Diagnostic accessor for tests
    /// and observability call sites; not part of the `CacheBackend` trait.
    pub fn config(&self) -> &BackendConfig {
        &self.config
    }

    /// Compute the on-disk path for a CloudKey:
    ///   {root}/{first_2_hex_chars}/{remaining_62_hex_chars}
    pub(crate) fn path_for(&self, key: &CloudKey) -> PathBuf {
        let hex = hex::encode(key);
        self.root.join(&hex[..2]).join(&hex[2..])
    }
}

/// COOK-233 — best-effort last-access bump on a CAS blob.
///
/// LRU eviction (`cook cache gc`) needs to know when an entry was last
/// used. The signal is the blob file's own mtime, restamped with a single
/// `utimensat` per cache hit; "LRU order" is therefore ascending *blob*
/// mtime. The rejected alternative was a `last_access` field inside
/// `.meta.json` rewritten on every read, which puts write amplification on
/// the hot read path.
///
/// Returns `()` on purpose: a failed touch must never fail a cache read.
/// Failures are logged at `debug!`, not `warn!` — read-only mounts and
/// exotic filesystems make this an expected outcome, not an anomaly.
fn touch_on_read(path: &std::path::Path) {
    if let Err(e) = filetime::set_file_mtime(path, filetime::FileTime::now()) {
        tracing::debug!("cache last-access: touch {} failed: {e}", path.display());
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

    fn get(&self, key: &CloudKey) -> BackendResult<Option<Box<dyn Read + Send>>> {
        Ok(self.get_with_meta(key)?.map(|(r, _)| r))
    }

    fn get_with_meta(
        &self,
        key: &CloudKey,
    ) -> BackendResult<Option<(Box<dyn Read + Send>, ArtifactMeta)>> {
        let path = self.path_for(key);

        // Read the sidecar first — without a recorded `content_hash` we
        // have no integrity proof and MUST NOT install the bytes. A
        // missing or unparseable sidecar surfaces as `Ok(None)`, same
        // as CS-0054's pre-streaming behaviour.
        let meta_path = path.with_extension("meta.json");
        let meta_bytes = match std::fs::read(&meta_path) {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                // Missing sidecar: either no entry at all, or partial
                // write recovery — both surface as miss.
                if path.exists() {
                    tracing::warn!(
                        "cache integrity: missing sidecar for {}; treating as miss",
                        path.display()
                    );
                }
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
        // CS-0054 orphan-on-upgrade: a sidecar whose `content_hash` is
        // the zero sentinel is a pre-CS-0054 entry without an integrity
        // proof. Fail closed, treat as miss, force rebuild.
        if meta.content_hash == ArtifactMeta::zero_content_hash() {
            tracing::warn!(
                    "cache integrity: legacy zero-sentinel content_hash at {}; treating as miss",
                meta_path.display()
            );
            return Ok(None);
        }

        // Open the bytes file for streaming. The `VerifyingReader`
        // wrapper tees bytes through a SHA-256 hasher and surfaces a
        // mismatch as `io::Error(InvalidData)` on EOF — the streaming
        // equivalent of CS-0054's in-memory check.
        let file = match File::open(&path) {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                // Sidecar present but bytes missing — partial write or
                // partial replication. Same fail-closed semantics.
                tracing::warn!(
                    "cache integrity: sidecar without bytes at {}; treating as miss",
                    path.display()
                );
                return Ok(None);
            }
            Err(e) => return Err(BackendError::Other(format!("open {}: {e}", path.display()))),
        };

        // COOK-233 — this is a hit; stamp last-access. Every miss path above
        // has already returned, so mtime moves only on genuine hits, and
        // `get` delegates here, so both read entry points are covered.
        // `batch_query` is an existence probe, not a read, and is excluded.
        //
        // SAFETY ARGUMENT — this touch is inert *only because restore is a
        // byte copy, not a hardlink*. `cook_fingerprint::check::restore_one`
        // does `read_to_end` -> write tmp -> rename, so the CAS blob and the
        // restored workspace file are different inodes and restamping the
        // blob cannot perturb workspace-file mtimes. If restore is ever
        // changed to hardlink or reflink the blob into the workspace, the two
        // become the same inode and this touch would corrupt input-freshness
        // detection. Do not make that change without removing this touch.
        //
        // Accepted imprecision (not a defect): the touch fires here, so ANY
        // read that opens the blob and then abandons it still counts as an
        // access. That covers a *tampered* blob (`VerifyingReader` only
        // rejects at EOF, strictly after this point) and every caller-side
        // bail in `restore_one` — `create_dir_all` failure, `read_to_end`
        // failure, warm-path xxh3 mismatch. All bump mtime for a read that
        // ends as a miss. LRU is marginally less precise; there is no
        // correctness consequence.
        touch_on_read(&path);

        Ok(Some((Box::new(VerifyingReader::new(file, meta.content_hash)), meta)))
    }

    fn put(
        &self,
        key: &CloudKey,
        reader: &mut dyn Read,
        meta: &mut ArtifactMeta,
    ) -> BackendResult<()> {
        let path = self.path_for(key);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| BackendError::Other(format!("mkdir {}: {e}", parent.display())))?;
        }

        // Stream the bytes to a temporary file, hashing as they flow.
        // The temp file is our scratch space until conflict detection
        // and the caller-claimed-hash check pass; on rejection we discard
        // it without ever exposing the new bytes to readers.
        let tmp = path.with_extension("tmp");
        let mut tmp_file = File::create(&tmp)
            .map_err(|e| BackendError::Other(format!("create {}: {e}", tmp.display())))?;
        let mut hasher = Sha256::new();
        let mut buf = [0u8; 64 * 1024];
        let mut total: u64 = 0;
        let limit = self.config.max_artifact_bytes;
        loop {
            let n = reader.read(&mut buf).map_err(|e| {
                let _ = std::fs::remove_file(&tmp);
                BackendError::Other(format!("read source for {}: {e}", path.display()))
            })?;
            if n == 0 {
                break;
            }
            // CS-0057: enforce `max_artifact_bytes` as bytes flow. The
            // check happens during streaming, not pre-flight — the caller
            // may not know the size up front (e.g., a streaming source).
            // On overflow, abort: discard the temp file, return an error
            // that names the limit. No partial bytes ever surface to a
            // reader because the rename-into-place commit hasn't run.
            total = total.saturating_add(n as u64);
            if total > limit {
                drop(tmp_file);
                let _ = std::fs::remove_file(&tmp);
                return Err(BackendError::Other(format!(
                    "artifact exceeds max_artifact_bytes ({total}); cap {limit}"
                )));
            }
            hasher.update(&buf[..n]);
            tmp_file.write_all(&buf[..n]).map_err(|e| {
                let _ = std::fs::remove_file(&tmp);
                BackendError::Other(format!("write {}: {e}", tmp.display()))
            })?;
        }
        tmp_file.flush().map_err(|e| {
            let _ = std::fs::remove_file(&tmp);
            BackendError::Other(format!("flush {}: {e}", tmp.display()))
        })?;
        drop(tmp_file);
        let computed: [u8; 32] = hasher.finalize().into();

        // Caller-claimed `content_hash` consistency check. The standard
        // calling convention is to pass the zero sentinel and let `put`
        // stamp the computed hash; a non-zero caller-claimed hash that
        // matches is honoured (idempotent re-stamp), but a non-zero
        // hash that doesn't match the bytes is a caller bug — refuse to
        // persist a sidecar inconsistent with the bytes.
        let zero = ArtifactMeta::zero_content_hash();
        if meta.content_hash != zero && meta.content_hash != computed {
            let _ = std::fs::remove_file(&tmp);
            return Err(BackendError::Other(format!(
                "caller-claimed content_hash differs from streamed bytes \
                 (claimed={}, computed={})",
                hex::encode(meta.content_hash),
                hex::encode(computed),
            )));
        }

        // CS-0055: idempotency / conflict detection against any prior
        // artifact at this key. The temp file is already written; on
        // idempotent match we discard it, on conflict we discard it,
        // and on no-prior-artifact we rename it into place.
        let meta_path = path.with_extension("meta.json");
        let path_exists = path.exists();
        if path_exists {
            match std::fs::read(&meta_path) {
                Ok(existing_meta_bytes) => {
                    match serde_json::from_slice::<ArtifactMeta>(&existing_meta_bytes) {
                        Ok(existing) => {
                            // Pre-CS-0054 sidecars deserialize with the
                            // zero sentinel for content_hash. Treat that
                            // as "no recorded hash" and write through.
                            if existing.content_hash == zero {
                                tracing::warn!(
                                    "cache idempotency: legacy sentinel content_hash at {}; treating as no prior artifact",
                                    meta_path.display(),
                                );
                            } else if existing.content_hash == computed {
                                // Idempotent re-put — same bytes. Discard
                                // the temp; stamp meta.content_hash so the
                                // caller observes the canonical hash even
                                // on the no-op path.
                                let _ = std::fs::remove_file(&tmp);
                                meta.content_hash = computed;
                                return Ok(());
                            } else {
                                let _ = std::fs::remove_file(&tmp);
                                let key_hex = hex::encode(key);
                                return Err(BackendError::Other(format!(
                                    "artifact key conflict at {key_hex}: existing content_hash differs from new bytes \
                                     (existing={}, new={})",
                                    hex::encode(existing.content_hash),
                                    hex::encode(computed),
                                )));
                            }
                        }
                        Err(e) => {
                            // Malformed sidecar — no recoverable hash.
                            // Fall through to write path.
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
                    let _ = std::fs::remove_file(&tmp);
                    return Err(BackendError::Other(format!(
                        "read meta {}: {e}",
                        meta_path.display()
                    )));
                }
            }
        }

        // Commit the temp file to its final path (atomic via rename).
        std::fs::rename(&tmp, &path)
            .map_err(|e| BackendError::Other(format!("rename {}: {e}", path.display())))?;

        // Stamp the computed hash into the caller's meta (in-place) so
        // they observe the canonical hash, then persist the sidecar.
        // The stamp is authoritative whether the caller passed the zero
        // sentinel or a matching hash; either way `meta.content_hash`
        // ends up equal to `computed`.
        meta.content_hash = computed;
        // size_bytes was historically populated by the caller; we leave
        // it untouched here (the caller's bytes-len is already correct
        // for its source). For streaming callers who don't know the
        // length up front, the streamed `total` is available — but
        // overwriting could regress callers who pre-set size_bytes
        // intentionally. Keep it caller-set; surface the streamed total
        // as a tracing field for observability.
        let _ = total; // silenced — see comment above

        let meta_tmp = path.with_extension("meta.json.tmp");
        let meta_bytes = serde_json::to_vec(meta)
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
        // `path_for` yields a path with no extension, so `with_extension`
        // here appends rather than replaces — the same construction
        // `put_manifest` uses to write this sidecar in the first place.
        // Best-effort: a sidecar that was never written is not an error.
        let _ = std::fs::remove_file(path.with_extension("provenance.json"));
        Ok(())
    }

    fn health(&self) -> BackendResult<()> {
        std::fs::metadata(&self.root)
            .map(|_| ())
            .map_err(|e| BackendError::Other(format!("root {}: {e}", self.root.display())))
    }

    fn put_manifest(
        &self,
        key: &CloudKey,
        manifest: &DeterminantManifest,
    ) -> BackendResult<()> {
        let path = self.path_for(key).with_extension("provenance.json");
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| BackendError::Other(format!("mkdir {}: {e}", parent.display())))?;
        }
        let bytes = serde_json::to_vec(manifest)
            .map_err(|e| BackendError::Other(format!("serialize manifest: {e}")))?;
        // Build the temp path explicitly: `path` already ends in
        // `…62.provenance.json`, so `with_extension("…tmp")` would replace
        // the `.json` segment and mangle the sibling base. Append `.tmp` to
        // the full file name instead so temp and final are siblings.
        let tmp = path.with_file_name(format!(
            "{}.tmp",
            path.file_name().unwrap().to_string_lossy()
        ));
        std::fs::write(&tmp, &bytes)
            .map_err(|e| BackendError::Other(format!("write {}: {e}", tmp.display())))?;
        std::fs::rename(&tmp, &path)
            .map_err(|e| BackendError::Other(format!("rename {}: {e}", path.display())))?;
        Ok(())
    }

    fn get_manifest(&self, key: &CloudKey) -> BackendResult<Option<DeterminantManifest>> {
        let path = self.path_for(key).with_extension("provenance.json");
        match std::fs::read(&path) {
            Ok(b) => match serde_json::from_slice::<DeterminantManifest>(&b) {
                Ok(m) => Ok(Some(m)),
                Err(e) => {
                    tracing::warn!(
                        "cache manifest: malformed sidecar at {} ({e}); treating as absent",
                        path.display()
                    );
                    Ok(None)
                }
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(BackendError::Other(format!(
                "read manifest {}: {e}",
                path.display()
            ))),
        }
    }
}

/// Exactly `len` lowercase hex characters — the shape used both for shard
/// directory names (2) and blob file names (62). Anything else (uppercase,
/// wrong length, non-hex) fails the predicate. This single predicate is what
/// excludes `.meta.json`, `.provenance.json`, `.tmp`, and `.meta.json.tmp`
/// sidecars from `LocalBackend::enumerate` — no extension-specific logic
/// needed.
fn is_lowercase_hex(s: &str, len: usize) -> bool {
    s.len() == len && s.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

// Deliberately a SEPARATE `impl LocalBackend` block (not folded into the one
// above `impl CacheBackend for LocalBackend`) so this addition stays
// non-adjacent to COOK-233's concurrent edits inside `get`/`get_with_meta`.
impl LocalBackend {
    /// Walk the on-disk CAS at `{root}/{2 hex}/{62 hex}` and return one
    /// `EvictCandidate` per blob found. Ordering is unspecified; callers
    /// that need a stable order (e.g. LRU eviction) must sort.
    ///
    /// Deliberately an **inherent** method, not a `CacheBackend` trait
    /// method (milestone D2): enumerating every artifact a backend holds is
    /// exactly the capability a client of a shared, multi-tenant store (a
    /// future `CloudBackend`) must never be granted. Keeping `enumerate`
    /// off the trait means it can only exist where it's safe — the local,
    /// single-tenant filesystem backend — so a `Box<dyn CacheBackend>` call
    /// site can never accidentally acquire it.
    ///
    /// Implemented with plain two-level `std::fs::read_dir` (matching
    /// `path_for`'s `{2}/{62}` layout); no `walkdir` dependency needed for a
    /// fixed-depth tree.
    pub fn enumerate(&self) -> BackendResult<Vec<EvictCandidate>> {
        let mut out = Vec::new();

        let root_entries = match std::fs::read_dir(&self.root) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                // `with_config` always `create_dir_all`s the root, so in
                // practice this only fires for a backend whose root was
                // removed after construction, or a test pointed directly at
                // a path that was never created. Either way: no blobs on
                // disk, so an empty result — not an error — is correct.
                return Ok(out);
            }
            Err(e) => {
                return Err(BackendError::Other(format!(
                    "read_dir {}: {e}",
                    self.root.display()
                )))
            }
        };

        for shard_entry in root_entries {
            // Per-entry I/O failure below the root is skipped, not fatal —
            // one unreadable directory entry must not fail a 30k-object walk.
            let Ok(shard_entry) = shard_entry else {
                continue;
            };
            let shard_os_name = shard_entry.file_name();
            let Some(shard_name) = shard_os_name.to_str() else {
                continue;
            };
            if !is_lowercase_hex(shard_name, 2) {
                continue;
            }
            let shard_path = shard_entry.path();
            let Ok(shard_entries) = std::fs::read_dir(&shard_path) else {
                continue;
            };

            for blob_entry in shard_entries {
                let Ok(blob_entry) = blob_entry else {
                    continue;
                };
                let blob_os_name = blob_entry.file_name();
                let Some(file_name) = blob_os_name.to_str() else {
                    continue;
                };
                if !is_lowercase_hex(file_name, 62) {
                    continue;
                }

                let Ok(key_bytes) = hex::decode(format!("{shard_name}{file_name}")) else {
                    continue;
                };
                let Ok(key): Result<CloudKey, _> = key_bytes.try_into() else {
                    continue;
                };

                // `DirEntry::metadata()` (not `fs::metadata(path)`): it
                // stats relative to the already-open directory (cheaper at
                // scale) and, importantly, does NOT follow symlinks — a
                // 62-hex-named symlink pointing outside the CAS must not
                // report its target's size as CAS-resident.
                let metadata = match blob_entry.metadata() {
                    Ok(metadata) => metadata,
                    Err(e) => {
                        tracing::debug!(
                            "enumerate: unreadable blob metadata at {} ({e}); skipped",
                            blob_entry.path().display()
                        );
                        continue;
                    }
                };
                let size = metadata.len();
                let last_access = metadata
                    .modified()
                    .ok()
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| d.as_secs())
                    .unwrap_or(0);

                // Sidecar read is best-effort: a missing or malformed
                // `.meta.json` is NOT an error here. The bytes still occupy
                // disk and MUST still show up so `cook cache du` accounts
                // for them; we just can't attribute them to a recipe.
                let meta_path = shard_path.join(format!("{file_name}.meta.json"));
                let (kind, recipe_namespace) = match std::fs::read(&meta_path) {
                    Ok(bytes) => match serde_json::from_slice::<ArtifactMeta>(&bytes) {
                        Ok(meta) => (meta.kind, meta.recipe_namespace),
                        Err(e) => {
                            tracing::debug!(
                                "enumerate: malformed sidecar at {} ({e}); orphan blob",
                                meta_path.display()
                            );
                            (None, String::new())
                        }
                    },
                    Err(e) => {
                        tracing::debug!(
                            "enumerate: no sidecar at {} ({e}); orphan blob",
                            meta_path.display()
                        );
                        (None, String::new())
                    }
                };

                out.push(EvictCandidate {
                    key,
                    size,
                    last_access,
                    kind,
                    recipe_namespace,
                });
            }
        }

        Ok(out)
    }

    /// Execute `plan`: remove the blob and both sidecars for every victim.
    ///
    /// Deliberately an **inherent** method, not a `CacheBackend` trait method
    /// (milestone D2), for the same reason as `enumerate`: a client of a
    /// shared, multi-tenant store must never be able to issue deletes.
    /// `plan_eviction` is pure policy shared with the future cloud-side
    /// sweep; only the local, single-tenant filesystem backend is trusted to
    /// actually apply it.
    ///
    /// Per victim: remove the blob with a single `remove_file` call and
    /// count `size` toward the returned `EvictOutcome` only if THIS call
    /// was the one that actually removed it (`Ok`). Stat-then-delete would
    /// open a TOCTOU window: a concurrent sweep could remove the blob
    /// between the stat and the delete, and this run would still count
    /// `size`, double-counting the freed bytes across both sweeps — exactly
    /// what this method must not do. Deriving "removed" from the delete's
    /// own result closes that window, and also stops counting a blob whose
    /// removal failed for some other reason (e.g. a permission error): the
    /// bytes are still on disk, so they must not be reported as freed.
    ///
    /// The sidecars are removed unconditionally (best-effort), whether or
    /// not the blob was present, so a half-removed object still gets
    /// cleaned up.
    ///
    /// Never returns `Err` for a per-object filesystem failure; every
    /// removal is best-effort, matching `delete`'s shape. The `BackendResult`
    /// return type is kept for symmetry with `enumerate` and so a future
    /// cloud-shaped caller (which *can* fail wholesale, e.g. on an auth or
    /// connectivity error) has a slot to put it in.
    ///
    /// `.tmp` files are never victims — `enumerate` cannot produce them
    /// (see `is_lowercase_hex`), so `plan.victims` never names one and no
    /// extra guard is needed here.
    pub fn apply_eviction(&self, plan: &EvictPlan) -> BackendResult<EvictOutcome> {
        let mut outcome = EvictOutcome::default();

        for victim in &plan.victims {
            let path = self.path_for(&victim.key);
            let blob_removed = std::fs::remove_file(&path).is_ok();

            let _ = std::fs::remove_file(path.with_extension("meta.json"));
            let _ = std::fs::remove_file(path.with_extension("provenance.json"));

            if blob_removed {
                outcome.objects += 1;
                outcome.bytes = outcome.bytes.saturating_add(victim.size);
            }
        }

        Ok(outcome)
    }
}

/// What a sweep actually removed, as opposed to what the plan projected.
/// `plan.freed_bytes` / `plan.victims.len()` are the projection at plan time;
/// this is ground truth as of `apply_eviction`'s actual filesystem walk —
/// the two can differ when a victim vanished between planning and applying
/// (see `apply_eviction`'s doc comment).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct EvictOutcome {
    /// Count of victims whose blob this call's own `remove_file` actually
    /// removed (i.e. it was still present and the removal succeeded).
    pub objects: usize,
    /// Sum of `size` over those same victims.
    pub bytes: u64,
}

#[cfg(test)]
#[path = "tests/backend_tests.rs"]
mod tests;
