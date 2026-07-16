use mlua::prelude::*;
use std::collections::BTreeSet;
use std::path::Path;
use std::{cell::RefCell, rc::Rc};

use crate::capture::RegisteredRecipe;
use crate::{RegisterError, SharedBodySlot};

/// Set up the `recipe` global table with name and resolved ingredient files.
/// No cache operations — cache evaluation is handled by cook-engine.
pub fn setup_recipe_context(
    lua: &Lua,
    recipe: &RegisteredRecipe,
    working_dir: &Path,
    workspace_root: &Path,
    warnings: &Rc<RefCell<Vec<String>>>,
) -> Result<(), RegisterError> {
    // Build recipe context table
    let recipe_table = lua.create_table()?;
    recipe_table.set("name", recipe.name.as_str())?;

    // Resolve exclude patterns into a set for fast lookup
    let mut excluded: BTreeSet<String> = BTreeSet::new();
    for pattern in &recipe.metadata.excludes {
        excluded.extend(cook_fingerprint::resolve_ingredient_glob(working_dir, workspace_root, pattern).map_err(mlua::Error::runtime)?);
    }

    // Build ingredients table by resolving glob patterns, minus excludes
    let ingredients_table = lua.create_table()?;
    for (i, pattern) in recipe.metadata.ingredients.iter().enumerate() {
        let files = cook_fingerprint::resolve_ingredient_glob(working_dir, workspace_root, pattern).map_err(mlua::Error::runtime)?;
        if files.is_empty() {
            warnings.borrow_mut().push(format!(
                "ingredient {pattern:?} matched 0 files (recipe {})",
                recipe.name
            ));
        }
        let filtered: BTreeSet<String> = files
            .into_iter()
            .filter(|f| !excluded.contains(f))
            .collect();
        let files_table = lua.create_table()?;
        for (idx, file) in filtered.iter().enumerate() {
            files_table.set(idx + 1, file.as_str())?;
        }
        ingredients_table.set(i + 1, files_table)?;
    }
    recipe_table.set("ingredients", ingredients_table)?;

    lua.globals().set("recipe", recipe_table)?;
    Ok(())
}

/// Register `cook.resolve_ingredients(includes, excludes)` on the cook global table.
/// Returns a flat Lua table of relative file paths after glob+exclude resolution.
pub fn register_resolve_ingredients(lua: &Lua, working_dir: &Path, workspace_root: &Path) -> Result<(), RegisterError> {
    let cook: LuaTable = lua.globals().get("cook")?;
    let wd = working_dir.to_path_buf();
    let root = workspace_root.to_path_buf();
    let resolve_fn = lua.create_function(move |lua, (includes, excludes): (LuaTable, LuaTable)| {
        // Collect exclude patterns and resolve them
        let mut excluded: BTreeSet<String> = BTreeSet::new();
        for exc in excludes.sequence_values::<String>() {
            let pattern = exc.map_err(|e| mlua::Error::runtime(format!("bad exclude: {e}")))?;
            excluded.extend(cook_fingerprint::resolve_ingredient_glob(&wd, &root, &pattern).map_err(mlua::Error::runtime)?);
        }

        // Resolve include patterns, filtering out excludes
        let mut result: Vec<String> = Vec::new();
        for inc in includes.sequence_values::<String>() {
            let pattern = inc.map_err(|e| mlua::Error::runtime(format!("bad include: {e}")))?;
            let files = cook_fingerprint::resolve_ingredient_glob(&wd, &root, &pattern).map_err(mlua::Error::runtime)?;
            for f in files {
                if !excluded.contains(&f) {
                    result.push(f);
                }
            }
        }

        // Build Lua table
        let table = lua.create_table()?;
        for (i, file) in result.iter().enumerate() {
            table.set(i + 1, file.as_str())?;
        }
        Ok(table)
    })?;
    cook.set("resolve_ingredients", resolve_fn)?;
    Ok(())
}

/// Register `cook.recipe_name()` on the cook global table (Standard §22.7).
///
/// Returns the enclosing recipe's fully-qualified name — the same value
/// `cook.add_test` already reads for its `suite` default (Standard §22.4).
/// Unlike that default, which degrades a missing `current_recipe` to an
/// empty string via `.unwrap_or_default()` (`test_api.rs`), this hard
/// errors: an empty name would silently corrupt any caller folding it into
/// a path or identifier (`lib.a`, `build/obj//`).
pub fn register_recipe_name_api(lua: &Lua, body_slot: SharedBodySlot) -> Result<(), RegisterError> {
    let cook: LuaTable = lua.globals().get("cook")?;
    let recipe_name_fn = lua.create_function(move |_, ()| {
        let slot = body_slot.borrow();
        // Only the body loop opens the slot, so `None` covers every
        // outside-a-body caller at once — top level, `config`/`register`
        // blocks, and a `for_each`-feeding probe's `produce` (the prepass
        // runs before the body loop). No probe-specific guard needed.
        slot.as_ref()
            .and_then(|body| body.current_recipe.clone())
            .ok_or_else(|| {
                mlua::Error::runtime(
                    "cook.recipe_name: no enclosing recipe is active; call `cook.recipe_name()` only from inside a recipe body (Standard \u{00a7}22.7, CS-0141)",
                )
            })
    })?;
    cook.set("recipe_name", recipe_name_fn)?;
    Ok(())
}

/// Register `cook.require_recipe(name)` on the cook global table (Standard
/// §22.8, CS-0144).
///
/// Declares that the enclosing recipe's register-phase body depends on
/// another recipe, named BARE — the same unqualified namespace the
/// `requires` metadata field and the surface `recipe A : B` dep-list use.
/// Accumulates into `BodyCaptureState.dynamic_requires` (order-preserving,
/// de-duplicated). This task owns only the API surface, its guard rails,
/// and the accumulator; actually forcing the named recipe's body and
/// merging the edge into `requires` is a later task.
///
/// Sibling to `register_recipe_name_api` immediately above: same
/// registration pattern, same "only inside a recipe body" error voice,
/// reading the same `body_slot`/`current_recipe` `None` signal — one check
/// covers every outside-a-body caller (top level, a `register` block, and
/// a `for_each`-feeding probe's `produce` on the register VM).
pub fn register_require_recipe_api(lua: &Lua, body_slot: SharedBodySlot) -> Result<(), RegisterError> {
    let cook: LuaTable = lua.globals().get("cook")?;
    let require_recipe_fn = lua.create_function(move |_, name: LuaValue| {
        let mut slot = body_slot.borrow_mut();
        let body = match slot.as_mut() {
            Some(body) if body.current_recipe.is_some() => body,
            _ => {
                return Err(mlua::Error::runtime(
                    "cook.require_recipe: no enclosing recipe is active; call `cook.require_recipe(name)` only from inside a recipe body (Standard \u{00a7}22.8, CS-0144)",
                ))
            }
        };

        // CS-0143's `parse_origin_meta` precedent: match on the raw Lua
        // value so a numeric argument is rejected outright rather than
        // coerced to its decimal string.
        let name = match name {
            LuaValue::String(s) => s.to_string_lossy().to_string(),
            other => {
                return Err(mlua::Error::runtime(format!(
                    "cook.require_recipe: `name` must be a string, got {} (Standard \u{00a7}22.8, CS-0144)",
                    other.type_name()
                )))
            }
        };
        if name.is_empty() {
            return Err(mlua::Error::runtime(
                "cook.require_recipe: `name` must be a non-empty string, got an empty string (Standard \u{00a7}22.8, CS-0144)",
            ));
        }

        // Bare-to-bare self-reference check. `current_recipe_bare` is
        // stamped alongside `current_recipe` at the same point in
        // `engine.rs`, so it is guaranteed `Some` here — see its doc on
        // why comparing against `current_recipe` (qualified) instead would
        // silently never fire under an import prefix.
        let current_bare = body
            .current_recipe_bare
            .clone()
            .expect("current_recipe_bare stamped alongside current_recipe");
        if current_bare == name {
            return Err(mlua::Error::runtime(format!(
                "cook.require_recipe: recipe \"{name}\" cannot require itself (Standard \u{00a7}22.8, CS-0144)"
            )));
        }

        if !body.dynamic_requires.contains(&name) {
            body.dynamic_requires.push(name);
        }
        Ok(())
    })?;
    cook.set("require_recipe", require_recipe_fn)?;
    Ok(())
}

/// Resolve a glob pattern into a sorted set of relative file paths.
///
/// Matches whose final (symlink-resolved) metadata is a directory are
/// dropped (CS-0064): `recipe.ingredients` and `cook.resolve_ingredients`
/// feed straight into `cook.add_unit` inputs, which CS-0063 already
/// rejects directory paths from. Filtering here keeps a glob like
/// `src/*` well-defined when `src/` contains sub-directories.
#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    /// CS-0064: `recipe.ingredients` / `cook.resolve_ingredients` MUST
    /// drop sub-directory matches, so a tree with a file and a sibling
    /// directory both matched by `*` yields only the file.
    #[test]
    fn resolve_glob_filters_directories() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("a.txt"), "").unwrap();
        std::fs::create_dir(dir.path().join("nested")).unwrap();

        let got = cook_fingerprint::resolve_ingredient_glob(dir.path(), dir.path(), "*").unwrap();
        let expected: BTreeSet<String> = ["a.txt".to_string()].into_iter().collect();
        assert_eq!(got, expected);
    }

    #[test]
    fn excludes_match_lexically_equivalent_include_paths() {
        let dir = TempDir::new().unwrap();
        std::fs::create_dir(dir.path().join("dir")).unwrap();
        std::fs::write(dir.path().join("file"), "").unwrap();
        let lua = Lua::new();
        lua.globals().set("cook", lua.create_table().unwrap()).unwrap();
        register_resolve_ingredients(&lua, dir.path(), dir.path()).unwrap();

        for expression in [
            r#"return cook.resolve_ingredients({"dir/../file"}, {"file"})"#,
            r#"return cook.resolve_ingredients({"file"}, {"dir/../file"})"#,
        ] {
            let files: LuaTable = lua.load(expression).eval().unwrap();
            assert_eq!(files.raw_len(), 0, "{expression}");
        }
    }

}
