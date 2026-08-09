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

/// Every name reachable from `seeds` through `deps`, seeds included.
///
/// `deps` maps a registered name to the names it requires. A name absent from
/// `deps` is not registered here and is skipped rather than added: the caller's
/// map is the universe, and a dep naming something outside it — a cross-Cookfile
/// reference in a per-Cookfile map, a typo the analyzer will reject with a
/// better diagnostic — is not this function's to resolve. Seeds get the same
/// treatment, so an unknown seed contributes nothing.
///
/// The decision this owns is which chore bodies a register pass may invoke
/// (Standard §7.6, CS-0218). `cook-register` asks it per Cookfile, over that
/// Cookfile's own `requires` map and its dispatch target; `cook-plan` asks it
/// over the composed workspace map, to tell each Cookfile which of its names
/// an edge from somewhere else reaches. The two MUST agree — a name one calls
/// reachable and the other does not is a chore that either runs unasked or
/// registers zero units while the build waits for it — and they agreed by
/// having been written twice, in different crates, until this became the one
/// implementation.
pub fn reachable_from(
    deps: &BTreeMap<String, Vec<String>>,
    seeds: impl IntoIterator<Item = String>,
) -> std::collections::BTreeSet<String> {
    let mut reachable: std::collections::BTreeSet<String> =
        std::collections::BTreeSet::new();
    let mut stack: Vec<String> = seeds
        .into_iter()
        .filter(|s| deps.contains_key(s))
        .collect();
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

#[cfg(test)]
#[path = "tests/recipe_tests.rs"]
mod tests;
