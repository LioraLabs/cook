use mlua::prelude::*;
use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::rc::Rc;

use cook_contracts::RecipeUnits;

use crate::capture::register_cook_api_capture;
use crate::context::setup_recipe_context;
use crate::dep_output_api::SharedTerminalOutputs;
use crate::export_api::SharedExportStore;
use crate::module_loader::{ModuleLoaderState, SharedModuleLoaderState};
use crate::{CaptureState, RegisterError, SharedCaptureState};

pub struct Registry {
    working_dir: PathBuf,
    env_vars: Rc<RefCell<HashMap<String, String>>>,
    export_store: SharedExportStore,
    terminal_outputs: SharedTerminalOutputs,
    selected_config: Option<String>,
}

impl Registry {
    pub fn new(working_dir: PathBuf, env_vars: HashMap<String, String>) -> Self {
        Self {
            working_dir,
            env_vars: Rc::new(RefCell::new(env_vars)),
            export_store: Rc::new(RefCell::new(BTreeMap::new())),
            terminal_outputs: Rc::new(RefCell::new(BTreeMap::new())),
            selected_config: None,
        }
    }

    pub fn with_selected_config(mut self, selected_config: Option<String>) -> Self {
        self.selected_config = selected_config;
        self
    }

    pub fn working_dir(&self) -> &PathBuf {
        &self.working_dir
    }

    /// Run a recipe in "registration mode": execute the Lua recipe body with
    /// capture-mode APIs so that work units are recorded instead of executed.
    /// Returns a `RecipeUnits` describing what the recipe wants to do.
    pub fn register_recipe(
        &self,
        lua_source: &str,
        recipe_name: &str,
    ) -> Result<RecipeUnits, RegisterError> {
        let lua = Lua::new();
        let capture_state: SharedCaptureState = Rc::new(RefCell::new(CaptureState::new()));

        let recipes = register_cook_api_capture(
            &lua,
            self.env_vars.clone(),
            &self.working_dir,
            capture_state.clone(),
            recipe_name,
        )?;
        crate::fs_api::register_fs_api(&lua, &self.working_dir)?;
        crate::path_api::register_path_api(&lua)?;

        // Module system APIs
        crate::platform_api::register_platform_api(&lua)?;

        let module_state: SharedModuleLoaderState = Rc::new(RefCell::new(
            ModuleLoaderState::new(self.working_dir.clone()),
        ));
        crate::module_loader::register_module_loader(&lua, module_state.clone())?;
        crate::module_loader::register_cache_api(&lua, module_state.clone())?;
        crate::unit_api::register_unit_api(&lua, capture_state.clone(), recipe_name)?;
        crate::export_api::register_export_api(&lua, self.export_store.clone())?;
        crate::test_api::register_test_api(&lua, capture_state.clone())?;
        crate::dep_output_api::register_dep_output_api(
            &lua,
            self.terminal_outputs.clone(),
            capture_state.clone(),
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

        // Store terminal outputs so downstream recipes can call cook.dep_output()
        self.terminal_outputs
            .borrow_mut()
            .insert(recipe_name.to_string(), terminal_outputs_list.clone());

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
