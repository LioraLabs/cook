use super::*;

fn root() -> PathBuf {
    PathBuf::from("/proj")
}

fn confined() -> SandboxPolicy {
    SandboxPolicy::Confined { project_root: root() }
}

#[test]
fn off_passes_everything() {
    let p = SandboxPolicy::Off;
    assert!(p.resolve("fs.read", Path::new("/proj"), "../etc/passwd").is_ok());
    assert!(p.resolve("fs.read", Path::new("/proj"), "/etc/passwd").is_ok());
}

#[test]
fn confined_allows_relative_inside() {
    let p = confined();
    assert!(p.resolve("fs.read", Path::new("/proj"), "src/main.rs").is_ok());
    assert!(p.resolve("fs.read", Path::new("/proj"), "./build/x").is_ok());
}

#[test]
fn confined_allows_subdir_cwd() {
    let p = confined();
    // CS-0017: imported Cookfiles run with their own subdir as
    // working_dir, but the project_root is still /proj. A relative
    // path from the subdir cwd that stays inside /proj is fine.
    assert!(p.resolve("fs.read", Path::new("/proj/lib"), "data.txt").is_ok());
    assert!(p.resolve("fs.read", Path::new("/proj/lib"), "../data.txt").is_ok());
}

#[test]
fn confined_rejects_absolute_outside() {
    let p = confined();
    let err = p.resolve("fs.read", Path::new("/proj"), "/etc/passwd").unwrap_err();
    assert!(matches!(err, SandboxError::Escape { .. }), "got {err}");
}

#[test]
fn confined_rejects_relative_traversal() {
    let p = confined();
    let err = p.resolve("fs.read", Path::new("/proj/lib"), "../../etc/passwd").unwrap_err();
    assert!(matches!(err, SandboxError::Escape { .. }), "got {err}");
}

#[test]
fn confined_rejects_dotdot_to_above_root() {
    let p = confined();
    // /proj/.. = /, not inside /proj
    let err = p.resolve("fs.read", Path::new("/proj"), "../somefile").unwrap_err();
    assert!(matches!(err, SandboxError::Escape { .. }));
}

#[test]
fn confined_allows_absolute_inside() {
    // An absolute path that points into the project is fine.
    let p = confined();
    assert!(p.resolve("fs.read", Path::new("/proj"), "/proj/src/x.rs").is_ok());
    assert!(p.resolve("fs.read", Path::new("/proj"), "/proj").is_ok());
}

#[test]
fn shell_escape_disabled_under_confined() {
    assert!(!confined().shell_escape_hatches_enabled());
    assert!(SandboxPolicy::Off.shell_escape_hatches_enabled());
}

#[test]
fn lexical_normalize_basic() {
    assert_eq!(lexical_normalize(Path::new("/a/b/./c")), PathBuf::from("/a/b/c"));
    assert_eq!(lexical_normalize(Path::new("/a/b/../c")), PathBuf::from("/a/c"));
    assert_eq!(lexical_normalize(Path::new("a/b/../c")), PathBuf::from("a/c"));
    assert_eq!(lexical_normalize(Path::new("../x")), PathBuf::from("../x"));
}

#[test]
fn live_source_observes_post_install_changes() {
    let slot = Arc::new(Mutex::new(SandboxPolicy::Off));
    let src = SandboxSource::Live(Arc::clone(&slot));
    assert!(matches!(src.resolve(), SandboxPolicy::Off));

    *slot.lock().unwrap() = SandboxPolicy::Confined { project_root: root() };
    assert!(matches!(src.resolve(), SandboxPolicy::Confined { .. }));
}
