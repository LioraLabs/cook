//! Captured output-stream identity.

/// Which file descriptor a captured output line came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum OutputStream {
    Stdout,
    Stderr,
}
