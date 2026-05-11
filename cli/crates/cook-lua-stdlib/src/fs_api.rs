//! `fs.*` — filesystem helpers, working-directory rooted (§6.5).
//!
//! All entries resolve relative paths against the working directory
//! provided by the supplied [`WorkingDirSource`]. The
//! `WorkingDirSource::Live` variant resolves the cwd on every call so
//! a worker VM that processes items from multiple Cookfiles
//! (CS-0017 multi-Cookfile imports) sees each item's own cwd, not the
//! cwd in effect when the `fs` table was first registered.
//!
//! Bug fixes to `fs.*` semantics MUST land here so both the
//! register-phase and execute-phase VMs benefit (CS-0044).
//!
//! Each entry consults a [`SandboxSource`] (CS-0045) before performing
//! I/O. Under `SandboxPolicy::Confined` the call rejects paths that
//! resolve outside the project root with a Lua runtime error; under
//! `SandboxPolicy::Off` the call behaves as it did pre-CS-0045. Plate
//! step Lua bodies are the only execute-phase context that runs with
//! `Off`; cook/test/chore step bodies and all register-phase Lua run
//! with `Confined`.

use mlua::prelude::*;

use crate::sandbox::{SandboxPolicy, SandboxSource};
use crate::WorkingDirSource;

/// Register the `fs` table on the supplied Lua VM, with no sandbox
/// (pre-CS-0045 behavior). Kept as a thin wrapper for callers that
/// have not yet been ported to the sandbox-aware factory; new call
/// sites SHOULD use [`register_fs_api_with_sandbox`] directly.
pub fn register_fs_api(lua: &Lua, wd_source: WorkingDirSource) -> LuaResult<()> {
    register_fs_api_with_sandbox(lua, wd_source, SandboxSource::off())
}

/// Register the `fs` table on the supplied Lua VM with a sandbox
/// policy. CS-0045.
///
/// `wd_source` and `sandbox` are each cloned once per registered
/// closure so every entry independently resolves its working directory
/// and policy at call time.
pub fn register_fs_api_with_sandbox(
    lua: &Lua,
    wd_source: WorkingDirSource,
    sandbox: SandboxSource,
) -> LuaResult<()> {
    let fs = lua.create_table()?;

    let s = wd_source.clone();
    let sb = sandbox.clone();
    fs.set(
        "exists",
        lua.create_function(move |_, path: String| {
            let full = check_path(&sb, "fs.exists", &s.resolve(), &path)?;
            Ok(full.exists())
        })?,
    )?;

    let s = wd_source.clone();
    let sb = sandbox.clone();
    fs.set(
        "size",
        lua.create_function(move |_, path: String| {
            let full = check_path(&sb, "fs.size", &s.resolve(), &path)?;
            let meta = std::fs::metadata(&full)
                .map_err(|e| mlua::Error::runtime(format!("fs.size: {e}")))?;
            Ok(meta.len())
        })?,
    )?;

    let s = wd_source.clone();
    let sb = sandbox.clone();
    fs.set(
        "read",
        lua.create_function(move |_, path: String| {
            let full = check_path(&sb, "fs.read", &s.resolve(), &path)?;
            let content = std::fs::read_to_string(&full)
                .map_err(|e| mlua::Error::runtime(format!("fs.read: {e}")))?;
            Ok(content)
        })?,
    )?;

    let s = wd_source.clone();
    let sb = sandbox.clone();
    fs.set(
        "glob",
        lua.create_function(move |lua, pattern: String| {
            // Glob's pattern is itself a path-like string; sandbox it
            // with the same resolution as fs.read/fs.write so
            // `fs.glob("/etc/*")` raises rather than enumerating
            // outside the project. The resulting matches are also
            // re-checked: a glob that crosses a `..` boundary mid-
            // pattern (`fs.glob("../*")`) must reject every match.
            let full_pattern_path = check_path(&sb, "fs.glob", &s.resolve(), &pattern)?;
            let full_pattern = full_pattern_path.to_string_lossy().to_string();
            let policy = sb.resolve();
            let wd = s.resolve();
            let mut paths: Vec<String> = Vec::new();
            for entry in glob::glob(&full_pattern)
                .map_err(|e| mlua::Error::runtime(format!("fs.glob: {e}")))?
            {
                let path = match entry {
                    Ok(p) => p,
                    Err(_) => continue,
                };
                // Re-check each match against the sandbox. This guards
                // the case where `pattern` itself sandbox-checked clean
                // but a wildcard expands to a symlink target that
                // escapes the root (best-effort: see Note 6.5.3 — we
                // do not chase hostile symlinks).
                let lossy = path.to_string_lossy().to_string();
                if policy.resolve("fs.glob", &wd, &lossy).is_ok()
                    && !resolves_to_directory(&path)
                {
                    paths.push(lossy);
                }
            }
            let table = lua.create_table()?;
            for (i, path) in paths.iter().enumerate() {
                table.set(i + 1, path.as_str())?;
            }
            Ok(table)
        })?,
    )?;

    let s = wd_source.clone();
    let sb = sandbox.clone();
    fs.set(
        "mtime",
        lua.create_function(move |_, path: String| {
            let full = check_path(&sb, "fs.mtime", &s.resolve(), &path)?;
            let meta = std::fs::metadata(&full)
                .map_err(|e| mlua::Error::runtime(format!("fs.mtime: {e}")))?;
            let mtime = meta
                .modified()
                .map_err(|e| mlua::Error::runtime(format!("fs.mtime: {e}")))?;
            let duration = mtime
                .duration_since(std::time::UNIX_EPOCH)
                .map_err(|e| mlua::Error::runtime(format!("fs.mtime: {e}")))?;
            Ok(duration.as_secs_f64())
        })?,
    )?;

    let s = wd_source.clone();
    let sb = sandbox.clone();
    fs.set(
        "write",
        lua.create_function(move |_, (path, content): (String, String)| {
            let full = check_path(&sb, "fs.write", &s.resolve(), &path)?;
            if let Some(parent) = full.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| mlua::Error::runtime(format!("fs.write: {e}")))?;
            }
            std::fs::write(&full, content)
                .map_err(|e| mlua::Error::runtime(format!("fs.write: {e}")))?;
            Ok(())
        })?,
    )?;

    let s = wd_source.clone();
    let sb = sandbox.clone();
    fs.set(
        "mkdir_p",
        lua.create_function(move |_, path: String| {
            let full = check_path(&sb, "fs.mkdir_p", &s.resolve(), &path)?;
            std::fs::create_dir_all(&full)
                .map_err(|e| mlua::Error::runtime(format!("fs.mkdir_p: {e}")))?;
            Ok(())
        })?,
    )?;

    lua.globals().set("fs", fs)?;
    Ok(())
}

/// Resolve `user_path` against `working_dir` and apply the active
/// sandbox policy. On success returns the absolute path the OS call
/// should use; on failure raises a Lua runtime error tagged with the
/// `api` label so the user sees which entry rejected the path.
fn check_path(
    sandbox: &SandboxSource,
    api: &'static str,
    working_dir: &std::path::Path,
    user_path: &str,
) -> LuaResult<std::path::PathBuf> {
    let policy: SandboxPolicy = sandbox.resolve();
    policy
        .resolve(api, working_dir, user_path)
        .map_err(|e| mlua::Error::runtime(e.to_string()))
}

/// True iff `path` resolves to a directory after following symlinks.
/// `fs.glob` filters these out (§6.5.6, CS-0064): cook's only
/// downstream consumer of glob results — `cook.add_unit` inputs —
/// already rejects directory paths (CS-0063), so a glob like
/// `dir/*` that matches a sub-directory would otherwise raise the
/// directory-rejection diagnostic for a path the author never wrote
/// by hand. Drop it here instead.
///
/// `std::fs::metadata` follows symlinks, so a symlink whose target is
/// a directory is also treated as a directory. A broken symlink (or
/// any other stat error) is treated as "not a directory" — `fs.glob`
/// is a read-only enumerator and any downstream consumer that needs
/// the path to actually exist will diagnose the missing file with a
/// more specific message.
fn resolves_to_directory(path: &std::path::Path) -> bool {
    matches!(std::fs::metadata(path), Ok(m) if m.is_dir())
}

#[cfg(test)]
mod tests {
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
        assert!(err.contains("CS-0045"), "diagnostic missing CS-0045 tag: {err}");
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
        assert!(err.contains("CS-0045"), "got: {err}");
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
        assert!(err.contains("CS-0045"), "got: {err}");
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
}
