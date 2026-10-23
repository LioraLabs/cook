use super::{covers_root, covers_too_broad, escapes_base, has_glob_meta, is_dir_output, normalize};
use std::path::{Path, PathBuf};

#[test]
fn glob_metacharacters() {
    assert!(has_glob_meta("src/*.c"));
    assert!(has_glob_meta("a?b"));
    assert!(has_glob_meta("[abc]"));
    assert!(!has_glob_meta("src/main.c"));
    assert!(!has_glob_meta(""));
}

/// The recursive forms are the ones a real Cookfile writes, and they are
/// metacharacter-bearing by virtue of `*` rather than by a rule of their own.
/// Carried down from cook-cache (COOK-425), where they were being asserted two
/// crates away from the function.
#[test]
fn recursive_wildcards_are_metacharacters() {
    assert!(has_glob_meta("src/**"));
    assert!(has_glob_meta("src/**/*"));
    assert!(has_glob_meta("apps/web/.next/**"));
    assert!(!has_glob_meta("apps/web/.next/BUILD_ID"));
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
    assert!(is_terminal_output("a/**")); // CS-0085 glob
    assert!(is_terminal_output("dist/")); // CS-0119 directory output
    assert!(!is_terminal_output("dist/app.js"));
}

/// Brace alternation is not glob syntax here, and the reason is a property of
/// the reference engine rather than a preference: `glob = "0.3"` has no brace
/// alternation, so CS-0085 excludes `{` from the metacharacter set and
/// `out/{a,b}.txt` is a LITERAL PATH. Brace expansion may be added by a future
/// CS once the reference engine supports it; until then, a classifier that said
/// otherwise would send a real filename down the expansion path and drop it.
#[test]
fn braces_are_not_glob_metacharacters() {
    assert!(!has_glob_meta("out/{a,b}.txt"));
    assert!(!has_glob_meta("src/{lib,app}/main.c"));
}

/// A glob anywhere in an entry can match anything under its literal prefix,
/// so the prefix is the root the breadth rule judges (§22.5.2.2).
#[test]
fn a_covers_root_is_the_literal_prefix_before_the_first_glob() {
    for (entry, root) in [("/h*", "/"), ("/*/lib", "/"), ("~/[.a-z]*", "~"), ("../../../**/x", "../../.."), ("**/node_modules", ""), ("/opt/x/**", "/opt/x"), ("~/.nvm/*/bin", "~/.nvm"), ("/usr/**", "/usr"), ("node_modules", "node_modules")] {
        assert_eq!(covers_root(entry), root, "{entry}");
    }
}

#[test]
fn registration_refuses_what_is_too_broad_on_every_host() {
    for e in ["/h*", "/u*", "/*/lib", "/[a-z]*/x", "~/[.a-z]*", "~/*/x", "/", "/usr", "/usr/**", "/**/node_modules", "~", "~/", "~/..", ".", "**", "a/..", "/usr/.", "?*", "[!.]*", "{*,.*}", "./?*", "**/?*", "src/../?*", "**/*.ts", "**/..", "{*,x/y}", "{**,x/y}", "**/{*,x/y}", "x/../{*,a/b}", "{*,a/b/c}", "{a/b/c,*}", "**/{*,a/b/c}", "{*,a/b/c}/**", "{*,{a/b,c/d}}", "{*,a/b/c,d}", "{*,[}]/b/c}", "{*,a[}]/b/c}", r"{*,a\}/b/c}", r"{*,\}/b/c}", "{*,x/y}/z", "{a,b}/node_modules", "*[a/b/c]", "**/[a-z]*/node_modules"] {
        assert!(covers_too_broad(e, None), "{e}");
    }
    // Host-dependent: judged at audit, not here.
    for e in ["**/node_modules", "/opt/x/**", "~/.nvm/*/bin", "/home/me", "/home/me/**", "../../../**/x", "../../x", "/usr/lib/*/x", "?*/x", "*/node_modules"] {
        assert!(!covers_too_broad(e, None), "{e}");
    }
}

#[test]
fn audit_adds_home_and_climbing_out_judged_as_resolved() {
    let home = Path::new("/home/me");
    let at = |dir: &'static str| Some((Path::new(dir), Some(home)));
    for e in ["/home/me", "/home/me/", "/home/me/**", "/home/me/*/x", "~/../me", "../../../**/x", "../../..", "../*", "/home/**", "/home/me/.."] {
        assert!(covers_too_broad(e, at("/a/b/c")), "{e}");
    }
    // A project at a top-level directory still covers its own node_modules.
    for e in ["**/node_modules", "node_modules", "./**/node_modules"] {
        assert!(!covers_too_broad(e, at("/app")), "{e}");
    }
    for e in ["**/node_modules", "/opt/x/**", "~/.nvm/*/bin", "~/.rustup/toolchains", "../../x"] {
        assert!(!covers_too_broad(e, at("/a/b/c")), "{e}");
    }
}

/// A home below a top-level directory (`/var/lib/jenkins` on CI) has an
/// ancestor that is not top-level; covering it covers home.
#[test]
fn audit_refuses_an_ancestor_of_a_deep_home() {
    let at = Some((Path::new("/a/b"), Some(Path::new("/var/lib/jenkins"))));
    assert!(covers_too_broad("/var/lib/**", at));
    assert!(covers_too_broad("/var/lib", at));
    assert!(!covers_too_broad("/var/lib/jenkins/.nvm", at));
    assert!(!covers_too_broad("/var/lib", None));
}

/// Climbing out and back names the Cookfile's own directory: as broad as `**`.
#[test]
fn audit_refuses_a_relative_root_climbing_back_to_its_own_directory() {
    let at = Some((Path::new("/home/me/dev/proj"), Some(Path::new("/home/me"))));
    // `../proj/**/node_modules` is written `**/node_modules`.
    for e in ["../proj", "../proj/**", "../proj/?*", "../../dev/**", "../proj/{a,*}", "../**/proj", "../*/proj/**", "../proj/**/node_modules", "../*/src"] {
        assert!(covers_too_broad(e, at), "{e}");
    }
    for e in ["../proj/src", "../other/**", "../../x"] {
        assert!(!covers_too_broad(e, at), "{e}");
    }
}
