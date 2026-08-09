//! The Make depfile grammar: what a compiler's `-MMD` output names as the
//! prerequisites of the thing it just built.
//!
//! COOK-425 split this out of `cook_cache::depfile`, which read the file and
//! decided its meaning in one function. The reading stays there — it needs the
//! filesystem and the per-run stat memo — and the meaning came here, where the
//! constitution puts a wire format with two ends. There are two ends: the
//! executor folds this list into a unit's cache key, and `cook why` renders it
//! as the discovered edges of the graph. If those two ever disagreed about
//! what a depfile said, `cook why` would explain a rebuild that did not happen
//! for the reason it names.

/// Text that cannot be read as Make output at all.
///
/// The grammar can only see syntax, so this is the only failure it can report.
/// A missing or unreadable file is the reader's problem, not the grammar's.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DepfileSyntax {
    pub byte_offset: usize,
    pub reason: String,
}

impl std::fmt::Display for DepfileSyntax {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "depfile malformed at byte {}: {}", self.byte_offset, self.reason)
    }
}

impl std::error::Error for DepfileSyntax {}

/// Read the prerequisite list out of Make depfile text.
///
/// Returns the paths in first-occurrence order, deduped, with the target text
/// and these two classes dropped:
///
///   - entries beginning with `/` (absolute paths: system headers, outside the
///     project, produced by nothing in the build);
///   - entries equal to `source_path` (the translation unit naming itself; the
///     caller already declared it).
///
/// `source_path` may be the empty string, which disables the self-skip.
///
/// Whether a named path EXISTS is not a question this function can ask. The
/// caller applies that filter (see `cook_cache::parse_make_depfile`), and it
/// commutes with the dedupe here: existence is a pure function of the token
/// within a run, so filtering before or after deduping yields the same list in
/// the same order.
pub fn parse_prerequisites(
    content: &str,
    source_path: &str,
) -> Result<Vec<String>, DepfileSyntax> {
    // Locate the first ':' separating the target from the prerequisites.
    let colon_pos = match content.find(':') {
        Some(p) => p,
        None => {
            return Err(DepfileSyntax {
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
