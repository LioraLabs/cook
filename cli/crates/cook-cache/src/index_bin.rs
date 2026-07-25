//! Binary on-disk encoding for the recipe index (CS-0166, COOK-313).
//!
//! Replaces the v6 TOML index. The motivation is measured: on a 1,711-node
//! DuckDB build the TOML index reached 69 MB, of which 48.8% was `toml`'s
//! array-of-tables restating the step key once per input record, and whose
//! 328k input records resolved to 6,730 distinct paths (a ~49x path
//! redundancy inherent to C++ headers fanning out across translation units).
//! Parsing it cost 0.75s on a settled no-op that executed nothing.
//!
//! ## Layout
//!
//! Little-endian throughout. A 32-byte header, then a payload:
//!
//! ```text
//! header
//!   0..8    magic         b"COOKIDX\0"
//!   8..12   schema_ver    u32   == CACHE_VERSION
//!   12..16  reserved      u32   == 0
//!   16..24  payload_len   u64   == bytes.len() - HEADER_LEN
//!   24..32  payload_hash  u64   == xxh3_64(payload)
//!
//! payload
//!   paths     count u32, blob_len u32, [u32; count+1] offsets, blob
//!   records   count u32, [{ path_id u32, mtime u64, hash u64 }; count]
//!   steps     count u32, keyblob_len u32, [u32; count+1] key offsets, key blob,
//!             [{ inputs_start u32, inputs_len u32,
//!                outputs_start u32, outputs_len u32,
//!                command_hash u64, env_contribution u64,
//!                seal_contribution u64 }; count]
//!   globs     count u32, keyblob_len u32, [u32; count+1] key offsets, key blob,
//!             member_count u32, [u32; count+1] member offsets,
//!             [u32; member_count] member path ids
//! ```
//!
//! Records live in one flat pool; a step names a contiguous slice of it. Paths
//! are interned into one table per index, so a header shared by 800 translation
//! units is stored once.
//!
//! ## Why hand-rolled
//!
//! No dependency: `std::fs::read` plus `u64::from_le_bytes`. The format is
//! small enough that a codec is cheaper than the semantics of a serialisation
//! framework, and keeping the layout explicit is what lets the decoder be
//! total (see below). Deliberately not `mmap`ed: reading ~8 MB from page cache
//! is a few milliseconds and it sidesteps the torn-file-under-mmap hazard on a
//! cache-correctness path.
//!
//! ## Totality
//!
//! `decode` is total. Every length and offset read out of the payload is
//! bounds-checked before use, so a corrupt or hostile file produces
//! `Err(DecodeError)` rather than a panic or an out-of-bounds read. The
//! checksum catches accidental corruption first; the bounds checks exist for
//! the case where it does not (a resealed file, a deliberate edit). Callers
//! treat any `Err` as a cache miss, because the index is regeneratable.
//!
//! ## Determinism
//!
//! The same `RecipeCache` always encodes to the same bytes. Path ids are
//! assigned in sorted order rather than first-encounter order, so insertion
//! history cannot leak into the file. `ThreadSafeCacheManager::update_step`
//! short-circuits when a re-written entry equals the stored one, and the dirty
//! set drives whether a recipe is re-serialised at all; both assume an
//! unchanged index produces an unchanged file.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use cook_fingerprint::record::{FileRecord, StepEntry, CACHE_VERSION};

use crate::store::RecipeCache;

pub(crate) const MAGIC: &[u8; 8] = b"COOKIDX\0";
pub(crate) const HEADER_LEN: usize = 32;

/// Why a byte string is not a readable recipe index. Every variant is a
/// cache-miss at the call site, never a build error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DecodeError {
    /// Not a cook index file at all.
    BadMagic,
    /// Ran out of bytes: a short header, a payload shorter than its declared
    /// length, or a section whose declared extent runs past the payload.
    Truncated,
    /// The payload does not match the checksum in the header.
    ChecksumMismatch,
    /// Written by a cook with a different index layout. Carries the version
    /// found on the wire.
    SchemaVersion(u32),
    /// A path or step key in the blob is not valid UTF-8.
    Utf8,
    /// An index into the path table or record pool points outside it.
    BadReference,
}

impl std::fmt::Display for DecodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DecodeError::BadMagic => write!(f, "not a cook index file (bad magic)"),
            DecodeError::Truncated => write!(f, "index truncated"),
            DecodeError::ChecksumMismatch => write!(f, "index payload checksum mismatch"),
            DecodeError::SchemaVersion(v) => {
                write!(f, "index schema_version {v} != {CACHE_VERSION}")
            }
            DecodeError::Utf8 => write!(f, "index contains a non-UTF-8 path"),
            DecodeError::BadReference => write!(f, "index contains an out-of-range reference"),
        }
    }
}

impl std::error::Error for DecodeError {}

// ---------------------------------------------------------------- encoding

/// Append-only cursor. Every write is a push, so there is no offset arithmetic
/// to get wrong on this side.
struct Writer {
    buf: Vec<u8>,
}

impl Writer {
    fn new() -> Self {
        Self { buf: Vec::new() }
    }
    fn u32(&mut self, v: u32) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }
    fn u64(&mut self, v: u64) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }
    fn bytes(&mut self, v: &[u8]) {
        self.buf.extend_from_slice(v);
    }

    /// Write a string table: count, blob length, `count + 1` ascending
    /// offsets, then the blob. The trailing offset is the blob length, so a
    /// reader recovers item `i` as `blob[off[i]..off[i + 1]]` without a
    /// special case for the last entry.
    fn string_table<'a>(&mut self, items: impl ExactSizeIterator<Item = &'a str>) {
        let items: Vec<&str> = items.collect();
        let mut blob = Vec::new();
        let mut offsets = Vec::with_capacity(items.len() + 1);
        for s in &items {
            offsets.push(blob.len() as u32);
            blob.extend_from_slice(s.as_bytes());
        }
        offsets.push(blob.len() as u32);
        self.u32(items.len() as u32);
        self.u32(blob.len() as u32);
        for off in offsets {
            self.u32(off);
        }
        self.bytes(&blob);
    }
}

/// Serialise a recipe index. Deterministic: equal caches produce equal bytes.
pub fn encode(cache: &RecipeCache) -> Vec<u8> {
    // Intern every path the index mentions. Sorted assignment (BTreeSet) keeps
    // ids independent of insertion history.
    let mut paths: BTreeSet<&str> = BTreeSet::new();
    for step in cache.steps.values() {
        for r in step.inputs.iter().chain(step.outputs.iter()) {
            paths.insert(&*r.path);
        }
    }
    for members in cache.globs.values() {
        for m in members {
            paths.insert(m.as_str());
        }
    }
    let path_ids: BTreeMap<&str, u32> = paths
        .iter()
        .enumerate()
        .map(|(i, p)| (*p, i as u32))
        .collect();

    let mut w = Writer::new();
    w.string_table(paths.iter().copied());

    // Record pool: every step's inputs then its outputs, steps in key order.
    let record_count: usize = cache
        .steps
        .values()
        .map(|s| s.inputs.len() + s.outputs.len())
        .sum();
    w.u32(record_count as u32);
    for step in cache.steps.values() {
        for r in step.inputs.iter().chain(step.outputs.iter()) {
            w.u32(path_ids[&*r.path]);
            w.u64(r.mtime);
            w.u64(r.hash);
        }
    }

    // Steps, in the same order the pool was written, so the slice bounds below
    // line up by construction.
    w.string_table(cache.steps.keys().map(String::as_str));
    let mut cursor: u32 = 0;
    for step in cache.steps.values() {
        let inputs_start = cursor;
        let inputs_len = step.inputs.len() as u32;
        let outputs_start = cursor + inputs_len;
        let outputs_len = step.outputs.len() as u32;
        cursor = outputs_start + outputs_len;
        w.u32(inputs_start);
        w.u32(inputs_len);
        w.u32(outputs_start);
        w.u32(outputs_len);
        w.u64(step.command_hash);
        w.u64(step.env_contribution);
        w.u64(step.seal_contribution);
    }

    // Globs: key table, then a flat member pool with per-key offsets.
    w.string_table(cache.globs.keys().map(String::as_str));
    let member_total: usize = cache.globs.values().map(BTreeSet::len).sum();
    w.u32(member_total as u32);
    let mut acc: u32 = 0;
    for members in cache.globs.values() {
        w.u32(acc);
        acc += members.len() as u32;
    }
    w.u32(acc);
    for members in cache.globs.values() {
        for m in members {
            w.u32(path_ids[m.as_str()]);
        }
    }

    let payload = w.buf;
    let mut out = Vec::with_capacity(HEADER_LEN + payload.len());
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&CACHE_VERSION.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&(payload.len() as u64).to_le_bytes());
    out.extend_from_slice(&xxhash_rust::xxh3::xxh3_64(&payload).to_le_bytes());
    out.extend_from_slice(&payload);
    out
}

// ---------------------------------------------------------------- decoding

/// Bounds-checked sequential reader. Every accessor returns `Result`, which is
/// what makes `decode` total against arbitrary bytes.
struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn new(buf: &'a [u8]) -> Self {
        Self { buf, pos: 0 }
    }

    fn take(&mut self, n: usize) -> Result<&'a [u8], DecodeError> {
        let end = self.pos.checked_add(n).ok_or(DecodeError::Truncated)?;
        let slice = self.buf.get(self.pos..end).ok_or(DecodeError::Truncated)?;
        self.pos = end;
        Ok(slice)
    }

    fn u32(&mut self) -> Result<u32, DecodeError> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }

    fn u64(&mut self) -> Result<u64, DecodeError> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into().unwrap()))
    }

    /// Inverse of [`Writer::string_table`]. Validates that offsets ascend and
    /// stay inside the blob before slicing, so a corrupt offset is an error
    /// rather than a panic.
    ///
    /// Yields `Arc<str>` because the path table's entries are shared by every
    /// record that names them — that sharing is the point of interning, and it
    /// is what makes loading an index allocate once per distinct path instead
    /// of once per record. Key tables are small and convert to `String` at the
    /// call site.
    fn string_table(&mut self) -> Result<Vec<Arc<str>>, DecodeError> {
        let count = self.u32()? as usize;
        let blob_len = self.u32()? as usize;
        // Reject absurd counts before allocating: each entry costs one offset
        // in the payload, so a count that cannot be backed by the remaining
        // bytes is corrupt regardless of what the offsets say.
        let remaining = self.buf.len().saturating_sub(self.pos);
        if count
            .checked_add(1)
            .and_then(|n| n.checked_mul(4))
            .map_or(true, |need| need > remaining)
        {
            return Err(DecodeError::Truncated);
        }
        let mut offsets = Vec::with_capacity(count + 1);
        for _ in 0..=count {
            offsets.push(self.u32()? as usize);
        }
        let blob = self.take(blob_len)?;
        if offsets[count] != blob_len {
            return Err(DecodeError::BadReference);
        }
        let mut out = Vec::with_capacity(count);
        for i in 0..count {
            let (start, end) = (offsets[i], offsets[i + 1]);
            if start > end || end > blob_len {
                return Err(DecodeError::BadReference);
            }
            out.push(Arc::from(
                std::str::from_utf8(&blob[start..end]).map_err(|_| DecodeError::Utf8)?,
            ));
        }
        Ok(out)
    }
}

/// Parse a recipe index. Total: any malformed input yields `Err`.
pub fn decode(bytes: &[u8]) -> Result<RecipeCache, DecodeError> {
    if bytes.len() < HEADER_LEN {
        return Err(DecodeError::Truncated);
    }
    if &bytes[0..8] != MAGIC {
        return Err(DecodeError::BadMagic);
    }
    let schema_version = u32::from_le_bytes(bytes[8..12].try_into().unwrap());
    if schema_version != CACHE_VERSION {
        return Err(DecodeError::SchemaVersion(schema_version));
    }
    let payload_len = u64::from_le_bytes(bytes[16..24].try_into().unwrap());
    let payload_hash = u64::from_le_bytes(bytes[24..32].try_into().unwrap());
    let payload = bytes
        .get(HEADER_LEN..)
        .ok_or(DecodeError::Truncated)?;
    if payload.len() as u64 != payload_len {
        return Err(DecodeError::Truncated);
    }
    if xxhash_rust::xxh3::xxh3_64(payload) != payload_hash {
        return Err(DecodeError::ChecksumMismatch);
    }

    let mut r = Reader::new(payload);
    let paths = r.string_table()?;

    let record_count = r.u32()? as usize;
    // 20 bytes per record; refuse a count the payload cannot back before
    // reserving for it.
    let remaining = payload.len().saturating_sub(r.pos);
    if record_count.checked_mul(20).map_or(true, |n| n > remaining) {
        return Err(DecodeError::Truncated);
    }
    let mut records = Vec::with_capacity(record_count);
    for _ in 0..record_count {
        let path_id = r.u32()? as usize;
        let mtime = r.u64()?;
        let hash = r.u64()?;
        // Arc clone: a refcount bump, not an allocation. This is the line the
        // whole interning design exists for.
        let path = paths.get(path_id).ok_or(DecodeError::BadReference)?;
        records.push(FileRecord { path: Arc::clone(path), mtime, hash });
    }

    let step_keys = r.string_table()?;
    let mut steps = BTreeMap::new();
    for key in step_keys {
        let key = key.to_string();
        let inputs_start = r.u32()? as usize;
        let inputs_len = r.u32()? as usize;
        let outputs_start = r.u32()? as usize;
        let outputs_len = r.u32()? as usize;
        let command_hash = r.u64()?;
        let env_contribution = r.u64()?;
        let seal_contribution = r.u64()?;
        steps.insert(
            key,
            StepEntry {
                inputs: slice_records(&records, inputs_start, inputs_len)?,
                outputs: slice_records(&records, outputs_start, outputs_len)?,
                command_hash,
                env_contribution,
                seal_contribution,
            },
        );
    }

    let glob_keys = r.string_table()?;
    let member_total = r.u32()? as usize;
    let mut member_offsets = Vec::with_capacity(glob_keys.len() + 1);
    for _ in 0..=glob_keys.len() {
        member_offsets.push(r.u32()? as usize);
    }
    let remaining = payload.len().saturating_sub(r.pos);
    if member_total.checked_mul(4).map_or(true, |n| n > remaining) {
        return Err(DecodeError::Truncated);
    }
    let mut members = Vec::with_capacity(member_total);
    for _ in 0..member_total {
        let id = r.u32()? as usize;
        members.push(paths.get(id).ok_or(DecodeError::BadReference)?.to_string());
    }
    let mut globs = BTreeMap::new();
    for (i, key) in glob_keys.into_iter().map(|k| k.to_string()).enumerate() {
        let (start, end) = (member_offsets[i], member_offsets[i + 1]);
        if start > end || end > members.len() {
            return Err(DecodeError::BadReference);
        }
        globs.insert(key, members[start..end].iter().cloned().collect());
    }

    Ok(RecipeCache { schema_version, globs, steps })
}

fn slice_records(
    pool: &[FileRecord],
    start: usize,
    len: usize,
) -> Result<Vec<FileRecord>, DecodeError> {
    let end = start.checked_add(len).ok_or(DecodeError::BadReference)?;
    pool.get(start..end)
        .map(<[FileRecord]>::to_vec)
        .ok_or(DecodeError::BadReference)
}

#[cfg(test)]
#[path = "tests/index_bin_tests.rs"]
mod tests;
