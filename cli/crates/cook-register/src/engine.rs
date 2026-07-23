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
    workspace_root: PathBuf,
    ingredient_warnings: Rc<RefCell<Vec<String>>>,
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
        let workspace_root = working_dir.clone();
        Self {
            working_dir,
            workspace_root,
            ingredient_warnings: Rc::new(RefCell::new(Vec::new())),
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
    pub fn with_workspace_root(mut self, root: PathBuf) -> Self { self.workspace_root = root; self }

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
/// 12. Invoke every recipe body via the re-entrant `BodyDriver`, opening a
///     fresh `BodyCaptureState` immediately before each call and draining it
///     back immediately after. Step 11's sort is the SEED order, not the
///     authority: `cook.require_recipe` (Standard §22.8) lets a body declare
///     a dependency edge while running, which a sort computed before any
///     body ran cannot know — so the driver forces bodies on demand and
///     merges the resulting edges into `requires` at drain.
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
    // Shared with the body-invocation driver (step 12), which outlives this
    // function's stack frame: the Lua closure backing `cook.require_recipe`
    // holds it for as long as the VM does.
    let builder = Rc::new(builder);

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
    // `cook.probes.get` binding (installed below) reads it first.
    let prepass_store: crate::module_loader::SharedPrepassStore =
        Rc::new(RefCell::new(BTreeMap::new()));
    // The forcer cell `cook.require_recipe` reads at call time. Created here,
    // filled at step 12 once the driver exists: the top-level chunk (step 6)
    // runs long before that, and a Cookfile aliasing the function there
    // (`local rr = cook.require_recipe`) keeps whatever closure it captured
    // forever. One closure over a late-filled cell is what keeps the alias and
    // a fresh `cook.` lookup indistinguishable (Standard §22.8, CS-0144).
    let recipe_forcer: crate::context::SharedRecipeForcer = Rc::new(RefCell::new(None));
    // Standard §22.9, CS-0149: the `cook.on_register_complete` finalizer
    // queue for this pass. Created here, alongside the forcer cell, for the
    // same reason: `install_remaining_apis` needs it at install time (step
    // 5b, below), but nothing drains it until step 12c, well after the
    // top-level chunk and every recipe body have queued into it.
    let finalizer_queue: crate::on_register_api::SharedFinalizerQueue =
        Rc::new(RefCell::new(Vec::new()));
    let module_state = install_remaining_apis(
        &lua,
        &builder,
        body_slot.clone(),
        cache_ctx.as_ref(),
        prepass_store.clone(),
        recipe_forcer.clone(),
        finalizer_queue.clone(),
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
    //    blocks are present. `host_reads` collects the config's `host.*`
    //    reads for provenance (Standard §5.3.2).
    let host_reads: crate::config_sandbox::SharedHostReads =
        Rc::new(RefCell::new(Vec::new()));
    let final_env =
        dispatch_config_blocks(&lua, &builder, &recipes.borrow(), &host_reads)?;

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
    //      `local _items = cook.probes.get("<key>")`. That call resolves nil
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

    // 12. Invoke every registered recipe body, demand-driven.
    //
    //     `topo` is the SEED order, not the authority: it is sorted from the
    //     STATIC `requires` graph before any body runs, so it structurally
    //     cannot know about an edge a body declares at run time via
    //     `cook.require_recipe` (Standard §22.8, CS-0144). The driver's
    //     re-entrant `ensure_invoked` is what actually orders the pass; the
    //     seed loop just guarantees every recipe is reached.
    //
    //     `ensure_invoked` recurses into BOTH edge kinds — a recipe's local
    //     static `requires` and the dynamic edges its body declares — so it is
    //     a full DFS over the requires graph and `topo` is NOT load-bearing for
    //     correctness. It only pins a deterministic order among INDEPENDENT
    //     recipes. That matters: any order that reaches every recipe would be
    //     correct, so leaning on this one to have already evaluated a forced
    //     recipe's static deps is exactly the assumption that let a forced body
    //     jump ahead of its own dep list and read a nil export.
    //
    //     A Cookfile that never calls the API invokes bodies in exactly the old
    //     order: `topo` already yields deps-first, so every recursion the driver
    //     adds finds its target already visited and is a no-op.
    let driver = Rc::new(BodyDriver {
        builder: builder.clone(),
        recipes: recipes.clone(),
        probe_registry: probe_registry.clone(),
        session_state: session_state.clone(),
        body_slot: body_slot.clone(),
        reachable_from_target,
        units_by_recipe: RefCell::new(BTreeMap::new()),
        names: RefCell::new(Vec::with_capacity(topo.len())),
        visit: RefCell::new(BTreeMap::new()),
        path: RefCell::new(Vec::new()),
    });

    // Fill the forcer cell now that the driver exists. The `cook.require_recipe`
    // closure installed at step 5b reads this cell at CALL time, so filling it
    // reaches every caller at once — including one that aliased the function
    // during the top-level chunk, which no amount of re-registering the `cook`
    // table entry could reach. Nothing has called it yet: the pre-pass and the
    // top-level chunk are both outside a recipe body, where the guard rail
    // fires ahead of the forcer.
    {
        let forcer_driver = driver.clone();
        *recipe_forcer.borrow_mut() =
            Some(Rc::new(move |lua: &Lua, name: &str| forcer_driver.force(lua, name)));
    }

    for name in &topo {
        driver.ensure_invoked(&lua, name, false)?;
    }

    let units_by_recipe = std::mem::take(&mut *driver.units_by_recipe.borrow_mut());
    let names = std::mem::take(&mut *driver.names.borrow_mut());

    // 12a. (Standard §22.8, CS-0144) Cycle detection over the MERGED
    //      `requires` — static dep-lists plus every edge the bodies just
    //      declared. Redundant today: the driver traverses every edge source
    //      that can reach the merged `requires`, because `force()` runs before
    //      the edge is recorded, so its `Visiting` check already catches every
    //      cycle here. Retained as a cheap guard (one in-memory sort per pass;
    //      it can only ever reject) because the failure mode if a future edge
    //      source bypasses the driver is a silently accepted cycle.
    {
        let merged: BTreeMap<String, Vec<String>> = names
            .iter()
            .map(|r| (r.name.clone(), r.requires.clone()))
            .collect();
        local_topological_sort(&merged)?;
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

    // 12c. (Standard §22.9, CS-0149) Drain the `cook.on_register_complete`
    //      finalizer queue. Sits exactly here — after 12a/12b, before
    //      `flush_all` below — for two reasons pulling in opposite
    //      directions on the same boundary:
    //
    //      * AFTER 12a/12b: those two checks are what "the recipe and unit
    //        set is closed" MEANS in this pass. A callback that ran before
    //        either could observe a units_by_recipe / probe_registry that
    //        the merged-cycle or for_each static-input check would still
    //        reject, or race a `cook.recipe`/`cook.probe` call against a
    //        validation pass not yet run over the very state it just
    //        mutated — so callbacks must see a pass that has ALREADY been
    //        accepted, not one still being decided.
    //
    //      * BEFORE `flush_all`: module-held per-VM state must still be live
    //        when a callback runs, and any mutation a callback makes to it
    //        must be visible to the flush that commits it into the
    //        register→execute handoff. Draining after the flush would let
    //        a callback's `cook.load_module`-returned state mutations go
    //        uncommitted.
    //
    //      Drain-until-empty, not a fixed-length pass over the queue as it
    //      stood when this loop started: §22.9 lets a callback itself call
    //      `cook.on_register_complete`, and the newly queued callback MUST
    //      still run this pass, in append order. An index cursor (rather
    //      than `Vec::drain` up front) is what makes that safe — the vec
    //      can grow while we're mid-walk.
    {
        let mut cursor = 0usize;
        loop {
            let next = finalizer_queue.borrow().get(cursor).cloned();
            let callback = match next {
                Some(cb) => cb,
                None => break,
            };
            cursor += 1;

            // §22.9: "Recipe and probe registration is rejected." A
            // callback runs after the body-invocation loop above has
            // already closed the recipe and unit sets — a `cook.recipe` or
            // `cook.probe` call reached from here would never have its
            // body evaluated in this pass, so accepting it would silently
            // reproduce the exact register/execute disagreement §22.8's
            // forcing rules exist to prevent. Both `cook.recipe` and
            // `cook.probe` still WORK when called from a callback (neither
            // API checks `body_slot`, and body_slot is legitimately `None`
            // here just as it is at top level) — so this is enforced by
            // snapshotting each registry's size around the call and
            // rejecting growth afterward, rather than by teaching either
            // installer about finalizer callbacks.
            let pre_recipes = recipes.borrow().len();
            let pre_probes = probe_registry.borrow().probes.len();

            let call_result: LuaResult<()> = callback.call(());
            call_result.map_err(RegisterError::Lua)?;

            let post_recipes = recipes.borrow().len();
            let post_probes = probe_registry.borrow().probes.len();
            if post_recipes > pre_recipes {
                return Err(RegisterError::Lua(mlua::Error::runtime(
                    "cook.on_register_complete: a callback called cook.recipe, but the \
                     register pass's recipe set is already closed by the time any callback \
                     runs — the new recipe's body would never be evaluated this pass. Move \
                     the registration before cook.on_register_complete is called (Standard \
                     \u{00a7}22.9, CS-0149)",
                )));
            }
            if post_probes > pre_probes {
                return Err(RegisterError::Lua(mlua::Error::runtime(
                    "cook.on_register_complete: a callback called cook.probe, but the \
                     register pass's probe set is already closed by the time any callback \
                     runs — the new probe would never be evaluated this pass. Move the \
                     registration before cook.on_register_complete is called (Standard \
                     \u{00a7}22.9, CS-0149)",
                )));
            }
        }
    }

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

    let warnings = builder.ingredient_warnings.borrow().clone();
    Ok(crate::RegisteredCookfile {
        names,
        units_by_recipe,
        probes,
        final_env,
        warnings,
        config_host_reads: host_reads.take(),
    })
}

/// What one `invoke_body` call actually did — the distinction `ensure_invoked`
/// turns into `VisitState::Visited` vs `VisitState::Skipped`.
enum Outcome {
    /// The body was evaluated to completion. The register-order guarantee is
    /// satisfied for this recipe.
    Ran,
    /// A skip arm declined to evaluate the body and registered the recipe with
    /// no units. The guarantee is NOT satisfied; a force must re-invoke.
    Skipped,
}

/// Where a recipe's body sits in the current pass's demand-driven visit.
///
/// `Clone` only — the map is read by `match`, never compared, and
/// `mlua::Error` is not `PartialEq`.
#[derive(Clone)]
enum VisitState {
    /// Its body is on the invocation stack right now. Re-entering it is a
    /// cycle — only reachable via `cook.require_recipe`, since the seed loop
    /// visits at the top level where nothing is in flight.
    Visiting,
    /// Its body ran to completion.
    ///
    /// `forced` is NOT "was this body reached via `cook.require_recipe`" — it
    /// is "has forcing been pushed DOWN into this recipe's static `requires`",
    /// which is the only question a later visit needs answered. The two
    /// coincide because `visit_requires_then_body` walks the deps under the
    /// same `forced` it runs the body under.
    ///
    /// The flag lives ON the state rather than in a parallel `forced_visited`
    /// set on purpose. This enum has now been the site of three bugs in one
    /// family — a force bypassing an ordering the seed loop was silently
    /// providing — and every one of them was a state that answered fewer
    /// questions than the code asked of it. A side set would be a second
    /// source of truth about the same node, kept in sync by hand, and nothing
    /// would oblige a future arm to consult it; folding it in means the
    /// compiler makes every reader of `Visited` say out loud what it does
    /// about forcing.
    ///
    /// `Skipped` carries no such flag because it cannot need one: a skip arm
    /// only declines when `!forced` (the parametric-chore arms stand down when
    /// forced; the `for_each` arm raises), so `Skipped` always implies
    /// un-forced, and a forced visit to it re-invokes unconditionally.
    Visited { forced: bool },
    /// Its body was deliberately NOT run: a skip arm in `invoke_body` decided
    /// the recipe isn't being built and registered it with no units.
    ///
    /// Distinct from `Visited` because every skip arm is gated on the STATIC
    /// reachability pre-pass, which by construction cannot see an edge a body
    /// declares while running. Collapsing the two states loses exactly the
    /// information a later force needs: the seed loop walks a LEXICOGRAPHIC
    /// order, so a required recipe whose name sorts before its requirer's is
    /// reached — and skipped — before the force that would have rescued it,
    /// and a force that treats the skip as a completed visit returns `Ok(())`
    /// having evaluated nothing. The recipe then registers zero units while
    /// the edge still places it in the build closure: expressly non-conforming
    /// per §22.8, and silent. A force on this state MUST re-invoke with
    /// `forced = true` (which is what tells the skip arms to stand down) or
    /// raise the arm's designed error.
    Skipped,
    /// Its body ran and RAISED. The stored error is the original diagnostic,
    /// re-raised verbatim on any later visit.
    ///
    /// Needed because a body may swallow a force error with `pcall`: without
    /// a terminal record the entry would either be left `Visiting` — which the
    /// seed loop's later visit reads as a cycle, fabricating a diagnostic and
    /// destroying the real one — or be cleared, re-running a body §22.8 says
    /// is evaluated at most once per pass. Replaying satisfies both: the pass
    /// still fails, with the failure the author needs to see.
    ///
    /// Holds the `mlua::Error` rather than a rendered string so the replay is
    /// byte-identical to the original — re-wrapping a `RegisterError`'s
    /// `to_string()` would stutter (`lua error: runtime error: lua error:
    /// runtime error: …`). A body error is always `RegisterError::Lua`, so the
    /// common path round-trips exactly; the rarer structured variants flatten
    /// to one `runtime` error, which is precisely what `force` already does to
    /// every `RegisterError` on its way back into Lua.
    Failed(mlua::Error),
}

/// Step 12's body-invocation driver (Standard §22.8, CS-0144).
///
/// Owns everything the loop used to close over, plus the visit state that
/// makes it re-entrant: `cook.require_recipe` forces a body *from inside
/// another body*, so what was a flat loop is now a DFS whose edges are
/// discovered as the bodies run.
///
/// Everything mutable lives behind a `RefCell` because the Lua closure
/// backing `cook.require_recipe` holds an `Rc<BodyDriver>` and re-enters
/// `ensure_invoked` mid-`func.call()`. No borrow of any of these cells — nor
/// of `recipes` — may be held across a `func.call()` for the same reason.
struct BodyDriver {
    builder: Rc<RegisterSessionBuilder>,
    recipes: Rc<RefCell<Vec<crate::capture::RegisteredRecipe>>>,
    probe_registry: Rc<RefCell<ProbeRegistry>>,
    session_state: SharedSessionCaptureState,
    body_slot: SharedBodySlot,
    /// Names reachable from `target_recipe` via the STATIC `requires` graph,
    /// computed before any body ran — so it structurally cannot know a
    /// dynamic edge. Every consumer below therefore also honours `forced`.
    reachable_from_target: std::collections::BTreeSet<String>,
    units_by_recipe: RefCell<BTreeMap<String, RecipeUnits>>,
    names: RefCell<Vec<crate::RegisteredRecipePub>>,
    visit: RefCell<BTreeMap<String, VisitState>>,
    /// Bare names of the bodies currently on the invocation stack, innermost
    /// last. Renders the cycle path.
    path: RefCell<Vec<String>>,
}

impl BodyDriver {
    /// The `cook.require_recipe` forcer: validate the name against the
    /// registered set, then force. Everything here is dynamic-call-specific
    /// diagnostics; the shared visit lives in `ensure_invoked`.
    fn force(&self, lua: &Lua, name: &str) -> Result<(), mlua::Error> {
        // The requiring recipe, for the diagnostics. Still the caller's body
        // at this point — `ensure_invoked` swaps the slot, and hasn't run yet.
        let requiring = self
            .body_slot
            .borrow()
            .as_ref()
            .and_then(|b| b.current_recipe_bare.clone())
            .unwrap_or_default();

        // Unknown name. Every `cook.recipe` registration completes before any
        // body runs, so absence is definitive at call time — no need to defer
        // to the engine's cross-cookfile analyzer.
        //
        // Membership is tested against the borrow directly; the name list is
        // materialised only to build the diagnostic. This is the API's hot
        // path — every call reaches it — and cloning every registered name
        // per call to answer a yes/no question is the kind of cost that
        // scales with the wrong thing (Cookfile size, not call count).
        let known = self.recipes.borrow().iter().any(|r| r.name == name);
        if !known {
            let registered: Vec<String> =
                self.recipes.borrow().iter().map(|r| r.name.clone()).collect();
            let closest = crate::env_api::closest_declared(name, &registered, 3);
            return Err(mlua::Error::runtime(format!(
                "cook.require_recipe: recipe \"{name}\" (required by \"{requiring}\") is not \
                 registered in this Cookfile. Closest registered names: {}. Check the spelling, \
                 or register \"{name}\" before it is required (Standard \u{00a7}22.8, CS-0144)",
                closest.join(", "),
            )));
        }

        // The failure crosses back into Lua as a message, so a structured
        // `RegisterError` variant raised beneath here degrades to
        // `RegisterError::Lua` by the time the seed loop re-raises it. The
        // diagnostic text — all the Standard's error contract and the CLI
        // render — survives intact, which is why no variant-preserving
        // side channel is worth its stale-state hazards.
        self.ensure_invoked(lua, name, true)
            .map_err(|e| mlua::Error::runtime(e.to_string()))
    }

    /// Evaluate `name`'s body to completion, unless it already ran this pass.
    ///
    /// `forced` distinguishes a `cook.require_recipe` call from the seed
    /// loop's own visit. It is not cosmetic: every body-skip arm below is
    /// gated (directly or not) on the STATIC reachability pre-pass, which
    /// cannot see a dynamic edge — so a forced recipe that hit a skip arm
    /// would register zero units while the engine, reading the very
    /// `requires` edge this call records, still pulls it into the build
    /// closure. Register and engine would then disagree about what got built.
    fn ensure_invoked(&self, lua: &Lua, name: &str, forced: bool) -> Result<(), RegisterError> {
        // Cloned into a `let` BEFORE the `match`, not matched on
        // `self.visit.borrow()…` directly: a temporary in a match scrutinee
        // lives until the end of the whole `match`, so the forced-propagation
        // arm below — which recurses — would re-enter `ensure_invoked` with
        // this borrow still live and panic `RefCell already borrowed`.
        let state = self.visit.borrow().get(name).cloned();
        match state {
            // Already fully propagated: the body ran AND its static deps were
            // walked forced. Nothing left for any later visit to contribute.
            Some(VisitState::Visited { forced: true }) => return Ok(()),
            Some(VisitState::Visited { forced: false }) if !forced => return Ok(()),
            // The body ran, but UN-forced — so its static deps were walked
            // un-forced too, and any of them that hit a skip arm is still
            // sitting at zero units. This force is new information: it puts
            // `name` in the build closure, and the engine builds that closure
            // from `requires`, so every recipe `name` requires is in it too.
            //
            // Reachable purely by NAME: `topo` seeds lexicographically, so a
            // forcer sorting AFTER `name` (`zzz` -> `bbb` -> `achore`) finds
            // `name` already `Visited`, while one sorting before it (`app`)
            // finds it `None` and propagates via the normal body path. Without
            // this arm the dep registers zero units while the edge still builds
            // it — expressly non-conforming per §22.8 — and, worse, the
            // `for_each` arm's DESIGNED hard error is silently swallowed.
            //
            // The body is NOT re-run: §22.8 says at most once per pass, and it
            // already ran to completion. Only `forced` is pushed down.
            Some(VisitState::Visited { forced: false }) => {
                // `name` pushed onto `self.path` around the walk, mirroring the
                // normal (`None`-state) body path a few lines down: `self.path`
                // is what the `Visiting` arm renders a cycle from, and without
                // this push a cycle reached back through THIS arm is missing
                // `name` off the stack. `name` itself is never marked
                // `Visiting` (see below), so `path` is the only record of it
                // being mid-walk — drop the push and a mixed static/dynamic
                // cycle reached this way collapses to a fabricated one-element
                // self-cycle on whichever dep the recursion re-enters,
                // regardless of it having nothing to do with `name`.
                self.path.borrow_mut().push(name.to_string());
                let result = self.ensure_static_requires(lua, name, true);
                self.path.borrow_mut().pop();
                result?;
                // Recorded only on success, matching every other terminal-state
                // write here: if the propagation raised (arm 3's designed error
                // is the live case), leaving `name` at `forced: false` means a
                // later force re-walks and re-raises rather than inheriting a
                // silence. `name`'s own body genuinely ran and succeeded, so
                // marking it `Failed` would be a lie that the seed loop would
                // then replay as a body error that never happened.
                //
                // Terminates despite not marking `name` `Visiting` first: this
                // walk follows STATIC edges, and step 11's
                // `local_topological_sort` has already rejected a static cycle,
                // so the only way back into `name` is a body forcing it — and
                // that body can only be running beneath some static dep `D` of
                // `name` that this very loop marked `Visiting`, which the
                // re-walk then reports as the cycle it is.
                self.visit
                    .borrow_mut()
                    .insert(name.to_string(), VisitState::Visited { forced: true });
                return Ok(());
            }
            // A completed body is a no-op for a repeat visit; a SKIPPED one is
            // not. Fall through to re-invoke — `forced` is now true, so the
            // arm that skipped it either stands down (the parametric-chore
            // arms) or raises its designed error (the `for_each` arm). Only
            // the seed loop can reach a skipped name with `forced = false`,
            // and it visits each name once, so the re-invoke is bounded.
            Some(VisitState::Skipped) if !forced => return Ok(()),
            Some(VisitState::Skipped) => {}
            // Re-raise the original failure rather than the body. Order
            // matters here: this arm sits ahead of the `Visiting` check
            // because a failed body is popped off `path` but is not, and must
            // not be read as, a cycle.
            Some(VisitState::Failed(err)) => return Err(RegisterError::Lua(err)),
            Some(VisitState::Visiting) => {
                // Only reachable from a dynamic call, so the diagnostic can
                // name the API unconditionally. Mirrors
                // `local_topological_sort`'s cycle rendering: the path from
                // the recurring node, with that node repeated at the end.
                let path = self.path.borrow();
                let start = path.iter().position(|n| n == name).unwrap_or(0);
                let mut cycle: Vec<String> = path[start..].to_vec();
                cycle.push(name.to_string());
                return Err(RegisterError::Lua(mlua::Error::runtime(format!(
                    "cook.require_recipe: dependency cycle: {}. Forcing is synchronous, so this \
                     would recurse without bound; break the cycle by removing one of the \
                     `cook.require_recipe` calls on that path (Standard \u{00a7}22.8, CS-0144)",
                    cycle.join(" -> "),
                ))));
            }
            None => {}
        }

        self.visit.borrow_mut().insert(name.to_string(), VisitState::Visiting);
        self.path.borrow_mut().push(name.to_string());

        // Marked `Visiting` and pushed on `path` BEFORE the deps recursion, so
        // a cycle through the static edges renders the same path as one through
        // the dynamic ones.
        let result = self.visit_requires_then_body(lua, name, forced);

        self.path.borrow_mut().pop();

        // Every exit records a terminal state, symmetric with the `path.pop()`
        // above. Leaving `Visiting` behind on the error path is what let a
        // pcall-swallowed force fabricate a cycle out of the abandoned mark.
        let outcome = match result {
            // `forced` verbatim: `visit_requires_then_body` walked this
            // recipe's static deps under exactly this flag, so it is precisely
            // the "has forcing been pushed down" the state records.
            Ok(Outcome::Ran) => VisitState::Visited { forced },
            Ok(Outcome::Skipped) => VisitState::Skipped,
            Err(e) => {
                // Preserve the `mlua::Error` as-is where there is one; flatten
                // the structured variants the same way `force` does.
                let stored = match &e {
                    RegisterError::Lua(le) => le.clone(),
                    other => mlua::Error::runtime(other.to_string()),
                };
                self.visit
                    .borrow_mut()
                    .insert(name.to_string(), VisitState::Failed(stored));
                return Err(e);
            }
        };
        self.visit.borrow_mut().insert(name.to_string(), outcome);
        Ok(())
    }

    /// `name`'s own static `requires` first, then `name`'s body.
    ///
    /// Split out of `ensure_invoked` so that BOTH failure sources — a dep that
    /// raised and a body that raised — land on the single terminal-state record
    /// there. A dep failure must mark `name` `Failed` too: `name`'s body never
    /// ran, so leaving `Visiting` behind would let the seed loop's later visit
    /// read the abandoned mark as a cycle and fabricate a diagnostic over the
    /// real one — the same hazard the body-error path already guards.
    fn visit_requires_then_body(
        &self,
        lua: &Lua,
        name: &str,
        forced: bool,
    ) -> Result<Outcome, RegisterError> {
        self.ensure_static_requires(lua, name, forced)?;

        // Both re-entrancy hazards, saved across the nested invocation:
        //   - the body slot is a single shared `Option<BodyCaptureState>`, so
        //     without this the callee's units land in the caller's recipe;
        //   - `setup_recipe_context` rebinds the Lua `recipe` global, so
        //     without this the caller sees the callee's `recipe.name` after
        //     the call returns.
        // Both are no-ops at the top level (the slot is already `None` and
        // the next seed iteration rebinds `recipe` regardless), so the
        // no-`require_recipe` path is unchanged. The deps recursion above needs
        // no save/restore of its own — each nested `ensure_invoked` performs
        // this same save/restore around its own body.
        let saved_body = self.body_slot.borrow_mut().take();
        let saved_recipe_global: LuaValue = lua.globals().get("recipe")?;

        let result = self.invoke_body(lua, name, forced);

        *self.body_slot.borrow_mut() = saved_body;
        lua.globals().set("recipe", saved_recipe_global)?;
        result
    }

    /// Evaluate every LOCAL static `requires` of `name` before `name`'s body.
    ///
    /// Without this the visit is not a DFS: it recurses only into the edges a
    /// body declares via `cook.require_recipe`, and leans on the seed loop's
    /// `topo` order to have already evaluated the static ones. A force bypasses
    /// the seed loop, so when the requirer sorts before the forced recipe's own
    /// static dep, the forced body ran before that dep — and its
    /// `cook.import(dep)` returned nil. The mislink was PARTIAL (the force
    /// itself worked), hence silent, which is the failure class §22.8 exists to
    /// kill; it also contradicted §22.8's own "registration order within a pass
    /// is dependency-driven".
    ///
    /// Recursing here makes `topo` non-load-bearing for correctness — it now
    /// only pins a deterministic order among INDEPENDENT recipes — and closes
    /// the merged-graph cycle hole as a side effect: with every static edge
    /// walked, the `Visiting` check catches a mixed static/dynamic cycle at the
    /// moment it is traversed (step 12a is retained as a belt-and-braces check;
    /// see the note there).
    ///
    /// `forced` propagates: the engine's analyzer builds the build closure from
    /// `requires`, so if `name` is in the closure then so is every recipe it
    /// requires. A dep visited un-forced could hit a skip arm and register zero
    /// units while the edge still had it built — the same register/engine
    /// disagreement a direct force already refuses to allow.
    ///
    /// Refs absent from the local set are skipped, exactly as
    /// `local_topological_sort` does: those are cross-Cookfile `requires`, and
    /// the engine's cross-cookfile dep analyzer owns resolving them.
    fn ensure_static_requires(
        &self,
        lua: &Lua,
        name: &str,
        forced: bool,
    ) -> Result<(), RegisterError> {
        // Cloned out, and the borrow dropped, before any recursion: the callee
        // may register probes or re-enter through `cook.require_recipe`, and a
        // live `recipes` borrow across that is a `BorrowMutError`.
        let deps: Vec<String> = {
            let registry = self.recipes.borrow();
            match registry.iter().find(|r| r.name == name) {
                Some(recipe) => recipe.metadata.requires.clone(),
                // `invoke_body` raises the real `RecipeNotFound` a few lines
                // on; don't pre-empt its diagnostic from here.
                None => return Ok(()),
            }
        };
        for dep in deps {
            let local = self.recipes.borrow().iter().any(|r| r.name == dep);
            if local {
                self.ensure_invoked(lua, &dep, forced)?;
            }
        }
        Ok(())
    }

    /// Invoke one recipe body and drain its captures. Step 12's loop body,
    /// verbatim but for the `forced` handling and the drain-time merge.
    ///
    /// The `Outcome` is load-bearing: an `Ok` from a skip arm and an `Ok` from
    /// a body that actually ran mean opposite things to a later force, and
    /// returning the bare `Ok(())` for both is what silently swallowed the
    /// forcing (§22.8).
    fn invoke_body(&self, lua: &Lua, name: &str, forced: bool) -> Result<Outcome, RegisterError> {
        let builder = &self.builder;

        // Open fresh body slot.
        *self.body_slot.borrow_mut() = Some(BodyCaptureState::new());

        // Look up the recipe entry. Borrow scope kept tight so we can mutate
        // body_slot below without overlapping the recipes borrow — and so no
        // borrow is live across the `func.call()`, which can re-enter.
        let (
            func_key_clone,
            static_requires,
            source,
            kind,
            qualified_name,
            params_meta,
            source_line,
            skip_for_each_body,
            origin,
        ): (
            LuaRegistryKey,
            Vec<String>,
            crate::capture::RegistrationSource,
            crate::RecipeKind,
            String,
            Vec<crate::capture::ChoreParamMeta>,
            usize,
            bool,
            Option<String>,
        );
        {
            let registry = self.recipes.borrow();
            let recipe = registry
                .iter()
                .find(|r| r.name == name)
                .ok_or_else(|| RegisterError::RecipeNotFound(name.to_string()))?;

            // COOK-64 §22.5.9 demand-driven rule: a probe-sourced `for_each`
            // recipe that is NOT reachable from the build target had its probe
            // skipped by the pre-pass, so its body's `cook.probes.get` would
            // error. Skip the body — the recipe is not being built — registering
            // it with no units, mirroring the parametric-sibling skip below.
            skip_for_each_body = builder.target_recipe.is_some()
                && !self.reachable_from_target.contains(name)
                && matches!(
                    recipe.for_each,
                    Some(crate::capture::ForEachDescriptor::Probe { .. })
                );

            // Run recipe context setup (ingredient resolution).
            setup_recipe_context(lua, recipe, &builder.working_dir, &builder.workspace_root, &builder.ingredient_warnings)?;

            // The `LuaRegistryKey` doesn't impl Clone, so we materialize the
            // function now and stash it for the call below; the registry
            // entry itself stays untouched.
            let func: LuaFunction = lua.registry_value(&recipe.function)?;
            // Re-stash so we can drop the `registry` borrow before calling.
            func_key_clone = lua.create_registry_value(func)?;
            static_requires = recipe.metadata.requires.clone();
            source = recipe.source;
            kind = recipe.kind;
            params_meta = recipe.metadata.params.clone();
            origin = recipe.metadata.origin.clone();
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

        // Skip arm 3 — a probe-sourced `for_each` recipe not statically
        // reachable from the target.
        if skip_for_each_body {
            if forced {
                // Unlike the parametric-chore arms, forcing cannot rescue this
                // one: the body needs a probe value the pre-pass never
                // computed. Evaluating the probe lazily here IS feasible —
                // `run_for_each_prepass` is a free function, and calling it
                // with `reachable_from_target = {name}` and `has_target = true`
                // would resolve exactly this driver's probe. It is declined on
                // re-entrancy risk: that path holds `recipes` and
                // `probe_registry` borrowed across `run_prepass_produce`'s Lua
                // call, and its signature forces every caller into that shape,
                // so a forced body that touched `cook.probe` or `cook.recipe`
                // would hit a `BorrowMutError`. Erroring keeps register and
                // engine agreeing about what got built; the static dep in the
                // hint puts the recipe in `local_reachable_set`, so the
                // pre-pass evaluates its probe and the force then succeeds.
                lua.remove_registry_value(func_key_clone)?;
                return Err(RegisterError::Lua(mlua::Error::runtime(format!(
                    "cook.require_recipe: recipe \"{name}\" is a probe-sourced `for_each` recipe \
                     that is not reachable from the build target, so its feeding probe was not \
                     evaluated by the register pre-pass and its body cannot run. Add a static \
                     `: {name}` dep to the requiring recipe's header so the pre-pass sees it \
                     (Standard \u{00a7}22.8, CS-0144)"
                ))));
            }
            lua.remove_registry_value(func_key_clone)?;
            let _ = self.body_slot.borrow_mut().take();
            self.register_skipped(name, source, kind, static_requires, params_meta, origin);
            return Ok(Outcome::Skipped);
        }

        // Stamp current_recipe on the body so cook.add_test defaults the
        // suite field correctly (CS-0061 §3.2). current_recipe_bare is
        // stamped alongside it, from the same (bare) `name`, for
        // cook.require_recipe's self-reference check and the cycle-path
        // rendering (Standard §22.8, CS-0144) — see
        // BodyCaptureState::current_recipe_bare.
        {
            let mut slot = self.body_slot.borrow_mut();
            let body = slot
                .as_mut()
                .expect("body slot just opened above");
            body.current_recipe = Some(qualified_name.clone());
            body.current_recipe_bare = Some(name.to_string());
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
            .map(|t| t == name)
            .unwrap_or(false);
        if kind == crate::RecipeKind::Chore {
            if is_target {
                // Targeted chore: bind argv and call with __cook_params.
                let argv = &builder.target_argv;
                let (bound, prelude) = build_chore_params_table(
                    lua,
                    &params_meta,
                    argv,
                    name,
                    source_line,
                )?;
                // Store the prelude on the body slot so cook.add_unit can
                // prepend it to lua_code units captured in this chore body.
                self.set_chore_prelude(prelude);
                func.call::<()>((bound,))
                    .map_err(RegisterError::Lua)?;
            } else if params_meta.is_empty() {
                // Paramless chore: cheap to invoke, captures units for dep
                // linkage when reachable and for enumeration tools when not.
                // Either way the body is safe to call with no argument — no
                // `__cook_params` references. Covers both the targeted-but-
                // not-this-one and the no-target cases.
                func.call::<()>(()).map_err(RegisterError::Lua)?;
            } else if forced || self.reachable_from_target.contains(name) {
                // Skip arms 1 and 2, rescued. §7.5.1: a parametric chore that
                // is a dep of the target runs with no argv supplied
                // (required-no-default surfaces a legitimate register-time
                // error here); an unrelated parametric sibling gets skipped by
                // the arm below, which is also what the no-target path used to
                // do unconditionally — `reachable_from_target` is empty when
                // `target_recipe` is `None` (COOK-61, a52063d).
                //
                // A FORCED chore is a dep of the requiring recipe, hence
                // reachable by definition, so it takes this same path on both
                // the target and the no-target branch. Without the `forced`
                // disjunct, `cook.require_recipe` on a parametric chore would
                // silently register zero units — and the no-target branch is
                // the one `cook list`, `cook dag`, and most tests take.
                let (bound, prelude) = build_chore_params_table(
                    lua,
                    &params_meta,
                    &[],
                    name,
                    source_line,
                )?;
                self.set_chore_prelude(prelude);
                func.call::<()>((bound,)).map_err(RegisterError::Lua)?;
            } else {
                // Parametric chore, neither targeted, reachable, nor forced —
                // skip body invocation: the body would raise a nil-index Lua
                // error on its first `local NAME = __cook_params.NAME` prelude
                // line. Record an empty units entry so downstream stages still
                // see the recipe in the registered set.
                lua.remove_registry_value(func_key_clone)?;
                let _ = self.body_slot.borrow_mut().take();
                self.register_skipped(name, source, kind, static_requires, params_meta, origin);
                return Ok(Outcome::Skipped);
            }
        } else {
            // Normal recipe: validate that no argv was supplied (§7.1.2).
            if is_target && !builder.target_argv.is_empty() {
                return Err(RegisterError::RecipeWithArgv {
                    name: name.to_string(),
                    supplied: builder.target_argv.len(),
                });
            }
            func.call::<()>(()).map_err(RegisterError::Lua)?;
        }
        // Cleanup the transient registry entry to avoid leaking refs across
        // many recipes in large Cookfiles.
        lua.remove_registry_value(func_key_clone)?;

        // Drain the body slot back to None.
        let mut body = self
            .body_slot
            .borrow_mut()
            .take()
            .expect("body slot populated above");

        // The edge (Standard §22.8, CS-0144). Merged here, at drain, rather
        // than read off the pre-body `static_requires` clone: the body only
        // just accumulated `dynamic_requires`, so that clone is stale. Routing
        // through `requires` is the whole design — it is the single source of
        // truth the engine's analyzer builds the closure, validates unknown
        // names, and detects cycles from, so the edge becomes indistinguishable
        // from a `recipe A : B` dep-list entry. Dedup spans static + dynamic:
        // a recipe carrying both must yield ONE entry.
        let mut requires = static_requires;
        for dep in &body.dynamic_requires {
            if !requires.contains(dep) {
                requires.push(dep.clone());
            }
        }

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
                meta.recipe_name = name.to_string();
            }
        }

        // Resolve probe references — same end-of-body check that
        // register_recipe runs, scoped to this recipe's units.
        {
            use std::collections::BTreeSet;
            let probe_reg = self.probe_registry.borrow();
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

        // COOK-96: build the per-member output map for $<recipe[in]> joins
        // (COOK-221/CS-0137). Mirror terminal-output keying (qualified_name);
        // last-wins per member across step groups matches
        // last_cook_step_outputs' last-wins.
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
        let probes = self.session_state.borrow().probes.clone();

        let units = RecipeUnits {
            recipe_name: name.to_string(),
            deps: requires.clone(),
            units: body.units,
            step_groups: body.step_groups,
            working_dir: builder.working_dir.clone(),
            env_vars: env_btree,
            terminal_outputs: terminal_outputs_list,
            dep_edges: body.dep_edges,
            probes,
        };
        self.units_by_recipe.borrow_mut().insert(name.to_string(), units);

        // `units_by_recipe` is a map (last write wins), but `names` is a
        // Vec: a recipe re-invoked after a skip would otherwise appear twice
        // in the discovered set (§22.6). Replace the placeholder entry
        // `register_skipped` left rather than appending — the real one carries
        // the merged `requires` and belongs at the skipped entry's position,
        // which is where the seed order put it.
        let entry = crate::RegisteredRecipePub {
            name: name.to_string(),
            source,
            kind,
            requires,
            params: params_meta,
            origin,
        };
        {
            let mut names = self.names.borrow_mut();
            match names.iter().position(|r| r.name == name) {
                Some(idx) => names[idx] = entry,
                None => names.push(entry),
            }
        }
        Ok(Outcome::Ran)
    }

    /// Record a recipe whose body was skipped: no units, `requires` static
    /// only (a body that never ran declared no dynamic edges). Shared by the
    /// two surviving skip arms.
    ///
    /// The entry is a PLACEHOLDER when the skip is later rescued by a force:
    /// `invoke_body`'s drain overwrites it in place on the re-invocation.
    fn register_skipped(
        &self,
        name: &str,
        source: crate::capture::RegistrationSource,
        kind: crate::RecipeKind,
        requires: Vec<String>,
        params: Vec<crate::capture::ChoreParamMeta>,
        origin: Option<String>,
    ) {
        let entry = crate::RegisteredRecipePub {
            name: name.to_string(),
            source,
            kind,
            requires: requires.clone(),
            params,
            origin,
        };
        {
            let mut names = self.names.borrow_mut();
            match names.iter().position(|r| r.name == name) {
                Some(idx) => names[idx] = entry,
                None => names.push(entry),
            }
        }
        self.units_by_recipe.borrow_mut().insert(
            name.to_string(),
            RecipeUnits {
                recipe_name: name.to_string(),
                deps: requires,
                units: vec![],
                step_groups: vec![],
                working_dir: self.builder.working_dir.clone(),
                env_vars: self
                    .builder
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
    }

    /// Stash a chore's bound-parameter prelude on the open body slot so
    /// `cook.add_unit` can prepend it to `lua_code` units.
    fn set_chore_prelude(&self, prelude: String) {
        let mut slot = self.body_slot.borrow_mut();
        if let Some(body) = slot.as_mut() {
            body.chore_param_prelude = prelude;
        }
    }
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
/// `local _items = cook.probes.get("<ref>")`, `<ref>` being the verbatim
/// `ingredients <ref>` source ref carried by codegen. That value does not
/// exist until the feeding probe runs, and probes normally run as DAG nodes
/// in the execute phase — far too late for register-time fan-out. So we
/// evaluate every `for_each`-feeding probe (and its transitive probe
/// `requires`) here, synchronously on the register VM, before any recipe
/// body runs.
///
/// COOK-190 / §22.5.10: a ref is resolved against the probe registry via
/// `resolve_probe_ref` — an exact whole-ref key match wins, else the ref is
/// a `key:field` selector. The evaluation mirrors the execute-phase probe
/// path (`executor.rs` G4/G5): resolve declared inputs → fingerprint →
/// cache GET → on a miss run `produce` on the VM → cache PUT. The resolved
/// value is stashed in `prepass_store` keyed by probe key; for a
/// field-selector ref, the selected array is additionally stashed under the
/// verbatim ref (see below), which is what the `cook.probes.get` binding in
/// the generated body actually reads. When no `CacheContext` is wired
/// (tests / `list_names`), `produce` runs uncached.
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

    // (recipe, verbatim source ref) per probe-sourced driver.
    //
    // §22.5.9 demand-driven rule: when a build target is set, only evaluate
    // probes for recipes reachable from it. When no target is set every recipe
    // is being built, so every probe-sourced driver is in scope. The body loop
    // applies the mirror rule (`should_skip_for_each_body`) so a non-reachable
    // driver's body — which would call `cook.probes.get` on an unevaluated probe
    // — is skipped rather than erroring.
    let driver_reachable = |name: &str| !has_target || reachable_from_target.contains(name);
    let drivers: Vec<(&str, &str)> = recipes
        .iter()
        .filter(|r| driver_reachable(&r.name))
        .filter_map(|r| match &r.for_each {
            Some(ForEachDescriptor::Probe { source_ref }) => {
                Some((r.name.as_str(), source_ref.as_str()))
            }
            _ => None,
        })
        .collect();
    if drivers.is_empty() {
        return Ok(());
    }

    // COOK-190: resolve each ref against the registry (exact key match wins,
    // else trailing `:field` selector). A ref that names no declared probe
    // under either interpretation is rejected, naming the full ref.
    let mut resolved: Vec<(&str, &str, Option<&str>)> = Vec::new();
    for (recipe, source_ref) in &drivers {
        match resolve_probe_ref(source_ref, probe_registry) {
            Some((key, field)) => resolved.push((source_ref, key, field)),
            None => {
                return Err(RegisterError::ForEachProbeUndeclared {
                    recipe: (*recipe).to_string(),
                    key: (*source_ref).to_string(),
                })
            }
        }
    }

    let env_lookup = |name: &str| env_vars.borrow().get(name).cloned();
    let mut upstream_fps: BTreeMap<String, [u8; 32]> = BTreeMap::new();
    let mut done: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();

    // Evaluate each driver probe (and its transitive `requires`) in
    // dependency order. Probe cycles are already rejected (step 9 above), so
    // the recursion terminates; `in_progress` is a defensive belt-and-braces.
    for (_, key, _) in &resolved {
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
    for (source_ref, key, field) in &resolved {
        let store = prepass_store.borrow();
        let value = store.get(*key).expect("driver probe evaluated above");
        let (resolved_value, selector): (&serde_json::Value, String) = match field {
            Some(f) => match json_map_get(value, f) {
                Some(v) => (v, (*source_ref).to_string()),
                None => {
                    return Err(RegisterError::ForEachNotArray {
                        selector: (*source_ref).to_string(),
                        shape: "nil (no such field)".to_string(),
                    })
                }
            },
            None => (value, (*source_ref).to_string()),
        };
        if !matches!(resolved_value, serde_json::Value::Array(_)) {
            return Err(RegisterError::ForEachNotArray {
                selector,
                shape: json_shape(resolved_value).to_string(),
            });
        }
    }

    // COOK-190: the body reads `cook.probes.get("<verbatim ref>")`. For a
    // `key:field` selector, stash the selected array under the verbatim ref
    // (validated array-shaped by the diagnostic loop above).
    for (source_ref, key, field) in &resolved {
        let Some(f) = field else { continue };
        let items = {
            let store = prepass_store.borrow();
            let value = store.get(*key).expect("driver probe evaluated above");
            json_map_get(value, f).expect("validated above").clone()
        };
        prepass_store
            .borrow_mut()
            .insert((*source_ref).to_string(), items);
    }

    Ok(())
}

/// COOK-190 / §22.5.10: resolve an `ingredients <probe>` source ref against
/// the probe registry. Probe keys are canonically two-segment (`ns:name`),
/// so a `:` in the ref is ambiguous between a two-segment key and a
/// `key:field` selector. A declared probe whose key equals the entire ref
/// wins; otherwise the segment after the final `:` is a field selector on
/// the remaining (declared) key. `None` when neither interpretation names a
/// declared probe.
fn resolve_probe_ref<'a>(
    source_ref: &'a str,
    probe_registry: &ProbeRegistry,
) -> Option<(&'a str, Option<&'a str>)> {
    if probe_registry.probes.contains_key(source_ref) {
        return Some((source_ref, None));
    }
    let (key, field) = source_ref.rsplit_once(':')?;
    probe_registry
        .probes
        .contains_key(key)
        .then_some((key, Some(field)))
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
        let Some(ForEachDescriptor::Probe { source_ref }) = &recipe.for_each else {
            continue;
        };
        let Some((key, _)) = resolve_probe_ref(source_ref, probe_registry) else {
            continue; // unresolvable ref already rejected by the pre-pass
        };
        let Some(reg) = probe_registry.probes.get(key) else {
            // unreachable in practice: resolve_probe_ref just proved the key is declared
            continue;
        };
        for file in &reg.probe.inputs.files {
            if outputs.contains(&normalise_rel(file)) {
                return Err(RegisterError::ForEachProbeArtifactDep {
                    key: key.to_string(),
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
    // empty — `cook.probes.get` falls through to its module-context behaviour.
    let _module_state = install_remaining_apis(
        &lua,
        &builder,
        body_slot.clone(),
        None,
        Rc::new(RefCell::new(BTreeMap::new())),
        // Forcer cell left empty for good: `list_names` invokes no recipe
        // body, so every `cook.require_recipe` call it can reach is outside
        // one and stops at the guard rail before the cell is consulted.
        Rc::new(RefCell::new(None)),
        // Finalizer queue created fresh and never drained: per §22.9,
        // "Discovery surfaces MAY skip the queue" — `list_names` invokes no
        // recipe body, and a callback can't register a recipe (that call is
        // rejected wherever it IS drained), so the discovered set this
        // returns cannot depend on having run one. The call itself must
        // still be accepted as ordinary register-phase Lua, hence installing
        // the API rather than leaving `cook.on_register_complete` undefined.
        Rc::new(RefCell::new(Vec::new())),
    )?;

    // Load top-level Lua. Recipe registration happens via `cook.recipe(...)`
    // calls captured into `recipes`. Bodies are stashed as `LuaRegistryKey`
    // values but never invoked here.
    let chunk_name = format!("@{}", cookfile_label);
    lua.load(lua_source).set_name(chunk_name).exec()?;

    // Run config blocks so per-config gating (e.g. recipe registration
    // inside a `config "release"` block) is reflected in the listed set.
    // Listing does not surface config provenance, so the host-reads sink is
    // a throwaway.
    let host_reads: crate::config_sandbox::SharedHostReads =
        Rc::new(RefCell::new(Vec::new()));
    let _final_env =
        dispatch_config_blocks(&lua, &builder, &recipes.borrow(), &host_reads)?;

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
            params: r.metadata.params.clone(),
            origin: r.metadata.origin.clone(),
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
    recipe_forcer: crate::context::SharedRecipeForcer,
    finalizer_queue: crate::on_register_api::SharedFinalizerQueue,
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
    crate::context::register_recipe_name_api(lua, body_slot.clone())?;
    // Installed ONCE, here, over a forcer CELL the caller fills later —
    // `register_cookfile` once its driver exists, never on `list_names`
    // (which invokes no body, so every call it can reach is outside a recipe
    // body and stops at the guard rail). Installing once is the point: this
    // is the only closure any caller can ever capture, so an alias taken
    // during the top-level chunk sees the driver appear exactly as a fresh
    // `cook.require_recipe` lookup does (Standard §22.8, CS-0144).
    crate::context::register_require_recipe_api(lua, body_slot.clone(), recipe_forcer.clone())?;
    // Standard §22.9, CS-0149: installs `cook.on_register_complete`, which
    // only queues onto `finalizer_queue` — `register_cookfile`'s step 12c
    // drains it after every recipe body has run; `list_names` never drains
    // it at all (see the comment at its call site).
    crate::on_register_api::register_on_register_complete(lua, finalizer_queue)?;
    crate::dep_output_api::register_dep_output_api(
        lua,
        builder.terminal_outputs.clone(),
        body_slot.clone(),
        builder.alias_dirs.clone(),
        builder.qualified_prefix.clone(),
        builder.alias_qualified_prefixes.clone(),
    )?;
    // COOK-297 (revised): must follow register_dep_output_api — it wraps the
    // `cook.dep_order` that call installs, giving it require_recipe's
    // register-order guarantee so a maker can drop require_recipe entirely.
    crate::context::register_import_forcing(
        lua,
        body_slot.clone(),
        recipe_forcer.clone(),
        builder.export_store.clone(),
    )?;
    crate::context::register_dep_order_forcing(lua, body_slot.clone(), recipe_forcer)?;
    crate::dep_output_api::register_member_output_api(
        lua,
        builder.member_outputs.clone(),
        body_slot.clone(),
        builder.qualified_prefix.clone(),
        builder.alias_qualified_prefixes.clone(),
    )?;
    crate::context::register_resolve_ingredients(lua, &builder.working_dir, &builder.workspace_root)?;
    // CS-0101: cook.file_ref — register-phase resolution of `$<file:PATH>`
    // placeholders (hoisted locals emitted by cook-luagen).
    crate::file_ref::register_file_ref(lua, &builder.working_dir)?;
    // cook.json_decode / cook.yaml_decode are both-phase (§24.8, CS-0123);
    // the shared implementation lives in cook-lua-stdlib so the worker VMs
    // in cook-luaotp install byte-identical behaviour.
    let cook_tbl: LuaTable = lua.globals().get("cook")?;
    cook_lua_stdlib::register_codec_api(lua, &cook_tbl)?;
    // CS-0158: cook.tools.id — canonical tool identity, both-phase (a probe
    // produce body must behave identically on the register pre-pass and the
    // execute-phase demand path).
    cook_lua_stdlib::register_tools_api(lua, &cook_tbl)?;
    Ok(module_state)
}

/// Dispatch any `__cook_run_config_blocks` function emitted by codegen
/// and return the final post-dispatch env snapshot.
///
/// When codegen has emitted a config-block dispatcher, this:
///
/// 1. Sandboxes the dispatcher's `_ENV` (Standard §5.3.2, CS-0163): the config
///    body sees `host.*` (its only external-input surface), the `env` output
///    sink (an alias of `cook.env`), and a pure-Lua subset — never `os`/`io`/
///    clocks/randomness. `host.*` reads are recorded into `host_reads`.
/// 2. Calls the dispatcher with the builder's `selected_config`.
/// 3. Freezes the env keyset against the post-dispatch table.
/// 4. Re-applies any `--set KEY=VALUE` CLI overrides on top.
/// 5. Snapshots the env back into the builder's shared `env_vars` map.
/// 6. Emits one §5.2.3 shadowing warning per recipe-name / declared-env
///    collision (deduped via `builder.shadow_warnings_emitted`).
/// 7. The `env` sink lives inside the sandbox `_ENV`, not on the real globals,
///    so recipe bodies access env only through `cook.env` with nothing to
///    remove afterward.
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
    host_reads: &crate::config_sandbox::SharedHostReads,
) -> Result<BTreeMap<String, String>, RegisterError> {
    if let Ok(dispatch) = lua.globals().get::<LuaFunction>("__cook_run_config_blocks") {
        let cook_tbl: LuaTable = lua.globals().get("cook")?;
        let env_tbl: LuaTable = cook_tbl.get("env")?;

        // Sandbox the config function (Standard §5.3.2, CS-0163). `_ENV` is
        // swapped for the restricted table so `os`/`io`/clock/randomness/etc.
        // are unreachable and the only external-input surface is `host.*`; the
        // `env` output sink is exposed inside the sandbox rather than as a real
        // global, so it never leaks to recipe bodies. `set_environment` returns
        // false only when the function references no globals at all (an empty /
        // pure-literal config body) — nothing to sandbox in that case.
        let sandbox = crate::config_sandbox::build_config_sandbox_env(
            lua,
            &env_tbl,
            &builder.working_dir,
            host_reads,
        )?;
        dispatch.set_environment(sandbox)?;

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

        // `env_vars` was just refreshed above so it's the canonical source;
        // copy from there. (The `env` sink lives inside the config sandbox
        // `_ENV`, not on the real globals, so there is no global to remove —
        // recipe bodies never see it.)
        let final_env: BTreeMap<String, String> = builder
            .env_vars
            .borrow()
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();

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
