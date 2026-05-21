use mlua::prelude::*;
use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Command;
use std::rc::Rc;
use std::cell::RefCell;

use cook_contracts::{
    CapturedUnit, DepKind, WorkPayload, REGISTER_SURFACE_CHORE_NAME, REGISTER_SURFACE_NAME,
};

use crate::{RecipeKind, SharedBodySlot};

/// Where a `RegisteredRecipe` came from. Used by Phase 2 collision detection
/// (and surface diagnostics) to name BOTH sites of a name conflict and
/// identify the kind of each.
///
/// - `Static`  — emitted by codegen from a surface `recipe NAME` /
///   `chore NAME` block via `cook.__register_surface(...)` or
///   `cook.__register_surface_chore(...)`. The `RecipeKind` carried alongside
///   on `RegisteredRecipe` distinguishes recipe from chore so
///   `detect_collisions` can label the site correctly.
/// - `Dynamic` — recorded by user / wrapper Lua code calling
///   `cook.recipe(...)` (e.g. `cook_cc.bin` target-makers). Always
///   recipe-kind: chores cannot be registered dynamically.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegistrationSource {
    /// Emitted by codegen from a surface `recipe NAME` block.
    Static { line: usize },
    /// Recorded by user / wrapper Lua code calling `cook.recipe(...)`.
    Dynamic { line: usize },
}

pub struct RegisteredRecipe {
    pub name: String,
    pub function: LuaRegistryKey,
    pub metadata: RegisteredMetadata,
    pub source: RegistrationSource,
    /// Whether the registered name is a normal recipe or a chore.
    ///
    /// Codegen sets `Chore` only via `cook.__register_surface_chore`
    /// (surface `chore NAME` blocks). All other registration paths
    /// (`cook.recipe`, `cook.__register_surface`) set `Recipe`.
    pub kind: RecipeKind,
}

/// One parameter declared in a `chore NAME param …` header.
///
/// Mirrors the `kind` strings emitted by `cook-luagen` into the
/// `__params` metadata table. Only `required` and `defaulted_string`
/// are supported in this build; other kinds (from future Tasks 5/6)
/// are rejected at registration time with a runtime error.
#[derive(Debug, Clone)]
pub enum ChoreParamMeta {
    /// A required positional — must be supplied by argv.
    Required { name: String },
    /// A defaulted positional — falls back to `default` when argv
    /// is exhausted at this position.
    DefaultedString { name: String, default: String },
}

impl ChoreParamMeta {
    /// The parameter name (for binding into the Lua table).
    pub fn param_name(&self) -> &str {
        match self {
            ChoreParamMeta::Required { name } => name,
            ChoreParamMeta::DefaultedString { name, .. } => name,
        }
    }
}

#[derive(Debug)]
pub struct RegisteredMetadata {
    pub ingredients: Vec<String>,
    pub excludes: Vec<String>,
    pub requires: Vec<String>,
    /// Ordered list of declared chore parameters. Empty for normal
    /// recipes (which do not take parameters).
    pub params: Vec<ChoreParamMeta>,
}

/// Parse the (ingredients, excludes, requires) string-list fields from a
/// Lua metadata table. Missing or non-table values yield empty vectors;
/// individual non-string entries are silently skipped (matches the historical
/// inline parser in `cook.recipe`).
///
/// Shared by `cook.recipe`, `cook.__register_surface`, and
/// `cook.__register_surface_chore` so the three registration paths see
/// identical metadata semantics.
fn parse_meta_lists(meta: &LuaTable) -> LuaResult<(Vec<String>, Vec<String>, Vec<String>)> {
    let mut ingredients = Vec::new();
    if let Ok(t) = meta.get::<LuaTable>("ingredients") {
        for pair in t.sequence_values::<String>() {
            if let Ok(s) = pair {
                ingredients.push(s);
            }
        }
    }
    let mut excludes = Vec::new();
    if let Ok(t) = meta.get::<LuaTable>("excludes") {
        for pair in t.sequence_values::<String>() {
            if let Ok(s) = pair {
                excludes.push(s);
            }
        }
    }
    let mut requires = Vec::new();
    if let Ok(t) = meta.get::<LuaTable>("requires") {
        for pair in t.sequence_values::<String>() {
            if let Ok(s) = pair {
                requires.push(s);
            }
        }
    }
    Ok((ingredients, excludes, requires))
}

/// Extract the `__params` metadata array from a chore's registration table.
///
/// Returns an empty `Vec` when `meta` has no `__params` key (i.e. a recipe or
/// a chore with no declared parameters). Iterates the sequence and dispatches
/// on the `kind` string field:
///
/// - `"required"` → `ChoreParamMeta::Required { name }`
/// - `"defaulted_string"` → `ChoreParamMeta::DefaultedString { name, default }`
/// - anything else → runtime error (planned for Tasks 5/6)
fn parse_chore_params_meta(meta: &LuaTable) -> LuaResult<Vec<ChoreParamMeta>> {
    let params_tbl = match meta.get::<Option<LuaTable>>("__params")? {
        Some(t) => t,
        None => return Ok(Vec::new()),
    };
    let mut out = Vec::new();
    for pair in params_tbl.sequence_values::<LuaTable>() {
        let entry = pair?;
        let kind: String = entry.get("kind")?;
        let name: String = entry.get("name")?;
        match kind.as_str() {
            "required" => {
                out.push(ChoreParamMeta::Required { name });
            }
            "defaulted_string" => {
                let default: String = entry.get("default")?;
                out.push(ChoreParamMeta::DefaultedString { name, default });
            }
            other => {
                return Err(mlua::Error::runtime(format!(
                    "chore parameter kind '{other}' is not yet supported in this build"
                )));
            }
        }
    }
    Ok(out)
}

/// Install the register-phase `cook.*` API surface on the given Lua VM.
/// This is the whole namespace (recipe registration, capture-mode
/// `cook.exec`/`cook.sh`, etc.), not just `cook.recipe`.
pub fn install_cook_api(
    lua: &Lua,
    env_vars: Rc<RefCell<HashMap<String, String>>>,
    working_dir: &PathBuf,
    body_slot: SharedBodySlot,
    recipe_name: &str,
) -> LuaResult<Rc<RefCell<Vec<RegisteredRecipe>>>> {
    let recipes: Rc<RefCell<Vec<RegisteredRecipe>>> = Rc::new(RefCell::new(Vec::new()));
    let cook = lua.create_table()?;

    // cook.recipe(name, metadata, fn) — the public API.
    // Always tagged Dynamic; chores cannot be registered through this path.
    let recipes_clone = recipes.clone();
    let recipe_fn =
        lua.create_function(move |lua, (name, meta, func): (String, LuaTable, LuaFunction)| {
            let key = lua.create_registry_value(func)?;
            let (ingredients, excludes, requires) = parse_meta_lists(&meta)?;
            let line = caller_line_in_cookfile(lua).unwrap_or(0);

            recipes_clone.borrow_mut().push(RegisteredRecipe {
                name,
                function: key,
                metadata: RegisteredMetadata {
                    ingredients,
                    excludes,
                    requires,
                    params: vec![],
                },
                source: RegistrationSource::Dynamic { line },
                kind: RecipeKind::Recipe,
            });
            Ok(())
        })?;
    cook.set("recipe", recipe_fn)?;

    // cook.__register_surface(name, meta, body) — codegen-private API.
    //
    // Emitted by `cook-luagen` for surface `recipe NAME` blocks. Distinct
    // from `cook.recipe` (which tags Dynamic) so collision diagnostics can
    // identify a surface declaration vs. a register-phase Lua call by source
    // kind, not just by line. The `__line = N` field in `meta` carries the
    // Cookfile source line of the surface block; the Lua call-stack walk
    // used by `cook.recipe` is not the right answer here because the codegen
    // splices into the top-level chunk and the call site line is the
    // generated chunk line, not the Cookfile source line.
    //
    // Not part of the public Cook Lua API (CS-0077 §6.4 implementation note).
    let recipes_surface = recipes.clone();
    let surface_fn = lua.create_function(
        move |lua, (name, meta, func): (String, LuaTable, LuaFunction)| {
            let key = lua.create_registry_value(func)?;
            // `__line` is always written by codegen (`generate_metadata_with_line`).
            // The `unwrap_or(0)` is defensive — a hand-typed
            // `cook.__register_surface` call without the field would land 0,
            // matching the legacy `cook.recipe` "no line info" sentinel.
            let line: usize = meta.get("__line").unwrap_or(0);
            let (ingredients, excludes, requires) = parse_meta_lists(&meta)?;
            recipes_surface.borrow_mut().push(RegisteredRecipe {
                name,
                function: key,
                metadata: RegisteredMetadata {
                    ingredients,
                    excludes,
                    requires,
                    params: vec![],
                },
                source: RegistrationSource::Static { line },
                kind: RecipeKind::Recipe,
            });
            Ok(())
        },
    )?;
    cook.set(REGISTER_SURFACE_NAME, surface_fn)?;

    // cook.__register_surface_chore(name, meta, body) — codegen-private API.
    //
    // Same shape as `cook.__register_surface` but tagged `RecipeKind::Chore`.
    // Emitted by `cook-luagen` for surface `chore NAME` blocks. Chores have
    // no `ingredients`/`excludes` (parser guarantees), but the helper parses
    // them defensively to keep one code path for metadata extraction.
    let recipes_chore = recipes.clone();
    let chore_fn = lua.create_function(
        move |lua, (name, meta, func): (String, LuaTable, LuaFunction)| {
            let key = lua.create_registry_value(func)?;
            let line: usize = meta.get("__line").unwrap_or(0);
            let (ingredients, excludes, requires) = parse_meta_lists(&meta)?;
            let params = parse_chore_params_meta(&meta)?;
            recipes_chore.borrow_mut().push(RegisteredRecipe {
                name,
                function: key,
                metadata: RegisteredMetadata {
                    ingredients,
                    excludes,
                    requires,
                    params,
                },
                source: RegistrationSource::Static { line },
                kind: RecipeKind::Chore,
            });
            Ok(())
        },
    )?;
    cook.set(REGISTER_SURFACE_CHORE_NAME, chore_fn)?;

    // cook.exec(cmd, line) — capture mode
    let body_slot_exec = body_slot.clone();
    let exec_fn = lua.create_function(move |_, (cmd, line): (String, usize)| {
        let mut slot = body_slot_exec.borrow_mut();
        let body = slot.as_mut().ok_or_else(|| {
            mlua::Error::runtime("cook.exec called outside a recipe body")
        })?;
        if body.inside_layer {
            body.layer_commands.push((cmd, line));
        } else {
            let unit = CapturedUnit {
                payload: WorkPayload::Shell {
                    cmd: cmd.clone(),
                    line,
                },
                cache_meta: None,
                dep_kind: DepKind::Sequential,
                probes: vec![],
            };
            body.units.push(unit);
        }
        Ok("".to_string())
    })?;
    cook.set("exec", exec_fn)?;

    // cook.interactive(cmd, line) — capture mode
    let body_slot_i = body_slot.clone();
    let interactive_capture_fn = lua.create_function(move |_, (cmd, line): (String, usize)| {
        let mut slot = body_slot_i.borrow_mut();
        let body = slot.as_mut().ok_or_else(|| {
            mlua::Error::runtime("cook.interactive called outside a recipe body")
        })?;
        let unit = CapturedUnit {
            payload: WorkPayload::Interactive {
                cmd: cmd.clone(),
                line,
                is_chore: false,
            },
            cache_meta: None,
            dep_kind: DepKind::Sequential,
            probes: vec![],
        };
        body.units.push(unit);
        Ok("".to_string())
    })?;
    cook.set("interactive", interactive_capture_fn)?;

    // cook.sh(cmd) — capture mode: inside a layer it captures like exec;
    // outside a layer it actually executes (user-facing utility that returns stdout).
    //
    // cook.sh has a long-standing top-level use as a utility (e.g. version
    // detection in module init code that returns the stdout). When called
    // without an active body slot, behave as the "execute immediately" path:
    // there is no layer context outside a body anyway, so this preserves the
    // existing surface. Inside a body, the layer check applies as before.
    let body_slot_sh = body_slot.clone();
    let wd_sh = working_dir.clone();
    let env_sh = env_vars.clone();
    let sh_recipe_name = recipe_name.to_string();
    let sh_fn = lua.create_function(move |_, cmd: String| {
        {
            let mut slot = body_slot_sh.borrow_mut();
            if let Some(body) = slot.as_mut() {
                if body.inside_layer {
                    body.layer_commands.push((cmd, 0));
                    return Ok("".to_string());
                }
            }
        }
        // Execute immediately — cook.sh is a user-facing utility
        // and callers depend on its return value for control flow.
        let env_snapshot = env_sh.borrow();
        run_shell_command(&cmd, &wd_sh, &env_snapshot, 0, &sh_recipe_name)
    })?;
    cook.set("sh", sh_fn)?;

    // cook.env table (initial population; may be mutated by config dispatch)
    let env_table = lua.create_table()?;
    {
        let snap = env_vars.borrow();
        for (key, value) in snap.iter() {
            env_table.set(key.as_str(), value.as_str())?;
        }
    }
    cook.set("env", env_table)?;

    lua.globals().set("cook", cook)?;
    Ok(recipes)
}

/// Upper bound on the Lua call-stack walk in `caller_line_in_cookfile`.
/// A safety cap — 40 frames comfortably exceeds any realistic Cookfile
/// call chain; the early `None` return on missing frames is the
/// expected termination.
const MAX_LUA_STACK_DEPTH: usize = 40;

/// Walk the Lua call stack and return the line number of the topmost frame
/// whose source string matches the Cookfile path label set by
/// `__cook_cookfile_path` (or any module loaded via `module_loader` with a
/// `@{module_path}` chunk name that ends with the cookfile-relative label).
/// Returns `None` if the Cookfile frame can't be located.
///
/// Used by `cook.recipe` to tag each `RegisteredRecipe` with the line number
/// of the user-code site that registered it. When the registry value isn't
/// populated (legacy/test call sites) or the matching frame can't be found,
/// callers default to `line = 0`.
fn caller_line_in_cookfile(lua: &Lua) -> Option<usize> {
    let target: String = lua
        .named_registry_value::<String>("__cook_cookfile_path")
        .ok()?;

    // Lua call levels: 1 = the closure, 2 = the caller, 3+ = caller's caller, ...
    for level in 1..MAX_LUA_STACK_DEPTH {
        match lua.inspect_stack(level) {
            None => return None,
            Some(dbg) => {
                let src_opt = dbg.source().source;
                let source: &str = src_opt.as_deref().unwrap_or("");
                // Module-loaded chunks have an "@" prefix (see module_loader.rs); the
                // `__cook_cookfile_path` registry value does not. Match either form.
                if source == target || source.ends_with(&target) {
                    return Some(dbg.curr_line() as usize);
                }
            }
        }
    }
    None
}

/// Maximum bytes per captured stream included in a COOK_CMD_FAILED error
/// message. Keep in sync with cook-luaotp's `COOK_CMD_FAIL_STREAM_CAP`.
const COOK_CMD_FAIL_STREAM_CAP: usize = 64 * 1024;

fn truncate_captured_stream(stream: &[u8]) -> String {
    if stream.is_empty() {
        return String::new();
    }
    let head_slice = if stream.len() > COOK_CMD_FAIL_STREAM_CAP {
        &stream[..COOK_CMD_FAIL_STREAM_CAP]
    } else {
        stream
    };
    let mut head = String::from_utf8_lossy(head_slice).into_owned();
    if stream.len() > COOK_CMD_FAIL_STREAM_CAP {
        if !head.ends_with('\n') {
            head.push('\n');
        }
        head.push_str(&format!(
            "... ({} bytes truncated)\n",
            stream.len() - COOK_CMD_FAIL_STREAM_CAP
        ));
    }
    head
}

/// Build the canonical COOK_CMD_FAILED error string with captured streams
/// inlined. Mirrors the helper in cook-luaotp's pool.rs; duplicated here
/// to avoid creating a cross-crate dep edge for a 30-line formatter. The
/// first line preserves the legacy `COOK_CMD_FAILED:<line>:<code>:<cmd>`
/// shape so the parser at `cook-cli/src/pipeline.rs:348` continues to
/// extract line/code while flowing the trailing captured streams through
/// to the user.
fn format_cmd_failed(line: usize, code: i32, cmd: &str, stdout: &[u8], stderr: &[u8]) -> String {
    let mut msg = format!("COOK_CMD_FAILED:{line}:{code}:{cmd}");
    let stdout_str = truncate_captured_stream(stdout);
    if !stdout_str.is_empty() {
        msg.push_str("\n--- stdout ---\n");
        msg.push_str(&stdout_str);
        if !msg.ends_with('\n') {
            msg.push('\n');
        }
    }
    let stderr_str = truncate_captured_stream(stderr);
    if !stderr_str.is_empty() {
        msg.push_str("--- stderr ---\n");
        msg.push_str(&stderr_str);
    }
    msg
}

fn run_shell_command(
    cmd: &str,
    wd: &std::path::Path,
    env: &HashMap<String, String>,
    _line: usize,
    _recipe_name: &str,
) -> mlua::Result<String> {
    let mut child_env: HashMap<String, String> = std::env::vars().collect();
    for (k, v) in env {
        child_env.insert(k.clone(), v.clone());
    }

    let output = Command::new("/bin/sh")
        .arg("-c")
        .arg(cmd)
        .current_dir(wd)
        .envs(&child_env)
        .output()
        .map_err(|e| mlua::Error::runtime(format!("failed to execute: {e}")))?;

    if !output.status.success() {
        let code = output.status.code().unwrap_or(1);
        return Err(mlua::Error::runtime(format_cmd_failed(
            _line,
            code,
            cmd,
            &output.stdout,
            &output.stderr,
        )));
    }

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    Ok(stdout)
}
