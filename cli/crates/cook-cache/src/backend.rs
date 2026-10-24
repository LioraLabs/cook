//! `LocalBackend` — the v3 filesystem implementation of `CacheBackend`.
//!
//! The trait lives in `crate::cas_backend`, and the key types and key
//! composition (`cloud_key`, `artifact_key`) in `cook_contracts::cache::cas`;
//! this module is the persistence side.
//! For back-compat we re-export the trait/key types here so existing callers
//! that say `cook_cache::backend::*` continue to compile.

use std::fs::File;
use std::io::{self, Cursor, Read, Write};
use std::path::PathBuf;

use cook_contracts::cache::cas::artifact_kind;
use sha2::{Digest, Sha256};

pub use crate::cas_backend::{
    artifact_key, cloud_key, ArtifactMeta, BackendConfig, BackendError, BackendResult, CacheBackend,
    CloudKey, CloudKeyInputs, DeterminantManifest, EvictCandidate,
};
pub use cook_contracts::evict::{
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

    /// Sidecar read + blob open, unverified: the miss rules and the
    /// last-access touch of `get_with_meta`, without the `VerifyingReader`.
    /// For a caller that forwards the bytes to someone who verifies them.
    pub(crate) fn open_raw(&self, key: &CloudKey) -> BackendResult<Option<(File, ArtifactMeta)>> {
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
        // byte copy, not a hardlink*. `crate::check::restore_one`
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

        Ok(Some((file, meta)))
    }
}

/// COOK-233 — best-effort last-access bump on a CAS blob.
///
/// LRU eviction (`cook cache gc`) needs to know when an entry was last
/// used. The signal is the file's own mtime (a blob's, or a manifest's),
/// restamped with a single `utimensat` per cache hit; "LRU order" is
/// therefore ascending mtime of the blob or manifest. The rejected
/// alternative was a `last_access` field inside
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

/// Why a put failed, by who is to blame: the server maps these to statuses.
#[derive(Debug)]
pub(crate) enum PutError {
    /// Reading the caller's stream failed.
    Source(String),
    /// A claimed-hash mismatch (the caller's bytes disagree with its claim).
    Rejected(String),
    /// The key holds different bytes already (409 on the wire).
    Conflict(String),
    /// The stream outgrew the artifact cap (413 on the wire).
    TooLarge(String),
    /// A local filesystem failure; `kind` lets a server tell a full disk apart.
    Store { kind: io::ErrorKind, msg: String },
}

impl From<PutError> for BackendError {
    fn from(e: PutError) -> Self {
        match e {
            PutError::Source(m)
            | PutError::Rejected(m)
            | PutError::Conflict(m)
            | PutError::TooLarge(m)
            | PutError::Store { msg: m, .. } => {
                BackendError::Other(m)
            }
        }
    }
}

/// The determinant manifest's file extension: the one place the name is decided.
const MANIFEST_EXT: &str = "provenance.json";

/// The 62-hex stem of a manifest file name, or `None` if `name` is not one.
fn manifest_stem(name: &str) -> Option<&str> {
    name.strip_suffix(MANIFEST_EXT)?.strip_suffix('.')
}

impl LocalBackend {
    /// `path_for` has no extension, so `with_extension` appends the sidecar's.
    fn manifest_path(&self, key: &CloudKey) -> PathBuf {
        self.path_for(key).with_extension(MANIFEST_EXT)
    }

    /// Write the `provenance.json` sidecar for `key` (tmp + rename). The
    /// server calls this directly so an IO error keeps its `ErrorKind`.
    pub(crate) fn write_manifest(
        &self,
        key: &CloudKey,
        manifest: &DeterminantManifest,
    ) -> Result<(), PutError> {
        let path = self.manifest_path(key);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| store_err(format!("mkdir {}", parent.display()), e))?;
        }
        let bytes = serde_json::to_vec(manifest).map_err(|e| PutError::Store {
            kind: io::ErrorKind::InvalidData,
            msg: format!("serialize manifest: {e}"),
        })?;
        // `path` already ends in `.provenance.json`, so append `.tmp` to the
        // file name (`with_extension` would replace the `.json` segment).
        let tmp = path.with_file_name(format!(
            "{}.tmp",
            path.file_name().unwrap().to_string_lossy()
        ));
        // The fixed temp name is private to whoever holds the shard lock.
        let _lock = self
            .lock_shard(key)
            .map_err(|e| store_err(format!("lock shard for manifest {}", path.display()), e))?;
        std::fs::write(&tmp, &bytes)
            .map_err(|e| (e, format!("write manifest {}", tmp.display())))
            .and_then(|()| {
                std::fs::rename(&tmp, &path).map_err(|e| (e, format!("rename {}", path.display())))
            })
            .map_err(|(e, ctx)| {
                let _ = std::fs::remove_file(&tmp);
                store_err(ctx, e)
            })
    }
}

fn store_err(ctx: String, e: io::Error) -> PutError {
    PutError::Store { kind: e.kind(), msg: format!("{ctx}: {e}") }
}

/// Verified bytes in a uniquely named temp file; removed on drop unless it
/// was renamed into place (the name may by then be another process's temp).
pub(crate) struct Staged {
    tmp: PathBuf,
    computed: [u8; 32],
    placed: bool,
}

impl Staged {
    fn place(&mut self, to: &std::path::Path) -> Result<(), PutError> {
        std::fs::rename(&self.tmp, to).map_err(|e| store_err(format!("rename {}", to.display()), e))?;
        self.placed = true;
        Ok(())
    }
}

impl Drop for Staged {
    fn drop(&mut self) {
        if !self.placed {
            let _ = std::fs::remove_file(&self.tmp);
        }
    }
}

/// The advisory lock file in each shard dir, `<root>/<2hex>/<this>`: every
/// mutation of a key's files holds its shard's lock (`flock`, one open file
/// description per acquisition, so threads exclude each other too). Nothing
/// may ever delete it: unlinking a held lock file lets a second process lock
/// a fresh inode and enter beside the first. Neither 62-hex nor a staged-temp
/// name, so `enumerate` and `sweep_stale_temps` pass it by.
const SHARD_LOCK: &str = "lock";

/// Names tried for one temp before `stage` gives up.
const STAGE_ATTEMPTS: usize = 10_000;

static STAGE_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

impl LocalBackend {
    /// First half of a put: stream `reader` into a private temp in the shard
    /// dir, capped and hashed, and check the caller's claimed hash. Touches no
    /// key's state, so it needs no lock.
    pub(crate) fn stage(
        &self,
        key: &CloudKey,
        reader: &mut dyn Read,
        meta: &ArtifactMeta,
    ) -> Result<Staged, PutError> {
        let path = self.path_for(key);
        let parent = path.parent().expect("path_for has a shard dir");
        std::fs::create_dir_all(parent)
            .map_err(|e| store_err(format!("mkdir {}", parent.display()), e))?;

        // `<62hex>.<pid>.<n>.tmp` is never a 62-hex name, so `enumerate` skips it.
        // create_new: a pid alone is not unique across PID namespaces sharing a store.
        let mut attempts = 0;
        let (tmp, mut tmp_file) = loop {
            let seq = STAGE_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let tmp = path.with_extension(format!("{}.{seq}.tmp", std::process::id()));
            match std::fs::OpenOptions::new().write(true).create_new(true).open(&tmp) {
                Ok(f) => break (tmp, f),
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists && attempts < STAGE_ATTEMPTS => {
                    attempts += 1;
                }
                Err(e) => return Err(store_err(format!("create {}", tmp.display()), e)),
            }
        };
        let mut staged = Staged { tmp, computed: [0; 32], placed: false };
        let mut hasher = Sha256::new();
        let mut buf = [0u8; 64 * 1024];
        let mut cap = crate::cap::CapCounter::new(self.config.max_artifact_bytes);
        loop {
            let n = reader.read(&mut buf).map_err(|e| {
                PutError::Source(format!("read source for {}: {e}", path.display()))
            })?;
            if n == 0 {
                break;
            }
            // CS-0057: the cap is enforced as bytes flow (the caller may not
            // know the size up front); nothing is visible before the commit.
            cap.add(n as u64).map_err(PutError::TooLarge)?;
            hasher.update(&buf[..n]);
            tmp_file
                .write_all(&buf[..n])
                .map_err(|e| store_err(format!("write staged {}", staged.tmp.display()), e))?;
        }
        tmp_file
            .flush()
            .map_err(|e| store_err(format!("flush {}", staged.tmp.display()), e))?;
        drop(tmp_file);
        staged.computed = hasher.finalize().into();

        // A non-zero caller-claimed hash must match the bytes; refuse to
        // persist a sidecar inconsistent with them.
        if meta.content_hash != ArtifactMeta::zero_content_hash()
            && meta.content_hash != staged.computed
        {
            return Err(PutError::Rejected(format!(
                "caller-claimed content_hash differs from streamed bytes \
                 (claimed={}, computed={})",
                hex::encode(meta.content_hash),
                hex::encode(staged.computed),
            )));
        }
        Ok(staged)
    }

    /// The key's shard lock, held until the returned file drops. A shard dir
    /// that does not exist is `NotFound`: nothing there to mutate. Errors
    /// keep their kind and say which key's shard failed.
    fn lock_shard(&self, key: &CloudKey) -> io::Result<File> {
        let lock = self.path_for(key).with_file_name(SHARD_LOCK);
        // Writable where we can (flock emulated over fcntl, as on NFS, needs it for an
        // exclusive lock); read-only when another user's lock file refuses us. create makes no dir.
        let opened = match std::fs::OpenOptions::new().write(true).create(true).truncate(false).open(&lock) {
            Err(e) if e.kind() == io::ErrorKind::PermissionDenied => File::open(&lock),
            other => other,
        };
        opened
            .and_then(|f| f.lock().map(|()| f))
            .map_err(|e| io::Error::new(e.kind(), format!("lock shard of {}: {e}", hex::encode(key))))
    }

    /// Second half of a put: conflict detection, then blob and sidecar into
    /// place, all under the shard lock.
    pub(crate) fn commit(
        &self,
        key: &CloudKey,
        staged: Staged,
        meta: &mut ArtifactMeta,
    ) -> Result<(), PutError> {
        let _lock = self
            .lock_shard(key)
            .map_err(|e| PutError::Store { kind: e.kind(), msg: e.to_string() })?;
        self.commit_locked(key, staged, meta)
    }

    fn commit_locked(
        &self,
        key: &CloudKey,
        mut staged: Staged,
        meta: &mut ArtifactMeta,
    ) -> Result<(), PutError> {
        let path = self.path_for(key);
        let computed = staged.computed;
        let zero = ArtifactMeta::zero_content_hash();

        // CS-0055: idempotency / conflict detection against any prior artifact.
        let meta_path = path.with_extension("meta.json");
        if path.exists() {
            match std::fs::read(&meta_path) {
                Ok(existing_meta_bytes) => {
                    match serde_json::from_slice::<ArtifactMeta>(&existing_meta_bytes) {
                        Ok(existing) => {
                            // Pre-CS-0054 sidecars carry the zero sentinel:
                            // "no recorded hash", write through.
                            if existing.content_hash == zero {
                                tracing::warn!(
                                    "cache idempotency: legacy sentinel content_hash at {}; treating as no prior artifact",
                                    meta_path.display(),
                                );
                            } else if existing.content_hash == computed {
                                // Same bytes. Keep the stored blob only if it
                                // still verifies; otherwise these good bytes
                                // replace it (the sidecar already matches).
                                // The whole-blob re-hash runs under the shard lock on purpose (a concurrent
                                // writer must not replace the blob mid-check); same-shard writers wait one blob read.
                                let intact = File::open(&path)
                                    .and_then(|f| {
                                        io::copy(&mut VerifyingReader::new(f, computed), &mut io::sink())
                                    })
                                    .is_ok();
                                if !intact {
                                    tracing::warn!(
                                        "cache integrity: blob at {} fails its recorded hash; healing from the re-put",
                                        path.display(),
                                    );
                                    staged.place(&path)?;
                                }
                                meta.content_hash = computed;
                                return Ok(());
                            } else {
                                let key_hex = hex::encode(key);
                                return Err(PutError::Conflict(format!(
                                    "artifact key conflict at {key_hex}: existing content_hash differs from new bytes \
                                     (existing={}, new={})",
                                    hex::encode(existing.content_hash),
                                    hex::encode(computed),
                                )));
                            }
                        }
                        Err(e) => {
                            // Malformed sidecar: no recoverable hash, write through.
                            tracing::warn!(
                                "cache idempotency: malformed sidecar at {} ({e}); treating as no prior artifact",
                                meta_path.display(),
                            );
                        }
                    }
                }
                Err(e) if e.kind() == io::ErrorKind::NotFound => {
                    // Missing sidecar: partial write recovery, write through.
                    tracing::warn!(
                        "cache idempotency: missing sidecar for {}; treating as no prior artifact",
                        path.display(),
                    );
                }
                Err(e) => return Err(store_err(format!("read meta {}", meta_path.display()), e)),
            }
        }

        // Blob first (atomic rename), then the sidecar that makes it readable.
        staged.place(&path)?;

        // The stamp is authoritative whether the caller passed the zero
        // sentinel or a matching hash. size_bytes stays caller-set.
        meta.content_hash = computed;

        let meta_tmp = path.with_extension("meta.json.tmp");
        let meta_bytes = serde_json::to_vec(meta).map_err(|e| {
            PutError::Store { kind: io::ErrorKind::InvalidData, msg: format!("serialize meta: {e}") }
        })?;
        std::fs::write(&meta_tmp, &meta_bytes)
            .map_err(|e| store_err(format!("write meta {}", meta_tmp.display()), e))?;
        std::fs::rename(&meta_tmp, &meta_path)
            .map_err(|e| store_err(format!("rename meta {}", meta_path.display()), e))
    }
}

impl CacheBackend for LocalBackend {
    fn batch_query(&self, keys: &[CloudKey]) -> BackendResult<std::collections::BTreeSet<CloudKey>> {
        let mut hits = std::collections::BTreeSet::new();
        for k in keys {
            // Both halves: a blob alone (crash between renames, mid-eviction) is a miss on every get.
            let path = self.path_for(k);
            if path.exists() && path.with_extension("meta.json").exists() {
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
        // The `VerifyingReader` tees bytes through SHA-256 and surfaces a
        // mismatch as `io::Error(InvalidData)` on EOF.
        Ok(self.open_raw(key)?.map(|(file, meta)| {
            (
                Box::new(VerifyingReader::new(file, meta.content_hash)) as Box<dyn Read + Send>,
                meta,
            )
        }))
    }

    fn put(
        &self,
        key: &CloudKey,
        reader: &mut dyn Read,
        meta: &mut ArtifactMeta,
    ) -> BackendResult<()> {
        let staged = self.stage(key, reader, meta)?;
        Ok(self.commit(key, staged, meta)?)
    }

    fn delete(&self, key: &CloudKey) -> BackendResult<()> {
        let _lock = match self.lock_shard(key) {
            Ok(l) => l,
            // No shard dir: nothing was ever stored here.
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(BackendError::Other(e.to_string())),
        };
        let path = self.path_for(key);
        // Sidecar first: it is the commit point readers check, so a crash between the
        // removals leaves a blob nobody serves, never a sidecar vouching for a stranger blob.
        let _ = std::fs::remove_file(path.with_extension("meta.json"));
        let _ = std::fs::remove_file(&path);
        // `path_for` yields a path with no extension, so `with_extension`
        // here appends rather than replaces — the same construction
        // `put_manifest` uses to write this sidecar in the first place.
        // Best-effort: a sidecar that was never written is not an error.
        let _ = std::fs::remove_file(self.manifest_path(key));
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
        Ok(self.write_manifest(key, manifest)?)
    }

    fn get_manifest(&self, key: &CloudKey) -> BackendResult<Option<DeterminantManifest>> {
        let path = self.manifest_path(key);
        match std::fs::read(&path) {
            Ok(b) => match serde_json::from_slice::<DeterminantManifest>(&b) {
                Ok(m) => {
                    // Ages with the artifacts it serves; a malformed one is not a hit.
                    touch_on_read(&path);
                    Ok(Some(m))
                }
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
/// wrong length, non-hex) fails the predicate. This is what excludes
/// `.meta.json`, `.tmp`, and `.meta.json.tmp` sidecars from
/// `LocalBackend::enumerate`; `enumerate` strips `.provenance.json` itself
/// before applying it.
fn is_lowercase_hex(s: &str, len: usize) -> bool {
    s.len() == len && s.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

// Deliberately a SEPARATE `impl LocalBackend` block (not folded into the one
// above `impl CacheBackend for LocalBackend`) so this addition stays
// non-adjacent to COOK-233's concurrent edits inside `get`/`get_with_meta`.
impl LocalBackend {
    /// Walk the on-disk CAS at `{root}/{2 hex}/{62 hex}` and return one
    /// `EvictCandidate` per blob found, plus one per determinant manifest
    /// (`{62 hex}.provenance.json`) with no blob beside it, labelled
    /// `artifact_kind::MANIFEST`. Ordering is unspecified; callers
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
                // A manifest is listed only when no blob sits beside it: the
                // blob's own candidate already removes the manifest.
                let (stem, is_manifest) = match manifest_stem(file_name) {
                    Some(stem) => (stem, true),
                    None => (file_name, false),
                };
                if !is_lowercase_hex(stem, 62)
                    || (is_manifest && shard_path.join(stem).symlink_metadata().is_ok())
                {
                    continue;
                }

                let Ok(key_bytes) = hex::decode(format!("{shard_name}{stem}")) else {
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
                let mut size = metadata.len();
                let last_access = metadata
                    .modified()
                    .ok()
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| d.as_secs())
                    .unwrap_or(0);

                // The manifest is untrusted and up to 4 MiB: it is sized and
                // aged by its file, never parsed.
                if is_manifest {
                    out.push(EvictCandidate {
                        key,
                        size,
                        last_access,
                        kind: Some(artifact_kind::MANIFEST.to_string()),
                        recipe_namespace: String::new(),
                    });
                    continue;
                }

                // A manifest beside this blob goes with it, so it is sized with it.
                let beside = shard_path.join(format!("{file_name}.{MANIFEST_EXT}"));
                size = size.saturating_add(beside.symlink_metadata().map_or(0, |m| m.len()));

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
    /// Per victim, under its shard lock (which excludes writers and other
    /// sweeps, so stat-then-remove is not a TOCTOU): stat the blob and the
    /// manifest (no-follow), `remove_file` each, and add a file's length to
    /// `bytes` only if THIS call's remove of it succeeded. `objects` grows by
    /// one if either removal took. A file whose removal failed (e.g. a
    /// permission error) is still on disk and is not reported as freed; a
    /// manifest-only victim simply has no blob. The `.meta.json` sidecar is
    /// removed best-effort and never counted. Nothing branches on the
    /// victim's `kind`.
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
            // One victim per acquisition, so a writer never waits out the whole plan.
            let _lock = match self.lock_shard(&victim.key) {
                Ok(l) => l,
                Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
                Err(e) => {
                    tracing::warn!("evict: {e}; skipped");
                    continue;
                }
            };
            let path = self.path_for(&victim.key);
            // Sidecar first (the commit point readers check), as in `delete`.
            let _ = std::fs::remove_file(path.with_extension("meta.json"));
            let freed = [path, self.manifest_path(&victim.key)].map(|file| {
                let len = file.symlink_metadata().map_or(0, |m| m.len());
                std::fs::remove_file(&file).is_ok().then_some(len)
            });
            if freed.iter().any(Option::is_some) {
                outcome.objects += 1;
                let bytes: u64 = freed.iter().flatten().sum();
                outcome.bytes = outcome.bytes.saturating_add(bytes);
            }
        }

        Ok(outcome)
    }
}

/// `<62 lowercase hex>.<pid>.<seq>.tmp`: the name `stage` gives an upload.
fn is_staged_temp(name: &str) -> bool {
    let mut parts = name.split('.');
    matches!(
        (parts.next(), parts.next(), parts.next(), parts.next(), parts.next()),
        (Some(k), Some(pid), Some(seq), Some("tmp"), None)
            if is_lowercase_hex(k, 62)
                && [pid, seq].iter().all(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
    )
}

impl LocalBackend {
    /// Remove staged-upload temps (`stage`'s names, nothing else) not written
    /// to for `older_than`: a crash or SIGKILL mid-upload orphans one, and
    /// neither `enumerate` nor eviction ever sees it. Returns how many went.
    pub fn sweep_stale_temps(&self, older_than: std::time::Duration) -> usize {
        let Ok(shards) = std::fs::read_dir(&self.root) else {
            return 0;
        };
        let mut removed = 0;
        for shard in shards.flatten() {
            if !shard.file_name().to_str().is_some_and(|n| is_lowercase_hex(n, 2)) {
                continue;
            }
            let Ok(files) = std::fs::read_dir(shard.path()) else {
                continue;
            };
            for file in files.flatten() {
                let stale = file.file_name().to_str().is_some_and(is_staged_temp)
                    && file
                        .metadata()
                        .and_then(|m| m.modified())
                        .is_ok_and(|t| t.elapsed().is_ok_and(|age| age > older_than));
                if stale && std::fs::remove_file(file.path()).is_ok() {
                    removed += 1;
                }
            }
        }
        removed
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
