//! Filesystem layout law: the `cook_modules/` tree and the `.cook/` tree
//! (COOK-393).
//!
//! Module resolution — the tree root, the LuaRocks share/lib subtrees, the
//! §7 four-candidate probe order, the `package.path`/`package.cpath`
//! templates, so/dll selection, and the stash keys — was spelled
//! independently by the register phase (cook-register/module_loader), the
//! execute phase (cook-luaotp/pool, twice), and the installer (cook-cli
//! modules/*). The two phases were byte-identical BY HAND, each carrying a
//! comment admitting the mirror (one cross-reference already stale); drift
//! = a module that resolves at register but not at execute — the classic
//! "works in the Cookfile, missing-file at execute" failure. The pure half
//! lives here; the mlua `package.*` mutation stays per-crate and calls
//! [`compose_lua_search_paths`].

use std::path::{Path, PathBuf};

// ---------------------------------------------------------------------------
// cook_modules/ — the module tree (Standard §7, §12)
// ---------------------------------------------------------------------------

/// The project-relative module tree root.
pub const COOK_MODULES_DIR: &str = "cook_modules";

/// LuaRocks' pure-Lua install subtree under [`COOK_MODULES_DIR`].
pub const MODULES_SHARE_LUA_SUBDIR: &str = "share/lua/5.4";

/// LuaRocks' C-extension install subtree under [`COOK_MODULES_DIR`].
pub const MODULES_LIB_LUA_SUBDIR: &str = "lib/lua/5.4";

/// `package` stash key holding the VM's pre-cook `package.path`, so
/// repeated prepends are idempotent (both phases use the same key: a VM
/// that ran one phase's mutation must not double-prepend under the other's).
pub const PACKAGE_PATH_STASH_KEY: &str = "_cook_original_path";

/// `package` stash key holding the VM's pre-cook `package.cpath`.
pub const PACKAGE_CPATH_STASH_KEY: &str = "_cook_original_cpath";

/// Native-extension file extension Lua's loader expects on this platform:
/// `dll` on Windows, `so` elsewhere (Lua's convention; LuaRocks emits `.so`
/// on macOS too).
pub fn native_lua_ext() -> &'static str {
    if cfg!(target_os = "windows") {
        "dll"
    } else {
        "so"
    }
}

/// The module tree root for a Cookfile working directory.
pub fn modules_dir(working_dir: &Path) -> PathBuf {
    working_dir.join(COOK_MODULES_DIR)
}

/// The §7 / CS-0069 four-candidate resolution order for
/// `cook.load_module(name)`: hand-vendored wins over LuaRocks-installed.
/// BOTH phases must probe exactly this list in exactly this order.
pub fn module_candidates(working_dir: &Path, name: &str) -> [PathBuf; 4] {
    let modules = modules_dir(working_dir);
    let share = modules.join(MODULES_SHARE_LUA_SUBDIR);
    [
        modules.join(format!("{}.lua", name)),
        modules.join(name).join("init.lua"),
        share.join(format!("{}.lua", name)),
        share.join(name).join("init.lua"),
    ]
}

/// The candidate list as the diagnostic renders it (`tried …`), shared so
/// the two phases' module-not-found errors describe the same probe order.
pub fn module_candidates_description(name: &str) -> String {
    format!(
        "{name}.lua, {name}/init.lua, {share}/{name}.lua, {share}/{name}/init.lua",
        share = MODULES_SHARE_LUA_SUBDIR
    )
}

/// A composed `package.path` / `package.cpath` pair.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LuaSearchPaths {
    pub path: String,
    pub cpath: String,
}

/// Compose the `package.path` / `package.cpath` values that make
/// sub-requires within a multi-file rock resolve against `cook_modules/`:
///
/// ```text
/// path:  <wd>/cook_modules/?.lua ; <wd>/cook_modules/?/init.lua ;
///        <wd>/cook_modules/share/lua/5.4/?.lua ;
///        <wd>/cook_modules/share/lua/5.4/?/init.lua ; <original>
/// cpath: <wd>/cook_modules/?.<ext> ;
///        <wd>/cook_modules/lib/lua/5.4/?.<ext> ; <original>
/// ```
///
/// The caller stashes the originals under [`PACKAGE_PATH_STASH_KEY`] /
/// [`PACKAGE_CPATH_STASH_KEY`] on first mutation so refresh is idempotent.
pub fn compose_lua_search_paths(
    working_dir: &Path,
    original_path: &str,
    original_cpath: &str,
) -> LuaSearchPaths {
    let cm = modules_dir(working_dir).display().to_string();
    let ext = native_lua_ext();
    LuaSearchPaths {
        path: format!(
            "{cm}/?.lua;{cm}/?/init.lua;\
             {cm}/{share}/?.lua;{cm}/{share}/?/init.lua;\
             {original_path}",
            share = MODULES_SHARE_LUA_SUBDIR
        ),
        cpath: format!(
            "{cm}/?.{ext};{cm}/{lib}/?.{ext};{original_cpath}",
            lib = MODULES_LIB_LUA_SUBDIR
        ),
    }
}

// ---------------------------------------------------------------------------
// .cook/ — the project state tree
// ---------------------------------------------------------------------------

/// The project state directory.
pub const DOT_COOK_DIR: &str = ".cook";

/// `.cook/cache` — the per-recipe step-index tree (`*.idx`).
pub fn cache_dir(base: &Path) -> PathBuf {
    base.join(DOT_COOK_DIR).join("cache")
}

/// `.cook/probes` — materialised probe values.
pub fn probes_dir(base: &Path) -> PathBuf {
    base.join(DOT_COOK_DIR).join("probes")
}

/// `.cook/logs` — the run-log store.
pub fn logs_dir(base: &Path) -> PathBuf {
    base.join(DOT_COOK_DIR).join("logs")
}

// ---------------------------------------------------------------------------
// .idx basename percent-encoding
// ---------------------------------------------------------------------------
//
// A recipe name may contain `/` (import-qualified pnpm task names like
// `@cap/env:build`), and a raw join would address a directory that never
// exists. Only `%` (the escape itself) and `/` are encoded, so every name
// without them keeps its historical file name. The encoder lived private
// in cook-cache while cook-engine hand-wrote the inverse (COOK-393).

/// Encode a recipe name into its `.idx` file basename (without extension).
pub fn encode_index_basename(recipe_name: &str) -> String {
    recipe_name.replace('%', "%25").replace('/', "%2F")
}

/// Decode an `.idx` file basename (without extension) back to the recipe
/// name. The inverse of [`encode_index_basename`]; `%2F` before `%25` so a
/// literal `%2F` in the original name survives the round trip.
pub fn decode_index_basename(encoded: &str) -> String {
    encoded.replace("%2F", "/").replace("%25", "%")
}

#[cfg(test)]
#[path = "tests/layout_tests.rs"]
mod tests;
