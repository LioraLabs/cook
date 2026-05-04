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

use mlua::prelude::*;

use crate::WorkingDirSource;

/// Register the `fs` table on the supplied Lua VM.
///
/// `wd_source` is cloned once per registered closure so each entry
/// independently resolves its working directory at call time.
pub fn register_fs_api(lua: &Lua, wd_source: WorkingDirSource) -> LuaResult<()> {
    let fs = lua.create_table()?;

    let s = wd_source.clone();
    fs.set(
        "exists",
        lua.create_function(move |_, path: String| Ok(s.resolve().join(&path).exists()))?,
    )?;

    let s = wd_source.clone();
    fs.set(
        "size",
        lua.create_function(move |_, path: String| {
            let full = s.resolve().join(&path);
            let meta = std::fs::metadata(&full)
                .map_err(|e| mlua::Error::runtime(format!("fs.size: {e}")))?;
            Ok(meta.len())
        })?,
    )?;

    let s = wd_source.clone();
    fs.set(
        "read",
        lua.create_function(move |_, path: String| {
            let full = s.resolve().join(&path);
            let content = std::fs::read_to_string(&full)
                .map_err(|e| mlua::Error::runtime(format!("fs.read: {e}")))?;
            Ok(content)
        })?,
    )?;

    let s = wd_source.clone();
    fs.set(
        "glob",
        lua.create_function(move |lua, pattern: String| {
            let full_pattern = s.resolve().join(&pattern).to_string_lossy().to_string();
            let paths: Vec<String> = glob::glob(&full_pattern)
                .map_err(|e| mlua::Error::runtime(format!("fs.glob: {e}")))?
                .filter_map(|p| p.ok())
                .map(|p| p.to_string_lossy().to_string())
                .collect();
            let table = lua.create_table()?;
            for (i, path) in paths.iter().enumerate() {
                table.set(i + 1, path.as_str())?;
            }
            Ok(table)
        })?,
    )?;

    let s = wd_source.clone();
    fs.set(
        "mtime",
        lua.create_function(move |_, path: String| {
            let full = s.resolve().join(&path);
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
    fs.set(
        "write",
        lua.create_function(move |_, (path, content): (String, String)| {
            let full = s.resolve().join(&path);
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
    fs.set(
        "mkdir_p",
        lua.create_function(move |_, path: String| {
            let full = s.resolve().join(&path);
            std::fs::create_dir_all(&full)
                .map_err(|e| mlua::Error::runtime(format!("fs.mkdir_p: {e}")))?;
            Ok(())
        })?,
    )?;

    lua.globals().set("fs", fs)?;
    Ok(())
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
}
