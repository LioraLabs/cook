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

/// Forces the named recipe's body to be evaluated to completion, and
/// reports the failure as a Lua error if it cannot be.
///
/// Supplied by `engine.rs`'s body-invocation driver, which owns the
/// re-entrant visit. An `Rc` rather than a `Box` so `require_recipe_fn` can
/// clone the forcer out of its cell and DROP the borrow before calling it:
/// forcing re-enters that same closure via the callee's body, which borrows
/// the cell again.
pub type RecipeForcer = Rc<dyn Fn(&Lua, &str) -> Result<(), mlua::Error>>;

/// The forcer cell `cook.require_recipe` reads at call time.
///
/// A CELL, not a value, because of an ordering fact that has already caused
/// one silent-failure bug: the top-level Lua chunk runs BEFORE the
/// body-invocation driver exists, so any forcer passed by value at
/// API-install time is necessarily forcer-less. Re-registering the function
/// once the driver exists does not fix that — `local rr = cook.require_recipe`
/// at top level is ordinary Lua (`local sh = cook.sh` is idiomatic), and the
/// alias keeps the closure it captured, forcer and all. ONE closure reading a
/// cell filled in later is what makes an alias and a fresh `cook.` lookup
/// behave identically.
///
/// Empty forever on VMs with no driver at all (`list_names`), and on the
/// `register_cookfile` VM until `engine.rs` fills it. Every call reachable
/// before then is outside a recipe body, so the guard rail fires first — but
/// an empty cell reached from INSIDE a body is a hard error, never a silent
/// no-op (§22.8: the forcing MUST NOT degrade to a silent skip).
pub type SharedRecipeForcer = Rc<RefCell<Option<RecipeForcer>>>;

/// Register `cook.require_recipe(name)` on the cook global table (Standard
/// §22.8, CS-0144).
///
/// Declares that the enclosing recipe's register-phase body depends on
/// another recipe, named BARE — the same unqualified namespace the
/// `requires` metadata field and the surface `recipe A : B` dep-list use.
/// Two effects, in order: `force` evaluates `name`'s body to completion
/// (the register-order guarantee — everything that body exports is
/// observable once this call returns), then the name accumulates into
/// `BodyCaptureState.dynamic_requires` (order-preserving, de-duplicated),
/// which `engine.rs` merges into the caller's `requires` at body drain.
///
/// Sibling to `register_recipe_name_api` immediately above: same
/// registration pattern, same "only inside a recipe body" error voice,
/// reading the same `body_slot`/`current_recipe` `None` signal — one check
/// covers every outside-a-body caller (top level, a `register` block, and
/// a `for_each`-feeding probe's `produce` on the register VM).
pub fn register_require_recipe_api(
    lua: &Lua,
    body_slot: SharedBodySlot,
    force: SharedRecipeForcer,
) -> Result<(), RegisterError> {
    let cook: LuaTable = lua.globals().get("cook")?;
    let require_recipe_fn = lua.create_function(move |lua, name: LuaValue| {
        // Validate against the caller's body state, then drop the borrow
        // *before* forcing: `force` re-enters this very closure via the
        // callee's body, and swaps the body slot out from under us.
        let name = {
            let slot = body_slot.borrow();
            let body = match slot.as_ref() {
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
            // why comparing against `current_recipe` (qualified) instead
            // would silently never fire under an import prefix.
            let current_bare = body
                .current_recipe_bare
                .clone()
                .expect("current_recipe_bare stamped alongside current_recipe");
            if current_bare == name {
                return Err(mlua::Error::runtime(format!(
                    "cook.require_recipe: recipe \"{name}\" cannot require itself (Standard \u{00a7}22.8, CS-0144)"
                )));
            }
            name
        };

        // The register-order guarantee. Unconditional even for a name
        // already in `dynamic_requires` — the driver's visit map is what
        // makes a repeat call a no-op, and routing every call through it
        // keeps "already evaluated" one decision in one place.
        //
        // Cloned out of the cell so the borrow is released before the call:
        // `force` re-enters this closure via the callee's body, which reads
        // the cell again.
        let forcer = force.borrow().clone();
        match forcer {
            Some(force) => force(lua, &name)?,
            // Unreachable via any path that exists today — the guard rail
            // above already rejected every outside-a-body caller, and inside
            // a body the driver has long since filled the cell. It is an
            // error rather than a `()` because the silent alternative is the
            // exact defect this API exists to eliminate: the edge below would
            // still be recorded, so the recipe would join the build closure
            // while its export stayed unobservable and `cook.import` returned
            // nil. A future VM that installs the API without a driver must
            // fail loudly here, not mislink.
            None => {
                return Err(mlua::Error::runtime(format!(
                    "cook.require_recipe: cannot force recipe \"{name}\": no body-invocation \
                     driver is available on this register-phase VM. This is an implementation \
                     fault, not a Cookfile error; please report it (Standard \u{00a7}22.8, CS-0144)"
                )))
            }
        }

        // The slot the driver restored on the way out is the caller's
        // again, so this lands the edge on the right recipe.
        let mut slot = body_slot.borrow_mut();
        let body = slot
            .as_mut()
            .expect("body slot restored by the driver before force returns");
        if !body.dynamic_requires.contains(&name) {
            body.dynamic_requires.push(name);
        }
        Ok(())
    })?;
    cook.set("require_recipe", require_recipe_fn)?;
    Ok(())
}

/// Give `cook.dep_order` `require_recipe`'s register-order guarantee.
///
/// COOK-297 (revised): `dep_order` is the fine-grained replacement for
/// `require_recipe`, not a companion to it. A module that resolves link
/// references needs the referent's body evaluated so `cook.import` returns its
/// export — that forcing was the ONLY reason `cook_cc` called
/// `require_recipe`, and the coarse whole-recipe barrier came along as an
/// unwanted side effect. Moving the force here lets a maker drop
/// `require_recipe` altogether: `dep_order` forces the body, records a
/// per-unit edge, and (via `RecipeInfo.orders`) establishes closure
/// membership — everything `require_recipe` did except manufacture the barrier.
///
/// Installed AFTER `register_dep_output_api`, which is what defines the
/// function being wrapped. The raw accumulator-only implementation is kept at
/// `cook.__dep_order_raw` and fetched per call rather than captured, so this
/// wrapper holds no Lua handle across registrations.
pub fn register_dep_order_forcing(
    lua: &Lua,
    body_slot: SharedBodySlot,
    force: SharedRecipeForcer,
) -> Result<(), RegisterError> {
    let cook: LuaTable = lua.globals().get("cook")?;
    let raw: LuaFunction = cook.get("dep_order")?;
    cook.set("__dep_order_raw", raw)?;

    let dep_order_fn = lua.create_function(move |lua, name: String| {
        // Validate against the caller's body state, then drop the borrow
        // before forcing — `force` re-enters register APIs via the callee's
        // body and swaps the body slot out from under us.
        {
            let slot = body_slot.borrow();
            match slot.as_ref() {
                Some(body) if body.current_recipe.is_some() => {}
                _ => {
                    return Err(mlua::Error::runtime(
                        "cook.dep_order must be called inside a recipe body",
                    ))
                }
            }
        }
        let forcer = force.borrow().clone();
        match forcer {
            Some(force) => force(lua, &name)?,
            None => {
                return Err(mlua::Error::runtime(format!(
                    "cook.dep_order: cannot force recipe \"{name}\": no \
                     body-invocation driver is installed"
                )))
            }
        }
        let cook: LuaTable = lua.globals().get("cook")?;
        let raw: LuaFunction = cook.get("__dep_order_raw")?;
        raw.call::<()>(name)
    })?;
    cook.set("dep_order", dep_order_fn)?;
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
