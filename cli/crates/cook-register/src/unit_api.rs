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

/// Classify one declared input entry as a path or a pattern (§17.1.1.2,
/// CS-0186). This is the ONLY place the question is asked; everything
/// downstream reads the answer off the declaration.
///
/// **An entry naming an existing regular file is a path, whatever is in its
/// name.** That arm is what the rule is for: `ingredients "pages/*.tsx"` is
/// resolved here at register phase, so `pages/[id].tsx` reaches this call as a
/// file the register phase has already seen in the tree, and re-reading it as a
/// character class would expand it, match nothing, and drop it out of the
/// unit's key. The same holds for a module's own `fs.glob` results, which is
/// why the test is over the tree rather than over a list of the routes by which
/// an entry can arrive — one test, and every register-phase resolution
/// satisfies it by construction.
///
/// **Anything else is decided by the same test its producer applies to the same
/// declaration.** An entry that is not a file here is either a consumed output
/// whose producer has not run (`dist/**`, `dist/`, `build/app.o`) or a plain
/// miss; in both cases the string IS the declaration, and §17.6 rule 1's test
/// is the one that must answer, or one declaration would mean a pattern to the
/// unit capturing it and a path to the unit reading it.
///
/// A directory is not a regular file, so `dist/` falls to the second arm and is
/// classified a pattern there — which is what it is (CS-0119).
fn classify_declared_input(working_dir: &Path, path: &str) -> cook_contracts::cache::DeclaredInput {
    let resolved: PathBuf = if Path::new(path).is_absolute() {
        PathBuf::from(path)
    } else {
        working_dir.join(path)
    };
    // `metadata` rather than `symlink_metadata`: a symlink to a regular file is
    // a file the unit reads, and the cache hashes what it points at.
    if std::fs::metadata(&resolved).map(|m| m.is_file()).unwrap_or(false) {
        return cook_contracts::cache::DeclaredInput::path(path);
    }
    if cook_fingerprint::is_terminal_output(path) {
        cook_contracts::cache::DeclaredInput::pattern(path)
    } else {
        cook_contracts::cache::DeclaredInput::path(path)
    }
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

/// CS-0074: collect the probe keys a `command` references through
/// `$<key:...>` sigils, so `cook.add_unit` can merge them into the unit's
/// `probes` list automatically.
///
/// **This no longer rewrites the command (CS-0188).** It used to return a Lua
/// chunk that read each value with `cook.probes.get` and called `cook.sh` with
/// the resolved text, because §22.5.7 required exactly that. The requirement is
/// withdrawn: it was a mechanism mandate in a section about substitution, and
/// the mechanism it mandated is what silenced the unit. A rewritten command
/// became an execute-phase Lua chunk, whose captured output travelled a
/// different path from a shell command's, so a step printed nothing when — and
/// only when — its command happened to mention a probe.
///
/// The command now stays a command. Substitution happens where the values live,
/// in the execute-phase worker, which holds both the resolved probe store and a
/// live VM and so needs no generated source at all.
///
/// Detection uses `cook_contracts::sigil` — `scan` for the placeholder grammar,
/// `probe_ref` for the key/path split — the same parse codegen uses, so this
/// path and codegen cannot disagree about what a probe sigil means (COOK-357;
/// before that the walker was copied here and the two escaped the key
/// differently).
///
/// Returns the distinct keys in order of first appearance; empty for a command
/// with no probe reference.
fn scan_probe_keys(command: &str) -> Result<Vec<String>, String> {
    let spans = cook_contracts::sigil::scan(command);

    // CS-0101/CS-0187: `file:` is a reserved placeholder namespace, not a probe
    // key. Raw register-block add_unit command strings do not support file refs
    // — fail loudly rather than misparse `file:x` as a probe key or silently
    // pass the bytes through.
    if let Some(span) = spans.iter().find(|s| s.ident.starts_with("file:")) {
        return Err(format!(
            "$<{}>: $<file:PATH> is not supported in raw cook.add_unit command strings; \
             write the step in a Cookfile recipe body instead",
            span.ident
        ));
    }

    // `probe_ref` owns the colon discriminator AND the `file:` exclusion, so
    // there is no second filter to keep in step.
    let mut seen = std::collections::BTreeSet::new();
    let mut keys: Vec<String> = vec![];
    for span in &spans {
        if let Some(r) = cook_contracts::sigil::probe_ref(&span.ident) {
            if seen.insert(r.key().to_string()) {
                keys.push(r.key().to_string());
            }
        }
    }
    Ok(keys)
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
        // CS-0045 / CS-0127 / CS-0153: the originating step kind drives the
        // execute-phase sandbox policy on the resulting LuaChunk. Codegen
        // omits the field for cook/chore bodies, both of which run
        // sandboxed to the project root — there is no unsandboxed step
        // kind (CS-0135 retired the `plate` step). The captured-unit
        // default is `cook` because that is the strictest policy. An
        // unrecognised string, or any non-string value, is a hard error
        // rather than a silent fall-through to the default. `"test"` is
        // rejected outright (CS-0153, §22.1): a test work unit is
        // registrable only through `cook.add_test` (§22.4), which builds
        // `WorkPayload::Test` directly rather than routing through this
        // `step_kind` field — accepting `"test"` here would silently build
        // a unit invisible to `cook test`.
        let step_kind: cook_contracts::StepKind = match tbl.get::<LuaValue>("step_kind") {
            Ok(LuaValue::Nil) | Err(_) => cook_contracts::StepKind::Cook,
            Ok(LuaValue::String(s)) => {
                let sv = s.to_string_lossy().to_string();
                match sv.as_str() {
                    "chore" => cook_contracts::StepKind::Chore,
                    "cook" => cook_contracts::StepKind::Cook,
                    // CS-0185 supersedes CS-0153's refusal. A test unit is an
                    // ordinary work unit; this is what makes it one, and the
                    // registration bookkeeping now happens here rather than in
                    // a second function that had to be kept in step.
                    "test" => cook_contracts::StepKind::Test,
                    _ => {
                        return Err(type_err(
                            "step_kind",
                            "one of \"cook\", \"test\", \"chore\"",
                            &format!("{sv:?}"),
                        ))
                    }
                }
            }
            Ok(other) => return Err(type_err("step_kind", "a string", other.type_name())),
        };
        // CS-0185: read before `step_kind` is moved into a payload variant below.
        let is_test = matches!(step_kind, cook_contracts::StepKind::Test);

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

        // §17.1.1.2: each entry is classified HERE, once, and carries the
        // answer from here on. `inputs` and the dep-output paths are strings
        // whose provenance this call cannot see, so they are classified
        // against the tree.
        //
        // Deduplicated, order of first occurrence (COOK-84): a path named by
        // both `inputs` and a step-group dep arrives twice, and what a unit
        // declares must not depend on how many ways a file was reached.
        let cache_inputs: Vec<cook_contracts::cache::DeclaredInput> = {
            let mut out: Vec<cook_contracts::cache::DeclaredInput> = Vec::new();
            let mut push = |e: cook_contracts::cache::DeclaredInput| {
                if !out.iter().any(|prev| prev.path == e.path) {
                    out.push(e);
                }
            };
            for p in inputs
                .iter()
                .map(|s| s.as_str())
                .chain(dep_input_paths.iter().map(|s| s.as_str()))
                .chain(member_dep_input_paths.iter().map(|s| s.as_str()))
            {
                push(classify_declared_input(&wd_for_add_unit, p));
            }
            out
        };

        // Read consulted_env_keys from the table and look up values in the
        // declared-variable store (CS-0172) — the resolved `var` namespace the
        // command actually consumed at substitution time, per spec §5.3.1.
        // Reading from std::env::var would miss config-overlay values and
        // capture process env the command never saw — both produce false
        // misses. Values are rendered with `var_to_string` so a boolean-valued
        // variable keys as `true`/`false` rather than being silently dropped.
        let mut consulted_env: std::collections::BTreeMap<String, String> =
            std::collections::BTreeMap::new();
        let env_table: Option<LuaTable> = crate::var_api::var_store(lua).ok();
        // CS-0127: `consulted_env_keys` must be a table of strings, or the
        // literal string "*" — never coerced, and element collection is
        // strict (a non-string element is a hard error, not a silent drop).
        match tbl.get::<LuaValue>("consulted_env_keys") {
            Ok(LuaValue::Nil) | Err(_) => {}
            Ok(LuaValue::Table(list)) => {
                let keys = collect_string_list(&list, "consulted_env_keys")?;
                // A Lua-body unit reads `var.NAME` on the execute-phase VM,
                // where values arrive in string form. Rather than coerce — and
                // hand the body a truthy `"false"` — CS-0172 rejects the read
                // here, at the declaration site's own phase.
                let lua_body = !matches!(tbl.get::<LuaValue>("lua_code"), Ok(LuaValue::Nil) | Err(_));
                if let Some(env) = &env_table {
                    for v in keys {
                        if let Ok(val) = env.get::<LuaValue>(v.clone()) {
                            if !val.is_nil() {
                                if lua_body && !matches!(val, LuaValue::String(_)) {
                                    return Err(LuaError::runtime(format!(
                                        "var.{v} is a {} and is read by an \
                                         execute-phase Lua body, which receives \
                                         declared variables in string form \
                                         (Standard §5.3.1). Interpolate it as \
                                         $<{v}>, or branch on it in a register \
                                         block and pass the result in.",
                                        val.type_name()
                                    )));
                                }
                                consulted_env
                                    .insert(v.clone(), crate::var_api::var_to_string(&v, &val)?);
                            }
                        }
                    }
                }
            }
            Ok(LuaValue::String(s)) if s.to_str().ok().as_deref() == Some("*") => {
                if let Some(env) = &env_table {
                    for pair in env.clone().pairs::<String, LuaValue>() {
                        if let Ok((k, v)) = pair {
                            let rendered = crate::var_api::var_to_string(&k, &v)?;
                            consulted_env.insert(k, rendered);
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

        // COOK-64 §8.2/§17.1: a member fan-out unit carries its data member
        // (canonical-rendered by `cook.member_to_string`). Fold it into the
        // command hash so each member's unit gets a distinct fingerprint —
        // editing one member re-runs only its unit (observable #5). NUL
        // delimiters keep the member byte-range disjoint from the command.
        // Shell bodies already bake the member into the command text; this
        // additionally covers Lua-block bodies whose `item` reads are opaque to
        // the command string. `None` (non-member-fanout units) hashes as before.
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

        // CS-0185 §22.4: a test unit MUST declare no outputs. The empty output
        // list is what makes its hit replay a recorded outcome rather than
        // restore artifacts; `step_kind` is what makes it a test. Neither is
        // inferred from the other, so an output here is an author error and
        // not a silent reclassification.
        if is_test && !output_paths.is_empty() {
            return Err(LuaError::runtime(format!(
                "cook.add_unit: a step_kind = \"test\" unit declares no outputs, \
                 but {} was given (Cook Standard \u{00a7}22.4, CS-0185)",
                output_paths.len()
            )));
        }
        let consumes: Vec<String> = match tbl.get::<LuaValue>("consumes") {
            Ok(LuaValue::Nil) | Err(_) => vec![],
            Ok(LuaValue::Table(t)) => {
                let mut out = Vec::new();
                for v in t.sequence_values::<LuaValue>() {
                    match v.map_err(|e| LuaError::runtime(format!("cook.add_unit: `consumes`: {e}")))? {
                        LuaValue::String(sv) => {
                            let sv = sv.to_string_lossy().to_string();
                            // Validated with the matcher the engine folds with, so a
                            // pattern accepted here is the pattern that runs.
                            // Rejected at register time because an unparseable glob
                            // matches nothing, and "matches nothing" on an allowlist
                            // points the under-keying way.
                            if let Err(e) = cook_fingerprint::consumes::validate_pattern(&sv) {
                                return Err(LuaError::runtime(format!(
                                    "cook.add_unit: `consumes` entry '{sv}' is not a \
                                     valid glob ({e}); it would match no predecessor \
                                     output, silently widening the cached-pass window"
                                )));
                            }
                            out.push(sv);
                        }
                        other => {
                            return Err(type_err("consumes", "a table of strings", other.type_name()))
                        }
                    }
                }
                out
            }
            Ok(other) => return Err(type_err("consumes", "a table of strings", other.type_name())),
        };

        // CS-0186: a test unit carries cache metadata like any other unit.
        // The gate that stood here — `cache_enabled && !is_test` — kept test
        // units out of the step index while a separate result store answered
        // for them. Both halves of that are gone in the same change, because
        // either alone leaves two stores disagreeing: attaching the metadata
        // without serving the hits would publish records nothing reads, and
        // serving them without the metadata has nothing to read.
        //
        // What makes this safe is that an empty output list is now a fact with
        // its own meaning (§17.1.1.1) rather than a synonym for uncacheable:
        // `cacheability` sends this unit down the ResultOnly arm, which looks
        // up a verdict instead of artifacts, and the publish path files a
        // record with no outputs to upload.
        //
        // CS-0186 §17.4 rule 1: nothing to key on means no key. A unit with no
        // declared output, no declared input and no materialised data member
        // has nothing whose movement could invalidate it, so caching it would
        // serve its first result forever.
        //
        // One rule, every unit, and it fires only where it must. A `cook` unit
        // always declares an output, so it never fires for one. It fires for
        // `test { cargo test }` — no output, no source, nothing iterated —
        // whose true inputs are the whole source tree and therefore opaque
        // here; a key over the command text alone would be a false green. It
        // does NOT fire for a fan-out unit that declares no file but carries a
        // member, because the member is an observable input (§17.1 observable
        // 5) and is already folded into `command_hash` above. Before CS-0186
        // that case was refused along with the source-less one, so a `test`
        // fanned out over `ingredients <probe>` re-ran on every invocation
        // while its `cook` sibling over the same source cached per member.
        //
        // This is the DECLARED half of the rule. The engine asks it again when
        // the unit is ready, over the RESOLVED list, because a declaration can
        // be non-empty and resolve to nothing (§17.4 rule 1). Both sides call
        // `has_something_to_key_on` so there is one rule and two vantage points
        // rather than two rules.
        let member_keyed = member.is_some();
        let cache_enabled = cache_enabled
            && cook_contracts::cache::record::has_something_to_key_on(
                output_paths.len(),
                cache_inputs.len(),
                member_keyed,
            );
        let cache_meta = if cache_enabled {
            let cache_key = build_local_cache_key(
                &cookfile_path,
                &rname,
                &output_paths,
                &cache_inputs,
                command_hash,
                env_contribution_val,
                &seal_keys,
            );
            Some(CacheMeta {
                recipe_name: rname.clone(),
                project_id,
                cookfile_path,
                cache_key,
                inputs: cache_inputs.clone(),
                consumes: consumes.clone(),
                member_keyed,
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

        // CS-0152: literal `cook.probes.get("key")` reads inside a Lua-code
        // unit's body demand the probe automatically, mirroring the
        // shell-side `$<key>` sigil auto-union above (and the seal-key
        // union just above). A shell body's `probes` field is populated by
        // scanning its command string for sigils; a Lua body had no
        // equivalent surface, so a probe consumed ONLY from Lua code was
        // never demand-scheduled ahead of the unit and read nil at execute
        // time. Scanning here — statically, at capture time — closes that
        // gap for the common literal-key case; non-literal reads still fall
        // through to the execute-phase hard error.
        if let Some(code) = &lua_code {
            for k in cook_luagen::lua_var::scan_probe_reads(code) {
                if !probes.contains(&k) {
                    probes.push(k);
                }
            }
        }

        // CS-0185 §22.4: the two fields a test unit adds. `suite` is NOT among
        // them — it is removed, and passing it is an error rather than a
        // silent no-op, since a caller writing it means something by it.
        if is_test && !matches!(tbl.get::<LuaValue>("suite"), Ok(LuaValue::Nil) | Err(_)) {
            return Err(LuaError::runtime(
                "cook.add_unit: `suite` was removed with cook.add_test; a test unit \
                 belongs to the recipe that registers it (Cook Standard \u{00a7}22.4, \
                 CS-0185)"
                    .to_string(),
            ));
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
        // CS-0185: the ENCLOSING recipe names a test unit. Read here with the
        // other pre-payload body reads, because `add_unit` is registered once
        // per Lua state and `rname` is the registration-time name rather than
        // the recipe currently running. This is a field read, not a count over
        // the units already recorded, so it needs no ordering with them.
        let current_recipe: String = {
            let slot = body_slot_add.borrow();
            slot.as_ref()
                .and_then(|b| b.current_recipe.clone())
                .unwrap_or_default()
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
        // CS-0185 §22.4: a test unit MUST carry exactly one non-empty body.
        // `cook.add_test` enforced this and `cook.add_unit` does not, because a
        // declaration-only cook unit — outputs, no command — is legal. A test
        // unit with no body is not: it would pass vacuously, which is the false
        // green the test kind exists to prevent.
        if is_test {
            let has_cmd = !command.is_empty();
            let has_lua = lua_code.as_deref().is_some_and(|c| !c.is_empty());
            if has_cmd == has_lua {
                return Err(LuaError::runtime(format!(
                    "cook.add_unit: a step_kind = \"test\" unit requires exactly one of \
                     `command` or `lua_code`, non-empty — got {} (Cook Standard \u{00a7}22.4, \
                     CS-0185)",
                    if has_cmd { "both" } else { "neither" }
                )));
            }
        }
        // CS-0191: a test unit takes the same payloads as any other unit,
        // because that is what it is. CS-0185 made it register through the one
        // function; CS-0186 made it cache through the one record; this is the
        // same sentence reaching the runner. A command becomes `Shell`, a body
        // becomes `LuaChunk`, and what is left over — the name the reporter
        // knows it by — rides on the unit as `test_name`.
        //
        // Gone with the variant: `timeout`, hardcoded `u64::MAX` since CS-0135
        // removed the modifier that set it, so the kill loop reading it was
        // unreachable; `should_fail`, hardcoded `false` on the same terms, with
        // inversion written into the body instead; and `iteration_item`, which
        // CS-0185 already recorded as a duplicate of `member` and which is now
        // simply `member`.
        let test_name: Option<String> =
            is_test.then(|| format!("{}_test{}", current_recipe, line));
        let payload = if is_test {
            // An empty `lua_code` reads as ABSENT, matching the removed
            // `cook.add_test`: `{ command = "true", lua_code = "" }` is a
            // command test, not a body-less one.
            match lua_code.filter(|c| !c.is_empty()) {
                Some(code) => WorkPayload::LuaChunk {
                    code,
                    inputs: inputs.clone(),
                    // A test declares no outputs. That is the whole of what
                    // makes it an observing unit (§17.1.1.1); nothing else
                    // about it is special.
                    outputs: Vec::new(),
                    ingredient_groups: vec![],
                    step_kind,
                    is_chore: false,
                    line,
                },
                None => WorkPayload::Shell { cmd: command, line },
            }
        } else if let Some(code) = lua_code {
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
            // CS-0074: scan the command for `$<key:field>` probe-value
            // sigils and auto-add the keys it references to `probes`, so the
            // unit gets its DAG edge, its fingerprint fold and demand-driven
            // scheduling without the author restating what the command already
            // says.
            //
            // CS-0188: the command is NOT rewritten. It used to become a
            // LuaChunk calling `cook.probes.get` and `cook.sh`, which is what
            // made a probe-referencing step silent — that path reported no
            // captured output. §22.5.7's requirement to rewrite is withdrawn,
            // so a `command` is a Shell unit whether or not it mentions a
            // probe, and the two report identically. Substitution moved to the
            // worker, which has the values and a VM and needs no generated
            // source.
            //
            // This also retires the CS-0127/CS-0153 concern that used to live
            // here: the rewritten chunk had to carry the originating
            // `step_kind` so it ran under the right sandbox policy. There is
            // no chunk, and a shell command was never confined by the Lua
            // sandbox in the first place, so there is nothing left to carry.
            match scan_probe_keys(&command) {
                Ok(detected_keys) => {
                    for k in detected_keys {
                        if !probes.contains(&k) {
                            probes.push(k);
                        }
                    }
                    // `line`, where the no-probe arm used to hardcode 0. Two
                    // units differing only by a probe reference reported
                    // different lines in their failures, which is the same
                    // divergence in miniature.
                    WorkPayload::Shell { cmd: command, line }
                }
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
            test_name,
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
    // passthrough rule for `test` and bare shell steps: those step kinds
    // don't write files, but a downstream `$<recipe>` reference (or
    // another test step that falls back to the recipe's last-step
    // outputs) needs the input list to be visible as the recipe's
    // terminal outputs.
    //
    // Codegen calls this once per test/shell step, inside the
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

    // cook.step_group(fn, opts?)
    //
    // `opts.dep_order` (list of recipe names) declares the group's ordering
    // deps up front instead of relying on a bare `cook.dep_order` call landing
    // in the right place. The accumulator those calls write is positional —
    // it applies to every `add_unit` after it and is cleared when the group
    // closes — so a bare call's meaning depends on where the author put it.
    // Declaring the names on the group makes the scope syntactic: these deps,
    // these units, no ordering to get wrong. Equivalent to calling
    // `cook.dep_order(name)` as the group's first statement, including the
    // register-order guarantee.
    let body_slot_sg = body_slot.clone();
    // Either argument order is accepted. `cook.step_group(opts, fn)` is the
    // form to use inside a recipe body: the CS-0134 Lua-span parser ends the
    // statement at the function literal's `end`, so a trailing `, { … })`
    // after it is read as a loose shell command and rejected. Opts-first keeps
    // the whole call inside one brace-balanced span, and reads better anyway —
    // the declaration precedes the body it scopes.
    let step_group_fn = lua.create_function(move |lua, (a, b): (LuaValue, Option<LuaValue>)| {
        let (func, opts): (LuaFunction, Option<LuaTable>) = match (a, b) {
            (LuaValue::Function(f), None) => (f, None),
            (LuaValue::Function(f), Some(LuaValue::Table(t))) => (f, Some(t)),
            (LuaValue::Table(t), Some(LuaValue::Function(f))) => (f, Some(t)),
            (LuaValue::Function(f), Some(LuaValue::Nil)) => (f, None),
            _ => {
                return Err(mlua::Error::runtime(
                    "cook.step_group takes a function, optionally with an opts \
                     table on either side: step_group(fn), step_group(opts, fn), \
                     or step_group(fn, opts)",
                ))
            }
        };
        // Resolve the declared deps BEFORE opening the group: cook.dep_order
        // forces the referent's body, which re-enters the register APIs, and
        // it must not do so with a half-open group on the caller's body state.
        let declared: Vec<String> = match &opts {
            Some(t) => match t.get::<Option<LuaTable>>("dep_order")? {
                Some(list) => list
                    .sequence_values::<String>()
                    .collect::<LuaResult<Vec<String>>>()
                    .map_err(|e| {
                        mlua::Error::runtime(format!(
                            "cook.step_group: opts.dep_order must be a list of recipe-name strings: {e}"
                        ))
                    })?,
                None => Vec::new(),
            },
            None => Vec::new(),
        };
        {
            let mut slot = body_slot_sg.borrow_mut();
            let body = slot.as_mut().ok_or_else(|| {
                mlua::Error::runtime("cook.step_group called outside a recipe body")
            })?;
            let group_idx = body.step_groups.len();
            body.step_groups.push(Vec::new());
            body.current_group = Some(group_idx);
        }
        if !declared.is_empty() {
            let cook: LuaTable = lua.globals().get("cook")?;
            let dep_order: LuaFunction = cook.get("dep_order")?;
            for name in declared {
                dep_order.call::<()>(name)?;
            }
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

    // CS-0186 §8.6.1: the outputs of the preceding output-producing step in
    // the enclosing recipe body — the iteration source a `test` step falls back
    // on when the recipe declares no `ingredients`.
    //
    // Read at REGISTER phase rather than baked into codegen, which is what
    // makes it see every unit however it was registered. A parse-time local
    // (`_cook_outputs_N`) only exists for a `cook` step the parser lowered, so
    // a `test` following a unit declared by `cook.add_unit` — which is how
    // every module target-maker declares its units — had no source at all, and
    // the engine covered for it by walking the unit's DAG predecessors. That
    // walk is what CS-0186 removes: it worked on the graph, so it could only be
    // justified for units already known to be tests, and it left the engine
    // holding an input set the declaration did not contain.
    //
    // `last_cook_step_outputs` is already maintained for both routes — drained
    // from a completed `cook.step_group`, and set directly by a `Sequential`
    // `add_unit` with outputs (§10.4.1) — so both see the same answer here.
    // With a `member` argument it answers for that member alone (§22.6): the
    // outputs of the unit the preceding step registered FOR that member. A
    // fan-out `test` needs exactly this and neither of its neighbours — the
    // whole set costs the fan-out its per-member reuse (§17.1 observable 5),
    // and an empty set leaves the unit keyed on nothing its producer wrote,
    // which is a member-keyed unit replaying a pass over an artifact that has
    // since gone bad. Last-wins per member, matching how `member_outputs` is
    // built for the cross-recipe `$<recipe[in]>` join.
    //
    // The fallback is not a convenience: a member-fanout recipe may follow a step
    // that GATHERED rather than fanned out (§{cat.probes.member-source}), and that
    // step's one unit carries no member. Its outputs are what every member unit
    // then reads.
    let body_slot_prior = body_slot.clone();
    let prior_outputs_fn = lua.create_function(move |lua, member: Option<String>| {
        let slot = body_slot_prior.borrow();
        let body = match slot.as_ref() {
            Some(b) => b,
            None => return lua.create_sequence_from(Vec::<String>::new()),
        };
        if let Some(m) = member.filter(|m| !m.is_empty()) {
            let own: Option<Vec<String>> = body
                .units
                .iter()
                .filter(|u| u.member.as_deref() == Some(m.as_str()) && !u.output_paths.is_empty())
                .map(|u| u.output_paths.clone())
                .next_back();
            if let Some(paths) = own {
                return lua.create_sequence_from(paths);
            }
        }
        lua.create_sequence_from(body.last_cook_step_outputs.clone())
    })?;
    cook.set("prior_outputs", prior_outputs_fn)?;

    Ok(())
}

/// The marker that opens an observing unit's identity (§17.1.1.1, CS-0186).
///
/// Producing keys are a declared output path, optionally suffixed with the env
/// contribution. Observing keys are a digest, and the two share one index, so
/// the digest is written in a form no declared output path takes. `:` is not a
/// path separator on any supported platform and a workspace-relative output
/// beginning with one is not a path any Cookfile writes.
///
/// A collision would not be a correctness failure — the two units' output
/// counts disagree, so each is judged stale against the other's record and
/// rebuilds rather than replaying it — but they would then clobber each other
/// on every run, which is the permanent-churn shape CS-0169 exists to refuse.
/// Nothing else refuses it for us: `reject_duplicate_outputs` compares DECLARED
/// OUTPUTS, and an observing unit declares none, so this marker is the whole of
/// what keeps the two spaces apart.
pub const OBSERVING_KEY_MARKER: char = ':';

/// Build a local cache key that encodes env_contribution so simultaneous
/// variant builds (e.g. different env-selected toolchains) coexist without
/// overwriting each other.
///
/// Two shapes, one convention: `<identity>` or `<identity>@<env-hex>`. What
/// serves as the identity is what the unit's effect kind (§17.1.1.1) leaves
/// available.
fn build_local_cache_key(
    _cookfile_path: &str,
    _recipe: &str,
    output_paths: &[String],
    inputs: &[cook_contracts::cache::DeclaredInput],
    command_hash: u64,
    env_contribution: u64,
    seal_keys: &std::collections::BTreeSet<String>,
) -> String {
    let identity = match output_paths.first() {
        // A producing unit is identified by a declared output path, which
        // CS-0169 makes claimable by at most one unit.
        Some(first) => first.clone(),
        // An observing unit has no output path, so it is identified by a
        // digest of its DECLARATION (CS-0186).
        None => observing_identity(inputs, command_hash, seal_keys),
    };
    if env_contribution != 0 {
        format!("{identity}@{env_contribution:x}")
    } else {
        identity
    }
}

/// The identity of a unit that declares no outputs: a digest over the paths it
/// declares and the command it runs.
///
/// Three exclusions are normative (§17.1.1.1), and each is load-bearing:
///
/// * **Not the input CONTENTS.** This is the one that decides the design. An
///   identity that moved whenever the contents moved would never be found
///   twice, so no prior record would ever be available to compare against,
///   every invalidation would present as a first-ever build, and `cook why`
///   could report a key but never a cause. Identity says WHICH UNIT; the
///   recorded determinants and input records say WHETHER IT MAY BE REPLAYED.
/// * **Not the recipe name, and not the source position.** §17.4 requires that
///   moving a test within a recipe, or a recipe between Cookfiles, not bust its
///   cache. Both are already in scope here — `build_local_cache_key` takes
///   `_cookfile_path` and `_recipe` and has never used either — and they stay
///   unused deliberately rather than by omission.
/// * **Nothing further than the determinants the declaration carries.** Two
///   units in one index that no such determinant separates share one record,
///   and replaying either for the other cannot be observed.
///
///   The **effective seal key set** is one of those determinants, which is why
///   it is hashed here. Sealed VALUES cannot be — they are execute-phase, and
///   this runs at register phase — but leaving the KEYS out too makes
///   `test { ./run } seal toolchain` and a bare `test { ./run }` in one recipe
///   one identity. They then share a record whose recorded seal contribution
///   matches at most one of them, so each invalidates the other on every run:
///   not a false green, but the permanent churn CS-0169 exists to refuse.
///

/// What it replaces: `<first-input>@<command-hash>`. That form was weakly
/// unique — every unit sharing a first input and a command text collided, and
/// a unit declaring NO inputs keyed as the empty string plus its command hash,
/// so two such units in one recipe were one entry. It was nearly unreachable
/// while output-less units were nearly never cached, and CS-0186 makes it
/// carry every test unit in the project.
///
/// Paths are deduplicated, order-preserving, for the same reason the test
/// payload's input list is (COOK-84): a path named by both `inputs` and a
/// step-group dep arrived twice, and a unit's identity must not depend on how
/// many ways a file was reached. Order is otherwise kept as declared, which is
/// deterministic per registration; sorting would additionally erase a
/// reordering that IS a declaration change.
fn observing_identity(
    inputs: &[cook_contracts::cache::DeclaredInput],
    command_hash: u64,
    seal_keys: &std::collections::BTreeSet<String>,
) -> String {
    let mut hasher = xxhash_rust::xxh3::Xxh3::new();
    let mut seen: Vec<&str> = Vec::with_capacity(inputs.len());
    for entry in inputs {
        let p = entry.path.as_str();
        if seen.contains(&p) {
            continue;
        }
        seen.push(p);
        // NUL-terminated so ["ab", "c"] and ["a", "bc"] cannot hash alike;
        // a path cannot contain NUL, so the separator is unambiguous. The kind
        // rides along because it is part of what was declared: the same string
        // read as a file and read as a pattern are two different declarations
        // (§17.1.1.2).
        hasher.update(p.as_bytes());
        hasher.update(if entry.is_pattern() { b"\0*" } else { b"\0=" });
    }
    hasher.update(&command_hash.to_le_bytes());
    // The seal key set, sorted by the BTreeSet it arrives in: the same keys
    // declared in a different order are the same declaration.
    for key in seal_keys {
        hasher.update(key.as_bytes());
        hasher.update(b"\0");
    }
    format!("{}{:016x}", OBSERVING_KEY_MARKER, hasher.digest())
}

/// Retrieve the cookfile-relative path stored in the Lua named registry value
/// `__cook_cookfile_path`. Falls back to "Cookfile" when absent (legacy / test
/// call sites that don't thread a `CacheContext` through).
fn cookfile_relative_path(lua: &Lua) -> String {
    lua.named_registry_value::<String>("__cook_cookfile_path")
        .unwrap_or_else(|_| "Cookfile".to_string())
}

#[cfg(test)]
#[path = "tests/unit_api_tests.rs"]
mod tests;
