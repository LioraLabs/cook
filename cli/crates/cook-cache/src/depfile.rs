//! Reading a Make-format depfile: the `.d`-style dependency files emitted by
//! `discovered_inputs` (target: prerequisite prerequisite ...).
//!
//! What a depfile MEANS is `cook_contracts::depfile` — a function of text
//! alone, shared by the executor that keys on the answer and the `cook why`
//! renderer that draws it. What is left here is the part that needs the world:
//! reading the file, and dropping the prerequisites that do not exist on disk.
//! COOK-425 drew that line; before it, deciding a depfile's meaning required a
//! filesystem, and every test of the grammar built a directory tree to assert
//! something about a string.

use std::io;
use std::path::Path;

use cook_contracts::depfile::parse_prerequisites;

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

/// Read a Make-format depfile. Returns paths in input order, deduped.
///
/// The grammar — strip the target text up to the first `:`, join continuation
/// lines, drop absolute entries and the source itself, dedupe — is
/// `cook_contracts::depfile::parse_prerequisites`. What this adds is the two
/// things that need the world: reading the file, and dropping entries whose
/// path does not exist on disk relative to `working_dir`.
///
/// `source_path` may be the empty string (no self-skip).
///
/// The existence filter runs AFTER the dedupe rather than interleaved with it,
/// which is where it used to sit. The list is identical either way: existence
/// is a pure function of the token within a run, so it cannot promote a later
/// duplicate into a slot the first occurrence would not have taken.
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

    let named = parse_prerequisites(&content, source_path).map_err(|e| {
        DepfileError::Malformed { byte_offset: e.byte_offset, reason: e.reason }
    })?;

    // Filter: skip non-existent paths (relative to working_dir).
    //
    // COOK-306: this runs for every prerequisite of every depfile on every
    // run, and C++ prerequisite lists are overwhelmingly the same headers
    // over and over — on DuckDB, 1,687 depfiles named ~320k prerequisites
    // resolving to 6,730 distinct paths. Answered through the per-run stat
    // memo, which shares its entries with the input check below and is
    // disarmed by the first write cook performs.
    Ok(named
        .into_iter()
        .filter(|token| crate::statmemo::stat_mtime_memo(working_dir, token).is_some())
        .collect())
}

#[cfg(test)]
#[path = "tests/depfile_tests.rs"]
mod tests;
