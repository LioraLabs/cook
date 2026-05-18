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
    BodyCaptureState, RegisterError, RegistrationSite, RegistrationSiteKind,
    SessionCaptureState, SharedBodySlot, SharedSessionCaptureState,
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
/// `kind` on each `RegisteredRecipePub` is copied from the internal
/// `RegisteredRecipe.kind` — surface `chore NAME` blocks lower to
/// `cook.__register_surface_chore` (codegen path; see SHI-222 Phase 3
/// Task 3.1) which tags `RecipeKind::Chore`; everything else tags
/// `RecipeKind::Recipe`.
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
    // Seed the registry value in the fallback path too — `caller_line_in_cookfile`
    // returns early when the registry value is absent, so without this seed
    // the line tag on every recipe registered in the no-CacheContext path
    // collapses to 0. Mirrors `list_names`'s setup.
    let cookfile_label: String = match lua.named_registry_value::<String>("__cook_cookfile_path") {
        Ok(s) => s,
        Err(_) => {
            let fallback = "Cookfile".to_string();
            lua.set_named_registry_value("__cook_cookfile_path", fallback.clone())
                .map_err(RegisterError::Lua)?;
            fallback
        }
    };

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

    // 5b. Install the remaining API surface (fs/path/platform sandboxing,
    //     module loader, unit/export/test/dep_output/codec APIs). Shared
    //     with `list_names` via the extracted helper. The helper returns
    //     the module-loader handle so we can `flush_all()` it after all
    //     body invocations complete.
    let module_state = install_remaining_apis(
        &lua,
        &builder,
        body_slot.clone(),
        cache_ctx.as_ref(),
    )?;

    // 6. Execute the top-level Lua. Name the chunk with an `@` prefix so
    //    `caller_line_in_cookfile`'s `ends_with(&target)` (target is the
    //    raw cookfile-relative path) still matches — see Task 1.4 review.
    //    Recipe registration happens here via `cook.recipe(...)` calls,
    //    which capture each body's `LuaRegistryKey` into the shared
    //    `recipes` Rc returned from `install_cook_api`.
    let chunk_name = format!("@{}", cookfile_label);
    lua.load(lua_source).set_name(chunk_name).exec()?;

    // 7. Config block dispatch — shared with `list_names` via the
    //    extracted helper. Returns the final env snapshot (post-config,
    //    post-CLI-override) or just the initial env_vars when no config
    //    blocks are present.
    let final_env =
        dispatch_config_blocks(&lua, &builder, &recipes.borrow())?;

    // 8. Collision detection — Task 2.3. A name registered more than once
    //    (surface vs dynamic, dynamic vs dynamic, chore vs dynamic) is a
    //    hard error per spec §8. Static-vs-static within a single Cookfile
    //    is impossible: cook-lang's parser rejects duplicate
    //    `recipe`/`chore` declarations at parse time.
    detect_collisions(&recipes.borrow())?;

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
            kind,
            qualified_name,
        ): (
            LuaRegistryKey,
            Vec<String>,
            crate::capture::RegistrationSource,
            crate::RecipeKind,
            String,
        );
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
            kind = recipe.kind;
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
        let mut body = body_slot
            .borrow_mut()
            .take()
            .expect("body slot populated above");

        // Patch the recipe_name on every unit's cache_meta. The closures in
        // register_unit_api / install_cook_api capture an empty recipe_name
        // at install time (since register_cookfile has no single recipe at
        // API-install — see comments above the install_cook_api call).
        // Per-body attribution happens here at drain time using the
        // qualified name we already know. Downstream engine cache keying
        // depends on cache_meta.recipe_name being the actual recipe name,
        // not "".
        for unit in &mut body.units {
            if let Some(meta) = unit.cache_meta.as_mut() {
                meta.recipe_name = qualified_name.clone();
            }
        }

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
            kind,
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

/// Diagnose duplicate recipe-name registrations within a single
/// `register_cookfile` pass (spec §8). A name appearing more than once —
/// surface-vs-dynamic or dynamic-vs-dynamic — is a hard error.
///
/// Each site is tagged by line and kind so the CLI can render a
/// multi-line diagnostic naming both the surface declaration and the
/// register-phase Lua call. With the Phase 3 codegen split, kind on
/// `RegisteredRecipe` distinguishes a surface `recipe NAME` block from a
/// surface `chore NAME` block; combined with `RegistrationSource` we map:
///
///   - `Static + Recipe` → `SurfaceRecipe`
///   - `Static + Chore`  → `SurfaceChore`
///   - `Dynamic + _`     → `Dynamic` (chores can't register dynamically;
///     the Recipe constraint is enforced by `cook.recipe` itself which
///     always tags `RecipeKind::Recipe`).
///
/// Returns on the first colliding name (deterministic via `BTreeMap`
/// key sort). Fail-fast is intentional per SHI-222 spec §8: collisions
/// are a hard error that prevents `register_cookfile` from producing a
/// coherent registered set, and accumulating across names would only
/// delay the same diagnostic by one CI cycle. Multiple sites for a
/// single colliding name are all preserved in the error.
///
/// Re-used by `list_names` in Task 2.4.
fn detect_collisions(
    recipes: &[crate::capture::RegisteredRecipe],
) -> Result<(), RegisterError> {
    use std::collections::BTreeMap;
    let mut by_name: BTreeMap<&str, Vec<RegistrationSite>> = BTreeMap::new();
    for r in recipes {
        let kind = match (r.source, r.kind) {
            (crate::capture::RegistrationSource::Static { .. }, crate::RecipeKind::Recipe) => {
                RegistrationSiteKind::SurfaceRecipe
            }
            (crate::capture::RegistrationSource::Static { .. }, crate::RecipeKind::Chore) => {
                RegistrationSiteKind::SurfaceChore
            }
            (crate::capture::RegistrationSource::Dynamic { .. }, _) => {
                RegistrationSiteKind::Dynamic
            }
        };
        let line = match r.source {
            crate::capture::RegistrationSource::Static { line } => line,
            crate::capture::RegistrationSource::Dynamic { line } => line,
        };
        let site = RegistrationSite { line, kind };
        by_name.entry(r.name.as_str()).or_default().push(site);
    }
    for (name, sites) in by_name {
        if sites.len() > 1 {
            return Err(RegisterError::RecipeCollision {
                name: name.to_string(),
                sites,
            });
        }
    }
    Ok(())
}

/// Cheap name-only register pass for surface dispatch (CS-0077 Phase 2).
///
/// Runs the Cookfile's top-level Lua to collect the set of registered
/// recipe names, runs the config block (so per-config recipe gating
/// surfaces correctly), detects name collisions, and validates the probe
/// `requires` graph — but does NOT invoke any recipe body and does NOT
/// fire any probe queries. Used by the surface CLI to list recipes and
/// to validate `cook NAME` arguments without paying the full
/// DAG-discovery cost.
///
/// `kind` on each returned [`crate::RegisteredRecipePub`] is copied
/// from the internal `RegisteredRecipe.kind`: surface `chore NAME`
/// blocks (codegen path via `cook.__register_surface_chore`) surface
/// as `RecipeKind::Chore`; everything else (including all dynamic
/// `cook.recipe(...)` registrations) surfaces as `RecipeKind::Recipe`.
pub fn list_names(
    builder: RegisterSessionBuilder,
    lua_source: &str,
) -> Result<Vec<crate::RegisteredRecipePub>, RegisterError> {
    // SAFETY: matches register_cookfile/register_recipe — see comment there.
    let lua = unsafe { Lua::unsafe_new() };

    // body_slot stays `None` for the whole call: no recipe body executes,
    // so any closure that requires an active body (cook.exec, cook.add_unit,
    // …) returns a clean Lua error if invoked from top-level Lua. That
    // keeps list_names cheap and honest. No SessionCaptureState is
    // constructed — list_names doesn't surface probes.
    let body_slot: SharedBodySlot = Rc::new(RefCell::new(None));
    let probe_registry = Rc::new(RefCell::new(ProbeRegistry::default()));

    // Cookfile label: list_names is called without a CacheContext, so fall
    // back to the bare "Cookfile" label used by tests/legacy call sites.
    let cookfile_label: String = "Cookfile".to_string();

    // Wire the named registry value used by `caller_line_in_cookfile` to
    // match chunk source labels against the Cookfile path. Without this,
    // `cook.recipe` calls would all record `line = 0`. register_cookfile
    // sets this from CacheContext.project_root; list_names has no
    // CacheContext, so seed it from `cookfile_label` directly so the
    // chunk-naming line-tagging wiring still works.
    lua.set_named_registry_value("__cook_cookfile_path", cookfile_label.clone())
        .map_err(RegisterError::Lua)?;

    // Install cook.* core surface. `recipe_name` is "" — list_names never
    // invokes a body, so cook.add_unit's recipe_name capture is moot here.
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
    // list_names doesn't invoke any body, so the returned module-loader
    // handle is dropped here — no flush needed (no module bodies ran).
    let _module_state = install_remaining_apis(&lua, &builder, body_slot.clone(), None)?;

    // Load top-level Lua. Recipe registration happens via `cook.recipe(...)`
    // calls captured into `recipes`. Bodies are stashed as `LuaRegistryKey`
    // values but never invoked here.
    let chunk_name = format!("@{}", cookfile_label);
    lua.load(lua_source).set_name(chunk_name).exec()?;

    // Run config blocks so per-config gating (e.g. recipe registration
    // inside a `config "release"` block) is reflected in the listed set.
    let _final_env = dispatch_config_blocks(&lua, &builder, &recipes.borrow())?;

    // Same hard-error checks register_cookfile applies. Probe cycle
    // detection runs on the static `requires` graph — no probe BODY runs,
    // so this is cheap.
    detect_collisions(&recipes.borrow())?;
    probe_registry
        .borrow()
        .detect_cycles()
        .map_err(|msg| RegisterError::Lua(mlua::Error::runtime(msg)))?;

    let out: Vec<crate::RegisteredRecipePub> = recipes
        .borrow()
        .iter()
        .map(|r| crate::RegisteredRecipePub {
            name: r.name.clone(),
            source: r.source,
            kind: r.kind,
            requires: r.metadata.requires.clone(),
        })
        .collect();
    Ok(out)
}

/// Install the non-`cook.recipe` / non-`cook.probe` part of the
/// register-phase API surface on the given Lua VM: fs/path/platform
/// sandboxes, module loader, unit/export/test/dep_output/codec APIs.
///
/// Extracted as a shared helper so `register_cookfile` and `list_names`
/// see byte-identical API installation. `cook.recipe` and `cook.probe`
/// (which need access to the per-pass `recipes` Rc and probe registry
/// respectively) are still installed at the call site before this helper
/// runs.
///
/// `cache_ctx` is `Some` when the caller has a project root resolved
/// (`register_cookfile` in production); `None` for tests, legacy call
/// sites, and `list_names`. The sandbox falls back to the recipe's
/// working_dir in the `None` case, which matches single-Cookfile project
/// behavior (CS-0045).
fn install_remaining_apis(
    lua: &Lua,
    builder: &RegisterSessionBuilder,
    body_slot: SharedBodySlot,
    cache_ctx: Option<&Arc<cook_cache::cache_ctx::CacheContext>>,
) -> Result<SharedModuleLoaderState, RegisterError> {
    // Sandbox + fs/path/platform API. Project root falls back to
    // working_dir when no CacheContext is present.
    let project_root: std::path::PathBuf = cache_ctx
        .map(|c| c.project_root.clone())
        .unwrap_or_else(|| builder.working_dir.clone());
    cook_lua_stdlib::register_fs_api_with_sandbox(
        lua,
        cook_lua_stdlib::WorkingDirSource::Static(builder.working_dir.clone()),
        cook_lua_stdlib::SandboxSource::confined(project_root.clone()),
    )?;
    cook_lua_stdlib::register_path_api(lua)?;
    cook_lua_stdlib::install_shell_escape_guards(
        lua,
        cook_lua_stdlib::SandboxSource::confined(project_root),
    )?;
    {
        let cook_tbl: LuaTable = lua.globals().get("cook")?;
        cook_lua_stdlib::register_platform_api(lua, &cook_tbl)?;
    }

    // Module loader + remaining cook APIs.
    let module_state: SharedModuleLoaderState = Rc::new(RefCell::new(
        ModuleLoaderState::new(builder.working_dir.clone()),
    ));
    crate::module_loader::register_module_loader(lua, module_state.clone())?;
    crate::module_loader::register_cache_api(lua, module_state.clone())?;
    crate::unit_api::register_unit_api(
        lua,
        body_slot.clone(),
        "",
        builder.terminal_outputs.clone(),
        builder.working_dir.clone(),
    )?;
    crate::export_api::register_export_api(lua, builder.export_store.clone())?;
    crate::test_api::register_test_api(lua, body_slot.clone())?;
    crate::dep_output_api::register_dep_output_api(
        lua,
        builder.terminal_outputs.clone(),
        body_slot.clone(),
        builder.alias_dirs.clone(),
        builder.qualified_prefix.clone(),
        builder.alias_qualified_prefixes.clone(),
    )?;
    crate::context::register_resolve_ingredients(lua, &builder.working_dir)?;
    crate::codec_api::register_codec_api(lua)?;
    Ok(module_state)
}

/// Dispatch any `__cook_run_config_blocks` function emitted by codegen
/// and return the final post-dispatch env snapshot.
///
/// When codegen has emitted a config-block dispatcher, this:
///
/// 1. Exposes `env` as an alias of `cook.env` so the block body can write.
/// 2. Calls the dispatcher with the builder's `selected_config`.
/// 3. Freezes the env keyset against the post-dispatch table.
/// 4. Re-applies any `--set KEY=VALUE` CLI overrides on top.
/// 5. Snapshots the env back into the builder's shared `env_vars` map.
/// 6. Emits one §5.2.3 shadowing warning per recipe-name / declared-env
///    collision (deduped via `builder.shadow_warnings_emitted`).
/// 7. Removes the `env` global so subsequent code (recipe bodies)
///    accesses env only through `cook.env`.
///
/// When no config blocks are present, the helper returns the initial
/// `env_vars` map unchanged.
///
/// Extracted from `register_cookfile`'s body so `list_names` can run
/// the same config-block pass without duplicating the env / shadowing
/// logic — listing surfaces MUST observe per-config recipe gating.
fn dispatch_config_blocks(
    lua: &Lua,
    builder: &RegisterSessionBuilder,
    recipes: &[crate::capture::RegisteredRecipe],
) -> Result<BTreeMap<String, String>, RegisterError> {
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

        // §5.2.3 shadowing diagnostic.
        let declared_env: std::collections::BTreeSet<String> =
            builder.env_keyset.declared_list().into_iter().collect();
        let recipe_names_set: std::collections::BTreeSet<String> =
            recipes.iter().map(|r| r.name.clone()).collect();
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
        // canonical source; copy from there.
        let final_env: BTreeMap<String, String> = builder
            .env_vars
            .borrow()
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();

        lua.globals().set("env", mlua::Value::Nil)?;
        Ok(final_env)
    } else {
        // No config blocks — `final_env` is just the initial env_vars.
        Ok(builder
            .env_vars
            .borrow()
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect())
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
