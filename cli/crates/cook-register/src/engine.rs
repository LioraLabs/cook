use mlua::prelude::*;
use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::rc::Rc;

use cook_contracts::RecipeUnits;

use crate::capture::register_cook_api_capture;
use crate::context::setup_recipe_context;
use crate::export_api::SharedExportStore;
use crate::module_loader::{ModuleLoaderState, SharedModuleLoaderState};
use crate::{CaptureState, RegisterError, SharedCaptureState};

pub struct Registry {
    working_dir: PathBuf,
    env_vars: HashMap<String, String>,
    export_store: SharedExportStore,
}

impl Registry {
    pub fn new(working_dir: PathBuf, env_vars: HashMap<String, String>) -> Self {
        Self {
            working_dir,
            env_vars,
            export_store: Rc::new(RefCell::new(BTreeMap::new())),
        }
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
            &self.env_vars,
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
        crate::context::register_resolve_ingredients(&lua, &self.working_dir)?;

        lua.load(lua_source).exec()?;

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

        // Convert HashMap env_vars to BTreeMap for RecipeUnits
        let env_btree: BTreeMap<String, String> = self.env_vars.iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();

        Ok(RecipeUnits {
            recipe_name: recipe_name.to_string(),
            deps,
            units: cap.units.clone(),
            step_groups: cap.step_groups.clone(),
            working_dir: self.working_dir.clone(),
            env_vars: env_btree,
        })
    }
}
