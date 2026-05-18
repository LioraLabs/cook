use mlua::prelude::*;
use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Command;
use std::rc::Rc;
use std::cell::RefCell;

use cook_contracts::{CapturedUnit, DepKind, WorkPayload};

use crate::SharedBodySlot;

/// Where a `RegisteredRecipe` came from. Used by Phase 2 collision detection
/// (and surface diagnostics) to name BOTH sites of a name conflict and
/// identify the kind of each.
///
/// - `Static`  — emitted by codegen from a surface `recipe NAME` block
///   via `cook.__register_surface(...)`. Wired in Phase 3.
/// - `Dynamic` — recorded by user / wrapper Lua code calling
///   `cook.recipe(...)` (e.g. `cook_cc.bin` target-makers).
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
}

#[derive(Debug)]
pub struct RegisteredMetadata {
    pub ingredients: Vec<String>,
    pub excludes: Vec<String>,
    pub requires: Vec<String>,
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

    // cook.recipe(name, metadata, fn) — same as normal
    let recipes_clone = recipes.clone();
    let recipe_fn =
        lua.create_function(move |lua, (name, meta, func): (String, LuaTable, LuaFunction)| {
            let key = lua.create_registry_value(func)?;

            let mut ingredients = Vec::new();
            if let Ok(ing_table) = meta.get::<LuaTable>("ingredients") {
                for pair in ing_table.sequence_values::<String>() {
                    if let Ok(s) = pair {
                        ingredients.push(s);
                    }
                }
            }

            let mut excludes = Vec::new();
            if let Ok(exc_table) = meta.get::<LuaTable>("excludes") {
                for pair in exc_table.sequence_values::<String>() {
                    if let Ok(s) = pair {
                        excludes.push(s);
                    }
                }
            }

            let mut requires = Vec::new();
            if let Ok(req_table) = meta.get::<LuaTable>("requires") {
                for pair in req_table.sequence_values::<String>() {
                    if let Ok(s) = pair {
                        requires.push(s);
                    }
                }
            }

            let line = caller_line_in_cookfile(lua).unwrap_or(0);

            recipes_clone.borrow_mut().push(RegisteredRecipe {
                name,
                function: key,
                metadata: RegisteredMetadata {
                    ingredients,
                    excludes,
                    requires,
                },
                source: RegistrationSource::Dynamic { line },
            });
            Ok(())
        })?;
    cook.set("recipe", recipe_fn)?;

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
    for level in 1..40 {
        match lua.inspect_stack(level) {
            None => return None,
            Some(dbg) => {
                let src_opt = dbg.source().source;
                let source: &str = src_opt.as_deref().unwrap_or("");
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
