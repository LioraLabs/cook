//! cook-register: Capture-mode Lua VM for discovering work units from Cookfiles.
//!
//! Runs generated Cookfile Lua in capture mode to discover work units
//! (commands, inputs, outputs) without executing them.

pub mod capture;
pub mod context;
pub mod engine;
pub mod export_api;
pub mod fs_api;
pub mod module_cache;
pub mod module_loader;
pub mod path_api;
pub mod platform_api;
pub mod test_api;
pub mod unit_api;

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
}

impl CaptureState {
    pub fn new() -> Self {
        Self {
            inside_layer: false,
            layer_commands: Vec::new(),
            units: Vec::new(),
            current_group: None,
            step_groups: Vec::new(),
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
pub use engine::Registry;
pub use fs_api::register_fs_api;
pub use path_api::register_path_api;
