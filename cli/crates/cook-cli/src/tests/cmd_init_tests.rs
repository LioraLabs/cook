use super::*;

#[test]
fn merge_creates_section_when_no_gitignore() {
    let merged = merge_cook_gitignore_section(None);
    match merged {
        GitignoreMerge::Created(content) => {
            assert!(content.contains(COOK_GITIGNORE_MARKER));
            // CS-0207: `.cook/**` is the WHOLE rule. The block used to carry
            // `cook_modules/lib/` beside it, which covered the native-extension
            // subtree and left `cook_modules/share/lua/5.4/` — where every
            // pure-Lua rock lands — untracked and un-ignored in the user's
            // repo, looking exactly like code they wrote.
            assert!(content.contains(".cook/**"));
            assert!(
                !content.contains("cook_modules"),
                "the managed block must not name the retired package directory"
            );
            assert!(content.ends_with('\n'));
            // Guard against drift: the comment must reference the
            // current subcommand name, not the renamed-and-removed
            // `cook modules add`.
            assert!(content.contains("cook modules install"));
            assert!(!content.contains("cook modules add"));
        }
        other => panic!("expected Created, got {other:?}"),
    }
}

#[test]
fn merge_is_idempotent_when_marker_present() {
    let existing = format!("target/\n\n{COOK_GITIGNORE_SECTION}");
    assert_eq!(
        merge_cook_gitignore_section(Some(&existing)),
        GitignoreMerge::Unchanged,
    );
}

#[test]
fn merge_appends_with_blank_line_separator() {
    let existing = "target/\nnode_modules/\n";
    match merge_cook_gitignore_section(Some(existing)) {
        GitignoreMerge::Appended(content) => {
            assert!(content.starts_with("target/\nnode_modules/\n\n"));
            assert!(content.contains(COOK_GITIGNORE_MARKER));
            assert!(content.contains(".cook/**"));
        }
        other => panic!("expected Appended, got {other:?}"),
    }
}

#[test]
fn merge_normalizes_missing_trailing_newline_before_appending() {
    let existing = "target/";
    match merge_cook_gitignore_section(Some(existing)) {
        GitignoreMerge::Appended(content) => {
            assert!(content.starts_with("target/\n\n"));
            assert!(content.contains(COOK_GITIGNORE_MARKER));
        }
        other => panic!("expected Appended, got {other:?}"),
    }
}

#[test]
fn merge_treats_empty_file_like_creation() {
    match merge_cook_gitignore_section(Some("")) {
        GitignoreMerge::Appended(content) => {
            assert!(content.starts_with(COOK_GITIGNORE_MARKER));
        }
        other => panic!("expected Appended, got {other:?}"),
    }
}
