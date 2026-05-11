use mlua::prelude::*;
use std::collections::BTreeSet;
use std::path::Path;

use crate::capture::RegisteredRecipe;
use crate::RegisterError;

/// Set up the `recipe` global table with name and resolved ingredient files.
/// No cache operations — cache evaluation is handled by cook-engine.
pub fn setup_recipe_context(
    lua: &Lua,
    recipe: &RegisteredRecipe,
    working_dir: &Path,
) -> Result<(), RegisterError> {
    // Build recipe context table
    let recipe_table = lua.create_table()?;
    recipe_table.set("name", recipe.name.as_str())?;

    // Resolve exclude patterns into a set for fast lookup
    let mut excluded: BTreeSet<String> = BTreeSet::new();
    for pattern in &recipe.metadata.excludes {
        excluded.extend(resolve_glob(working_dir, pattern));
    }

    // Build ingredients table by resolving glob patterns, minus excludes
    let ingredients_table = lua.create_table()?;
    for (i, pattern) in recipe.metadata.ingredients.iter().enumerate() {
        let files = resolve_glob(working_dir, pattern);
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
pub fn register_resolve_ingredients(lua: &Lua, working_dir: &Path) -> Result<(), RegisterError> {
    let cook: LuaTable = lua.globals().get("cook")?;
    let wd = working_dir.to_path_buf();
    let resolve_fn = lua.create_function(move |lua, (includes, excludes): (LuaTable, LuaTable)| {
        // Collect exclude patterns and resolve them
        let mut excluded: BTreeSet<String> = BTreeSet::new();
        for exc in excludes.sequence_values::<String>() {
            let pattern = exc.map_err(|e| mlua::Error::runtime(format!("bad exclude: {e}")))?;
            excluded.extend(resolve_glob(&wd, &pattern));
        }

        // Resolve include patterns, filtering out excludes
        let mut result: Vec<String> = Vec::new();
        for inc in includes.sequence_values::<String>() {
            let pattern = inc.map_err(|e| mlua::Error::runtime(format!("bad include: {e}")))?;
            let files = resolve_glob(&wd, &pattern);
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

/// Resolve a glob pattern into a sorted set of relative file paths.
///
/// Matches whose final (symlink-resolved) metadata is a directory are
/// dropped (CS-0064): `recipe.ingredients` and `cook.resolve_ingredients`
/// feed straight into `cook.add_unit` inputs, which CS-0063 already
/// rejects directory paths from. Filtering here keeps a glob like
/// `src/*` well-defined when `src/` contains sub-directories.
fn resolve_glob(root: &Path, pattern: &str) -> BTreeSet<String> {
    let full_pattern = root.join(pattern);
    let prefix = root.to_string_lossy().to_string();

    let paths = match glob::glob(&full_pattern.to_string_lossy()) {
        Ok(p) => p,
        Err(_) => return BTreeSet::new(),
    };

    paths
        .filter_map(Result::ok)
        .filter(|p| !matches!(std::fs::metadata(p), Ok(m) if m.is_dir()))
        .map(|p| {
            let path_str = p.to_string_lossy().to_string();
            path_str
                .strip_prefix(&prefix)
                .unwrap_or(&path_str)
                .trim_start_matches('/')
                .to_string()
        })
        .collect()
}

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

        let got = resolve_glob(dir.path(), "*");
        let expected: BTreeSet<String> = ["a.txt".to_string()].into_iter().collect();
        assert_eq!(got, expected);
    }
}
