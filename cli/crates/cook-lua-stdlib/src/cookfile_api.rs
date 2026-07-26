//! `cook.cookfile.*` — structure-preserving Cookfile edits from Lua
//! (Standard §22.12, CS-0179).
//!
//! The surface a module's project-management chores (`cc.add`, `cc.link`,
//! `cc.need`) use to write back into the Cookfile that invoked them. The
//! editing itself lives in `cook-cookfile`; this module is the binding, the
//! sandbox check, and the read/modify/write.
//!
//! # Why this is blessed rather than left to modules
//!
//! Every one of these calls edits a file the user wrote by hand and did not
//! ask to have reformatted. Locating a multi-line call whose braces nest and
//! whose strings may contain braces is genuinely hard, and a module doing it
//! in Lua would be hand-rolling a scanner whose failure mode is corrupting
//! that file. One correct implementation, shared, is worth the surface.
//!
//! # Sandbox
//!
//! Paths go through the same [`check_path`] gate as `fs.*` (CS-0045), so a
//! chore cannot rewrite a file outside the project root. Nothing here can
//! reach a Cookfile belonging to another project.

use std::path::PathBuf;

use mlua::{Lua, Result as LuaResult, Table as LuaTable};

use crate::fs_api::check_path;
use crate::sandbox::SandboxSource;
use crate::WorkingDirSource;

/// Read the Cookfile at `path` through the sandbox gate.
fn read_source(
    sandbox: &SandboxSource,
    api: &'static str,
    wd: &PathBuf,
    path: &str,
) -> LuaResult<(PathBuf, String)> {
    let full = check_path(sandbox, api, wd, path)?;
    let source = std::fs::read_to_string(&full)
        .map_err(|e| mlua::Error::runtime(format!("{api}: {}: {e}", full.display())))?;
    Ok((full, source))
}

/// Register the `cook.cookfile` table on the supplied VM.
///
/// `wd_source` and `sandbox` are cloned per closure so each call resolves the
/// working directory and policy at call time, matching `fs.*`.
pub fn register_cookfile_api(
    lua: &Lua,
    cook: &LuaTable,
    wd_source: WorkingDirSource,
    sandbox: SandboxSource,
) -> LuaResult<()> {
    let tbl = lua.create_table()?;

    // cook.cookfile.splice_field(path, recipe, field, entry) -> true
    //
    // Insert `entry` into `field`'s `{ ... }` list, in the module call inside
    // `recipe`. `entry` is written verbatim, so the caller renders its own
    // quoting — `"\"mathlib\""` for a string entry.
    //
    // Errors rather than guessing when the shape is not what it expects; the
    // message names the manual fix. That honesty is the entire reason this is
    // a splice and not a decode/re-encode, which cannot fail this way because
    // it cannot tell that anything was wrong.
    let s = wd_source.clone();
    let sb = sandbox.clone();
    tbl.set(
        "splice_field",
        lua.create_function(
            move |_, (path, recipe, field, entry): (String, String, String, String)| {
                let (full, source) =
                    read_source(&sb, "cook.cookfile.splice_field", &s.resolve(), &path)?;
                let edited = cook_cookfile::splice_into_field(&source, &recipe, &field, &entry)
                    .map_err(|e| {
                        mlua::Error::runtime(format!(
                            "cook.cookfile.splice_field: {}: {e}",
                            full.display()
                        ))
                    })?;
                std::fs::write(&full, edited).map_err(|e| {
                    mlua::Error::runtime(format!(
                        "cook.cookfile.splice_field: writing {}: {e}",
                        full.display()
                    ))
                })?;
                Ok(true)
            },
        )?,
    )?;

    // cook.cookfile.append(path, text) -> true
    //
    // Append a declaration at end of file, with exactly one blank line before
    // it and a trailing newline after. What `cc.add` uses: a new `recipe`
    // block has no enclosing structure to splice into.
    let s = wd_source.clone();
    let sb = sandbox.clone();
    tbl.set(
        "append",
        lua.create_function(move |_, (path, text): (String, String)| {
            let (full, source) = read_source(&sb, "cook.cookfile.append", &s.resolve(), &path)?;
            let edited = cook_cookfile::append_declaration(&source, &text);
            std::fs::write(&full, edited).map_err(|e| {
                mlua::Error::runtime(format!("cook.cookfile.append: writing {}: {e}", full.display()))
            })?;
            Ok(true)
        })?,
    )?;

    // cook.cookfile.find_call(path, recipe) -> {callee=, text=} | nil
    //
    // Look without editing. Returns nil when the recipe holds no module call,
    // so a verb can check before it writes; a genuinely broken Cookfile still
    // raises, because "no call here" and "this file does not parse" are
    // different answers and a caller that conflates them would report the
    // wrong fix to the user.
    let s = wd_source.clone();
    let sb = sandbox.clone();
    tbl.set(
        "find_call",
        lua.create_function(move |lua, (path, recipe): (String, String)| {
            let (full, source) =
                read_source(&sb, "cook.cookfile.find_call", &s.resolve(), &path)?;
            match cook_cookfile::find_call(&source, &recipe) {
                Ok(call) => {
                    let t = lua.create_table()?;
                    t.set("callee", call.callee)?;
                    t.set("text", source[call.span].to_string())?;
                    Ok(mlua::Value::Table(t))
                }
                Err(cook_cookfile::EditError::RecipeNotFound { .. })
                | Err(cook_cookfile::EditError::NoModuleCall { .. }) => Ok(mlua::Value::Nil),
                Err(e) => Err(mlua::Error::runtime(format!(
                    "cook.cookfile.find_call: {}: {e}",
                    full.display()
                ))),
            }
        })?,
    )?;

    cook.set("cookfile", tbl)?;
    Ok(())
}

#[cfg(test)]
#[path = "tests/cookfile_api_tests.rs"]
mod tests;
