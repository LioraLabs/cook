use mlua::prelude::*;
use std::collections::BTreeSet;
use std::path::Path;
use std::{cell::RefCell, rc::Rc};

use crate::capture::RegisteredRecipe;
use crate::{RegisterError, SharedBodySlot};

pub(crate) fn source_line_in_cookfile(lua: &Lua, generated_line: usize) -> usize {
    lua.named_registry_value::<LuaTable>(crate::SOURCE_LINE_MAP_REGISTRY_KEY)
        .ok()
        .and_then(|map| map.get(generated_line).ok())
        .unwrap_or(generated_line)
}

fn capture_materializer(lua: &Lua, produce: LuaFunction) -> LuaResult<String> {
    let json = lua.create_function(|_, value: LuaValue| {
        let value = cook_lua_stdlib::lua_to_json(&value)
            .map_err(|e| LuaError::runtime(format!("unsupported captured value: {e}")))?;
        serde_json::to_string(&value).map_err(LuaError::external)
    })?;
    let capture: LuaFunction = lua
        .load(
            r#"
return function(root, encode_json)
  local active = {}
  local seen_tables = {}
  local function reject_aliases(value, path)
    if type(value) == "table" then
      if seen_tables[value] then
        error(path .. ": shared/aliased mutable captures are unsupported", 0)
      end
      seen_tables[value] = true
      for key, child in pairs(value) do
        reject_aliases(key, path)
        reject_aliases(child, path)
      end
    elseif type(value) == "function" then
      if active[value] then return end
      active[value] = true
      local index = 1
      while true do
        local name, captured = debug.getupvalue(value, index)
        if not name then break end
        if name ~= "_ENV" then
          reject_aliases(captured, path .. " upvalue '" .. name .. "'")
        end
        index = index + 1
      end
      active[value] = nil
    end
  end
  local function quote_bytes(bytes)
    return '"' .. bytes:gsub('.', function(byte)
      return string.format('\\%03d', string.byte(byte))
    end) .. '"'
  end
  local function encode(value, path)
    if type(value) ~= "function" then
      local ok, json = pcall(encode_json, value)
      if not ok then error(path .. ": " .. tostring(json), 0) end
      return "cook.json_decode(" .. string.format("%q", json) .. ")"
    end
    if active[value] then error(path .. ": recursive captured functions are unsupported", 0) end
    active[value] = true
    local setup = {}
    local index = 1
    while true do
      local name, captured = debug.getupvalue(value, index)
      if not name then break end
      if name ~= "_ENV" then
        setup[#setup + 1] = "assert(debug.setupvalue(f," .. index .. "," ..
          encode(captured, path .. " upvalue '" .. name .. "'") .. "))"
      end
      index = index + 1
    end
    active[value] = nil
    return "(function() local f=assert(load(" .. quote_bytes(string.dump(value, true)) ..
      "));" .. table.concat(setup, ";") .. ";return f end)()"
  end
  reject_aliases(root, "produce")
  return "return " .. encode(root, "produce") .. "()"
end
"#,
        )
        .eval()?;
    capture
        .call((produce, json))
        .map_err(|e| LuaError::runtime(format!("cook.materialize: cannot capture producer: {e}")))
}

pub fn register_materialize_api(
    lua: &Lua,
    working_dir: &Path,
    workspace_root: &Path,
    qualified_prefix: &str,
    cookfile_label: &str,
    cache_ctx: Option<&std::sync::Arc<cook_cache::CacheContext>>,
    runner: Option<crate::MaterializeRunner>,
    module_state: crate::module_loader::SharedModuleLoaderState,
) -> Result<(), RegisterError> {
    use cook_cache::recipe_namespace;
    use std::collections::{BTreeMap, BTreeSet};

    let cook: LuaTable = lua.globals().get("cook")?;
    let cook_api = cook.clone();
    let source_line_map = lua.create_table()?;
    lua.set_named_registry_value(crate::SOURCE_LINE_MAP_REGISTRY_KEY, source_line_map)?;
    let source_line_map_fn = lua.create_function(move |lua, map: LuaTable| {
        let stored = lua.create_table()?;
        for pair in map.pairs::<usize, usize>() {
            let (generated, source) = pair?;
            stored.set(generated, source)?;
        }
        lua.set_named_registry_value(crate::SOURCE_LINE_MAP_REGISTRY_KEY, stored)?;
        Ok(())
    })?;
    cook.set(
        cook_contracts::registration::SOURCE_LINE_MAP_NAME,
        source_line_map_fn,
    )?;
    let wd = working_dir.to_path_buf();
    let root = workspace_root.to_path_buf();
    let prefix = qualified_prefix.to_string();
    let cookfile = cookfile_label.to_string();
    let ctx = cache_ctx.cloned();
    let runner = runner.clone();
    let f = lua.create_function(move |lua, (key, spec, produce): (LuaValue, LuaValue, LuaValue)| {
        let key = match key {
            LuaValue::String(s) if !s.as_bytes().is_empty() => s.to_string_lossy().to_string(),
            LuaValue::String(_) => return Err(LuaError::runtime("cook.materialize: `key` must be non-empty")),
            v => return Err(LuaError::runtime(format!("cook.materialize: `key` must be a string, got {}", v.type_name()))),
        };
        let (caller_source, mut call_line) = lua
            .inspect_stack(1)
            .map(|frame| {
                (
                    frame
                        .source()
                        .source
                        .map(|source| source.into_owned())
                        .unwrap_or_else(|| cookfile.clone()),
                    frame.curr_line().max(0) as usize,
                )
            })
            .unwrap_or_else(|| (cookfile.clone(), 0));
        if caller_source == cookfile || caller_source == format!("@{cookfile}") {
            call_line = source_line_in_cookfile(lua, call_line);
        }
        let module = module_state
            .borrow()
            .module_for_source(&caller_source)
            .map(str::to_owned);
        let local = module
            .as_deref()
            .map_or_else(|| key.clone(), |module| format!("{module}.{key}"));
        let qualified = cook_contracts::naming::qualified_name(&prefix, &local);
        let declaration_site = format!("{caller_source}:{call_line}");
        let diagnostic = |cause: String| {
            LuaError::runtime(format!(
                "cook.materialize {key} ({qualified}) declared at {declaration_site}: {cause}"
            ))
        };
        let spec = match spec {
            LuaValue::Table(t) => t,
            v => {
                return Err(diagnostic(format!(
                    "`spec` must be a table, got {}",
                    v.type_name()
                )))
            }
        };
        let produce = match produce {
            LuaValue::Function(f) => f,
            v => {
                return Err(diagnostic(format!(
                    "`produce` must be a function, got {}",
                    v.type_name()
                )))
            }
        };
        for field in ["requires", "dependencies", "deps"] {
            if !spec
                .get::<LuaValue>(field)
                .map_err(|e| diagnostic(e.to_string()))?
                .is_nil()
            {
                return Err(diagnostic(
                    "materializers cannot depend on recipes or other materializers".into(),
                ));
            }
        }
        let list = |field: &str| -> LuaResult<Vec<String>> {
            match spec
                .get::<LuaValue>(field)
                .map_err(|e| diagnostic(e.to_string()))?
            {
                LuaValue::Nil => Ok(Vec::new()),
                LuaValue::Table(t) => t
                    .sequence_values::<String>()
                    .collect::<LuaResult<Vec<_>>>()
                    .map_err(|e| {
                        diagnostic(format!("`{field}` must be a list of strings: {e}"))
                    }),
                v => Err(diagnostic(format!(
                    "`{field}` must be a list of strings, got {}",
                    v.type_name()
                ))),
            }
        };

        let mut determinants = Vec::new();
        let mut declared_inputs = BTreeSet::new();
        let mut resolved_inputs = Vec::new();
        let mut files = BTreeSet::new();
        for pattern in list("files")? {
            files.extend(
                cook_cache::resolve_gather_glob(&wd, &root, &pattern)
                    .map_err(|e| diagnostic(e.to_string()))?,
            );
            declared_inputs.insert(
                (if pattern.starts_with("//") { &root } else { &wd }).join(
                    cook_cache::normalize_glob_pattern(
                        pattern.strip_prefix("//").unwrap_or(&pattern),
                    )
                    .as_ref(),
                ),
            );
        }
        for file in files {
            let path = std::fs::canonicalize(wd.join(&file))
                .map_err(|e| diagnostic(format!("input {file:?} cannot be read: {e}")))?;
            let hash = cook_cache::hash_file(&path)
                .ok_or_else(|| diagnostic(format!("input {file:?} cannot be read")))?;
            determinants.push(cook_cache::hash_str(&format!("file\0{file}\0{hash}")));
            resolved_inputs.push(path);
        }
        for tool in list("tools")? {
            let (hash, _) = cook_cache::tool_identity(&tool)
                .ok_or_else(|| diagnostic(format!("tool {tool:?} was not found")))?;
            determinants.push(cook_cache::hash_str(&format!("tool\0{tool}\0{hash}")));
        }
        let var: LuaTable = lua
            .globals()
            .get("var")
            .map_err(|e| diagnostic(e.to_string()))?;
        let mut env = BTreeMap::new();
        for name in list("env")? {
            let value: LuaValue = var
                .get(name.clone())
                .map_err(|e| diagnostic(e.to_string()))?;
            env.insert(
                name,
                crate::var_api::var_to_string(
                    cook_contracts::registration::MATERIALIZER_KIND,
                    &value,
                )
                .map_err(|e| diagnostic(e.to_string()))?,
            );
        }
        for seal in list("seals")? {
            let probes: LuaTable = cook_api
                .get("probes")
                .map_err(|e| diagnostic(e.to_string()))?;
            let get: LuaFunction = probes.get("get").map_err(|e| diagnostic(e.to_string()))?;
            let value: LuaValue = get
                .call(seal.clone())
                .map_err(|e| diagnostic(format!("seal {seal:?}: {e}")))?;
            let value = cook_lua_stdlib::lua_to_json(&value)
                .map_err(|e| diagnostic(format!("seal {seal:?}: {e}")))?;
            determinants.push(cook_cache::hash_str(&format!(
                "seal\0{seal}\0{}",
                serde_json::to_string(&value).unwrap()
            )));
        }
        determinants.sort_unstable();
        let env_hash = ctx
            .as_ref()
            .map(|c| cook_cache::env_contribution(&env, &c.denylist))
            .unwrap_or_else(|| cook_cache::hash_env(&env));
        let produce_source =
            capture_materializer(lua, produce).map_err(|e| diagnostic(e.to_string()))?;
        let mut body = std::io::Cursor::new(produce_source.as_bytes());
        let body_hash = cook_cache::hash_reader(&mut body).unwrap();
        let name = recipe_namespace(
            ctx.as_ref().map_or("", |c| c.project_id.as_str()),
            &cookfile,
            &format!("@materialize/{qualified}"),
        );
        let project_root = ctx
            .as_ref()
            .map_or(wd.as_path(), |c| c.project_root.as_path());
        let request = crate::MaterializeRequest {
            produce_source,
            key: key.clone(),
            qualified_key: qualified.clone(),
            declaration_site: declaration_site.clone(),
            declared_inputs: declared_inputs.into_iter().collect(),
            resolved_inputs,
            working_dir: wd.clone(),
            project_root: project_root.to_path_buf(),
            cache_ctx: ctx.clone(),
            recipe_namespace: name,
            command_hash: body_hash,
            env_contribution: env_hash,
            input_content_hashes: determinants,
            consulted_env_keys: env.keys().cloned().collect(),
        };
        let result = runner
            .as_ref()
            .ok_or_else(|| diagnostic("no materialization runner".into()))?(request)
            .map_err(diagnostic)?;
        let json = cook_contracts::probe_value::decode_json(&result.bytes).map_err(|e| {
            diagnostic(format!("cached or produced value is invalid: {e}"))
        })?;
        cook_lua_stdlib::json_codec::json_to_lua(lua, &json)
            .map_err(|e| diagnostic(format!("cached or produced value cannot be returned: {e}")))
    })?;
    cook.set("materialize", f)?;
    Ok(())
}

#[cfg(test)]
mod materializer_identity_tests {
    #[test]
    fn prefix_and_local_key_have_an_unambiguous_boundary() {
        let left = cook_contracts::naming::qualified_name("a", "bc");
        let right = cook_contracts::naming::qualified_name("ab", "c");
        assert_eq!(left, "a.bc");
        assert_eq!(right, "ab.c");
        assert_ne!(left, right);
    }
}

/// Set up the `recipe` global table with name and resolved input files.
/// No cache operations — cache evaluation is handled by cook-engine.
pub fn setup_recipe_context(
    lua: &Lua,
    recipe: &RegisteredRecipe,
    working_dir: &Path,
    workspace_root: &Path,
    warnings: &Rc<RefCell<Vec<String>>>,
) -> Result<(), RegisterError> {
    // Build recipe context table
    let recipe_table = lua.create_table()?;
    recipe_table.set("name", recipe.name.as_str())?;

    // Resolve exclude patterns into a set for fast lookup
    let mut excluded: BTreeSet<String> = BTreeSet::new();
    for pattern in &recipe.metadata.excludes {
        excluded.extend(
            cook_cache::resolve_gather_glob(working_dir, workspace_root, pattern)
                .map_err(mlua::Error::runtime)?,
        );
    }

    // Build inputs table by resolving glob patterns, minus excludes
    let gather_table = lua.create_table()?;
    for (i, pattern) in recipe.metadata.inputs.iter().enumerate() {
        let files = cook_cache::resolve_gather_glob(working_dir, workspace_root, pattern)
            .map_err(mlua::Error::runtime)?;
        if files.is_empty() {
            warnings.borrow_mut().push(format!(
                "input {pattern:?} matched 0 files (recipe {})",
                recipe.name
            ));
        }
        let filtered: BTreeSet<String> = files
            .into_iter()
            .filter(|f| !excluded.contains(f))
            .collect();
        let files_table = lua.create_table()?;
        for (idx, file) in filtered.iter().enumerate() {
            files_table.set(idx + 1, file.as_str())?;
        }
        gather_table.set(i + 1, files_table)?;
    }
    recipe_table.set("inputs", gather_table)?;

    lua.globals().set("recipe", recipe_table)?;
    Ok(())
}

/// Register `cook.resolve_gather(includes, excludes)` on the cook global table.
/// Returns a flat Lua table of relative file paths after glob+exclude resolution.
pub fn register_resolve_gather(
    lua: &Lua,
    working_dir: &Path,
    workspace_root: &Path,
) -> Result<(), RegisterError> {
    let cook: LuaTable = lua.globals().get("cook")?;
    let wd = working_dir.to_path_buf();
    let root = workspace_root.to_path_buf();
    let resolve_fn =
        lua.create_function(move |lua, (includes, excludes): (LuaTable, LuaTable)| {
            // Collect exclude patterns and resolve them
            let mut excluded: BTreeSet<String> = BTreeSet::new();
            for exc in excludes.sequence_values::<String>() {
                let pattern = exc.map_err(|e| mlua::Error::runtime(format!("bad exclude: {e}")))?;
                excluded.extend(
                    cook_cache::resolve_gather_glob(&wd, &root, &pattern)
                        .map_err(mlua::Error::runtime)?,
                );
            }

            // Resolve include patterns, filtering out excludes
            let mut result: Vec<String> = Vec::new();
            for inc in includes.sequence_values::<String>() {
                let pattern = inc.map_err(|e| mlua::Error::runtime(format!("bad include: {e}")))?;
                let files = cook_cache::resolve_gather_glob(&wd, &root, &pattern)
                    .map_err(mlua::Error::runtime)?;
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
    cook.set("resolve_gather", resolve_fn)?;
    Ok(())
}

/// Register `cook.recipe_name()` on the cook global table (Standard §22.7).
///
/// Returns the enclosing recipe's fully-qualified name — the same value
/// `cook.add_test` already reads for its `suite` default (Standard §22.4).
/// Unlike that default, which degrades a missing `current_recipe` to an
/// empty string via `.unwrap_or_default()` (`test_api.rs`), this hard
/// errors: an empty name would silently corrupt any caller folding it into
/// a path or identifier (`lib.a`, `build/obj//`).
pub fn register_recipe_name_api(lua: &Lua, body_slot: SharedBodySlot) -> Result<(), RegisterError> {
    let cook: LuaTable = lua.globals().get("cook")?;
    let recipe_name_fn = lua.create_function(move |_, ()| {
        let slot = body_slot.borrow();
        // Only the body loop opens the slot, so `None` covers every
        // outside-a-body caller at once — top level, `config`/`register`
        // blocks, and a member-source probe's `produce` (the prepass
        // runs before the body loop). No probe-specific guard needed.
        slot.as_ref()
            .and_then(|body| body.current_recipe.clone())
            .ok_or_else(|| {
                mlua::Error::runtime(
                    "cook.recipe_name: no enclosing recipe is active; call `cook.recipe_name()` only from inside a recipe body (Standard \u{00a7}22.7, CS-0141)",
                )
            })
    })?;
    cook.set("recipe_name", recipe_name_fn)?;
    Ok(())
}

/// Forces the named recipe's body to be evaluated to completion, and
/// reports the failure as a Lua error if it cannot be.
///
/// Supplied by `engine.rs`'s body-invocation driver, which owns the
/// re-entrant visit. An `Rc` rather than a `Box` so `require_recipe_fn` can
/// clone the forcer out of its cell and DROP the borrow before calling it:
/// forcing re-enters that same closure via the callee's body, which borrows
/// the cell again.
pub type RecipeForcer = Rc<dyn Fn(&Lua, &str) -> Result<(), mlua::Error>>;

/// The forcer cell `cook.require_recipe` reads at call time.
///
/// A CELL, not a value, because of an ordering fact that has already caused
/// one silent-failure bug: the top-level Lua chunk runs BEFORE the
/// body-invocation driver exists, so any forcer passed by value at
/// API-install time is necessarily forcer-less. Re-registering the function
/// once the driver exists does not fix that — `local rr = cook.require_recipe`
/// at top level is ordinary Lua (`local sh = cook.sh` is idiomatic), and the
/// alias keeps the closure it captured, forcer and all. ONE closure reading a
/// cell filled in later is what makes an alias and a fresh `cook.` lookup
/// behave identically.
///
/// Empty forever on VMs with no driver at all (`list_names`), and on the
/// `register_cookfile` VM until `engine.rs` fills it. Every call reachable
/// before then is outside a recipe body, so the guard rail fires first — but
/// an empty cell reached from INSIDE a body is a hard error, never a silent
/// no-op (§22.8: the forcing MUST NOT degrade to a silent skip).
pub type SharedRecipeForcer = Rc<RefCell<Option<RecipeForcer>>>;

/// Register `cook.require_recipe(name)` on the cook global table (Standard
/// §22.8, CS-0144).
///
/// Declares that the enclosing recipe's register-phase body depends on
/// another recipe, named BARE — the same unqualified namespace the
/// `requires` metadata field and the surface `recipe A : B` dep-list use.
/// Two effects, in order: `force` evaluates `name`'s body to completion
/// (the register-order guarantee — everything that body exports is
/// observable once this call returns), then the name accumulates into
/// `BodyCaptureState.dynamic_requires` (order-preserving, de-duplicated),
/// which `engine.rs` merges into the caller's `requires` at body drain.
///
/// Sibling to `register_recipe_name_api` immediately above: same
/// registration pattern, same "only inside a recipe body" error voice,
/// reading the same `body_slot`/`current_recipe` `None` signal — one check
/// covers every outside-a-body caller (top level, a `register` block, and
/// a member-source probe's `produce` on the register VM).
pub fn register_require_recipe_api(
    lua: &Lua,
    body_slot: SharedBodySlot,
    force: SharedRecipeForcer,
) -> Result<(), RegisterError> {
    let cook: LuaTable = lua.globals().get("cook")?;
    let require_recipe_fn = lua.create_function(move |lua, name: LuaValue| {
        // Validate against the caller's body state, then drop the borrow
        // *before* forcing: `force` re-enters this very closure via the
        // callee's body, and swaps the body slot out from under us.
        let name = {
            let slot = body_slot.borrow();
            let body = match slot.as_ref() {
                Some(body) if body.current_recipe.is_some() => body,
                _ => {
                    return Err(mlua::Error::runtime(
                        "cook.require_recipe: no enclosing recipe is active; call `cook.require_recipe(name)` only from inside a recipe body (Standard \u{00a7}22.8, CS-0144)",
                    ))
                }
            };

            // CS-0143's `parse_origin_meta` precedent: match on the raw Lua
            // value so a numeric argument is rejected outright rather than
            // coerced to its decimal string.
            let name = match name {
                LuaValue::String(s) => s.to_string_lossy().to_string(),
                other => {
                    return Err(mlua::Error::runtime(format!(
                        "cook.require_recipe: `name` must be a string, got {} (Standard \u{00a7}22.8, CS-0144)",
                        other.type_name()
                    )))
                }
            };
            if name.is_empty() {
                return Err(mlua::Error::runtime(
                    "cook.require_recipe: `name` must be a non-empty string, got an empty string (Standard \u{00a7}22.8, CS-0144)",
                ));
            }

            // Bare-to-bare self-reference check. `current_recipe_bare` is
            // stamped alongside `current_recipe` at the same point in
            // `engine.rs`, so it is guaranteed `Some` here — see its doc on
            // why comparing against `current_recipe` (qualified) instead
            // would silently never fire under an import prefix.
            let current_bare = body
                .current_recipe_bare
                .clone()
                .expect("current_recipe_bare stamped alongside current_recipe");
            if current_bare == name {
                return Err(mlua::Error::runtime(format!(
                    "cook.require_recipe: recipe \"{name}\" cannot require itself (Standard \u{00a7}22.8, CS-0144)"
                )));
            }
            name
        };

        // The register-order guarantee. Unconditional even for a name
        // already in `dynamic_requires` — the driver's visit map is what
        // makes a repeat call a no-op, and routing every call through it
        // keeps "already evaluated" one decision in one place.
        //
        // Cloned out of the cell so the borrow is released before the call:
        // `force` re-enters this closure via the callee's body, which reads
        // the cell again.
        let forcer = force.borrow().clone();
        match forcer {
            Some(force) => force(lua, &name)?,
            // Unreachable via any path that exists today — the guard rail
            // above already rejected every outside-a-body caller, and inside
            // a body the driver has long since filled the cell. It is an
            // error rather than a `()` because the silent alternative is the
            // exact defect this API exists to eliminate: the edge below would
            // still be recorded, so the recipe would join the build closure
            // while its export stayed unobservable and `cook.import` returned
            // nil. A future VM that installs the API without a driver must
            // fail loudly here, not mislink.
            None => {
                return Err(mlua::Error::runtime(format!(
                    "cook.require_recipe: cannot force recipe \"{name}\": no body-invocation \
                     driver is available on this register-phase VM. This is an implementation \
                     fault, not a Cookfile error; please report it (Standard \u{00a7}22.8, CS-0144)"
                )))
            }
        }

        // The slot the driver restored on the way out is the caller's
        // again, so this lands the edge on the right recipe.
        let mut slot = body_slot.borrow_mut();
        let body = slot
            .as_mut()
            .expect("body slot restored by the driver before force returns");
        if !body.dynamic_requires.contains(&name) {
            body.dynamic_requires.push(name);
        }
        Ok(())
    })?;
    cook.set("require_recipe", require_recipe_fn)?;
    Ok(())
}

/// Make `cook.import` force the referent's body when called from inside a
/// recipe body.
///
/// Forcing is a property of READING an export, not of declaring an edge. A
/// maker calls `cook.import(N)` at exactly the moment it needs `N`'s body to
/// have run; before this, it had to arrange that separately — `cook_cc` forced
/// its whole `links` list up front, inside an empty `cook.step_group` whose
/// only job was to discard the references the forcing call recorded. Folding
/// the force into `import` deletes that dance: `resolve_links` calls `import`,
/// `import` forces, the export resolves.
///
/// Scope. The force fires only inside a recipe body. Outside one — a
/// `cook.on_register_complete` finalizer reading exports to emit a compile
/// database, say — the recipe set is closed and forcing is illegal (§22.9), so
/// `import` stays the plain store lookup it has always been. Already-exported
/// names skip the forcer entirely, which keeps the common case a single map hit
/// and makes the whole thing a no-op once a body has run.
///
/// A name with no export that is also not a registered recipe surfaces the
/// forcer's own unknown-recipe error rather than returning nil. That is the
/// point: a silent nil here is the mislink `cook.require_recipe`'s guard exists
/// to prevent, and `cook_cc`'s transitive walk would quietly emit a link line
/// missing the library.
pub fn register_import_forcing(
    lua: &Lua,
    body_slot: SharedBodySlot,
    force: SharedRecipeForcer,
    store: crate::export_api::SharedExportStore,
) -> Result<(), RegisterError> {
    let cook: LuaTable = lua.globals().get("cook")?;
    let raw: LuaFunction = cook.get("import")?;
    cook.set("__import_raw", raw)?;

    let import_fn = lua.create_function(move |lua, name: String| {
        let already_exported = store.borrow().contains_key(&name);
        let in_body = {
            let slot = body_slot.borrow();
            matches!(slot.as_ref(), Some(body) if body.current_recipe.is_some())
        };
        if !already_exported && in_body {
            // Drop every borrow before forcing: the forcer re-enters the
            // register APIs via the callee's body and swaps the body slot.
            let forcer = force.borrow().clone();
            if let Some(force) = forcer {
                force(lua, &name)?;
            }
        }
        let cook: LuaTable = lua.globals().get("cook")?;
        let raw: LuaFunction = cook.get("__import_raw")?;
        raw.call::<LuaValue>(name)
    })?;
    cook.set("import", import_fn)?;
    Ok(())
}

/// Give `cook.dep_order` `require_recipe`'s register-order guarantee.
///
/// COOK-297 (revised): `dep_order` is the fine-grained replacement for
/// `require_recipe`, not a companion to it. A module that resolves link
/// references needs the referent's body evaluated so `cook.import` returns its
/// export — that forcing was the ONLY reason `cook_cc` called
/// `require_recipe`, and the coarse whole-recipe barrier came along as an
/// unwanted side effect. Moving the force here lets a maker drop
/// `require_recipe` altogether: `dep_order` forces the body, records a
/// per-unit edge, and (via `RecipeInfo.orders`) establishes closure
/// membership — everything `require_recipe` did except manufacture the barrier.
///
/// Installed AFTER `register_dep_output_api`, which is what defines the
/// function being wrapped. The raw accumulator-only implementation is kept at
/// `cook.__dep_order_raw` and fetched per call rather than captured, so this
/// wrapper holds no Lua handle across registrations.
pub fn register_dep_order_forcing(
    lua: &Lua,
    body_slot: SharedBodySlot,
    force: SharedRecipeForcer,
) -> Result<(), RegisterError> {
    let cook: LuaTable = lua.globals().get("cook")?;
    let raw: LuaFunction = cook.get("dep_order")?;
    cook.set("__dep_order_raw", raw)?;

    let dep_order_fn = lua.create_function(move |lua, name: String| {
        // Validate against the caller's body state, then drop the borrow
        // before forcing — `force` re-enters register APIs via the callee's
        // body and swaps the body slot out from under us.
        {
            let slot = body_slot.borrow();
            match slot.as_ref() {
                Some(body) if body.current_recipe.is_some() => {}
                _ => {
                    return Err(mlua::Error::runtime(
                        "cook.dep_order must be called inside a recipe body",
                    ))
                }
            }
        }
        let forcer = force.borrow().clone();
        match forcer {
            Some(force) => force(lua, &name)?,
            None => {
                return Err(mlua::Error::runtime(format!(
                    "cook.dep_order: cannot force recipe \"{name}\": no \
                     body-invocation driver is installed"
                )))
            }
        }
        let cook: LuaTable = lua.globals().get("cook")?;
        let raw: LuaFunction = cook.get("__dep_order_raw")?;
        raw.call::<()>(name)
    })?;
    cook.set("dep_order", dep_order_fn)?;
    Ok(())
}

/// Resolve a glob pattern into a sorted set of relative file paths.
///
/// Matches whose final (symlink-resolved) metadata is a directory are
/// dropped (CS-0064): `recipe.inputs` and `cook.resolve_gather`
/// feed straight into `cook.add_unit` inputs, which CS-0063 already
/// rejects directory paths from. Filtering here keeps a glob like
/// `src/*` well-defined when `src/` contains sub-directories.
#[cfg(test)]
#[path = "tests/context_tests.rs"]
mod tests;
