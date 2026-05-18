# Unified Register-Phase + Work-Unit DAG — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Implement the design at `standard/specs/2026-05-18-unified-register-and-dag-design.md` (committed `189bdbb`). After this plan executes: (1) `cook NAME` dispatches recipes registered by `cook.recipe(...)` at register-phase (closes SHI-222); (2) the engine maintains a single unified work-unit DAG; `wave_grouper` is deleted; lifecycle events are unit-driven; (3) the Lua register VM is a one-shot tool that produces the DAG and is then dropped; (4) `cook list` enumerates Lua-registered recipes via a cheap no-body-invocation path; (5) collisions between surface and dynamic registrations of the same name are diagnosed as hard errors; (6) the Standard ships CS-NNNN with §13 (unified DAG), §22.3 (collision rule), and a new §22 recipe-discovery section, plus one positive and one negative conformance fixture; (7) the `examples/raylib-game/` build dispatches `cook build` end-to-end. Clean break: `Registry::register_recipe` is deleted, `Registry` is renamed `RegisterSessionBuilder`, `RegistryEntry` collapses; no shim, no `#[deprecated]`.

**Architecture:** Eight phases. Phase 1 sets up the worktree and locks in the new types (`RegistrationSource`, split `CaptureState`, `RegisterSessionBuilder` rename) without changing public behavior. Phase 2 adds the new `register_cookfile` and `list_names` entry points alongside the old `register_recipe` (transitional). Phase 3 changes codegen to emit `cook.__register_surface`. Phase 4 delivers the unified work-unit DAG in `cook-engine` and deletes `wave_grouper`. Phase 5 migrates the CLI commands (`cmd_run`, `cmd_list`, `cmd_test`, `cmd_menu`, `cmd_dag`) to the new entry points. Phase 6 is the clean break — delete `Registry::register_recipe`, migrate `cook-register/src/tests.rs`. Phase 7 lands the Standard amendments and conformance fixtures. Phase 8 validates against existing test suites and the raylib-game smoke. TDD where new behavior is introduced (collision detection, list_names); refactor-with-existing-tests-green elsewhere.

**Tech Stack:** Rust (edition 2021, workspace at `cli/`), `cargo test`, `mlua` for Lua 5.4 register-phase VM, MDX (Astro Starlight) for the Standard, Lua Cookfile fixtures under `standard/conformance/` and `examples/raylib-game/`.

---

## Working directory and prerequisites

Create a worktree before starting:

```bash
cd /home/alex/dev/cook
git worktree add ../cook-shi-222 -b matg9192/shi-222-unified-register-and-dag main
cd ../cook-shi-222
```

All paths below are relative to `/home/alex/dev/cook-shi-222` unless noted. The Standard-side spec at `standard/specs/2026-05-18-unified-register-and-dag-design.md` is already committed on `main` (`189bdbb`); that satisfies the spec-first hook for all language-surface changes in this plan.

Confirm the spec-first hook is installed:

```bash
git config --get core.hooksPath
# Expected: .githooks
```

If empty: `git config core.hooksPath .githooks`.

Confirm a clean working tree before starting:

```bash
git status --short
# Expected: empty output
```

## Per-task verification commands

| Scope | Command | Expected |
|---|---|---|
| Lang unit tests | `cd cli && cargo test -p cook-lang` | clean |
| Conformance | `cd cli && cargo test -p cook-lang --test conformance` | clean |
| Luagen unit tests | `cd cli && cargo test -p cook-luagen` | clean |
| Register unit tests | `cd cli && cargo test -p cook-register` | clean |
| Engine unit tests | `cd cli && cargo test -p cook-engine --lib` | clean |
| Engine integration | `cd cli && cargo test -p cook-engine --test '*'` | clean |
| CLI tests | `cd cli && cargo test -p cook-cli` | clean |
| Whole CLI test suite | `cd cli && cargo test` | clean |
| Spec build | `cd standard && pnpm install --frozen-lockfile && pnpm build` | exit 0 |
| raylib-game smoke | `cd examples/raylib-game && cook build` | exit 0; `build/bin/game` exists |
| cook list raylib-game | `cd examples/raylib-game && cook list` | prints `raylib` and `game` |

## File structure

| File | Responsibility | Phase / Tasks |
|---|---|---|
| `cli/crates/cook-register/src/lib.rs` | Re-exports (`RegisterSessionBuilder`, `RegisteredCookfile`, `RegisteredRecipe`, `RegistrationSource`, `register_cookfile`, `list_names`, `RegisterError`). CaptureState type split. | 1.2, 1.3, 2.1 |
| `cli/crates/cook-register/src/capture.rs` | `SessionCaptureState` / `BodyCaptureState` split; tagged `RegisteredRecipe`; `install_cook_api`; `cook.__register_surface` API; `cook.recipe` API with call-site detection | 1.2, 1.3, 1.4, 3.1 |
| `cli/crates/cook-register/src/engine.rs` | `RegisterSessionBuilder` (renamed Registry); `register_cookfile` and `list_names` functions; collision detection; topo-sort for `cook.dep_output` ordering; old `register_recipe` is deleted in Phase 6 | 1.1, 2.1, 2.2, 2.3, 2.4, 6.1 |
| `cli/crates/cook-register/src/tests.rs` | Migrated to `register_cookfile` / `list_names`; new collision unit tests; new list_names unit test | 2.5, 6.2 |
| `cli/crates/cook-engine/src/registered_cookfile.rs` (new) | `RegisteredCookfile` struct definition (moved from cook-register? — actually lives in cook-register; this file holds engine-side adapters and `RegisteredWorkspace` aggregation) | 4.1 |
| `cli/crates/cook-engine/src/registry_entry.rs` | Deleted | 5.5, 6.3 |
| `cli/crates/cook-engine/src/pipeline/registries.rs` | Renamed to `pipeline/registers.rs`; `build_workspace_registers` returns a `RegisteredWorkspace` | 5.1, 5.2 |
| `cli/crates/cook-engine/src/pipeline/recipe_info.rs` | Trimmed: `build_single_recipe_infos` is deleted; `build_workspace_recipe_info` is replaced by `build_recipe_infos_from_registered(workspace)` which reads from the per-import `RegisteredCookfile`s | 5.2, 5.3 |
| `cli/crates/cook-engine/src/wave_grouper.rs` | Deleted | 4.3 |
| `cli/crates/cook-engine/src/run.rs` | Wave loop replaced with a unified-DAG executor walk; lifecycle events become unit-driven | 4.4, 4.5, 4.6 |
| `cli/crates/cook-engine/src/dag_builder.rs` | Accepts all `RecipeUnits` at once; cross-import edges wire here | 4.2 |
| `cli/crates/cook-engine/src/lib.rs` | `pub use cook_register::RegisteredCookfile;` plus the new `RegisteredWorkspace` re-export; `EngineError::DependencyCycle` variant carries cycle names; remove `EngineError::CycleDetected` wave-grouper variant if unreferenced | 4.4, 4.7 |
| `cli/crates/cook-cli/src/error.rs` | New `CookError::RecipeCollision(String)` variant; exit code 3 | 5.6 |
| `cli/crates/cook-cli/src/pipeline.rs` | `cmd_run`, `cmd_list`, `cmd_menu`, `cmd_dag`, `cmd_test` migrate to `register_cookfile` / `list_names`; pipeline-error mapper adds collision case | 5.4, 5.5, 5.6 |
| `cli/crates/cook-luagen/src/recipe.rs` | Codegen emits `cook.__register_surface("name", { …, __line = N, … }, function() … end)` for surface recipes; chores remain `cook.recipe(...)` (no surface block) or get `__register_surface_chore` (see Task 3.1 for chore handling decision) | 3.1, 3.2 |
| `cli/crates/cook-luagen/src/tests.rs` | Codegen tests for the `cook.__register_surface` shape and `__line` propagation | 3.2 |
| `standard/src/content/docs/13-two-phase.mdx` | §13 amendment: unified DAG | 7.1 |
| `standard/src/content/docs/22-register-phase.mdx` | §22.3 collision rule + new recipe-discovery section | 7.2 |
| `standard/src/content/docs/appendix/B-rationale.mdx` (or equivalent) | New rationale annex entry: `recipe-name-uniqueness` | 7.3 |
| `standard/src/content/docs/appendix/D-changes.mdx` (or equivalent) | CS-NNNN entry | 7.3 |
| `standard/conformance/positive/recipe-dispatch-from-register-block.cook` (new) | Positive fixture: register-block calls `cook.recipe("hello", ...)` directly | 7.4 |
| `standard/conformance/positive/recipe-dispatch-from-register-block.expected.txt` (new) | Expected output for the positive fixture | 7.4 |
| `standard/conformance/negative/recipe-name-collision-surface-vs-dynamic.cook` (new) | Negative fixture: `recipe build` + register block calling `cook.recipe("build", ...)` | 7.5 |
| `standard/conformance/negative/recipe-name-collision-surface-vs-dynamic.expected.txt` (new) | Expected diagnostic + exit code | 7.5 |

---

## Phase 1 — Foundation: types and rename (no public-API change)

These tasks restructure internals so subsequent phases can introduce the new API cleanly. Phase 1 keeps `Registry::register_recipe` working and all existing tests green.

### Task 1.1: Rename `Registry` → `RegisterSessionBuilder`

**Goal:** Mechanical rename. `Registry` today is misleadingly named (it's a builder, not a registry). Renaming first means later code reads the way the design specifies.

**Files:**
- Modify: `cli/crates/cook-register/src/engine.rs` (struct and impl)
- Modify: `cli/crates/cook-register/src/lib.rs` (re-export)
- Modify: `cli/crates/cook-register/src/tests.rs` (call sites)
- Modify: `cli/crates/cook-engine/src/registry_entry.rs` (field type)
- Modify: `cli/crates/cook-engine/src/pipeline/registries.rs` (constructor + field usage)
- Modify: `cli/crates/cook-engine/src/run.rs` (any usage)
- Modify: `cli/crates/cook-cli/src/pipeline.rs` (if any direct reference)

- [ ] **Step 1: Rename in `engine.rs`**

In `cli/crates/cook-register/src/engine.rs`, replace every `Registry` with `RegisterSessionBuilder`:

```rust
pub struct RegisterSessionBuilder {
    working_dir: PathBuf,
    // ... existing fields unchanged ...
}

impl RegisterSessionBuilder {
    pub fn new(working_dir: PathBuf, env_vars: HashMap<String, String>) -> Self { /* unchanged */ }
    // ... existing methods unchanged ...
    pub fn register_recipe(&self, ...) { /* unchanged for now */ }
}
```

- [ ] **Step 2: Update `lib.rs` re-export**

In `cli/crates/cook-register/src/lib.rs`, change:

```rust
pub use engine::Registry;
```

to:

```rust
pub use engine::RegisterSessionBuilder;
```

- [ ] **Step 3: Update callers**

Find and replace `cook_register::Registry` → `cook_register::RegisterSessionBuilder` across:
- `cli/crates/cook-engine/src/registry_entry.rs`
- `cli/crates/cook-engine/src/pipeline/registries.rs`
- `cli/crates/cook-engine/src/run.rs`
- `cli/crates/cook-cli/src/pipeline.rs`
- `cli/crates/cook-register/src/tests.rs`

Use:

```bash
rg -l 'cook_register::Registry\b' | xargs sed -i 's/cook_register::Registry\b/cook_register::RegisterSessionBuilder/g'
rg -l '\bRegistry::' cli/crates/cook-register/ cli/crates/cook-engine/ cli/crates/cook-cli/ | xargs sed -i 's/\bRegistry::/RegisterSessionBuilder::/g'
```

Then re-check for any straggling `Registry` references:

```bash
rg '\bRegistry\b' cli/crates/cook-register/ cli/crates/cook-engine/ cli/crates/cook-cli/
```

Expect: only matches inside string literals or comments, not type references. Inspect each.

- [ ] **Step 4: Run all CLI tests**

Run: `cd cli && cargo test`
Expected: PASS (clean rename, no behavior change).

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "refactor(cook-register): rename Registry to RegisterSessionBuilder

Mechanical rename. Registry was misleading — the type is a builder
that produces register-phase state, not a registry of anything. Sets
up subsequent SHI-222 commits that introduce the actual session-style
register entry points.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

### Task 1.2: Split `CaptureState` into `SessionCaptureState` and `BodyCaptureState`

**Goal:** Separate per-build fields (probes, modules, exports) from per-body fields (units, step_groups, dep_edges). Phase 2 needs the body slot to be `Option<BodyCaptureState>` so closures can detect "called outside a body" cleanly.

**Files:**
- Modify: `cli/crates/cook-register/src/lib.rs` (define both structs + type aliases)
- Modify: `cli/crates/cook-register/src/capture.rs` (closures take both halves)
- Modify: `cli/crates/cook-register/src/unit_api.rs` (closures take both halves)
- Modify: `cli/crates/cook-register/src/test_api.rs` (closures take both halves)
- Modify: `cli/crates/cook-register/src/dep_output_api.rs` (closures take both halves)
- Modify: `cli/crates/cook-register/src/probe_api.rs` (drains from session probe registry)
- Modify: `cli/crates/cook-register/src/context.rs` (uses BodyCaptureState)
- Modify: `cli/crates/cook-register/src/engine.rs` (manages both states)

- [ ] **Step 1: Define the new types in `lib.rs`**

Replace the existing `CaptureState` definition with two structs. The split lines are exactly as documented in spec §7.

```rust
/// Session-level capture state. One instance per `register_cookfile` call;
/// shared by all body invocations within that call.
pub struct SessionCaptureState {
    /// Probes drained from the per-session probe registry after top-level load.
    /// Each invoked body receives a clone of this set in its RecipeUnits.probes.
    pub probes: Vec<cook_contracts::ProbeUnit>,
}

impl SessionCaptureState {
    pub fn new() -> Self {
        Self { probes: Vec::new() }
    }
}

impl Default for SessionCaptureState {
    fn default() -> Self {
        Self::new()
    }
}

/// Per-recipe-body capture state. Constructed fresh inside `invoke_body`;
/// drained into a `RecipeUnits` and dropped when the body returns.
pub struct BodyCaptureState {
    pub inside_layer: bool,
    pub layer_commands: Vec<(String, usize)>,
    pub units: Vec<cook_contracts::CapturedUnit>,
    pub current_group: Option<usize>,
    pub step_groups: Vec<Vec<usize>>,
    pub current_step_outputs: Vec<String>,
    pub last_cook_step_outputs: Vec<String>,
    pub dep_edges: Vec<(usize, String)>,
    pub step_group_dep_refs: Vec<String>,
    pub step_group_dep_input_paths: Vec<String>,
    pub current_chore_active: bool,
    pub current_recipe: Option<String>,
}

impl BodyCaptureState {
    pub fn new() -> Self {
        Self {
            inside_layer: false,
            layer_commands: Vec::new(),
            units: Vec::new(),
            current_group: None,
            step_groups: Vec::new(),
            current_step_outputs: Vec::new(),
            last_cook_step_outputs: Vec::new(),
            dep_edges: Vec::new(),
            step_group_dep_refs: Vec::new(),
            step_group_dep_input_paths: Vec::new(),
            current_chore_active: false,
            current_recipe: None,
        }
    }
}

impl Default for BodyCaptureState {
    fn default() -> Self {
        Self::new()
    }
}

pub type SharedSessionCaptureState = Rc<RefCell<SessionCaptureState>>;
pub type SharedBodySlot         = Rc<RefCell<Option<BodyCaptureState>>>;
```

Remove the existing `CaptureState` struct and `SharedCaptureState` type. Keep the `hash_str` function and re-exports unchanged.

- [ ] **Step 2: Update every closure to thread the right state**

In each of `capture.rs`, `unit_api.rs`, `test_api.rs`, `dep_output_api.rs`, `probe_api.rs`, `context.rs`: each function that previously took `capture_state: SharedCaptureState` now takes the relevant halves. The split rule is:

- Closures that read/write probes → `session_state: SharedSessionCaptureState`
- Closures that read/write units / step_groups / dep_edges / layer state / current_recipe / current_chore_active → `body_slot: SharedBodySlot`. Inside the closure, borrow the slot and pattern-match on `Some(body)`; if `None`, return `mlua::Error::runtime("cook.X called outside a recipe body")`.

Concrete example for `cook.exec` in `capture.rs`:

```rust
let body_slot_exec = body_slot.clone();
let exec_fn = lua.create_function(move |_, (cmd, line): (String, usize)| {
    let mut slot = body_slot_exec.borrow_mut();
    let body = slot.as_mut().ok_or_else(|| {
        mlua::Error::runtime("cook.exec called outside a recipe body")
    })?;
    if body.inside_layer {
        body.layer_commands.push((cmd, line));
    } else {
        let unit = CapturedUnit {
            payload: WorkPayload::Shell { cmd: cmd.clone(), line },
            cache_meta: None,
            dep_kind: DepKind::Sequential,
            probes: vec![],
        };
        body.units.push(unit);
    }
    Ok("".to_string())
})?;
```

Apply the same pattern to every API closure across the four files.

- [ ] **Step 3: Update `engine.rs` to construct the split state**

In `Registry::register_recipe` (now `RegisterSessionBuilder::register_recipe`), replace the existing `let capture_state = ...` construction with:

```rust
let session_state: SharedSessionCaptureState = Rc::new(RefCell::new(SessionCaptureState::new()));
let body_slot:    SharedBodySlot           = Rc::new(RefCell::new(None));

// ... install_cook_api(&lua, session_state.clone(), body_slot.clone(), ...)?;

// Before invoking the recipe body:
*body_slot.borrow_mut() = Some(BodyCaptureState::new());

// ... existing setup_recipe_context call, func.call(), etc.

// After the body returns, drain:
let body = body_slot.borrow_mut().take()
    .expect("body slot populated above");

// drain probe_registry into session_state.probes (same as today)
{
    let probe_reg = probe_registry.borrow();
    let mut sess = session_state.borrow_mut();
    for (_key, reg) in &probe_reg.probes {
        sess.probes.push(reg.probe.clone());
    }
}

// Build the RecipeUnits return value from `body` and a clone of `session_state.probes`.
```

Update the existing RecipeUnits construction to pull fields from `body` (the drained BodyCaptureState) and `session_state.probes.clone()` (the per-session probe set, cloned for each recipe).

- [ ] **Step 4: Run cook-register unit tests**

Run: `cd cli && cargo test -p cook-register`
Expected: PASS. Some tests may have been touching `capture_state` fields directly — update those test helpers to use the split states.

- [ ] **Step 5: Run full CLI test suite**

Run: `cd cli && cargo test`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "refactor(cook-register): split CaptureState into Session and Body halves

SessionCaptureState lives for the duration of a register pass and holds
probes. BodyCaptureState is constructed fresh per invoke and drained
into the returned RecipeUnits. Closures now borrow the right half and
return a clean Lua error when called outside a recipe body.

Lays the groundwork for the persistent register VM (and unified DAG)
introduced in subsequent SHI-222 commits.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

### Task 1.3: Rename `register_cook_api_capture` → `install_cook_api`

**Goal:** The function installs the entire register-phase `cook.*` API surface, not just the recipe registration. Rename for accuracy.

**Files:**
- Modify: `cli/crates/cook-register/src/capture.rs` (function definition)
- Modify: `cli/crates/cook-register/src/engine.rs` (call site)

- [ ] **Step 1: Rename the function**

In `cli/crates/cook-register/src/capture.rs`:

```rust
pub fn install_cook_api(
    lua: &Lua,
    env_vars: Rc<RefCell<HashMap<String, String>>>,
    working_dir: &PathBuf,
    session_state: SharedSessionCaptureState,
    body_slot: SharedBodySlot,
    recipe_name: &str,
) -> LuaResult<Rc<RefCell<Vec<RegisteredRecipe>>>> {
    // ... existing body, with the closures updated per Task 1.2 ...
}
```

- [ ] **Step 2: Update the call site in `engine.rs`**

Replace the existing `register_cook_api_capture(...)` call with `install_cook_api(...)`. Pass `session_state.clone()` and `body_slot.clone()` from Task 1.2.

- [ ] **Step 3: Run cook-register unit tests**

Run: `cd cli && cargo test -p cook-register`
Expected: PASS.

- [ ] **Step 4: Commit**

```bash
git add -A
git commit -m "refactor(cook-register): rename register_cook_api_capture to install_cook_api

The function installs the whole register-phase cook.* API surface, not
just capture-mode recipe registration. Rename matches what it does.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

### Task 1.4: Introduce `RegistrationSource` and tagged `RegisteredRecipe`

**Goal:** Each `cook.recipe` and `cook.__register_surface` call records its source kind and line. Required for Phase 2's collision detection.

**Files:**
- Modify: `cli/crates/cook-register/src/capture.rs` (`RegisteredRecipe` gains a `source` field; `cook.recipe` closure records `Dynamic`)
- Modify: `cli/crates/cook-register/src/lib.rs` (re-export `RegistrationSource`)

- [ ] **Step 1: Define `RegistrationSource`**

In `cli/crates/cook-register/src/capture.rs`, just above `RegisteredRecipe`:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegistrationSource {
    /// Emitted by codegen from a surface `recipe NAME` block.
    Static  { line: usize },
    /// Recorded by user / wrapper Lua code calling `cook.recipe(...)`.
    Dynamic { line: usize },
}
```

Extend `RegisteredRecipe`:

```rust
pub struct RegisteredRecipe {
    pub name: String,
    pub function: LuaRegistryKey,
    pub metadata: RegisteredMetadata,
    pub source: RegistrationSource,
}
```

- [ ] **Step 2: Update the `cook.recipe` closure to record `Dynamic`**

In the same file, where `cook.recipe` is defined, walk the Lua call stack to find the topmost frame whose source matches the Cookfile path:

```rust
// Inside the cook.recipe closure, after extracting ingredients/excludes/requires:

let line = caller_line_in_cookfile(lua).unwrap_or(0);

recipes_clone.borrow_mut().push(RegisteredRecipe {
    name,
    function: key,
    metadata: RegisteredMetadata { ingredients, excludes, requires },
    source: RegistrationSource::Dynamic { line },
});
Ok(())
```

Add this helper just below the closures in `capture.rs`:

```rust
/// Walk the Lua call stack and return the line number of the topmost frame
/// whose source string matches the Cookfile path label set by
/// `__cook_cookfile_path`. Returns `0` if the Cookfile frame can't be located.
fn caller_line_in_cookfile(lua: &Lua) -> Option<usize> {
    let target: String = lua
        .named_registry_value::<String>("__cook_cookfile_path")
        .ok()?;

    // Lua call levels: 1 = the closure, 2 = the caller, 3+ = caller's caller, ...
    // Walk outward until we find the Cookfile frame or run out.
    for level in 1..40 {
        match lua.inspect_stack(level) {
            None => return None,
            Some(dbg) => {
                let source = dbg.source().source.unwrap_or("");
                if source == target || source.ends_with(&target) {
                    return Some(dbg.curr_line() as usize);
                }
            }
        }
    }
    None
}
```

Note: `mlua` exposes call-stack inspection via `Lua::inspect_stack(level)`; confirm the exact method signature with `mlua` version pinned in `cli/Cargo.toml` and adjust if needed.

- [ ] **Step 3: Re-export `RegistrationSource` from `lib.rs`**

In `cli/crates/cook-register/src/lib.rs`:

```rust
pub use capture::RegistrationSource;
```

- [ ] **Step 4: Run cook-register tests**

Run: `cd cli && cargo test -p cook-register`
Expected: PASS. Update any test that asserts on the old `RegisteredRecipe` shape — the new `source` field is required.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat(cook-register): tag registered recipes with their source kind and line

Each cook.recipe(...) registration now carries RegistrationSource —
Dynamic for user/wrapper calls (e.g. from cook_cc.bin), Static for
codegen-emitted surface recipes (lands in a later commit). Dynamic
registrations capture the topmost user-code frame's line via the Lua
debug API. Prepares collision detection (SHI-222).

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Phase 2 — New register API: `register_cookfile`, `list_names`, collision detection

The existing `RegisterSessionBuilder::register_recipe` keeps working through Phase 2 (transitional). The new entry points are introduced alongside.

### Task 2.1: Define `RegisteredCookfile` and `RegisteredRecipe` (public-API shape)

**Goal:** Lock down the public types. These are the artifacts the new entry points return; the engine and CLI consume them in Phase 4/5.

**Files:**
- Modify: `cli/crates/cook-register/src/lib.rs` (public types)
- Modify: `cli/crates/cook-register/src/engine.rs` (private placeholder for now)

- [ ] **Step 1: Define the public types in `lib.rs`**

```rust
/// The artifact of a full `register_cookfile` pass.
pub struct RegisteredCookfile {
    pub names:          Vec<RegisteredRecipePub>,
    pub units_by_recipe: std::collections::BTreeMap<String, cook_contracts::RecipeUnits>,
    pub probes:         std::collections::BTreeMap<String, cook_contracts::ProbeUnit>,
    pub final_env:      std::collections::HashMap<String, String>,
}

/// Public summary of one registered recipe. Distinct from the internal
/// `capture::RegisteredRecipe` (which holds a LuaRegistryKey closure).
#[derive(Debug, Clone)]
pub struct RegisteredRecipePub {
    pub name:     String,
    pub source:   RegistrationSource,
    pub kind:     RecipeKind,
    pub requires: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecipeKind {
    Recipe,
    Chore,
}
```

The DAG itself is constructed in cook-engine (Phase 4) by feeding `units_by_recipe` into `dag_builder::build_dag` plus the engine's edge map; the cook-register layer returns the per-recipe units, not the assembled DAG.

- [ ] **Step 2: Add a private placeholder body for `register_cookfile`**

In `cli/crates/cook-register/src/engine.rs`, add a stub that fails fast — Task 2.2 implements it. The compile-time presence is enough for Phase 2 plumbing.

```rust
pub fn register_cookfile(
    _builder: RegisterSessionBuilder,
    _lua_source: &str,
    _cache_ctx: Option<std::sync::Arc<cook_cache::cache_ctx::CacheContext>>,
) -> Result<RegisteredCookfile, RegisterError> {
    unimplemented!("register_cookfile lands in Task 2.2")
}

pub fn list_names(
    _builder: RegisterSessionBuilder,
    _lua_source: &str,
) -> Result<Vec<RegisteredRecipePub>, RegisterError> {
    unimplemented!("list_names lands in Task 2.4")
}
```

Re-export from `lib.rs`:

```rust
pub use engine::{register_cookfile, list_names};
```

- [ ] **Step 3: Confirm it compiles**

Run: `cd cli && cargo check -p cook-register`
Expected: clean. No tests yet; the stubs are unreachable.

- [ ] **Step 4: Commit**

```bash
git add -A
git commit -m "feat(cook-register): introduce RegisteredCookfile / RegisteredRecipePub types

Type-only commit. The new entry points register_cookfile and list_names
are stubbed; subsequent commits flesh them out. RegistryEntry in
cook-engine and the CLI commands continue to use the legacy register_recipe
path for now.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

### Task 2.2: Implement `register_cookfile` (full pass, no collision detection yet)

**Goal:** Land the eleven-step pipeline from spec §6 minus collision detection (Task 2.3) and topo-sort cycle reporting (Task 2.4 covers the new error variant; the topo-sort itself is implemented here using the existing `analyzer::topological_sort`).

**Files:**
- Modify: `cli/crates/cook-register/src/engine.rs` (real implementation)
- Modify: `cli/crates/cook-register/src/lib.rs` (add `RecipeKind` derivation from chores; see Step 2)

- [ ] **Step 1: Write the failing test**

Add to `cli/crates/cook-register/src/tests.rs`:

```rust
#[test]
fn register_cookfile_invokes_each_body_once_in_topo_order() {
    use crate::{register_cookfile, RegisterSessionBuilder};

    // Two recipes; `app` requires `lib`. Both are dynamic registrations.
    let lua_src = r#"
        cook.recipe("lib", {requires = {}}, function()
            cook.exec("touch lib.txt", 1)
        end)
        cook.recipe("app", {requires = {"lib"}}, function()
            cook.exec("touch app.txt", 2)
        end)
    "#;

    let tmpdir = tempfile::TempDir::new().unwrap();
    let builder = RegisterSessionBuilder::new(tmpdir.path().to_path_buf(), Default::default());
    let registered = register_cookfile(builder, lua_src, None).unwrap();

    assert_eq!(registered.names.len(), 2);
    assert_eq!(registered.names[0].name, "lib"); // topo: lib first
    assert_eq!(registered.names[1].name, "app");

    let lib_units = registered.units_by_recipe.get("lib").unwrap();
    let app_units = registered.units_by_recipe.get("app").unwrap();
    assert_eq!(lib_units.units.len(), 1);
    assert_eq!(app_units.units.len(), 1);
}
```

- [ ] **Step 2: Run the test to confirm it fails**

Run: `cd cli && cargo test -p cook-register register_cookfile_invokes_each_body_once_in_topo_order -- --nocapture`
Expected: panic with "register_cookfile lands in Task 2.2" (the `unimplemented!`).

- [ ] **Step 3: Implement `register_cookfile`**

In `cli/crates/cook-register/src/engine.rs`, replace the stub body with the eleven-step pipeline. The existing `register_recipe` body is the template for steps 1–7; the per-recipe loop replaces step 8 onwards.

Key structural points:

1. Construct fresh `Lua::unsafe_new()`.
2. Set `app_data` for `cache_ctx`; set `__cook_cookfile_path` named registry value.
3. Construct `session_state` and `body_slot`.
4. Construct `probe_registry`.
5. Call `install_cook_api(...)` — returns the shared `recipes` Rc.
6. Install `cook.require_env`, `cook.probe`, `fs.*`, `path.*`, `cook.platform.*`, `cook.dep_output`, `cook.add_unit`, `cook.add_test`, etc. — copy verbatim from current `register_recipe`.
7. `lua.load(lua_source).exec()?;`
8. Dispatch `__cook_run_config_blocks` if present; freeze env keyset; re-apply CLI overrides; snapshot `cook.env` back into `env_vars`. **Capture the final env into `final_env`.**
9. (Task 2.3 inserts collision detection here.)
10. `probe_registry.borrow().detect_cycles()?` — once per session.
11. Drain `probe_registry` into `session_state.probes`.
12. Construct the recipes view: read `recipes.borrow()` and build a `BTreeMap<String, RegistrationSource>` for downstream collision use.
13. Topologically sort recipe names by their `metadata.requires`:

```rust
use std::collections::BTreeMap;
let names_to_requires: BTreeMap<String, Vec<String>> = recipes.borrow().iter()
    .map(|r| (r.name.clone(), r.metadata.requires.clone()))
    .collect();
// Reuse cook-engine's topological_sort? No — that's an engine concern.
// Do it inline here with a small DFS.
let topo = local_topological_sort(&names_to_requires)?; // returns Vec<String> or RegisterError::DependencyCycle
```

Add `local_topological_sort` as a private function in `engine.rs`:

```rust
fn local_topological_sort(
    deps: &std::collections::BTreeMap<String, Vec<String>>,
) -> Result<Vec<String>, RegisterError> {
    #[derive(Clone, Copy, PartialEq)]
    enum State { Unvisited, Visiting, Visited }
    let mut state: std::collections::BTreeMap<&str, State> = deps.keys()
        .map(|k| (k.as_str(), State::Unvisited))
        .collect();
    let mut order = Vec::new();
    let mut stack_for_cycle = Vec::new();
    fn visit<'a>(
        node: &'a str,
        deps: &'a std::collections::BTreeMap<String, Vec<String>>,
        state: &mut std::collections::BTreeMap<&'a str, State>,
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
                // Skip references the local set doesn't know about — those are
                // cross-recipe `requires` to dependencies registered elsewhere
                // (e.g. workspace imports). The engine's cross-cookfile dep
                // analyzer will reject genuinely unknown names later.
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
        visit(name, deps, &mut state, &mut order, &mut stack_for_cycle)?;
    }
    Ok(order)
}
```

14. For each recipe name in `topo` order:
    - `*body_slot.borrow_mut() = Some(BodyCaptureState::new());`
    - Look up the recipe in `recipes.borrow()`.
    - Call `setup_recipe_context(&lua, recipe, &builder.working_dir)?`.
    - Set `body.current_recipe = Some(qualified_name)`.
    - `func.call::<()>(())?;`
    - Drain the body slot into a `RecipeUnits` (same field mapping that `register_recipe` does today). Store in `units_by_recipe.insert(name, units)`.
15. After the loop, build the public `names: Vec<RegisteredRecipePub>` (one entry per recipe in topo order; `kind` is hard-coded to `RecipeKind::Recipe` here — chores are addressed in Task 3.1 where codegen distinguishes them).
16. Build the `probes` `BTreeMap<String, ProbeUnit>` by cloning from `session_state.probes`.
17. Return `Ok(RegisteredCookfile { names, units_by_recipe, probes, final_env })`.

(Note: `kind` defaults to `Recipe`. Chore-vs-recipe tagging requires the codegen change in Task 3.1, where surface chores emit `cook.__register_surface_chore` and dynamic recipes are always `Recipe`. Until then, `RecipeKind::Recipe` for everything is correct because no caller distinguishes yet.)

- [ ] **Step 4: Run the test to confirm it passes**

Run: `cd cli && cargo test -p cook-register register_cookfile_invokes_each_body_once_in_topo_order`
Expected: PASS.

- [ ] **Step 5: Run cook-register full unit tests**

Run: `cd cli && cargo test -p cook-register`
Expected: PASS. (Existing tests use `register_recipe`, which is untouched.)

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "feat(cook-register): implement register_cookfile

One-shot register pass. Loads source, dispatches config blocks, freezes
env, runs probe cycle detection once, topologically sorts recipes by
declared requires, invokes each body in turn, and returns RegisteredCookfile
with units_by_recipe + final_env + probes.

DependencyCycle error variant added; collision detection follows in the
next commit. Existing register_recipe path is untouched.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

### Task 2.3: Add collision detection

**Goal:** A name registered twice — surface vs dynamic, or dynamic vs dynamic — is a hard error from `register_cookfile`. Per spec §8.

**Files:**
- Modify: `cli/crates/cook-register/src/engine.rs` (collision check function)
- Modify: `cli/crates/cook-register/src/lib.rs` (`RegisterError::RecipeCollision` variant)

- [ ] **Step 1: Write a failing test**

In `cli/crates/cook-register/src/tests.rs`, add:

```rust
#[test]
fn register_cookfile_rejects_duplicate_dynamic_registration() {
    use crate::{register_cookfile, RegisterError, RegisterSessionBuilder};

    let lua_src = r#"
        cook.recipe("build", {requires = {}}, function() end)
        cook.recipe("build", {requires = {}}, function() end)
    "#;
    let tmpdir = tempfile::TempDir::new().unwrap();
    let builder = RegisterSessionBuilder::new(tmpdir.path().to_path_buf(), Default::default());
    let err = register_cookfile(builder, lua_src, None).unwrap_err();

    match err {
        RegisterError::RecipeCollision { name, sites } => {
            assert_eq!(name, "build");
            assert_eq!(sites.len(), 2);
        }
        other => panic!("expected RecipeCollision, got {other:?}"),
    }
}
```

A second test (surface-vs-dynamic) lands after codegen emits `cook.__register_surface` in Phase 3 — for now the synthetic test above is enough to drive the collision-detection logic, since both registrations are tagged `Dynamic`.

- [ ] **Step 2: Run to confirm it fails**

Run: `cd cli && cargo test -p cook-register register_cookfile_rejects_duplicate_dynamic_registration`
Expected: FAIL (no collision check yet; the test gets back an `Ok` or a different error).

- [ ] **Step 3: Add `RegisterError::RecipeCollision`**

In `cli/crates/cook-register/src/lib.rs`:

```rust
#[derive(Debug, Clone)]
pub struct RegistrationSite {
    pub line: usize,
    pub kind: RegistrationSiteKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegistrationSiteKind {
    /// Surface `recipe NAME` block (codegen-emitted).
    SurfaceRecipe,
    /// Surface `chore NAME` block (codegen-emitted).
    SurfaceChore,
    /// `cook.recipe(...)` call from a `register` block, top-level module call,
    /// or wrapper Lua function (e.g. cook_cc.bin).
    Dynamic,
}

// In RegisterError, add:
#[error("recipe '{name}' is registered more than once")]
RecipeCollision { name: String, sites: Vec<RegistrationSite> },

#[error("dependency cycle: {recipes:?}")]
DependencyCycle { recipes: Vec<String> },
```

- [ ] **Step 4: Implement the collision check**

In `cli/crates/cook-register/src/engine.rs`, add a private helper after `local_topological_sort`:

```rust
fn detect_collisions(
    recipes: &[crate::capture::RegisteredRecipe],
) -> Result<(), RegisterError> {
    use std::collections::BTreeMap;
    let mut by_name: BTreeMap<&str, Vec<RegistrationSite>> = BTreeMap::new();
    for r in recipes {
        let site = match r.source {
            crate::capture::RegistrationSource::Static { line } => RegistrationSite {
                line, kind: RegistrationSiteKind::SurfaceRecipe,
            },
            crate::capture::RegistrationSource::Dynamic { line } => RegistrationSite {
                line, kind: RegistrationSiteKind::Dynamic,
            },
        };
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
```

Wire it into `register_cookfile` immediately after `lua.load(lua_source).exec()?` and `config block dispatch`, before topo-sort:

```rust
// (after config-block dispatch + env freeze + cli overrides re-apply)
detect_collisions(&recipes.borrow())?;
// (then continue to probe cycle detection + topo sort)
```

Also apply the same check in `list_names` once it's implemented in Task 2.4.

- [ ] **Step 5: Run the test to confirm it passes**

Run: `cd cli && cargo test -p cook-register register_cookfile_rejects_duplicate_dynamic_registration`
Expected: PASS.

- [ ] **Step 6: Run cook-register full unit tests**

Run: `cd cli && cargo test -p cook-register`
Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add -A
git commit -m "feat(cook-register): hard-error on duplicate recipe registration

register_cookfile now diagnoses a name registered more than once with
RegisterError::RecipeCollision, naming both registration sites by line
and kind (SurfaceRecipe, SurfaceChore, Dynamic). Closes part of
SHI-222 acceptance.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

### Task 2.4: Implement `list_names`

**Goal:** Cheap no-body-invocation path for `cook list` / `cook menu`. Returns name + source + requires; does not produce a DAG or invoke any body.

**Files:**
- Modify: `cli/crates/cook-register/src/engine.rs` (real implementation)
- Modify: `cli/crates/cook-register/src/tests.rs` (test)

- [ ] **Step 1: Write a failing test**

```rust
#[test]
fn list_names_returns_registrations_without_invoking_bodies() {
    use crate::{list_names, RegisterSessionBuilder};
    use std::sync::Mutex;
    use std::sync::Arc;

    // A counter that would increment if the body ran. Increments here are
    // observable from Rust via a side-channel — we use cook.exec to attempt
    // a write to a file in capture mode; if the body invoked, the body
    // capture state would record a unit (which list_names never builds).
    let lua_src = r#"
        cook.recipe("a", {requires = {}}, function()
            error("body must not run during list_names")
        end)
        cook.recipe("b", {requires = {"a"}}, function() end)
    "#;
    let tmpdir = tempfile::TempDir::new().unwrap();
    let builder = RegisterSessionBuilder::new(tmpdir.path().to_path_buf(), Default::default());
    let names = list_names(builder, lua_src).unwrap();
    let by_name: std::collections::BTreeMap<_, _> = names.iter()
        .map(|r| (r.name.clone(), r))
        .collect();
    assert!(by_name.contains_key("a"));
    assert!(by_name.contains_key("b"));
    assert_eq!(by_name["b"].requires, vec!["a".to_string()]);
}
```

- [ ] **Step 2: Run to confirm it fails**

Run: `cd cli && cargo test -p cook-register list_names_returns_registrations_without_invoking_bodies`
Expected: panic from `unimplemented!`.

- [ ] **Step 3: Implement `list_names`**

In `cli/crates/cook-register/src/engine.rs`:

```rust
pub fn list_names(
    builder: RegisterSessionBuilder,
    lua_source: &str,
) -> Result<Vec<RegisteredRecipePub>, RegisterError> {
    let lua = unsafe { Lua::unsafe_new() };
    let session_state: SharedSessionCaptureState = Rc::new(RefCell::new(SessionCaptureState::new()));
    let body_slot:    SharedBodySlot           = Rc::new(RefCell::new(None));
    let probe_registry = Rc::new(RefCell::new(ProbeRegistry::default()));

    let recipes = install_cook_api(
        &lua,
        builder.env_vars.clone(),
        &builder.working_dir,
        session_state.clone(),
        body_slot.clone(),
        "", // recipe_name unused for list-only mode
    )?;
    install_remaining_apis(
        &lua,
        &builder,
        session_state.clone(),
        body_slot.clone(),
        probe_registry.clone(),
        None, // no cache_ctx for list-only
    )?;

    lua.load(lua_source).exec()?;
    dispatch_config_blocks(&lua, &builder)?; // factored helper; see Task 2.2 refactor

    detect_collisions(&recipes.borrow())?;
    probe_registry.borrow().detect_cycles().map_err(|m| RegisterError::Lua(mlua::Error::runtime(m)))?;

    let out: Vec<RegisteredRecipePub> = recipes.borrow().iter()
        .map(|r| RegisteredRecipePub {
            name:     r.name.clone(),
            source:   r.source,
            kind:     RecipeKind::Recipe,  // codegen Task 3.1 introduces RecipeKind::Chore
            requires: r.metadata.requires.clone(),
        })
        .collect();
    Ok(out)
}
```

`install_remaining_apis` and `dispatch_config_blocks` should be extracted as small private helpers in `engine.rs` so `register_cookfile` and `list_names` can share them. Refactor `register_cookfile` to call those helpers as part of this task.

- [ ] **Step 4: Run the test to confirm it passes**

Run: `cd cli && cargo test -p cook-register list_names_returns_registrations_without_invoking_bodies`
Expected: PASS.

- [ ] **Step 5: Run full cook-register tests**

Run: `cd cli && cargo test -p cook-register`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "feat(cook-register): add list_names — cheap no-body register pass

list_names loads source, runs config blocks, detects collisions, and
returns the registered name set with requires metadata. No recipe
body is invoked; no probe queries fire. Used by cook list / cook menu
once the CLI migration lands in Phase 5.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

### Task 2.5: Add a positive test exercising a dynamic registration via a wrapper-like closure

**Goal:** Pin the behavior the cpp roadmap depends on: a Lua module exposes a `mod.bin(name, ...)` helper that internally calls `cook.recipe(name, ...)`, and `register_cookfile` surfaces the registered name with `Dynamic` source. Smoke-tests the call-stack walk in `caller_line_in_cookfile`.

**Files:**
- Modify: `cli/crates/cook-register/src/tests.rs`

- [ ] **Step 1: Write the test**

```rust
#[test]
fn register_cookfile_surfaces_wrapper_registered_recipe() {
    use crate::{register_cookfile, RegisterSessionBuilder, RegistrationSource};

    // A wrapper module that internally calls cook.recipe. Inline so the
    // test is self-contained.
    let lua_src = r#"
        local mod = {}
        function mod.bin(name)
            cook.recipe(name, {requires = {}}, function()
                cook.exec("touch " .. name, 0)
            end)
        end

        mod.bin("game")
    "#;

    let tmpdir = tempfile::TempDir::new().unwrap();
    let builder = RegisterSessionBuilder::new(tmpdir.path().to_path_buf(), Default::default());
    let registered = register_cookfile(builder, lua_src, None).unwrap();

    assert_eq!(registered.names.len(), 1);
    assert_eq!(registered.names[0].name, "game");
    match registered.names[0].source {
        RegistrationSource::Dynamic { line } => {
            // Line should point at the `mod.bin("game")` call site, not at the
            // `cook.recipe(...)` line inside mod.bin. Don't pin the exact number
            // (depends on how Lua maps stack frames here) — just assert it's
            // greater than zero and within the source range.
            assert!(line > 0 && line <= 8, "line was {line}");
        }
        other => panic!("expected Dynamic, got {other:?}"),
    }
    assert!(registered.units_by_recipe.contains_key("game"));
}
```

- [ ] **Step 2: Run the test**

Run: `cd cli && cargo test -p cook-register register_cookfile_surfaces_wrapper_registered_recipe`
Expected: PASS. If the line-resolution doesn't yet identify the call site (Task 1.4's `caller_line_in_cookfile`), it may still PASS if line is `0`, but the bounded-range assertion forces correctness. Adjust the helper if it fails — the resolution must skip frames whose source matches the module file, not the Cookfile.

- [ ] **Step 3: Commit**

```bash
git add -A
git commit -m "test(cook-register): pin wrapper-registered recipe dispatch via register_cookfile

Mirrors cook_cc.bin's registration pattern: a Lua function that wraps
cook.recipe and is called from a Cookfile top level. register_cookfile
must surface the registered name and tag it Dynamic with the call site's
line, not the module's line.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Phase 3 — Codegen: emit `cook.__register_surface`

### Task 3.1: Codegen surface recipes via `cook.__register_surface`; chores stay distinct

**Goal:** Surface `recipe NAME` and `chore NAME` blocks lower to a tagged registration so the collision check (Task 2.3) sees the right source kind. Chores get a distinct codegen helper (`cook.__register_surface_chore`) so the engine can tag `RecipeKind::Chore` correctly.

**Files:**
- Modify: `cli/crates/cook-luagen/src/recipe.rs` (codegen emit)
- Modify: `cli/crates/cook-register/src/capture.rs` (install `cook.__register_surface` and `cook.__register_surface_chore`)
- Modify: `cli/crates/cook-luagen/src/tests.rs` (codegen tests)

- [ ] **Step 1: Write a failing codegen test**

In `cli/crates/cook-luagen/src/tests.rs`:

```rust
#[test]
fn codegen_emits_register_surface_for_surface_recipes() {
    let cookfile = Cookfile {
        recipes: vec![Recipe {
            name: "build".to_string(),
            deps: vec![],
            ingredients: vec![],
            excludes: vec![],
            steps: vec![],
            line: 5,
        }],
        ..Default::default()
    };
    let lua = codegen(&cookfile);
    assert!(lua.contains(r#"cook.__register_surface("build""#));
    assert!(lua.contains(r#"__line = 5"#));
}
```

- [ ] **Step 2: Run to confirm it fails**

Run: `cd cli && cargo test -p cook-luagen codegen_emits_register_surface_for_surface_recipes`
Expected: FAIL — codegen still emits `cook.recipe(...)`.

- [ ] **Step 3: Update codegen**

In `cli/crates/cook-luagen/src/recipe.rs` around line 728, replace the `TopLevelItem::Recipe` arm's emission:

```rust
TopLevelItem::Recipe(recipe) => {
    out.push_str(&format!(
        "cook.__register_surface(\"{}\", {}, function()\n",
        escape_lua_string(&recipe.name),
        generate_metadata_with_line(recipe)
    ));
    // ... existing body emission unchanged ...
}
TopLevelItem::Chore(chore) => {
    out.push_str(&format!(
        "cook.__register_surface_chore(\"{}\", {}, function()\n",
        escape_lua_string(&chore.name),
        generate_chore_metadata_with_line(chore)
    ));
    // ... existing body emission unchanged ...
}
```

`generate_metadata_with_line` extends the existing `generate_metadata` helper to add `__line = N` as a metadata field. Example output:

```lua
{ingredients = {}, excludes = {}, requires = {"raylib"}, __line = 5}
```

- [ ] **Step 4: Install the two codegen-private APIs in `cook-register`**

In `cli/crates/cook-register/src/capture.rs`, alongside the existing `cook.recipe` registration in `install_cook_api`:

```rust
// cook.__register_surface(name, meta, body) — codegen-private API.
// Tags the registration Static using meta.__line.
let recipes_surface = recipes.clone();
let surface_fn = lua.create_function(move |_lua, (name, meta, func): (String, LuaTable, LuaFunction)| {
    let key = _lua.create_registry_value(func)?;
    let line: usize = meta.get("__line").unwrap_or(0);
    let (ingredients, excludes, requires) = parse_meta_lists(&meta)?;
    recipes_surface.borrow_mut().push(RegisteredRecipe {
        name,
        function: key,
        metadata: RegisteredMetadata { ingredients, excludes, requires },
        source: RegistrationSource::Static { line },
        kind: RecipeKind::Recipe,
    });
    Ok(())
})?;
cook.set("__register_surface", surface_fn)?;

// cook.__register_surface_chore(name, meta, body) — same shape, chore-tagged.
let recipes_chore = recipes.clone();
let chore_fn = lua.create_function(move |_lua, (name, meta, func): (String, LuaTable, LuaFunction)| {
    let key = _lua.create_registry_value(func)?;
    let line: usize = meta.get("__line").unwrap_or(0);
    let (ingredients, excludes, requires) = parse_meta_lists(&meta)?;
    recipes_chore.borrow_mut().push(RegisteredRecipe {
        name,
        function: key,
        metadata: RegisteredMetadata { ingredients, excludes, requires },
        source: RegistrationSource::Static { line },
        kind: RecipeKind::Chore,
    });
    Ok(())
})?;
cook.set("__register_surface_chore", chore_fn)?;
```

This requires extending `RegisteredRecipe` to also carry a `kind: RecipeKind` field. Update the `cook.recipe` closure to set `kind: RecipeKind::Recipe`.

Update `RegisteredRecipePub` construction in `register_cookfile` and `list_names` to copy `r.kind` instead of hardcoding `RecipeKind::Recipe`.

Add `parse_meta_lists`:

```rust
fn parse_meta_lists(meta: &LuaTable) -> LuaResult<(Vec<String>, Vec<String>, Vec<String>)> {
    let mut ingredients = Vec::new();
    if let Ok(t) = meta.get::<LuaTable>("ingredients") {
        for pair in t.sequence_values::<String>() { if let Ok(s) = pair { ingredients.push(s) } }
    }
    let mut excludes = Vec::new();
    if let Ok(t) = meta.get::<LuaTable>("excludes") {
        for pair in t.sequence_values::<String>() { if let Ok(s) = pair { excludes.push(s) } }
    }
    let mut requires = Vec::new();
    if let Ok(t) = meta.get::<LuaTable>("requires") {
        for pair in t.sequence_values::<String>() { if let Ok(s) = pair { requires.push(s) } }
    }
    Ok((ingredients, excludes, requires))
}
```

- [ ] **Step 5: Run codegen test**

Run: `cd cli && cargo test -p cook-luagen codegen_emits_register_surface_for_surface_recipes`
Expected: PASS.

- [ ] **Step 6: Run full cook-luagen tests**

Run: `cd cli && cargo test -p cook-luagen`
Expected: PASS. Existing tests that look for `cook.recipe(...)` in codegen output for surface recipes will need updating — fix them to assert `cook.__register_surface(...)`. Tests for register-block bodies (which still call `cook.recipe`) stay as-is.

- [ ] **Step 7: Run full cook-register tests**

Run: `cd cli && cargo test -p cook-register`
Expected: PASS.

- [ ] **Step 8: Commit**

```bash
git add -A
git commit -m "feat(cook-luagen, cook-register): split surface vs dynamic recipe registration

Surface recipes (cookfile.recipes / cookfile.chores) now lower to
codegen-private cook.__register_surface / cook.__register_surface_chore
with a __line field. Dynamic registrations (register blocks, top-level
module calls, wrapper Lua functions) keep using the public cook.recipe
API per Standard §22.3.

This is the mechanism that lets the engine diagnose surface-vs-dynamic
collisions cleanly.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

### Task 3.2: Pin chore + recipe-with-deps codegen shapes

**Goal:** Codegen tests for the new shape, including chores and recipes with declared `requires`.

**Files:**
- Modify: `cli/crates/cook-luagen/src/tests.rs`

- [ ] **Step 1: Write the tests**

```rust
#[test]
fn codegen_chore_uses_register_surface_chore() {
    let cookfile = Cookfile {
        chores: vec![Chore {
            name: "clean".to_string(),
            deps: vec![],
            steps: vec![],
            line: 8,
        }],
        ..Default::default()
    };
    let lua = codegen(&cookfile);
    assert!(lua.contains(r#"cook.__register_surface_chore("clean""#));
    assert!(lua.contains(r#"__line = 8"#));
}

#[test]
fn codegen_register_surface_includes_requires() {
    let cookfile = Cookfile {
        recipes: vec![Recipe {
            name: "app".to_string(),
            deps: vec!["lib".to_string()],
            ingredients: vec![],
            excludes: vec![],
            steps: vec![],
            line: 10,
        }],
        ..Default::default()
    };
    let lua = codegen(&cookfile);
    assert!(lua.contains(r#"cook.__register_surface("app""#));
    assert!(lua.contains(r#"requires = {"lib"}"#));
    assert!(lua.contains(r#"__line = 10"#));
}
```

- [ ] **Step 2: Run the tests**

Run: `cd cli && cargo test -p cook-luagen codegen_chore_uses_register_surface_chore codegen_register_surface_includes_requires`
Expected: PASS.

- [ ] **Step 3: Run full cook-luagen tests**

Run: `cd cli && cargo test -p cook-luagen`
Expected: PASS.

- [ ] **Step 4: Now add a collision test exercising surface-vs-dynamic**

In `cli/crates/cook-register/src/tests.rs`:

```rust
#[test]
fn register_cookfile_rejects_surface_vs_dynamic_collision() {
    use crate::{register_cookfile, RegisterError, RegisterSessionBuilder};

    // Simulate codegen output directly: surface recipe + register block.
    let lua_src = r#"
        cook.__register_surface("build", {ingredients = {}, excludes = {}, requires = {}, __line = 3}, function() end)
        cook.recipe("build", {requires = {}}, function() end)
    "#;
    let tmpdir = tempfile::TempDir::new().unwrap();
    let builder = RegisterSessionBuilder::new(tmpdir.path().to_path_buf(), Default::default());
    let err = register_cookfile(builder, lua_src, None).unwrap_err();
    match err {
        RegisterError::RecipeCollision { name, sites } => {
            assert_eq!(name, "build");
            assert_eq!(sites.len(), 2);
            assert!(sites.iter().any(|s| matches!(s.kind, crate::RegistrationSiteKind::SurfaceRecipe)));
            assert!(sites.iter().any(|s| matches!(s.kind, crate::RegistrationSiteKind::Dynamic)));
        }
        other => panic!("expected RecipeCollision, got {other:?}"),
    }
}
```

- [ ] **Step 5: Run the new test**

Run: `cd cli && cargo test -p cook-register register_cookfile_rejects_surface_vs_dynamic_collision`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "test(cook-luagen, cook-register): pin surface-vs-dynamic codegen and collision shapes

Codegen tests confirm chores and requires-bearing recipes emit
__register_surface[_chore] with __line. Register test confirms the
surface-vs-dynamic collision path produces a diagnostic that names
both sites with the correct kind labels.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Phase 4 — Engine: unified work-unit DAG

The engine refactor lands after the new register API exists and codegen is up-to-date. Phase 4 introduces the unified DAG, deletes `wave_grouper`, and rewires `run.rs`.

### Task 4.1: Define `RegisteredWorkspace` in `cook-engine`

**Goal:** A workspace-level container that aggregates per-import `RegisteredCookfile`s. The CLI calls `register_workspace` (Phase 5) and gets back a `RegisteredWorkspace`, which the executor walks.

**Files:**
- Create: `cli/crates/cook-engine/src/registered_workspace.rs`
- Modify: `cli/crates/cook-engine/src/lib.rs` (module declaration + re-export)

- [ ] **Step 1: Create the file**

```rust
//! Workspace-wide aggregation of per-Cookfile RegisteredCookfile.
//!
//! Each Cookfile (root + each import) is registered independently by
//! cook-register::register_cookfile. The aggregation merges names
//! (with qualified prefix), units (keyed by qualified recipe name),
//! probes, and final_env into a single workspace-wide view that the
//! engine's DAG builder and executor consume.

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;

use cook_contracts::RecipeUnits;
use cook_register::{RegisteredCookfile, RegisteredRecipePub};

pub struct RegisteredWorkspace {
    /// All recipes across all Cookfiles, names qualified with their import prefix.
    pub names:           Vec<RegisteredRecipePub>,
    /// Per-recipe captured units, keyed by fully-qualified recipe name.
    pub units_by_recipe: BTreeMap<String, RecipeUnits>,
    /// Probes keyed by qualified probe key.
    pub probes:          BTreeMap<String, cook_contracts::ProbeUnit>,
    /// Per-Cookfile final env. Imports don't inherit the root's config writes.
    pub final_env_by_cookfile: BTreeMap<String, HashMap<String, String>>,
    /// Per-Cookfile working directory, keyed by qualified prefix ("" for root).
    pub working_dir_by_prefix: BTreeMap<String, PathBuf>,
    /// Per-recipe alias_dirs from the workspace (for cook.dep_output rewriting).
    pub alias_dirs_by_prefix:  BTreeMap<String, BTreeMap<String, PathBuf>>,
}
```

- [ ] **Step 2: Register the module**

In `cli/crates/cook-engine/src/lib.rs`:

```rust
pub mod registered_workspace;
pub use registered_workspace::RegisteredWorkspace;
pub use cook_register::{RegisteredCookfile, RegisteredRecipePub, RecipeKind};
```

- [ ] **Step 3: Compile-check**

Run: `cd cli && cargo check -p cook-engine`
Expected: clean.

- [ ] **Step 4: Commit**

```bash
git add -A
git commit -m "feat(cook-engine): add RegisteredWorkspace aggregation type

Workspace-level container that merges per-import RegisteredCookfile
into a single view the executor can walk. No behavior change yet —
the wiring happens in subsequent commits.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

### Task 4.2: Adapt `dag_builder` to accept all units at once

**Goal:** Today's `build_dag` accepts one wave's worth of `Vec<RecipeUnits>`. The unified-DAG model passes every reachable recipe's units in a single call. The function signature already takes a `Vec`, so the change is at the call-site: stop calling per-wave and start calling once. This task pre-validates that `build_dag` handles cross-recipe edges correctly across the full set.

**Files:**
- Modify: `cli/crates/cook-engine/src/dag_builder.rs` (audit + minor improvements if necessary; likely no change beyond a docstring update)
- Modify: `cli/crates/cook-engine/tests/...` (add a new integration test that builds a DAG from N recipes with cross-recipe edges spanning what used to be separate waves)

- [ ] **Step 1: Audit `build_dag`**

Read `cli/crates/cook-engine/src/dag_builder.rs` end-to-end. Identify any wave-coupling assumptions (e.g., assumptions that cross-recipe dep names always resolve to other units in the same `Vec<RecipeUnits>` argument). If found, lift them — the unified path passes every reachable recipe in one call, so cross-recipe edges become intra-call. No assumption should require waves.

If no wave coupling exists, this is a no-op step; proceed to step 2.

- [ ] **Step 2: Write an integration test**

Create `cli/crates/cook-engine/tests/unified_dag_build.rs`:

```rust
//! Smoke test: build_dag handles cross-recipe edges in a single call
//! that spans every recipe in the workspace. The unified-DAG model
//! relies on this property; today the wave loop hid it because each
//! call saw only one wave.

use cook_contracts::{CapturedUnit, DepKind, RecipeUnits, WorkPayload};
use cook_engine::dag_builder::build_dag;

fn unit(out: &str, recipe: &str) -> RecipeUnits {
    RecipeUnits {
        recipe_name: recipe.to_string(),
        units: vec![CapturedUnit {
            payload: WorkPayload::Shell { cmd: format!("touch {out}"), line: 1 },
            cache_meta: None,
            dep_kind: DepKind::Sequential,
            probes: vec![],
        }],
        deps: vec![],
        probes: vec![],
        step_groups: vec![],
        test_results: vec![],
        terminal_outputs: vec![out.to_string()],
        dep_edges: vec![],
    }
}

#[test]
fn dag_builder_assembles_cross_recipe_edges_across_full_set() {
    let mut a = unit("lib.a", "lib");
    let mut b = unit("app.b", "app");
    // app depends on lib via deps field — simulating a static `requires lib` edge.
    b.deps = vec!["lib".to_string()];

    let dag = build_dag(vec![a, b]).expect("build_dag should succeed");
    assert!(dag.len() >= 2, "expected at least two nodes, got {}", dag.len());
}
```

- [ ] **Step 3: Run the test**

Run: `cd cli && cargo test -p cook-engine --test unified_dag_build`
Expected: PASS.

- [ ] **Step 4: Commit**

```bash
git add -A
git commit -m "test(cook-engine): pin dag_builder cross-recipe edge handling

Confirms build_dag handles cross-recipe edges in a single call covering
multiple recipes. Required by the unified-DAG model that lands next.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

### Task 4.3: Delete `wave_grouper`

**Goal:** Remove the wave-grouping module entirely. Nothing in the unified-DAG model needs it; cross-recipe edges live on the work-unit DAG.

**Files:**
- Delete: `cli/crates/cook-engine/src/wave_grouper.rs`
- Modify: `cli/crates/cook-engine/src/lib.rs` (remove `pub mod wave_grouper`)
- Modify: `cli/crates/cook-engine/src/run.rs` (Task 4.4 rewrites this; for now, comment out the wave_grouper call)

This task is paired with Task 4.4 — deleting wave_grouper breaks `run.rs` until 4.4 rewires the executor. Land both in one commit OR feature-flag the rewrite. For clarity, **do Task 4.4 first, then 4.3**. We'll revisit the order.

Reordering: **Skip this task; merged into Task 4.4 below.**

---

### Task 4.4: Rewrite `run.rs` to walk the unified DAG

**Goal:** Replace the wave loop with a single DAG build + executor walk. Lifecycle events become unit-driven per spec §9.

**Files:**
- Modify: `cli/crates/cook-engine/src/run.rs` (substantial)
- Delete: `cli/crates/cook-engine/src/wave_grouper.rs`
- Modify: `cli/crates/cook-engine/src/lib.rs` (remove wave_grouper)
- Modify: `cli/crates/cook-engine/src/executor.rs` (if lifecycle event emission lives there)

- [ ] **Step 1: Sketch the new shape**

Replace the wave-loop section of `run_inner` (around `run.rs:478-630` per the current source) with:

```rust
// `dependency_edges_multi` still computes the recipe-level edge map.
// We use it for BuildStarted's topology event and for cross-recipe DAG wiring.
let edges = analyzer::dependency_edges_multi(recipe_infos, targets).map_err(|e| match e {
    GraphError::CycleDetected(s) => EngineError::CycleDetected(s),
    GraphError::UnknownRecipe(s) => EngineError::UnknownRecipe(s),
})?;

// Reachable recipes from target(s). Today the wave loop computes this implicitly.
let reachable: BTreeSet<String> = edges.keys().cloned().collect();

// Pull RecipeUnits for every reachable recipe from the RegisteredWorkspace.
let mut all_units: Vec<RecipeUnits> = Vec::with_capacity(reachable.len());
for name in &reachable {
    let units = registered_workspace.units_by_recipe.get(name).ok_or_else(|| {
        EngineError::UnknownRecipe(name.clone())
    })?;
    let mut u = units.clone();
    if let Some(deps) = edges.get(name) {
        u.deps = deps.clone();
    }
    all_units.push(u);
}

// Build the unified DAG in ONE call.
let dag = dag_builder::build_dag(all_units)?;

// Emit BuildStarted + RecipeQueued events.
let topos: Vec<crate::RecipeTopology> = ... // same shape as today, derived from `edges`/reachable
on_event(EngineEvent::BuildStarted { recipes: topos, total_nodes });
for name in &reachable {
    on_event(EngineEvent::RecipeQueued { name: name.clone() });
}

// Per-recipe cache managers (one per reachable recipe).
let cache_managers: BTreeMap<String, Arc<ThreadSafeCacheManager>> = reachable.iter()
    .map(|name| {
        let wd = registered_workspace.working_dir_by_prefix
            .get(&split_recipe_name(name).0)
            .cloned()
            .unwrap_or_else(|| std::path::PathBuf::from("."));
        let cache_dir = wd.join(".cook").join("cache");
        (name.clone(), Arc::new(ThreadSafeCacheManager::new(cache_dir)))
    })
    .collect();

// Execute the DAG. Lifecycle events fire from the executor (Task 4.5).
let exec_result = executor::execute(&dag, &cache_managers, &on_event, num_jobs, /* ...other ctx... */)?;

// Aggregate exec_result into RunResult and return.
Ok(RunResult { /* ... */ })
```

The key changes:
- One `build_dag` call for the whole reachable set.
- No wave loop, no per-wave `register_recipe` call.
- Cache managers are constructed up front, one per reachable recipe.
- The executor receives the full DAG and the cache-manager map.

- [ ] **Step 2: Delete `wave_grouper.rs`**

```bash
rm cli/crates/cook-engine/src/wave_grouper.rs
```

In `cli/crates/cook-engine/src/lib.rs`, remove `pub mod wave_grouper;` and any re-exports.

- [ ] **Step 3: Adjust call signature**

`run_inner` previously took `&BTreeMap<String, RegistryEntry>`. It now takes `&RegisteredWorkspace`. Update the wrapper `run_with_progress` accordingly.

- [ ] **Step 4: Synthetic lifecycle events for zero-unit recipes**

Today's wave loop emits synthetic `RecipeStarted`/`RecipeCompleted` for recipes whose DAG node count is zero (meta-targets). Preserve this: after building the DAG, compute the set of recipes with at least one node, then for each reachable recipe NOT in that set, emit the synthetic pair before the executor starts:

```rust
let recipes_in_dag: BTreeSet<&str> = (0..dag.len())
    .map(|i| dag.node(i).payload().recipe_name.as_str())
    .collect();
let kind_by_name: BTreeMap<&str, RecipeKind> = registered_workspace.names.iter()
    .map(|r| (r.name.as_str(), r.kind))
    .collect();
for name in &reachable {
    if !recipes_in_dag.contains(name.as_str()) {
        let kind = kind_by_name.get(name.as_str()).copied().unwrap_or(RecipeKind::Recipe);
        on_event(EngineEvent::RecipeStarted { name: name.clone(), total_nodes: 0 });
        on_event(EngineEvent::RecipeCompleted {
            name: name.clone(),
            elapsed: std::time::Duration::ZERO,
            cached_nodes: 0,
            total_nodes: 0,
            kind,
        });
    }
}
```

- [ ] **Step 5: Run engine unit tests**

Run: `cd cli && cargo test -p cook-engine --lib`
Expected: PASS, modulo tests that asserted wave-grouper API or wave structure. Update those to use the unified DAG path.

- [ ] **Step 6: Run engine integration tests**

Run: `cd cli && cargo test -p cook-engine --test '*'`
Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add -A
git commit -m "feat(cook-engine): unified work-unit DAG; delete wave_grouper

Replaces the per-wave register+DAG+execute loop with a single DAG build
across every reachable recipe and a single executor walk. wave_grouper.rs
is deleted; cross-recipe edges live directly on the unified DAG.

Lifecycle events (RecipeStarted/RecipeCompleted) still fire per recipe;
zero-unit recipes get synthetic events as today.

Required for SHI-222 acceptance.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

### Task 4.5: Lifecycle events become unit-driven

**Goal:** `RecipeStarted` fires when the first unit owned by recipe N transitions out of `Waiting`; `RecipeCompleted` fires when the last finishes. The executor today emits these at wave-boundary times; we need them tied to actual unit motion.

**Files:**
- Modify: `cli/crates/cook-engine/src/executor.rs` (event emission)

- [ ] **Step 1: Locate event emission in the executor**

Read `cli/crates/cook-engine/src/executor.rs` end-to-end. Identify where `RecipeStarted` and `RecipeCompleted` are emitted today.

- [ ] **Step 2: Re-emit at unit-state-transition points**

Maintain a per-recipe pending-unit counter inside the executor:

```rust
let mut pending_units_by_recipe: BTreeMap<String, usize> = BTreeMap::new();
let mut started_recipes: BTreeSet<String> = BTreeSet::new();

// Initialize: count units per recipe across the whole DAG.
for i in 0..dag.len() {
    let recipe = dag.node(i).payload().recipe_name.clone();
    *pending_units_by_recipe.entry(recipe).or_insert(0) += 1;
}

// When a unit transitions out of Waiting (i.e. starts):
let recipe = dag.node(unit_id).payload().recipe_name.clone();
if !started_recipes.contains(&recipe) {
    started_recipes.insert(recipe.clone());
    on_event(EngineEvent::RecipeStarted {
        name: recipe.clone(),
        total_nodes: pending_units_by_recipe[&recipe],
    });
}

// When a unit completes (success OR failure OR cached):
let recipe = dag.node(unit_id).payload().recipe_name.clone();
let pending = pending_units_by_recipe.get_mut(&recipe).unwrap();
*pending -= 1;
if *pending == 0 {
    on_event(EngineEvent::RecipeCompleted { /* ... */ });
}
```

The exact hooks depend on the executor's loop structure — locate the "dequeue Waiting unit" point and the "unit finished" point.

- [ ] **Step 3: Run engine tests**

Run: `cd cli && cargo test -p cook-engine`
Expected: PASS, modulo tests asserting specific wave-aligned event ordering. Update those to assert unit-driven ordering: a recipe's `RecipeStarted` precedes its first unit's `UnitStarted`; `RecipeCompleted` follows its last unit's `UnitCompleted`.

- [ ] **Step 4: Commit**

```bash
git add -A
git commit -m "feat(cook-engine): unit-driven RecipeStarted/RecipeCompleted events

The executor now emits RecipeStarted when a recipe's first unit
transitions out of Waiting, and RecipeCompleted when its last unit
finishes. Wave-aligned firing is gone; events now reflect actual work.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

### Task 4.6: Surface `RegisterError::DependencyCycle` through `EngineError`

**Goal:** Cycle errors from `register_cookfile`'s topo-sort step need to surface to the CLI cleanly. Map them onto an existing or new `EngineError` variant.

**Files:**
- Modify: `cli/crates/cook-engine/src/lib.rs` (EngineError mapping)
- Modify: `cli/crates/cook-cli/src/error.rs` (Task 5.6 — already covered for collision)

- [ ] **Step 1: Map `RegisterError::DependencyCycle` to `EngineError::CycleDetected`**

In `cli/crates/cook-engine/src/lib.rs`, where `EngineError` is defined, ensure `CycleDetected(String)` exists. Add a `From<cook_register::RegisterError>` impl that maps `DependencyCycle { recipes }` to `EngineError::CycleDetected(format!("recipe cycle: {recipes:?}"))`.

```rust
impl From<cook_register::RegisterError> for EngineError {
    fn from(e: cook_register::RegisterError) -> Self {
        match e {
            cook_register::RegisterError::DependencyCycle { recipes } => {
                EngineError::CycleDetected(format!("recipe cycle: {recipes:?}"))
            }
            cook_register::RegisterError::RecipeCollision { name, .. } => {
                EngineError::RegistrationFailed {
                    recipe: name.clone(),
                    message: format!("recipe '{name}' is registered more than once"),
                }
            }
            cook_register::RegisterError::Lua(le) => {
                EngineError::RegistrationFailed { recipe: String::new(), message: le.to_string() }
            }
            cook_register::RegisterError::CommandFailed { command, line, code } => {
                EngineError::RegistrationFailed {
                    recipe: String::new(),
                    message: format!("Cookfile:{line}: command failed (exit {code}): {command}"),
                }
            }
            cook_register::RegisterError::RecipeNotFound(name) => {
                EngineError::UnknownRecipe(name)
            }
        }
    }
}
```

- [ ] **Step 2: Run engine tests**

Run: `cd cli && cargo test -p cook-engine`
Expected: PASS.

- [ ] **Step 3: Commit**

```bash
git add -A
git commit -m "refactor(cook-engine): map RegisterError to EngineError variants

DependencyCycle from register_cookfile maps to EngineError::CycleDetected;
RecipeCollision maps to RegistrationFailed (the CLI lifts it to its own
CookError::RecipeCollision in a later commit). Other variants are
plumbed through the existing engine error surface.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Phase 5 — CLI migration: cmd_run, cmd_list, cmd_test, cmd_menu, cmd_dag

### Task 5.1: Build `register_workspace` helper

**Goal:** A single pipeline helper that calls `register_cookfile` for the root and each import, merges the per-import results into a `RegisteredWorkspace`. Replaces today's `build_*_registries`.

**Files:**
- Create: `cli/crates/cook-engine/src/pipeline/registers.rs`
- Modify: `cli/crates/cook-engine/src/pipeline/mod.rs` (re-export)

- [ ] **Step 1: Sketch the function**

```rust
//! Workspace-level register pass: invokes cook-register::register_cookfile
//! once per Cookfile, merges per-import results into a RegisteredWorkspace.

use std::collections::{BTreeMap, HashMap};
use std::path::Path;

use cook_register::{register_cookfile, RegisterSessionBuilder};

use crate::registered_workspace::RegisteredWorkspace;
use super::env::{load_env, parse_cli_overrides, resolve_env};
use super::error::PipelineError;
use super::recipe_info::find_full_prefix;
use super::workspace::Workspace;

pub fn register_single_cookfile(
    cookfile_dir: &Path,
    env_vars: HashMap<String, String>,
    env_overrides: &[String],
    lua_source: String,
    selected_config: Option<&str>,
    cache_ctx: Option<std::sync::Arc<cook_cache::cache_ctx::CacheContext>>,
) -> Result<RegisteredWorkspace, PipelineError> {
    let cli_overrides = parse_cli_overrides(env_overrides)?;
    let builder = RegisterSessionBuilder::new(cookfile_dir.to_path_buf(), env_vars)
        .with_cli_overrides(cli_overrides)
        .with_selected_config(selected_config.map(|s| s.to_string()));
    let registered = register_cookfile(builder, &lua_source, cache_ctx)
        .map_err(|e| PipelineError::Other(e.to_string()))?;

    let mut ws = RegisteredWorkspace {
        names: registered.names,
        units_by_recipe: registered.units_by_recipe,
        probes: registered.probes,
        final_env_by_cookfile: BTreeMap::new(),
        working_dir_by_prefix: BTreeMap::new(),
        alias_dirs_by_prefix:  BTreeMap::new(),
    };
    ws.final_env_by_cookfile.insert(String::new(), registered.final_env);
    ws.working_dir_by_prefix.insert(String::new(), cookfile_dir.to_path_buf());
    ws.alias_dirs_by_prefix.insert(String::new(), BTreeMap::new());
    Ok(ws)
}

pub fn register_workspace(
    workspace: &Workspace,
    config: Option<&str>,
    env_overrides: &[String],
    cache_ctx: Option<std::sync::Arc<cook_cache::cache_ctx::CacheContext>>,
) -> Result<RegisteredWorkspace, PipelineError> {
    let dotenv_vars = load_env(&workspace.root.dir);
    let root_env = resolve_env(config, dotenv_vars, env_overrides)?;
    let cli_overrides = parse_cli_overrides(env_overrides)?;
    let shared_outputs: cook_register::SharedTerminalOutputs =
        std::sync::Arc::new(std::sync::Mutex::new(BTreeMap::new()));

    let mut ws = RegisteredWorkspace {
        names: Vec::new(),
        units_by_recipe: BTreeMap::new(),
        probes: BTreeMap::new(),
        final_env_by_cookfile: BTreeMap::new(),
        working_dir_by_prefix: BTreeMap::new(),
        alias_dirs_by_prefix:  BTreeMap::new(),
    };

    // Root.
    let root_alias_dirs = workspace.alias_dirs_for(&workspace.root.dir);
    let root_alias_qp   = workspace.alias_qualified_prefixes_for(&workspace.root.dir);
    let root_builder = RegisterSessionBuilder::new(workspace.root.dir.clone(), root_env)
        .with_cli_overrides(cli_overrides.clone())
        .with_selected_config(config.map(|s| s.to_string()))
        .with_shared_terminal_outputs(shared_outputs.clone())
        .with_qualified_prefix(String::new())
        .with_alias_dirs(root_alias_dirs.clone())
        .with_alias_qualified_prefixes(root_alias_qp);
    let root_registered = register_cookfile(root_builder, &workspace.root.lua_source, cache_ctx.clone())
        .map_err(|e| PipelineError::Other(e.to_string()))?;
    merge_into(&mut ws, "", root_registered);
    ws.working_dir_by_prefix.insert(String::new(), workspace.root.dir.clone());
    ws.alias_dirs_by_prefix.insert(String::new(), root_alias_dirs);

    // Imports.
    for (canonical_path, loaded) in &workspace.imports {
        let prefix = find_full_prefix(workspace, canonical_path);
        let import_env = resolve_env(config, HashMap::new(), env_overrides)?;
        let alias_dirs = workspace.alias_dirs_for(&loaded.dir);
        let alias_qp   = workspace.alias_qualified_prefixes_for(&loaded.dir);
        let builder = RegisterSessionBuilder::new(loaded.dir.clone(), import_env)
            .with_cli_overrides(cli_overrides.clone())
            .with_selected_config(config.map(|s| s.to_string()))
            .with_shared_terminal_outputs(shared_outputs.clone())
            .with_qualified_prefix(prefix.clone())
            .with_alias_dirs(alias_dirs.clone())
            .with_alias_qualified_prefixes(alias_qp);
        let import_registered = register_cookfile(builder, &loaded.lua_source, cache_ctx.clone())
            .map_err(|e| PipelineError::Other(e.to_string()))?;
        merge_into(&mut ws, &prefix, import_registered);
        ws.working_dir_by_prefix.insert(prefix.clone(), loaded.dir.clone());
        ws.alias_dirs_by_prefix.insert(prefix.clone(), alias_dirs);
    }

    Ok(ws)
}

fn merge_into(ws: &mut RegisteredWorkspace, prefix: &str, rc: cook_register::RegisteredCookfile) {
    let qualify = |name: &str| if prefix.is_empty() { name.to_string() } else { format!("{prefix}.{name}") };
    for n in rc.names {
        let mut qn = n.clone();
        qn.name = qualify(&n.name);
        ws.names.push(qn);
    }
    for (name, units) in rc.units_by_recipe {
        ws.units_by_recipe.insert(qualify(&name), units);
    }
    for (key, probe) in rc.probes {
        ws.probes.insert(if prefix.is_empty() { key } else { format!("{prefix}.{key}") }, probe);
    }
    ws.final_env_by_cookfile.insert(prefix.to_string(), rc.final_env);
}
```

- [ ] **Step 2: Re-export in `pipeline/mod.rs`**

```rust
mod registers;
pub use registers::{register_single_cookfile, register_workspace};
```

- [ ] **Step 3: Compile-check**

Run: `cd cli && cargo check -p cook-engine`
Expected: clean.

- [ ] **Step 4: Commit**

```bash
git add -A
git commit -m "feat(cook-engine): add register_workspace / register_single_cookfile pipeline helpers

Calls cook-register::register_cookfile once per Cookfile (root + each
import), merges per-import results into a RegisteredWorkspace with
qualified names. Used by the CLI commands in subsequent commits.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

### Task 5.2: Replace `recipe_info.rs` build helpers

**Goal:** The CLI no longer builds `recipe_infos` from AST. It pulls from `RegisteredWorkspace.names` and synthesizes a `BTreeMap<String, RecipeInfo>` for the dependency analyzer.

**Files:**
- Modify: `cli/crates/cook-engine/src/pipeline/recipe_info.rs`

- [ ] **Step 1: Add `build_recipe_infos_from_registered`**

In `cli/crates/cook-engine/src/pipeline/recipe_info.rs`:

```rust
pub fn build_recipe_infos_from_registered(
    ws: &crate::registered_workspace::RegisteredWorkspace,
) -> BTreeMap<String, RecipeInfo> {
    let mut infos = BTreeMap::new();
    for name in &ws.names {
        // `serves` is populated only for surface recipes whose units carry
        // terminal_outputs; for dynamic recipes (cook_cc.bin etc.) this is
        // empty and they rely on declared `requires` instead.
        let serves: Vec<String> = ws.units_by_recipe.get(&name.name)
            .map(|u| u.terminal_outputs.clone())
            .unwrap_or_default();
        infos.insert(
            name.name.clone(),
            RecipeInfo {
                ingredients: vec![],
                serves,
                requires: name.requires.clone(),
            },
        );
    }
    infos
}
```

Delete the now-unused `build_single_recipe_infos` and `build_workspace_recipe_info` functions and their `workspace_to_layout` helper. The analyzer's `WorkspaceLayout` type may still be needed by other callers; if so, keep `find_full_prefix` reachable through a small helper or move it into a sibling module.

- [ ] **Step 2: Compile-check**

Run: `cd cli && cargo check -p cook-engine`
Expected: clean. If `build_single_recipe_infos` or `build_workspace_recipe_info` are called from cook-cli, update those call sites in Task 5.4.

- [ ] **Step 3: Commit**

```bash
git add -A
git commit -m "refactor(cook-engine): build recipe_infos from RegisteredWorkspace

recipe_infos no longer come from the AST; they come from the unified
register pass. This makes Lua-registered recipes (cook_cc.bin, etc.)
first-class members of the dependency graph the engine resolves.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

### Task 5.3: Migrate `cmd_run`

**Goal:** `cmd_run` calls `register_workspace` (or `register_single_cookfile`), builds the `recipe_infos` from the result, and hands the `RegisteredWorkspace` to the runner.

**Files:**
- Modify: `cli/crates/cook-cli/src/pipeline.rs` (cmd_run and run_with_progress)

- [ ] **Step 1: Update `cmd_run`**

Replace `cmd_run`'s body in `cli/crates/cook-cli/src/pipeline.rs:447-498`:

```rust
pub fn cmd_run(globals: &Globals, recipe_name: &str, config: Option<&str>) -> Result<(), CookError> {
    let parsed = read_and_parse(globals)?;
    pipeline::validate_selected_config(&parsed.cookfile, config)
        .map_err(pipeline_error_to_cook_error)?;

    let num_jobs = resolve_num_jobs(globals);
    let targets = vec![recipe_name.to_string()];

    let registered = if !parsed.cookfile.imports.is_empty() {
        let workspace_root = pipeline::resolve_workspace_root(&globals.file, globals.root.clone())
            .map_err(pipeline_error_to_cook_error)?;
        let workspace = Workspace::load(&globals.file, &workspace_root, &globals.set)
            .map_err(pipeline_error_to_cook_error)?;
        pipeline::register_workspace(&workspace, config, &globals.set, /*cache_ctx*/ None)
            .map_err(pipeline_error_to_cook_error)?
    } else {
        let cookfile_dir = globals.file.parent().unwrap_or(std::path::Path::new("."));
        let dotenv_vars = pipeline::load_env(cookfile_dir);
        let env_vars = pipeline::resolve_env(config, dotenv_vars, &globals.set)
            .map_err(pipeline_error_to_cook_error)?;
        pipeline::register_single_cookfile(
            cookfile_dir,
            env_vars,
            &globals.set,
            parsed.lua_source,
            config,
            None,
        )
        .map_err(pipeline_error_to_cook_error)?
    };

    let recipe_infos = pipeline::build_recipe_infos_from_registered(&registered);
    let inferred_deps = pipeline::compute_inferred_deps_from_registered(&registered);
    print_dep_conflicts(&pipeline::dep_conflicts_from_registered(&registered, &inferred_deps));

    run_with_progress(globals, &recipe_infos, &targets, &registered, num_jobs, &inferred_deps)?;
    Ok(())
}
```

`compute_inferred_deps_from_registered` and `dep_conflicts_from_registered` are new helpers that look at each `RecipeUnits.dep_edges` (or terminal_outputs) instead of the AST. They live in `cook-engine/src/pipeline/recipe_info.rs` alongside `build_recipe_infos_from_registered`.

- [ ] **Step 2: Update `run_with_progress`**

The function now takes `&RegisteredWorkspace` instead of `&BTreeMap<String, RegistryEntry>`. Inside, the call into `cook_engine::run` follows the new signature from Task 4.4. The cache_ctx construction that today happens inside `run_inner` moves UP into the CLI so the same cache_ctx can be threaded through `register_workspace` (so probes registered at register-time see real machine identity). Confirm this by reading the current `run_inner` cache bootstrap and lifting it before the `register_workspace` call:

```rust
// In cmd_run, before register_workspace:
let cache_ctx = build_cache_ctx(&project_root)?; // moved out of run_inner
// Thread cache_ctx into both register_workspace and run_with_progress.
```

- [ ] **Step 3: Run CLI tests**

Run: `cd cli && cargo test -p cook-cli`
Expected: PASS, modulo tests asserting wave-loop behavior. Update those.

- [ ] **Step 4: Run whole test suite**

Run: `cd cli && cargo test`
Expected: PASS. Failures in `cook-engine` tests asserting wave-aligned events go in Task 8.1's audit.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat(cook-cli): cmd_run uses register_workspace + unified DAG

cmd_run now registers the whole workspace up front and walks the
unified DAG. Lua-registered recipes (cook_cc.bin etc.) become
dispatchable — closes SHI-222's primary acceptance.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

### Task 5.4: Migrate `cmd_list` and `cmd_menu` to `list_names`

**Goal:** `cook list` and `cook menu` enumerate the full name set including Lua-registered recipes via the cheap `list_names` path.

**Files:**
- Modify: `cli/crates/cook-cli/src/pipeline.rs` (cmd_list, cmd_menu)

- [ ] **Step 1: Add a workspace-level `list_names` helper to the pipeline**

In `cook-engine/src/pipeline/registers.rs`:

```rust
pub fn list_workspace_names(
    workspace: &Workspace,
    config: Option<&str>,
    env_overrides: &[String],
) -> Result<Vec<(String /* qualified name */, cook_register::RecipeKind)>, PipelineError> {
    let dotenv_vars = load_env(&workspace.root.dir);
    let root_env = resolve_env(config, dotenv_vars, env_overrides)?;
    let cli_overrides = parse_cli_overrides(env_overrides)?;
    let mut out: Vec<(String, cook_register::RecipeKind)> = Vec::new();

    let root_builder = RegisterSessionBuilder::new(workspace.root.dir.clone(), root_env)
        .with_cli_overrides(cli_overrides.clone())
        .with_selected_config(config.map(|s| s.to_string()));
    let root_names = cook_register::list_names(root_builder, &workspace.root.lua_source)
        .map_err(|e| PipelineError::Other(e.to_string()))?;
    for n in root_names { out.push((n.name, n.kind)); }

    for (canonical_path, loaded) in &workspace.imports {
        let prefix = find_full_prefix(workspace, canonical_path);
        let import_env = resolve_env(config, HashMap::new(), env_overrides)?;
        let builder = RegisterSessionBuilder::new(loaded.dir.clone(), import_env)
            .with_cli_overrides(cli_overrides.clone())
            .with_selected_config(config.map(|s| s.to_string()))
            .with_qualified_prefix(prefix.clone());
        let names = cook_register::list_names(builder, &loaded.lua_source)
            .map_err(|e| PipelineError::Other(e.to_string()))?;
        for n in names { out.push((format!("{prefix}.{}", n.name), n.kind)); }
    }

    Ok(out)
}

pub fn list_single_cookfile_names(
    cookfile_dir: &Path,
    env_vars: HashMap<String, String>,
    env_overrides: &[String],
    lua_source: String,
    selected_config: Option<&str>,
) -> Result<Vec<(String, cook_register::RecipeKind)>, PipelineError> {
    let cli_overrides = parse_cli_overrides(env_overrides)?;
    let builder = RegisterSessionBuilder::new(cookfile_dir.to_path_buf(), env_vars)
        .with_cli_overrides(cli_overrides)
        .with_selected_config(selected_config.map(|s| s.to_string()));
    let names = cook_register::list_names(builder, &lua_source)
        .map_err(|e| PipelineError::Other(e.to_string()))?;
    Ok(names.into_iter().map(|n| (n.name, n.kind)).collect())
}
```

- [ ] **Step 2: Rewrite `cmd_list`**

```rust
pub fn cmd_list(globals: &Globals, args: &crate::cli::ListArgs) -> Result<(), CookError> {
    if args.recipes_only && args.chores_only {
        return Err(CookError::Other(
            "--recipes-only and --chores-only are mutually exclusive".to_string(),
        ));
    }

    let parsed = read_and_parse(globals)?;
    let want_recipes = !args.chores_only;
    let want_chores  = !args.recipes_only;

    let names = if !parsed.cookfile.imports.is_empty() {
        let workspace_root = pipeline::resolve_workspace_root(&globals.file, globals.root.clone())
            .map_err(pipeline_error_to_cook_error)?;
        let workspace = Workspace::load(&globals.file, &workspace_root, &globals.set)
            .map_err(pipeline_error_to_cook_error)?;
        pipeline::list_workspace_names(&workspace, /* config */ None, &globals.set)
            .map_err(pipeline_error_to_cook_error)?
    } else {
        let cookfile_dir = globals.file.parent().unwrap_or(std::path::Path::new("."));
        let dotenv_vars = pipeline::load_env(cookfile_dir);
        let env_vars = pipeline::resolve_env(None, dotenv_vars, &globals.set)
            .map_err(pipeline_error_to_cook_error)?;
        pipeline::list_single_cookfile_names(
            cookfile_dir, env_vars, &globals.set, parsed.lua_source, None,
        )
        .map_err(pipeline_error_to_cook_error)?
    };

    for (name, kind) in names {
        let is_chore = matches!(kind, cook_register::RecipeKind::Chore);
        if (is_chore && want_chores) || (!is_chore && want_recipes) {
            println!("{name}");
        }
    }
    Ok(())
}
```

- [ ] **Step 3: Update `cmd_menu` similarly**

`cmd_menu` already exists and renders an interactive picker. Replace its name source with `list_workspace_names` / `list_single_cookfile_names` (same as `cmd_list`).

- [ ] **Step 4: Run CLI tests**

Run: `cd cli && cargo test -p cook-cli`
Expected: PASS.

- [ ] **Step 5: Smoke test cook list**

```bash
cd examples/raylib-game && cargo run -p cook-cli -- list
```
Expected: prints `raylib` and `game`.

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "feat(cook-cli): cmd_list/cmd_menu enumerate via list_names

cook list and cook menu now use the cheap list_names path through
register-phase load — so Lua-registered recipes (cook_cc.bin etc.)
appear in the output without invoking any recipe body or firing
probe queries.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

### Task 5.5: Migrate `cmd_test` and `cmd_dag`

**Goal:** Both commands reach the engine via `register_workspace`. `cmd_test` uses the resulting `names` for scope resolution and the `units_by_recipe` for execution; `cmd_dag` emits the DAG directly.

**Files:**
- Modify: `cli/crates/cook-cli/src/pipeline.rs` (cmd_test, cmd_dag)

- [ ] **Step 1: cmd_dag**

`cmd_dag` today walks recipe_infos and emits a graphviz-style DAG. Replace `recipe_infos` construction with `register_workspace` + `build_recipe_infos_from_registered`. Emit the unified DAG itself (built via `dag_builder::build_dag`) instead of just the recipe-level edges. Output shape: per-unit nodes labeled with their recipe-name owner.

- [ ] **Step 2: cmd_test**

Replace `collect_workspace_recipe_names` (cli/crates/cook-cli/src/pipeline.rs:536) with a call to `list_workspace_names` (cheap, no body invocation). Use the returned name list for scope resolution. For the execution path, switch to `register_workspace` + the same engine entry point as `cmd_run`, but with the test-scope filter applied to the executor's reachable set.

- [ ] **Step 3: Run CLI tests**

Run: `cd cli && cargo test -p cook-cli`
Expected: PASS.

- [ ] **Step 4: Commit**

```bash
git add -A
git commit -m "feat(cook-cli): cmd_test and cmd_dag migrate to register_workspace

cmd_test resolves scope via list_names and runs through the same
unified-DAG executor path as cmd_run, filtered to the test scope.
cmd_dag emits the unified work-unit DAG instead of the recipe-level
edge map.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

### Task 5.6: Add `CookError::RecipeCollision` with exit code 3

**Goal:** The CLI maps `RecipeCollision` from the engine onto a top-level error variant with exit code 3, matching `RecipeNotFound`.

**Files:**
- Modify: `cli/crates/cook-cli/src/error.rs`
- Modify: `cli/crates/cook-cli/src/pipeline.rs` (error mapper)

- [ ] **Step 1: Add the variant**

In `cli/crates/cook-cli/src/error.rs`:

```rust
pub enum CookError {
    // ... existing variants ...
    RecipeCollision(String),
}

impl CookError {
    pub fn exit_code(&self) -> i32 {
        match self {
            CookError::RecipeNotFound(_)   => 3,
            CookError::RecipeCollision(_)  => 3,
            // ... existing arms ...
        }
    }
}

impl std::fmt::Display for CookError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CookError::RecipeNotFound(name)  => write!(f, "recipe not found: {name}"),
            CookError::RecipeCollision(msg)  => write!(f, "{msg}"),
            // ... existing arms ...
        }
    }
}
```

- [ ] **Step 2: Add `PipelineError::RecipeCollision` and route through it**

In `cli/crates/cook-engine/src/pipeline/error.rs` (the file where `PipelineError` is defined — locate via `rg 'pub enum PipelineError'` if the path differs), extend the enum:

```rust
pub enum PipelineError {
    // existing variants ...
    RecipeCollision {
        name: String,
        sites: Vec<cook_register::RegistrationSite>,
    },
}
```

In `register_workspace` and `register_single_cookfile` (Task 5.1), replace the generic `PipelineError::Other(e.to_string())` mapping for `RegisterError::RecipeCollision` with a typed mapping:

```rust
.map_err(|e| match e {
    cook_register::RegisterError::RecipeCollision { name, sites } =>
        PipelineError::RecipeCollision { name, sites },
    other => PipelineError::Other(other.to_string()),
})?
```

- [ ] **Step 3: Format the diagnostic at CLI emit time**

```rust
fn pipeline_error_to_cook_error(e: cook_engine::pipeline::PipelineError) -> CookError {
    match e {
        // ...
        cook_engine::pipeline::PipelineError::RecipeCollision { name, sites } => {
            let mut msg = format!("error: recipe '{name}' is registered more than once:\n");
            for s in &sites {
                let kind_str = match s.kind {
                    cook_register::RegistrationSiteKind::SurfaceRecipe => "as a `recipe` block",
                    cook_register::RegistrationSiteKind::SurfaceChore  => "as a `chore` block",
                    cook_register::RegistrationSiteKind::Dynamic       => "by cook.recipe at register-phase",
                };
                msg.push_str(&format!("  - Cookfile:{}: {}\n", s.line, kind_str));
            }
            msg.push_str("rename one of them.");
            CookError::RecipeCollision(msg)
        }
        // ...
    }
}
```

- [ ] **Step 4: Run CLI tests**

Run: `cd cli && cargo test -p cook-cli`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat(cook-cli): map recipe collisions to CookError::RecipeCollision (exit 3)

Diagnostic names both registration sites with line and kind labels,
matches the format in the SHI-222 spec §8.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Phase 6 — Clean break: delete `Registry::register_recipe`

### Task 6.1: Delete `Registry::register_recipe` (now `RegisterSessionBuilder::register_recipe`)

**Goal:** Final cleanup. Remove the legacy method and any plumbing that exists solely to support it.

**Files:**
- Modify: `cli/crates/cook-register/src/engine.rs` (remove the method body)
- Modify: `cli/crates/cook-register/src/lib.rs` (any re-exports tied to it)

- [ ] **Step 1: Find remaining call sites**

```bash
rg 'register_recipe\(' cli/crates/
```

By this point, the only remaining call sites should be in `cook-register/src/tests.rs` (legacy tests). Phase 6 Task 6.2 migrates those; this task deletes the method.

If any non-test call site remains, **stop and fix that call site first** — Phase 5 should have migrated all of them. Likely culprit: a missed `cmd_*` arm. Audit and rewire before continuing.

- [ ] **Step 2: Delete the method**

In `cli/crates/cook-register/src/engine.rs`, remove the entire `impl RegisterSessionBuilder` block's `register_recipe` method.

- [ ] **Step 3: Run compile**

Run: `cd cli && cargo check`
Expected: clean — except for `cook-register/src/tests.rs` complaints, which are Task 6.2's job.

- [ ] **Step 4: Commit (the deletion + 6.2 migration land together)**

Hold the commit; pair with 6.2.

---

### Task 6.2: Migrate `cook-register/src/tests.rs` to `register_cookfile`

**Goal:** Every test in `cook-register/src/tests.rs` that calls `register_recipe` migrates to `register_cookfile` (taking from the new `units_by_recipe` map by recipe name).

**Files:**
- Modify: `cli/crates/cook-register/src/tests.rs`

- [ ] **Step 1: Translate each test**

For each test in `tests.rs` that calls:

```rust
let result = rt.register_recipe(lua_src, "build", None).unwrap();
```

Replace with:

```rust
let registered = register_cookfile(rt, lua_src, None).unwrap();
let result = registered.units_by_recipe.get("build").expect("build should be registered").clone();
```

`rt` here was previously a `Registry`; if needed, replace `Registry::new(...)` with `RegisterSessionBuilder::new(...)`. `cargo fix` won't help with this — go through each test in the file manually.

The file has ~30 call sites. Touch each. For tests that previously asserted `register_recipe` errors (e.g. `RecipeNotFound`), assert on `register_cookfile` errors:

```rust
// Before:
let err = rt.register_recipe(lua_src, "missing", None).unwrap_err();
assert!(matches!(err, RegisterError::RecipeNotFound(_)));

// After: "missing" is requested as a target by the engine, not by cook-register.
// The cook-register-level test simply asserts that "missing" isn't in units_by_recipe.
let registered = register_cookfile(rt, lua_src, None).unwrap();
assert!(!registered.units_by_recipe.contains_key("missing"));
```

- [ ] **Step 2: Run cook-register tests**

Run: `cd cli && cargo test -p cook-register`
Expected: PASS.

- [ ] **Step 3: Run full CLI test suite**

Run: `cd cli && cargo test`
Expected: PASS.

- [ ] **Step 4: Commit (delete + migrate, together)**

```bash
git add -A
git commit -m "refactor(cook-register): delete Registry::register_recipe; migrate tests

Final clean break: the legacy per-recipe register entry point is gone.
All cook-register/src/tests.rs call sites migrate to register_cookfile
and read from units_by_recipe by name. No shim, no #[deprecated].

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

### Task 6.3: Delete `RegistryEntry` and `pipeline/registries.rs`

**Goal:** The transitional type and module are unused after Phase 5. Remove them.

**Files:**
- Delete: `cli/crates/cook-engine/src/registry_entry.rs`
- Delete: `cli/crates/cook-engine/src/pipeline/registries.rs`
- Modify: `cli/crates/cook-engine/src/lib.rs` and `cli/crates/cook-engine/src/pipeline/mod.rs` (remove module/re-exports)

- [ ] **Step 1: Confirm no remaining callers**

```bash
rg 'RegistryEntry|build_single_registries|build_workspace_registries' cli/crates/
```

Expect: empty output. Any remaining hits must be removed first (Phase 5 audit miss).

- [ ] **Step 2: Delete**

```bash
git rm cli/crates/cook-engine/src/registry_entry.rs
git rm cli/crates/cook-engine/src/pipeline/registries.rs
```

In `cli/crates/cook-engine/src/lib.rs`, remove:

```rust
pub mod registry_entry;
pub use registry_entry::RegistryEntry;
```

In `cli/crates/cook-engine/src/pipeline/mod.rs`, remove:

```rust
pub mod registries;
pub use registries::{build_single_registries, build_workspace_registries};
```

- [ ] **Step 3: Compile**

Run: `cd cli && cargo check`
Expected: clean.

- [ ] **Step 4: Run full CLI test suite**

Run: `cd cli && cargo test`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "refactor(cook-engine): delete RegistryEntry and pipeline/registries.rs

Unused after the register_workspace migration. RegisteredWorkspace
is the engine-side transit type now.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Phase 7 — Standard amendments and conformance fixtures

The spec-first hook will block any code commit in `cli/crates/cook-lang/`, `cli/crates/cook-luagen/`, `cli/crates/cook-register/`, or `standard/conformance/` unless a matching `standard/` file is touched. Phase 7 lands the Standard side of the change. Phase 7 commits MUST land on the same branch as Phases 1–6; the squash-merge or final review can re-order, but every individual commit in the language-surface crates needs the spec change already merged or in the same PR.

### Task 7.1: §13 amendment — unified DAG

**Files:**
- Modify: `standard/src/content/docs/13-two-phase.mdx`

- [ ] **Step 1: Locate §13**

Find the section where the two-phase model is described. Look for a passage talking about waves; this is the canonical place for the amendment.

- [ ] **Step 2: Add normative text**

Add a new subsection (numbering depending on existing chapter structure):

```mdx
## 13.X. Work-unit DAG unification [#two-phase.dag-unification]

The register phase produces a single **work-unit DAG** covering every recipe reachable from any dispatched target. Cross-recipe dependencies are edges of this unified DAG, not synchronization boundaries between recipe-level partitions.

A conforming implementation MAY use any topological ordering of recipe-body invocation at register time (for example, to resolve `cook.dep_output` references — §{lua.cook-dep-output}); the choice of order is not normative. The execute phase walks the unified DAG; the order in which recipe bodies were invoked at register time does not constrain the execute-time parallelism of the resulting work units.

A conforming implementation MUST NOT expose register-phase grouping (sometimes informally called "waves" in earlier reference implementations) as a normative concept to authoring users.
```

- [ ] **Step 3: Remove or rewrite informative wave references**

Search `standard/src/` for prose mentioning waves:

```bash
rg -i '\bwave' standard/src/content/
```

For each hit, either delete the reference (if it was purely informative scaffolding) or rewrite as "DAG region" / "reachable subgraph" depending on context.

- [ ] **Step 4: Build the Standard locally**

```bash
cd standard && pnpm install --frozen-lockfile && pnpm build
```
Expected: exit 0.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "standard(§13): unified work-unit DAG; deprecate wave terminology

§13 amendment defining the unified work-unit DAG. Wave terminology
is removed from normative prose; informative references are
rewritten as 'DAG region' / 'reachable subgraph'.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

(The above uses a non-ASCII section sign § in the commit message — invoke `git commit` via the same HEREDOC pattern used in earlier tasks if your shell mangles it.)

---

### Task 7.2: §22.3 amendment + new §22 discovery section

**Files:**
- Modify: `standard/src/content/docs/22-register-phase.mdx`

- [ ] **Step 1: Amend §22.3 (`cook.recipe`)**

Find §22.3 in `22-register-phase.mdx` (around the existing paragraph that begins "Surface recipes ... are compiled to exactly one `cook.recipe` call.").

Add the following paragraph after the existing prose:

```mdx
Within a single Cookfile, the set of names registered by `cook.recipe` (whether by surface `recipe NAME` declarations or by register-phase Lua code, including invocations made by Lua module wrappers such as `cook_cc.bin`) MUST be unique. A conforming implementation MUST diagnose a duplicate name with a single error naming both registration sites by line and identifying the kind of each registration (surface `recipe` block, surface `chore` block, or `cook.recipe` call from register-phase Lua). See §{rationale.recipe-name-uniqueness}.
```

- [ ] **Step 2: Add new §22.X (recipe discovery)**

Append a new section to `22-register-phase.mdx`:

```mdx
## 22.X. Recipe discovery [#register-phase.recipe-discovery]

A conforming implementation's recipe-discovery procedure MUST include both surface recipe declarations and recipes registered by `cook.recipe` (§{lua.cook-recipe}) during register-phase Lua execution. Dispatch of `cook NAME` MUST resolve `NAME` against the unified discovered set. A dispatch invocation that names a recipe registered only through register-phase Lua MUST execute identically to one naming a surface recipe declaration.

A conforming implementation MAY expose a listing surface (such as a `cook list` CLI subcommand) that enumerates the same unified set without invoking any recipe body. When such a listing surface exists, it MUST surface Lua-registered names alongside surface declarations.
```

- [ ] **Step 3: Build the Standard**

```bash
cd standard && pnpm build
```
Expected: exit 0. Resolve any link warnings for `§{rationale.recipe-name-uniqueness}` by ensuring the rationale appendix entry lands in Task 7.3.

- [ ] **Step 4: Commit**

```bash
git add -A
git commit -m "standard(§22): recipe-discovery contract; §22.3 collision rule

§22.3 gains a normative paragraph requiring unique recipe names with
a diagnostic identifying both registration sites. New §22.X states
that recipe-discovery normatively includes register-phase Lua
registrations and the dispatch path MUST resolve against the unified
set. Implementations exposing a listing surface MUST surface
Lua-registered names alongside surface declarations.

Closes part of SHI-222.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

### Task 7.3: Rationale + CS-NNNN appendix entries

**Files:**
- Modify: `standard/src/content/docs/appendix/B-rationale.mdx` (or equivalent)
- Modify: `standard/src/content/docs/appendix/D-changes.mdx` (or equivalent)

- [ ] **Step 1: Locate appendix files**

```bash
ls standard/src/content/docs/appendix/
```

Identify the rationale and changes appendix files (names may differ slightly; existing files like `B-rationale.mdx` and `D-changes.mdx` are typical).

- [ ] **Step 2: Add rationale entry**

In the rationale appendix, add:

```mdx
### B.X. Recipe name uniqueness [#rationale.recipe-name-uniqueness]

Allowing two `cook.recipe` registrations for the same name produced silent precedence-by-source-order in earlier implementations: whichever registration appeared first in the codegen output won, with no diagnostic. This was a footgun in two common authoring patterns:

1. A user declares `recipe build` as a surface block, then adds a Lua wrapper like `cook_cc.bin("build", ...)` in a `register` block to extend the build. Surface-vs-dynamic precedence was hidden.
2. A user invokes a wrapper twice by mistake (e.g. `cook_cc.bin("game", ...)` in two different `register` blocks within the same Cookfile). Dynamic-vs-dynamic precedence was hidden.

Both patterns are authoring errors. The §22.3 rule makes them diagnosable. The diagnostic identifies both sites and their kinds so the author knows precisely what to rename.
```

- [ ] **Step 3: Add CS-NNNN changes entry**

In the changes appendix, allocate the next CS number. The hook expects this to land in the same PR as the code change; replace `NNNN` with the allocated number throughout this plan after the assignment is made.

```mdx
### CS-NNNN. Unified register-phase + work-unit DAG

**Date.** 2026-05-XX

**Sections.** §13, §22.3, §22.X.

**Summary.** The register phase produces a single unified work-unit DAG across every reachable recipe. Wave-grouping is removed as a normative concept. `cook.recipe` registrations within a single Cookfile MUST be unique; duplicates produce a hard error naming both sites. Recipe-discovery normatively includes register-phase Lua registrations.

**Migration.** No surface syntax changes. Authors who relied on duplicate-name precedence (uncommon — the previous behaviour was silent) MUST rename one of the registrations.

**Reference implementation.** SHI-222 in the `cook` repository.
```

- [ ] **Step 4: Build the Standard**

```bash
cd standard && pnpm build
```
Expected: exit 0; no dangling link warnings.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "standard: rationale and CS-NNNN entries for SHI-222

Rationale annex for recipe-name uniqueness; CS-NNNN allocated for the
register/DAG unification change.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

### Task 7.4: Positive conformance fixture

**Files:**
- Create: `standard/conformance/positive/recipe-dispatch-from-register-block.cook`
- Create: `standard/conformance/positive/recipe-dispatch-from-register-block.expected.txt`

- [ ] **Step 1: Cookfile**

```
# recipe-dispatch-from-register-block.cook
#
# Confirms that a recipe registered by cook.recipe at register-phase
# is dispatchable via `cook NAME`. No surface `recipe` block declares
# the name `hello`; it is registered exclusively via the register block.

register
    cook.recipe("hello", {requires = {}}, function()
        cook.exec("printf 'hello from register-block recipe\\n'", 3)
    end)
```

- [ ] **Step 2: Expected output**

```
hello from register-block recipe
```

- [ ] **Step 3: Run the conformance harness**

```bash
cd cli && cargo test -p cook-lang --test conformance recipe-dispatch-from-register-block
```
Expected: PASS.

- [ ] **Step 4: Commit**

```bash
git add -A
git commit -m "test(conformance): positive fixture for register-block recipe dispatch

Pins SHI-222 acceptance: a recipe registered by cook.recipe inside a
register block dispatches identically to a surface recipe declaration.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

### Task 7.5: Negative conformance fixture (surface vs dynamic collision)

**Files:**
- Create: `standard/conformance/negative/recipe-name-collision-surface-vs-dynamic.cook`
- Create: `standard/conformance/negative/recipe-name-collision-surface-vs-dynamic.expected.txt`

- [ ] **Step 1: Cookfile**

```
# recipe-name-collision-surface-vs-dynamic.cook
#
# Confirms that registering the same name as both a surface recipe and
# a register-block cook.recipe call is a hard error.

recipe build
    cook.exec("echo surface", 2)

register
    cook.recipe("build", {requires = {}}, function()
        cook.exec("echo dynamic", 6)
    end)
```

- [ ] **Step 2: Expected diagnostic**

The conformance harness compares against a normalised diagnostic. Provide an expected file that pins the shape:

```
error: recipe 'build' is registered more than once:
  - Cookfile:1: as a `recipe` block
  - Cookfile:5: by cook.recipe at register-phase
rename one of them.
```

(Adjust line numbers to match the fixture exactly; codegen `__line` values come from the surface `recipe` block's line and `caller_line_in_cookfile` for the dynamic registration.)

- [ ] **Step 3: Run the conformance harness**

```bash
cd cli && cargo test -p cook-lang --test conformance recipe-name-collision-surface-vs-dynamic
```
Expected: PASS (the harness verifies the negative diagnostic matches).

- [ ] **Step 4: Commit**

```bash
git add -A
git commit -m "test(conformance): negative fixture for surface-vs-dynamic recipe collision

Pins SHI-222 acceptance: registering the same name as both a surface
recipe and a register-block cook.recipe call is a hard error with a
diagnostic naming both sites and their kinds.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Phase 8 — Validation

### Task 8.1: Audit and migrate any tests asserting wave-aligned events

**Files:**
- Modify: any cook-engine test that asserts wave structure or wave-aligned lifecycle event timing

- [ ] **Step 1: Find candidates**

```bash
rg -i 'wave|RecipeStarted|RecipeCompleted' cli/crates/cook-engine/tests/
```

For each hit, read the test and decide:
- If it asserts something about wave grouping: rewrite to assert unit-driven equivalent, or delete if the behavior no longer exists.
- If it asserts lifecycle event ordering: confirm the new unit-driven emission still produces the asserted order; update if not.

- [ ] **Step 2: Run all engine tests**

Run: `cd cli && cargo test -p cook-engine`
Expected: PASS.

- [ ] **Step 3: Commit**

```bash
git add -A
git commit -m "test(cook-engine): migrate wave-aligned assertions to unit-driven equivalents

Tests that asserted wave structure or wave-aligned lifecycle event
ordering now assert the unit-driven semantics introduced by SHI-222.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

### Task 8.2: raylib-game smoke test (the SHI-222 acceptance gate)

**Files:** none modified; this is a validation step.

- [ ] **Step 1: Build cook from the worktree**

```bash
cd /home/alex/dev/cook-shi-222
cargo build --release -p cook-cli
```
Expected: clean build.

- [ ] **Step 2: Run cook list against raylib-game**

```bash
cd examples/raylib-game
PATH=/home/alex/dev/cook-shi-222/cli/target/release:$PATH cook list
```
Expected output:
```
raylib
game
```

- [ ] **Step 3: Run cook build**

```bash
PATH=/home/alex/dev/cook-shi-222/cli/target/release:$PATH cook build
```
Expected: clean exit; `build/bin/game` is produced.

- [ ] **Step 4: Run the binary**

```bash
./build/bin/game
```
Expected: raylib-game runs (window opens, or smoke output for headless). Acceptable to skip the actual run on a headless box, but `file build/bin/game` should report ELF/Mach-O executable.

- [ ] **Step 5: Commit**

No code changes for the smoke itself; if any small example-level fix is needed (e.g. a Cookfile typo that the new path surfaces), commit it here.

- [ ] **Step 6: Note the acceptance status**

Add a row to the PR description (Task 8.4) noting the smoke status.

---

### Task 8.3: Run the full test suite

- [ ] **Step 1: Run everything**

```bash
cd cli && cargo test
```
Expected: clean.

```bash
cd standard && pnpm build
```
Expected: exit 0.

```bash
cd cli && cargo test -p cook-lang --test conformance
```
Expected: clean.

- [ ] **Step 2: If anything fails**

Diagnose. Common shapes:

- A test was asserting `RegistryEntry` API — migrate to `RegisteredWorkspace`.
- A test was asserting wave-structure events — see Task 8.1.
- A conformance fixture's expected output drifted because env-finalization order changed — re-examine and update only if the new order is the intended semantics per §6.

- [ ] **Step 3: Commit any fixes**

```bash
git add -A
git commit -m "test: post-refactor follow-ups for SHI-222 (full suite green)"
```

---

### Task 8.4: PR preparation

- [ ] **Step 1: Squash-review the commit log**

```bash
git log --oneline main..HEAD
```
Expected: a sequence of well-scoped commits, one per task in Phases 1–8. Optionally interactive-rebase to clean up fixups; do NOT use `-i` here — the harness doesn't support interactive shells. Use `git rebase --root` with explicit instructions only if mandatory; otherwise leave the history as-is and rely on a PR squash-merge.

- [ ] **Step 2: Push the branch**

```bash
git push -u origin matg9192/shi-222-unified-register-and-dag
```

- [ ] **Step 3: Open the PR**

Title: `engine(SHI-222): unified register-phase and work-unit DAG`

Body should reference the spec at `standard/specs/2026-05-18-unified-register-and-dag-design.md`, list the acceptance criteria from spec §16, and note the smoke result from Task 8.2.

```bash
gh pr create --title "engine(SHI-222): unified register-phase and work-unit DAG" --body "$(cat <<'EOF'
## Summary

Implements the design at `standard/specs/2026-05-18-unified-register-and-dag-design.md` (`189bdbb`). Closes SHI-222.

- `cook NAME` dispatches recipes registered via `cook.recipe(...)` at register-phase (cook_cc.bin, etc.)
- Single unified work-unit DAG across every reachable recipe; `wave_grouper` deleted
- Lifecycle events become unit-driven
- `cook list` enumerates Lua-registered names via cheap `list_names` (no body invocation)
- Hard error on duplicate registration (surface-vs-dynamic, dynamic-vs-dynamic)
- Standard CS-NNNN: §13 unified DAG, §22.3 collision rule, new §22.X discovery section
- Two conformance fixtures (one positive, one negative)
- Clean break: `Registry::register_recipe` deleted; `Registry` renamed `RegisterSessionBuilder`; `RegistryEntry` collapsed

## Test plan

- [x] `cd cli && cargo test` — clean
- [x] `cd standard && pnpm build` — exit 0
- [x] `cd cli && cargo test -p cook-lang --test conformance` — clean
- [x] `examples/raylib-game/` smoke: `cook list` prints `raylib`, `game`; `cook build` produces `build/bin/game`

🤖 Generated with [Claude Code](https://claude.com/claude-code)
EOF
)"
```

- [ ] **Step 4: Self-review the diff**

Walk the PR diff one file at a time. Check for:
- Any TODO comments accidentally landed.
- Any debug eprintln! / dbg! left behind.
- Any test files that drift in scope from their task.

If anything turns up, fix-up commits + amend the PR.

---

## Self-review checklist (run by the engineer before requesting review)

1. **Spec coverage.** Every acceptance criterion in §16 of the spec has a passing test or smoke.
2. **Naming consistency.** No leftover `Registry::register_recipe` references. No leftover `wave_grouper` references. `RegisterSessionBuilder` is used everywhere.
3. **Standard side green.** `cd standard && pnpm build` exit 0; no dangling link warnings.
4. **Conformance harness green.** Both new fixtures pass; no existing fixture regressed.
5. **Spec-first hook satisfied.** Every commit in `cli/crates/cook-lang/`, `cli/crates/cook-luagen/`, `cli/crates/cook-register/`, or `standard/conformance/` is paired with a `standard/` change in the same branch (the §13 + §22 amendments cover everything in this plan).
6. **raylib-game smoke green.** `cook list` and `cook build` both work end-to-end.
7. **No legacy code remains.** No `#[deprecated]`, no "TODO: remove later", no shim modules.
