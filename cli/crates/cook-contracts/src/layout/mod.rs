//! Filesystem layout law: the `.cook/` tree, including the installed-module
//! tree at `.cook/modules/` (COOK-393, CS-0207).
//!
//! Module resolution — the tree root, the LuaRocks share/lib subtrees, the
//! §12 candidate probe order, the `package.path`/`package.cpath`
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
// .cook/modules/ — the installed-module tree (Standard §12, §27.1.1)
// ---------------------------------------------------------------------------

/// The installed-module tree, under [`DOT_COOK_DIR`] (§27.1.1, CS-0207).
///
/// It holds only what `cook modules install` put there. Before CS-0207 this
/// was a top-level `cook_modules/`, which was ALSO the only legal home for a
/// hand-authored module — one directory holding source and build output at
/// once, which is why no ignore rule for it could be correct (COOK-412,
/// COOK-430). A module the project authors is now addressed by path
/// ([`normalise_use_path`]) and lives wherever the author put it.
pub const MODULES_SUBDIR: &str = "modules";

/// The pre-CS-0207 tree root, retained for ONE purpose: recognising a stale
/// checkout so [`module_not_found_message`] can name the migration.
///
/// It is never searched. A dual-read window was refused because it would have
/// to keep `cook_modules/?.lua` in the search path for its whole life, and
/// that path IS the conflation CS-0207 exists to delete — the window would
/// ship the defect it is a window for. What a window buys is that a user with
/// a stale tree is told what happened; that is bought here instead, and a
/// stale tree can be reported but never silently loaded.
pub const LEGACY_MODULES_DIR: &str = "cook_modules";

/// LuaRocks' pure-Lua install subtree under [`modules_dir`].
pub const MODULES_SHARE_LUA_SUBDIR: &str = "share/lua/5.4";

/// LuaRocks' C-extension install subtree under [`modules_dir`].
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

/// The installed-module tree root for a Cookfile working directory:
/// `<working_dir>/.cook/modules` (§27.1.1, CS-0207).
pub fn modules_dir(working_dir: &Path) -> PathBuf {
    working_dir.join(DOT_COOK_DIR).join(MODULES_SUBDIR)
}

/// The pre-CS-0207 tree root, for [`module_not_found_message`]'s migration
/// hint only. Nothing resolves against it.
pub fn legacy_modules_dir(working_dir: &Path) -> PathBuf {
    working_dir.join(LEGACY_MODULES_DIR)
}

/// The §12 / CS-0069 resolution order for `cook.load_module(name)`. BOTH
/// phases must probe exactly this list in exactly this order.
///
/// Two candidates, not four. CS-0069's list led with `cook_modules/<name>.lua`
/// and `cook_modules/<name>/init.lua` — the hand-vendored top level, which
/// existed so a project could shadow a rock by dropping a file beside it.
/// CS-0207 withdrew that workflow rather than relocating it: shadowing by
/// precedence leaves no record in the Cookfile that it happened, so a reader
/// saw `use cook_cc` and had to know the search order and inspect the tree to
/// learn which `cook_cc` ran. An author who needs a patched module now points
/// a path-form `use` at their own file, which names the file it means. With
/// the top level gone there is no precedence question left to specify.
pub fn module_candidates(working_dir: &Path, name: &str) -> [PathBuf; 2] {
    let share = modules_dir(working_dir).join(MODULES_SHARE_LUA_SUBDIR);
    [
        share.join(format!("{}.lua", name)),
        share.join(name).join("init.lua"),
    ]
}

/// The candidate list as the diagnostic renders it (`tried …`), shared so
/// the two phases' module-not-found errors describe the same probe order.
pub fn module_candidates_description(name: &str) -> String {
    format!(
        "{share}/{name}.lua, {share}/{name}/init.lua",
        share = MODULES_SHARE_LUA_SUBDIR
    )
}

// ---------------------------------------------------------------------------
// Path-addressed modules (§12.5, §12.2.1 — CS-0206)
// ---------------------------------------------------------------------------
//
// A path form's target is confined to the declaring Cookfile's own subtree.
// The rule is here, not in the parser, because the parser is not the only
// door: `cook.load_module` takes a string a body computed, so a check that
// lived only at parse time would constrain spellings rather than the system.

/// Why a `use` path is not admissible (§12.2.1). Ordered as the checks run,
/// most-specific first, so a `//` sigil reports as a sigil rather than as the
/// absolute path it also is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UsePathRejection {
    /// `//build/helpers.lua` — `import`'s workspace-root sigil, which `use`
    /// does not admit.
    Sigil,
    /// `/opt/cook/helpers.lua` — absolute.
    Absolute,
    /// `../shared/helpers.lua` — escapes the Cookfile's subtree.
    DotDotSegment,
    /// `./my helpers.lua` — whitespace inside a path.
    ///
    /// App. A.5's `path_segment` is `/[^\s\n\/]+/`, which admits none, and
    /// `tree-sitter-cook` matches the Standard. The Rust lexer's argument
    /// splitter honours the quoted form so that a path CAN be quoted, and
    /// without this rule that leniency would let `use h "./my helpers.lua"`
    /// parse in one implementation and fail in the other — a Cookfile whose
    /// meaning two conforming readers disagree about, which is worse than
    /// either answer (COOK-431 review).
    Whitespace,
    /// The path is empty, or every segment was elided by normalisation.
    ///
    /// Not reachable from either door: everything that gets here has already
    /// passed `module_binding::is_path_target`, so it ends in `.lua` and has a
    /// non-empty final segment that is neither `.` nor `..`. Kept because this
    /// function is public law and its contract should not depend on who calls
    /// it, and tested directly.
    Empty,
}

impl UsePathRejection {
    /// The clause of the diagnostic that says what is wrong and what to do.
    /// One text, so the parse-time refusal and the resolver's refusal cannot
    /// tell an author two different stories about one rule.
    pub fn reason(self) -> &'static str {
        match self {
            UsePathRejection::Sigil => {
                "the '//' workspace-root sigil is not permitted in a `use` path; \
                 a module is confined to the declaring Cookfile's own subtree (§12.2.1)"
            }
            UsePathRejection::Absolute => {
                "absolute paths are not permitted in a `use` path; \
                 write it relative to the Cookfile's directory"
            }
            UsePathRejection::DotDotSegment => {
                "'..' segments are not permitted in a `use` path; \
                 a module is confined to the declaring Cookfile's own subtree (§12.2.1)"
            }
            UsePathRejection::Whitespace => {
                "whitespace is not permitted in a `use` path (App. A.5 `path_segment`); \
                 quoting does not admit it"
            }
            UsePathRejection::Empty => "a `use` path names no file",
        }
    }
}

/// The §12.2.1 refusal diagnostic, named after the path it refuses.
pub fn use_path_rejected_message(raw: &str, rejection: UsePathRejection) -> String {
    format!("use path '{}': {}", raw, rejection.reason())
}

/// Normalise a `use` path to the single spelling it is keyed and recorded
/// under, or say why it is not admissible (§12.2.1).
///
/// Normalisation drops a leading `./`, interior `.` segments, and repeated
/// separators, so `./build/helpers.lua`, `build/./helpers.lua` and
/// `build//helpers.lua` all key as `build/helpers.lua`. §12.3.2 requires that:
/// two spellings of one file must be one module instance, not two.
///
/// It does NOT touch symlinks or the filesystem — it is pure, and runs in the
/// parser where there is no working directory yet. The escape a symlink can
/// still perform is caught at resolution by [`contains_resolved_module_path`].
pub fn normalise_use_path(raw: &str) -> Result<String, UsePathRejection> {
    if raw.chars().any(char::is_whitespace) {
        return Err(UsePathRejection::Whitespace);
    }
    if raw.starts_with("//") {
        return Err(UsePathRejection::Sigil);
    }
    if raw.starts_with('/') {
        return Err(UsePathRejection::Absolute);
    }
    let mut segments: Vec<&str> = Vec::new();
    for segment in raw.split('/') {
        match segment {
            "" | "." => continue,
            ".." => return Err(UsePathRejection::DotDotSegment),
            other => segments.push(other),
        }
    }
    if segments.is_empty() {
        return Err(UsePathRejection::Empty);
    }
    Ok(segments.join("/"))
}

/// The file a normalised `use` path names, under a Cookfile's working
/// directory.
pub fn module_path_candidate(working_dir: &Path, normalised: &str) -> PathBuf {
    working_dir.join(normalised)
}

/// Does `resolved` lie inside `working_dir` once both are canonicalised?
///
/// The last of the three containment checks, and the only one that touches the
/// filesystem: `normalise_use_path` cannot see that `build/helpers.lua` is a
/// symlink to `../../elsewhere/helpers.lua`. It matters for the same reason
/// the other two do — [`relative_module_paths`] drops a determinant it cannot
/// express relative to the working directory, so a module reached across the
/// boundary would be run and not keyed.
///
/// A path that cannot be canonicalised (it does not exist yet, or a component
/// is unreadable) is reported as NOT contained: the caller's next step is a
/// not-found or refusal diagnostic either way, and answering "contained" for a
/// path nothing could resolve would be the permissive direction on a rule
/// whose failure mode is a silent wrong cache.
pub fn contains_resolved_module_path(working_dir: &Path, resolved: &Path) -> bool {
    match (working_dir.canonicalize(), resolved.canonicalize()) {
        (Ok(root), Ok(target)) => target.starts_with(&root),
        _ => false,
    }
}

/// The §12.2.1 refusal for a path that normalised cleanly but resolved outside
/// the Cookfile's subtree — in practice, through a symbolic link.
pub fn module_path_escaped_message(working_dir: &Path, raw: &str, resolved: &Path) -> String {
    format!(
        "cook.load_module: module path '{}' resolves to {}, outside the Cookfile's directory {} \
         (§12.2.1); a module loaded from outside cannot be recorded as a determinant of the unit \
         that loaded it, so it is refused rather than silently left out of the cache key",
        raw,
        resolved.display(),
        working_dir.display()
    )
}

/// The resolution failure for a path form. Distinct from
/// [`module_not_found_message`] because there is no candidate list to report:
/// the declaration named one file, and it is not there.
pub fn module_path_not_found_message(working_dir: &Path, raw: &str, candidate: &Path) -> String {
    format!(
        "cook.load_module: module file '{}' not found at {} (relative to {})",
        raw,
        candidate.display(),
        working_dir.display()
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

/// The memoisation / in-flight key for a module load: `(working_dir, target)`,
/// per §12.3.2–12.3.3. The register VM has one working_dir for its lifetime,
/// so the prefix is constant there; a worker VM is reused across Cookfiles
/// (CS-0017) and the prefix is what keeps two Cookfiles' same-named modules
/// distinct.
///
/// `target` is a module name or a NORMALISED path (CS-0206). The two share one
/// key space and cannot collide in it: a name is a Lua identifier and admits
/// no `.`, a path ends in `.lua`. Passing a raw path here rather than
/// [`normalise_use_path`]'s output would key `./x.lua` and `x.lua` separately
/// and evaluate one file twice, against §12.3.2.
pub fn module_memo_key(working_dir: &Path, target: &str) -> String {
    format!("{}::{}", working_dir.display(), target)
}

/// Render observed module paths for recording (§{exec.cache.module-source},
/// CS-0204): workspace-relative, sorted, deduplicated, and **confined to the
/// project**.
///
/// # Why relative
///
/// A recorded module path is re-hashed later — on the next run, and on another
/// machine that fetches the entry — by joining it onto that reader's working
/// directory. An absolute `/home/alice/proj/.cook/modules/x.lua` re-hashes to
/// nothing on Bob's machine, so every shared entry would degrade to a cold
/// miss.
///
/// # Why a path outside the project is DROPPED, not kept absolute
///
/// `cook.load_module` cannot resolve outside `<working_dir>`, but
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
///
/// CS-0207 gives it two conditional clauses, and they are the reason a hard
/// cut was affordable without a deprecation window. A window would have had to
/// keep the old root in the search path for its whole life, which is the
/// conflation the cut exists to delete; what a window actually buys is that a
/// user is TOLD, and that is bought here. A stale `cook_modules/` in the
/// working directory is recognised and named, and so is the `rm -rf .cook`
/// habit that now also removes installed rocks — the one cost of putting the
/// tree under the build-output root.
pub fn module_not_found_message(working_dir: &Path, name: &str) -> String {
    let mut msg = format!(
        "cook.load_module: module '{}' not found under {} (tried {})",
        name,
        modules_dir(working_dir).display(),
        module_candidates_description(name)
    );
    let legacy = legacy_modules_dir(working_dir);
    if legacy.is_dir() {
        msg.push_str(&format!(
            "\n  note: {} still exists. Installed modules moved to {} in CS-0207; \
             run `cook modules install` to repopulate it. A module you WROTE is no \
             longer resolved by name from there — move it where it belongs in the \
             repo and `use ./path/to/it.lua`.",
            legacy.display(),
            modules_dir(working_dir).display()
        ));
    } else if !modules_dir(working_dir).exists() {
        msg.push_str(
            "\n  note: no module tree here. If you removed `.cook/` it took the \
             installed modules with it; `cook modules install` rebuilds them from \
             cook.toml + cook.lock.",
        );
    }
    msg
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
/// sub-requires within a multi-file rock resolve against `.cook/modules/`:
///
/// ```text
/// path:  <wd>/.cook/modules/share/lua/5.4/?.lua ;
///        <wd>/.cook/modules/share/lua/5.4/?/init.lua ; <original>
/// cpath: <wd>/.cook/modules/lib/lua/5.4/?.<ext> ; <original>
/// ```
///
/// The top-level `?.lua` / `?.<ext>` entries that used to lead each list went
/// with the hand-vendored candidates they served (CS-0207); these mirror
/// [`module_candidates`] and MUST stay in step with it, since a name that
/// resolves through one and not the other is exactly the "works at register,
/// missing at execute" failure this module exists to prevent.
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
            "{cm}/{share}/?.lua;{cm}/{share}/?/init.lua;{original_path}",
            share = MODULES_SHARE_LUA_SUBDIR
        ),
        cpath: format!(
            "{cm}/{lib}/?.{ext};{original_cpath}",
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
