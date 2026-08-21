use mlua::prelude::*;
use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::{Arc, Mutex};

use cook_contracts::lua_string;
use cook_contracts::probe_key::LocalProbeKey;
use cook_contracts::RecipeUnits;

use crate::capture::install_cook_api;
use crate::context::setup_recipe_context;
use crate::dep_output_api::{SharedMemberOutputs, SharedTerminalOutputs};
use crate::export_api::SharedExportStore;
use crate::module_loader::{ModuleLoaderState, SharedModuleLoaderState};
use crate::probe_api::{install_cook_probe, ProbeRegistry};
use crate::var_api::{install_var_api, VarKeyset};
use crate::{
    BodyCaptureState, RegisterError, RegistrationSite, RegistrationSiteKind,
    SessionCaptureState, SharedBodySlot, SharedSessionCaptureState,
};

pub struct RegisterSessionBuilder {
    working_dir: PathBuf,
    workspace_root: PathBuf,
    gather_warnings: Rc<RefCell<Vec<String>>>,
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
    env_keyset: VarKeyset,
    /// (recipe, env-var) shadowing pairs we've already warned about, so
    /// we don't repeat the diagnostic on every recipe-register call within
    /// a single Cookfile load.
    shadow_warnings_emitted: Rc<RefCell<std::collections::BTreeSet<(String, String)>>>,
    /// The targeted recipe / chore name (unqualified within this Cookfile).
    /// When set, the body invocation for this recipe will receive `argv` as
    /// bound chore-parameter values (COOK-36 Task 4).
    ///
    /// `None` for non-dispatch paths (e.g. `cook list`).
    pub(crate) target_recipe: Option<String>,
    /// Positional argv to bind as chore parameters for `target_recipe`.
    /// Empty for normal recipes (which don't accept parameters).
    pub(crate) target_argv: Vec<String>,
    /// LOCAL names this Cookfile registers that the CALLER has determined are
    /// reachable from the dispatch target, seeded into `reachable_from_target`
    /// alongside this pass's own local closure (Standard §7.6, CS-0218).
    ///
    /// A register pass sees one Cookfile, so its `requires` graph stops at the
    /// Cookfile boundary: `chore a: sub.b` is an edge the ROOT knows about and
    /// `sub`'s own pass cannot see, and `sub`'s pass is the one that decides
    /// whether `b`'s body runs. Left to itself it would decide "not reachable"
    /// and register `b` with zero units while `b` sits in the build closure —
    /// the register/execute disagreement §22.8 declares non-conforming.
    ///
    /// So the workspace layer, which holds the whole graph, supplies the
    /// answer. Empty for a single-Cookfile pass, where the local closure is
    /// already the whole truth, and for tests.
    pub(crate) reachable_names: std::collections::BTreeSet<String>,
}

impl RegisterSessionBuilder {
    pub fn new(working_dir: PathBuf, env_vars: HashMap<String, String>) -> Self {
        let workspace_root = working_dir.clone();
        Self {
            working_dir,
            workspace_root,
            gather_warnings: Rc::new(RefCell::new(Vec::new())),
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
            env_keyset: VarKeyset::new(),
            shadow_warnings_emitted: Rc::new(RefCell::new(std::collections::BTreeSet::new())),
            target_recipe: None,
            target_argv: Vec::new(),
            reachable_names: std::collections::BTreeSet::new(),
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

    /// Seed `reachable_from_target` with LOCAL names the caller has determined
    /// are reachable from the dispatch target across Cookfile boundaries.
    ///
    /// See the field's own documentation for why a per-Cookfile pass cannot
    /// answer this for itself. The names are seeds, not the whole answer: this
    /// pass still closes over its own `requires` graph from each of them, so a
    /// caller supplies the boundary-crossing entry points and nothing more.
    pub fn with_reachable_names(mut self, names: std::collections::BTreeSet<String>) -> Self {
        self.reachable_names = names;
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
    // One context for unit-registration identity AND the `inputs
    // <probe>` pre-pass backend. COOK-359 split them because wiring identity
    // would have put the checkout directory's name into recipe_namespace and
    // thence the cloud key; CS-0196 made key-side identity configured-or-
    // empty, so the split's reason is gone and the two collapse (COOK-364).
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

    // 4b. Module-loader state. Built HERE, ahead of the core API install,
    //     because `cook.chore` (CS-0176) checks its namespace prefix against
    //     `current_module` and so must close over this handle at install time.
    //     `install_remaining_apis` registers the loader itself over it below.
    let module_state: SharedModuleLoaderState = Rc::new(RefCell::new(ModuleLoaderState::new(
        builder.working_dir.clone(),
    )));

    // 5. Install the full register-phase API surface (`cook.*` core,
    //    fs/path/platform sandboxing, module loader,
    //    unit/export/test/dep_output/codec APIs). Shared with `list_names`
    //    via the extracted helper so both passes see byte-identical
    //    installation. Returns the per-pass `recipes` Rc that
    //    `cook.recipe(...)` captures into.
    // COOK-64: the register pre-pass populates this with resolved
    // member-source probe values before any recipe body runs; the
    // `cook.probes.get` binding (installed below) reads it first.
    let prepass_store: crate::module_loader::SharedPrepassStore =
        Rc::new(RefCell::new(BTreeMap::new()));
    // CS-0219: one owner for register-phase probe resolution. The pre-pass
    // below fills it for `gather <probe>` drivers; a register-phase
    // `cook.probes.get` read fills it on demand for whatever a body asks for.
    let probe_resolver = Rc::new(RegisterProbeResolver::new(
        probe_registry.clone(),
        prepass_store.clone(),
        builder.working_dir.clone(),
        builder.qualified_prefix.clone(),
        cache_ctx.clone(),
        body_slot.clone(),
    ));
    // The forcer cell `cook.require_recipe` reads at call time. Created here,
    // filled at step 12 once the driver exists: the top-level chunk (step 6)
    // runs long before that, and a Cookfile aliasing the function there
    // (`local rr = cook.require_recipe`) keeps whatever closure it captured
    // forever. One closure over a late-filled cell is what keeps the alias and
    // a fresh `cook.` lookup indistinguishable (Standard §22.8, CS-0144).
    let recipe_forcer: crate::context::SharedRecipeForcer = Rc::new(RefCell::new(None));
    // Standard §22.9, CS-0149: the `cook.on_register_complete` finalizer
    // queue for this pass. Created here, alongside the forcer cell, for the
    // same reason: `install_all_apis` needs it at install time (step
    // 5, below), but nothing drains it until step 12c, well after the
    // top-level chunk and every recipe body have queued into it.
    let finalizer_queue: crate::on_register_api::SharedFinalizerQueue =
        Rc::new(RefCell::new(Vec::new()));
    let recipes = install_all_apis(
        &lua,
        &builder,
        body_slot.clone(),
        cache_ctx.as_ref(),
        probe_resolver.clone(),
        recipe_forcer.clone(),
        finalizer_queue.clone(),
        module_state.clone(),
        probe_registry.clone(),
        cookfile_label.clone(),
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
    let final_env = dispatch_config_blocks(&lua, &builder, &host_reads)?;

    // 7b. CS-0172: with config resolved, run the rest of the program — the
    //     top-level module calls, register blocks, and recipe registrations
    //     codegen wrapped in `__cook_main`.
    run_main_program(&lua)?;
    warn_var_shadowing(&builder, &recipes.borrow(), &probe_registry.borrow());

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

    // 11b. (COOK-61) Set of names reachable from the dispatch target via the
    //      `requires` graph. The body-invocation loop uses it to decide
    //      whether a chore body runs at all (Standard §7.6, CS-0218) and
    //      whether a dep-of-target parametric chore binds empty argv (§7.5.1).
    //
    //      Two sources, unioned. This pass's own local closure from
    //      `target_recipe` — empty when there is no target or the target is
    //      not registered here — and the workspace layer's
    //      `reachable_names`, which are the entry points a cross-Cookfile
    //      edge lands on (CS-0218). A register pass sees ONE Cookfile, so
    //      `chore a: sub.b` is invisible to `sub`'s pass; closing over the
    //      local graph from each supplied seed is what turns "`b` is
    //      reachable" into "`b` and everything `b` requires are reachable".
    //
    //      The closure itself is `cook_contracts::recipe::reachable_from` —
    //      the same function `cook-plan` runs over the composed workspace.
    //      Two implementations of "reachable" would let a chore be reachable
    //      to the layer that supplies the seeds and unreachable to the layer
    //      that acts on them, which is a chore registering zero units while
    //      the build waits for it.
    let reachable_from_target: std::collections::BTreeSet<String> =
        cook_contracts::recipe::reachable_from(
            &names_to_requires,
            builder
                .target_recipe
                .iter()
                .cloned()
                .chain(builder.reachable_names.iter().cloned()),
        );

    // 11c. (COOK-64 §22.5.10) The member-source register pre-pass. Every recipe
    //      body runs during register to discover its units, and a
    //      probe-sourced member-fanout body opens with
    //      `local _items = cook.probes.get("<key>")`. That call resolves nil
    //      (or errors "outside a module context") unless the feeding probe
    //      has already been evaluated — so we evaluate every probe that feeds
    //      a member source (and its transitive probe `requires`) here,
    //      before the body loop, stashing each value in `prepass_store` keyed
    //      by probe key. `$(cmd)` and the reserved `(lua)` sources need no
    //      pre-pass (the former materialises through `cook.sh` at body time).
    //      The driver list is COPIED out of `recipes` first. The pre-pass runs
    //      `produce` bodies on this VM, and that Lua reaches `cook.recipe` and
    //      `cook.load_module`, both of which borrow `recipes` — holding the
    //      borrow across it turned an authoring mistake into a RefCell panic.
    let member_source_drivers: Vec<(String, crate::capture::MemberSourceDescriptor)> = recipes
        .borrow()
        .iter()
        .filter_map(|r| r.member_source.clone().map(|ms| (r.name.clone(), ms)))
        .collect();
    run_member_source_prepass(
        &lua,
        &member_source_drivers,
        &probe_resolver,
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
        *recipe_forcer.borrow_mut() = Some(Rc::new(move |lua: &Lua, name: &str| {
            forcer_driver.force(lua, name)
        }));
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

    // 12b. (COOK-64 §22.5.10, generalised by CS-0219) The static-input rule.
    //      Now that every body has run and `units_by_recipe` holds the full
    //      set of recipe outputs, reject any probe THE REGISTER PHASE
    //      RESOLVED that declares a file input produced by a recipe in this
    //      Cookfile. A build artifact is not statically evaluable: the value
    //      was read before any recipe ran, so it could only have seen a stale
    //      or absent file.
    //
    //      The set checked is `resolver.resolved_keys()`, not the member-source
    //      drivers alone. Since CS-0219 a register-phase `cook.probes.get`
    //      resolves whatever a body asks for, and checking only the fan-out
    //      drivers would leave that read as an unchecked route to exactly the
    //      dependence this rule exists to forbid.
    check_register_resolved_static_inputs(
        &probe_resolver.resolved_keys(),
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
    //        the merged-cycle or member-source static-input check would still
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

    // 12d. (§22.5.10, CS-0219) The static-input rule again, over whatever the
    //      finalizer drain resolved. A `cook.on_register_complete` callback is
    //      register-phase Lua like any other and may read a probe, so the 12b
    //      check — which runs before the drain — cannot be the last word: a
    //      probe first resolved by a callback would otherwise slip past the
    //      rule entirely, which is precisely the "by whichever route" the
    //      section requires. Idempotent and cheap: the resolved set is
    //      normally unchanged by the drain, and the check is a set walk.
    check_register_resolved_static_inputs(
        &probe_resolver.resolved_keys(),
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
    // included. The workspace-level probes map is what the executor
    // resolves a probe against before dispatching it
    // (cli/crates/cook-engine/src/run.rs builds `probe_units_by_node` from
    // this map); a body-scope probe that doesn't appear here reaches a
    // worker with no declaration behind it, so a synthesised `files`/`tools`
    // producer would be dispatched as Lua and die on its sentinel.
    let probes: BTreeMap<LocalProbeKey, cook_contracts::ProbeUnit> = probe_registry
        .borrow()
        .probes
        .iter()
        .map(|(key, reg)| (key.clone(), reg.probe.clone()))
        .collect();

    let warnings = builder.gather_warnings.borrow().clone();
    Ok(crate::RegisteredCookfile {
        names,
        units_by_recipe,
        probes,
        final_env,
        warnings,
        // COOK-526: captured before `probe_resolver` (and the registry Rc
        // it borrows) goes out of scope at the end of this function.
        resolved_probe_keys: probe_resolver.resolved_keys(),
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
    /// only declines when `!forced` (the speculative-chore arm stands down when
    /// forced; the member-fanout arm raises), so `Skipped` always implies
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
            let closest = crate::var_api::closest_declared(name, &registered, 3);
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
            // member-fanout arm's DESIGNED hard error is silently swallowed.
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
            // arm that skipped it either stands down (the speculative-chore
            // arms) or raises its designed error (the member-fanout arm). Only
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
            skip_member_fanout_body,
            file_member_source,
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
            bool,
            Option<String>,
        );
        {
            let registry = self.recipes.borrow();
            let recipe = registry
                .iter()
                .find(|r| r.name == name)
                .ok_or_else(|| RegisterError::RecipeNotFound(name.to_string()))?;

            // COOK-64 §22.5.10 demand-driven rule: a probe-sourced member-fanout
            // recipe that is NOT reachable from the build target had its probe
            // skipped by the pre-pass, so its body's `cook.probes.get` would
            // error. Skip the body — the recipe is not being built — registering
            // it with no units, mirroring the speculative-chore skip below.
            skip_member_fanout_body = builder.target_recipe.is_some()
                && !self.reachable_from_target.contains(name)
                && matches!(
                    recipe.member_source,
                    Some(crate::capture::MemberSourceDescriptor::Probe { .. })
                        | Some(crate::capture::MemberSourceDescriptor::Gather { .. })
                );
            file_member_source = match &recipe.member_source {
                Some(crate::capture::MemberSourceDescriptor::Gather { source_ref }) => {
                    let probes = self.probe_registry.borrow();
                    resolve_probe_ref(source_ref, &probes).is_some_and(|(key, field)| {
                        field.is_none() && probes.probes.get(key).is_some_and(|r| {
                            r.probe.produce_source == cook_contracts::probe_value::FILES_MANIFEST_PRODUCE
                        })
                    })
                }
                _ => false,
            };

            // Run recipe context setup (input resolution).
            setup_recipe_context(
                lua,
                recipe,
                &builder.working_dir,
                &builder.workspace_root,
                &builder.gather_warnings,
            )?;

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

        // Skip arm 3 — a probe-sourced member-fanout recipe not statically
        // reachable from the target.
        if skip_member_fanout_body {
            if forced {
                // Unlike the speculative-chore arm, forcing cannot rescue this
                // one: the body needs a probe value the pre-pass never
                // computed. Evaluating the probe lazily here IS feasible —
                // `run_member_source_prepass` is a free function, and calling it
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
                    "cook.require_recipe: recipe \"{name}\" fans out over a probe member source \
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
        // A chore body is invoked when the chore is the dispatch target (with
        // argv bound into `__cook_params`), or when it is reachable from that
        // target — statically or through `cook.require_recipe` — in which case
        // it runs with no argv supplied. In every other case the invocation
        // would be speculative and the body is not invoked at all (Standard
        // §7.6, CS-0218). Parameter declarations do not enter into that
        // decision: reachability does, and only reachability.
        //
        // Recipe bodies are unaffected — they take no `__cook_params` and are
        // invoked normally, because a recipe's units are the build graph the
        // register pass exists to discover.
        let func: LuaFunction = lua.registry_value(&func_key_clone)?;
        let is_target = builder
            .target_recipe
            .as_deref()
            .map(|t| t == name)
            .unwrap_or(false);
        if kind == crate::RecipeKind::Chore {
            // CS-0176: establish chore unit semantics (no-cache, interactive)
            // for the duration of the body.
            //
            // A SURFACE chore gets these from codegen, which wraps the body in
            // `cook._enter_chore()` / `cook._exit_chore()` (cook-luagen
            // recipe.rs:1362, :1474); those flip the same `current_chore_active`
            // flag this guard sets. A `cook.chore` body is a plain Lua function
            // with no such wrapper, so without this bracket its units would be
            // cacheable and non-interactive — a silent §7.4 violation, and
            // silent is the whole problem: the chore would appear to work while
            // quietly caching a command whose entire purpose is to re-run.
            //
            // Bracketing every chore rather than only the `Dynamic` ones keeps
            // one code path. For a surface chore the codegen enter/exit nests
            // inside a bracket that is already true, and the guard restores the
            // flag to its on-entry value either way.
            let _chore_guard = ChoreActiveGuard::enter(&self.body_slot);
            if is_target {
                // Targeted chore: bind argv and call with __cook_params.
                let argv = &builder.target_argv;
                let (bound, prelude) =
                    build_chore_params_table(lua, &params_meta, argv, name, source_line, &origin)?;
                // Store the prelude on the body slot so cook.add_unit can
                // prepend it to lua_code units captured in this chore body.
                self.set_chore_prelude(prelude);
                func.call::<()>((bound,))
                    .map_err(RegisterError::Lua)?;
            } else if forced || self.reachable_from_target.contains(name) {
                // Reachable, so asked for: run the body with no argv supplied.
                // §7.5.1 — a chore that is a dep of the target runs as if
                // invoked with no positional arguments (a required-no-default
                // parameter surfaces a legitimate register-time error here).
                // For an EMPTY `params_meta` this builds an empty table and an
                // empty prelude, which is what a paramless chore body wants:
                // codegen emits `function(__cook_params)` even then, and a
                // `cook.chore` body declared `function()` ignores the extra
                // argument.
                //
                // A FORCED chore is a dep of the requiring recipe, hence
                // reachable by definition, so it takes this same path on both
                // the target and the no-target branch. Without the `forced`
                // disjunct, `cook.require_recipe` on a chore would silently
                // register zero units — and the no-target branch is the one
                // `cook list` and most tests take.
                let (bound, prelude) =
                    build_chore_params_table(lua, &params_meta, &[], name, source_line, &origin)?;
                self.set_chore_prelude(prelude);
                func.call::<()>((bound,)).map_err(RegisterError::Lua)?;
            } else {
                // SPECULATIVE: a chore that is neither the dispatch target,
                // nor reachable from it, nor forced. Its body is not invoked
                // (Standard §7.6, CS-0218). An empty units entry is recorded
                // so every downstream stage still sees the chore in the
                // registered set — it stays listable and invocable, it just
                // did not run.
                //
                // This arm used to cover only the PARAMETRIC case, on the
                // narrow grounds that such a body would nil-index
                // `__cook_params` on its first prelude line. A paramless body
                // has no such line, so it was invoked unconditionally: "cheap
                // to invoke, captures units for dep linkage when reachable and
                // for enumeration tools when not." Both halves were wrong.
                // Dep linkage when reachable is the arm above. Enumeration
                // never came through here at all — `list_names` invokes no
                // body, which is what `cook menu` and `cook list` run — so the
                // captured units were built and then dropped by the planner, for
                // every non-target chore of every invocation.
                //
                // What made that stop being harmless is CS-0179: a chore body
                // can now rewrite the author's Cookfile, and a module verb
                // that happened to take no arguments would splice or scaffold
                // on every `cook build` of anything. Gating the surfaces
                // instead of the invocation was considered and declined — the
                // VM is `Lua::unsafe_new()`, so a body can reach `os.execute`
                // whatever `cook.cookfile.*` and `fs.*` do about it. Declining
                // to run a body nobody asked for is the only form of this that
                // is a guarantee rather than a fence.
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
                if let (true, Some(path)) = (file_member_source, unit.member.clone()) {
                    let input = cook_contracts::cache::DeclaredInput::path(path);
                    if !meta.inputs.contains(&input) { meta.inputs.push(input); }
                }
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
                        return Err(RegisterError::Lua(mlua::Error::runtime(
                            unresolved_probe_key_message(unit, idx, key),
                        )));
                    }
                }
            }
        }

        // CS-0219 §22.1.3: resolve every `after` entry now that the body has
        // closed and the recipe's whole unit list is known. Deferring it to
        // here rather than checking inside `cook.add_unit` is what lets the
        // diagnostic tell "no unit declares this output" apart from "a unit
        // does, but it is registered later" — inside the call the later unit
        // does not exist yet, so both look identical. The resolution itself is
        // `cook_contracts::unit_graph::resolve_after`, the same function
        // `unit_graph::plan` uses to draw the edge; this call is here for the
        // register-phase diagnostic, not for a second answer.
        cook_contracts::unit_graph::resolve_after(&body.units).map_err(|source| {
            RegisterError::AfterUnresolved {
                recipe: qualified_name.clone(),
                message: source.to_string(),
            }
        })?;

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

/// RAII bracket that marks a chore body active for the duration of its
/// invocation, so units captured inside it get chore semantics — no-cache
/// and interactive (§7.4, CS-0176).
///
/// Sets `BodyCaptureState::current_chore_active`, the same flag
/// `cook._enter_chore()` / `cook._exit_chore()` drive for surface chores
/// (unit_api.rs). This exists because a `cook.chore` body is a plain Lua
/// function that codegen never wrapped, so nothing else would set it.
///
/// Restores the previous value rather than clearing to `false`, and tolerates
/// a `None` body slot on drop: the speculative-skip arm of the chore branch
/// takes the slot and returns while this guard is still alive, so drop must
/// not assume a body is still there to restore.
///
/// The borrow is taken and released inside `enter` and `drop` only — never
/// held across the body call, which re-enters and borrows the slot itself.
struct ChoreActiveGuard<'a> {
    slot: &'a SharedBodySlot,
    previous: bool,
}

impl<'a> ChoreActiveGuard<'a> {
    fn enter(slot: &'a SharedBodySlot) -> Self {
        let previous = {
            let mut borrowed = slot.borrow_mut();
            match borrowed.as_mut() {
                Some(body) => {
                    let previous = body.current_chore_active;
                    body.current_chore_active = true;
                    previous
                }
                None => false,
            }
        };
        Self { slot, previous }
    }
}

impl Drop for ChoreActiveGuard<'_> {
    fn drop(&mut self) {
        if let Some(body) = self.slot.borrow_mut().as_mut() {
            body.current_chore_active = self.previous;
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
    origin: &Option<String>,
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
                    origin: origin.clone(),
                })?;
                table.set(name.as_str(), value.as_str()).map_err(RegisterError::Lua)?;
                prelude.push_str(&format!("local {} = {}\n", name, lua_string::literal(value)));
            }
            ChoreParamMeta::DefaultedString { name, default } => {
                let value = argv_iter
                    .next()
                    .map(|s| s.as_str())
                    .unwrap_or(default.as_str());
                table.set(name.as_str(), value).map_err(RegisterError::Lua)?;
                prelude.push_str(&format!("local {} = {}\n", name, lua_string::literal(value)));
            }
            ChoreParamMeta::DefaultedLua {
                name,
                default_key_name,
            } => {
                if let Some(arg) = argv_iter.next() {
                    table.set(name.as_str(), arg.as_str()).map_err(RegisterError::Lua)?;
                    prelude.push_str(&format!("local {} = {}\n", name, lua_string::literal(arg)));
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
                                origin: origin.clone(),
                            });
                        }
                    };
                    // Spec §7.1.2: result is coerced via Lua tostring rules
                    // for the scalar types (String, Integer, Number, Boolean).
                    // Non-coercible types (Nil, Table, Function, Thread,
                    // UserData, LightUserData, Error) raise ChoreParamDefaultLuaNonString.
                    let coerced: Option<String> = match &result {
                        mlua::Value::String(s) => {
                            s.to_str().map_err(RegisterError::Lua)?.to_string().into()
                        }
                        mlua::Value::Integer(n) => Some(n.to_string()),
                        mlua::Value::Number(n) => Some(n.to_string()),
                        mlua::Value::Boolean(b) => Some(b.to_string()),
                        _ => None,
                    };
                    match coerced {
                        Some(s_str) => {
                            table.set(name.as_str(), s_str.as_str()).map_err(RegisterError::Lua)?;
                            prelude
                                .push_str(&format!("local {} = {}\n", name, lua_string::literal(&s_str)));
                        }
                        None => {
                            return Err(RegisterError::ChoreParamDefaultLuaNonString {
                                chore: chore_name.to_string(),
                                name: name.clone(),
                                line: source_line,
                                ty: result.type_name().to_string(),
                                origin: origin.clone(),
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
                        origin: origin.clone(),
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
                    .map(|v| lua_string::literal(v))
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
                    .map(|v| lua_string::literal(v))
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

/// The end-of-pass resolution failure for a probe key a unit consumes, told in
/// terms of the surface that put the key there (CS-0235).
///
/// A unit's `probes` list is a confluence: it holds what `cook.add_unit`'s
/// `probes` field named, plus the recipe's `seal` refs (unioned in so a sealed
/// probe is scheduled ahead of the unit exactly as a consumed one is), plus
/// keys scanned out of `$<key>` sigils and literal `cook.probes.get` reads. Only
/// the first of those is a `probes` list the author wrote, so only the first
/// gets §22.5.6 rule 1's mandated sentence about one. A `seal` ref that resolves
/// to nothing gets §8.4.3.1 rule 4's sentence instead, naming the step the
/// author actually wrote and the declaration kinds a ref may name.
///
/// The provenance is still on the unit — `CacheMeta.seal_keys` is the seal set
/// as declared, before the union — so one branch here answers for every caller
/// rather than each surface carrying its own copy of the check.
fn unresolved_probe_key_message(
    unit: &cook_contracts::unit::CapturedUnit,
    idx: usize,
    key: &str,
) -> String {
    let sealed = unit
        .cache_meta
        .as_ref()
        .is_some_and(|m| m.seal_keys.contains(key));
    if sealed {
        return format!(
            "seal: '{key}' does not name a probe, or a top-level `files` or `tools` declaration"
        );
    }
    let unit_name = unit
        .cache_meta
        .as_ref()
        .and_then(|m| m.output_paths.first())
        .map(|p| p.as_str())
        .unwrap_or("");
    let unit_label = if unit_name.is_empty() {
        format!("<unit-{idx}>")
    } else {
        unit_name.to_string()
    };
    format!("unit '{unit_label}' lists probe key '{key}' in `probes` but no such probe was declared")
}

/// COOK-64 §22.5.10: the member-source register pre-pass.
///
/// Every probe-sourced member source opens its body with
/// `local _items = cook.probes.get("<ref>")`, `<ref>` being the verbatim
/// `gather <ref>` source ref carried by codegen. That value does not
/// exist until the feeding probe runs, and probes normally run as DAG nodes
/// in the execute phase — far too late for register-time fan-out. So we
/// evaluate every member-source probe (and its transitive probe
/// `requires`) here, synchronously on the register VM, before any recipe
/// body runs.
///
/// COOK-190 / §22.5.10: a ref is resolved against the probe registry via
/// `resolve_probe_ref` — an exact whole-ref key match wins, else the ref is
/// a `key:field` selector. The evaluation mirrors the execute-phase probe
/// path (`executor.rs`): resolve the declared `tools`/`files` sets, take a
/// value a synthesised producer kind determines without a VM, else run
/// `produce` on the VM, then materialise `.cook/probes/<key>.json`. The
/// resolved value is stashed in `prepass_store` keyed by probe key; for a
/// field-selector ref, the selected array is additionally stashed under the
/// verbatim ref (see below), which is what the `cook.probes.get` binding in
/// the generated body actually reads.
///
/// Both retained member-source descriptors name probes and require this
/// pre-pass; `Gather` additionally admits named files manifests.
fn run_member_source_prepass(
    lua: &Lua,
    member_source_drivers: &[(String, crate::capture::MemberSourceDescriptor)],
    resolver: &RegisterProbeResolver,
    reachable_from_target: &std::collections::BTreeSet<String>,
    has_target: bool,
) -> Result<(), RegisterError> {
    let probe_registry_guard = resolver.registry.borrow();
    let probe_registry = &*probe_registry_guard;
    let prepass_store = resolver.store();
    use crate::capture::MemberSourceDescriptor;

    // (recipe, verbatim source ref) per probe-sourced driver.
    //
    // §22.5.10 demand-driven rule: when a build target is set, only evaluate
    // probes for recipes reachable from it. When no target is set every recipe
    // is being built, so every probe-sourced driver is in scope. The body loop
    // applies the mirror rule (`should_skip_member_fanout_body`) so a non-reachable
    // driver's body — which would call `cook.probes.get` on an unevaluated probe
    // — is skipped rather than erroring.
    let driver_reachable = |name: &str| !has_target || reachable_from_target.contains(name);
    let drivers: Vec<(&str, &str, bool)> = member_source_drivers
        .iter()
        .filter(|(name, _)| driver_reachable(name))
        .map(|(name, source)| {
            let (source_ref, gather) = match source {
                MemberSourceDescriptor::Probe { source_ref } => (source_ref, false),
                MemberSourceDescriptor::Gather { source_ref } => (source_ref, true),
            };
            (name.as_str(), source_ref.as_str(), gather)
        })
        .collect();
    if drivers.is_empty() {
        return Ok(());
    }

    // COOK-190: resolve each ref against the registry (exact key match wins,
    // else trailing `:field` selector). A ref that names no declared probe
    // under either interpretation is rejected, naming the full ref.
    let mut resolved: Vec<(&str, &str, Option<&str>, bool)> = Vec::new();
    for (recipe, source_ref, gather) in &drivers {
        match resolve_probe_ref(source_ref, probe_registry) {
            Some((key, field)) => resolved.push((source_ref, key, field, *gather)),
            None => {
                return Err(RegisterError::MemberSourceProbeUndeclared {
                    recipe: (*recipe).to_string(),
                    key: (*source_ref).to_string(),
                })
            }
        }
    }

    // Evaluate each driver probe (and its transitive `requires`) in
    // dependency order. Probe cycles are already rejected (step 9 above), so
    // the recursion terminates.
    //
    // The registry borrow above is released for the duration: a `produce` body
    // runs author Lua on this VM, and that Lua may declare or read probes.
    let keys: Vec<String> = resolved.iter().map(|(_, k, _, _)| (*k).to_string()).collect();
    drop(probe_registry_guard);
    for key in &keys {
        resolver.resolve(lua, key)?;
    }
    let probe_registry = &*resolver.registry.borrow();

    // §22.5.10 non-array diagnostic: a driver's resolved source must be a
    // sequence. With a `:field` selector, the named field must be the array.
    let mut files_members = Vec::new();
    for (source_ref, key, field, gather) in &resolved {
        let store = prepass_store.borrow();
        let value = store.get(*key).expect("driver probe evaluated above");
        let (resolved_value, selector): (&serde_json::Value, String) = match field {
            Some(f) => match json_map_get(value, f) {
                Some(v) => (v, (*source_ref).to_string()),
                None => {
                    return Err(RegisterError::MemberSourceNotArray {
                        selector: (*source_ref).to_string(),
                        shape: "nil (no such field)".to_string(),
                    })
                }
            },
            None => (value, (*source_ref).to_string()),
        };
        let files_source = field.is_none() && probe_registry.probes.get(*key).is_some_and(|r| {
            r.probe.produce_source == cook_contracts::probe_value::FILES_MANIFEST_PRODUCE
        });
        if *gather && files_source {
            let paths = resolved_value.as_object().expect("files declaration yields a manifest")
                .keys().cloned().map(serde_json::Value::String).collect();
            files_members.push(((*source_ref).to_string(), serde_json::Value::Array(paths)));
        } else if !matches!(resolved_value, serde_json::Value::Array(_)) {
            return Err(RegisterError::MemberSourceNotArray {
                selector,
                shape: json_shape(resolved_value).to_string(),
            });
        }
    }
    prepass_store.borrow_mut().extend(files_members);

    // COOK-190: the body reads `cook.probes.get("<verbatim ref>")`. For a
    // `key:field` selector, stash the selected array under the verbatim ref
    // (validated array-shaped by the diagnostic loop above).
    for (source_ref, key, field, _) in &resolved {
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

/// Resolve a `gather <probe>` source ref against the probe registry. Keys
/// admit any number of segments, so the final `:` may separate either key
/// segments or a `key:field` selector. A declared probe whose key equals the entire ref
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

/// Everything the register phase needs to turn a declared probe key into a
/// materialised value, and the record of which keys it has (CS-0219).
///
/// One owner for two callers that used to be one. The `gather <probe>`
/// pre-pass resolves the probes a reachable recipe's fan-out cardinality
/// depends on, before any body runs; a register-phase `cook.probes.get` read
/// resolves whichever probe a body actually asks for, at the moment it asks.
/// They are the same operation at different times, so they share the
/// evaluation state — the memo of what is already done, the `requires`
/// recursion, the produce-body frame stack — rather than keeping two, which
/// would let one path re-produce a key the other had already resolved and
/// disagree with it.
///
/// It also keeps [`Self::resolved_keys`], the set of keys the register phase
/// evaluated at all. That set is what §22.5.10's static-input rule is checked
/// against once the body loop closes: a probe resolved before any recipe runs
/// cannot depend on a file a recipe is going to write, however it was reached.
pub struct RegisterProbeResolver {
    registry: Rc<RefCell<ProbeRegistry>>,
    store: crate::module_loader::SharedPrepassStore,
    working_dir: PathBuf,
    qualified_prefix: String,
    cache_ctx: Option<Arc<cook_cache::cache_ctx::CacheContext>>,
    /// The recipe-body capture slot, emptied for the duration of a `produce`
    /// run. A produce body evaluates author Lua on this VM, and since CS-0219
    /// it can fire in the middle of a recipe body — where the slot is `Some`,
    /// so a `cook.add_unit` or `cook.exec` inside `produce` would capture into
    /// whichever recipe happened to be registering. That makes the recipe's
    /// unit set depend on which body was mid-registration when the probe was
    /// demanded, which is registration output as a function of something other
    /// than registration input. Emptied here, those calls raise their ordinary
    /// outside-a-recipe-body error, which is what they did when the only
    /// producer was the pre-pass.
    body_slot: SharedBodySlot,
    /// Does a `cook.probes.get` naming a declared probe resolve it?
    ///
    /// False on a discovery pass (`list_names` / `cook menu`), which registers
    /// no units and executes nothing: running a `produce` body there would put
    /// the cost of every probe a module reads on every menu listing and on
    /// every workspace member scan, uncached, for an answer nothing consults.
    /// A read there falls through to the module store, which is what it did
    /// before CS-0219. §22.9's "discovery surfaces MAY skip the queue" is the
    /// same judgement about the same kind of pass.
    resolution_enabled: bool,
    state: RefCell<ResolverState>,
}

/// One `produce` body currently running, and the upstream keys it is allowed
/// to read.
struct ProducingFrame {
    key: String,
    requires: std::collections::BTreeSet<String>,
}

#[derive(Default)]
struct ResolverState {
    done: std::collections::BTreeSet<String>,
    in_progress: Vec<String>,
    resolved: std::collections::BTreeSet<LocalProbeKey>,
    /// The stack of `produce` bodies running on this VM right now (§22.5.4).
    producing: Vec<ProducingFrame>,
}

impl RegisterProbeResolver {
    pub fn new(
        registry: Rc<RefCell<ProbeRegistry>>,
        store: crate::module_loader::SharedPrepassStore,
        working_dir: PathBuf,
        qualified_prefix: String,
        cache_ctx: Option<Arc<cook_cache::cache_ctx::CacheContext>>,
        body_slot: SharedBodySlot,
    ) -> Self {
        Self {
            registry,
            store,
            working_dir,
            qualified_prefix,
            cache_ctx,
            body_slot,
            resolution_enabled: true,
            state: RefCell::new(ResolverState::default()),
        }
    }

    /// A resolver for a discovery pass: reads fall through, `produce` never
    /// runs. See [`Self::resolution_enabled`].
    pub fn for_discovery(
        registry: Rc<RefCell<ProbeRegistry>>,
        working_dir: PathBuf,
        body_slot: SharedBodySlot,
    ) -> Self {
        let mut r = Self::new(
            registry,
            Rc::new(RefCell::new(BTreeMap::new())),
            working_dir,
            String::new(),
            None,
            body_slot,
        );
        r.resolution_enabled = false;
        r
    }

    /// Would a `cook.probes.get` on this key resolve a probe?
    ///
    /// False for a key no probe declares, and false on a discovery pass.
    pub fn resolves(&self, key: &str) -> bool {
        self.resolution_enabled && self.declares(key)
    }

    /// Does any registered probe carry this key? (CS-0240.)
    ///
    /// The half of [`Self::resolves`] that is about the NAME rather than about
    /// whether this pass will act on it, and the membership question
    /// §{xref.resolution} step 3 asks. Only the first half belongs there: a
    /// probe sigil means the same thing on a discovery pass as on a resolving
    /// one, and classifying it by the pass mode would make `$<keyed_obs>` a
    /// probe reference or a declared-variable reference depending on which
    /// pass happened to read it.
    pub fn declares(&self, key: &str) -> bool {
        self.registry.borrow().probes.contains_key(key)
    }

    /// §22.5.4: a read made from inside a `produce` body may only name a key
    /// the running probe declared in `inputs.requires`. `Ok(())` when the read
    /// is permitted (including when no produce body is running).
    ///
    /// Without this rule the read is silently outside the ordering `requires`
    /// establishes: the upstream is scheduled only for keys named there, so an
    /// undeclared read reaches whatever happens to have been resolved already.
    /// It is also what makes a `produce` source mean the same thing in both
    /// phases: on a worker VM the same undeclared read raises §22.5.8's
    /// unmaterialised error, because nothing scheduled the upstream.
    ///
    /// It is the cycle guard too. A `produce` body naming its own key, or two
    /// bodies naming each other, is not a `requires` edge, so §22.5.9's
    /// end-of-pass cycle check never sees it; before this rule such a body
    /// re-entered resolution with a clean slate and recursed until the Lua
    /// stack gave out.
    pub fn check_produce_read(&self, key: &str) -> Result<(), RegisterError> {
        let state = self.state.borrow();
        let Some(frame) = state.producing.last() else {
            return Ok(());
        };
        if frame.requires.contains(key) {
            return Ok(());
        }
        Err(RegisterError::ProbeReadOutsideRequires {
            reader: frame.key.clone(),
            key: key.to_string(),
        })
    }

    /// The pre-pass value store this resolver publishes into.
    pub fn store(&self) -> &crate::module_loader::SharedPrepassStore {
        &self.store
    }

    /// Is `key` a probe someone declared in this pass?
    pub fn is_declared(&self, key: &str) -> bool {
        self.registry.borrow().probes.contains_key(key)
    }

    /// Every probe key the register phase resolved, in key order.
    pub fn resolved_keys(&self) -> Vec<LocalProbeKey> {
        self.state.borrow().resolved.iter().cloned().collect()
    }

    /// Materialise `key`'s value into the store, recursing through its declared
    /// probe `requires` first so an upstream is resolved before the body that
    /// reads it (CS-0244: `requires` orders, it does not key).
    ///
    /// Idempotent; `in_progress` guards against (already-rejected) `requires`
    /// cycles, and [`Self::check_produce_read`] guards the produce-body route
    /// §22.5.9's check cannot see.
    ///
    /// **No `RefCell` borrow may be held across the produce call.** Producing
    /// runs author Lua on the register VM, and that Lua re-enters this crate's
    /// APIs, so the state is read into locals, released, and merged back
    /// afterwards. The one thing deliberately held across it is the
    /// `producing` frame, which is the guard.
    pub fn resolve(&self, lua: &Lua, key: &str) -> Result<(), RegisterError> {
        if self.state.borrow().done.contains(key) {
            return Ok(());
        }
        let probe = {
            let registry = self.registry.borrow();
            let Some(reg) = registry.probes.get(key) else {
                return Err(RegisterError::ProbeProduceFailed {
                    key: key.to_string(),
                    message: format!("requires upstream probe '{key}' which was not declared"),
                });
            };
            reg.probe.clone()
        };

        {
            let mut state = self.state.borrow_mut();
            if state.in_progress.iter().any(|k| k == key) {
                return Ok(()); // defensive — step 9 already rejects probe cycles
            }
            state.in_progress.push(key.to_string());
        }
        let recursed = (|| -> Result<(), RegisterError> {
            for req in &probe.inputs.requires {
                self.resolve(lua, req)?;
            }
            Ok(())
        })();
        self.state.borrow_mut().in_progress.pop();
        recursed?;

        // Everything from resolving the declared inputs to materialising the
        // canonical local copy is `cook_probe::eval` (COOK-359). It is the same
        // call the executor makes, so the two phases cannot drift again: the
        // declared `tools`/`files` resolution, the CS-0148 top-level `files`
        // value synthesis, and the CS-0102/CS-0243 local copy all have one
        // implementation. The register VM is the only phase-specific part, and
        // it is the parameter.
        let eval_ctx = cook_probe::eval::EvalCtx {
            working_dir: &self.working_dir,
            project_root: self.cache_ctx.as_ref().map(|ctx| ctx.project_root.as_path()),
            declaring_prefix: &self.qualified_prefix,
        };
        // The produce window. Two things are true only inside it, and both are
        // restored unconditionally below: reads are confined to this probe's
        // declared `requires`, and the recipe-body capture slot is empty.
        self.state.borrow_mut().producing.push(ProducingFrame {
            key: key.to_string(),
            requires: probe.inputs.requires.iter().cloned().collect(),
        });
        let saved_body = self.body_slot.borrow_mut().take();
        let produced = cook_probe::eval::evaluate(
            &probe,
            &eval_ctx,
            &RegisterVmRunner { lua },
        );
        *self.body_slot.borrow_mut() = saved_body;
        self.state.borrow_mut().producing.pop();
        let evaluated = produced.map_err(|e| RegisterError::ProbeProduceFailed {
            key: key.to_string(),
            message: e.message,
        })?;
        for warning in &evaluated.warnings {
            eprintln!("cook: warning: {warning}");
        }
        // `evaluated.tool_paths` is deliberately dropped here. CS-0157's
        // resolved locations are a read view for execute-phase Lua consumers,
        // served from the per-run ProbeValueStore; the register-phase store
        // holds decoded values and has no such channel. If one is ever added,
        // this is where it gets populated.

        let jv = cook_contracts::probe_value::decode_json(&evaluated.bytes).map_err(|e| {
            RegisterError::ProbeProduceFailed {
                key: key.to_string(),
                message: format!("decode observed value: {e}"),
            }
        })?;
        self.store.borrow_mut().insert(key.to_string(), jv);
        let mut state = self.state.borrow_mut();
        state.done.insert(key.to_string());
        state.resolved.insert(LocalProbeKey::new(key));
        Ok(())
    }
}

/// The register phase's half of the `cook_probe::eval` seam: run a probe's
/// `produce` source on the REGISTER VM.
///
/// This is the one step the two phases genuinely do differently — the executor
/// runs the same source on a worker VM under execute-phase policy — and it is
/// the reason the whole sequence used to exist twice.
struct RegisterVmRunner<'a> {
    lua: &'a Lua,
}

impl cook_probe::eval::ProduceRunner for RegisterVmRunner<'_> {
    fn run(&self, key: &str, source: &str) -> Result<cook_probe::eval::Produced, String> {
        // CS-0244: what a `produce` body loads is no longer snapshotted. It fed
        // the probe fingerprint's module-source fold and nothing else, and a
        // reached probe re-observes on every invocation regardless (CS-0243),
        // so the next observation simply runs whatever the module says now.
        Ok(cook_probe::eval::Produced {
            bytes: run_prepass_produce(self.lua, key, source)?,
        })
    }
}

/// Run a probe's `produce` source on the register VM and return the
/// canonical-JSON bytes (the execute-VM counterpart is `cook-execute`'s
/// `execute_probe`; CS-0102). The lowering — chunk name and wrapper — is
/// `cook_contracts::probe::lower_produce`, the one law both VMs evaluate
/// under, so a produce body's error reports the same line numbers
/// whichever phase ran it.
fn run_prepass_produce(lua: &Lua, key: &str, produce: &str) -> Result<Vec<u8>, String> {
    let lowered = cook_contracts::probe::lower_produce(key, produce);
    let value: LuaValue = lua
        .load(&lowered.source)
        .set_name(&lowered.chunk_name)
        .eval()
        .map_err(|e| e.to_string())?;
    let jv = crate::probe_value::lua_to_json(&value)?;
    Ok(crate::probe_value::encode_canonical_json(&jv))
}

/// Look up a string-keyed field in a JSON object. `None` for non-objects or a
/// missing key.
fn json_map_get<'a>(v: &'a serde_json::Value, field: &str) -> Option<&'a serde_json::Value> {
    v.as_object().and_then(|m| m.get(field))
}

/// §22.5.10 static-input rule: reject a probe THE REGISTER PHASE RESOLVED
/// whose declared file inputs include a build artifact (an output produced by
/// a recipe in this Cookfile). Register-phase resolution happens before any
/// recipe runs, so depending on a not-yet-built file is incoherent — the value
/// could only ever have observed a stale or absent file.
///
/// `resolved` is every key the register phase materialised, whether reached as
/// a `gather <probe>` fan-out driver, as a transitive `inputs.requires`
/// of one, or by a register-phase `cook.probes.get` read (CS-0219). Checking
/// the drivers alone would leave the read as a hole in the rule, and the rule
/// is what lets cook resolve the whole graph before the first command runs.
///
/// Runs after the body loop, when `units_by_recipe` carries every recipe's
/// output paths. Paths are compared in normalised relative form.
fn check_register_resolved_static_inputs(
    resolved: &[LocalProbeKey],
    probe_registry: &ProbeRegistry,
    units_by_recipe: &BTreeMap<String, RecipeUnits>,
) -> Result<(), RegisterError> {
    if resolved.is_empty() {
        return Ok(());
    }
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

    for key in resolved {
        let Some(reg) = probe_registry.probes.get(key) else {
            continue; // resolved implies declared; nothing to check otherwise
        };
        for file in &reg.probe.inputs.files {
            if outputs.contains(&normalise_rel(file)) {
                return Err(RegisterError::MemberSourceProbeArtifactDep {
                    key: key.to_string(),
                    path: file.clone(),
                });
            }
        }
    }
    Ok(())
}

/// Normalise a relative path for comparison: drop a leading `./`. Both
/// member-source probe file inputs and recipe output paths are relative to the
/// project working directory, so a textual normalise suffices.
///
/// The rule itself is `cook_contracts::pathlaw::strip_dot_slash` (COOK-414's
/// point, applied): §22.1.3's `after` resolution compares declared paths by
/// exactly this equivalence, and one rule with two spellings is a rule that
/// drifts.
fn normalise_rel(p: &str) -> String {
    cook_contracts::pathlaw::strip_dot_slash(p).to_string()
}

/// Human-readable JSON value-kind, for the §22.5.10 non-array diagnostic.
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
fn detect_collisions(recipes: &[crate::capture::RegisteredRecipe]) -> Result<(), RegisterError> {
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
            (crate::capture::RegistrationSource::Dynamic { .. }, crate::RecipeKind::Recipe) => {
                RegistrationSiteKind::Dynamic
            }
            // CS-0176: a `cook.chore` registration, so the collision names
            // `cook.chore` rather than a `cook.recipe` call that never happened.
            (crate::capture::RegistrationSource::Dynamic { .. }, crate::RecipeKind::Chore) => {
                RegistrationSiteKind::DynamicChore
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

    // Module-loader state, ahead of the core API install — `cook.chore`
    // closes over it (CS-0176). `list_names` DOES need this wired properly:
    // it evaluates the top-level chunk, so `use cook_cc` runs the module and
    // its `cook.chore` registrations, which is exactly how `cook menu` comes
    // to list module-provided verbs.
    let module_state: SharedModuleLoaderState = Rc::new(RefCell::new(ModuleLoaderState::new(
        builder.working_dir.clone(),
    )));

    // Install the full API surface via the shared helper — byte-identical
    // to `register_cookfile`'s installation, which is exactly why `cook
    // menu` lists what a build would register. `list_names` never invokes
    // recipe bodies: no flush of the module-loader handle is needed (no
    // module bodies ran), and the pre-pass store stays empty —
    // `cook.probes.get` falls through to its module-context behaviour.
    let recipes = install_all_apis(
        &lua,
        &builder,
        body_slot.clone(),
        None,
        Rc::new(RegisterProbeResolver::for_discovery(
            probe_registry.clone(),
            builder.working_dir.clone(),
            body_slot.clone(),
        )),
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
        module_state,
        probe_registry.clone(),
        cookfile_label.clone(),
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
    let _final_env = dispatch_config_blocks(&lua, &builder, &host_reads)?;

    // CS-0172: registration lives in `__cook_main`, so the listed set does not
    // exist until it runs — and it must run after config dispatch so a
    // `register` block that gates a registration on a `var` sees the resolved
    // value. Bodies are still never invoked.
    run_main_program(&lua)?;
    warn_var_shadowing(&builder, &recipes.borrow(), &probe_registry.borrow());

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

/// Install the ENTIRE register-phase API surface on the given Lua VM:
/// `cook.*` core (recipe/chore/var/probe), fs/path/platform sandboxes,
/// module loader, unit/export/test/dep_output/codec APIs.
///
/// Extracted as a shared helper so `register_cookfile` and `list_names`
/// see byte-identical API installation — drift here means `cook menu`
/// sees a different Lua API than a build (COOK-396 folded the previously
/// call-site-duplicated core install into this helper for that reason).
///
/// `cache_ctx` is `Some` when the caller has a project root resolved
/// (`register_cookfile` in production); `None` for tests, legacy call
/// sites, and `list_names`. The sandbox falls back to the recipe's
/// working_dir in the `None` case, which matches single-Cookfile project
/// behavior (CS-0045).
///
/// Returns the per-pass `recipes` Rc that `cook.recipe(...)` captures into.
#[allow(clippy::too_many_arguments)]
fn install_all_apis(
    lua: &Lua,
    builder: &RegisterSessionBuilder,
    body_slot: SharedBodySlot,
    cache_ctx: Option<&Arc<cook_cache::cache_ctx::CacheContext>>,
    probe_resolver: Rc<RegisterProbeResolver>,
    recipe_forcer: crate::context::SharedRecipeForcer,
    finalizer_queue: crate::on_register_api::SharedFinalizerQueue,
    // CS-0176: created by the caller, before `install_cook_api`, because
    // `cook.chore` validates its namespace against the module being evaluated
    // and therefore needs this handle at install time — earlier than the
    // module loader itself used to be built.
    module_state: SharedModuleLoaderState,
    probe_registry: Rc<RefCell<ProbeRegistry>>,
    cookfile_label: String,
) -> Result<Rc<RefCell<Vec<crate::capture::RegisteredRecipe>>>, RegisterError> {
    // `cook.*` core. `recipe_name` is the legacy closure-capture argument
    // used by `cook.add_unit` for `cache_meta.recipe_name`; neither caller
    // has a single recipe at install time, so it is always "" here —
    // per-recipe attribution happens through `body.current_recipe`.
    let recipes = install_cook_api(
        lua,
        builder.env_vars.clone(),
        &builder.working_dir,
        body_slot.clone(),
        "",
        module_state.clone(),
    )?;
    {
        let cook_tbl: LuaTable = lua.globals().get("cook")?;
        install_var_api(lua, &cook_tbl, builder.env_keyset.clone())?;
    }
    {
        let cook_tbl: LuaTable = lua.globals().get("cook")?;
        install_cook_probe(
            lua,
            &cook_tbl,
            probe_registry,
            body_slot.clone(),
            cookfile_label,
        )?;
    }
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
        cook_lua_stdlib::SandboxSource::confined(project_root.clone()),
    )?;
    {
        let cook_tbl: LuaTable = lua.globals().get("cook")?;
        cook_lua_stdlib::register_platform_api(lua, &cook_tbl)?;
        // CS-0179: `cook.cookfile.*`, under the same sandbox policy as `fs.*`
        // — a chore may rewrite the Cookfile that invoked it, never one
        // outside the project root.
        cook_lua_stdlib::register_cookfile_api(
            lua,
            &cook_tbl,
            cook_lua_stdlib::WorkingDirSource::Static(builder.working_dir.clone()),
            cook_lua_stdlib::SandboxSource::confined(project_root.clone()),
        )?;
    }

    // Module loader + remaining cook APIs. `module_state` is the caller's
    // (see the parameter note) — the loader is registered over it here.
    crate::module_loader::register_module_loader(lua, module_state.clone())?;
    crate::module_loader::register_cache_api(lua, module_state.clone(), probe_resolver)?;
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
    crate::context::register_resolve_gather(lua, &builder.working_dir, &builder.workspace_root)?;
    // cook.json_decode / cook.yaml_decode are both-phase (§24.8, CS-0123);
    // the shared implementation lives in cook-lua-stdlib so the worker VMs
    // in cook-execute install byte-identical behaviour.
    let cook_tbl: LuaTable = lua.globals().get("cook")?;
    cook_lua_stdlib::register_codec_api(lua, &cook_tbl)?;
    // CS-0158: cook.tools.id — canonical tool identity, both-phase (a probe
    // produce body must behave identically on the register pre-pass and the
    // execute-phase demand path).
    cook_lua_stdlib::register_tools_api(lua, &cook_tbl)?;
    Ok(recipes)
}

/// Dispatch any `__cook_run_config_blocks` function emitted by codegen
/// and return the final post-dispatch env snapshot.
///
/// When codegen has emitted a config-block dispatcher, this:
///
/// 1. Sandboxes the dispatcher's `_ENV` (Standard §5.3.2, CS-0163): the config
///    body sees `host.*` (its only external-input surface), the `var` output
///    sink (CS-0164, an alias of `cook.env`), and a pure-Lua subset — never
///    `os`/`io`/clocks/randomness. `host.*` reads are recorded into
///    `host_reads`.
/// 2. Calls the dispatcher with the builder's `selected_config`.
/// 3. Freezes the env keyset against the post-dispatch table.
/// 4. Re-applies any `--set KEY=VALUE` CLI overrides on top.
/// 5. Snapshots the env back into the builder's shared `env_vars` map.
/// 6. Emits one §5.2.3 shadowing warning per recipe-name / declared-env
///    collision (deduped via `builder.shadow_warnings_emitted`).
/// 7. The `var` sink lives inside the sandbox `_ENV`, not on the real globals,
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
    host_reads: &crate::config_sandbox::SharedHostReads,
) -> Result<BTreeMap<String, String>, RegisterError> {
    if let Ok(dispatch) = lua.globals().get::<LuaFunction>(cook_contracts::registration::CONFIG_DISPATCH_NAME) {
        // CS-0172: the store the config bodies write, reachable only through
        // the registry — the `var` global outside a config block is a read-only
        // proxy onto it.
        let store: LuaTable = crate::var_api::var_store(lua)?;

        // Sandbox the config function (Standard §5.3.2, CS-0163). `_ENV` is
        // swapped for the restricted table so `os`/`io`/clock/randomness/etc.
        // are unreachable and the only external-input surface is `host.*`; the
        // `var` output sink is exposed inside the sandbox rather than as a real
        // global, so a config body writes the store while every other Lua
        // context sees the read-only proxy. `set_environment` returns false
        // only when the function references no globals at all (an empty /
        // pure-literal config body) — nothing to sandbox in that case.
        let sandbox = crate::config_sandbox::build_config_sandbox_env(
            lua,
            &store,
            &builder.working_dir,
            host_reads,
        )?;
        dispatch.set_environment(sandbox)?;

        let name_arg: Option<String> = builder.selected_config.clone();
        dispatch.call::<()>(name_arg)?;

        builder.env_keyset.freeze(&store)?;

        // CLI overrides apply on top of the config blocks so `--set` wins over
        // a config-block default however the block was authored (closing the
        // clobber trap of App. D.15). CS-0172: an override may only name a
        // variable the config blocks declared — the namespace has no other
        // source, so inventing one here would resurrect the undeclared-var
        // channel this CS removes.
        check_overrides_declared(builder)?;
        for (k, v) in &builder.cli_overrides {
            store.set(k.as_str(), v.as_str())?;
        }

        // Snapshot the store into the session's string-valued map — the form
        // the cache key hashes and `$<NAME>` interpolates. A declared value
        // that is not a string, number, or boolean is rejected here, naming the
        // variable (CS-0172); it cannot reach a determinant otherwise.
        {
            let mut env_map = builder.env_vars.borrow_mut();
            for pair in store.pairs::<String, LuaValue>() {
                let (k, v) = pair?;
                let rendered = crate::var_api::var_to_string(&k, &v)?;
                env_map.insert(k, rendered);
            }
        }

        // `env_vars` was just refreshed above so it's the canonical source;
        // copy from there. (The `var` sink lives inside the config sandbox
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
        // No config blocks: the declared set is empty, so any `--set` names an
        // undeclared variable (CS-0172). Previously these were silently
        // injected and `$<NAME>` resolved them.
        check_overrides_declared(builder)?;
        Ok(builder
            .env_vars
            .borrow()
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect())
    }
}

/// Reject every `--set NAME=VALUE` whose `NAME` no `config` block declared
/// (CS-0172, §5.3.1), suggesting the closest declared names.
fn check_overrides_declared(builder: &RegisterSessionBuilder) -> Result<(), RegisterError> {
    if builder.cli_overrides.is_empty() {
        return Ok(());
    }
    let declared = builder.env_keyset.declared_list();
    // Deterministic diagnostic when several overrides are wrong: report the
    // lexicographically first offender rather than whichever the map yields.
    let mut offenders: Vec<(&String, &String)> = builder
        .cli_overrides
        .iter()
        .filter(|(k, _)| !builder.env_keyset.contains(k))
        .collect();
    offenders.sort_by(|a, b| a.0.cmp(b.0));
    if let Some((name, value)) = offenders.first() {
        return Err(RegisterError::UndeclaredSet {
            name: (*name).clone(),
            value: (*value).clone(),
            closest: crate::var_api::closest_declared(name, &declared, 3),
        });
    }
    Ok(())
}

/// Run the transpiled program's `__cook_main` body (CS-0172).
///
/// When a Cookfile declares any `config` block, codegen wraps everything after
/// the config function — top-level `module_call`s, `register` blocks, and the
/// `cook.recipe(...)` registrations — in `__cook_main`, and the chunk's `exec`
/// therefore only *defines*. Calling it here, after
/// [`dispatch_config_blocks`], is what lets register-phase Lua read resolved
/// `var` values: a `cook_cc.toolchain({ optimize = var.optimize })` at top
/// level runs with the config already applied.
///
/// A Cookfile with no config block emits no wrapper and has already run its
/// top level during `exec`, so the absent global is the no-op case.
fn run_main_program(lua: &Lua) -> Result<(), RegisterError> {
    if let Ok(main) = lua.globals().get::<LuaFunction>(cook_contracts::registration::MAIN_PROGRAM_NAME) {
        main.call::<()>(())?;
    }
    Ok(())
}

/// Emit the §5.2.3 diagnostic for each recipe name that collides with a
/// declared `var` name.
///
/// Runs after [`run_main_program`] rather than inside
/// [`dispatch_config_blocks`]: registration now happens after config dispatch
/// (CS-0172), so the recipe set does not exist yet at dispatch time.
fn warn_var_shadowing(
    builder: &RegisterSessionBuilder,
    recipes: &[crate::capture::RegisteredRecipe],
    probes: &ProbeRegistry,
) {
    let declared: std::collections::BTreeSet<String> =
        builder.env_keyset.declared_list().into_iter().collect();
    if declared.is_empty() {
        return;
    }
    let recipe_names: std::collections::BTreeSet<String> =
        recipes.iter().map(|r| r.name.clone()).collect();
    // CS-0240: a probe key resolves at §{xref.resolution} step 3, ahead of the
    // declared variable at step 4, so it shadows a variable exactly as a
    // recipe does. §{xref.env-shadowing} is what CS-0240 chose INSTEAD of a
    // hard error for this one pair, on the ground that a config-block variable
    // has no declaration site a parser could name — and that choice is only
    // honest if the warning it defers to actually fires. So the probe keyset
    // intersects here beside the recipe set.
    let probe_keys: std::collections::BTreeSet<String> =
        probes.probes.keys().map(|k| k.to_string()).collect();
    let mut emitted = builder.shadow_warnings_emitted.borrow_mut();
    for name in recipe_names.intersection(&declared) {
        let key = (name.clone(), name.clone());
        if emitted.insert(key) {
            eprintln!(
                "cook: warning: recipe '{name}' shadows declared var \
                 '{name}': $<{name}> resolves to the recipe (Standard \
                 §5.2.3). Rename one of them."
            );
        }
    }
    for name in probe_keys.intersection(&declared) {
        // Keyed distinctly from the recipe case: a name that is somehow both
        // should report both, not have the second swallowed as a duplicate.
        let key = (format!("probe:{name}"), name.clone());
        if emitted.insert(key) {
            eprintln!(
                "cook: warning: probe '{name}' shadows declared var \
                 '{name}': $<{name}> resolves to the probe (Standard \
                 §5.2.3). Rename one of them, or write $<var.{name}> for \
                 the variable."
            );
        }
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
