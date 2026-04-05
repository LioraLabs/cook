use mlua::prelude::*;
use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Command;
use std::rc::Rc;
use std::cell::RefCell;

use cook_contracts::{CapturedUnit, DepKind, WorkPayload};

use crate::SharedCaptureState;

pub struct RegisteredRecipe {
    pub name: String,
    pub function: LuaRegistryKey,
    pub metadata: RegisteredMetadata,
}

#[derive(Debug)]
pub struct RegisteredMetadata {
    pub ingredients: Vec<String>,
    pub excludes: Vec<String>,
    pub requires: Vec<String>,
}

/// Register cook.* APIs in capture mode. Same recipe registration as normal,
/// but cook.exec/cook.sh capture instead of executing.
pub fn register_cook_api_capture(
    lua: &Lua,
    env_vars: Rc<RefCell<HashMap<String, String>>>,
    working_dir: &PathBuf,
    capture_state: SharedCaptureState,
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

            recipes_clone.borrow_mut().push(RegisteredRecipe {
                name,
                function: key,
                metadata: RegisteredMetadata {
                    ingredients,
                    excludes,
                    requires,
                },
            });
            Ok(())
        })?;
    cook.set("recipe", recipe_fn)?;

    // cook.exec(cmd, line) — capture mode
    let cs = capture_state.clone();
    let exec_fn = lua.create_function(move |_, (cmd, line): (String, usize)| {
        let mut state = cs.borrow_mut();
        if state.inside_layer {
            state.layer_commands.push((cmd, line));
        } else {
            let unit = CapturedUnit {
                payload: WorkPayload::Shell {
                    cmd: cmd.clone(),
                    line,
                },
                cache_meta: None,
                dep_kind: DepKind::Sequential,
            };
            state.units.push(unit);
        }
        Ok("".to_string())
    })?;
    cook.set("exec", exec_fn)?;

    // cook.interactive(cmd, line) — capture mode
    let cs_i = capture_state.clone();
    let interactive_capture_fn = lua.create_function(move |_, (cmd, line): (String, usize)| {
        let mut state = cs_i.borrow_mut();
        let unit = CapturedUnit {
            payload: WorkPayload::Interactive {
                cmd: cmd.clone(),
                line,
            },
            cache_meta: None,
            dep_kind: DepKind::Sequential,
        };
        state.units.push(unit);
        Ok("".to_string())
    })?;
    cook.set("interactive", interactive_capture_fn)?;

    // cook.sh(cmd) — capture mode: inside a layer it captures like exec;
    // outside a layer it actually executes (user-facing utility that returns stdout).
    let cs2 = capture_state.clone();
    let wd_sh = working_dir.clone();
    let env_sh = env_vars.clone();
    let sh_recipe_name = recipe_name.to_string();
    let sh_fn = lua.create_function(move |_, cmd: String| {
        let mut state = cs2.borrow_mut();
        if state.inside_layer {
            state.layer_commands.push((cmd, 0));
            drop(state);
            Ok("".to_string())
        } else {
            drop(state);
            // Execute immediately — cook.sh is a user-facing utility
            // and callers depend on its return value for control flow.
            let env_snapshot = env_sh.borrow();
            run_shell_command(&cmd, &wd_sh, &env_snapshot, 0, &sh_recipe_name)
        }
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
        return Err(mlua::Error::runtime(format!(
            "COOK_CMD_FAILED:{}:{}:{}",
            _line, code, cmd
        )));
    }

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    Ok(stdout)
}
