//! CS-0101: `$<file:PATH>` resolution for the register phase.
//!
//! Two consumers share `resolve_file_ref`:
//!   * `cook.file_ref(pattern)` — the hoisted substitution call emitted by
//!     cook-luagen; returns the space-joined match list.
//!   * `cook.add_unit`'s `file_refs` field — resolves each pattern and
//!     appends the matches to the unit's `cache_input_paths`, which flows
//!     into `CacheMeta.input_paths` and from there into the existing
//!     content-hash invalidation (cook-fingerprint check_inputs).
//!
//! Resolution base: the declaring Cookfile's directory — the same
//! `working_dir` that `cook.resolve_ingredients` and the cache layer use.

use mlua::prelude::*;
use std::path::Path;

use cook_fingerprint::{has_glob_meta, resolve_glob};

use crate::RegisterError;

/// Resolve one `$<file:PATTERN>` pattern against `working_dir`.
///
/// * Literal paths (no glob metacharacters) MUST name an existing file.
/// * Glob patterns MUST match at least one file; matches come back sorted
///   (CS-0085 glob semantics via `cook_fingerprint::resolve_glob`, which
///   already drops directory matches per CS-0064).
/// * Absolute paths and `..` segments are rejected — file references are
///   project-relative inputs (CS-0101).
pub fn resolve_file_ref(working_dir: &Path, pattern: &str) -> Result<Vec<String>, String> {
    if pattern.starts_with('/') || pattern.split('/').any(|s| s == "..") {
        return Err(format!(
            "$<file:{pattern}>: file reference paths must be relative and must not contain '..' (CS-0101)"
        ));
    }
    if has_glob_meta(pattern) {
        let matches = resolve_glob(working_dir, pattern);
        if matches.is_empty() {
            return Err(format!(
                "$<file:{pattern}>: glob matched no files (CS-0101)"
            ));
        }
        Ok(matches.into_iter().collect()) // BTreeSet → sorted
    } else if working_dir.join(pattern).is_file() {
        Ok(vec![pattern.to_string()])
    } else {
        Err(format!("$<file:{pattern}>: file not found (CS-0101)"))
    }
}

/// Register `cook.file_ref(pattern)` on the cook global table.
///
/// Returns the space-joined, sorted match list — the value the hoisted
/// `local _cook_fr_T_N = cook.file_ref("PATTERN")` locals emitted by
/// cook-luagen splice into command strings (CS-0101).
pub fn register_file_ref(lua: &Lua, working_dir: &Path) -> Result<(), RegisterError> {
    let cook: LuaTable = lua.globals().get("cook")?;
    let wd = working_dir.to_path_buf();
    let f = lua.create_function(move |_, pattern: String| {
        resolve_file_ref(&wd, &pattern)
            .map(|paths| paths.join(" "))
            .map_err(mlua::Error::runtime)
    })?;
    cook.set("file_ref", f)?;
    Ok(())
}
