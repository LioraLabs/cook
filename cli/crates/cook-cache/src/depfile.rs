//! Make-format depfile parser. See the design at
//! `standard/specs/2026-05-04-discovered-inputs-design.md` §4.3.

use std::io;
use std::path::Path;

/// Result of attempting to read a Make-format depfile.
#[derive(Debug)]
pub enum DepfileError {
    NotFound,
    Io(io::Error),
    Malformed { byte_offset: usize, reason: String },
}

impl std::fmt::Display for DepfileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DepfileError::NotFound => write!(f, "depfile not found"),
            DepfileError::Io(e) => write!(f, "depfile io error: {e}"),
            DepfileError::Malformed { byte_offset, reason } => {
                write!(f, "depfile malformed at byte {byte_offset}: {reason}")
            }
        }
    }
}

impl std::error::Error for DepfileError {}

/// Parse a Make-format depfile.
///
/// Filter rules (see design §4.3):
///   - Strip the leading target text up to and including the first `:`.
///   - Join continuation lines (`\\\n` and `\\\r\n`).
///   - Skip entries beginning with `/` (absolute paths).
///   - Skip entries equal to `source_path`.
///   - Skip entries whose path does not exist on disk relative to `working_dir`.
///
/// `source_path` may be the empty string (no self-skip).
pub fn parse_make_depfile(
    depfile_path: &Path,
    source_path: &str,
    working_dir: &Path,
) -> Result<Vec<String>, DepfileError> {
    todo!("Task 4")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn returns_not_found_for_missing_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let result = parse_make_depfile(
            &dir.path().join("nonexistent.d"),
            "src/a.c",
            dir.path(),
        );
        assert!(matches!(result, Err(DepfileError::NotFound)));
    }
}
