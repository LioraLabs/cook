//! CS-0171: per-unit facts hung onto the graph's nodes.
//!
//! The graph knows shape and nothing else. Everything about whether a unit
//! will actually run comes from `cook_engine::why`, and everything about what
//! it cost comes from the observations `cook_engine::why` reads back (this
//! named `cook_engine::timings`, a module that no longer exists). This module
//! is the seam: a map
//! from `(recipe, cache_key)` to what those two know, which
//! [`crate::emit::aggregate`] rolls up as it collapses the graph.
//!
//! Keeping the seam explicit is the point. The graph crate deliberately holds
//! no cache logic of its own — the previous arrangement, where it ran a
//! private local-index-only rebuild check, is exactly the duplication CS-0171
//! removes.

use std::collections::BTreeMap;

/// What is known about one work unit, from outside this crate.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UnitFacts {
    /// The unit will be served from cache rather than run. Either tier counts:
    /// a locally-warm unit does not rebuild just because the shared store has
    /// never heard of it.
    pub served: bool,
    /// Wall time the unit was last observed to take, if any retained build
    /// timed it. `None` is "never observed", which is not the same as fast.
    pub observed_ms: Option<u64>,
    /// How many retained builds back that observation came from; `0` is the
    /// most recent build. Carried so a node can report how stale the number
    /// feeding it is — an observation from fifteen builds ago deserves less
    /// weight than one from the last run, and the reader can only know that if
    /// told.
    pub observed_builds_ago: usize,
}

/// Facts for every classified unit, keyed by `(recipe, cache_key)`.
///
/// A unit absent from the map is *unclassified* rather than assumed either
/// way: a non-cacheable node (a bare shell step, a chore body) has no cache
/// verdict to report, and silently counting it as a rebuild would inflate
/// every tally on the page.
#[derive(Debug, Clone, Default)]
pub struct Annotations {
    by_unit: BTreeMap<(String, String), UnitFacts>,
}

impl Annotations {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&mut self, recipe: &str, cache_key: &str, facts: UnitFacts) {
        self.by_unit
            .insert((recipe.to_string(), cache_key.to_string()), facts);
    }

    pub fn get(&self, recipe: &str, cache_key: &str) -> Option<&UnitFacts> {
        self.by_unit
            .get(&(recipe.to_string(), cache_key.to_string()))
    }

    /// Look up the facts for a graph node, which requires both a recipe and a
    /// cache key. A file node has neither; a non-cacheable unit has no key.
    pub fn for_node(&self, node: &crate::dag_data::NodeData) -> Option<&UnitFacts> {
        let recipe = node.recipe.as_deref()?;
        let key = node.cache_key.as_deref()?;
        self.get(recipe, key)
    }

    pub fn is_empty(&self) -> bool {
        self.by_unit.is_empty()
    }

    pub fn len(&self) -> usize {
        self.by_unit.len()
    }
}

#[cfg(test)]
#[path = "tests/annotate_tests.rs"]
mod tests;
