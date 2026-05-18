use mlua::prelude::*;
use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::{Arc, Mutex};

use cook_contracts::RecipeUnits;

use crate::capture::install_cook_api;
use crate::context::setup_recipe_context;
use crate::dep_output_api::SharedTerminalOutputs;
use crate::env_api::{install_require_env, EnvKeyset};
use crate::export_api::SharedExportStore;
use crate::module_loader::{ModuleLoaderState, SharedModuleLoaderState};
use crate::probe_api::{install_cook_probe, ProbeRegistry};
use crate::{
    BodyCaptureState, RegisterError, SessionCaptureState, SharedBodySlot,
    SharedSessionCaptureState,
};

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
        // Split capture state: SessionCaptureState lives for the whole register
        // pass (probes), and a SharedBodySlot holds the BodyCaptureState during
        // body invocation.
        //
        // SHI-222 Phase 1 Task 1.2: the slot is `Option<BodyCaptureState>` so
        // closures can detect "called outside a recipe body" cleanly once
        // Phase 2 introduces the new `register_cookfile` entry point. In the
        // current per-recipe `register_recipe` entry point, top-level
        // register-block bodies and the recipe body share the same accumulator
        // (units accumulated in a register block must surface through the
        // returned RecipeUnits for the chosen recipe). To preserve that
        // semantic for now, the slot is opened BEFORE `lua.load(...).exec()`
        // (which runs the register-block splices) and drained after the
        // recipe body returns. The Option shape still nails down the calling
        // contract for Phase 2.
        let session_state: SharedSessionCaptureState =
            Rc::new(RefCell::new(SessionCaptureState::new()));
        let body_slot: SharedBodySlot =
            Rc::new(RefCell::new(Some(BodyCaptureState::new())));

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

        let recipes = install_cook_api(
            &lua,
            self.env_vars.clone(),
            &self.working_dir,
            body_slot.clone(),
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
            install_cook_probe(&lua, &cook_tbl, probe_registry.clone(), body_slot.clone(), cookfile_label)?;
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
            body_slot.clone(),
            recipe_name,
            self.terminal_outputs.clone(),
            self.working_dir.clone(),
        )?;
        crate::export_api::register_export_api(&lua, self.export_store.clone())?;
        crate::test_api::register_test_api(&lua, body_slot.clone())?;
        crate::dep_output_api::register_dep_output_api(
            &lua,
            self.terminal_outputs.clone(),
            body_slot.clone(),
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
        // The body slot was opened at the start of register_recipe (see the
        // Phase 1.2 comment there); we just stamp the recipe name on it now,
        // immediately before invoking the body.
        {
            let mut slot = body_slot.borrow_mut();
            let body = slot
                .as_mut()
                .expect("body slot populated at start of register_recipe");
            body.current_recipe = Some(qualified_name.clone());
        }

        // Execute recipe function — body capture state gets populated via the
        // cook.* closures, which now borrow `body_slot`.
        let func: LuaFunction = lua.registry_value(&recipe.function)?;
        func.call::<()>(())?;

        // Drain the body capture state — the slot returns to `None` so any
        // post-body closure invocation (there shouldn't be any) would fail
        // cleanly rather than silently appending to a stale body.
        let body = body_slot
            .borrow_mut()
            .take()
            .expect("body slot populated at start of register_recipe");

        // Flush module caches
        module_state.borrow().flush_all();

        // Drain the probe registry into the session capture state. ProbeRegistry
        // uses a BTreeMap keyed by probe key, so iteration order is
        // deterministic. cook.probe calls made during this register pass
        // (including those in imported modules) are all collected here.
        {
            let probe_reg = probe_registry.borrow();
            let mut sess = session_state.borrow_mut();
            for (_key, reg) in &probe_reg.probes {
                sess.probes.push(reg.probe.clone());
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
            for (idx, unit) in body.units.iter().enumerate() {
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

        let terminal_outputs_list = body.last_cook_step_outputs.clone();
        self.terminal_outputs
            .lock()
            .expect("terminal_outputs mutex poisoned")
            .insert(qualified_name, terminal_outputs_list.clone());

        // Convert HashMap env_vars to BTreeMap for RecipeUnits
        let env_btree: BTreeMap<String, String> = self.env_vars.borrow().iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();

        // Build the per-recipe RecipeUnits from the drained body plus a clone
        // of the session-scoped probe set (each recipe receives the same set;
        // the unified-DAG work in Phase 2 will deduplicate probe units across
        // recipes).
        let probes = session_state.borrow().probes.clone();

        Ok(RecipeUnits {
            recipe_name: recipe_name.to_string(),
            deps,
            units: body.units,
            step_groups: body.step_groups,
            working_dir: self.working_dir.clone(),
            env_vars: env_btree,
            terminal_outputs: terminal_outputs_list,
            dep_edges: body.dep_edges,
            probes,
        })
    }
}

/// Unified register-phase entry point (CS-0077 Phase 2).
///
/// Runs the Cookfile's top-level Lua exactly once, collects every
/// registered recipe (surface `recipe NAME` blocks and dynamic
/// `cook.recipe(...)` calls alike), invokes each registered body
/// to discover its work units, and returns the aggregate as a
/// [`RegisteredCookfile`].
///
/// Pipeline (Standard §6, SHI-222 Phase 2 Task 2.2):
///
/// 1. Build a fresh `Lua::unsafe_new()` VM.
/// 2. Wire `cache_ctx` app_data and `__cook_cookfile_path` registry value.
/// 3. Construct empty `SessionCaptureState` and a `SharedBodySlot` starting
///    as `None` (top-level Lua runs without an active body — closures that
///    require one return clean Lua errors if invoked at top level).
/// 4. Build a fresh `ProbeRegistry`.
/// 5. Install the full `cook.*` / `fs.*` / `path.*` / module-loader API
///    surface on the VM (identical to `register_recipe`'s setup phase).
/// 6. Execute the source as a named chunk so the call-stack helper used by
///    `cook.recipe`'s line tagging matches the Cookfile's source label.
/// 7. Dispatch `__cook_run_config_blocks` if present; freeze the env keyset;
///    re-apply CLI overrides; snapshot the final env back into `env_vars`.
/// 8. (Task 2.3 will insert collision detection here.)
/// 9. Run probe cycle detection ONCE per session.
/// 10. Drain the probe registry into `session_state.probes`.
/// 11. Topologically sort registered recipes by `metadata.requires` (local
///     DFS; reports cycles via `RegisterError::DependencyCycle`).
/// 12. Invoke each recipe body in topo order, opening a fresh
///     `BodyCaptureState` immediately before the call and draining it
///     back to `None` immediately after.
/// 13. Assemble `RegisteredCookfile { names, units_by_recipe, probes, final_env }`.
///
/// `kind` on each `RegisteredRecipePub` is hard-coded to
/// [`crate::RecipeKind::Recipe`] here — chore-vs-recipe tagging waits on
/// the codegen change in Task 3.1 (no surface chore distinction yet).
pub fn register_cookfile(
    builder: RegisterSessionBuilder,
    lua_source: &str,
    cache_ctx: Option<Arc<cook_cache::cache_ctx::CacheContext>>,
) -> Result<crate::RegisteredCookfile, RegisterError> {
    // 1. Fresh Lua VM. unsafe_new() matches register_recipe — see the
    //    comment block there for the C-extension rationale.
    // SAFETY: mlua's unsafe_new() opens all Lua standard libraries; the
    //   cook API layer below sandboxes the dangerous surfaces.
    let lua = unsafe { Lua::unsafe_new() };

    // 2. Wire CacheContext + cookfile path label.
    if let Some(ref ctx) = cache_ctx {
        lua.set_app_data(ctx.clone());
        let cookfile_rel =
            cookfile_path_relative_to(&ctx.project_root, &builder.working_dir.join("Cookfile"));
        lua.set_named_registry_value("__cook_cookfile_path", cookfile_rel)
            .map_err(RegisterError::Lua)?;
    }

    // Compute the source label used both as the loaded chunk's name and as
    // the lookup target inside `caller_line_in_cookfile`. When no CacheContext
    // is available (tests, legacy call sites) we fall back to a bare
    // "Cookfile" label so the helper's `ends_with` match still resolves.
    let cookfile_label: String = lua
        .named_registry_value::<String>("__cook_cookfile_path")
        .unwrap_or_else(|_| "Cookfile".to_string());

    // 3. Session-scope state; body slot starts None (no active recipe body
    //    during top-level load — spec §6 step 4).
    let session_state: SharedSessionCaptureState =
        Rc::new(RefCell::new(SessionCaptureState::new()));
    let body_slot: SharedBodySlot = Rc::new(RefCell::new(None));

    // 4. Probe registry for this register pass.
    let probe_registry = Rc::new(RefCell::new(ProbeRegistry::default()));

    // 5. Install `cook.*` core API. `recipe_name` here is the legacy
    //    closure-capture argument used by `cook.add_unit` for
    //    `cache_meta.recipe_name`; in register_cookfile there is no single
    //    recipe, so pass an empty string. Per-recipe attribution happens
    //    through `body.current_recipe` instead.
    let recipes = install_cook_api(
        &lua,
        builder.env_vars.clone(),
        &builder.working_dir,
        body_slot.clone(),
        "",
    )?;
    {
        let cook_tbl: LuaTable = lua.globals().get("cook")?;
        install_require_env(&lua, &cook_tbl, builder.env_keyset.clone())?;
    }
    {
        let cook_tbl: LuaTable = lua.globals().get("cook")?;
        install_cook_probe(
            &lua,
            &cook_tbl,
            probe_registry.clone(),
            body_slot.clone(),
            cookfile_label.clone(),
        )?;
    }

    // Sandbox + fs/path/platform API. Project root falls back to working_dir
    // when no CacheContext is present (matches register_recipe).
    let project_root: std::path::PathBuf = cache_ctx
        .as_ref()
        .map(|c| c.project_root.clone())
        .unwrap_or_else(|| builder.working_dir.clone());
    cook_lua_stdlib::register_fs_api_with_sandbox(
        &lua,
        cook_lua_stdlib::WorkingDirSource::Static(builder.working_dir.clone()),
        cook_lua_stdlib::SandboxSource::confined(project_root.clone()),
    )?;
    cook_lua_stdlib::register_path_api(&lua)?;
    cook_lua_stdlib::install_shell_escape_guards(
        &lua,
        cook_lua_stdlib::SandboxSource::confined(project_root),
    )?;
    {
        let cook_tbl: LuaTable = lua.globals().get("cook")?;
        cook_lua_stdlib::register_platform_api(&lua, &cook_tbl)?;
    }

    // Module loader + remaining cook APIs.
    let module_state: SharedModuleLoaderState = Rc::new(RefCell::new(
        ModuleLoaderState::new(builder.working_dir.clone()),
    ));
    crate::module_loader::register_module_loader(&lua, module_state.clone())?;
    crate::module_loader::register_cache_api(&lua, module_state.clone())?;
    crate::unit_api::register_unit_api(
        &lua,
        body_slot.clone(),
        "",
        builder.terminal_outputs.clone(),
        builder.working_dir.clone(),
    )?;
    crate::export_api::register_export_api(&lua, builder.export_store.clone())?;
    crate::test_api::register_test_api(&lua, body_slot.clone())?;
    crate::dep_output_api::register_dep_output_api(
        &lua,
        builder.terminal_outputs.clone(),
        body_slot.clone(),
        builder.alias_dirs.clone(),
        builder.qualified_prefix.clone(),
        builder.alias_qualified_prefixes.clone(),
    )?;
    crate::context::register_resolve_ingredients(&lua, &builder.working_dir)?;
    crate::codec_api::register_codec_api(&lua)?;

    // 6. Execute the top-level Lua. Name the chunk with an `@` prefix so
    //    `caller_line_in_cookfile`'s `ends_with(&target)` (target is the
    //    raw cookfile-relative path) still matches — see Task 1.4 review.
    //    Recipe registration happens here via `cook.recipe(...)` calls,
    //    which capture each body's `LuaRegistryKey` into the shared
    //    `recipes` Rc returned from `install_cook_api`.
    let chunk_name = format!("@{}", cookfile_label);
    lua.load(lua_source).set_name(chunk_name).exec()?;

    // 7. Config block dispatch — identical to register_recipe.
    let final_env: BTreeMap<String, String>;
    if let Ok(dispatch) = lua.globals().get::<LuaFunction>("__cook_run_config_blocks") {
        let cook_tbl: LuaTable = lua.globals().get("cook")?;
        let env_tbl: LuaTable = cook_tbl.get("env")?;
        lua.globals().set("env", env_tbl.clone())?;

        let name_arg: Option<String> = builder.selected_config.clone();
        dispatch.call::<()>(name_arg)?;

        builder.env_keyset.freeze(&env_tbl)?;

        for (k, v) in &builder.cli_overrides {
            env_tbl.set(k.as_str(), v.as_str())?;
        }

        {
            let mut env_map = builder.env_vars.borrow_mut();
            for pair in env_tbl.pairs::<String, String>() {
                let (k, v) = pair?;
                env_map.insert(k, v);
            }
        }

        // Shadowing diagnostic — same logic as register_recipe.
        let declared_env: std::collections::BTreeSet<String> =
            builder.env_keyset.declared_list().into_iter().collect();
        let recipe_names_set: std::collections::BTreeSet<String> = recipes
            .borrow()
            .iter()
            .map(|r| r.name.clone())
            .collect();
        let mut emitted = builder.shadow_warnings_emitted.borrow_mut();
        for name in recipe_names_set.intersection(&declared_env) {
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

        // Snapshot final env BEFORE removing the global so the table is
        // still readable. `env_vars` was just refreshed above so it's the
        // canonical source; copy from there to avoid re-borrowing `env_tbl`.
        final_env = builder
            .env_vars
            .borrow()
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();

        lua.globals().set("env", mlua::Value::Nil)?;
    } else {
        // No config blocks — `final_env` is just the initial env_vars.
        final_env = builder
            .env_vars
            .borrow()
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
    }

    // 8. Collision detection — Task 2.3.

    // 9. Probe cycle detection — once per session.
    probe_registry
        .borrow()
        .detect_cycles()
        .map_err(|msg| RegisterError::Lua(mlua::Error::runtime(msg)))?;

    // 10. Drain probe registry into session state.
    {
        let probe_reg = probe_registry.borrow();
        let mut sess = session_state.borrow_mut();
        for (_key, reg) in &probe_reg.probes {
            sess.probes.push(reg.probe.clone());
        }
    }

    // 11. Topological sort of registered names by `requires`.
    let names_to_requires: BTreeMap<String, Vec<String>> = recipes
        .borrow()
        .iter()
        .map(|r| (r.name.clone(), r.metadata.requires.clone()))
        .collect();
    let topo = local_topological_sort(&names_to_requires)?;

    // 12. Invoke each recipe body in topo order. The body slot is opened
    //     immediately before the call and drained back to `None` afterwards
    //     so a stray closure invocation between bodies fails cleanly rather
    //     than silently appending to a stale body.
    let mut units_by_recipe: BTreeMap<String, RecipeUnits> = BTreeMap::new();
    let mut names: Vec<crate::RegisteredRecipePub> = Vec::with_capacity(topo.len());

    for name in &topo {
        // Open fresh body slot.
        *body_slot.borrow_mut() = Some(BodyCaptureState::new());

        // Look up the recipe entry. Borrow scope kept tight so we can mutate
        // body_slot below without overlapping the recipes borrow.
        let (
            func_key_clone,
            requires,
            source,
            qualified_name,
        ): (LuaRegistryKey, Vec<String>, crate::capture::RegistrationSource, String);
        {
            let registry = recipes.borrow();
            let recipe = registry
                .iter()
                .find(|r| &r.name == name)
                .ok_or_else(|| RegisterError::RecipeNotFound(name.clone()))?;

            // Run recipe context setup (ingredient resolution).
            setup_recipe_context(&lua, recipe, &builder.working_dir)?;

            // The `LuaRegistryKey` doesn't impl Clone, so we materialize the
            // function now and stash it for the call below; the registry
            // entry itself stays untouched.
            let func: LuaFunction = lua.registry_value(&recipe.function)?;
            // Re-stash so we can drop the `registry` borrow before calling.
            func_key_clone = lua.create_registry_value(func)?;
            requires = recipe.metadata.requires.clone();
            source = recipe.source;
            qualified_name = if builder.qualified_prefix.is_empty() {
                recipe.name.clone()
            } else {
                format!("{}.{}", builder.qualified_prefix, recipe.name)
            };
        }

        // Stamp current_recipe on the body so cook.add_test defaults the
        // suite field correctly (CS-0061 §3.2).
        {
            let mut slot = body_slot.borrow_mut();
            let body = slot
                .as_mut()
                .expect("body slot just opened above");
            body.current_recipe = Some(qualified_name.clone());
        }

        // Call the body. Any error short-circuits — earlier bodies' captures
        // are dropped along with the function return.
        let func: LuaFunction = lua.registry_value(&func_key_clone)?;
        func.call::<()>(())?;
        // Cleanup the transient registry entry to avoid leaking refs across
        // many recipes in large Cookfiles.
        lua.remove_registry_value(func_key_clone)?;

        // Drain the body slot back to None.
        let body = body_slot
            .borrow_mut()
            .take()
            .expect("body slot populated above");

        // Resolve probe references — same end-of-body check that
        // register_recipe runs, scoped to this recipe's units.
        {
            use std::collections::BTreeSet;
            let probe_reg = probe_registry.borrow();
            let registered_keys: BTreeSet<&str> =
                probe_reg.probes.keys().map(|s| s.as_str()).collect();
            for (idx, unit) in body.units.iter().enumerate() {
                for key in &unit.probes {
                    if !registered_keys.contains(key.as_str()) {
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

        // Record terminal outputs for cross-recipe dep_output lookups.
        let terminal_outputs_list = body.last_cook_step_outputs.clone();
        builder
            .terminal_outputs
            .lock()
            .expect("terminal_outputs mutex poisoned")
            .insert(qualified_name.clone(), terminal_outputs_list.clone());

        let env_btree: BTreeMap<String, String> = builder
            .env_vars
            .borrow()
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();

        // Each per-recipe RecipeUnits carries a clone of the session probe
        // set today; Phase 3 dedup will replace this with a session-level
        // probe view consumed off the RegisteredCookfile directly.
        let probes = session_state.borrow().probes.clone();

        let units = RecipeUnits {
            recipe_name: name.clone(),
            deps: requires.clone(),
            units: body.units,
            step_groups: body.step_groups,
            working_dir: builder.working_dir.clone(),
            env_vars: env_btree,
            terminal_outputs: terminal_outputs_list,
            dep_edges: body.dep_edges,
            probes,
        };
        units_by_recipe.insert(name.clone(), units);

        names.push(crate::RegisteredRecipePub {
            name: name.clone(),
            source,
            kind: crate::RecipeKind::Recipe,
            requires,
        });
    }

    // Flush module caches once at the end of the pass.
    module_state.borrow().flush_all();

    // 13. Probes view: BTreeMap keyed by probe key (deterministic).
    let probes: BTreeMap<String, cook_contracts::ProbeUnit> = session_state
        .borrow()
        .probes
        .iter()
        .map(|p| (p.key.clone(), p.clone()))
        .collect();

    Ok(crate::RegisteredCookfile {
        names,
        units_by_recipe,
        probes,
        final_env,
    })
}

/// Local DFS-based topological sort of recipe names by their declared
/// `requires`. Returns names in dependency-first order (a recipe appears
/// after every recipe it requires that is also present in `deps`).
///
/// Edges to recipe names absent from `deps` are skipped — those are
/// cross-Cookfile `requires` whose resolution is the engine's
/// responsibility. Unknown references are surfaced by the cross-cookfile
/// dep analyzer downstream, not here.
///
/// Cycles are reported as [`RegisterError::DependencyCycle`] with the
/// path of names forming the cycle (first and last elements coincide).
fn local_topological_sort(
    deps: &BTreeMap<String, Vec<String>>,
) -> Result<Vec<String>, RegisterError> {
    #[derive(Clone, Copy, PartialEq)]
    enum State {
        Unvisited,
        Visiting,
        Visited,
    }
    let mut state: BTreeMap<&str, State> =
        deps.keys().map(|k| (k.as_str(), State::Unvisited)).collect();
    let mut order: Vec<String> = Vec::new();
    let mut path: Vec<String> = Vec::new();
    fn visit<'a>(
        node: &'a str,
        deps: &'a BTreeMap<String, Vec<String>>,
        state: &mut BTreeMap<&'a str, State>,
        order: &mut Vec<String>,
        path: &mut Vec<String>,
    ) -> Result<(), RegisterError> {
        match state.get(node) {
            Some(State::Visited) => return Ok(()),
            Some(State::Visiting) => {
                let cycle_start = path.iter().position(|n| n == node).unwrap_or(0);
                let mut cycle: Vec<String> = path[cycle_start..].to_vec();
                cycle.push(node.to_string());
                return Err(RegisterError::DependencyCycle { recipes: cycle });
            }
            _ => {}
        }
        state.insert(node, State::Visiting);
        path.push(node.to_string());
        if let Some(children) = deps.get(node) {
            for child in children {
                // Skip references the local set doesn't know about — those
                // are cross-recipe `requires` to dependencies registered
                // elsewhere (e.g. workspace imports). The engine's
                // cross-cookfile dep analyzer will reject genuinely unknown
                // names later.
                if deps.contains_key(child) {
                    visit(child, deps, state, order, path)?;
                }
            }
        }
        path.pop();
        state.insert(node, State::Visited);
        order.push(node.to_string());
        Ok(())
    }
    for name in deps.keys() {
        visit(name, deps, &mut state, &mut order, &mut path)?;
    }
    Ok(order)
}

/// Cheap name-only register pass for surface dispatch (CS-0077 Phase 2).
///
/// Runs the Cookfile's top-level Lua to collect the set of registered
/// recipe names, but does NOT invoke any recipe body. Used by the
/// surface CLI to list recipes and to validate `cook NAME` arguments
/// without paying the full DAG-discovery cost.
///
/// Stub: lands in Task 2.4.
pub fn list_names(
    _builder: RegisterSessionBuilder,
    _lua_source: &str,
) -> Result<Vec<crate::RegisteredRecipePub>, RegisterError> {
    unimplemented!("list_names lands in Task 2.4")
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
