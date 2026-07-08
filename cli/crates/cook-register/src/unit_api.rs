use mlua::prelude::*;
use std::path::{Path, PathBuf};
use cook_contracts::{CacheMeta, CapturedUnit, DepKind, WorkPayload};

use crate::dep_output_api::SharedTerminalOutputs;
use crate::{hash_str, SharedBodySlot};

/// Validate that a path string supplied as a `cook.add_unit` input does not
/// resolve to a directory. Cook's cache hashing layer reads files, not
/// directories — silently accepting a directory produces an empty cache
/// record and the unit re-runs every invocation. Reject at register time
/// with a clear, actionable diagnostic.
///
/// Inputs MUST exist (per add_unit semantics — the input contributes to the
/// cache key), so a non-existent path is also rejected here.
fn validate_input_not_directory(working_dir: &Path, path: &str) -> Result<(), String> {
    let resolved: PathBuf = if Path::new(path).is_absolute() {
        PathBuf::from(path)
    } else {
        working_dir.join(path)
    };
    let meta = match std::fs::symlink_metadata(&resolved) {
        Ok(m) => m,
        Err(_) => {
            // Don't error here on missing inputs — other layers (cache
            // record_completion, _cook_in iteration) already produce focused
            // diagnostics for missing files. We only reject directories.
            return Ok(());
        }
    };
    // Resolve symlinks to a concrete file type so a symlink-to-directory is
    // also rejected.
    let final_meta = if meta.file_type().is_symlink() {
        match std::fs::metadata(&resolved) {
            Ok(m) => m,
            Err(_) => return Ok(()),
        }
    } else {
        meta
    };
    if final_meta.is_dir() {
        return Err(format!(
            "cook.add_unit: input '{path}' is a directory; cook does not support directory inputs (use a glob like 'dir/*' or list specific files)"
        ));
    }
    Ok(())
}

/// Validate that a path string supplied as a `cook.add_unit` output does not
/// already exist as a directory. Output paths are typically not yet created
/// at register time, so a missing path is fine; what we reject is the case
/// where the path is occupied by a directory (which the cache cannot hash).
fn validate_output_not_directory(working_dir: &Path, path: &str) -> Result<(), String> {
    let resolved: PathBuf = if Path::new(path).is_absolute() {
        PathBuf::from(path)
    } else {
        working_dir.join(path)
    };
    let meta = match std::fs::symlink_metadata(&resolved) {
        Ok(m) => m,
        Err(_) => return Ok(()),
    };
    let final_meta = if meta.file_type().is_symlink() {
        match std::fs::metadata(&resolved) {
            Ok(m) => m,
            Err(_) => return Ok(()),
        }
    } else {
        meta
    };
    if final_meta.is_dir() {
        return Err(format!(
            "cook.add_unit: output '{path}' is a directory; cook does not support directory outputs (declare a specific file path)"
        ));
    }
    Ok(())
}

/// Uniform register-phase type error for a `cook.add_unit` field (CS-0127):
/// a wrong-typed field is a hard error naming the field, the expected type,
/// and the received Lua type — never silently coerced to its default. This
/// is the shared message-shape for every field-typing check in `add_unit`
/// (mirrors the `command` precedent landed under CS-0122).
fn type_err(field: &str, expected: &str, got: &str) -> LuaError {
    LuaError::runtime(format!(
        "cook.add_unit: `{field}` must be {expected}, got {got} (Standard \u{00a7}22.1, CS-0127)"
    ))
}

/// Strictly collect a Lua sequence table into `Vec<String>` for a
/// `cook.add_unit` field, erroring (naming `field`) on the first non-string
/// element rather than silently dropping it. Replaces the old
/// `sequence_values::<String>().filter_map(Result::ok)` pattern, which both
/// silently dropped non-string elements and — via mlua's implicit
/// number-to-string coercion in its `String: FromLua` impl — would have
/// silently coerced numeric elements into strings had they not been dropped
/// by a prior `Result` error (CS-0127).
fn collect_string_list(t: &LuaTable, field: &str) -> Result<Vec<String>, LuaError> {
    let mut out = Vec::new();
    for v in t.sequence_values::<LuaValue>() {
        let v = v.map_err(|e| LuaError::runtime(format!("cook.add_unit: `{field}`: {e}")))?;
        match v {
            LuaValue::String(s) => out.push(s.to_string_lossy().to_string()),
            other => return Err(type_err(field, "a table of strings", other.type_name())),
        }
    }
    Ok(out)
}

/// Escape a string for embedding in a Lua double-quoted string literal.
fn lua_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\0' => out.push_str("\\0"),
            c => out.push(c),
        }
    }
    out
}

/// CS-0074: If `command` contains `$<key:...>` probe-value sigils, rewrite it
/// into a Lua chunk string that performs the substitution using `cook.cache.get`
/// at execute time and invokes `cook.sh` with the fully-resolved command.
///
/// Detection uses the same `cook_luagen::sigil::scan` scanner that codegen uses,
/// ensuring one source of truth for the probe-sigil grammar.
///
/// Returns `Some(lua_code_string, keys)` when probe sigils are detected, `None`
/// when the command is plain (no rewriting needed).
///
/// Also returns the distinct set of probe keys so callers can merge them into
/// `unit.probes` automatically.
fn try_expand_probe_templates(command: &str) -> Result<Option<(String, Vec<String>)>, String> {
    let spans = cook_luagen::sigil::scan(command);

    // CS-0101: `file:` is a reserved placeholder namespace, not a probe key.
    // Raw register-block add_unit command strings do not support file refs
    // in v1 — fail loudly rather than misparse `file:x` as a probe key or
    // silently pass the bytes through.
    if let Some(span) = spans.iter().find(|s| s.ident.starts_with("file:")) {
        return Err(format!(
            "$<{}>: $<file:PATH> is not supported in raw cook.add_unit command strings; \
             write the step in a Cookfile recipe body instead",
            span.ident
        ));
    }

    // Filter to probe-shaped sigils (ident contains `:`), excluding the
    // reserved `file:` namespace (rejected above — belt and braces).
    let probe_spans: Vec<_> = spans
        .iter()
        .filter(|s| s.ident.contains(':') && !s.ident.starts_with("file:"))
        .collect();
    if probe_spans.is_empty() {
        return Ok(None);
    }

    // Collect distinct probe keys in order of first appearance.
    let mut seen_keys = std::collections::BTreeSet::new();
    let mut keys: Vec<String> = vec![];
    for span in &probe_spans {
        // Key is everything before the first `.` or `[` after the `:`.
        let colon = span.ident.find(':').unwrap();
        let after_colon = &span.ident[colon + 1..];
        let path_start = after_colon.find(|c: char| c == '.' || c == '[')
            .map(|p| colon + 1 + p)
            .unwrap_or(span.ident.len());
        let key = &span.ident[..path_start];
        if seen_keys.insert(key.to_string()) {
            keys.push(key.to_string());
        }
    }

    // Build the Lua access expression for a probe sigil ident.
    // Returns the `cook.cache.get("key").field[N]...` expression.
    let build_access = |ident: &str| -> String {
        let colon = ident.find(':').unwrap();
        let after_colon = &ident[colon + 1..];
        let path_start = after_colon.find(|c: char| c == '.' || c == '[')
            .map(|p| colon + 1 + p)
            .unwrap_or(ident.len());
        let key = &ident[..path_start];
        let path_str = &ident[path_start..];

        let mut access = format!("cook.cache.get(\"{}\")", lua_escape(key));
        let mut chars = path_str.chars().peekable();
        while let Some(&c) = chars.peek() {
            match c {
                '.' => {
                    chars.next();
                    let mut name = String::new();
                    while let Some(&nc) = chars.peek() {
                        if nc.is_alphanumeric() || nc == '_' { name.push(nc); chars.next(); }
                        else { break; }
                    }
                    if !name.is_empty() { access.push('.'); access.push_str(&name); }
                }
                '[' => {
                    chars.next();
                    let mut idx = String::new();
                    while let Some(&nc) = chars.peek() {
                        if nc == ']' { chars.next(); break; }
                        idx.push(nc); chars.next();
                    }
                    access.push('['); access.push_str(&idx); access.push(']');
                }
                _ => { chars.next(); }
            }
        }
        access
    };

    // Build the command as a Lua concatenation expression over all spans.
    let mut parts: Vec<String> = vec![];
    let mut cursor = 0usize;

    for span in &spans {
        if span.range.start > cursor {
            parts.push(format!("\"{}\"", lua_escape(&command[cursor..span.range.start])));
        }
        if span.ident.contains(':') {
            // Probe-value sigil → cache read.
            let access = build_access(&span.ident);
            parts.push(format!("tostring({})", access));
        } else {
            // Non-probe sigil in a register-block add_unit command — treat as
            // literal (the sigil text, including $<...>). These are unusual but
            // must not corrupt the Lua chunk.
            parts.push(format!("\"{}\"", lua_escape(&command[span.range.clone()])));
        }
        cursor = span.range.end;
    }
    if cursor < command.len() {
        parts.push(format!("\"{}\"", lua_escape(&command[cursor..])));
    }

    let concat_expr = if parts.len() == 1 {
        parts.into_iter().next().unwrap()
    } else {
        parts.join(" .. ")
    };
    let lua = format!("cook.sh({} )", concat_expr);

    Ok(Some((lua, keys)))
}

/// Register `cook.add_unit(table)`, `cook.step_group(fn)`, `cook._enter_chore()`,
/// and `cook._exit_chore()` on the cook table.
///
/// `working_dir` is the recipe's working directory; it's used to resolve
/// relative input/output paths for the directory-rejection check.
pub fn register_unit_api(
    lua: &Lua,
    body_slot: SharedBodySlot,
    recipe_name: &str,
    terminal_outputs: SharedTerminalOutputs,
    working_dir: PathBuf,
) -> LuaResult<()> {
    let cook: LuaTable = lua.globals().get("cook")?;

    // cook._enter_chore() — called by chore-generated Lua before the body runs.
    let body_slot_enter = body_slot.clone();
    let enter_fn = lua.create_function(move |_, ()| {
        let mut slot = body_slot_enter.borrow_mut();
        let body = slot.as_mut().ok_or_else(|| {
            mlua::Error::runtime("cook._enter_chore called outside a recipe body")
        })?;
        body.current_chore_active = true;
        Ok(())
    })?;
    cook.set("_enter_chore", enter_fn)?;

    // cook._exit_chore() — called by chore-generated Lua after the body runs.
    let body_slot_exit = body_slot.clone();
    let exit_fn = lua.create_function(move |_, ()| {
        let mut slot = body_slot_exit.borrow_mut();
        let body = slot.as_mut().ok_or_else(|| {
            mlua::Error::runtime("cook._exit_chore called outside a recipe body")
        })?;
        body.current_chore_active = false;
        Ok(())
    })?;
    cook.set("_exit_chore", exit_fn)?;

    // cook.add_unit(table)
    let body_slot_add = body_slot.clone();
    let rname = recipe_name.to_string();
    let wd_for_add_unit = working_dir.clone();
    // terminal_outputs is no longer consulted in add_unit; dep_output_api.rs
    // now accumulates importer-relative rewritten paths in
    // body.step_group_dep_input_paths so that cache_meta.input_paths
    // contains stat-able paths from the importer's working directory.
    let _ = terminal_outputs;
    let add_unit_fn = lua.create_function(move |lua, tbl: LuaTable| {
        // CS-0122: `command` must be a string (or absent for lua_code
        // units). The old `.unwrap_or_default()` coerced ANY non-string — most
        // damagingly the probe-deferred `function() ... end` closure old codegen
        // emitted — to "" and the empty command "succeeded" silently.
        let command: String = match tbl.get::<LuaValue>("command") {
            Ok(LuaValue::Nil) | Err(_) => String::new(),
            Ok(LuaValue::String(s)) => s.to_string_lossy().to_string(),
            Ok(other) => {
                return Err(LuaError::runtime(format!(
                    "cook.add_unit: `command` must be a string, got {}; \
                     function-valued (deferred) commands are not supported — \
                     probe values belong in the command string as $<key:field> \
                     placeholders (Standard \u{00a7}22.5.7, CS-0122)",
                    other.type_name()
                )));
            }
        };
        // CS-0127: `lua_code`, if present, must be a string — never coerced.
        let lua_code: Option<String> = match tbl.get::<LuaValue>("lua_code") {
            Ok(LuaValue::Nil) | Err(_) => None,
            Ok(LuaValue::String(s)) => Some(s.to_string_lossy().to_string()),
            Ok(other) => return Err(type_err("lua_code", "a string", other.type_name())),
        };
        // CS-0127: `interactive` must be a boolean — never coerced.
        let interactive: bool = match tbl.get::<LuaValue>("interactive") {
            Ok(LuaValue::Nil) | Err(_) => false,
            Ok(LuaValue::Boolean(b)) => b,
            Ok(other) => return Err(type_err("interactive", "a boolean", other.type_name())),
        };
        // CS-0127: `line` must be a non-negative integer — never coerced.
        let line: usize = match tbl.get::<LuaValue>("line") {
            Ok(LuaValue::Nil) | Err(_) => 0,
            Ok(LuaValue::Integer(n)) if n >= 0 => n as usize,
            Ok(other) => return Err(type_err("line", "a non-negative integer", other.type_name())),
        };
        // CS-0127: `cache` must be a boolean — never coerced.
        let cache_enabled: bool = match tbl.get::<LuaValue>("cache") {
            Ok(LuaValue::Nil) | Err(_) => true,
            Ok(LuaValue::Boolean(b)) => b,
            Ok(other) => return Err(type_err("cache", "a boolean", other.type_name())),
        };
        // CS-0045 / CS-0127: the originating step kind drives the
        // execute-phase sandbox policy on the resulting LuaChunk. Codegen
        // passes `step_kind = "plate"` for plate-step bodies (which are
        // unsandboxed by design) and omits the field for cook/test/
        // chore bodies. The captured-unit default is `cook` because
        // that is the strictest policy: a misclassified plate body
        // becomes a Lua runtime error rather than a silent escape. An
        // unrecognised string, or any non-string value, is a hard error
        // rather than a silent fall-through to the default.
        let step_kind: cook_contracts::StepKind = match tbl.get::<LuaValue>("step_kind") {
            Ok(LuaValue::Nil) | Err(_) => cook_contracts::StepKind::Cook,
            Ok(LuaValue::String(s)) => {
                let sv = s.to_string_lossy().to_string();
                match sv.as_str() {
                    "plate" => cook_contracts::StepKind::Plate,
                    "test" => cook_contracts::StepKind::Test,
                    "chore" => cook_contracts::StepKind::Chore,
                    "cook" => cook_contracts::StepKind::Cook,
                    _ => {
                        return Err(type_err(
                            "step_kind",
                            "one of \"cook\", \"plate\", \"test\", \"chore\"",
                            &format!("{sv:?}"),
                        ))
                    }
                }
            }
            Ok(other) => return Err(type_err("step_kind", "a string", other.type_name())),
        };

        // §{chores.no-caching}: cache = true is not permitted inside a chore body.
        {
            let slot = body_slot_add.borrow();
            let body = slot.as_ref().ok_or_else(|| {
                LuaError::runtime("cook.add_unit called outside a recipe body")
            })?;
            if cache_enabled && body.current_chore_active {
                return Err(LuaError::RuntimeError(
                    "cook.add_unit: cache = true is not permitted in a chore body \
                     (§{chores.no-caching}); chore units are never cached"
                        .into(),
                ));
            }
        }
        // CS-0127: `inputs` must be a table of strings — never coerced
        // (including mlua's implicit number-to-string coercion on elements).
        let inputs: Vec<String> = match tbl.get::<LuaValue>("inputs") {
            Ok(LuaValue::Nil) | Err(_) => vec![],
            Ok(LuaValue::Table(t)) => collect_string_list(&t, "inputs")?,
            Ok(other) => return Err(type_err("inputs", "a table of strings", other.type_name())),
        };
        // CS-0127: `output` must be a string — never coerced.
        let output: Option<String> = match tbl.get::<LuaValue>("output") {
            Ok(LuaValue::Nil) | Err(_) => None,
            Ok(LuaValue::String(s)) => Some(s.to_string_lossy().to_string()),
            Ok(other) => return Err(type_err("output", "a string", other.type_name())),
        };
        // CS-0127: `outputs` must be a table of strings — never coerced.
        let outputs: Option<Vec<String>> = match tbl.get::<LuaValue>("outputs") {
            Ok(LuaValue::Nil) | Err(_) => None,
            Ok(LuaValue::Table(t)) => Some(collect_string_list(&t, "outputs")?),
            Ok(other) => return Err(type_err("outputs", "a table of strings", other.type_name())),
        };
        // CS-0127: `ingredient_groups` must be a table of tables of
        // strings — strict at both levels, never coerced.
        let ingredient_groups: Vec<Vec<String>> = match tbl.get::<LuaValue>("ingredient_groups") {
            Ok(LuaValue::Nil) | Err(_) => Vec::new(),
            Ok(LuaValue::Table(outer)) => {
                let mut groups = Vec::new();
                for v in outer.sequence_values::<LuaValue>() {
                    let v = v.map_err(|e| {
                        LuaError::runtime(format!("cook.add_unit: `ingredient_groups`: {e}"))
                    })?;
                    match v {
                        LuaValue::Table(inner) => {
                            groups.push(collect_string_list(&inner, "ingredient_groups")?);
                        }
                        other => {
                            return Err(type_err(
                                "ingredient_groups",
                                "a table of tables of strings",
                                other.type_name(),
                            ))
                        }
                    }
                }
                groups
            }
            Ok(other) => {
                return Err(type_err(
                    "ingredient_groups",
                    "a table of tables of strings",
                    other.type_name(),
                ))
            }
        };
        if output.is_some() && outputs.is_some() {
            return Err(LuaError::RuntimeError(
                "cook.add_unit: only one of `output` or `outputs` may be provided".into(),
            ));
        }
        let output_paths: Vec<String> = if let Some(list) = outputs {
            list
        } else if let Some(single) = output {
            vec![single]
        } else {
            Vec::new()
        };
        let outputs_for_tracking = output_paths.clone();

        // Reject directory inputs/outputs at register time. Cook's cache
        // hashing layer reads files; silently accepting a directory
        // produces an empty cache record (only `_source_hash`) and the
        // unit re-runs every invocation. Catching it here gives the user
        // an actionable diagnostic instead of mysterious cache misses.
        for inp in &inputs {
            if let Err(msg) = validate_input_not_directory(&wd_for_add_unit, inp) {
                return Err(LuaError::RuntimeError(msg));
            }
        }
        for out in &output_paths {
            if cook_fingerprint::is_dir_output(out) {
                // CS-0119: a trailing slash declares a build-owned directory
                // output; it is intentionally a directory and MUST NOT be
                // rejected by the file-path check below.
                continue;
            }
            if let Err(msg) = validate_output_not_directory(&wd_for_add_unit, out) {
                return Err(LuaError::RuntimeError(msg));
            }
        }

        // 2026-05-02 addendum spec §4.3: cross-recipe dep refs accumulated by
        // cook.dep_output / cook.dep_output_list calls within this step_group
        // produce paths the command consumed via {NAME} substitution. Append
        // those paths to cache_meta.input_paths so cache invalidation tracks
        // dep-output content drift. Keep them out of WorkPayload inputs (which
        // drive _cook_in iteration / Lua-visible inputs).
        //
        // Use step_group_dep_input_paths (the importer-relative rewritten paths
        // accumulated by dep_output_api) rather than reading raw paths from
        // terminal_outputs. The raw paths are importee-relative and cannot be
        // stat'd from the importer's working directory — using them would cause
        // MissingFile errors in record_completion, silently dropping demo.bin.
        //
        // COOK-96: cook.dep_output_member records its member's upstream paths
        // into a SEPARATE per-unit buffer (pending_member_dep_input_paths)
        // rather than the step-group-wide accumulator, because a fan-out recipe
        // packs every member's unit into ONE step group. Drain that buffer here
        // and fold it into ONLY this unit's fingerprint so editing render's s1
        // output re-runs mux-s1 alone, not mux-s2. A single borrow_mut both
        // clones the step-group-wide paths and takes the pending per-member ones.
        let (dep_input_paths, member_dep_input_paths): (Vec<String>, Vec<String>) = {
            let mut slot = body_slot_add.borrow_mut();
            let body = slot.as_mut().ok_or_else(|| {
                LuaError::runtime("cook.add_unit called outside a recipe body")
            })?;
            (
                body.step_group_dep_input_paths.clone(),
                std::mem::take(&mut body.pending_member_dep_input_paths),
            )
        };
        // CS-0101: resolve `file_refs` patterns (file-reference placeholders)
        // and fold the matches into this unit's cache input set. Resolution
        // failures are load-time diagnostics (missing literal / empty glob).
        // Paths go into cache_meta.input_paths ONLY — never WorkPayload
        // inputs, which drive _cook_in iteration.
        // CS-0127: `file_refs` must be a table of strings — never coerced.
        let file_ref_patterns: Vec<String> = match tbl.get::<LuaValue>("file_refs") {
            Ok(LuaValue::Nil) | Err(_) => Vec::new(),
            Ok(LuaValue::Table(t)) => collect_string_list(&t, "file_refs")?,
            Ok(other) => return Err(type_err("file_refs", "a table of strings", other.type_name())),
        };
        let mut file_ref_paths: Vec<String> = Vec::new();
        for pat in &file_ref_patterns {
            let resolved = crate::file_ref::resolve_file_ref(&wd_for_add_unit, pat)
                .map_err(|e| LuaError::runtime(format!("cook.add_unit: {e}")))?;
            file_ref_paths.extend(resolved);
        }
        file_ref_paths.sort();
        file_ref_paths.dedup();

        let cache_input_paths: Vec<String> = inputs
            .iter()
            .cloned()
            .chain(dep_input_paths.into_iter())
            .chain(member_dep_input_paths.into_iter())
            .chain(file_ref_paths.into_iter())
            .collect();

        // Read consulted_env_keys from the table and look up values in cook.env
        // (the merged Cookfile-config + process env that the command actually
        // consumed at substitution time, per spec §4.3). Reading from
        // std::env::var would miss config-overlay values and capture process
        // env that the command never saw — both produce false misses.
        let mut consulted_env: std::collections::BTreeMap<String, String> =
            std::collections::BTreeMap::new();
        let env_table: Option<LuaTable> = lua
            .globals()
            .get::<LuaTable>("cook")
            .and_then(|c| c.get::<LuaTable>("env"))
            .ok();
        // CS-0127: `consulted_env_keys` must be a table of strings, or the
        // literal string "*" — never coerced, and element collection is
        // strict (a non-string element is a hard error, not a silent drop).
        match tbl.get::<LuaValue>("consulted_env_keys") {
            Ok(LuaValue::Nil) | Err(_) => {}
            Ok(LuaValue::Table(list)) => {
                let keys = collect_string_list(&list, "consulted_env_keys")?;
                if let Some(env) = &env_table {
                    for v in keys {
                        if let Ok(val) = env.get::<String>(v.clone()) {
                            consulted_env.insert(v, val);
                        }
                    }
                }
            }
            Ok(LuaValue::String(s)) if s.to_str().ok().as_deref() == Some("*") => {
                if let Some(env) = &env_table {
                    for pair in env.clone().pairs::<String, String>() {
                        if let Ok((k, v)) = pair {
                            consulted_env.insert(k, v);
                        }
                    }
                }
            }
            Ok(LuaValue::String(s)) => {
                return Err(type_err(
                    "consulted_env_keys",
                    "a table of strings or the string \"*\"",
                    &format!("the string {:?}", s.to_string_lossy()),
                ))
            }
            Ok(other) => {
                return Err(type_err(
                    "consulted_env_keys",
                    "a table of strings or the string \"*\"",
                    other.type_name(),
                ))
            }
        }

        // COOK-64 §8.3/§17.1: a `for_each` fan-out unit carries its data member
        // (canonical-rendered by `cook.member_to_string`). Fold it into the
        // command hash so each member's unit gets a distinct fingerprint —
        // editing one member re-runs only its unit (observable #5). NUL
        // delimiters keep the member byte-range disjoint from the command.
        // Shell bodies already bake the member into the command text; this
        // additionally covers Lua-block bodies whose `item` reads are opaque to
        // the command string. `None` (non-`for_each` units) hashes as before.
        // CS-0127: `member` must be a string — never coerced.
        let member: Option<String> = match tbl.get::<LuaValue>("member") {
            Ok(LuaValue::Nil) | Err(_) => None,
            Ok(LuaValue::String(s)) => Some(s.to_string_lossy().to_string()),
            Ok(other) => return Err(type_err("member", "a string", other.type_name())),
        };
        let hash_base: &str = lua_code.as_deref().unwrap_or(&command);
        let command_hash = match &member {
            Some(m) => hash_str(&format!("{hash_base}\u{0}member\u{0}{m}")),
            None => hash_str(hash_base),
        };

        // Retrieve the CacheContext if it was threaded in from cook-engine.
        // If absent (tests, legacy call sites where the engine has not yet
        // built its `CacheContext`), still compute env_contribution from the
        // captured consulted_env so a value change in any tracked env key
        // invalidates the cache. COOK-59 Task 4.5: without this, the static
        // Lua scanner for `cook.env.<KEY>` reads can record keys whose values
        // never reach the cache fingerprint — the very gap the scanner exists
        // to close.
        let cache_ctx = lua
            .app_data_ref::<std::sync::Arc<cook_cache::cache_ctx::CacheContext>>();

        let (env_contribution_val, project_id, cookfile_path) =
            if let Some(ctx) = cache_ctx {
                let ec = cook_cache::envkey::env_contribution(&consulted_env, &ctx.denylist);
                let pid = ctx.project_id.clone();
                let cfp = cookfile_relative_path(lua);
                (ec, pid, cfp)
            } else {
                let baseline = cook_cache::envkey::EnvDenylist::baseline();
                let ec = cook_cache::envkey::env_contribution(&consulted_env, &baseline);
                (ec, String::new(), cookfile_relative_path(lua))
            };

        // Read optional discovered_inputs table.
        let discovered_inputs: Option<cook_contracts::DiscoveredInputs> =
            match tbl.get::<LuaValue>("discovered_inputs") {
                Ok(LuaValue::Table(di_tbl)) => {
                    let from: String = di_tbl.get::<String>("from").map_err(|_| {
                        LuaError::RuntimeError(
                            "cook.add_unit: discovered_inputs.from is required and must be a string"
                                .into(),
                        )
                    })?;
                    let format: String = di_tbl.get::<String>("format").map_err(|_| {
                        LuaError::RuntimeError(
                            "cook.add_unit: discovered_inputs.format is required and must be a string"
                                .into(),
                        )
                    })?;
                    if from.is_empty() {
                        return Err(LuaError::RuntimeError(
                            "cook.add_unit: discovered_inputs.from must be non-empty".into(),
                        ));
                    }
                    if from.starts_with('/') {
                        return Err(LuaError::RuntimeError(format!(
                            "cook.add_unit: discovered_inputs.from must be a relative path; got absolute path {from:?}"
                        )));
                    }
                    if from.split('/').any(|seg| seg == "..") {
                        return Err(LuaError::RuntimeError(format!(
                            "cook.add_unit: discovered_inputs.from must not contain '..' segments; got {from:?}"
                        )));
                    }
                    if format != "make" {
                        return Err(LuaError::RuntimeError(format!(
                            "cook.add_unit: discovered_inputs.format = {format:?} is not supported by this implementation (supported: \"make\")"
                        )));
                    }
                    Some(cook_contracts::DiscoveredInputs { from, format })
                }
                Ok(LuaValue::Nil) | Err(_) => None,
                Ok(_) => {
                    return Err(LuaError::RuntimeError(
                        "cook.add_unit: discovered_inputs must be a table".into(),
                    ));
                }
            };

        // COOK-161: opts.seal — optional list of bare probe keys the author sealed
        // this unit on (§8.4.3). Their canonical VALUES fold into the cache key at
        // execute-phase; here we carry only the key set.
        let seal_keys: std::collections::BTreeSet<String> = match tbl.get::<LuaValue>("seal") {
            Ok(LuaValue::Nil) => Default::default(),
            Ok(LuaValue::Table(t)) => {
                let mut out = std::collections::BTreeSet::new();
                for v in t.sequence_values::<String>() {
                    out.insert(v.map_err(|e| {
                        LuaError::runtime(format!(
                            "cook.add_unit: seal must be a list of strings: {e}"
                        ))
                    })?);
                }
                out
            }
            _ => {
                return Err(LuaError::runtime(
                    "cook.add_unit: seal must be a table of strings".to_string(),
                ))
            }
        };

        // COOK-162 / I3: sharing disposition emitted by codegen as a string
        // field `sharing = "local"|"pinned"`, omitted for the shared
        // default. CS-0127: validate the string against the known set
        // BEFORE handing it to `Sharing::from_wire_str` (whose own
        // catch-all is relied on elsewhere to map absence/unknown to
        // `Shared` on the wire-decode path) — an unrecognised string or a
        // non-string value here is a hard error, not a silent default.
        let sharing = match tbl.get::<LuaValue>("sharing") {
            Ok(LuaValue::Nil) | Err(_) => cook_contracts::Sharing::Shared,
            Ok(LuaValue::String(s)) => {
                let sv = s.to_string_lossy().to_string();
                match sv.as_str() {
                    "local" | "pinned" | "shared" => cook_contracts::Sharing::from_wire_str(&sv),
                    _ => {
                        return Err(type_err(
                            "sharing",
                            "one of \"local\", \"pinned\", \"shared\"",
                            &format!("{sv:?}"),
                        ))
                    }
                }
            }
            Ok(other) => return Err(type_err("sharing", "a string", other.type_name())),
        };
        // COOK-163: opts.record — the `record` disposition. Marks an
        // intrinsically non-reproducible artifact; byte-equivalence is waived
        // at the cache decision (the key is unchanged). Accept only a bool.
        let record: bool = match tbl.get::<LuaValue>("record") {
            Ok(LuaValue::Nil) => false,
            Ok(LuaValue::Boolean(b)) => b,
            Ok(_) => {
                return Err(LuaError::runtime(
                    "cook.add_unit: record must be a boolean".to_string(),
                ))
            }
            Err(_) => false,
        };

        let cache_meta = if cache_enabled {
            let cache_key = build_local_cache_key(
                &cookfile_path,
                &rname,
                &output_paths,
                &cache_input_paths,
                command_hash,
                env_contribution_val,
            );
            Some(CacheMeta {
                recipe_name: rname.clone(),
                project_id,
                cookfile_path,
                cache_key,
                input_paths: cache_input_paths,
                output_paths: output_paths.clone(),
                command_hash,
                env_contribution: env_contribution_val,
                consulted_env,
                discovered_inputs,
                seal_keys: seal_keys.clone(),
                sharing,
                record,
            })
        } else {
            None
        };

        // Reject legacy `requires` field name (CS-0074 phase 2 rename).
        // The branch is unmerged and 0.11 is unreleased, so no compat shim.
        // Fire on any non-Nil value at `requires` (string, number, table, etc.)
        // so authors mid-migration don't silently slip past the guard.
        match tbl.get::<LuaValue>("requires") {
            Ok(LuaValue::Nil) | Err(_) => {}
            Ok(_) => {
                return Err(LuaError::runtime(
                    "cook.add_unit: field `requires` is no longer accepted for probe references; rename to `probes`".to_string(),
                ));
            }
        }

        // Parse opts.probes: optional list of probe-key strings (§{cat.probes.consumer}).
        let mut probes: Vec<String> = match tbl.get::<LuaValue>("probes") {
            Ok(LuaValue::Nil) => vec![],
            Ok(LuaValue::Table(t)) => {
                let mut out = vec![];
                for v in t.sequence_values::<String>() {
                    out.push(v.map_err(|e| {
                        LuaError::runtime(format!(
                            "cook.add_unit: probes must be a list of strings: {e}"
                        ))
                    })?);
                }
                out
            }
            Ok(_) => {
                return Err(LuaError::runtime(
                    "cook.add_unit: probes must be a list of strings (or nil)".to_string(),
                ));
            }
            Err(_) => vec![],
        };

        // Sealed probes are execute-phase determinants — the unit must run after
        // them so their values are materialised before the cache check (COOK-161).
        for k in &seal_keys {
            if !probes.contains(k) {
                probes.push(k.clone());
            }
        }

        // is_chore is read BEFORE the if/else below (and before the later
        // mutable borrow) so the borrow doesn't overlap with mutable use.
        let is_chore = {
            let slot = body_slot_add.borrow();
            let body = slot.as_ref().ok_or_else(|| {
                LuaError::runtime("cook.add_unit called outside a recipe body")
            })?;
            body.current_chore_active
        };
        // COOK-36 Task 4: when capturing a lua_code unit inside a chore body,
        // prepend the param-binding prelude so the execute-phase worker sees
        // the param locals resolved to their bound values.
        let chore_param_prelude: String = {
            let slot = body_slot_add.borrow();
            if let Some(body) = slot.as_ref() {
                body.chore_param_prelude.clone()
            } else {
                String::new()
            }
        };
        let payload = if let Some(code) = lua_code {
            let (final_code, chunk_line) = if !chore_param_prelude.is_empty() && is_chore {
                // The prelude is normally one `local NAME = "VALUE"\n` line
                // per bound chore param. Left as-is, N params would shift
                // the step's own code down by N lines within this same
                // `code` string — and pool.rs's padding can only ADD lines
                // ahead of `code`, never remove them, so once N reaches the
                // step's own (small) Cookfile line number there is no
                // non-negative padding that recovers the right answer
                // (verified empirically: a 2-param chore with its body on
                // Cookfile line 2 cannot be fixed by subtracting 2 from an
                // already-small line). Collapse the whole prelude onto a
                // SINGLE line (`; `-joined statements, one trailing
                // newline) so it always costs exactly one line regardless
                // of param count — then subtracting exactly 1 always lines
                // the step's own first line back up with its real Cookfile
                // line. The trailing newline (rather than gluing prelude
                // and code onto one shared line) also keeps this safe if
                // the step's code happens to start with a `--` comment.
                let prelude_single_line = format!(
                    "{}\n",
                    chore_param_prelude.trim_end_matches('\n').replace('\n', "; ")
                );
                (
                    format!("{prelude_single_line}{code}"),
                    line.saturating_sub(1),
                )
            } else {
                (code, line)
            };
            WorkPayload::LuaChunk {
                code: final_code,
                inputs,
                outputs: output_paths.clone(),
                ingredient_groups,
                step_kind,
                is_chore,
                line: chunk_line,
            }
        } else if interactive {
            WorkPayload::Interactive { cmd: command, line, is_chore }
        } else {
            // CS-0074: scan command for `$<key:field>` probe-value sigils.
            // If found, rewrite as a LuaChunk that resolves the values at
            // execute time via cook.cache.get and calls cook.sh. Also
            // auto-add the detected probe keys to probes.
            //
            // CS-0127: the rewritten LuaChunk carries the ALREADY-PARSED
            // `step_kind` local (see above) rather than a hardcoded
            // `StepKind::Cook`. `command` fields containing probe sigils are
            // not exclusive to native `cook` bodies — a `plate` body inside
            // a `for_each` recipe (unsandboxed by design, Standard §8.6)
            // lowers its command the same literal-sigil way (COOK-187 /
            // CS-0122) and passes `step_kind = "plate"`. Hardcoding `Cook`
            // here would silently flip a plate command's sandbox policy the
            // moment it referenced a probe value.
            match try_expand_probe_templates(&command) {
                Ok(Some((lua_code, detected_keys))) => {
                    for k in detected_keys {
                        if !probes.contains(&k) {
                            probes.push(k);
                        }
                    }
                    WorkPayload::LuaChunk {
                        code: lua_code,
                        inputs: inputs.clone(),
                        outputs: output_paths.clone(),
                        ingredient_groups: vec![],
                        step_kind,
                        is_chore,
                        line,
                    }
                }
                Ok(None) => WorkPayload::Shell { cmd: command, line: 0 },
                Err(e) => {
                    return Err(LuaError::runtime(format!(
                        "cook.add_unit: malformed probe placeholder in command: {e}"
                    )));
                }
            }
        };

        // Read optional per-unit env table (used by chore shell units to export
        // bound param values as env vars — COOK-36 §7.1.2).
        // CS-0127: `env` must be a table of string keys to string values —
        // never coerced; a bad pair is a hard error, not a silent drop.
        let unit_env_vars: std::collections::BTreeMap<String, String> =
            match tbl.get::<LuaValue>("env") {
                Ok(LuaValue::Nil) | Err(_) => std::collections::BTreeMap::new(),
                Ok(LuaValue::Table(t)) => {
                    // Iterate as LuaValue pairs so mlua's `String: FromLua`
                    // number-coercion cannot silently turn `env = { N = 1 }`
                    // into `"1"`; both key and value MUST already be strings.
                    let mut out = std::collections::BTreeMap::new();
                    for pair in t.pairs::<LuaValue, LuaValue>() {
                        let (k, v) = pair.map_err(|e| {
                            type_err(
                                "env",
                                "a table of string keys to string values",
                                &e.to_string(),
                            )
                        })?;
                        let key = match k {
                            LuaValue::String(s) => s.to_string_lossy().to_string(),
                            other => {
                                return Err(type_err(
                                    "env",
                                    "a table with string keys",
                                    other.type_name(),
                                ))
                            }
                        };
                        let val = match v {
                            LuaValue::String(s) => s.to_string_lossy().to_string(),
                            other => {
                                return Err(type_err(
                                    "env",
                                    "a table with string values",
                                    other.type_name(),
                                ))
                            }
                        };
                        out.insert(key, val);
                    }
                    out
                }
                Ok(other) => {
                    return Err(type_err(
                        "env",
                        "a table of string keys to string values",
                        other.type_name(),
                    ))
                }
            };

        let mut slot = body_slot_add.borrow_mut();
        let body = slot.as_mut().ok_or_else(|| {
            LuaError::runtime("cook.add_unit called outside a recipe body")
        })?;
        let dep_kind = if let Some(group_idx) = body.current_group {
            DepKind::StepGroup(group_idx)
        } else {
            DepKind::Sequential
        };
        let unit_idx = body.units.len();
        body.units.push(CapturedUnit {
            payload,
            cache_meta,
            dep_kind: dep_kind.clone(),
            probes,
            unit_env_vars,
            member: member.clone(),
            output_paths: output_paths.clone(),
        });
        if let DepKind::StepGroup(gi) = &dep_kind {
            body.step_groups[*gi].push(unit_idx);
        }
        for out in outputs_for_tracking {
            body.current_step_outputs.push(out);
        }
        // §10.4.1 terminal-output capture for module-registered recipes.
        // A module target-maker (e.g. `cook_cc.bin`) declares its units via
        // bare `cook.add_unit` calls — `DepKind::Sequential`, NOT wrapped in a
        // `cook.step_group` the way native `cook` steps are. The step_group
        // terminal-output capture (which feeds cross-recipe `$<recipe>` /
        // `cook.dep_output`, §10.2 step 2) therefore never fires for them, so
        // `dep_output` would resolve to the empty string. Mirror that capture
        // here: a Sequential unit's outputs become the recipe's running
        // terminal output (last-wins), so the recipe's output is its last
        // `add_unit`'s output. StepGroup units are left to the step_group drain.
        if matches!(dep_kind, DepKind::Sequential) && !output_paths.is_empty() {
            body.last_cook_step_outputs = output_paths.clone();
        }
        // Record dep edges: every dep ref accumulated in this step_group
        // applies to this unit.
        let dep_refs: Vec<String> = body.step_group_dep_refs.clone();
        for dep_name in dep_refs {
            body.dep_edges.push((unit_idx, dep_name));
        }
        Ok(())
    })?;
    cook.set("add_unit", add_unit_fn)?;

    // cook.passthrough(list) — declare the current step's "outputs" as a
    // copy of the given input list, without recording an emitting unit.
    // This is the register-side hook that implements Standard §5.4.1's
    // passthrough rule for `plate`, `test`, and bare shell steps: those
    // step kinds don't write files, but a downstream `$<recipe>` reference
    // (or another plate/test step that falls back to the recipe's
    // last-step outputs) needs the input list to be visible as the
    // recipe's terminal outputs.
    //
    // Codegen calls this once per plate/test/shell step, inside the
    // enclosing `cook.step_group`, with the same source expression the
    // step iterates over (`ingredients`, `_cook_outputs_N`, or a literal
    // list). The `step_group` close-out then drains the pushed values
    // into `last_cook_step_outputs` per the normal flow.
    let body_slot_pt = body_slot.clone();
    let passthrough_fn = lua.create_function(move |_, list: LuaTable| {
        let mut slot = body_slot_pt.borrow_mut();
        let body = slot.as_mut().ok_or_else(|| {
            mlua::Error::runtime("cook.passthrough called outside a recipe body")
        })?;
        for pair in list.sequence_values::<String>() {
            let item = pair.map_err(|e| {
                mlua::Error::runtime(format!("cook.passthrough: bad list element: {e}"))
            })?;
            body.current_step_outputs.push(item);
        }
        Ok(())
    })?;
    cook.set("passthrough", passthrough_fn)?;

    // cook.step_group(fn)
    let body_slot_sg = body_slot.clone();
    let step_group_fn = lua.create_function(move |_, func: LuaFunction| {
        {
            let mut slot = body_slot_sg.borrow_mut();
            let body = slot.as_mut().ok_or_else(|| {
                mlua::Error::runtime("cook.step_group called outside a recipe body")
            })?;
            let group_idx = body.step_groups.len();
            body.step_groups.push(Vec::new());
            body.current_group = Some(group_idx);
        }
        let result = func.call::<()>(());
        {
            let mut slot = body_slot_sg.borrow_mut();
            let body = slot.as_mut().ok_or_else(|| {
                mlua::Error::runtime("cook.step_group called outside a recipe body")
            })?;
            body.current_group = None;
            let outputs: Vec<String> = body.current_step_outputs.drain(..).collect();
            if !outputs.is_empty() {
                body.last_cook_step_outputs = outputs;
            }
            body.step_group_dep_refs.clear();
            body.step_group_dep_input_paths.clear();
            // NOTE: pending_member_dep_input_paths is deliberately NOT cleared
            // here — it is a per-add_unit buffer (drained via mem::take inside
            // add_unit), not a step-group-wide accumulator. Any dep_output_member
            // call is emitted inline in an add_unit's command expression, so it is
            // always consumed by that same add_unit before this close-out runs;
            // none should survive to the next step group.
        }
        result
    })?;
    cook.set("step_group", step_group_fn)?;

    Ok(())
}

/// Build a local cache key that encodes env_contribution so simultaneous
/// variant builds (e.g. different env-selected toolchains) coexist without
/// overwriting each other.
fn build_local_cache_key(
    _cookfile_path: &str,
    _recipe: &str,
    output_paths: &[String],
    inputs: &[String],
    command_hash: u64,
    env_contribution: u64,
) -> String {
    if let Some(first) = output_paths.first() {
        if env_contribution != 0 {
            format!("{first}@{:x}", env_contribution)
        } else {
            first.clone()
        }
    } else {
        let base = inputs.first().map(|s| s.as_str()).unwrap_or("");
        if env_contribution != 0 {
            format!("{}@{:x}:{:x}", base, command_hash, env_contribution)
        } else {
            format!("{}@{:x}", base, command_hash)
        }
    }
}

/// Retrieve the cookfile-relative path stored in the Lua named registry value
/// `__cook_cookfile_path`. Falls back to "Cookfile" when absent (legacy / test
/// call sites that don't thread a `CacheContext` through).
fn cookfile_relative_path(lua: &Lua) -> String {
    lua.named_registry_value::<String>("__cook_cookfile_path")
        .unwrap_or_else(|_| "Cookfile".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::rc::Rc;
    use crate::BodyCaptureState;
    use std::collections::BTreeMap;

    /// Convenience accessor used throughout the unit_api test module: borrow
    /// the body slot and panic if it's `None`. The slot is set to `Some(...)`
    /// by `make_lua_with_unit_api` for the duration of every test.
    fn body_ref(body_slot: &SharedBodySlot) -> std::cell::Ref<'_, BodyCaptureState> {
        std::cell::Ref::map(body_slot.borrow(), |slot| {
            slot.as_ref().expect("body slot populated for test")
        })
    }

    fn make_lua_with_unit_api(recipe_name: &str) -> (Lua, SharedBodySlot) {
        use std::sync::{Arc, Mutex};
        let lua = Lua::new();
        lua.globals().set("cook", lua.create_table().unwrap()).unwrap();
        let body_slot: SharedBodySlot =
            Rc::new(RefCell::new(Some(BodyCaptureState::new())));
        let terminal_outputs: SharedTerminalOutputs =
            Arc::new(Mutex::new(BTreeMap::new()));
        // Tests reference paths like "main.c" that don't exist; the
        // directory-rejection check skips non-existent paths, so any
        // working_dir is fine here.
        let working_dir = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        register_unit_api(
            &lua,
            body_slot.clone(),
            recipe_name,
            terminal_outputs,
            working_dir,
        )
        .unwrap();
        (lua, body_slot)
    }

    fn fake_cache_ctx() -> std::sync::Arc<cook_cache::cache_ctx::CacheContext> {
        let dir = tempfile::tempdir().expect("tempdir");
        let dir_path = dir.path().to_path_buf();
        std::mem::forget(dir); // tests are short-lived; let the OS clean up
        std::sync::Arc::new(cook_cache::cache_ctx::CacheContext {
            denylist: std::sync::Arc::new(cook_cache::envkey::EnvDenylist::baseline()),
            backend: std::sync::Arc::new(cook_cache::backend::LocalBackend::new(dir_path.clone())),
            cloud_config: std::sync::Arc::new(cook_cache::cloud_config::CloudConfig::default()),
            project_root: dir_path,
            project_id: "test-project".to_string(),
            publish_enabled: true,
        })
    }

    #[test]
    fn test_add_unit_basic() {
        let (lua, capture_state) = make_lua_with_unit_api("my_recipe");
        lua.set_app_data(fake_cache_ctx());
        lua.set_named_registry_value("__cook_cookfile_path", "Cookfile".to_string()).expect("set");
        lua.load(r#"
            cook.add_unit({
                command = "gcc -o main main.c",
                inputs = {"main.c"},
                output = "main",
            })
        "#).exec().unwrap();

        let state = body_ref(&capture_state);
        assert_eq!(state.units.len(), 1);
        let unit = &state.units[0];

        match &unit.payload {
            WorkPayload::Shell { cmd, line } => {
                assert_eq!(cmd, "gcc -o main main.c");
                assert_eq!(*line, 0);
            }
            _ => panic!("expected Shell payload"),
        }

        let meta = unit.cache_meta.as_ref().expect("expected cache_meta");
        assert_eq!(meta.recipe_name, "my_recipe");
        assert_eq!(meta.input_paths, vec!["main.c"]);
        assert_eq!(meta.output_paths, vec!["main".to_string()]);
        assert_eq!(meta.command_hash, hash_str("gcc -o main main.c"));

        assert!(matches!(unit.dep_kind, DepKind::Sequential));
    }

    #[test]
    fn test_add_unit_rejects_function_valued_command() {
        // A function-valued command used to coerce to "" and silently
        // no-op. It must now be a loud register-phase error.
        let (lua, _capture_state) = make_lua_with_unit_api("my_recipe");
        lua.set_app_data(fake_cache_ctx());
        lua.set_named_registry_value("__cook_cookfile_path", "Cookfile".to_string()).expect("set");
        let err = lua.load(r#"
            cook.add_unit({
                command = function() return "echo hi" end,
                inputs = {},
                output = "out/x.txt",
            })
        "#).exec().unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("`command` must be a string"), "got: {msg}");
        assert!(msg.contains("function"), "error must name the received type; got: {msg}");
    }

    #[test]
    fn test_add_unit_rejects_numeric_command() {
        let (lua, _capture_state) = make_lua_with_unit_api("my_recipe");
        lua.set_app_data(fake_cache_ctx());
        lua.set_named_registry_value("__cook_cookfile_path", "Cookfile".to_string()).expect("set");
        let err = lua.load(r#"cook.add_unit({ command = 42, output = "out/x.txt" })"#)
            .exec().unwrap_err();
        assert!(err.to_string().contains("`command` must be a string"), "got: {err}");
    }

    #[test]
    fn test_add_unit_no_cache() {
        let (lua, capture_state) = make_lua_with_unit_api("recipe");
        lua.set_app_data(fake_cache_ctx());
        lua.set_named_registry_value("__cook_cookfile_path", "Cookfile".to_string()).expect("set");
        lua.load(r#"
            cook.add_unit({
                command = "echo hello",
                cache = false,
            })
        "#).exec().unwrap();

        let state = body_ref(&capture_state);
        assert_eq!(state.units.len(), 1);
        assert!(state.units[0].cache_meta.is_none());
    }

    #[test]
    fn test_add_unit_interactive_flag() {
        let (lua, capture_state) = make_lua_with_unit_api("recipe");
        lua.set_app_data(fake_cache_ctx());
        lua.set_named_registry_value("__cook_cookfile_path", "Cookfile".to_string()).expect("set");
        lua.load(r#"
            cook.add_unit({
                command = "build/bin/lua -e 'print(1)'",
                interactive = true,
                cache = false,
            })
        "#).exec().unwrap();

        let state = body_ref(&capture_state);
        assert_eq!(state.units.len(), 1);
        match &state.units[0].payload {
            WorkPayload::Interactive { cmd, .. } => {
                assert_eq!(cmd, "build/bin/lua -e 'print(1)'");
            }
            other => panic!("expected Interactive payload, got {other:?}"),
        }
    }

    #[test]
    fn test_add_unit_sequential_by_default() {
        let (lua, capture_state) = make_lua_with_unit_api("recipe");
        lua.set_app_data(fake_cache_ctx());
        lua.set_named_registry_value("__cook_cookfile_path", "Cookfile".to_string()).expect("set");
        lua.load(r#"
            cook.add_unit({ command = "step1" })
            cook.add_unit({ command = "step2" })
        "#).exec().unwrap();

        let state = body_ref(&capture_state);
        assert_eq!(state.units.len(), 2);
        assert!(matches!(state.units[0].dep_kind, DepKind::Sequential));
        assert!(matches!(state.units[1].dep_kind, DepKind::Sequential));
    }

    #[test]
    fn test_step_group_makes_parallel() {
        let (lua, capture_state) = make_lua_with_unit_api("recipe");
        lua.set_app_data(fake_cache_ctx());
        lua.set_named_registry_value("__cook_cookfile_path", "Cookfile".to_string()).expect("set");
        lua.load(r#"
            cook.step_group(function()
                cook.add_unit({ command = "unit_a" })
                cook.add_unit({ command = "unit_b" })
            end)
        "#).exec().unwrap();

        let state = body_ref(&capture_state);
        assert_eq!(state.units.len(), 2);
        assert!(matches!(state.units[0].dep_kind, DepKind::StepGroup(0)));
        assert!(matches!(state.units[1].dep_kind, DepKind::StepGroup(0)));
        assert_eq!(state.step_groups.len(), 1);
        assert_eq!(state.step_groups[0], vec![0, 1]);
    }

    #[test]
    fn test_step_group_sequential_after() {
        let (lua, capture_state) = make_lua_with_unit_api("recipe");
        lua.set_app_data(fake_cache_ctx());
        lua.set_named_registry_value("__cook_cookfile_path", "Cookfile".to_string()).expect("set");
        lua.load(r#"
            cook.step_group(function()
                cook.add_unit({ command = "parallel_unit" })
            end)
            cook.add_unit({ command = "sequential_unit" })
        "#).exec().unwrap();

        let state = body_ref(&capture_state);
        assert_eq!(state.units.len(), 2);
        assert!(matches!(state.units[0].dep_kind, DepKind::StepGroup(0)));
        assert!(matches!(state.units[1].dep_kind, DepKind::Sequential));
    }

    #[test]
    fn test_last_cook_step_outputs_tracked() {
        let (lua, capture_state) = make_lua_with_unit_api("recipe");
        lua.set_app_data(fake_cache_ctx());
        lua.set_named_registry_value("__cook_cookfile_path", "Cookfile".to_string()).expect("set");
        lua.load(r#"
            -- First cook step (OneToOne, 2 outputs)
            cook.step_group(function()
                cook.add_unit({ command = "gcc -c a.c -o a.o", inputs = {"a.c"}, output = "a.o" })
                cook.add_unit({ command = "gcc -c b.c -o b.o", inputs = {"b.c"}, output = "b.o" })
            end)
            -- Second cook step (ManyToOne, 1 output)
            cook.step_group(function()
                cook.add_unit({ command = "ar rcs lib.a a.o b.o", inputs = {"a.o", "b.o"}, output = "lib.a" })
            end)
        "#).exec().unwrap();

        let state = body_ref(&capture_state);
        // Terminal outputs = from the LAST step group that produced outputs: ["lib.a"]
        assert_eq!(state.last_cook_step_outputs, vec!["lib.a"]);
    }

    #[test]
    fn test_plate_step_group_does_not_overwrite_terminal() {
        let (lua, capture_state) = make_lua_with_unit_api("recipe");
        lua.set_app_data(fake_cache_ctx());
        lua.set_named_registry_value("__cook_cookfile_path", "Cookfile".to_string()).expect("set");
        lua.load(r#"
            -- Cook step produces output
            cook.step_group(function()
                cook.add_unit({ command = "gcc -o app main.c", inputs = {"main.c"}, output = "app" })
            end)
            -- Plate-like step (no output field) -- should NOT overwrite terminal
            cook.step_group(function()
                cook.add_unit({ command = "./app", cache = false })
            end)
        "#).exec().unwrap();

        let state = body_ref(&capture_state);
        assert_eq!(state.last_cook_step_outputs, vec!["app"]);
    }

    #[test]
    fn test_add_unit_outputs_plural() {
        let (lua, capture_state) = make_lua_with_unit_api("my_recipe");
        lua.set_app_data(fake_cache_ctx());
        lua.set_named_registry_value("__cook_cookfile_path", "Cookfile".to_string()).expect("set");
        lua.load(r#"
            cook.add_unit({
                command = "split a.c",
                inputs = {"a.c"},
                outputs = {"a.o", "a.d"},
            })
        "#).exec().unwrap();

        let state = body_ref(&capture_state);
        assert_eq!(state.units.len(), 1);
        let unit = &state.units[0];
        let meta = unit.cache_meta.as_ref().expect("expected cache_meta");
        assert_eq!(
            meta.output_paths,
            vec!["a.o".to_string(), "a.d".to_string()]
        );
        // cache_key should embed context+env when they are non-zero
        assert!(meta.cache_key.starts_with("a.o"), "cache_key starts with first output");
    }

    #[test]
    fn test_add_unit_outputs_and_output_conflict_errors() {
        let (lua, _capture_state) = make_lua_with_unit_api("my_recipe");
        lua.set_app_data(fake_cache_ctx());
        lua.set_named_registry_value("__cook_cookfile_path", "Cookfile".to_string()).expect("set");
        let result = lua.load(r#"
            cook.add_unit({
                command = "split a.c",
                inputs = {"a.c"},
                output = "a.o",
                outputs = {"a.o", "a.d"},
            })
        "#).exec();
        assert!(
            result.is_err(),
            "expected error when both `output` and `outputs` are provided"
        );
    }

    #[test]
    fn test_add_unit_lua_code_one_to_one() {
        let (lua, capture_state) = make_lua_with_unit_api("my_recipe");
        lua.set_app_data(fake_cache_ctx());
        lua.set_named_registry_value("__cook_cookfile_path", "Cookfile".to_string()).expect("set");
        lua.load(
            r#"
            cook.add_unit({
                inputs = {"main.c"},
                output = "main.o",
                lua_code = "print('hi')",
                ingredient_groups = {{"a.c", "b.c"}},
            })
        "#,
        )
        .exec()
        .unwrap();

        let state = body_ref(&capture_state);
        assert_eq!(state.units.len(), 1);
        let unit = &state.units[0];
        match &unit.payload {
            WorkPayload::LuaChunk {
                code,
                inputs,
                outputs,
                ingredient_groups,
                step_kind: _,
                is_chore: _,
                line: _,
            } => {
                assert_eq!(code, "print('hi')");
                assert_eq!(inputs, &vec!["main.c".to_string()]);
                assert_eq!(outputs, &vec!["main.o".to_string()]);
                assert_eq!(
                    ingredient_groups,
                    &vec![vec!["a.c".to_string(), "b.c".to_string()]]
                );
            }
            other => panic!("expected LuaChunk, got {other:?}"),
        }
    }

    #[test]
    fn test_add_unit_lua_code_multi_output_block_step() {
        let (lua, capture_state) = make_lua_with_unit_api("recipe");
        lua.set_app_data(fake_cache_ctx());
        lua.set_named_registry_value("__cook_cookfile_path", "Cookfile".to_string()).expect("set");
        lua.load(
            r#"
            cook.add_unit({
                inputs = {"src.rs"},
                outputs = {"a.js", "a.wasm"},
                lua_code = "os.execute('wasm-pack build')",
                ingredient_groups = {{"src.rs"}},
            })
        "#,
        )
        .exec()
        .unwrap();

        let state = body_ref(&capture_state);
        assert_eq!(state.units.len(), 1);
        match &state.units[0].payload {
            WorkPayload::LuaChunk {
                code,
                inputs,
                outputs,
                ingredient_groups,
                step_kind: _,
                is_chore: _,
                line: _,
            } => {
                assert_eq!(code, "os.execute('wasm-pack build')");
                assert_eq!(inputs, &vec!["src.rs".to_string()]);
                assert_eq!(
                    outputs,
                    &vec!["a.js".to_string(), "a.wasm".to_string()]
                );
                assert_eq!(ingredient_groups, &vec![vec!["src.rs".to_string()]]);
            }
            other => panic!("expected LuaChunk, got {other:?}"),
        }
    }

    #[test]
    fn test_single_step_terminal_outputs() {
        let (lua, capture_state) = make_lua_with_unit_api("recipe");
        lua.set_app_data(fake_cache_ctx());
        lua.set_named_registry_value("__cook_cookfile_path", "Cookfile".to_string()).expect("set");
        lua.load(r#"
            cook.step_group(function()
                cook.add_unit({ command = "gcc -o app main.c", inputs = {"main.c"}, output = "app" })
            end)
        "#).exec().unwrap();

        let state = body_ref(&capture_state);
        assert_eq!(state.last_cook_step_outputs, vec!["app"]);
    }

    #[test]
    fn add_unit_populates_consulted_env_from_keys_list() {
        // The lookup reads from cook.env (the Cook Lua VM env table), NOT the
        // process env — that's the merged config-overlay+process value the
        // command actually consumed. Populate cook.env directly here; in real
        // usage, capture.rs seeds cook.env from process env at startup and
        // config dispatch may overlay project-specific values.
        let lua = Lua::new();
        let cook_table = lua.create_table().unwrap();
        let env_table = lua.create_table().unwrap();
        env_table.set("FOO_TEST_VAR_X", "the-value").unwrap();
        cook_table.set("env", env_table).unwrap();
        lua.globals().set("cook", cook_table).unwrap();

        let capture_state: SharedBodySlot = Rc::new(RefCell::new(Some(BodyCaptureState::new())));
        let terminal_outputs: SharedTerminalOutputs =
            std::sync::Arc::new(std::sync::Mutex::new(BTreeMap::new()));
        let working_dir = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        register_unit_api(
            &lua,
            capture_state.clone(),
            "my_recipe",
            terminal_outputs,
            working_dir,
        )
        .unwrap();

        lua.set_app_data(fake_cache_ctx());
        lua.set_named_registry_value("__cook_cookfile_path", "Cookfile".to_string()).expect("set");

        lua.load(r#"
            cook.add_unit({
                command = "make all",
                inputs = {"main.c"},
                output = "main",
                consulted_env_keys = {"FOO_TEST_VAR_X"},
            })
        "#).exec().unwrap();

        let state = body_ref(&capture_state);
        assert_eq!(state.units.len(), 1);
        let meta = state.units[0].cache_meta.as_ref().expect("cache_meta");
        assert_eq!(
            meta.consulted_env.get("FOO_TEST_VAR_X").map(|s| s.as_str()),
            Some("the-value"),
            "consulted_env must contain FOO_TEST_VAR_X=the-value (read from cook.env)"
        );
        // env_contribution must be non-zero because a non-denylisted var was consulted
        assert_ne!(meta.env_contribution, 0, "env_contribution must be non-zero");
    }

    #[test]
    fn add_unit_appends_resolved_dep_paths_to_input_paths() {
        // Spec §4.3: cross-recipe dep refs accumulated by cook.dep_output(name)
        // resolve to terminal output paths and land in cache_meta.input_paths
        // (only — never in WorkPayload.inputs).
        let lua = Lua::new();
        let cook_table = lua.create_table().unwrap();
        cook_table.set("env", lua.create_table().unwrap()).unwrap();
        lua.globals().set("cook", cook_table).unwrap();

        let capture_state: SharedBodySlot = Rc::new(RefCell::new(Some(BodyCaptureState::new())));
        let terminal_outputs: SharedTerminalOutputs = std::sync::Arc::new(std::sync::Mutex::new(BTreeMap::new()));
        terminal_outputs
            .lock().unwrap()
            .insert("greet".into(), vec!["build/greet.o".into()]);
        terminal_outputs
            .lock().unwrap()
            .insert("util".into(), vec!["build/util.o".into()]);

        let working_dir = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        register_unit_api(
            &lua,
            capture_state.clone(),
            "demo",
            terminal_outputs.clone(),
            working_dir,
        )
        .unwrap();
        crate::dep_output_api::register_dep_output_api(
            &lua,
            terminal_outputs,
            capture_state.clone(),
            std::collections::BTreeMap::new(),
            String::new(),
            std::collections::BTreeMap::new(),
        )
        .unwrap();

        lua.set_app_data(fake_cache_ctx());
        lua.set_named_registry_value("__cook_cookfile_path", "Cookfile".to_string())
            .expect("set");

        // Codegen sequence: cook.dep_output() called inside command construction
        // accumulates dep refs; add_unit then picks them up.
        lua.load(
            r#"
            local _ = cook.dep_output("greet")
            local _ = cook.dep_output("util")
            cook.add_unit({
                command = "gcc build/greet.o build/util.o -o build/demo",
                inputs = {},
                output = "build/demo",
            })
        "#,
        )
        .exec()
        .unwrap();

        let state = body_ref(&capture_state);
        assert_eq!(state.units.len(), 1);
        let meta = state.units[0]
            .cache_meta
            .as_ref()
            .expect("cache_meta present");
        assert_eq!(
            meta.input_paths,
            vec!["build/greet.o".to_string(), "build/util.o".to_string()],
            "cross-recipe dep paths must land in cache_meta.input_paths"
        );

        // WorkPayload inputs MUST remain empty — those drive iteration vars.
        match &state.units[0].payload {
            WorkPayload::Shell { cmd, .. } => {
                assert!(cmd.contains("gcc"));
            }
            other => panic!("expected Shell, got {other:?}"),
        }
    }

    #[test]
    fn add_unit_inside_chore_marks_payload_is_chore_true() {
        let (lua, capture_state) = make_lua_with_unit_api("my_chore");
        lua.set_app_data(fake_cache_ctx());
        lua.set_named_registry_value("__cook_cookfile_path", "Cookfile".to_string()).expect("set");
        lua.load(r#"
            cook._enter_chore()
            cook.add_unit({
                command = "fzf --prompt='> '",
                interactive = true,
                cache = false,
            })
            cook._exit_chore()
        "#).exec().unwrap();

        let state = body_ref(&capture_state);
        assert_eq!(state.units.len(), 1);
        match &state.units[0].payload {
            WorkPayload::Interactive { is_chore, .. } => {
                assert!(*is_chore, "unit emitted inside chore body must have is_chore=true");
            }
            other => panic!("expected Interactive payload, got {other:?}"),
        }
    }

    #[test]
    fn add_unit_inside_chore_marks_lua_chunk_is_chore_true() {
        let (lua, capture_state) = make_lua_with_unit_api("my_chore");
        lua.set_app_data(fake_cache_ctx());
        lua.set_named_registry_value("__cook_cookfile_path", "Cookfile".to_string()).expect("set");
        lua.load(r#"
            cook._enter_chore()
            cook.add_unit({
                lua_code = "print('hello from chore')",
                interactive = true,
                cache = false,
            })
            cook._exit_chore()
        "#).exec().unwrap();

        let state = body_ref(&capture_state);
        assert_eq!(state.units.len(), 1);
        match &state.units[0].payload {
            WorkPayload::LuaChunk { is_chore, .. } => {
                assert!(*is_chore, "lua chunk emitted inside chore body must have is_chore=true");
            }
            other => panic!("expected LuaChunk payload, got {other:?}"),
        }
    }

    #[test]
    fn add_unit_outside_chore_marks_payload_is_chore_false() {
        let (lua, capture_state) = make_lua_with_unit_api("my_recipe");
        lua.set_app_data(fake_cache_ctx());
        lua.set_named_registry_value("__cook_cookfile_path", "Cookfile".to_string()).expect("set");
        lua.load(r#"
            cook.add_unit({
                command = "build/bin/lua -e 'print(1)'",
                interactive = true,
                cache = false,
            })
        "#).exec().unwrap();

        let state = body_ref(&capture_state);
        assert_eq!(state.units.len(), 1);
        match &state.units[0].payload {
            WorkPayload::Interactive { is_chore, .. } => {
                assert!(!*is_chore, "unit emitted outside chore must have is_chore=false");
            }
            other => panic!("expected Interactive payload, got {other:?}"),
        }
    }

    #[test]
    fn add_unit_reads_discovered_inputs_table() {
        let (lua, capture_state) = make_lua_with_unit_api("demo");
        lua.set_app_data(fake_cache_ctx());
        lua.set_named_registry_value("__cook_cookfile_path", "Cookfile".to_string()).expect("set");

        lua.load(r#"
            cook.add_unit({
                inputs = { "src/a.c" },
                output = "build/a.o",
                command = "gcc -c src/a.c -o build/a.o",
                discovered_inputs = { from = ".cook/deps/a.d", format = "make" },
            })
        "#).exec().expect("exec");

        let st = body_ref(&capture_state);
        let unit: &CapturedUnit = st.units.last().expect("one unit");
        let cm = unit.cache_meta.as_ref().expect("cache_meta");
        let di = cm.discovered_inputs.as_ref().expect("discovered_inputs");
        assert_eq!(di.from, ".cook/deps/a.d");
        assert_eq!(di.format, "make");
    }

    #[test]
    fn add_unit_rejects_unsupported_discovered_inputs_format() {
        let (lua, _capture_state) = make_lua_with_unit_api("demo");
        lua.set_app_data(fake_cache_ctx());
        lua.set_named_registry_value("__cook_cookfile_path", "Cookfile".to_string()).expect("set");

        let result = lua.load(r#"
            cook.add_unit({
                inputs = { "x" }, output = "y", command = "true",
                discovered_inputs = { from = "x.d", format = "ninja" },
            })
        "#).exec();

        let err = result.expect_err("expected error for unsupported format").to_string();
        assert!(err.contains("ninja"), "diagnostic must name the unsupported format; got: {err}");
        assert!(err.contains("supported"), "diagnostic must say what is supported; got: {err}");
    }

    #[test]
    fn add_unit_rejects_absolute_discovered_from() {
        let (lua, _capture_state) = make_lua_with_unit_api("demo");
        lua.set_app_data(fake_cache_ctx());
        lua.set_named_registry_value("__cook_cookfile_path", "Cookfile".to_string()).expect("set");

        let result = lua.load(r#"
            cook.add_unit({
                inputs = { "x" }, output = "y", command = "true",
                discovered_inputs = { from = "/etc/secrets.d", format = "make" },
            })
        "#).exec();

        let err = result.expect_err("expected error for absolute path").to_string();
        assert!(
            err.contains("relative") || err.contains("absolute"),
            "diagnostic must mention 'relative' or 'absolute'; got: {err}"
        );
    }

    #[test]
    fn add_unit_rejects_dotdot_discovered_from() {
        let (lua, _capture_state) = make_lua_with_unit_api("demo");
        lua.set_app_data(fake_cache_ctx());
        lua.set_named_registry_value("__cook_cookfile_path", "Cookfile".to_string()).expect("set");

        let result = lua.load(r#"
            cook.add_unit({
                inputs = { "x" }, output = "y", command = "true",
                discovered_inputs = { from = "../escape.d", format = "make" },
            })
        "#).exec();

        let err = result.expect_err("expected error for '..' path").to_string();
        assert!(err.contains(".."), "diagnostic must contain '..'; got: {err}");
    }

    /// Regression: `cook.add_unit` MUST reject directory inputs at register
    /// time. The cache hashing layer reads files; passing a directory used
    /// to silently produce an empty cache record (only `_source_hash`),
    /// causing the unit to re-run on every invocation. We now fail fast
    /// with a clear, actionable diagnostic instead.
    #[test]
    fn add_unit_rejects_directory_input() {
        use std::sync::{Arc, Mutex};

        let tmp = tempfile::tempdir().expect("tempdir");
        // Build a real directory the recipe will (mistakenly) declare as
        // an input.
        let upstream = tmp.path().join("upstream").join("lib");
        std::fs::create_dir_all(&upstream).expect("mkdir upstream/lib");
        std::fs::write(upstream.join("a.txt"), b"a").expect("write a.txt");

        let lua = Lua::new();
        lua.globals()
            .set("cook", lua.create_table().unwrap())
            .unwrap();
        let capture_state: SharedBodySlot = Rc::new(RefCell::new(Some(BodyCaptureState::new())));
        let terminal_outputs: SharedTerminalOutputs =
            Arc::new(Mutex::new(BTreeMap::new()));
        register_unit_api(
            &lua,
            capture_state.clone(),
            "vendor",
            terminal_outputs,
            tmp.path().to_path_buf(),
        )
        .unwrap();

        lua.set_app_data(fake_cache_ctx());
        lua.set_named_registry_value(
            "__cook_cookfile_path",
            "Cookfile".to_string(),
        )
        .expect("set");

        let result = lua
            .load(
                r#"
                cook.add_unit({
                    command = "cp -a upstream/lib build/lib",
                    inputs = { "upstream/lib" },
                    output = "build/lib.stamp",
                })
            "#,
            )
            .exec();

        let err = result
            .expect_err("expected error for directory input")
            .to_string();
        assert!(
            err.contains("is a directory"),
            "diagnostic must contain 'is a directory'; got: {err}"
        );
        assert!(
            err.contains("upstream/lib"),
            "diagnostic must name the offending path; got: {err}"
        );
        assert!(
            err.contains("glob") || err.contains("specific files"),
            "diagnostic must suggest a fix (glob or list specific files); got: {err}"
        );
        // No unit must have been recorded.
        assert!(
            body_ref(&capture_state).units.is_empty(),
            "rejected add_unit must not record a unit"
        );
    }

    /// Files (existing or not) MUST still pass through. Verifies the
    /// directory-rejection check doesn't accidentally reject valid file
    /// inputs (the common case).
    #[test]
    fn add_unit_accepts_file_inputs() {
        use std::sync::{Arc, Mutex};

        let tmp = tempfile::tempdir().expect("tempdir");
        let src = tmp.path().join("upstream").join("lib");
        std::fs::create_dir_all(&src).expect("mkdir upstream/lib");
        std::fs::write(src.join("a.txt"), b"a").expect("write a.txt");
        std::fs::write(src.join("b.txt"), b"b").expect("write b.txt");

        let lua = Lua::new();
        lua.globals()
            .set("cook", lua.create_table().unwrap())
            .unwrap();
        let capture_state: SharedBodySlot = Rc::new(RefCell::new(Some(BodyCaptureState::new())));
        let terminal_outputs: SharedTerminalOutputs =
            Arc::new(Mutex::new(BTreeMap::new()));
        register_unit_api(
            &lua,
            capture_state.clone(),
            "vendor",
            terminal_outputs,
            tmp.path().to_path_buf(),
        )
        .unwrap();

        lua.set_app_data(fake_cache_ctx());
        lua.set_named_registry_value(
            "__cook_cookfile_path",
            "Cookfile".to_string(),
        )
        .expect("set");

        // Real file (exists) and a not-yet-built output (does not exist).
        lua.load(
            r#"
            cook.add_unit({
                command = "cp upstream/lib/a.txt build/a.txt",
                inputs = { "upstream/lib/a.txt" },
                output = "build/a.txt",
            })
        "#,
        )
        .exec()
        .expect("file input must be accepted");

        assert_eq!(body_ref(&capture_state).units.len(), 1);
    }

    /// `outputs` (plural) is also covered: declaring a directory as a
    /// declared output is rejected.
    #[test]
    fn add_unit_rejects_directory_in_outputs_plural() {
        use std::sync::{Arc, Mutex};

        let tmp = tempfile::tempdir().expect("tempdir");
        let dir = tmp.path().join("build").join("artifacts");
        std::fs::create_dir_all(&dir).expect("mkdir build/artifacts");

        let lua = Lua::new();
        lua.globals()
            .set("cook", lua.create_table().unwrap())
            .unwrap();
        let capture_state: SharedBodySlot = Rc::new(RefCell::new(Some(BodyCaptureState::new())));
        let terminal_outputs: SharedTerminalOutputs =
            Arc::new(Mutex::new(BTreeMap::new()));
        register_unit_api(
            &lua,
            capture_state.clone(),
            "build",
            terminal_outputs,
            tmp.path().to_path_buf(),
        )
        .unwrap();

        lua.set_app_data(fake_cache_ctx());
        lua.set_named_registry_value(
            "__cook_cookfile_path",
            "Cookfile".to_string(),
        )
        .expect("set");

        let result = lua
            .load(
                r#"
                cook.add_unit({
                    command = "mkdir -p build/artifacts && touch build/a.o build/b.o",
                    inputs = {},
                    outputs = { "build/a.o", "build/artifacts" },
                })
            "#,
            )
            .exec();

        let err = result
            .expect_err("expected error for directory output")
            .to_string();
        assert!(
            err.contains("is a directory"),
            "diagnostic must contain 'is a directory'; got: {err}"
        );
        assert!(
            err.contains("build/artifacts"),
            "diagnostic must name the offending path; got: {err}"
        );
    }

    #[test]
    fn add_unit_captures_probes_field() {
        let (lua, capture_state) = make_lua_with_unit_api("recipe");
        lua.set_app_data(fake_cache_ctx());
        lua.set_named_registry_value("__cook_cookfile_path", "Cookfile".to_string()).expect("set");

        lua.load(r#"
            cook.add_unit({
                name = "myapp.o",
                inputs = { "myapp.c" },
                outputs = { "build/myapp.o" },
                probes = { "cc:zlib", "cc:compiler" },
                command = "true",
            })
        "#)
        .exec()
        .unwrap();

        let state = body_ref(&capture_state);
        let u = state.units.first().expect("one unit");
        assert_eq!(u.probes, vec!["cc:zlib", "cc:compiler"]);
    }

    #[test]
    fn add_unit_seal_field_sets_cache_meta_and_probes() {
        // COOK-161: opts.seal carries the effective seal set onto CacheMeta and
        // unions into the unit's probe-dependency vec so the unit runs after the
        // sealed probes are materialised.
        let (lua, capture_state) = make_lua_with_unit_api("recipe");
        lua.set_app_data(fake_cache_ctx());
        lua.set_named_registry_value("__cook_cookfile_path", "Cookfile".to_string()).expect("set");

        lua.load(r#"
            cook.add_unit({
                name = "x.o",
                outputs = { "x.o" },
                command = "cc",
                seal = { "host" },
            })
        "#)
        .exec()
        .unwrap();

        let state = body_ref(&capture_state);
        let u = state.units.first().expect("one unit");
        assert!(
            u.probes.contains(&"host".to_string()),
            "sealed key must be unioned into probes; got: {:?}",
            u.probes
        );
        let cm = u.cache_meta.as_ref().expect("cache_meta present");
        assert!(
            cm.seal_keys.contains("host"),
            "sealed key must be on CacheMeta.seal_keys; got: {:?}",
            cm.seal_keys
        );
    }

    #[test]
    fn add_unit_local_pinned_disposition_booleans() {
        // COOK-162 / I3: opts.sharing ("local"/"pinned") is parsed into the
        // CacheMeta.sharing enum. Three sub-cases: local, pinned, neither.

        // Case 1: sharing = "local" → CacheMeta.sharing == Local
        {
            let (lua, capture_state) = make_lua_with_unit_api("recipe");
            lua.set_app_data(fake_cache_ctx());
            lua.set_named_registry_value("__cook_cookfile_path", "Cookfile".to_string()).expect("set");
            lua.load(r#"
                cook.add_unit({
                    command = "echo local",
                    output = "out.txt",
                    sharing = "local",
                })
            "#)
            .exec()
            .unwrap();
            let state = body_ref(&capture_state);
            let cm = state.units[0].cache_meta.as_ref().expect("cache_meta present");
            assert_eq!(
                cm.sharing,
                cook_contracts::Sharing::Local,
                "sharing=\"local\" should propagate to CacheMeta.sharing"
            );
        }

        // Case 2: sharing = "pinned" → CacheMeta.sharing == Pinned
        {
            let (lua, capture_state) = make_lua_with_unit_api("recipe");
            lua.set_app_data(fake_cache_ctx());
            lua.set_named_registry_value("__cook_cookfile_path", "Cookfile".to_string()).expect("set");
            lua.load(r#"
                cook.add_unit({
                    command = "echo pinned",
                    output = "out.txt",
                    sharing = "pinned",
                })
            "#)
            .exec()
            .unwrap();
            let state = body_ref(&capture_state);
            let cm = state.units[0].cache_meta.as_ref().expect("cache_meta present");
            assert_eq!(
                cm.sharing,
                cook_contracts::Sharing::Pinned,
                "sharing=\"pinned\" should propagate to CacheMeta.sharing"
            );
        }

        // Case 3: neither → Shared default
        {
            let (lua, capture_state) = make_lua_with_unit_api("recipe");
            lua.set_app_data(fake_cache_ctx());
            lua.set_named_registry_value("__cook_cookfile_path", "Cookfile".to_string()).expect("set");
            lua.load(r#"
                cook.add_unit({
                    command = "echo neither",
                    output = "out.txt",
                })
            "#)
            .exec()
            .unwrap();
            let state = body_ref(&capture_state);
            let cm = state.units[0].cache_meta.as_ref().expect("cache_meta present");
            assert_eq!(
                cm.sharing,
                cook_contracts::Sharing::Shared,
                "omitting sharing should leave CacheMeta.sharing Shared"
            );
        }
    }

    #[test]
    fn add_unit_without_probes_defaults_to_empty() {
        let (lua, capture_state) = make_lua_with_unit_api("recipe");
        lua.set_app_data(fake_cache_ctx());
        lua.set_named_registry_value("__cook_cookfile_path", "Cookfile".to_string()).expect("set");

        lua.load(r#"
            cook.add_unit({
                command = "echo hello",
                cache = false,
            })
        "#)
        .exec()
        .unwrap();

        let state = body_ref(&capture_state);
        let u = state.units.first().expect("one unit");
        assert!(u.probes.is_empty());
    }

    #[test]
    fn add_unit_probes_non_list_errors() {
        let (lua, _capture_state) = make_lua_with_unit_api("recipe");
        lua.set_app_data(fake_cache_ctx());
        lua.set_named_registry_value("__cook_cookfile_path", "Cookfile".to_string()).expect("set");

        let result = lua.load(r#"
            cook.add_unit({
                command = "echo hello",
                cache = false,
                probes = "not-a-list",
            })
        "#).exec();

        assert!(result.is_err(), "probes must be a list, not a string");
        let err = result.unwrap_err().to_string();
        assert!(err.contains("probes"), "error must mention 'probes'; got: {err}");
    }

    #[test]
    fn add_unit_legacy_requires_field_is_rejected() {
        let (lua, _capture_state) = make_lua_with_unit_api("recipe");
        lua.set_app_data(fake_cache_ctx());
        lua.set_named_registry_value("__cook_cookfile_path", "Cookfile".to_string()).expect("set");

        let result = lua.load(r#"
            cook.add_unit({
                name = "u",
                inputs = {}, outputs = {"out.txt"},
                cache = false,
                requires = { "cc:zlib" },
                command = "true",
            })
        "#).exec();

        assert!(result.is_err(), "legacy `requires` field must be rejected");
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("rename to `probes`"),
            "diagnostic must direct user to `probes`; got: {err}"
        );
    }

    #[test]
    fn add_unit_legacy_requires_field_as_string_is_rejected() {
        // A mid-migration Cookfile might write `requires = "cc:zlib"` (string)
        // rather than a table. The guard MUST still fire so the author learns
        // the field is gone — silently accepting non-table values would leave
        // partial migrations undetected.
        let (lua, _capture_state) = make_lua_with_unit_api("recipe");
        lua.set_app_data(fake_cache_ctx());
        lua.set_named_registry_value("__cook_cookfile_path", "Cookfile".to_string()).expect("set");

        let result = lua.load(r#"
            cook.add_unit({
                name = "u",
                inputs = {}, outputs = {"out.txt"},
                cache = false,
                requires = "cc:zlib",
                command = "true",
            })
        "#).exec();

        assert!(result.is_err(), "legacy `requires` field must be rejected even when non-table");
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("rename to `probes`"),
            "diagnostic must direct user to `probes`; got: {err}"
        );
    }

    /// CS-0074: cook.add_unit with a command containing `$<key:field>` probe-value
    /// sigils MUST be rewritten into a LuaChunk that resolves the probe value at
    /// execute time via cook.cache.get.
    #[test]
    fn add_unit_command_with_probe_template_is_rewritten() {
        let (lua, capture_state) = make_lua_with_unit_api("recipe");
        lua.set_app_data(fake_cache_ctx());
        lua.set_named_registry_value("__cook_cookfile_path", "Cookfile".to_string()).expect("set");

        lua.load(r#"
            cook.add_unit({
                name = "u",
                inputs = {}, outputs = {"out.txt"},
                cache = false,
                command = "echo $<demo:k.v> > out.txt",
            })
        "#)
        .exec()
        .unwrap();

        let state = body_ref(&capture_state);
        let unit = state.units.first().expect("one unit");

        let has_cache_get = match &unit.payload {
            WorkPayload::LuaChunk { code, .. } => code.contains("cook.cache.get"),
            WorkPayload::Shell { cmd, .. } => cmd.contains("cook.cache.get"),
            _ => false,
        };
        assert!(
            has_cache_get,
            "expected template to be expanded; got payload: {:?}",
            unit.payload
        );

        // The probe key (everything before the first `.` after the `:`) must be
        // auto-added to probes.
        assert!(
            unit.probes.contains(&"demo:k".to_string()),
            "detected probe key must be auto-added to probes; got: {:?}",
            unit.probes
        );
    }

    /// CS-0101: `$<file:PATH>` in a raw cook.add_unit command string is the
    /// reserved file-reference namespace, NOT a probe key. v1 does not
    /// support file refs in raw register-block command strings — the
    /// template expander must reject them loudly instead of misparsing
    /// `file` as a probe key.
    #[test]
    fn add_unit_command_with_file_ref_sigil_is_rejected() {
        let (lua, _capture_state) = make_lua_with_unit_api("recipe");
        lua.set_app_data(fake_cache_ctx());
        lua.set_named_registry_value("__cook_cookfile_path", "Cookfile".to_string()).expect("set");

        let err = lua.load(r#"
            cook.add_unit({
                inputs = {}, outputs = {"out.txt"},
                cache = false,
                command = "render --tokens $<file:tokens.css> > out.txt",
            })
        "#)
        .exec()
        .unwrap_err()
        .to_string();

        assert!(
            err.contains("not supported in raw cook.add_unit command strings"),
            "expected the raw-command file-ref rejection; got: {err}"
        );
    }

    /// COOK-96 Task 5: add_unit must record `member` and `output_paths` on the
    /// resulting `CapturedUnit` so the engine can build the per-member output map
    /// needed by `$<recipe[]>`.
    #[test]
    fn add_unit_retains_member_and_outputs() {
        let (lua, capture_state) = make_lua_with_unit_api("encode");
        lua.set_app_data(fake_cache_ctx());
        lua.set_named_registry_value("__cook_cookfile_path", "Cookfile".to_string()).expect("set");

        lua.load(r#"
            cook.add_unit({
                output = "build/s1.mp4",
                command = "echo hi",
                member = "{\"id\":\"s1\"}",
            })
        "#).exec().unwrap();

        let state = body_ref(&capture_state);
        let u = state.units.last().expect("a unit was captured");
        assert_eq!(u.member.as_deref(), Some("{\"id\":\"s1\"}"));
        assert_eq!(u.output_paths, vec!["build/s1.mp4".to_string()]);
    }

    #[test]
    fn add_unit_record_flag_threads_to_cache_meta() {
        // COOK-163: opts.record marks an intrinsically non-reproducible artifact.
        // The register layer must read opts.record and set it on CacheMeta.
        let (lua, capture_state) = make_lua_with_unit_api("recipe");
        lua.set_app_data(fake_cache_ctx());
        lua.set_named_registry_value("__cook_cookfile_path", "Cookfile".to_string()).expect("set");

        lua.load(r#"
            cook.add_unit({
                name = "x.o",
                outputs = { "x.o" },
                command = "cc",
                record = true,
            })
        "#)
        .exec()
        .unwrap();

        let state = body_ref(&capture_state);
        let u = state.units.first().expect("one unit");
        let cm = u.cache_meta.as_ref().expect("cache_meta present");
        assert!(
            cm.record,
            "record must be true on CacheMeta when opts.record = true; got: {}",
            cm.record
        );
    }

    #[test]
    fn add_unit_record_defaults_false() {
        // COOK-163: when opts.record is absent, it defaults to false.
        let (lua, capture_state) = make_lua_with_unit_api("recipe");
        lua.set_app_data(fake_cache_ctx());
        lua.set_named_registry_value("__cook_cookfile_path", "Cookfile".to_string()).expect("set");

        lua.load(r#"
            cook.add_unit({
                name = "x.o",
                outputs = { "x.o" },
                command = "cc",
            })
        "#)
        .exec()
        .unwrap();

        let state = body_ref(&capture_state);
        let u = state.units.first().expect("one unit");
        let cm = u.cache_meta.as_ref().expect("cache_meta present");
        assert!(
            !cm.record,
            "record must be false on CacheMeta when opts.record is absent; got: {}",
            cm.record
        );
    }

    /// CS-0119: a trailing-slash output `"pkg/"` declares a directory output.
    /// On the SECOND build `pkg/` already exists as a directory on disk; the
    /// register-time directory-rejection check MUST NOT fire for trailing-slash
    /// entries — they are intentionally directories, not a mistake.
    #[test]
    fn add_unit_accepts_directory_output_trailing_slash() {
        use std::sync::{Arc, Mutex};

        let tmp = tempfile::tempdir().expect("tempdir");
        // Simulate the "second build": pkg/ already exists on disk.
        let pkg_dir = tmp.path().join("pkg");
        std::fs::create_dir_all(&pkg_dir).expect("mkdir pkg");

        let lua = Lua::new();
        lua.globals()
            .set("cook", lua.create_table().unwrap())
            .unwrap();
        let capture_state: SharedBodySlot = Rc::new(RefCell::new(Some(BodyCaptureState::new())));
        let terminal_outputs: SharedTerminalOutputs =
            Arc::new(Mutex::new(BTreeMap::new()));
        register_unit_api(
            &lua,
            capture_state.clone(),
            "build",
            terminal_outputs,
            tmp.path().to_path_buf(),
        )
        .unwrap();

        lua.set_app_data(fake_cache_ctx());
        lua.set_named_registry_value(
            "__cook_cookfile_path",
            "Cookfile".to_string(),
        )
        .expect("set");

        // pkg/ already exists as a directory; the trailing slash signals
        // CS-0119 directory-output semantics — must be accepted, not rejected.
        let result = lua
            .load(
                r#"
                cook.add_unit({
                    command = "npm pack --pack-destination pkg/",
                    inputs = { "package.json" },
                    outputs = { "pkg/" },
                })
            "#,
            )
            .exec();

        assert!(
            result.is_ok(),
            "directory output with trailing slash must be accepted even when the \
             directory already exists on disk (CS-0119); got: {:?}",
            result.err()
        );
        assert_eq!(
            body_ref(&capture_state).units.len(),
            1,
            "unit must be recorded"
        );
    }
}
