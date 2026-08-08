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

// ---------------------------------------------------------------------------
// Module-load laws (§12.3, §24.2)
// ---------------------------------------------------------------------------
//
// The `cook.load_module` sequence runs on both the register VM and every
// worker VM. The mechanics live in `cook-lua-stdlib` (they need mlua); the
// decisions inside them — how a load is keyed, what its diagnostics say,
// what chunk name an eval runs under — are law, and they live here.

/// The memoisation / in-flight key for a module load: `(working_dir, name)`,
/// per §12.3.2–12.3.3. The register VM has one working_dir for its lifetime,
/// so the prefix is constant there; a worker VM is reused across Cookfiles
/// (CS-0017) and the prefix is what keeps two Cookfiles' same-named modules
/// distinct.
pub fn module_memo_key(working_dir: &Path, name: &str) -> String {
    format!("{}::{}", working_dir.display(), name)
}

/// Render observed module paths for recording (§{exec.cache.module-source},
/// CS-0204): workspace-relative, sorted, deduplicated, and **confined to the
/// project**.
///
/// # Why relative
///
/// A recorded module path is re-hashed later — on the next run, and on another
/// machine that fetches the entry — by joining it onto that reader's working
/// directory. An absolute `/home/alice/proj/cook_modules/x.lua` re-hashes to
/// nothing on Bob's machine, so every shared entry would degrade to a cold
/// miss.
///
/// # Why a path outside the project is DROPPED, not kept absolute
///
/// `cook.load_module` cannot resolve outside `<working_dir>/cook_modules`, but
/// Lua's `require` searches the composed `package.path`, whose tail is the
/// interpreter's own — a bundled rock, a system Lua tree, whatever the host
/// happens to have. Those are not the project's source; they are the toolchain
/// the project ran on, and §{exec.cache.single-key} is explicit that the engine
/// infers no toolchain or machine identity of its own. An author who wants the
/// toolchain in a key declares it as a probe and seals on it.
///
/// Keeping them would also be self-defeating in exactly the direction this
/// change exists to protect: an absolute host path folded into a
/// content-addressed key makes the entry unreconstructable anywhere else, and
/// moves the key whenever cook itself is reinstalled elsewhere.
pub fn relative_module_paths(working_dir: &Path, loaded: &[std::path::PathBuf]) -> Vec<String> {
    let mut out: Vec<String> = loaded
        .iter()
        .filter_map(|p| {
            p.strip_prefix(working_dir)
                .ok()
                .map(|rel| rel.to_string_lossy().into_owned())
        })
        .collect();
    out.sort();
    out.dedup();
    out
}

/// The §12.3.3 cycle diagnostic: `module cycle detected:` followed by the
/// in-flight module names joined by ` -> `, with the re-entered name
/// appended.
pub fn module_cycle_message(loading_stack: &[String], reentered: &str) -> String {
    let mut path = loading_stack.join(" -> ");
    if !path.is_empty() {
        path.push_str(" -> ");
    }
    path.push_str(reentered);
    format!("module cycle detected: {path}")
}

/// The §24.2 resolution-failure diagnostic: identifies the name and the
/// paths that were probed. One text for both phases (it was two).
pub fn module_not_found_message(working_dir: &Path, name: &str) -> String {
    format!(
        "cook.load_module: module '{}' not found under {} (tried {})",
        name,
        modules_dir(working_dir).display(),
        module_candidates_description(name)
    )
}

/// The diagnostic for a candidate that resolved but could not be read.
pub fn module_read_failed_message(name: &str, path: &Path, err: &str) -> String {
    format!(
        "cook.load_module: failed to read module '{}' at {}: {}",
        name,
        path.display(),
        err
    )
}

/// The chunk name a module's top-level chunk is loaded under: `@<path>`,
/// so Lua tracebacks point at the module file itself.
pub fn module_chunk_name(module_path: &Path) -> String {
    format!("@{}", module_path.display())
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
