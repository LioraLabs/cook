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
use crate::probe_api::{install_cook_probe, ProbeRegistry};
use crate::{CaptureState, RegisterError, SharedCaptureState};

pub struct RegisterSessionBuilder {
    working_dir: PathBuf,
    env_vars: Rc<RefCell<HashMap<String, String>>>,
    /// Explicit CLI `--set KEY=VALUE` overrides, kept separate so they can be
    /// re-applied to `cook.env` after the config block runs (CLI wins over
    /// config-block defaults regardless of authoring style).
    cli_overrides: HashMap<String, String>,
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
    /// (recipe, env-var) shadowing pairs we've already warned about, so
    /// we don't repeat the diagnostic on every recipe-register call within
    /// a single Cookfile load.
    shadow_warnings_emitted: Rc<RefCell<std::collections::BTreeSet<(String, String)>>>,
}

impl RegisterSessionBuilder {
    pub fn new(working_dir: PathBuf, env_vars: HashMap<String, String>) -> Self {
        Self {
            working_dir,
            env_vars: Rc::new(RefCell::new(env_vars)),
            cli_overrides: HashMap::new(),
            export_store: Rc::new(RefCell::new(BTreeMap::new())),
            terminal_outputs: Arc::new(Mutex::new(BTreeMap::new())),
            selected_config: None,
            qualified_prefix: String::new(),
            alias_dirs: BTreeMap::new(),
            alias_qualified_prefixes: BTreeMap::new(),
            env_keyset: EnvKeyset::new(),
            shadow_warnings_emitted: Rc::new(RefCell::new(std::collections::BTreeSet::new())),
        }
    }

    /// Record explicit `--set KEY=VALUE` overrides. They are re-applied to
    /// `cook.env` after the config-block dispatcher runs, so a config block's
    /// `env.KEY = "default"` no longer silently shadows a CLI override.
    pub fn with_cli_overrides(mut self, overrides: HashMap<String, String>) -> Self {
        self.cli_overrides = overrides;
        self
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
        // Use unsafe_new() so that cook modules containing C extensions (e.g.
        // lpeg.so, cjson.so) can be loaded during the register phase.  The
        // register VM already executes user-authored Lua (Cookfile bodies) with
        // the same level of trust as the execute-phase worker, so there is no
        // effective security regression here.  The FS sandbox (register_fs_api_
        // with_sandbox) is installed immediately below and still confines file-
        // system access to the project root.
        // SAFETY: mlua's unsafe_new() opens all Lua standard libraries; the
        //   caller is responsible for sandboxing any dangerous surfaces through
        //   the cook API layer.  Consistent with cook-luaotp/src/pool.rs:159.
        let lua = unsafe { Lua::unsafe_new() };
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

        // Create the probe registry for this register pass.
        let probe_registry = Rc::new(RefCell::new(ProbeRegistry::default()));

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

        // Install cook.probe on the same cook table.
        {
            let cook_tbl: LuaTable = lua.globals().get("cook")?;
            // Use the Cookfile path relative to the project root (when available),
            // or fall back to a bare "Cookfile" label for test/legacy call sites.
            let cookfile_label = lua
                .named_registry_value::<String>("__cook_cookfile_path")
                .unwrap_or_else(|_| "Cookfile".to_string());
            install_cook_probe(&lua, &cook_tbl, probe_registry.clone(), capture_state.clone(), cookfile_label)?;
        }
        // `fs.*`, `path.*`, `cook.platform.*` come from the shared
        // cook-lua-stdlib crate (CS-0044) so register-phase and
        // execute-phase VMs see byte-identical behavior.
        //
        // The register-phase VM is always confined to the project root
        // (CS-0045). The captured `lua_code` strings will replay at
        // execute time with the per-step-kind sandbox policy applied
        // there; here we just need to keep `cook.add_unit({...})`-time
        // file accesses (e.g. `cook.dep_output(...)` resolution helpers
        // that read from disk) within the project. `project_root` is
        // sourced from CacheContext when available; in the test/legacy
        // path we fall back to the recipe's working_dir, which is
        // operationally indistinguishable for single-Cookfile projects.
        let project_root: std::path::PathBuf = cache_ctx
            .as_ref()
            .map(|c| c.project_root.clone())
            .unwrap_or_else(|| self.working_dir.clone());
        cook_lua_stdlib::register_fs_api_with_sandbox(
            &lua,
            cook_lua_stdlib::WorkingDirSource::Static(self.working_dir.clone()),
            cook_lua_stdlib::SandboxSource::confined(project_root.clone()),
        )?;
        cook_lua_stdlib::register_path_api(&lua)?;
        cook_lua_stdlib::install_shell_escape_guards(
            &lua,
            cook_lua_stdlib::SandboxSource::confined(project_root),
        )?;

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
            self.working_dir.clone(),
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

            // Re-apply explicit `--set KEY=VALUE` overrides on top of any
            // values the config block wrote. CLI overrides MUST win over
            // config-block defaults, but config-block evaluation needs to
            // observe the override values during dispatch (so derived
            // expressions like `env.X = env.X .. "/foo"` work) — hence we
            // run the dispatch first and reassert the overrides after.
            for (k, v) in &self.cli_overrides {
                env_tbl.set(k.as_str(), v.as_str())?;
            }

            // Snapshot mutations from cook.env back into shared env_vars.
            {
                let mut env_map = self.env_vars.borrow_mut();
                for pair in env_tbl.pairs::<String, String>() {
                    let (k, v) = pair?;
                    env_map.insert(k, v);
                }
            }

            // Standard §5.2.3: when a placeholder name resolves to both a
            // recipe and a declared env var, the recipe wins, and a
            // conforming implementation MUST emit a warning naming both.
            // We compute the intersection of (registered recipes) ∩
            // (declared env keyset) here, after the keyset is frozen,
            // and emit one diagnostic per offending name (deduped across
            // recipe-register calls within this Cookfile load).
            let declared_env: std::collections::BTreeSet<String> =
                self.env_keyset.declared_list().into_iter().collect();
            let recipe_names: std::collections::BTreeSet<String> = recipes
                .borrow()
                .iter()
                .map(|r| r.name.clone())
                .collect();
            let mut emitted = self.shadow_warnings_emitted.borrow_mut();
            for name in recipe_names.intersection(&declared_env) {
                let key = (name.clone(), name.clone());
                if emitted.insert(key) {
                    eprintln!(
                        "cook: warning: recipe '{name}' shadows declared env var \
                         'env.{name}': $<{name}> resolves to the recipe (Standard \
                         §5.2.3). Use $<env.{name}> for the env-var value, or \
                         rename one of them."
                    );
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

        // Compute the fully-qualified recipe name used for dep-output storage and
        // for defaulting cook.add_test's suite field (CS-0061 §3.2).
        let qualified_name = if self.qualified_prefix.is_empty() {
            recipe_name.to_string()
        } else {
            format!("{}.{}", self.qualified_prefix, recipe_name)
        };

        // Track the currently-executing recipe so cook.add_test can default
        // the suite field to the enclosing recipe's name (CS-0061 §3.2).
        capture_state.borrow_mut().current_recipe = Some(qualified_name.clone());

        // Execute recipe function — capture_state gets populated
        let func: LuaFunction = lua.registry_value(&recipe.function)?;
        func.call::<()>(())?;

        // Clear the recipe tracking now that the body has finished.
        capture_state.borrow_mut().current_recipe = None;

        // Flush module caches
        module_state.borrow().flush_all();

        // Drain the probe registry into capture_state.probes.
        // ProbeRegistry uses a BTreeMap keyed by probe key, so iteration order
        // is deterministic. cook.probe calls made during this register pass
        // (including those in imported modules) are all collected here.
        {
            let probe_reg = probe_registry.borrow();
            let mut cap_mut = capture_state.borrow_mut();
            for (_key, reg) in &probe_reg.probes {
                cap_mut.probes.push(reg.probe.clone());
            }
        }

        // §22.5.8: Cycle detection on the probe `requires` graph.
        // MUST run after the register pass so all probes are visible.
        {
            let probe_reg = probe_registry.borrow();
            probe_reg
                .detect_cycles()
                .map_err(|msg| RegisterError::Lua(mlua::Error::runtime(msg)))?;
        }

        // §22.5.5: Resolve each cook.add_unit.probes key against the
        // registered probe set.  Unknown keys are rejected with a diagnostic
        // naming the unit and the missing key.  Resolution is deferred to
        // end-of-pass so the relative order of cook.probe and cook.add_unit
        // calls within one register pass is unconstrained.
        {
            use std::collections::BTreeSet;
            let probe_reg = probe_registry.borrow();
            let registered_keys: BTreeSet<&str> =
                probe_reg.probes.keys().map(|s| s.as_str()).collect();
            let cap = capture_state.borrow();
            for (idx, unit) in cap.units.iter().enumerate() {
                for key in &unit.probes {
                    if !registered_keys.contains(key.as_str()) {
                        // Derive a human-readable unit name from its first output
                        // path (deterministic); fall back to positional index.
                        let unit_name = unit
                            .cache_meta
                            .as_ref()
                            .and_then(|m| m.output_paths.first())
                            .map(|p| p.as_str())
                            .unwrap_or("")
                            .to_string();
                        let unit_label = if unit_name.is_empty() {
                            format!("<unit-{}>", idx)
                        } else {
                            unit_name
                        };
                        return Err(RegisterError::Lua(mlua::Error::runtime(format!(
                            "unit '{}' lists probe key '{}' in `probes` but no such probe was declared",
                            unit_label, key
                        ))));
                    }
                }
            }
        }

        let deps = recipe.metadata.requires.clone();

        // Extract results from capture state
        let cap = capture_state.borrow();

        let terminal_outputs_list = cap.last_cook_step_outputs.clone();
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
            probes: cap.probes.clone(),
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
