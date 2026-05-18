//! cook-register: Capture-mode Lua VM for discovering work units from Cookfiles.
//!
//! Runs generated Cookfile Lua in capture mode to discover work units
//! (commands, inputs, outputs) without executing them.

pub mod capture;
pub mod codec_api;
pub mod context;
pub mod dep_output_api;
pub mod engine;
pub mod env_api;
pub mod export_api;
pub mod module_cache;
pub mod module_loader;
pub mod probe_api;
pub mod probe_value;
pub mod test_api;
pub mod unit_api;

// `fs.*`, `path.*`, and `cook.platform.*` are part of the shared Cook
// Lua API surface (CS-0044). The implementation lives in
// `cook-lua-stdlib` so the same closures register in both the
// register-phase VM (here) and the execute-phase worker VMs in
// `cook-luaotp`. Re-exports preserve the historical
// `cook_register::register_{fs,path}_api` import paths used by the
// engine module.
pub use cook_lua_stdlib::{register_fs_api, register_path_api, register_platform_api};

#[cfg(test)]
mod tests;

use std::cell::RefCell;
use std::rc::Rc;

use thiserror::Error;

use cook_contracts::CapturedUnit;

#[derive(Error, Debug)]
pub enum RegisterError {
    #[error("lua error: {0}")]
    Lua(#[from] mlua::Error),
    #[error("Cookfile:{line}: command failed (exit {code}): {command}")]
    CommandFailed {
        command: String,
        line: usize,
        code: i32,
    },
    #[error("recipe not found: {0}")]
    RecipeNotFound(String),
    /// A cycle in the recipe `requires` graph was detected by
    /// `register_cookfile`'s local topo-sort. `recipes` is the path
    /// of names that forms the cycle, with the recurring node
    /// appearing as both the first and last element.
    ///
    /// Returned by `register_cookfile` (SHI-222 Phase 2 Task 2.2).
    #[error("dependency cycle: {}", recipes.join(" -> "))]
    DependencyCycle { recipes: Vec<String> },
}

/// Session-level capture state. One instance per `register_cookfile` call;
/// shared by all body invocations within that call.
///
/// Uses Rc<RefCell<>> and is therefore !Send. This is intentional —
/// registration runs on a single thread with a single Lua VM.
pub struct SessionCaptureState {
    /// Probes drained from the per-session probe registry after top-level load.
    /// Each invoked body receives a clone of this set in its RecipeUnits.probes.
    pub probes: Vec<cook_contracts::ProbeUnit>,
}

impl SessionCaptureState {
    pub fn new() -> Self {
        Self {
            probes: Vec::new(),
        }
    }
}

impl Default for SessionCaptureState {
    fn default() -> Self {
        Self::new()
    }
}

/// Per-recipe-body capture state. Constructed fresh inside `invoke_body`;
/// drained into a `RecipeUnits` and dropped when the body returns.
///
/// Uses Rc<RefCell<>> and is therefore !Send. This is intentional —
/// registration runs on a single thread with a single Lua VM.
pub struct BodyCaptureState {
    pub inside_layer: bool,
    pub layer_commands: Vec<(String, usize)>,
    pub units: Vec<CapturedUnit>,
    pub current_group: Option<usize>,
    pub step_groups: Vec<Vec<usize>>,
    /// Outputs collected during the current step_group call.
    pub current_step_outputs: Vec<String>,
    /// Terminal outputs: outputs from the last completed step_group.
    /// Updated each time a step_group finishes (last one wins).
    pub last_cook_step_outputs: Vec<String>,
    /// Fine-grained dep edges: (unit_idx, dep_recipe_name).
    pub dep_edges: Vec<(usize, String)>,
    /// Dep refs accumulated during current step_group.
    /// Cleared when step_group ends. Each add_unit call within the group
    /// inherits all accumulated dep refs as edges.
    pub step_group_dep_refs: Vec<String>,
    /// Importer-relative dep output paths accumulated during current step_group.
    /// These are the REWRITTEN paths (with alias_dirs applied) returned by
    /// cook.dep_output() calls. Stored separately from step_group_dep_refs so
    /// that add_unit can land the correct importer-relative paths in
    /// cache_meta.input_paths without re-reading the raw terminal_outputs map.
    pub step_group_dep_input_paths: Vec<String>,
    /// True while the register-phase body of a chore is executing.
    /// `cook.add_unit` raises a Lua error if `cache = true` is passed
    /// while this flag is set (§{chores.no-caching}).
    pub current_chore_active: bool,
    /// The fully-qualified name of the recipe currently executing in
    /// the register phase (e.g. "lib.build" for an imported recipe).
    /// Set just before the recipe body function is called; cleared after.
    /// Used by `cook.add_test` to default `suite` to the enclosing
    /// recipe's name when the caller omits the field (CS-0061 §3.2).
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

/// Ref-counted, interior-mutable handle to a [`SessionCaptureState`].
/// Threaded through every capture closure that needs to read or write
/// session-scoped state (probes only at this point).
pub type SharedSessionCaptureState = Rc<RefCell<SessionCaptureState>>;

/// Ref-counted, interior-mutable handle to the *currently-active*
/// [`BodyCaptureState`]. `None` when no recipe body is executing
/// (e.g. during top-level load, between body invocations).
/// Closures that touch body-scoped state borrow this slot and return
/// a Lua error when the slot is `None`.
pub type SharedBodySlot = Rc<RefCell<Option<BodyCaptureState>>>;

/// Hash a string using xxh3 (for command templates, env vars, etc.)
pub fn hash_str(s: &str) -> u64 {
    xxhash_rust::xxh3::xxh3_64(s.as_bytes())
}

// Re-exports for convenience
pub use capture::RegistrationSource;
pub use dep_output_api::SharedTerminalOutputs;
pub use engine::{list_names, register_cookfile, RegisterSessionBuilder};

/// The artifact of a full `register_cookfile` pass.
///
/// Aggregates every recipe registered during the pass (both
/// surface `recipe NAME` blocks and dynamic `cook.recipe(...)`
/// calls), the per-recipe `RecipeUnits` discovered by invoking
/// each body, the deduplicated probe set for the whole register
/// pass, and the final environment after config-block dispatch.
///
/// Distinct from the per-recipe `RecipeUnits` returned by the
/// legacy `RegisterSessionBuilder::register_recipe` entry point —
/// `RegisteredCookfile` is the unified-DAG payload produced by
/// the new `register_cookfile` entry point (CS-0077 Phase 2).
#[derive(Debug, Clone)]
pub struct RegisteredCookfile {
    pub names: Vec<RegisteredRecipePub>,
    pub units_by_recipe: std::collections::BTreeMap<String, cook_contracts::RecipeUnits>,
    pub probes: std::collections::BTreeMap<String, cook_contracts::ProbeUnit>,
    pub final_env: std::collections::BTreeMap<String, String>,
}

/// Public summary of one registered recipe. Distinct from the internal
/// `capture::RegisteredRecipe` (which holds a `LuaRegistryKey` closure
/// that cannot cross the public API boundary).
#[derive(Debug, Clone)]
pub struct RegisteredRecipePub {
    pub name: String,
    pub source: RegistrationSource,
    pub kind: RecipeKind,
    pub requires: Vec<String>,
}

/// Whether a registered name is a normal recipe or a chore.
///
/// Chores are register-phase-only side-effecting blocks that
/// MUST NOT pass `cache = true` to `cook.add_unit` (§{chores.no-caching}).
/// Tracked on the public summary so consumers (CLI dispatch, surface
/// diagnostics) can branch on it without reaching into the internal
/// `RegisteredRecipe` shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecipeKind {
    Recipe,
    Chore,
}
