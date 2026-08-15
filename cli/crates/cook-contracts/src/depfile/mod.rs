//! The Make depfile grammar: the prerequisites named by the FIRST rule of a
//! compiler's `-MMD` output.
//!
//! "First rule" is load-bearing and is a limitation, not a design. Only the
//! text up to the first `:` is treated as a target, so a `-MP` phony stanza
//! (`include/a.h:` on its own line) and a second rule in the same file
//! contribute their target text as a token with the colon still attached. Cook
//! has never emitted such a depfile from `discovered_inputs`, and the reader in
//! `cook-cache` drops those tokens anyway because no file is named `foo.h:`.
//! Pinned by test rather than fixed, because fixing it would change which paths
//! a real project records and that is a cache-invalidating decision with no
//! bug behind it.
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
///
/// Deliberately plain data with no `Display`: the one sentence a user ever sees
/// for this is `cook_cache::DepfileError::Malformed`'s, and a `Display` here
/// would be a second spelling of that sentence in a second crate, invisible to
/// the duplicate-literal gate because the interpolation differs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DepfileSyntax {
    pub byte_offset: usize,
    pub reason: String,
}

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
/// Whether a named path EXISTS is not a question this function can ask; the
/// caller applies that filter (see `cook_cache::parse_make_depfile`). Over any
/// FIXED tree the filter commutes with the dedupe here — it is order-preserving,
/// and a token it rejects is rejected at every occurrence — so it does not
/// matter that the reader now dedupes first and filters second. Over a tree
/// being written concurrently it is not a function at all, and neither order is
/// more correct than the other; a build racing its own generated headers has no
/// defined input set to be right about.
pub fn parse_prerequisites(content: &str, source_path: &str) -> Result<Vec<String>, DepfileSyntax> {
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
    let joined = after_colon.replace("\\\r\n", " ").replace("\\\n", " ");

    // Tokenise on any whitespace and apply filter rules. Preserve first-occurrence order.
    let mut seen = std::collections::HashSet::new();
    let mut out: Vec<String> = Vec::new();

    // `split_whitespace` never yields an empty token, so there is no
    // empty-token guard here. The version this moved from had one; it was
    // unreachable there too.
    for token in joined.split_whitespace() {
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
