//! Captured output-stream identity, and the unit of captured output.

/// Which file descriptor a captured output line came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum OutputStream {
    Stdout,
    Stderr,
}

/// One contiguous run of bytes a unit wrote to one of its streams.
///
/// A unit's captured output is a *sequence* of these, in the order the spawns
/// that produced them happened (CS-0188). A body calling `cook.sh` three times
/// contributes up to six chunks, and reading them in order reproduces the order
/// the calls ran in. Within one spawn stdout and stderr are separately
/// buffered, so one spawn contributes at most one chunk per stream and their
/// relative interleaving is not recoverable — which is why this is a chunk and
/// not a line: a line-level sequence would imply an interleaving the mechanism
/// cannot supply.
///
/// **Bytes, not `String`.** Compiler and linker output carries ANSI escapes and
/// occasionally invalid UTF-8, and a lossy conversion at capture time corrupts
/// the capture permanently. Convert at render, where the loss is visible and
/// local. This is also what lets a chunk be written to a content-addressed
/// store verbatim.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutputChunk {
    stream: OutputStream,
    bytes: Vec<u8>,
}

impl OutputChunk {
    /// Build a chunk. Returns `None` for empty `bytes`: a chunk records that a
    /// unit wrote something, so one recording that it wrote nothing is not a
    /// smaller chunk but an absent one. Admitting it would let a silent spawn
    /// contribute two chunks to every sequence and make "did this unit print
    /// anything" a question about lengths rather than emptiness.
    pub fn new(stream: OutputStream, bytes: impl Into<Vec<u8>>) -> Option<Self> {
        let bytes = bytes.into();
        if bytes.is_empty() {
            return None;
        }
        Some(Self { stream, bytes })
    }

    pub fn stream(&self) -> OutputStream {
        self.stream
    }

    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// The chunk's bytes as text, replacing anything that is not valid UTF-8.
    /// The only place the lossy conversion belongs is where the text is about
    /// to be shown.
    pub fn lossy(&self) -> std::borrow::Cow<'_, str> {
        String::from_utf8_lossy(&self.bytes)
    }
}

#[cfg(test)]
#[path = "tests/output_tests.rs"]
mod tests;
