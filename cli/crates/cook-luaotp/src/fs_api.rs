use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use mlua::prelude::*;

/// Register the `fs` table once per worker VM.
///
/// Each closure reads from a shared `Arc<Mutex<PathBuf>>` so paths resolve
/// against the *current* work item's working_dir, not the one in effect
/// when the table was first registered. This matters for CS-0017
/// multi-Cookfile imports, where the same worker VM may receive items
/// from different Cookfiles with different cwds.
pub fn register_fs_api(
    lua: &Lua,
    current_working_dir: &Arc<Mutex<PathBuf>>,
) -> LuaResult<()> {
    let fs = lua.create_table()?;

    fn cwd(slot: &Mutex<PathBuf>) -> PathBuf {
        slot.lock().expect("working_dir lock").clone()
    }

    let wd = Arc::clone(current_working_dir);
    fs.set(
        "exists",
        lua.create_function(move |_, path: String| {
            Ok(cwd(&wd).join(&path).exists())
        })?,
    )?;

    let wd = Arc::clone(current_working_dir);
    fs.set(
        "size",
        lua.create_function(move |_, path: String| {
            let full = cwd(&wd).join(&path);
            let meta = std::fs::metadata(&full)
                .map_err(|e| mlua::Error::runtime(format!("fs.size: {e}")))?;
            Ok(meta.len())
        })?,
    )?;

    let wd = Arc::clone(current_working_dir);
    fs.set(
        "read",
        lua.create_function(move |_, path: String| {
            let full = cwd(&wd).join(&path);
            let content = std::fs::read_to_string(&full)
                .map_err(|e| mlua::Error::runtime(format!("fs.read: {e}")))?;
            Ok(content)
        })?,
    )?;

    let wd = Arc::clone(current_working_dir);
    fs.set(
        "glob",
        lua.create_function(move |lua, pattern: String| {
            let full_pattern = cwd(&wd).join(&pattern).to_string_lossy().to_string();
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

    let wd = Arc::clone(current_working_dir);
    fs.set(
        "mtime",
        lua.create_function(move |_, path: String| {
            let full = cwd(&wd).join(&path);
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

    let wd = Arc::clone(current_working_dir);
    fs.set(
        "write",
        lua.create_function(move |_, (path, content): (String, String)| {
            let full = cwd(&wd).join(&path);
            if let Some(parent) = full.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| mlua::Error::runtime(format!("fs.write: {e}")))?;
            }
            std::fs::write(&full, content)
                .map_err(|e| mlua::Error::runtime(format!("fs.write: {e}")))?;
            Ok(())
        })?,
    )?;

    let wd = Arc::clone(current_working_dir);
    fs.set(
        "mkdir_p",
        lua.create_function(move |_, path: String| {
            let full = cwd(&wd).join(&path);
            std::fs::create_dir_all(&full)
                .map_err(|e| mlua::Error::runtime(format!("fs.mkdir_p: {e}")))?;
            Ok(())
        })?,
    )?;

    lua.globals().set("fs", fs)?;
    Ok(())
}
