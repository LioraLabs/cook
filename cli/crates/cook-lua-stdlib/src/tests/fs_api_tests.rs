use super::*;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tempfile::TempDir;

fn setup_static(dir: &std::path::Path) -> Lua {
    let lua = Lua::new();
    register_fs_api(&lua, WorkingDirSource::Static(dir.to_path_buf())).unwrap();
    lua
}

fn setup_live(slot: Arc<Mutex<PathBuf>>) -> Lua {
    let lua = Lua::new();
    register_fs_api(&lua, WorkingDirSource::Live(slot)).unwrap();
    lua
}

fn setup_confined(dir: &std::path::Path, project_root: &std::path::Path) -> Lua {
    let lua = Lua::new();
    register_fs_api_with_sandbox(
        &lua,
        WorkingDirSource::Static(dir.to_path_buf()),
        SandboxSource::confined(project_root.to_path_buf()),
    )
    .unwrap();
    lua
}

// ---- Static-source tests (cook-register call pattern) ------------

#[test]
fn static_write_creates_file() {
    let dir = TempDir::new().unwrap();
    let lua = setup_static(dir.path());
    lua.load(r#"fs.write("test.txt", "hello world")"#)
        .exec()
        .unwrap();
    let content = std::fs::read_to_string(dir.path().join("test.txt")).unwrap();
    assert_eq!(content, "hello world");
}

#[test]
fn static_write_overwrites_existing() {
    let dir = TempDir::new().unwrap();
    std::fs::write(dir.path().join("test.txt"), "old").unwrap();
    let lua = setup_static(dir.path());
    lua.load(r#"fs.write("test.txt", "new")"#).exec().unwrap();
    let content = std::fs::read_to_string(dir.path().join("test.txt")).unwrap();
    assert_eq!(content, "new");
}

#[test]
fn static_mkdir_p_creates_nested_dirs() {
    let dir = TempDir::new().unwrap();
    let lua = setup_static(dir.path());
    lua.load(r#"fs.mkdir_p("a/b/c")"#).exec().unwrap();
    assert!(dir.path().join("a/b/c").is_dir());
}

#[test]
fn static_mkdir_p_existing_is_ok() {
    let dir = TempDir::new().unwrap();
    std::fs::create_dir_all(dir.path().join("existing")).unwrap();
    let lua = setup_static(dir.path());
    lua.load(r#"fs.mkdir_p("existing")"#).exec().unwrap();
    assert!(dir.path().join("existing").is_dir());
}

#[test]
fn static_exists_reports_present_and_missing() {
    let dir = TempDir::new().unwrap();
    std::fs::write(dir.path().join("present.txt"), "x").unwrap();
    let lua = setup_static(dir.path());
    let yes: bool = lua
        .load(r#"return fs.exists("present.txt")"#)
        .eval()
        .unwrap();
    let no: bool = lua
        .load(r#"return fs.exists("missing.txt")"#)
        .eval()
        .unwrap();
    assert!(yes);
    assert!(!no);
}

// ---- Live-source tests (cook-luaotp call pattern, CS-0017) -------

/// The live source must reflect post-registration mutations to the
/// shared slot — this is the CS-0017 multi-Cookfile imports
/// requirement: one worker VM, many cwds.
#[test]
fn live_resolves_against_current_slot_on_each_call() {
    let dir1 = TempDir::new().unwrap();
    let dir2 = TempDir::new().unwrap();
    std::fs::write(dir1.path().join("data.txt"), "from-dir1").unwrap();
    std::fs::write(dir2.path().join("data.txt"), "from-dir2").unwrap();

    let slot = Arc::new(Mutex::new(dir1.path().to_path_buf()));
    let lua = setup_live(Arc::clone(&slot));

    let s1: String = lua.load(r#"return fs.read("data.txt")"#).eval().unwrap();
    assert_eq!(s1, "from-dir1");

    // Simulate the worker pulling a new work item from a different
    // Cookfile; update the slot in place.
    *slot.lock().unwrap() = dir2.path().to_path_buf();

    let s2: String = lua.load(r#"return fs.read("data.txt")"#).eval().unwrap();
    assert_eq!(
        s2, "from-dir2",
        "Live source must observe slot mutation between calls"
    );
}

#[test]
fn live_write_lands_under_current_slot() {
    let dir1 = TempDir::new().unwrap();
    let dir2 = TempDir::new().unwrap();
    let slot = Arc::new(Mutex::new(dir1.path().to_path_buf()));
    let lua = setup_live(Arc::clone(&slot));

    lua.load(r#"fs.write("a.txt", "first")"#).exec().unwrap();
    assert_eq!(
        std::fs::read_to_string(dir1.path().join("a.txt")).unwrap(),
        "first"
    );

    *slot.lock().unwrap() = dir2.path().to_path_buf();
    lua.load(r#"fs.write("b.txt", "second")"#).exec().unwrap();
    assert_eq!(
        std::fs::read_to_string(dir2.path().join("b.txt")).unwrap(),
        "second"
    );
    // The pre-mutation file stays put under dir1 — proves writes
    // didn't leak across the slot change.
    assert!(!dir2.path().join("a.txt").exists());
}

#[test]
fn live_glob_uses_current_slot() {
    let dir = TempDir::new().unwrap();
    std::fs::write(dir.path().join("a.txt"), "").unwrap();
    std::fs::write(dir.path().join("b.txt"), "").unwrap();

    let slot = Arc::new(Mutex::new(dir.path().to_path_buf()));
    let lua = setup_live(slot);

    let count: usize = lua
        .load(r#"return #fs.glob("*.txt")"#)
        .eval()
        .unwrap();
    assert_eq!(count, 2);
}

/// CS-0064: `fs.glob` drops sub-directories from its results so the
/// downstream `cook.add_unit` directory-input rejection (CS-0063)
/// never fires for a path the author didn't write by hand.
#[test]
fn static_glob_filters_out_directories() {
    let dir = TempDir::new().unwrap();
    std::fs::write(dir.path().join("a.txt"), "").unwrap();
    std::fs::write(dir.path().join("b.txt"), "").unwrap();
    std::fs::create_dir(dir.path().join("nested")).unwrap();
    std::fs::write(dir.path().join("nested/c.txt"), "").unwrap();

    let lua = setup_static(dir.path());
    let table: LuaTable = lua
        .load(r#"return fs.glob("*")"#)
        .eval()
        .unwrap();
    let mut got: Vec<String> = table
        .sequence_values::<String>()
        .map(Result::unwrap)
        .map(|p| std::path::Path::new(&p)
            .file_name()
            .unwrap()
            .to_string_lossy()
            .into_owned())
        .collect();
    got.sort();
    assert_eq!(got, vec!["a.txt".to_string(), "b.txt".to_string()]);
}

/// CS-0064: a symlink whose target is a directory is also dropped.
/// `std::fs::metadata` follows the link, so the filter sees the
/// terminal directory rather than the symlink itself.
#[cfg(unix)]
#[test]
fn static_glob_filters_symlink_to_directory() {
    let dir = TempDir::new().unwrap();
    let real = dir.path().join("real");
    std::fs::create_dir(&real).unwrap();
    std::fs::write(dir.path().join("a.txt"), "").unwrap();
    std::os::unix::fs::symlink(&real, dir.path().join("link")).unwrap();

    let lua = setup_static(dir.path());
    let table: LuaTable = lua
        .load(r#"return fs.glob("*")"#)
        .eval()
        .unwrap();
    let mut got: Vec<String> = table
        .sequence_values::<String>()
        .map(Result::unwrap)
        .map(|p| std::path::Path::new(&p)
            .file_name()
            .unwrap()
            .to_string_lossy()
            .into_owned())
        .collect();
    got.sort();
    assert_eq!(got, vec!["a.txt".to_string()]);
}

/// CS-0064: a symlink whose target is a regular file is kept.
/// Mirrors the previous test's setup to pin the negative direction
/// of the symlink-follow rule.
#[cfg(unix)]
#[test]
fn static_glob_keeps_symlink_to_file() {
    let dir = TempDir::new().unwrap();
    let real = dir.path().join("real.txt");
    std::fs::write(&real, "").unwrap();
    std::os::unix::fs::symlink(&real, dir.path().join("link.txt")).unwrap();

    let lua = setup_static(dir.path());
    let table: LuaTable = lua
        .load(r#"return fs.glob("*.txt")"#)
        .eval()
        .unwrap();
    let mut got: Vec<String> = table
        .sequence_values::<String>()
        .map(Result::unwrap)
        .map(|p| std::path::Path::new(&p)
            .file_name()
            .unwrap()
            .to_string_lossy()
            .into_owned())
        .collect();
    got.sort();
    assert_eq!(got, vec!["link.txt".to_string(), "real.txt".to_string()]);
}

// ---- Sandbox tests (CS-0045) ------------------------------------

/// A confined `fs.read` MUST reject an absolute path outside the
/// project root with a Lua error mentioning the path.
#[test]
fn confined_fs_read_rejects_absolute_outside_root() {
    let dir = TempDir::new().unwrap();
    let lua = setup_confined(dir.path(), dir.path());
    let err = lua
        .load(r#"return fs.read("/etc/passwd")"#)
        .exec()
        .unwrap_err()
        .to_string();
    assert!(err.contains("escapes project root"), "diagnostic missing escape text: {err}");
    assert!(err.contains("/etc/passwd"), "diagnostic missing path: {err}");
}

/// A confined `fs.read` MUST reject a relative path that escapes
/// the project root via `..`.
#[test]
fn confined_fs_read_rejects_dotdot_traversal() {
    let outside = TempDir::new().unwrap();
    let project = outside.path().join("project");
    std::fs::create_dir_all(&project).unwrap();
    std::fs::write(outside.path().join("secret.txt"), "shh").unwrap();

    let lua = setup_confined(&project, &project);
    let err = lua
        .load(r#"return fs.read("../secret.txt")"#)
        .exec()
        .unwrap_err()
        .to_string();
    assert!(err.contains("escapes project root"), "got: {err}");
}

/// A confined `fs.write` to a path inside the project root MUST
/// succeed.
#[test]
fn confined_fs_write_inside_root_succeeds() {
    let dir = TempDir::new().unwrap();
    let lua = setup_confined(dir.path(), dir.path());
    lua.load(r#"fs.write("sub/x.txt", "ok")"#).exec().unwrap();
    assert_eq!(
        std::fs::read_to_string(dir.path().join("sub/x.txt")).unwrap(),
        "ok"
    );
}

/// `fs.glob` rejects an absolute pattern outside the project root.
#[test]
fn confined_fs_glob_rejects_outside_pattern() {
    let dir = TempDir::new().unwrap();
    let lua = setup_confined(dir.path(), dir.path());
    let err = lua
        .load(r#"return fs.glob("/etc/*")"#)
        .exec()
        .unwrap_err()
        .to_string();
    assert!(err.contains("escapes project root"), "got: {err}");
}

/// CS-0017: a confined fs.* call from an imported Cookfile sees a
/// subdir cwd while the project root is the workspace root. Paths
/// that stay within the workspace root MUST be admitted, even when
/// they normalize via `..` from the importer's cwd.
#[test]
fn confined_subcookfile_relative_dotdot_inside_root_ok() {
    let project = TempDir::new().unwrap();
    let sub = project.path().join("lib");
    std::fs::create_dir_all(&sub).unwrap();
    std::fs::write(project.path().join("shared.txt"), "data").unwrap();

    let lua = setup_confined(&sub, project.path());
    // From /project/lib, ../shared.txt = /project/shared.txt — inside root.
    let s: String = lua
        .load(r#"return fs.read("../shared.txt")"#)
        .eval()
        .unwrap();
    assert_eq!(s, "data");
}

// ---- Array-form tests (CS-0079) --------------------------------------

/// CS-0079: `fs.glob` accepts an array of patterns. Results are
/// concatenated in call order.
#[test]
fn static_glob_array_concatenates_in_call_order() {
    let dir = TempDir::new().unwrap();
    std::fs::create_dir_all(dir.path().join("a")).unwrap();
    std::fs::create_dir_all(dir.path().join("b")).unwrap();
    std::fs::write(dir.path().join("a/x.txt"), "").unwrap();
    std::fs::write(dir.path().join("b/y.txt"), "").unwrap();

    let lua = setup_static(dir.path());
    let table: LuaTable = lua
        .load(r#"return fs.glob({"a/*.txt", "b/*.txt"})"#)
        .eval()
        .unwrap();
    let got: Vec<String> = table
        .sequence_values::<String>()
        .map(Result::unwrap)
        .map(|p| std::path::Path::new(&p)
            .file_name()
            .unwrap()
            .to_string_lossy()
            .into_owned())
        .collect();
    assert_eq!(got, vec!["x.txt".to_string(), "y.txt".to_string()]);
}

/// CS-0079: reversing pattern order reverses result blocks. This pins
/// "concat in call order" rather than "sort across all matches".
#[test]
fn static_glob_array_order_follows_pattern_order() {
    let dir = TempDir::new().unwrap();
    std::fs::create_dir_all(dir.path().join("a")).unwrap();
    std::fs::create_dir_all(dir.path().join("b")).unwrap();
    std::fs::write(dir.path().join("a/x.txt"), "").unwrap();
    std::fs::write(dir.path().join("b/y.txt"), "").unwrap();

    let lua = setup_static(dir.path());
    let table: LuaTable = lua
        .load(r#"return fs.glob({"b/*.txt", "a/*.txt"})"#)
        .eval()
        .unwrap();
    let got: Vec<String> = table
        .sequence_values::<String>()
        .map(Result::unwrap)
        .map(|p| std::path::Path::new(&p)
            .file_name()
            .unwrap()
            .to_string_lossy()
            .into_owned())
        .collect();
    assert_eq!(got, vec!["y.txt".to_string(), "x.txt".to_string()]);
}

/// CS-0079: per-pattern CS-0064 directory-filter MUST apply to each
/// element of the array independently.
#[test]
fn static_glob_array_filters_directories_per_pattern() {
    let dir = TempDir::new().unwrap();
    std::fs::create_dir_all(dir.path().join("src")).unwrap();
    std::fs::create_dir_all(dir.path().join("src/legacy")).unwrap();
    std::fs::write(dir.path().join("src/a.c"), "").unwrap();
    std::fs::write(dir.path().join("src/b.c"), "").unwrap();

    let lua = setup_static(dir.path());
    let table: LuaTable = lua
        .load(r#"return fs.glob({"src/*"})"#)
        .eval()
        .unwrap();
    let mut got: Vec<String> = table
        .sequence_values::<String>()
        .map(Result::unwrap)
        .map(|p| std::path::Path::new(&p)
            .file_name()
            .unwrap()
            .to_string_lossy()
            .into_owned())
        .collect();
    got.sort();
    // `legacy/` is a sub-directory and MUST be filtered out per CS-0064;
    // a.c and b.c remain.
    assert_eq!(got, vec!["a.c".to_string(), "b.c".to_string()]);
}

/// CS-0079: empty array returns empty sequence; no error.
#[test]
fn static_glob_empty_array_returns_empty() {
    let dir = TempDir::new().unwrap();
    let lua = setup_static(dir.path());
    let len: usize = lua
        .load(r#"return #fs.glob({})"#)
        .eval()
        .unwrap();
    assert_eq!(len, 0);
}

/// CS-0079: a single-string call MUST still return the same result as
/// it did before the array form was added. Backcompat smoke.
#[test]
fn static_glob_string_form_unchanged() {
    let dir = TempDir::new().unwrap();
    std::fs::write(dir.path().join("a.txt"), "").unwrap();
    std::fs::write(dir.path().join("b.txt"), "").unwrap();

    let lua = setup_static(dir.path());
    let table: LuaTable = lua
        .load(r#"return fs.glob("*.txt")"#)
        .eval()
        .unwrap();
    let mut got: Vec<String> = table
        .sequence_values::<String>()
        .map(Result::unwrap)
        .map(|p| std::path::Path::new(&p)
            .file_name()
            .unwrap()
            .to_string_lossy()
            .into_owned())
        .collect();
    got.sort();
    assert_eq!(got, vec!["a.txt".to_string(), "b.txt".to_string()]);
}

/// CS-0079: a non-string element inside the array MUST raise a Lua
/// runtime error naming `fs.glob`. (Holes in the sequence — i.e. a
/// `nil` mid-array — terminate iteration at the first hole per Lua
/// sequence semantics; we accept that as the implementation-defined
/// behavior and don't test it.)
#[test]
fn static_glob_array_non_string_element_raises() {
    let dir = TempDir::new().unwrap();
    let lua = setup_static(dir.path());
    let err = lua
        .load(r#"return fs.glob({"a.txt", 42})"#)
        .exec()
        .unwrap_err()
        .to_string();
    assert!(err.contains("fs.glob"), "diagnostic missing api tag: {err}");
}

/// CS-0079: a non-string non-table argument MUST raise a Lua runtime
/// error naming `fs.glob`.
#[test]
fn static_glob_bad_argument_type_raises() {
    let dir = TempDir::new().unwrap();
    let lua = setup_static(dir.path());
    let err = lua
        .load(r#"return fs.glob(42)"#)
        .exec()
        .unwrap_err()
        .to_string();
    assert!(err.contains("fs.glob"), "diagnostic missing api tag: {err}");
}

/// CS-0079: sandbox check MUST apply to every pattern in the array.
/// A confined `fs.glob` rejects an absolute pattern outside the
/// project root even when it appears in position 2+ of the array.
#[test]
fn confined_fs_glob_array_rejects_outside_pattern() {
    let dir = TempDir::new().unwrap();
    std::fs::write(dir.path().join("a.txt"), "").unwrap();
    let lua = setup_confined(dir.path(), dir.path());
    let err = lua
        .load(r#"return fs.glob({"*.txt", "/etc/*"})"#)
        .exec()
        .unwrap_err()
        .to_string();
    assert!(err.contains("escapes project root"), "got: {err}");
}
