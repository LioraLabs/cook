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

/// Match `pattern` against the filesystem, applying the sandbox guard
/// once on the pattern itself and once per match, and dropping
/// directory matches per CS-0064. Returns the matched paths as strings,
/// in the order `glob::glob` yields them.
///
/// Shared between the single-string and array-of-string forms of
/// `fs.glob`. CS-0079.
fn glob_one_pattern(
    sandbox: &SandboxSource,
    working_dir: &std::path::Path,
    pattern: &str,
) -> LuaResult<Vec<String>> {
    let full_pattern_path = check_path(sandbox, "fs.glob", working_dir, pattern)?;
    let full_pattern = full_pattern_path.to_string_lossy().to_string();
    let policy = sandbox.resolve();
    let mut paths: Vec<String> = Vec::new();
    for entry in glob::glob(&full_pattern)
        .map_err(|e| mlua::Error::runtime(format!("fs.glob: {e}")))?
    {
        let path = match entry {
            Ok(p) => p,
            Err(_) => continue,
        };
        let lossy = path.to_string_lossy().to_string();
        if policy.resolve("fs.glob", working_dir, &lossy).is_ok()
            && !resolves_to_directory(&path)
        {
            paths.push(lossy);
        }
    }
    Ok(paths)
}

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
        lua.create_function(move |lua, pattern: LuaValue| {
            // CS-0079: pattern is either a single Lua string or an array
            // of Lua strings. Build a Vec<String> of pattern texts, then
            // glob each in call order and concatenate the results,
            // preserving per-pattern internal order. The single-string
            // form is preserved bit-for-bit (one element vec, one call
            // into the helper, identical result table shape).
            //
            // Glob's pattern is itself a path-like string; sandbox it
            // with the same resolution as fs.read/fs.write so
            // `fs.glob("/etc/*")` raises rather than enumerating
            // outside the project. The resulting matches are also
            // re-checked: a glob that crosses a `..` boundary mid-
            // pattern (`fs.glob("../*")`) must reject every match.
            let patterns: Vec<String> = match pattern {
                LuaValue::String(luastr) => vec![luastr.to_str()?.to_string()],
                LuaValue::Table(t) => {
                    let mut v: Vec<String> = Vec::new();
                    for entry in t.sequence_values::<LuaValue>() {
                        let val = entry.map_err(|e| mlua::Error::runtime(
                            format!("fs.glob: array iteration failed: {e}")
                        ))?;
                        match val {
                            LuaValue::String(ls) => v.push(ls.to_str()?.to_string()),
                            other => {
                                return Err(mlua::Error::runtime(format!(
                                    "fs.glob: array elements must be strings, got {}",
                                    other.type_name()
                                )));
                            }
                        }
                    }
                    v
                }
                other => {
                    return Err(mlua::Error::runtime(format!(
                        "fs.glob: expected string or array of string, got {}",
                        other.type_name()
                    )));
                }
            };

            let wd = s.resolve();
            let mut all_paths: Vec<String> = Vec::new();
            for pat in &patterns {
                let matched = glob_one_pattern(&sb, &wd, pat)?;
                all_paths.extend(matched);
            }
            let table = lua.create_table()?;
            for (i, path) in all_paths.iter().enumerate() {
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
#[path = "tests/fs_api_tests.rs"]
mod tests;
