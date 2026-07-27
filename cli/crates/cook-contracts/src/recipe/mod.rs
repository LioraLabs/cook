//! Registered recipe-unit collections.

use crate::{CapturedUnit, ProbeUnit};
use std::collections::BTreeMap;
use std::path::PathBuf;

/// Result of registering a single recipe.
#[derive(Debug, Clone)]
pub struct RecipeUnits {
    pub recipe_name: String,
    pub deps: Vec<String>,
    pub units: Vec<CapturedUnit>,
    pub step_groups: Vec<Vec<usize>>,
    pub working_dir: PathBuf,
    pub env_vars: BTreeMap<String, String>,
    pub terminal_outputs: Vec<String>,
    pub dep_edges: Vec<(usize, String)>,
    pub probes: Vec<ProbeUnit>,
}
