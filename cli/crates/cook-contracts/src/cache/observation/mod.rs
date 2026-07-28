//! What running a unit was observed to do, beyond the bytes it wrote
//! (§{exec.cache.observation}).
//!
//! Recorded for every unit, not only output-less ones: a producing unit's
//! duration is as real as an observing unit's, and holding it here gives
//! `cook why`'s observed history a first-class home instead of best-effort
//! recovery from retained build logs.
//!
//! The observation is stored in two halves, and the split is normative rather
//! than an implementation convenience. [`Observation`] is scalars and belongs
//! wherever an implementation keeps the record it reads to judge freshness;
//! [`OutputLog`] is the streams and MUST NOT go there, because that index is
//! read wholesale on every invocation of every build and a chatty unit would
//! be re-read each time to answer a question no freshness check asks.
//!
//! This module owns the two POD types and the log's canonical encoding. It
//! owns no clock, no store and no policy: `recorded_at` is passed in, the
//! truncation cap is the caller's, and where the bytes are put is
//! `cook-cache`'s business.

use crate::output::{OutputChunk, OutputStream};
use serde::{Deserialize, Serialize};

/// The scalar half of a unit's observation.
///
/// Deliberately absent, in each case because the field would be a constant
/// dressed up as data:
///
/// * **No outcome.** A record exists only for a run that succeeded, so an
///   outcome recorded on it has one legal value. §{exec.cache.effect-kind}
///   already says the existence of the record is the verdict. The prototype
///   this recovers (78936318) carried a `UnitOutcome`; CS-0189 drops it.
/// * **No position in a local build history.** `recorded_at` is a timestamp,
///   not "three builds ago", because an observation may be served to a machine
///   that has no history of its own — which is the whole of §17.4 rule 4.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Observation {
    duration_ms: u64,
    recorded_at: u64,
    cause: Option<String>,
    log_bytes: u64,
}

impl Observation {
    /// `recorded_at` is passed in rather than read: this crate owns no clock.
    /// `log_bytes` is the encoded size of the corresponding [`OutputLog`], or
    /// `0` when the unit printed nothing.
    pub fn new(duration_ms: u64, recorded_at: u64, cause: Option<String>, log_bytes: u64) -> Self {
        Self {
            duration_ms,
            recorded_at,
            cause,
            log_bytes,
        }
    }

    pub fn duration_ms(&self) -> u64 {
        self.duration_ms
    }

    /// Unix seconds.
    pub fn recorded_at(&self) -> u64 {
        self.recorded_at
    }

    /// Why the unit ran on the run that was recorded (`input changed: <path>`,
    /// `env changed`, …), when the implementation had an attribution to give.
    ///
    /// History, not a verdict on the run being explained: it says why the unit
    /// ran *then*. Whether it will run *now* is computed at query time.
    pub fn cause(&self) -> Option<&str> {
        self.cause.as_deref()
    }

    /// Encoded size of the unit's output log.
    ///
    /// Carried here so a reader can report "1.4 MB recorded", or attribute a
    /// unit's log weight in a store-size accounting, without fetching the log
    /// itself. `0` means the unit printed nothing, which is distinguishable
    /// from "nothing was recorded" only by whether the observation exists.
    pub fn log_bytes(&self) -> u64 {
        self.log_bytes
    }
}

/// The stream half: what a unit printed, in the order the calls that printed
/// it ran.
///
/// The ordering guarantee is §{lua.cook-sh}'s and no stronger — preserved
/// across a unit's calls, not within one spawn, whose two streams are
/// separately buffered. That is why this is a sequence of [`OutputChunk`]
/// rather than of lines: a line-level sequence would imply an interleaving
/// between the streams that nobody observed.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct OutputLog {
    chunks: Vec<OutputChunk>,
    truncated_bytes: u64,
}

/// Magic prefix. Present so a corrupt or foreign blob fails at the door rather
/// than decoding into a plausible-looking log.
const MAGIC: &[u8; 8] = b"COOKLOG\0";
const VERSION: u32 = 1;

impl OutputLog {
    pub fn new(chunks: Vec<OutputChunk>, truncated_bytes: u64) -> Self {
        Self {
            chunks,
            truncated_bytes,
        }
    }

    pub fn chunks(&self) -> &[OutputChunk] {
        &self.chunks
    }

    /// How many bytes were dropped to keep the log within its cap. Non-zero
    /// means this log is a fragment, and §{exec.cache.observation} forbids
    /// presenting it as complete.
    pub fn truncated_bytes(&self) -> u64 {
        self.truncated_bytes
    }

    pub fn is_truncated(&self) -> bool {
        self.truncated_bytes > 0
    }

    /// Retain at most `cap` bytes of output, dropping from the MIDDLE.
    ///
    /// Head and tail are what a reader needs: a compiler's first errors and its
    /// summary line, a test's opening context and its assertion. The middle of
    /// a runaway log is the part nobody scrolls to. The elision is reported in
    /// bytes.
    ///
    /// A chunk that straddles the boundary is SPLIT rather than dropped whole.
    /// Chunk-granular truncation would be all-or-nothing in the case that
    /// matters most: a chunk is one spawn's bytes on one stream
    /// (§{lua.cook-sh}), so a single chatty `cc` invocation is a single chunk,
    /// and dropping it whole for overrunning the cap keeps neither the first
    /// errors nor the summary line this exists to preserve. A split retains a
    /// prefix or a suffix of what one spawn wrote to one stream, which is a
    /// true statement about that spawn; the whole log is a fragment already,
    /// and `truncated_bytes` is what says so.
    ///
    /// A `cap` of 0 keeps nothing and reports the whole log elided.
    pub fn truncate_to(self, cap: u64) -> Self {
        let total: u64 = self.chunks.iter().map(|c| c.bytes().len() as u64).sum();
        if total <= cap {
            return self;
        }

        // Head: whole chunks while they fit, then a prefix of the one that
        // straddles the boundary.
        let head_cap = cap / 2;
        let mut head: Vec<OutputChunk> = Vec::new();
        let mut head_used = 0u64;
        for c in &self.chunks {
            let room = head_cap - head_used;
            if room == 0 {
                break;
            }
            let n = c.bytes().len() as u64;
            if n <= room {
                head.push(c.clone());
                head_used += n;
            } else {
                // `room` is non-zero, so this never asks the constructor for an
                // empty chunk.
                head.extend(OutputChunk::new(c.stream(), c.bytes()[..room as usize].to_vec()));
                head_used += room;
                break;
            }
        }

        // Tail: the same from the other end, bounded by whatever the head left
        // unspent so a small head widens the tail rather than wasting the cap.
        //
        // The two cannot overlap: the head is a prefix of the concatenated
        // stream and the tail a suffix, and `head_used + tail_used <= cap <
        // total`.
        let tail_cap = cap - head_used;
        let mut tail: Vec<OutputChunk> = Vec::new();
        let mut tail_used = 0u64;
        for c in self.chunks.iter().rev() {
            let room = tail_cap - tail_used;
            if room == 0 {
                break;
            }
            let n = c.bytes().len() as u64;
            if n <= room {
                tail.push(c.clone());
                tail_used += n;
            } else {
                let start = (n - room) as usize;
                tail.extend(OutputChunk::new(c.stream(), c.bytes()[start..].to_vec()));
                tail_used += room;
                break;
            }
        }
        tail.reverse();

        head.extend(tail);
        Self {
            chunks: head,
            truncated_bytes: total - (head_used + tail_used),
        }
    }

    /// Encode to the canonical byte form (CS-0189):
    ///
    /// ```text
    /// "COOKLOG\0"  u32 version  u64 truncated_bytes
    /// repeat:      u8 stream    u32 len    bytes
    /// ```
    ///
    /// Bytes, not JSON. The two existing reserved artifacts carry JSON bodies,
    /// but a log is not text an encoder may re-interpret: compiler and linker
    /// output carries ANSI escapes and occasionally invalid UTF-8, and JSON
    /// cannot hold it without a lossy conversion that would corrupt the capture
    /// permanently. All integers little-endian.
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(
            MAGIC.len()
                + 12
                + self
                    .chunks
                    .iter()
                    .map(|c| c.bytes().len() + 5)
                    .sum::<usize>(),
        );
        out.extend_from_slice(MAGIC);
        out.extend_from_slice(&VERSION.to_le_bytes());
        out.extend_from_slice(&self.truncated_bytes.to_le_bytes());
        for c in &self.chunks {
            #[allow(unreachable_patterns)]
            out.push(match c.stream() {
                OutputStream::Stdout => 0,
                OutputStream::Stderr => 1,
                // `OutputStream` is `#[non_exhaustive]`. A stream this encoding
                // has no tag for must not be silently written as stdout.
                _ => 0xff,
            });
            out.extend_from_slice(&(c.bytes().len() as u32).to_le_bytes());
            out.extend_from_slice(c.bytes());
        }
        out
    }

    /// Decode the canonical form. `Err` means "not a log", which a caller
    /// reading a content-addressed store MUST treat as an absent observation
    /// rather than an error: a build must not fail because a log did not parse.
    pub fn decode(bytes: &[u8]) -> Result<Self, String> {
        let mut p = 0usize;
        let need = |p: usize, n: usize, what: &str| -> Result<(), String> {
            if bytes.len() < p + n {
                Err(format!(
                    "output log truncated: want {n} bytes for {what} at {p}"
                ))
            } else {
                Ok(())
            }
        };

        need(p, MAGIC.len(), "magic")?;
        if &bytes[p..p + MAGIC.len()] != MAGIC {
            return Err("output log: bad magic".to_string());
        }
        p += MAGIC.len();

        need(p, 4, "version")?;
        let version = u32::from_le_bytes(bytes[p..p + 4].try_into().unwrap());
        p += 4;
        if version != VERSION {
            return Err(format!("output log: unsupported version {version}"));
        }

        need(p, 8, "truncated_bytes")?;
        let truncated_bytes = u64::from_le_bytes(bytes[p..p + 8].try_into().unwrap());
        p += 8;

        let mut chunks = Vec::new();
        while p < bytes.len() {
            need(p, 1, "stream tag")?;
            let stream = match bytes[p] {
                0 => OutputStream::Stdout,
                1 => OutputStream::Stderr,
                other => return Err(format!("output log: unknown stream tag {other}")),
            };
            p += 1;

            need(p, 4, "chunk length")?;
            let len = u32::from_le_bytes(bytes[p..p + 4].try_into().unwrap()) as usize;
            p += 4;

            need(p, len, "chunk body")?;
            // `OutputChunk::new` refuses empty bytes, and the encoder never
            // writes an empty chunk because the constructor could not have made
            // one. A zero length is therefore a corrupt log, not an empty line.
            let chunk = OutputChunk::new(stream, bytes[p..p + len].to_vec())
                .ok_or_else(|| "output log: zero-length chunk".to_string())?;
            chunks.push(chunk);
            p += len;
        }

        Ok(Self {
            chunks,
            truncated_bytes,
        })
    }
}

#[cfg(test)]
#[path = "tests/observation_tests.rs"]
mod tests;
