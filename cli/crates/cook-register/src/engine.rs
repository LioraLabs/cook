use mlua::prelude::*;
use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::{Arc, Mutex};

use cook_contracts::RecipeUnits;

use crate::capture::register_cook_api_capture;
use crate::context::setup_recipe_context;
use crate::dep_output_api::SharedTerminalOutputs;
use crate::env_api::{install_require_env, EnvKeyset};
use crate::export_api::SharedExportStore;
use crate::module_loader::{ModuleLoaderState, SharedModuleLoaderState};
use crate::{CaptureState, RegisterError, SharedCaptureState};

pub struct Registry {
    working_dir: PathBuf,
    env_vars: Rc<RefCell<HashMap<String, String>>>,
    export_store: SharedExportStore,
    terminal_outputs: SharedTerminalOutputs,
    selected_config: Option<String>,
    qualified_prefix: String,
    alias_dirs: BTreeMap<String, PathBuf>,
    /// Per-alias canonical importee qualified prefix, used by `cook.dep_output`
    /// to resolve cross-Cookfile body refs to their workspace-global storage
    /// key. Distinct from `qualified_prefix` (which applies only to
    /// same-Cookfile name refs). Diamond imports require this — a Cookfile
    /// reachable via two import chains has one canonical storage prefix, and
    /// every importer's local alias must resolve to that same canonical
    /// prefix at lookup time.
    alias_qualified_prefixes: BTreeMap<String, String>,
    /// Frozen keyset of env-var names declared via config blocks.
    /// Shared between the Lua-env-construction call and the config-block
    /// evaluation call so both sides see the same Rc-backed set.
    env_keyset: EnvKeyset,
}

impl Registry {
    pub fn new(working_dir: PathBuf, env_vars: HashMap<String, String>) -> Self {
        Self {
            working_dir,
            env_vars: Rc::new(RefCell::new(env_vars)),
            export_store: Rc::new(RefCell::new(BTreeMap::new())),
            terminal_outputs: Arc::new(Mutex::new(BTreeMap::new())),
            selected_config: None,
            qualified_prefix: String::new(),
            alias_dirs: BTreeMap::new(),
            alias_qualified_prefixes: BTreeMap::new(),
            env_keyset: EnvKeyset::new(),
        }
    }

    pub fn with_selected_config(mut self, selected_config: Option<String>) -> Self {
        self.selected_config = selected_config;
        self
    }

    pub fn with_shared_terminal_outputs(mut self, shared: SharedTerminalOutputs) -> Self {
        self.terminal_outputs = shared;
        self
    }

    pub fn with_qualified_prefix(mut self, prefix: String) -> Self {
        self.qualified_prefix = prefix;
        self
    }

    pub fn with_alias_dirs(mut self, alias_dirs: BTreeMap<String, PathBuf>) -> Self {
        self.alias_dirs = alias_dirs;
        self
    }

    pub fn with_alias_qualified_prefixes(
        mut self,
        alias_qualified_prefixes: BTreeMap<String, String>,
    ) -> Self {
        self.alias_qualified_prefixes = alias_qualified_prefixes;
        self
    }

    pub fn working_dir(&self) -> &PathBuf {
        &self.working_dir
    }

    /// Run a recipe in "registration mode": execute the Lua recipe body with
    /// capture-mode APIs so that work units are recorded instead of executed.
    /// Returns a `RecipeUnits` describing what the recipe wants to do.
    ///
    /// `cache_ctx` is `None` during tests and legacy call sites that don't yet
    /// have a `CacheContext` available. When `Some`, it is set as app_data on
    /// the Lua VM so that `cook.add_unit` can access machine identity and the
    /// env denylist to populate real `CacheMeta` values.
    pub fn register_recipe(
        &self,
        lua_source: &str,
        recipe_name: &str,
        cache_ctx: Option<Arc<cook_cache::cache_ctx::CacheContext>>,
    ) -> Result<RecipeUnits, RegisterError> {
        let lua = Lua::new();
        let capture_state: SharedCaptureState = Rc::new(RefCell::new(CaptureState::new()));

        // Thread CacheContext into the Lua VM so cook.add_unit can compute
        // real context_hash and env_contribution values.
        if let Some(ref ctx) = cache_ctx {
            lua.set_app_data(ctx.clone());
            let cookfile_rel =
                cookfile_path_relative_to(&ctx.project_root, &self.working_dir.join("Cookfile"));
            lua.set_named_registry_value("__cook_cookfile_path", cookfile_rel)
                .map_err(RegisterError::Lua)?;
        }

        let recipes = register_cook_api_capture(
            &lua,
            self.env_vars.clone(),
            &self.working_dir,
            capture_state.clone(),
            recipe_name,
        )?;
        // Install cook.require_env immediately after the cook table is built.
        // The function closes over the env_table reference and the keyset;
        // the keyset is frozen after config-block evaluation completes.
        {
            let cook_tbl: LuaTable = lua.globals().get("cook")?;
            install_require_env(&lua, &cook_tbl, self.env_keyset.clone())?;
        }
        // `fs.*`, `path.*`, `cook.platform.*` come from the shared
        // cook-lua-stdlib crate (CS-0044) so register-phase and
        // execute-phase VMs see byte-identical behavior.
        cook_lua_stdlib::register_fs_api(
            &lua,
            cook_lua_stdlib::WorkingDirSource::Static(self.working_dir.clone()),
        )?;
        cook_lua_stdlib::register_path_api(&lua)?;

        // Module system APIs. `register_platform_api` now takes the
        // `cook` table directly rather than reading it from globals.
        {
            let cook_tbl: LuaTable = lua.globals().get("cook")?;
            cook_lua_stdlib::register_platform_api(&lua, &cook_tbl)?;
        }

        let module_state: SharedModuleLoaderState = Rc::new(RefCell::new(
            ModuleLoaderState::new(self.working_dir.clone()),
        ));
        crate::module_loader::register_module_loader(&lua, module_state.clone())?;
        crate::module_loader::register_cache_api(&lua, module_state.clone())?;
        crate::unit_api::register_unit_api(
            &lua,
            capture_state.clone(),
            recipe_name,
            self.terminal_outputs.clone(),
        )?;
        crate::export_api::register_export_api(&lua, self.export_store.clone())?;
        crate::test_api::register_test_api(&lua, capture_state.clone())?;
        crate::dep_output_api::register_dep_output_api(
            &lua,
            self.terminal_outputs.clone(),
            capture_state.clone(),
            self.alias_dirs.clone(),
            self.qualified_prefix.clone(),
            self.alias_qualified_prefixes.clone(),
        )?;
        crate::context::register_resolve_ingredients(&lua, &self.working_dir)?;
        crate::codec_api::register_codec_api(&lua)?;

        lua.load(lua_source).exec()?;

        // Config block dispatch: if codegen emitted __cook_run_config_blocks,
        // expose writable `env` alias, call it, then snapshot mutations back
        // into the shared env_vars map.
        if let Ok(dispatch) = lua.globals().get::<LuaFunction>("__cook_run_config_blocks") {
            // Expose `env` as an alias of cook.env for the block body to write.
            let cook_tbl: LuaTable = lua.globals().get("cook")?;
            let env_tbl: LuaTable = cook_tbl.get("env")?;
            lua.globals().set("env", env_tbl.clone())?;

            let name_arg: Option<String> = self.selected_config.clone();
            dispatch.call::<()>(name_arg)?;

            // Capture the post-evaluation env keyset as the declared set.
            // Must happen before the env global is removed so the table is
            // still accessible. freeze() is idempotent under union.
            self.env_keyset.freeze(&env_tbl)?;

            // Snapshot mutations from cook.env back into shared env_vars.
            {
                let mut env_map = self.env_vars.borrow_mut();
                for pair in env_tbl.pairs::<String, String>() {
                    let (k, v) = pair?;
                    env_map.insert(k, v);
                }
            }

            // Freeze: remove the `env` global from the recipe's view.
            lua.globals().set("env", mlua::Value::Nil)?;
        }

        let registry = recipes.borrow();
        let recipe = registry
            .iter()
            .find(|r| r.name == recipe_name)
            .ok_or_else(|| RegisterError::RecipeNotFound(recipe_name.to_string()))?;

        // Run recipe context setup (ingredient resolution, etc.)
        setup_recipe_context(&lua, recipe, &self.working_dir)?;

        // Execute recipe function — capture_state gets populated
        let func: LuaFunction = lua.registry_value(&recipe.function)?;
        func.call::<()>(())?;

        // Flush module caches
        module_state.borrow().flush_all();

        let deps = recipe.metadata.requires.clone();

        // Extract results from capture state
        let cap = capture_state.borrow();

        let terminal_outputs_list = cap.last_cook_step_outputs.clone();

        // Store terminal outputs so downstream recipes can call cook.dep_output().
        // Key is the fully-qualified name so cross-Cookfile lookups succeed
        // (e.g. "lib.lib_build" for a recipe in the "lib" Registry).
        let qualified_name = if self.qualified_prefix.is_empty() {
            recipe_name.to_string()
        } else {
            format!("{}.{}", self.qualified_prefix, recipe_name)
        };
        self.terminal_outputs
            .lock()
            .expect("terminal_outputs mutex poisoned")
            .insert(qualified_name, terminal_outputs_list.clone());

        // Convert HashMap env_vars to BTreeMap for RecipeUnits
        let env_btree: BTreeMap<String, String> = self.env_vars.borrow().iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();

        Ok(RecipeUnits {
            recipe_name: recipe_name.to_string(),
            deps,
            units: cap.units.clone(),
            step_groups: cap.step_groups.clone(),
            working_dir: self.working_dir.clone(),
            env_vars: env_btree,
            terminal_outputs: terminal_outputs_list,
            dep_edges: cap.dep_edges.clone(),
        })
    }
}

/// Compute a forward-slash, project-relative path for the Cookfile being
/// registered.  Falls back to the file name (or "Cookfile") on any failure.
fn cookfile_path_relative_to(project_root: &Path, abs: &Path) -> String {
    abs.strip_prefix(project_root)
        .ok()
        .map(|p| p.to_string_lossy().replace(std::path::MAIN_SEPARATOR, "/"))
        .unwrap_or_else(|| {
            abs.file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| "Cookfile".to_string())
        })
}
