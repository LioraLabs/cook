# Design: Unified register-phase and work-unit DAG

**Date:** 2026-05-18
**Status:** Draft — pending implementation plan
**Ticket:** SHI-222
**Standard change ID:** CS-NNNN (assigned at PR time)
**Scope:** `cook-register`, `cook-engine`, `cook-cli`, `cook-luagen`; Cook Standard §13 (two-phase), §22 (register-phase); new conformance fixtures. The Standard change is the source of truth; the implementation work follows lockstep per `CONTRIBUTING.md`.

## 1. Motivation

`cook NAME` only sees recipes the engine learned from the static `cookfile.recipes` AST. When a Cookfile registers recipes through `cook_cc.bin("name", ...)` or any other target-maker that calls `cook.recipe(...)` at register-phase, those names never reach the engine's dispatch path — the build reports `cook: recipe not found: <name>`. The recipe registered fine; the engine never asked.

This is the visible symptom. The architectural root cause is broader:

1. **The register VM is treated as a per-recipe disposable resource.** `Registry::register_recipe(lua_source, name)` is called once per recipe in the wave loop. Each call creates a fresh Lua VM, re-installs the entire `cook.*` API surface, re-loads + re-executes the full `lua_source`, then dispatches one body. For an N-recipe wave, top-level code (including all `cook.recipe(...)` registrations) runs N times. The recipe-table contents from each pass are discarded.
2. **The engine maintains two graphs.** A coarse recipe-level graph (`analyzer::dependency_edges_multi`) feeds a wave-grouping pass (`wave_grouper::compute_waves`); each wave then assembles a per-wave work-unit DAG. Same-wave merging for `{dep}` references is implemented as a synchronization barrier between recipes. Cross-recipe edges live on the recipe-level structure rather than directly on the work-unit graph where they semantically belong.
3. **`cook.recipe` registrations from register-block bodies and top-level module_calls are invisible to dispatch.** Surface `recipe NAME` blocks lower to `cook.recipe(name, meta, body)` calls in codegen. So do target-maker emissions like `cook_cc.bin(name, opts)`. From the VM's perspective these are indistinguishable. The engine reads only the AST list of surface recipes to populate its dispatch table — the Lua-surfaced ones are silently dropped.

Closing the discovery gap alone (acceptance criteria of SHI-222) can be done with a small "enumeration pre-pass" before dispatch. That fix doesn't address the per-recipe re-execution waste or the dual-DAG dichotomy, and it leaves the register VM lifecycle in its current confused shape. This design instead does the architectural cleanup the discovery gap was a symptom of.

## 2. Goals

- `cook NAME` dispatches Lua-registered recipes. Closes SHI-222 acceptance criteria.
- The register VM is a transient register-time tool whose artifact is the unified work-unit DAG.
- One DAG across all reachable recipes. Wave grouping disappears as a structural concept; what survives is a small recipe-level topological sort used as a register-time sequencing helper for `cook.dep_output` resolution.
- `cook list` enumerates Lua-registered names without invoking any recipe body. No probe side-effects from listing.
- Clean break: legacy `Registry::register_recipe` is deleted in the same PR; no shim, no `#[deprecated]`. All call sites in `cook-register/src/tests.rs` and integration tests migrate.
- Standard amendments (§13, §22) and one positive conformance fixture land in the same commit chain that lands the code, per the spec-first rule in `CONTRIBUTING.md`.

## 3. Non-goals

- **Persistent register VM** across CLI invocations or across the execute phase. The VM lives inside a single `register_cookfile` call and is dropped before execute begins.
- **Reordering config-block dispatch relative to register-block top-level code.** Today register-block top-level code reads `cook.env` *before* config-block dispatch mutates it. This is a long-standing footgun, but `register` is an advanced authoring surface and the constraint is not introduced by this change. Out of scope.
- **Output-based inferred-dep matching for Lua-registered recipes.** cook_cc's `links = {...}` model is explicit; `{dep}` placeholders against Lua-registered output paths are unsupported by this change. Static recipes continue to enjoy step-output-derived `serves` lists.
- **Lazy/two-pass `cook.dep_output` resolution.** Cross-recipe `cook.dep_output("other_recipe")` continues to require topological invocation order at register time. The recipe-level topo-sort survives as a register-time sequencing helper for this reason.

## 4. Architecture overview

The Lua register VM is a one-shot tool. Its lifetime spans a single `register_cookfile` call. Its job: load source, dispatch config blocks, invoke every recipe body reachable from the user's target (or all of them, when building the unified DAG), and produce the artifact — the unified work-unit DAG plus the recipe metadata the CLI needs. The VM is dropped before any work unit executes. Execute phase uses the cook-luaotp worker pool exclusively — separate VMs.

```
parse Cookfile                  (cook-lang, AST: recipes, chores, register_blocks,
   ↓                                    top_level_module_calls, config_blocks, uses)
codegen lua_source              (cook-luagen)
   ↓
register_cookfile(lua_source)   (cook-register)
   ├─ lua.load(lua_source).exec()     ← top-level: register_blocks splice, surface
   │                                    recipes lower to cook.__register_surface(...),
   │                                    user/wrapper cook.recipe(...) calls record
   ├─ __cook_run_config_blocks(cfg)   ← config-block dispatch mutates cook.env
   ├─ collision_check                 ← surface ∩ dynamic ≠ ∅ → hard error
   ├─ probe_registry.detect_cycles()  ← §22.5.8
   ├─ topo-sort recipes by `requires` ← register-time sequencing helper
   ├─ for recipe in topo order:
   │     invoke body, collect units   ← bodies see finalized env;
   │                                    cook.dep_output(...) resolves cleanly
   ├─ assemble unified work-unit DAG  ← one DAG across all reachable recipes
   └─ drop Lua VM
   ↓
RegisteredCookfile               (names, dag, probes, cache_managers, final_env)
   ↓
executor.run(registered, entry)  (cook-engine / cook-luaotp worker pool)
   walks reachable subgraph from `entry`'s units; executes
```

Two flavors of register are exposed:

- `register_cookfile(...)` — full pass. Invokes every recipe body, builds the unified DAG. Used by `cook NAME`, `cook dag`, `cook test`.
- `list_names(...)` — cheap pass. Loads source and runs config blocks for env finalization, then reads the recipe table without invoking bodies. Used by `cook list`, `cook menu`. No probe side-effects from body invocation.

## 5. API surface

In `cook-register`:

```rust
pub struct RegisterSessionBuilder { /* same fields as today's Registry */ }

impl RegisterSessionBuilder {
    pub fn new(working_dir: PathBuf, env_vars: HashMap<String, String>) -> Self;
    pub fn with_cli_overrides(mut self, _: HashMap<String, String>) -> Self;
    pub fn with_selected_config(mut self, _: Option<String>) -> Self;
    pub fn with_shared_terminal_outputs(mut self, _: SharedTerminalOutputs) -> Self;
    pub fn with_qualified_prefix(mut self, _: String) -> Self;
    pub fn with_alias_dirs(mut self, _: BTreeMap<String, PathBuf>) -> Self;
    pub fn with_alias_qualified_prefixes(mut self, _: BTreeMap<String, String>) -> Self;
}

pub fn register_cookfile(
    builder: RegisterSessionBuilder,
    lua_source: &str,
    cache_ctx: Option<Arc<CacheContext>>,
) -> Result<RegisteredCookfile, RegisterError>;

pub fn list_names(
    builder: RegisterSessionBuilder,
    lua_source: &str,
) -> Result<Vec<RegisteredRecipe>, RegisterError>;

pub struct RegisteredCookfile {
    pub names:          Vec<RegisteredRecipe>,
    pub dag:            WorkUnitDag,
    pub probes:         BTreeMap<String, ProbeUnit>,
    pub cache_managers: BTreeMap<String, Arc<ThreadSafeCacheManager>>,
    pub final_env:      HashMap<String, String>,
}

pub struct RegisteredRecipe {
    pub name:     String,
    pub source:   RegistrationSource,
    pub kind:     RecipeKind,            // Recipe | Chore
    pub requires: Vec<String>,
}

pub enum RegistrationSource {
    Static  { line: usize },   // emitted by codegen from cookfile.recipes
    Dynamic { line: usize },   // user/wrapper cook.recipe() call site
}

pub enum RegisterError {
    // existing variants...
    RecipeCollision { name: String, sites: Vec<RegistrationSite> },
    DependencyCycle { recipes: Vec<String> },     // surfaces from topo-sort step
}
```

`Registry::register_recipe` is deleted. `Registry` (today's confusingly-named config builder) is renamed `RegisterSessionBuilder`. `register_cook_api_capture` (today's API-installation helper inside cook-register) is renamed `install_cook_api`. `RegistryEntry` (in `cook-engine`) collapses — workspace machinery now holds `RegisteredCookfile` directly per import.

## 6. Internal flow of `register_cookfile`

The Rust-side sequence inside the call, in order:

1. Construct fresh `Lua::unsafe_new()`.
2. Install the full `cook.*` API surface (`install_cook_api`), threading `SessionCaptureState` and a slot for the current `BodyCaptureState` (see §7).
3. Populate `cook.env` from `env_vars` (system env + .env + CLI `--set`).
4. `lua.load(lua_source).exec()` — top-level run. All surface recipes register via `cook.__register_surface(name, meta, body)`; all dynamic registrations from register-block bodies or top-level module_calls register via `cook.recipe(name, meta, body)`. Top-level `cook.probe(...)` calls register their probe units. `__cook_run_config_blocks` is *defined* by codegen here but not yet called.
5. Dispatch config blocks: `__cook_run_config_blocks(selected_config)`. Freeze the env keyset; re-apply CLI overrides on top of any config-block writes; snapshot `cook.env` back into `env_vars`. `final_env` is captured here.
6. **Collision check** (§8). Returns `RecipeCollision` if surface ∩ dynamic ≠ ∅ or if any dynamic name appears twice.
7. `probe_registry.detect_cycles()` per Standard §22.5.8. One pass at session granularity instead of N passes today.
8. Topologically sort the recipe set by declared `requires`. Cycle here → `RegisterError::DependencyCycle`.
9. For each recipe in topological order:
    - Construct a fresh `BodyCaptureState` and swap it into the slot the API closures read.
    - Run `setup_recipe_context` (ingredient resolution).
    - Set `current_recipe = Some(qualified_name)`.
    - `func.call(())` — body executes, populates units / step_groups / dep_edges via the `cook.*` capture closures.
    - Drain the `BodyCaptureState` into a `RecipeUnits` and store it.
    - `current_recipe = None`.
10. Assemble the unified work-unit DAG from all collected `RecipeUnits` (§9).
11. Drop the Lua VM. Return `RegisteredCookfile`.

`list_names` runs steps 1–7 then reads the recipe table (the same one populated by `cook.__register_surface` / `cook.recipe` calls during step 4) and returns the names without performing steps 8–11. Step 7 (probe cycle detection) is preserved because it's cheap and gives consistent diagnostics across `cook list` and `cook NAME`; only body invocation is skipped.

## 7. `CaptureState` split

Today's `CaptureState` is a kitchen-sink struct with per-build and per-body fields mixed. It splits along the lifecycle line:

**`SessionCaptureState`** — constructed once per `register_cookfile`, alive for the whole call:
- `probes: Vec<ProbeUnit>` (populated post-load by draining the probe registry)
- module loader state, export store, env keyset

**`BodyCaptureState`** — constructed fresh per `invoke_body`, dropped after each body returns:
- `units`, `step_groups`, `current_group`, `current_step_outputs`, `last_cook_step_outputs`
- `dep_edges`, `step_group_dep_refs`, `step_group_dep_input_paths`
- `inside_layer`, `layer_commands`
- `current_recipe`, `current_chore_active`

The capture closures installed at session-open close over `(Rc<SessionCaptureState>, Rc<RefCell<Option<BodyCaptureState>>>)`. `register_cookfile` swaps the body slot's `Some(...)` between body calls. Closures that read body state error cleanly if called while the slot is `None` (e.g. attempted use during top-level register-block code is *already* covered today by `inside_layer` defaults — this clarifies the boundary).

This eliminates today's `reset_per_body` pattern: there's nothing to reset because the body state simply didn't exist before the body call.

## 8. Collision detection

Two `cook.recipe`-shaped registrations for the same name within a single Cookfile is an authoring error. The engine MUST diagnose it.

**Mechanism.** Codegen lowers static surface recipes via a private helper:

```lua
-- surface recipes (cookfile.recipes), emitted by cook-luagen
cook.__register_surface("build", {ingredients = {...}, requires = {...}}, function() ... end)
```

Register-block and top-level-module_call bodies splice verbatim. User and wrapper-library code (cook_cc, future cpp modules) keeps calling the public API as documented in Standard §22.3:

```lua
cook.recipe("game", {requires = {...}}, function() ... end)
```

`install_cook_api` registers both functions:
- `cook.__register_surface(name, meta, body)` — tags the registration `Static { line: meta.__line }`. (`__line` is added to the metadata table by codegen.)
- `cook.recipe(name, meta, body)` — tags the registration `Dynamic { line }`, where `line` is determined by walking the Lua call stack to the topmost frame whose source matches the Cookfile path (skipping frames internal to `cook_cc` or other modules).

After the top-level load completes, `register_cookfile` scans the recipe table. Any name with a `Static` entry AND any other entry → `RecipeCollision`. Any name with two `Dynamic` entries → `RecipeCollision`. Two `Static` entries cannot occur for the same name (codegen never emits duplicates from a single AST).

**Diagnostic format**:

```
cook: error: recipe 'build' is registered more than once:
  - Cookfile:7: as a `recipe` block
  - Cookfile:14: by cook.recipe at register-phase
rename one of them.
```

CLI exit code: a new `CookError::RecipeCollision` variant mapped to exit code `3`, matching the existing `CookError::RecipeNotFound` mapping (`cli/crates/cook-cli/src/error.rs:22`). Both are recipe-shape configuration errors and share the user-facing remediation pattern (rename / declare correctly).

## 9. Unified work-unit DAG

Today's two-tier model — recipe-level graph → wave grouping → per-wave work-unit DAGs — collapses to a single work-unit DAG that spans every reachable recipe.

**Construction.** After all bodies have been invoked (step 9 of §6), each recipe has contributed a `RecipeUnits` with its own internal unit graph and cross-recipe dep references. `dag_builder::build_dag` already handles cross-recipe dep wiring within a wave; the change is that its input is now *all* collected `RecipeUnits` at once, not one wave's worth at a time. Cross-recipe edges live directly on the unified DAG.

**Lifecycle events.** `RecipeStarted` / `RecipeCompleted` events were wave-aligned today (emitted at wave boundaries, with synthetic events for zero-work recipes inside a wave). They become unit-driven:

- `BuildStarted` fires once, before the executor starts (recipe topology from the unified DAG, in topo order).
- `RecipeQueued` fires for every recipe whose units appear in the executor's reachable subgraph.
- `RecipeStarted(name)` fires when the first unit owned by `name` transitions out of `Waiting`. Zero-unit recipes (meta-targets that have only deps) fire immediately on entering the reachable subgraph.
- `RecipeCompleted(name)` fires when the last unit owned by `name` finishes.

These triggers are strictly more accurate than today's wave-aligned firing — events now reflect actual work motion rather than wave boundaries.

**Wave grouper.** `cook-engine/src/wave_grouper.rs` is deleted. Its same-wave merging logic (for `{dep}` references) was a synchronization barrier between recipes; the unified DAG dissolves the need — `{dep}` references become ordinary edges between units, and the executor's existing parallelism handles them naturally.

**Recipe-level topo-sort.** Survives as a small helper used at step 8 of §6, only for ordering body invocation so that `cook.dep_output("other_recipe")` calls see populated `SharedTerminalOutputs`. This is a register-time concern, not an execute-time one. It is *not* exposed as an engine structure; it's an internal sequencing aid.

## 10. CLI integration

| Command | Before | After |
|---|---|---|
| `cmd_run` | `build_single_recipe_infos` (AST-only) + `build_single_registries` + `run_with_progress` wave loop | `register_cookfile` returns a `RegisteredCookfile`; `executor.run(registered, target)` walks the reachable DAG subgraph |
| `cmd_list`, `cmd_menu` | Read `cookfile.recipes` + chores from AST | `list_names` returns the full name set including Lua-registered names |
| `cmd_dag` | recipe_infos + topo + emit | `register_cookfile` produces the DAG directly; emit it |
| `cmd_test` | `collect_workspace_recipe_names` reads AST only | `register_cookfile` per Cookfile — its `names` field feeds scope resolution and its `dag` feeds execution (single load, both jobs) |
| `cmd_emit_lua` | Unchanged | Unchanged — prints codegen output; no VM load |

## 11. Workspace imports

Each Cookfile (root + each import) gets its own `register_cookfile` call producing its own `RegisteredCookfile`. The workspace pipeline merges:

- `names` from every import are prefixed with their qualified namespace (existing behavior, unchanged).
- `dag` from every import is merged into one workspace-wide unified DAG. Cross-import dep edges (via `cook.dep_output` resolved through `alias_qualified_prefixes`) wire into the merged DAG at assembly time.
- `probes` and `cache_managers` merge by qualified key (one per qualified recipe name).
- `final_env` per import is independent — imports do not inherit the root's config-block writes (preserves today's behavior).

Error timing improves: a malformed config block or a collision inside `lib/Cookfile` errors at workspace-load time, not when a `lib.something` recipe is first dispatched.

## 12. Standard amendments

Three updates to `standard/src/content/docs/`:

**§22.3 (`cook.recipe`)** — add a normative paragraph:

> Within a single Cookfile, the set of names registered by `cook.recipe` (whether by surface `recipe NAME` declarations or by register-phase Lua code) MUST be unique. A conforming implementation MUST diagnose a duplicate name with a single error naming both registration sites by line. The diagnostic MUST identify the kind of each registration (surface `recipe` block, `register` block, or top-level module call). Rationale annex entry to be added in the same commit chain.

**§22 (new section: recipe discovery)** — add a normative section:

> A conforming implementation's recipe-discovery procedure MUST include both surface recipe declarations and recipes registered by `cook.recipe` during register-phase Lua execution. The `cook NAME` dispatch path MUST resolve `NAME` against the unified set. A `cook NAME` invocation that names a recipe registered only through register-phase Lua MUST dispatch identically to one naming a surface recipe declaration.

**§13 (two-phase model)** — clarify that the work-unit DAG is unified:

> The register phase produces a single work-unit DAG covering every reachable recipe. Cross-recipe dependencies are edges of the unified DAG, not synchronization boundaries between recipe-level partitions. A conforming implementation MAY use any topological ordering of recipes at register time (for example, to resolve `cook.dep_output` references); the choice is not normative.

The §13 amendment makes wave-based partitioning a non-feature. Existing prose that referenced "waves" in informative annexes is removed or rewritten.

## 13. Conformance fixtures

At least one positive fixture under `standard/conformance/positive/`:

- **`recipe-dispatch-from-register-block.cook`** — Cookfile with a `register` block that calls `cook.recipe("hello", ...)` directly. Expected: `cook hello` dispatches and produces the expected output. (Exercises the discovery path without requiring `cook_cc` as a test dependency.)

At least one negative fixture under `standard/conformance/negative/`:

- **`recipe-name-collision-surface-vs-dynamic.cook`** — Cookfile with `recipe build` and a register block calling `cook.recipe("build", ...)`. Expected: hard error from `register_cookfile`, diagnostic matches §8 format, exit code matches the new `RecipeCollision` mapping.

Existing fixtures that exercise multi-recipe workspaces continue to pass without modification. Any fixture today that asserts wave-level event timing must be rewritten to assert unit-driven event timing (§9); these are reviewed in the plan.

## 14. Files touched

```
cli/crates/cook-register/src/engine.rs        # register_cookfile + list_names, drop register_recipe
cli/crates/cook-register/src/capture.rs       # split into SessionCaptureState + BodyCaptureState; cook.__register_surface
cli/crates/cook-register/src/lib.rs           # re-exports
cli/crates/cook-register/src/tests.rs         # migrate ~30 call sites to register_cookfile / list_names
cli/crates/cook-engine/src/registry_entry.rs  # collapsed; RegisteredCookfile is the new transit type
cli/crates/cook-engine/src/pipeline/registries.rs   # build_workspace_registers, merge per-import RegisteredCookfile
cli/crates/cook-engine/src/pipeline/recipe_info.rs  # delete or shrink; recipe_infos no longer the central artifact
cli/crates/cook-engine/src/wave_grouper.rs    # DELETED
cli/crates/cook-engine/src/run.rs             # executor walks unified DAG; lifecycle events become unit-driven
cli/crates/cook-engine/src/dag_builder.rs     # consumes all RecipeUnits at once
cli/crates/cook-cli/src/pipeline.rs           # cmd_run / cmd_list / cmd_test / cmd_menu / cmd_dag migrate
cli/crates/cook-luagen/src/recipe.rs          # emit cook.__register_surface for surface recipes; carry __line
standard/src/content/docs/13-two-phase.mdx    # §13 amendment
standard/src/content/docs/22-register-phase.mdx  # §22.3 collision rule, new discovery section
standard/conformance/positive/                # recipe-dispatch-from-register-block fixture
standard/conformance/negative/                # recipe-name-collision-surface-vs-dynamic fixture
```

Estimated diff: ~1400 LoC including the Standard amendment text and fixtures.

## 15. Risk surface

- **Tests asserting wave structure or wave-aligned events.** Audit `cli/crates/cook-engine/tests/` and the conformance harness for any explicit assertions about wave timing. Rewrite these to assert unit-driven event timing.
- **Cross-recipe `cook.dep_output` ordering.** The recipe-level topo-sort at step 8 of §6 is load-bearing for this. Any cycle in the declared `requires` graph that today would surface as a wave-grouper cycle now surfaces as a `RegisterError::DependencyCycle`. Diagnostic shape changes; conformance fixtures asserting cycle errors may need updating.
- **Per-import config-block independence.** Today's behavior — imports don't see the root's config-block writes — is preserved by giving each import its own `RegisterSessionBuilder`. Tests exercising workspace config layering must continue to pass.
- **Workspace-wide DAG assembly.** Merging per-import DAGs is new code. Watch for alias-prefix resolution edge cases at edge wire time, particularly diamond imports (a Cookfile reachable via two alias chains has one canonical qualified-prefix; the existing `alias_qualified_prefixes` resolution handles this and is preserved).
- **Probe registration timing.** Probes today register N times per wave (idempotent dedup hides the cost). After: register once per session. Probes that today incorrectly relied on multi-fire would surface as bugs; none expected, but worth auditing the probe set in cook_cc's busted tests.
- **`list_names` cost assumption.** The design assumes top-level `cook.probe(...)` calls register probe metadata only (no probe-query execution at registration time); execution is deferred until a body calls into the probe via `cc.checks.has_*` etc. If this assumption is wrong for any probe registered at top level, `cook list` would acquire side effects it doesn't have today. Verify in the plan by auditing `cook-register/src/probe_api.rs` and the probe registration path.

## 16. Acceptance

- [ ] `cook build` in `examples/raylib-game/` dispatches the `game` recipe registered by `cook_cc.bin("game", ...)` at register-phase. End-to-end binary produced.
- [ ] `cook list` in `examples/raylib-game/` prints `raylib` and `game` (the cook_cc-registered names) with no probe side-effects.
- [ ] Existing static-`recipe NAME` blocks continue to dispatch unchanged.
- [ ] Recipe-name collision between a surface block and a Lua-registered name produces the diagnostic in §8.
- [ ] One positive conformance fixture (`recipe-dispatch-from-register-block`) and one negative fixture (`recipe-name-collision-surface-vs-dynamic`) pass.
- [ ] Standard amendments to §13 and §22 land in the same commit chain.
- [ ] All 191 cook_cc busted tests stay green.
- [ ] Conformance harness stays green.
- [ ] Integration tests stay green; any that asserted wave-aligned lifecycle events are rewritten to unit-driven assertions.
- [ ] `Registry::register_recipe` is deleted; no shim, no `#[deprecated]`.
