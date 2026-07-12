//! Workspace-wide aggregation of per-Cookfile `RegisteredCookfile`.
//!
//! Each Cookfile (root + each import) is registered independently by
//! `cook_register::register_cookfile`. The aggregation merges names
//! (with qualified prefix), units (keyed by qualified recipe name),
//! probes, and `final_env` into a single workspace-wide view that the
//! engine's DAG builder and executor consume.
//!
//! Per-import merge logic lands in Phase 5 Task 5.1 (CS-0077); this
//! commit defines the type only.

use std::collections::BTreeMap;
use std::path::PathBuf;

use cook_contracts::{ProbeUnit, RecipeUnits};
use cook_register::RegisteredRecipePub;

/// Workspace-level container that aggregates per-Cookfile registration
/// results into a single view. Names are qualified with their import
/// prefix (root Cookfile uses the empty prefix `""`).
pub struct RegisteredWorkspace {
    pub warnings: Vec<String>,
    /// All recipes across all Cookfiles, names qualified with their import prefix.
    pub names: Vec<RegisteredRecipePub>,
    /// Per-recipe captured units, keyed by fully-qualified recipe name.
    pub units_by_recipe: BTreeMap<String, RecipeUnits>,
    /// Probes keyed by qualified probe key.
    pub probes: BTreeMap<String, ProbeUnit>,
    /// Per-Cookfile final env. Imports do not inherit the root's config writes.
    ///
    /// Keyed by qualified prefix (`""` for root). Value is a `BTreeMap` for
    /// parity with `RegisteredCookfile.final_env` and the project-wide
    /// "serialized collections are sorted" rule.
    pub final_env_by_cookfile: BTreeMap<String, BTreeMap<String, String>>,
    /// Per-Cookfile working directory, keyed by qualified prefix (`""` for root).
    pub working_dir_by_prefix: BTreeMap<String, PathBuf>,
    /// Per-Cookfile `alias_dirs` (for `cook.dep_output` rewriting), keyed by
    /// qualified prefix (`""` for root) and then by alias name.
    pub alias_dirs_by_prefix: BTreeMap<String, BTreeMap<String, PathBuf>>,
    /// Snapshot of the register session's terminal-outputs map (recipe
    /// qualified-name → terminal output paths), taken after every Cookfile
    /// has registered. Threaded to the execute-phase worker VMs so
    /// `cook.dep_output` / `cook.dep_output_list` (§24.7, "Both") resolve
    /// there. The map is closed before execute phase starts, so a snapshot
    /// taken at the end of `register_workspace` is sound.
    pub terminal_outputs: BTreeMap<String, Vec<String>>,
}
