use mlua::prelude::*;
use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::{Arc, Mutex};

use cook_contracts::RecipeUnits;

use crate::capture::install_cook_api;
use crate::context::setup_recipe_context;
use crate::dep_output_api::{SharedMemberOutputs, SharedTerminalOutputs};
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
    member_outputs: SharedMemberOutputs,
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
    /// Root-anchored Cookfile label (workspace-root-relative path of this
    /// member's Cookfile, forward-slashed). Folded into each unit's
    /// `CacheMeta.cookfile_path` so cache identity is invocation-independent
    /// (§20.2.3): the same member gets the same label whether it registers
    /// as the entry (workspace-of-one root) or as an import of an enclosing
    /// workspace. `None` falls back to the cache_ctx-derived relative path,
    /// then the bare "Cookfile" constant.
    cookfile_label: Option<String>,
    /// Frozen keyset of env-var names declared via config blocks.
    /// Shared between the Lua-env-construction call and the config-block
    /// evaluation call so both sides see the same Rc-backed set.
    env_keyset: EnvKeyset,
    /// (recipe, env-var) shadowing pairs we've already warned about, so
    /// we don't repeat the diagnostic on every recipe-register call within
    /// a single Cookfile load.
    shadow_warnings_emitted: Rc<RefCell<std::collections::BTreeSet<(String, String)>>>,
    /// The targeted recipe / chore name (unqualified within this Cookfile).
    /// When set, the body invocation for this recipe will receive `argv` as
    /// bound chore-parameter values (COOK-36 Task 4).
    ///
    /// `None` for non-dispatch paths (e.g. `cook list`, `cook dag`).
    pub(crate) target_recipe: Option<String>,
    /// Positional argv to bind as chore parameters for `target_recipe`.
    /// Empty for normal recipes (which don't accept parameters).
    pub(crate) target_argv: Vec<String>,
}

impl RegisterSessionBuilder {
    pub fn new(working_dir: PathBuf, env_vars: HashMap<String, String>) -> Self {
        Self {
            working_dir,
            env_vars: Rc::new(RefCell::new(env_vars)),
            cli_overrides: HashMap::new(),
            export_store: Rc::new(RefCell::new(BTreeMap::new())),
            terminal_outputs: Arc::new(Mutex::new(BTreeMap::new())),
            member_outputs: Arc::new(Mutex::new(BTreeMap::new())),
            selected_config: None,
            qualified_prefix: String::new(),
            alias_dirs: BTreeMap::new(),
            alias_qualified_prefixes: BTreeMap::new(),
            cookfile_label: None,
            env_keyset: EnvKeyset::new(),
            shadow_warnings_emitted: Rc::new(RefCell::new(std::collections::BTreeSet::new())),
            target_recipe: None,
            target_argv: Vec::new(),
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

    pub fn with_shared_member_outputs(mut self, shared: SharedMemberOutputs) -> Self {
        self.member_outputs = shared;
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

    /// Root-anchored Cookfile label (workspace-root-relative path of this
    /// member's Cookfile, forward-slashed). Folded into each unit's
    /// `CacheMeta.cookfile_path` so cache identity is invocation-independent
    /// (§20.2.3): the same member gets the same label whether it registers
    /// as the entry (workspace-of-one root) or as an import of an enclosing
    /// workspace.
    pub fn with_cookfile_label(mut self, label: String) -> Self {
        self.cookfile_label = Some(label);
        self
    }

    pub fn working_dir(&self) -> &PathBuf {
        &self.working_dir
    }

    /// Set the targeted recipe / chore name and its positional argv.
    ///
    /// When set, `register_cookfile` will pass the bound `__cook_params`
    /// table to the body function of `target` instead of calling it with
    /// no arguments. For normal recipes, `argv` must be empty (the call
    /// will surface `RegisterError::RecipeWithArgv` otherwise).
    pub fn with_target_argv(mut self, target: String, argv: Vec<String>) -> Self {
        self.target_recipe = Some(target);
        self.target_argv = argv;
        self
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

    // 2b. An explicit root-anchored label from the builder wins over the
    //     cache_ctx-derived path (§20.2.3): the workspace register pass
    //     computes the same workspace-root-relative label for a member
    //     whether it registers as the entry Cookfile or as an import, so
    //     `CacheMeta.cookfile_path` — and thus cache identity — cannot
    //     depend on the invocation directory.
    if let Some(ref label) = builder.cookfile_label {
        lua.set_named_registry_value("__cook_cookfile_path", label.clone())
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
    // COOK-64: the register pre-pass populates this with resolved
    // `for_each`-feeding probe values before any recipe body runs; the
    // `cook.cache.get` binding (installed below) reads it first.
    let prepass_store: crate::module_loader::SharedPrepassStore =
        Rc::new(RefCell::new(BTreeMap::new()));
    let module_state = install_remaining_apis(
        &lua,
        &builder,
        body_slot.clone(),
        cache_ctx.as_ref(),
        prepass_store.clone(),
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

    // 11b. (COOK-61) Set of names reachable from `target_recipe` via the
    //      local `requires` graph. The body-invocation loop uses this to
    //      distinguish dep-of-target parametric chores (run with empty argv
    //      per §7.5.1; required-no-default surfaces a legitimate register-time
    //      error) from unrelated parametric siblings (skipped, same as the
    //      no-target case). Empty when `target_recipe` is `None` or the
    //      target isn't in the local set.
    let reachable_from_target: std::collections::BTreeSet<String> = builder
        .target_recipe
        .as_deref()
        .map(|t| local_reachable_set(t, &names_to_requires))
        .unwrap_or_default();

    // 11c. (COOK-64 §22.5.9) The `for_each` register pre-pass. Every recipe
    //      body runs during register to discover its units, and a
    //      probe-sourced `for_each` body opens with
    //      `local _items = cook.cache.get("<key>")`. That call resolves nil
    //      (or errors "outside a module context") unless the feeding probe
    //      has already been evaluated — so we evaluate every probe that feeds
    //      a `for_each` driver (and its transitive probe `requires`) here,
    //      before the body loop, stashing each value in `prepass_store` keyed
    //      by probe key. `$(cmd)` and the reserved `(lua)` sources need no
    //      pre-pass (the former materialises through `cook.sh` at body time).
    run_for_each_prepass(
        &lua,
        &recipes.borrow(),
        &probe_registry.borrow(),
        &builder.env_vars,
        &builder.working_dir,
        cache_ctx.as_ref(),
        &prepass_store,
        &reachable_from_target,
        builder.target_recipe.is_some(),
    )?;

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
            params_meta,
            source_line,
            skip_for_each_body,
        ): (
            LuaRegistryKey,
            Vec<String>,
            crate::capture::RegistrationSource,
            crate::RecipeKind,
            String,
            Vec<crate::capture::ChoreParamMeta>,
            usize,
            bool,
        );
        {
            let registry = recipes.borrow();
            let recipe = registry
                .iter()
                .find(|r| &r.name == name)
                .ok_or_else(|| RegisterError::RecipeNotFound(name.clone()))?;

            // COOK-64 §22.5.9 demand-driven rule: a probe-sourced `for_each`
            // recipe that is NOT reachable from the build target had its probe
            // skipped by the pre-pass, so its body's `cook.cache.get` would
            // error. Skip the body — the recipe is not being built — registering
            // it with no units, mirroring the parametric-sibling skip below.
            skip_for_each_body = builder.target_recipe.is_some()
                && !reachable_from_target.contains(name)
                && matches!(
                    recipe.for_each,
                    Some(crate::capture::ForEachDescriptor::Probe { .. })
                );

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
            params_meta = recipe.metadata.params.clone();
            source_line = match recipe.source {
                crate::capture::RegistrationSource::Static { line } => line,
                crate::capture::RegistrationSource::Dynamic { line } => line,
            };
            qualified_name = if builder.qualified_prefix.is_empty() {
                recipe.name.clone()
            } else {
                format!("{}.{}", builder.qualified_prefix, recipe.name)
            };
        }

        // §22.5.9 demand-driven skip: a non-reachable probe-sourced `for_each`
        // recipe registers with no units (its probe was not pre-evaluated).
        if skip_for_each_body {
            lua.remove_registry_value(func_key_clone)?;
            let _ = body_slot.borrow_mut().take();
            names.push(crate::RegisteredRecipePub {
                name: name.clone(),
                source,
                kind,
                requires: requires.clone(),
            });
            units_by_recipe.insert(
                name.clone(),
                RecipeUnits {
                    recipe_name: name.clone(),
                    deps: requires,
                    units: vec![],
                    step_groups: vec![],
                    working_dir: builder.working_dir.clone(),
                    env_vars: builder
                        .env_vars
                        .borrow()
                        .iter()
                        .map(|(k, v)| (k.clone(), v.clone()))
                        .collect(),
                    terminal_outputs: vec![],
                    dep_edges: vec![],
                    probes: vec![],
                },
            );
            continue;
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
        //
        // COOK-36 Task 4: argv binding for chores.
        //
        // Only the *targeted* chore body gets invoked with a bound
        // `__cook_params` table. Non-targeted chore bodies are skipped
        // entirely: chores produce no cacheable units, so there is nothing
        // useful to capture from a non-targeted chore invocation, and the
        // body would fail with a Lua error if called with nil `__cook_params`
        // while referencing its parameters. Non-targeted recipe bodies are
        // invoked normally (they don't take __cook_params).
        let func: LuaFunction = lua.registry_value(&func_key_clone)?;
        let is_target = builder
            .target_recipe
            .as_deref()
            .map(|t| t == name.as_str())
            .unwrap_or(false);
        if kind == crate::RecipeKind::Chore {
            if is_target {
                // Targeted chore: bind argv and call with __cook_params.
                let argv = &builder.target_argv;
                let (bound, prelude) = build_chore_params_table(
                    &lua,
                    &params_meta,
                    argv,
                    name,
                    source_line,
                )?;
                // Store the prelude on the body slot so cook.add_unit can
                // prepend it to lua_code units captured in this chore body.
                {
                    let mut slot = body_slot.borrow_mut();
                    if let Some(body) = slot.as_mut() {
                        body.chore_param_prelude = prelude;
                    }
                }
                func.call::<()>((bound,))
                    .map_err(RegisterError::Lua)?;
            } else if builder.target_recipe.is_some() {
                // A target was requested but this is NOT the target. Split
                // on `reachable_from_target` (COOK-61): only chores that are
                // actual deps of the target should run with empty argv; an
                // unrelated parametric sibling must be skipped, same as the
                // no-target case (a52063d). Without this split, every
                // parametric sibling poisons every other-target invocation
                // via `build_chore_params_table`'s required-param check.
                //
                // §7.5.1: dep-of-target parametric chores run with no argv
                // supplied; required-no-default surfaces a configuration
                // error here. Unreachable siblings get no such validation —
                // they may declare any param shape.
                let is_dep_of_target = reachable_from_target.contains(name);
                if params_meta.is_empty() {
                    // Paramless chore: cheap to invoke, captures units for
                    // dep linkage when reachable and for enumeration tools
                    // when not. Either way, the body is safe to call with
                    // no argument — no `__cook_params` references.
                    func.call::<()>(()).map_err(RegisterError::Lua)?;
                } else if is_dep_of_target {
                    let (bound, prelude) = build_chore_params_table(
                        &lua,
                        &params_meta,
                        &[],
                        name,
                        source_line,
                    )?;
                    {
                        let mut slot = body_slot.borrow_mut();
                        if let Some(body) = slot.as_mut() {
                            body.chore_param_prelude = prelude;
                        }
                    }
                    func.call::<()>((bound,)).map_err(RegisterError::Lua)?;
                } else {
                    // Parametric sibling not reachable from target — skip
                    // body invocation, mirror the no-target / `cook list`
                    // path: record an empty units entry so downstream stages
                    // see the recipe in the registered set.
                    lua.remove_registry_value(func_key_clone)?;
                    let _ = body_slot.borrow_mut().take();
                    names.push(crate::RegisteredRecipePub {
                        name: name.clone(),
                        source,
                        kind,
                        requires: requires.clone(),
                    });
                    units_by_recipe.insert(
                        name.clone(),
                        RecipeUnits {
                            recipe_name: name.clone(),
                            deps: requires,
                            units: vec![],
                            step_groups: vec![],
                            working_dir: builder.working_dir.clone(),
                            env_vars: builder
                                .env_vars
                                .borrow()
                                .iter()
                                .map(|(k, v)| (k.clone(), v.clone()))
                                .collect(),
                            terminal_outputs: vec![],
                            dep_edges: vec![],
                            probes: vec![],
                        },
                    );
                    continue;
                }
            } else if !params_meta.is_empty() {
                // No specific target requested (e.g. cook list, cook dag,
                // or a test that doesn't set target_recipe) AND this chore
                // declares parameters. Skip the body invocation for the same
                // reason as the targeted-but-not-this-one case above: the
                // body would raise a nil-index Lua error on its first
                // `local NAME = __cook_params.NAME` prelude line. Paramless
                // chores fall through to the regular `func.call::<()>(())`
                // arm below (those bodies don't reference __cook_params, and
                // capturing their units is useful for list/dag enumeration).
                lua.remove_registry_value(func_key_clone)?;
                let _ = body_slot.borrow_mut().take();
                names.push(crate::RegisteredRecipePub {
                    name: name.clone(),
                    source,
                    kind,
                    requires: requires.clone(),
                });
                units_by_recipe.insert(
                    name.clone(),
                    RecipeUnits {
                        recipe_name: name.clone(),
                        deps: requires,
                        units: vec![],
                        step_groups: vec![],
                        working_dir: builder.working_dir.clone(),
                        env_vars: builder
                            .env_vars
                            .borrow()
                            .iter()
                            .map(|(k, v)| (k.clone(), v.clone()))
                            .collect(),
                        terminal_outputs: vec![],
                        dep_edges: vec![],
                        probes: vec![],
                    },
                );
                continue;
            } else {
                // No specific target, paramless chore — call normally. This
                // path preserves the pre-COOK-36 behavior of capturing chore
                // units during list/dag enumeration so tools can inspect them.
                func.call::<()>(()).map_err(RegisterError::Lua)?;
            }
        } else {
            // Normal recipe: validate that no argv was supplied (§7.1.2).
            if is_target && !builder.target_argv.is_empty() {
                return Err(RegisterError::RecipeWithArgv {
                    name: name.clone(),
                    supplied: builder.target_argv.len(),
                });
            }
            func.call::<()>(()).map_err(RegisterError::Lua)?;
        }
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
        // Per-body attribution happens here at drain time using the LOCAL
        // registered name (§20.2.3): the qualified prefix is assigned by the
        // importer, so folding it into cache identity would key the same
        // recipe differently depending on which Cookfile served as the entry
        // point. Every StepEntry-index read/write path derives the index name
        // from `meta.recipe_name` itself, so local naming propagates
        // consistently; per-Cookfile `.cook/cache` anchoring (working_dir)
        // prevents cross-Cookfile collisions of the index file. Downstream
        // engine cache keying depends on cache_meta.recipe_name being the
        // actual recipe name, not "".
        for unit in &mut body.units {
            if let Some(meta) = unit.cache_meta.as_mut() {
                meta.recipe_name = name.clone();
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

        // COOK-96: build the per-member output map for $<recipe[]> joins. Mirror
        // terminal-output keying (qualified_name); last-wins per member across step
        // groups matches last_cook_step_outputs' last-wins.
        {
            let mut mo = builder
                .member_outputs
                .lock()
                .expect("member_outputs mutex poisoned");
            let entry = mo.entry(qualified_name.clone()).or_default();
            for unit in &body.units {
                if let Some(m) = &unit.member {
                    if !unit.output_paths.is_empty() {
                        entry.insert(m.clone(), unit.output_paths.clone());
                    }
                }
            }
        }

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

    // 12b. (COOK-64 §22.5.9) Static-input rule for `for_each` sources. Now
    //      that every body has run and `units_by_recipe` holds the full set of
    //      recipe outputs, reject any `for_each`-feeding probe that declares a
    //      file input which is produced by a recipe in this Cookfile — a
    //      build artifact is not statically evaluable (the pre-pass resolved
    //      the probe before any recipe ran, so it could only have seen a
    //      stale or absent file).
    check_for_each_static_inputs(
        &recipes.borrow(),
        &probe_registry.borrow(),
        &units_by_recipe,
    )?;

    // Flush module caches once at the end of the pass.
    module_state.borrow().flush_all();

    // 13. Probes view: BTreeMap keyed by probe key (deterministic).
    //
    // Source is the probe_registry (not session_state.probes) so that
    // body-scope probes — registered during the recipe-body invocations
    // in step 12, after the session_state drain in step 10 — are also
    // included. The workspace-level probes map feeds the executor's
    // probe-cache fast path (cli/crates/cook-engine/src/run.rs builds
    // `probe_units_by_node` from this map); a body-scope probe that
    // doesn't appear here would re-execute on every run instead of
    // hitting the cache (CS-0074 §22.5.7).
    let probes: BTreeMap<String, cook_contracts::ProbeUnit> = probe_registry
        .borrow()
        .probes
        .iter()
        .map(|(key, reg)| (key.clone(), reg.probe.clone()))
        .collect();

    Ok(crate::RegisteredCookfile {
        names,
        units_by_recipe,
        probes,
        final_env,
    })
}

/// Build the `__cook_params` Lua table from declared parameter metadata and
/// supplied argv (COOK-36 Task 4).
///
/// Matches each element of `params_meta` in order against `argv`:
/// - `Required`: pops the next argv element; errors if argv is exhausted.
/// - `DefaultedString`: uses the next argv element if present; falls back
///   to the declared default string otherwise.
///
/// After all parameters are satisfied, any remaining argv elements are an
/// error (`ChoreTooManyArgv`). Each bound value is set on a fresh Lua table
/// under the parameter's declared name.
///
/// Also returns a Lua source prelude (`local NAME = "VALUE"\n` lines) that
/// the caller can set on the `BodyCaptureState.chore_param_prelude` field
/// so `cook.add_unit`'s `lua_code` units automatically include the bindings
/// in the execute-phase worker VM.
fn build_chore_params_table(
    lua: &Lua,
    params_meta: &[crate::capture::ChoreParamMeta],
    argv: &[String],
    chore_name: &str,
    source_line: usize,
) -> Result<(LuaTable, String), RegisterError> {
    use crate::capture::ChoreParamMeta;

    let table = lua.create_table().map_err(RegisterError::Lua)?;
    let mut argv_iter = argv.iter().peekable();
    let mut prelude = String::new();
    // Once a variadic absorbs the remaining argv, no further params are processed
    // and the too-many-argv check is suppressed.
    let mut variadic_consumed = false;

    for param in params_meta {
        match param {
            ChoreParamMeta::Required { name } => {
                let value = argv_iter.next().ok_or_else(|| RegisterError::ChoreParamMissing {
                    chore: chore_name.to_string(),
                    name: name.clone(),
                    line: source_line,
                })?;
                table.set(name.as_str(), value.as_str()).map_err(RegisterError::Lua)?;
                // Escape the value for Lua string literal.
                let escaped = lua_escape_string(value);
                prelude.push_str(&format!("local {} = \"{}\"\n", name, escaped));
            }
            ChoreParamMeta::DefaultedString { name, default } => {
                let value = argv_iter
                    .next()
                    .map(|s| s.as_str())
                    .unwrap_or(default.as_str());
                table.set(name.as_str(), value).map_err(RegisterError::Lua)?;
                let escaped = lua_escape_string(value);
                prelude.push_str(&format!("local {} = \"{}\"\n", name, escaped));
            }
            ChoreParamMeta::DefaultedLua { name, default_key_name } => {
                if let Some(arg) = argv_iter.next() {
                    table.set(name.as_str(), arg.as_str()).map_err(RegisterError::Lua)?;
                    let escaped = lua_escape_string(arg);
                    prelude.push_str(&format!("local {} = \"{}\"\n", name, escaped));
                } else {
                    // Retrieve and call the default closure.
                    let func: LuaFunction = lua
                        .named_registry_value(default_key_name.as_str())
                        .map_err(RegisterError::Lua)?;
                    let result: mlua::Value = match func.call::<mlua::Value>(()) {
                        Ok(v) => v,
                        Err(e) => {
                            return Err(RegisterError::ChoreParamDefaultLuaError {
                                chore: chore_name.to_string(),
                                name: name.clone(),
                                line: source_line,
                                message: e.to_string(),
                            });
                        }
                    };
                    // Spec §7.1.2: result is coerced via Lua tostring rules
                    // for the scalar types (String, Integer, Number, Boolean).
                    // Non-coercible types (Nil, Table, Function, Thread,
                    // UserData, LightUserData, Error) raise ChoreParamDefaultLuaNonString.
                    let coerced: Option<String> = match &result {
                        mlua::Value::String(s) => s.to_str()
                            .map_err(RegisterError::Lua)?
                            .to_string()
                            .into(),
                        mlua::Value::Integer(n) => Some(n.to_string()),
                        mlua::Value::Number(n) => Some(n.to_string()),
                        mlua::Value::Boolean(b) => Some(b.to_string()),
                        _ => None,
                    };
                    match coerced {
                        Some(s_str) => {
                            table.set(name.as_str(), s_str.as_str()).map_err(RegisterError::Lua)?;
                            let escaped = lua_escape_string(&s_str);
                            prelude.push_str(&format!("local {} = \"{}\"\n", name, escaped));
                        }
                        None => {
                            return Err(RegisterError::ChoreParamDefaultLuaNonString {
                                chore: chore_name.to_string(),
                                name: name.clone(),
                                line: source_line,
                                ty: result.type_name().to_string(),
                            });
                        }
                    }
                }
            }
            ChoreParamMeta::VariadicPlus { name } => {
                // Collect ALL remaining argv elements.
                let values: Vec<String> = argv_iter.by_ref().cloned().collect();
                if values.is_empty() {
                    return Err(RegisterError::ChoreVariadicEmpty {
                        chore: chore_name.to_string(),
                        name: name.clone(),
                        line: source_line,
                    });
                }
                // Build Lua sequence table.
                let seq = lua
                    .create_sequence_from(values.iter().map(|s| s.as_str()))
                    .map_err(RegisterError::Lua)?;
                table.set(name.as_str(), seq).map_err(RegisterError::Lua)?;
                // Build execute-phase prelude: `local NAME = {"a", "b", "c"}`
                let items: Vec<String> = values
                    .iter()
                    .map(|v| format!("\"{}\"", lua_escape_string(v)))
                    .collect();
                prelude.push_str(&format!("local {} = {{{}}}\n", name, items.join(", ")));
                variadic_consumed = true;
            }
            ChoreParamMeta::VariadicStar { name } => {
                // Collect ALL remaining argv elements (zero is fine).
                let values: Vec<String> = argv_iter.by_ref().cloned().collect();
                let seq = if values.is_empty() {
                    lua.create_table().map_err(RegisterError::Lua)?
                } else {
                    lua.create_sequence_from(values.iter().map(|s| s.as_str()))
                        .map_err(RegisterError::Lua)?
                };
                table.set(name.as_str(), seq).map_err(RegisterError::Lua)?;
                // Build execute-phase prelude (empty table or populated).
                let items: Vec<String> = values
                    .iter()
                    .map(|v| format!("\"{}\"", lua_escape_string(v)))
                    .collect();
                prelude.push_str(&format!("local {} = {{{}}}\n", name, items.join(", ")));
                variadic_consumed = true;
            }
        }
    }

    if !variadic_consumed {
        let remaining: Vec<&String> = argv_iter.collect();
        if !remaining.is_empty() {
            // COOK-36 Task 9: when a paramless chore (declared==0) receives
            // exactly one extra positional, surface it in first_unmatched so
            // the engine-level From impl can append a migration hint.
            let first_unmatched = if params_meta.is_empty() && remaining.len() == 1 {
                remaining[0].clone()
            } else {
                String::new()
            };
            return Err(RegisterError::ChoreTooManyArgv {
                chore: chore_name.to_string(),
                declared: params_meta.len(),
                supplied: argv.len(),
                first_unmatched,
            });
        }
    }

    Ok((table, prelude))
}

/// Escape a string value for inclusion in a Lua double-quoted string literal.
///
/// Only escapes characters that would break the literal if unescaped:
/// `\`, `"`, newline, carriage return, and NUL. This is sufficient for the
/// chore-param prelude use case (param values are CLI argv strings).
fn lua_escape_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\0' => out.push_str("\\0"),
            other => out.push(other),
        }
    }
    out
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

/// Set of names reachable from `target` via the local `requires` graph,
/// including `target` itself when it is present in `deps`. Returns an empty
/// set when `target` is not a key of `deps` (e.g. cross-Cookfile target or
/// unknown name — handled by the engine's analyzer downstream).
///
/// Mirrors `local_topological_sort`'s policy of skipping refs the local set
/// doesn't know about: cross-Cookfile `requires` edges are resolved later by
/// the engine's cross-cookfile dep analyzer.
///
/// Used by `register_cookfile` (COOK-61) to distinguish parametric chores
/// that are actual deps of the dispatch target (run with empty argv per
/// §7.5.1) from unrelated parametric siblings (skipped, same as the
/// no-target case).
fn local_reachable_set(
    target: &str,
    deps: &BTreeMap<String, Vec<String>>,
) -> std::collections::BTreeSet<String> {
    let mut reachable: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    if !deps.contains_key(target) {
        return reachable;
    }
    let mut stack: Vec<String> = vec![target.to_string()];
    while let Some(node) = stack.pop() {
        if !reachable.insert(node.clone()) {
            continue;
        }
        if let Some(children) = deps.get(&node) {
            for child in children {
                if deps.contains_key(child) && !reachable.contains(child) {
                    stack.push(child.clone());
                }
            }
        }
    }
    reachable
}

/// COOK-64 §22.5.9: the `for_each` register pre-pass.
///
/// Every probe-sourced `for_each` driver opens its body with
/// `local _items = cook.cache.get("<key>")`. That value does not exist until
/// the feeding probe runs, and probes normally run as DAG nodes in the
/// execute phase — far too late for register-time fan-out. So we evaluate
/// every `for_each`-feeding probe (and its transitive probe `requires`) here,
/// synchronously on the register VM, before any recipe body runs.
///
/// The evaluation mirrors the execute-phase probe path (`executor.rs` G4/G5):
/// resolve declared inputs → fingerprint → cache GET → on a miss run
/// `produce` on the VM → cache PUT. The resolved value is stashed in
/// `prepass_store` keyed by probe key, where the `cook.cache.get` binding
/// reads it. When no `CacheContext` is wired (tests / `list_names`), `produce`
/// runs uncached.
///
/// Only `ProbeKey` sources require a pre-pass; the `$(cmd)` and `(lua)` sources
/// were removed in COOK-97.
#[allow(clippy::too_many_arguments)]
fn run_for_each_prepass(
    lua: &Lua,
    recipes: &[crate::capture::RegisteredRecipe],
    probe_registry: &ProbeRegistry,
    env_vars: &Rc<RefCell<HashMap<String, String>>>,
    working_dir: &Path,
    cache_ctx: Option<&Arc<cook_cache::cache_ctx::CacheContext>>,
    prepass_store: &crate::module_loader::SharedPrepassStore,
    reachable_from_target: &std::collections::BTreeSet<String>,
    has_target: bool,
) -> Result<(), RegisterError> {
    use crate::capture::ForEachDescriptor;

    // (recipe, probe key, optional `:field` selector) per probe-sourced driver.
    //
    // §22.5.9 demand-driven rule: when a build target is set, only evaluate
    // probes for recipes reachable from it. When no target is set every recipe
    // is being built, so every probe-sourced driver is in scope. The body loop
    // applies the mirror rule (`should_skip_for_each_body`) so a non-reachable
    // driver's body — which would call `cook.cache.get` on an unevaluated probe
    // — is skipped rather than erroring.
    let driver_reachable = |name: &str| !has_target || reachable_from_target.contains(name);
    let drivers: Vec<(&str, &str, Option<&str>)> = recipes
        .iter()
        .filter(|r| driver_reachable(&r.name))
        .filter_map(|r| match &r.for_each {
            Some(ForEachDescriptor::Probe { key, field }) => {
                Some((r.name.as_str(), key.as_str(), field.as_deref()))
            }
            _ => None,
        })
        .collect();
    if drivers.is_empty() {
        return Ok(());
    }

    // Each driver's probe must be declared (§22.5.9).
    for (recipe, key, _) in &drivers {
        if !probe_registry.probes.contains_key(*key) {
            return Err(RegisterError::ForEachProbeUndeclared {
                recipe: (*recipe).to_string(),
                key: (*key).to_string(),
            });
        }
    }

    let env_lookup = |name: &str| env_vars.borrow().get(name).cloned();
    let mut upstream_fps: BTreeMap<String, [u8; 32]> = BTreeMap::new();
    let mut done: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();

    // Evaluate each driver probe (and its transitive `requires`) in
    // dependency order. Probe cycles are already rejected (step 9 above), so
    // the recursion terminates; `in_progress` is a defensive belt-and-braces.
    for (_, key, _) in &drivers {
        evaluate_prepass_probe(
            key,
            lua,
            probe_registry,
            &env_lookup,
            working_dir,
            cache_ctx,
            prepass_store,
            &mut upstream_fps,
            &mut done,
            &mut Vec::new(),
        )?;
    }

    // §22.5.9 non-array diagnostic: a driver's resolved source must be a
    // sequence. With a `:field` selector, the named field must be the array.
    for (_, key, field) in &drivers {
        let store = prepass_store.borrow();
        let value = store.get(*key).expect("driver probe evaluated above");
        let (resolved, selector): (&serde_json::Value, String) = match field {
            Some(f) => match json_map_get(value, f) {
                Some(v) => (v, format!("{key}:{f}")),
                None => {
                    return Err(RegisterError::ForEachNotArray {
                        selector: format!("{key}:{f}"),
                        shape: "nil (no such field)".to_string(),
                    })
                }
            },
            None => (value, (*key).to_string()),
        };
        if !matches!(resolved, serde_json::Value::Array(_)) {
            return Err(RegisterError::ForEachNotArray {
                selector,
                shape: json_shape(resolved).to_string(),
            });
        }
    }

    Ok(())
}

/// Evaluate a single probe for the pre-pass, recursing through its declared
/// probe `requires` first so their fingerprints feed this one's (§22.5.3).
/// Idempotent via `done`; `in_progress` guards against (already-rejected)
/// cycles. Stores the decoded value in `prepass_store` and records the
/// fingerprint in `upstream_fps`.
#[allow(clippy::too_many_arguments)]
fn evaluate_prepass_probe(
    key: &str,
    lua: &Lua,
    probe_registry: &ProbeRegistry,
    env_lookup: &dyn Fn(&str) -> Option<String>,
    working_dir: &Path,
    cache_ctx: Option<&Arc<cook_cache::cache_ctx::CacheContext>>,
    prepass_store: &crate::module_loader::SharedPrepassStore,
    upstream_fps: &mut BTreeMap<String, [u8; 32]>,
    done: &mut std::collections::BTreeSet<String>,
    in_progress: &mut Vec<String>,
) -> Result<(), RegisterError> {
    if done.contains(key) {
        return Ok(());
    }
    let Some(reg) = probe_registry.probes.get(key) else {
        return Err(RegisterError::ForEachProbeProduceFailed {
            key: key.to_string(),
            message: format!("requires upstream probe '{key}' which was not declared"),
        });
    };
    let probe = &reg.probe;

    if in_progress.iter().any(|k| k == key) {
        return Ok(()); // defensive — step 9 already rejects probe cycles
    }
    in_progress.push(key.to_string());
    for req in &probe.inputs.requires {
        evaluate_prepass_probe(
            req,
            lua,
            probe_registry,
            env_lookup,
            working_dir,
            cache_ctx,
            prepass_store,
            upstream_fps,
            done,
            in_progress,
        )?;
    }
    in_progress.pop();

    // Resolve fingerprint inputs + compute fingerprint (mirrors executor G4).
    let inputs =
        cook_fingerprint::probe::resolve_probe_inputs(probe, working_dir, env_lookup, upstream_fps)
            .map_err(|e| RegisterError::ForEachProbeProduceFailed {
                key: key.to_string(),
                message: e,
            })?;
    let fp = cook_fingerprint::compute_probe_fingerprint(&inputs);

    // Cache GET when a backend is wired; on a miss (or no backend) run
    // `produce` on the register VM, then PUT. Cached bytes that do not parse
    // as probe-value JSON are treated as a miss, never a hard error (CS-0102
    // stale-artifact defence — the V2 fingerprint marker already makes
    // pre-CS-0102 entries unreachable; this is the second layer).
    let bytes: Vec<u8> = match cache_ctx {
        Some(ctx) => {
            let cached = match cook_cache::backend::get_bytes(ctx.backend.as_ref(), &fp) {
                Ok(Some(b)) if cook_contracts::probe_value::decode_json(&b).is_ok() => Some(b),
                Ok(Some(_)) => {
                    eprintln!(
                        "cook: warning: probe '{key}': cached bytes are not \
                         probe-value JSON (pre-CS-0102 artifact?); treating as miss"
                    );
                    // Evict the stale entry so the put below can self-heal the
                    // key (CS-0055 conflict detection rejects overwrites with
                    // differing bytes).
                    let _ = ctx.backend.delete(&fp);
                    None
                }
                _ => None,
            };
            match cached {
                Some(b) => b,
                None => {
                    let b = run_prepass_produce(lua, key, &probe.produce_source)?;
                    let mut meta = cook_fingerprint::ArtifactMeta {
                        recipe_namespace: format!("probe:{key}"),
                        command_hash: 0,
                        env_contribution: 0,
                        seal_contribution: 0,
                        schema_version: cook_fingerprint::CACHE_VERSION,
                        size_bytes: b.len() as u64,
                        tags: std::collections::BTreeSet::new(),
                        consulted_env_keys: std::collections::BTreeSet::new(),
                        output_index: 0,
                        output_path: format!("probe:{key}"),
                        content_hash: cook_fingerprint::ArtifactMeta::zero_content_hash(),
                        kind: None,
                        mode: cook_fingerprint::ArtifactMeta::default_mode(),
                        target: None,
                    }
                    .as_probe_value();
                    // A cache PUT failure is non-fatal — the value is already in
                    // hand for this pass; we simply forgo persisting it.
                    let _ =
                        cook_cache::backend::put_bytes(ctx.backend.as_ref(), &fp, &b, &mut meta);
                    b
                }
            }
        }
        None => run_prepass_produce(lua, key, &probe.produce_source)?,
    };

    // CS-0102: materialise the canonical local copy at .cook/probes/<key>.json.
    // Non-fatal on failure — the in-memory value is already in hand.
    let probes_dir = cache_ctx
        .map(|ctx| ctx.project_root.clone())
        .unwrap_or_else(|| working_dir.to_path_buf())
        .join(".cook")
        .join("probes");
    if let Err(e) = cook_contracts::probe_value::write_probe_file(&probes_dir, key, &bytes) {
        eprintln!(
            "cook: warning: probe '{key}': failed to write {}: {e}",
            probes_dir.display()
        );
    }

    let jv = cook_contracts::probe_value::decode_json(&bytes).map_err(|e| {
        RegisterError::ForEachProbeProduceFailed {
            key: key.to_string(),
            message: format!("decode cached value: {e}"),
        }
    })?;
    prepass_store.borrow_mut().insert(key.to_string(), jv);
    upstream_fps.insert(key.to_string(), fp);
    done.insert(key.to_string());
    Ok(())
}

/// Run a probe's `produce` source on the register VM and return the
/// canonical-JSON bytes (mirrors `cook-luaotp`'s execute-VM `execute_probe`;
/// CS-0102).
fn run_prepass_produce(lua: &Lua, key: &str, produce: &str) -> Result<Vec<u8>, RegisterError> {
    let wrapped = format!("return (function()\n{}\nend)()", produce);
    let chunk = format!("@probe:{key}");
    let value: LuaValue = lua
        .load(&wrapped)
        .set_name(&chunk)
        .eval()
        .map_err(|e| RegisterError::ForEachProbeProduceFailed {
            key: key.to_string(),
            message: e.to_string(),
        })?;
    let jv = crate::probe_value::lua_to_json(&value).map_err(|e| {
        RegisterError::ForEachProbeProduceFailed {
            key: key.to_string(),
            message: e,
        }
    })?;
    Ok(crate::probe_value::encode_canonical_json(&jv))
}

/// Look up a string-keyed field in a JSON object. `None` for non-objects or a
/// missing key.
fn json_map_get<'a>(v: &'a serde_json::Value, field: &str) -> Option<&'a serde_json::Value> {
    v.as_object().and_then(|m| m.get(field))
}

/// COOK-64 §22.5.9 static-input rule: reject a `for_each`-feeding probe whose
/// declared file inputs include a build artifact (an output produced by a
/// recipe in this Cookfile). `for_each` sources are resolved by the pre-pass
/// before any recipe runs, so depending on a not-yet-built file is incoherent.
///
/// Runs after the body loop, when `units_by_recipe` carries every recipe's
/// output paths. Paths are compared in normalised relative form.
fn check_for_each_static_inputs(
    recipes: &[crate::capture::RegisteredRecipe],
    probe_registry: &ProbeRegistry,
    units_by_recipe: &BTreeMap<String, RecipeUnits>,
) -> Result<(), RegisterError> {
    use crate::capture::ForEachDescriptor;

    // Union of every recipe output path (normalised).
    let outputs: std::collections::BTreeSet<String> = units_by_recipe
        .values()
        .flat_map(|ru| ru.units.iter())
        .filter_map(|u| u.cache_meta.as_ref())
        .flat_map(|m| m.output_paths.iter())
        .map(|p| normalise_rel(p))
        .collect();
    if outputs.is_empty() {
        return Ok(());
    }

    for recipe in recipes {
        let Some(ForEachDescriptor::Probe { key, .. }) = &recipe.for_each else {
            continue;
        };
        let Some(reg) = probe_registry.probes.get(key) else {
            continue; // undeclared probe already rejected by the pre-pass
        };
        for file in &reg.probe.inputs.files {
            if outputs.contains(&normalise_rel(file)) {
                return Err(RegisterError::ForEachProbeArtifactDep {
                    key: key.clone(),
                    path: file.clone(),
                });
            }
        }
    }
    Ok(())
}

/// Normalise a relative path for comparison: drop a leading `./`. Both
/// `for_each` probe file inputs and recipe output paths are relative to the
/// project working directory, so a textual normalise suffices.
fn normalise_rel(p: &str) -> String {
    p.strip_prefix("./").unwrap_or(p).to_string()
}

/// Human-readable JSON value-kind, for the §22.5.9 non-array diagnostic.
fn json_shape(v: &serde_json::Value) -> &'static str {
    match v {
        serde_json::Value::Null => "nil",
        serde_json::Value::Bool(_) => "boolean",
        serde_json::Value::Number(_) => "number",
        serde_json::Value::String(_) => "string",
        serde_json::Value::Array(_) => "array",
        serde_json::Value::Object(_) => "map/record",
    }
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
    // `list_names` never invokes recipe bodies, so the pre-pass store stays
    // empty — `cook.cache.get` falls through to its module-context behaviour.
    let _module_state = install_remaining_apis(
        &lua,
        &builder,
        body_slot.clone(),
        None,
        Rc::new(RefCell::new(BTreeMap::new())),
    )?;

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
    prepass: crate::module_loader::SharedPrepassStore,
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
    crate::module_loader::register_cache_api(lua, module_state.clone(), prepass)?;
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
    crate::dep_output_api::register_member_output_api(
        lua,
        builder.member_outputs.clone(),
        body_slot.clone(),
        builder.qualified_prefix.clone(),
        builder.alias_qualified_prefixes.clone(),
    )?;
    crate::context::register_resolve_ingredients(lua, &builder.working_dir)?;
    // CS-0101: cook.file_ref — register-phase resolution of `$<file:PATH>`
    // placeholders (hoisted locals emitted by cook-luagen).
    crate::file_ref::register_file_ref(lua, &builder.working_dir)?;
    // cook.json_decode / cook.yaml_decode are both-phase (§24.8, CS-0123);
    // the shared implementation lives in cook-lua-stdlib so the worker VMs
    // in cook-luaotp install byte-identical behaviour.
    let cook_tbl: LuaTable = lua.globals().get("cook")?;
    cook_lua_stdlib::register_codec_api(lua, &cook_tbl)?;
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
