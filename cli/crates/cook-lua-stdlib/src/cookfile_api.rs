//! `cook.cookfile.*` — structure-preserving Cookfile edits from Lua
//! (Standard §22.13, CS-0179).
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

/// Read `splice_field`'s optional trailing options table (CS-0221).
///
/// Absent, it is [`AbsentField::Refuse`] — §22.13's total failure, which is the
/// behaviour a caller that says nothing must keep getting.
///
/// Unknown keys and non-boolean values are refused rather than ignored. The
/// two policies differ in whether a Cookfile gets a field written into it, and
/// a mistyped `create_if_missing` that silently means "refuse" would be
/// reported to the user as the engine declining an edit their verb believed it
/// had asked for.
fn absent_field_policy(options: Option<LuaTable>) -> LuaResult<cook_cookfile::AbsentField> {
    const API: &str = "cook.cookfile.splice_field";
    let Some(table) = options else {
        return Ok(cook_cookfile::AbsentField::Refuse);
    };
    let mut policy = cook_cookfile::AbsentField::Refuse;
    for pair in table.pairs::<mlua::Value, mlua::Value>() {
        let (key, value) = pair?;
        let name = match key.as_string() {
            Some(s) => s.to_string_lossy().to_string(),
            None => {
                return Err(mlua::Error::runtime(format!(
                    "{API}: options keys must be strings; the options are: create_if_absent"
                )))
            }
        };
        match name.as_str() {
            "create_if_absent" => match value {
                mlua::Value::Boolean(true) => policy = cook_cookfile::AbsentField::Create,
                mlua::Value::Boolean(false) => {}
                other => {
                    return Err(mlua::Error::runtime(format!(
                        "{API}: options.create_if_absent must be a boolean, got {}",
                        other.type_name()
                    )))
                }
            },
            other => {
                return Err(mlua::Error::runtime(format!(
                    "{API}: unknown option '{other}'; the options are: create_if_absent"
                )))
            }
        }
    }
    Ok(policy)
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

    // cook.cookfile.splice_field(path, recipe, field, entry [, options]) -> true
    //
    // Insert `entry` into `field`'s `{ ... }` list, in the module call inside
    // `recipe`. `entry` is written verbatim, so the caller renders its own
    // quoting — `"\"mathlib\""` for a string entry.
    //
    // Errors rather than guessing when the shape is not what it expects; the
    // message names the manual fix. That honesty is the entire reason this is
    // a splice and not a decode/re-encode, which cannot fail this way because
    // it cannot tell that anything was wrong.
    //
    // `options.create_if_absent = true` (CS-0221) is the one relaxation, and
    // it is opt-in for that reason: a caller that says nothing still gets the
    // total failure §22.13 specifies.
    let s = wd_source.clone();
    let sb = sandbox.clone();
    tbl.set(
        "splice_field",
        lua.create_function(
            move |_,
                  (path, recipe, field, entry, options): (
                String,
                String,
                String,
                String,
                Option<LuaTable>,
            )| {
                let absent = absent_field_policy(options)?;
                let (full, source) =
                    read_source(&sb, "cook.cookfile.splice_field", &s.resolve(), &path)?;
                let edited =
                    cook_cookfile::splice_into_field(&source, &recipe, &field, &entry, absent)
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

    // cook.cookfile.field_entries(path, recipe, field) -> { entry, ... } | nil
    //
    // The entries of a field's list, as written (CS-0221). nil where there is
    // nothing to read — no such recipe, no module call, no such field — which
    // is `find_call`'s rule.
    //
    // A verb asks this to be idempotent: `cc.link game math` twice must not
    // write `math` twice. The reason it is not left to the module is that the
    // obvious Lua answer, searching the call text for the entry, matches
    // inside `sources = { "src/math/main.cpp" }`.
    let s = wd_source.clone();
    let sb = sandbox.clone();
    tbl.set(
        "field_entries",
        lua.create_function(
            move |lua, (path, recipe, field): (String, String, String)| {
                let (full, source) =
                    read_source(&sb, "cook.cookfile.field_entries", &s.resolve(), &path)?;
                match cook_cookfile::field_entries(&source, &recipe, &field) {
                    Ok(Some(entries)) => Ok(mlua::Value::Table(lua.create_sequence_from(entries)?)),
                    Ok(None) => Ok(mlua::Value::Nil),
                    Err(e) => Err(mlua::Error::runtime(format!(
                        "cook.cookfile.field_entries: {}: {e}",
                        full.display()
                    ))),
                }
            },
        )?,
    )?;

    cook.set("cookfile", tbl)?;
    Ok(())
}

#[cfg(test)]
#[path = "tests/cookfile_api_tests.rs"]
mod tests;
