use super::{escapes_base, has_glob_meta, is_dir_output, normalize};
use std::path::{Path, PathBuf};

#[test]
fn glob_metacharacters() {
    assert!(has_glob_meta("src/*.c"));
    assert!(has_glob_meta("a?b"));
    assert!(has_glob_meta("[abc]"));
    assert!(!has_glob_meta("src/main.c"));
    assert!(!has_glob_meta(""));
}

#[test]
fn directory_outputs_end_with_a_slash() {
    assert!(is_dir_output("dist/"));
    assert!(!is_dir_output("dist"));
    assert!(!is_dir_output("dist/x.js"));
}

#[test]
fn normalize_resolves_dot_and_dotdot() {
    assert_eq!(normalize(Path::new("a/./b")), PathBuf::from("a/b"));
    assert_eq!(normalize(Path::new("a/b/../c")), PathBuf::from("a/c"));
    assert_eq!(normalize(Path::new("./a")), PathBuf::from("a"));
    assert_eq!(normalize(Path::new("a/b/..")), PathBuf::from("a"));
}

/// COOK-414: this was the behaviour both `lexically_normalize` and
/// `normalize_lexical` had. Pinned so a future reader cannot "fix" one of them
/// into disagreeing with the other, which is how they got here.
#[test]
fn normalize_drops_a_leading_dotdot_rather_than_preserving_it() {
    assert_eq!(normalize(Path::new("../a")), PathBuf::from("a"));
    assert_eq!(normalize(Path::new("../../a")), PathBuf::from("a"));
}

/// Which is exactly why `escapes_base` counts depth instead of normalising:
/// the case that matters is the one `normalize` erases.
#[test]
fn escapes_base_catches_what_normalize_erases() {
    assert!(escapes_base(Path::new("../a")));
    assert!(escapes_base(Path::new("a/../../b")));
    assert!(!escapes_base(Path::new("a/../b")));
    assert!(!escapes_base(Path::new("a/b/../c")));
    assert!(!escapes_base(Path::new("a/b")));
}

/// An absolute path leaves the base as surely as `..` does, and the original
/// implementation said so via its `RootDir`/`Prefix` arm. Pinned because a
/// depth-counting rewrite loses it silently, and this gates whether a declared
/// input may sit outside the project.
#[test]
fn an_absolute_path_escapes_the_base() {
    assert!(escapes_base(Path::new("/etc/passwd")));
    assert!(escapes_base(Path::new("/")));
    assert!(!escapes_base(Path::new("etc/passwd")));
}

#[test]
fn terminal_outputs_are_globs_or_directories() {
    use super::is_terminal_output;
    assert!(is_terminal_output("dist/*.js"));
    assert!(is_terminal_output("dist/"));
    assert!(!is_terminal_output("dist/app.js"));
}

/// Brace alternation is not glob syntax here.
#[test]
fn braces_are_not_glob_metacharacters() {
    assert!(!has_glob_meta("out/{a,b}.txt"));
}
