//! Recorded unit observations loaded from the fingerprint index.

use std::collections::BTreeMap;
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Observation {
    pub elapsed_ms: u64,
    pub recorded_at: u64,
    pub cause: Option<String>,
    pub log_bytes: u64,
}

#[derive(Debug, Default, Clone)]
pub struct Observations {
    by_unit: BTreeMap<(String, String), Observation>,
}

impl Observations {
    pub fn load(project_root: &Path) -> Self {
        let root = cook_contracts::layout::cache_dir(project_root);
        let mut by_unit = BTreeMap::new();
        let Ok(entries) = std::fs::read_dir(&root) else {
            return Self { by_unit };
        };
        for entry in entries.flatten() {
            let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
                continue;
            };
            let Some(encoded) = name.strip_suffix(".idx") else {
                continue;
            };
            let recipe = cook_contracts::layout::decode_index_basename(encoded);
            let Some(cache) = cook_cache::store::RecipeCache::load(&root, &recipe) else {
                continue;
            };
            for (cache_key, step) in cache.steps {
                if let Some(observed) = step.observed {
                    by_unit.insert(
                        (recipe.clone(), cache_key),
                        Observation {
                            elapsed_ms: observed.duration_ms(),
                            recorded_at: observed.recorded_at(),
                            cause: observed.cause().map(str::to_owned),
                            log_bytes: observed.log_bytes(),
                        },
                    );
                }
            }
        }
        Self { by_unit }
    }

    pub fn get(&self, recipe: &str, cache_key: &str) -> Option<&Observation> {
        self.by_unit.get(&(recipe.to_owned(), cache_key.to_owned()))
    }

    pub fn is_empty(&self) -> bool {
        self.by_unit.is_empty()
    }
}

// COOK-423: `render_ms` is deleted. It was this module's copy of the duration
// rendering, kept as a forwarding shim after COOK-392 moved the canon to
// `cook_contracts::render::duration_ms`, and it lost its last caller when
// cook-graph stopped importing it. Every renderer in the workspace now names
// the law directly. A shim is a second name for one decision, which is the
// shape CS-0198 removed; leaving one behind invites the next reader to add a
// band to whichever name they found first.
