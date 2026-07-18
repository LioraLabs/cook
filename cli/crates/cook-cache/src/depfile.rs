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
        let abs = working_dir.join(token);
        if !abs.exists() {
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
mod tests {
    use super::*;
    use std::fs;

    fn write_file(dir: &Path, rel: &str, content: &str) {
        let abs = dir.join(rel);
        if let Some(parent) = abs.parent() {
            fs::create_dir_all(parent).expect("mkdir");
        }
        fs::write(&abs, content).expect("write");
    }

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

    #[test]
    fn parses_single_line_depfile() {
        let dir = tempfile::tempdir().expect("tempdir");
        let wd = dir.path();
        write_file(wd, "src/a.c", "// source\n");
        write_file(wd, "include/a.h", "#pragma once\n");
        write_file(wd, ".cook/deps/a.d", "build/a.o: src/a.c include/a.h\n");

        let paths = parse_make_depfile(
            &wd.join(".cook/deps/a.d"),
            "src/a.c",
            wd,
        )
        .expect("ok");

        assert_eq!(paths, vec!["include/a.h".to_string()]);
    }

    #[test]
    fn joins_continuation_lines() {
        let dir = tempfile::tempdir().expect("tempdir");
        let wd = dir.path();
        write_file(wd, "src/a.c", "");
        write_file(wd, "include/a.h", "");
        write_file(wd, "include/b.h", "");
        write_file(wd, ".cook/deps/a.d",
            "build/a.o: src/a.c \\\n  include/a.h \\\n  include/b.h\n");

        let paths = parse_make_depfile(
            &wd.join(".cook/deps/a.d"),
            "src/a.c",
            wd,
        )
        .expect("ok");

        assert_eq!(paths, vec!["include/a.h".to_string(), "include/b.h".to_string()]);
    }

    #[test]
    fn joins_crlf_continuation_lines() {
        let dir = tempfile::tempdir().expect("tempdir");
        let wd = dir.path();
        write_file(wd, "src/a.c", "");
        write_file(wd, "include/a.h", "");
        write_file(wd, "include/b.h", "");
        write_file(wd, ".cook/deps/a.d",
            "build/a.o: src/a.c \\\r\n  include/a.h \\\r\n  include/b.h\r\n");

        let paths = parse_make_depfile(
            &wd.join(".cook/deps/a.d"),
            "src/a.c",
            wd,
        )
        .expect("ok");

        assert_eq!(paths, vec!["include/a.h".to_string(), "include/b.h".to_string()]);
    }

    #[test]
    fn skips_absolute_paths() {
        let dir = tempfile::tempdir().expect("tempdir");
        let wd = dir.path();
        write_file(wd, "src/a.c", "");
        write_file(wd, "include/a.h", "");
        write_file(wd, ".cook/deps/a.d",
            "build/a.o: src/a.c /usr/include/stdio.h include/a.h\n");

        let paths = parse_make_depfile(
            &wd.join(".cook/deps/a.d"),
            "src/a.c",
            wd,
        )
        .expect("ok");

        assert_eq!(paths, vec!["include/a.h".to_string()]);
    }

    #[test]
    fn skips_source_self_reference() {
        let dir = tempfile::tempdir().expect("tempdir");
        let wd = dir.path();
        write_file(wd, "src/a.c", "");
        write_file(wd, "include/a.h", "");
        write_file(wd, ".cook/deps/a.d",
            "build/a.o: src/a.c include/a.h src/a.c\n");

        let paths = parse_make_depfile(
            &wd.join(".cook/deps/a.d"),
            "src/a.c",
            wd,
        )
        .expect("ok");

        assert_eq!(paths, vec!["include/a.h".to_string()]);
    }

    #[test]
    fn skips_nonexistent_paths() {
        let dir = tempfile::tempdir().expect("tempdir");
        let wd = dir.path();
        write_file(wd, "src/a.c", "");
        write_file(wd, "include/exists.h", "");
        write_file(wd, ".cook/deps/a.d",
            "build/a.o: src/a.c include/exists.h include/missing.h\n");

        let paths = parse_make_depfile(
            &wd.join(".cook/deps/a.d"),
            "src/a.c",
            wd,
        )
        .expect("ok");

        assert_eq!(paths, vec!["include/exists.h".to_string()]);
    }

    #[test]
    fn empty_source_path_disables_self_skip() {
        let dir = tempfile::tempdir().expect("tempdir");
        let wd = dir.path();
        write_file(wd, "src/a.c", "");
        write_file(wd, ".cook/deps/a.d",
            "build/a.o: src/a.c\n");

        let paths = parse_make_depfile(
            &wd.join(".cook/deps/a.d"),
            "",
            wd,
        )
        .expect("ok");

        assert_eq!(paths, vec!["src/a.c".to_string()]);
    }

    #[test]
    fn malformed_no_colon_returns_error() {
        let dir = tempfile::tempdir().expect("tempdir");
        let wd = dir.path();
        write_file(wd, ".cook/deps/a.d", "no colon here at all\n");

        let result = parse_make_depfile(
            &wd.join(".cook/deps/a.d"),
            "src/a.c",
            wd,
        );

        assert!(matches!(result, Err(DepfileError::Malformed { .. })));
    }

    #[test]
    fn deduplicates_repeated_paths() {
        let dir = tempfile::tempdir().expect("tempdir");
        let wd = dir.path();
        write_file(wd, "src/a.c", "");
        write_file(wd, "include/a.h", "");
        write_file(wd, ".cook/deps/a.d",
            "build/a.o: src/a.c include/a.h include/a.h\n");

        let paths = parse_make_depfile(
            &wd.join(".cook/deps/a.d"),
            "src/a.c",
            wd,
        )
        .expect("ok");

        assert_eq!(paths, vec!["include/a.h".to_string()]);
    }
}
