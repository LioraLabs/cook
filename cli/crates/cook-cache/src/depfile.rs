//! Make-format depfile parser, for the `.d`-style dependency files emitted
//! by `discovered_inputs` (target: prerequisite prerequisite ...).

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

impl std::error::Error for DepfileError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            DepfileError::Io(e) => Some(e),
            _ => None,
        }
    }
}

/// Parse a Make-format depfile. Returns paths in input order, deduped.
///
/// Filter rules:
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
    let content = match std::fs::read_to_string(depfile_path) {
        Ok(s) => s,
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            return Err(DepfileError::NotFound);
        }
        Err(e) => return Err(DepfileError::Io(e)),
    };

    // Locate the first ':' separating the target from the prerequisites.
    let colon_pos = match content.find(':') {
        Some(p) => p,
        None => {
            return Err(DepfileError::Malformed {
                byte_offset: 0,
                reason: "no ':' separating target from prerequisites".to_string(),
            });
        }
    };

    // Strip target text and any leading whitespace after the colon.
    let after_colon = &content[colon_pos + 1..];

    // Join continuation lines: '\\\r\n' and '\\\n' both become a single space.
    // CRLF is processed first so the trailing '\r' doesn't leak into a token
    // when the file uses Windows line endings.
    let joined = after_colon
        .replace("\\\r\n", " ")
        .replace("\\\n", " ");

    // Tokenise on any whitespace and apply filter rules. Preserve first-occurrence order.
    let mut seen = std::collections::HashSet::new();
    let mut out: Vec<String> = Vec::new();

    for token in joined.split_whitespace() {
        if token.is_empty() {
            continue;
        }
        // Filter: skip absolute paths.
        if token.starts_with('/') {
            continue;
        }
        // Filter: skip the source itself.
        if !source_path.is_empty() && token == source_path {
            continue;
        }
        // Filter: skip non-existent paths (relative to working_dir).
        //
        // COOK-306: this runs for every prerequisite of every depfile on every
        // run, and C++ prerequisite lists are overwhelmingly the same headers
        // over and over — on DuckDB, 1,687 depfiles named ~320k prerequisites
        // resolving to 6,730 distinct paths. Answered through the per-run stat
        // memo, which shares its entries with the input check below and is
        // disarmed by the first write cook performs.
        if cook_fingerprint::statmemo::stat_mtime_memo(working_dir, token).is_none() {
            continue;
        }
        // Dedupe.
        if seen.insert(token.to_string()) {
            out.push(token.to_string());
        }
    }

    Ok(out)
}

#[cfg(test)]
#[path = "tests/depfile_tests.rs"]
mod tests;
