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
}

/// Shared state accumulated during capture-mode execution.
///
/// Uses Rc<RefCell<>> and is therefore !Send. This is intentional —
/// registration runs on a single thread with a single Lua VM.
pub struct CaptureState {
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
    /// Probe units registered via `cook.probe(key, opts)` (§22.5.2).
    pub probes: Vec<cook_contracts::ProbeUnit>,
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

impl CaptureState {
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
            probes: Vec::new(),
            step_group_dep_refs: Vec::new(),
            step_group_dep_input_paths: Vec::new(),
            current_chore_active: false,
            current_recipe: None,
        }
    }
}

impl Default for CaptureState {
    fn default() -> Self {
        Self::new()
    }
}

pub type SharedCaptureState = Rc<RefCell<CaptureState>>;

/// Hash a string using xxh3 (for command templates, env vars, etc.)
pub fn hash_str(s: &str) -> u64 {
    xxhash_rust::xxh3::xxh3_64(s.as_bytes())
}

// Re-exports for convenience
pub use dep_output_api::SharedTerminalOutputs;
pub use engine::Registry;
