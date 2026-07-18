//! cook-register: Capture-mode Lua VM for discovering work units from Cookfiles.
//!
//! Runs generated Cookfile Lua in capture mode to discover work units
//! (commands, inputs, outputs) without executing them.

pub mod capture;
pub mod context;
pub mod dep_output_api;
pub mod engine;
pub mod env_api;
pub mod export_api;
pub mod file_ref;
pub mod module_cache;
pub mod module_loader;
pub mod on_register_api;
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
    /// A name was registered more than once within a single Cookfile
    /// registration pass — e.g. a surface `recipe NAME` block plus a
    /// `cook.recipe("NAME", ...)` call, or two `cook.recipe(...)` calls
    /// using the same name. Each site is identified by line and kind
    /// (surface declaration vs. register-phase Lua call) so the CLI
    /// can render a multi-line diagnostic naming both.
    ///
    /// Returned by `register_cookfile` (SHI-222 Phase 2 Task 2.3, spec §8).
    #[error(
        "recipe '{name}' is registered more than once: {}",
        sites.iter().map(|s| format!("{} at line {}", s.kind.label(), s.line)).collect::<Vec<_>>().join(", ")
    )]
    RecipeCollision {
        name: String,
        sites: Vec<RegistrationSite>,
    },
    /// A cycle in the recipe `requires` graph was detected by
    /// `register_cookfile`'s local topo-sort. `recipes` is the path
    /// of names that forms the cycle, with the recurring node
    /// appearing as both the first and last element.
    ///
    /// Returned by `register_cookfile` (SHI-222 Phase 2 Task 2.2).
    #[error("dependency cycle: {}", recipes.join(" -> "))]
    DependencyCycle { recipes: Vec<String> },

    /// A required chore parameter was not supplied via argv.
    ///
    /// `cook CHORE_NAME` without the expected positional argument.
    #[error(
        "chore '{chore}' requires parameter '{name}' (declared at line {line}); \
         supply it as a positional argument"
    )]
    ChoreParamMissing {
        chore: String,
        name: String,
        line: usize,
    },

    /// More positional arguments were supplied than the chore declares parameters.
    #[error(
        "chore '{chore}' takes {declared} parameter(s) but {supplied} positional argument(s) \
         were supplied"
    )]
    ChoreTooManyArgv {
        chore: String,
        declared: usize,
        supplied: usize,
        /// COOK-36 Task 9: when declared=0 and supplied=1, this is the
        /// unmatched argv that the migration-hint diagnostic refers to.
        /// Empty in all other cases.
        first_unmatched: String,
    },

    /// A non-chore recipe received positional argv (not permitted; use @PRESET).
    #[error(
        "recipe '{name}': recipes do not take parameters; received {supplied} positional \
         argument(s) (use '@PRESET' to select a config preset)"
    )]
    RecipeWithArgv { name: String, supplied: usize },

    /// A `+NAME` variadic received zero argv elements (requires at least one).
    #[error(
        "chore '{chore}' requires one or more values for variadic '+{name}' (declared at line {line}); \
         supply at least one, or change to '*{name}' to allow zero"
    )]
    ChoreVariadicEmpty { chore: String, name: String, line: usize },

    /// A Lua-expression default for a chore parameter raised a Lua error
    /// when evaluated at invocation time.
    #[error(
        "chore '{chore}': default for parameter '{name}' raised a Lua error \
         (defined at line {line}): {message}"
    )]
    ChoreParamDefaultLuaError {
        chore: String,
        name: String,
        line: usize,
        message: String,
    },

    /// A Lua-expression default for a chore parameter returned a non-string value.
    #[error(
        "chore '{chore}': default for parameter '{name}' must evaluate to a string; \
         got {ty} (defined at line {line})"
    )]
    ChoreParamDefaultLuaNonString {
        chore: String,
        name: String,
        line: usize,
        ty: String,
    },

    /// COOK-64 §22.5.9: an `ingredients <probe>` source names a probe `KEY` that was
    /// never declared via `cook.probe(...)`. Surfaced by the register pre-pass.
    #[error("recipe '{recipe}': ingredients <probe> source names probe '{key}' but no such probe was declared")]
    ForEachProbeUndeclared { recipe: String, key: String },

    /// COOK-64 §22.5.9: an `ingredients <probe>`-feeding probe's `produce` raised an error
    /// when evaluated by the pre-pass (before any recipe body ran).
    #[error("probe '{key}' feeds an ingredients <probe> source but its produce raised: {message}")]
    ForEachProbeProduceFailed { key: String, message: String },

    /// COOK-64 §22.5.9: an `ingredients <probe>` source resolved to a non-array value.
    /// `selector` names the resolved location (`KEY` or `KEY:FIELD`); `shape`
    /// is the JSON value-kind that was found instead of a sequence.
    #[error(
        "ingredients <probe> source '{selector}' must resolve to an array; got {shape} \
         (an ingredients <probe> driver iterates the array's members; §22.5.9)"
    )]
    ForEachNotArray { selector: String, shape: String },

    /// COOK-64 §22.5.9: an `ingredients <probe>`-feeding probe declares a file input that
    /// is produced by a recipe in this Cookfile — i.e. a build artifact. An
    /// `ingredients <probe>` source MUST be statically evaluable (it is resolved before
    /// any recipe runs), so an artifact dependency is rejected.
    #[error(
        "probe '{key}' feeds an ingredients <probe> source but depends on build artifact '{path}'; \
         sources must be statically evaluable"
    )]
    ForEachProbeArtifactDep { key: String, path: String },
}

/// One site at which a recipe name was registered during a
/// `register_cookfile` pass. Carried inside [`RegisterError::RecipeCollision`]
/// so callers (e.g. the CLI's `cmd_run`) can render a diagnostic naming
/// both the surface declaration and the register-phase Lua call.
#[derive(Debug, Clone)]
pub struct RegistrationSite {
    /// Line number within the Cookfile where the registration was recorded.
    pub line: usize,
    /// What kind of registration this site is.
    pub kind: RegistrationSiteKind,
}

/// The flavor of a [`RegistrationSite`]. Distinguishes codegen-emitted
/// surface declarations (`recipe NAME` / `chore NAME` blocks) from
/// register-phase `cook.recipe(...)` calls so collision diagnostics can
/// name both sides of the conflict precisely.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegistrationSiteKind {
    /// Surface `recipe NAME` block (codegen-emitted via
    /// `cook.__register_surface`; SHI-222 Phase 3 Task 3.1).
    SurfaceRecipe,
    /// Surface `chore NAME` block (codegen-emitted via
    /// `cook.__register_surface_chore`; SHI-222 Phase 3 Task 3.1).
    SurfaceChore,
    /// `cook.recipe(...)` call from a `register` block, top-level module
    /// call, or wrapper Lua function (e.g. `cook_cc.bin`).
    Dynamic,
}

impl RegistrationSiteKind {
    /// Short, human-readable label used inside collision diagnostics.
    fn label(self) -> &'static str {
        match self {
            RegistrationSiteKind::SurfaceRecipe => "surface recipe",
            RegistrationSiteKind::SurfaceChore => "surface chore",
            RegistrationSiteKind::Dynamic => "cook.recipe call",
        }
    }
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
    /// COOK-96: per-member dep-output paths recorded by cook.dep_output_member,
    /// drained by the NEXT cook.add_unit so each fan-out member's unit folds ONLY
    /// its own member's upstream paths (unlike step_group_dep_input_paths, which is
    /// step-group-wide). Cleared on drain, not at step-group close.
    pub pending_member_dep_input_paths: Vec<String>,
    /// True while the register-phase body of a chore is executing.
    /// `cook.add_unit` raises a Lua error if `cache = true` is passed
    /// while this flag is set (§{chores.no-caching}).
    pub current_chore_active: bool,
    /// Lua source prelude prepended to every `lua_code` unit captured inside
    /// the current chore body. Set by the engine at body-invocation time when
    /// chore parameter values are bound (COOK-36 Task 4); empty for all other
    /// recipe/chore bodies (where params are either absent or not targeted).
    ///
    /// Example value for `chore greet msg` invoked with argv `["world"]`:
    ///   `local msg = "world"\n`
    ///
    /// `cook.add_unit` prepends this string to `lua_code` before storing the
    /// `LuaChunk` payload so the execute-phase worker sees the resolved locals.
    pub chore_param_prelude: String,
    /// The fully-qualified name of the recipe currently executing in
    /// the register phase (e.g. "lib.build" for an imported recipe).
    /// Set just before the recipe body function is called; cleared after.
    /// Used by `cook.add_test` to default `suite` to the enclosing
    /// recipe's name when the caller omits the field (CS-0061 §3.2).
    pub current_recipe: Option<String>,
    /// The BARE (unqualified) name of the recipe currently executing in
    /// the register phase — the local `topo`-order name, stamped alongside
    /// `current_recipe` (which is qualified) at the same point in
    /// `engine.rs`. `cook.require_recipe`'s self-reference check (Standard
    /// §22.8, CS-0144) MUST compare against this field, not
    /// `current_recipe`: the API's argument is bare per the settled design
    /// ruling, so comparing it against the qualified `current_recipe`
    /// would make the self-check silently never fire under an import
    /// prefix. The `None`/unstamped signal is shared with `current_recipe`
    /// and unaffected by this field — only the self-comparison and the
    /// cycle-path rendering (`BodyDriver::path` in `engine.rs`) need the bare
    /// form.
    pub current_recipe_bare: Option<String>,
    /// Recipe names accumulated by `cook.require_recipe` calls within the
    /// current recipe body (Standard §22.8, CS-0144). Order-preserving,
    /// de-duplicated by the API itself. This field is only the accumulator:
    /// `engine.rs` drains it after each body returns, forcing each named
    /// recipe's body and merging the names into that recipe's `requires`.
    pub dynamic_requires: Vec<String>,
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
            pending_member_dep_input_paths: Vec::new(),
            current_chore_active: false,
            current_recipe: None,
            current_recipe_bare: None,
            dynamic_requires: Vec::new(),
            chore_param_prelude: String::new(),
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

// Re-exports for convenience.
//
// `RegistrationSite` and `RegistrationSiteKind` are part of the public
// API shipped with `RegisterError::RecipeCollision` — they're defined at
// the crate root above and so are already accessible as
// `cook_register::RegistrationSite{,Kind}`. No explicit re-export needed.
pub use capture::RegistrationSource;
pub use dep_output_api::{SharedMemberOutputs, SharedTerminalOutputs};
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
    pub warnings: Vec<String>,
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
    /// Declared chore parameters (empty for normal recipes), carried through
    /// so `cook menu` can render `chore greet caller who="world"` instead of
    /// a bare name.
    pub params: Vec<crate::capture::ChoreParamMeta>,
    /// The module-qualified function name that minted this recipe (e.g.
    /// `"cook_pnpm.workspace"`), when the author opted in via the
    /// `cook.recipe(name, {origin = "..."}, body)` field. `None` when no
    /// `origin` was supplied, and always `None` for surface (`recipe NAME` /
    /// `chore NAME`) registrations — see `capture::parse_origin_meta`.
    pub origin: Option<String>,
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
