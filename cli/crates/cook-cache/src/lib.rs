//! Build artifact caching for the Cook build system.
//!
//! This crate handles hash computation, file-based cache storage, and
//! cache hit/miss logic for incremental builds.

pub mod backend;
pub mod check;
pub mod context;
pub mod envkey;
pub mod manager;
pub mod store;

use std::collections::BTreeSet;
use std::path::Path;

pub use check::{
    hash_env, hash_file, hash_secondary_inputs, needs_rebuild_cook, needs_rebuild_plate,
    stat_mtime, RebuildReason, RebuildResult,
};
pub use manager::{CacheState, SharedCacheState, ThreadSafeCacheManager};
pub use store::{FileRecord, RecipeCache, StepEntry, CACHE_VERSION};
pub use backend::{ArtifactMeta, BackendError, BackendResult, CacheBackend, CloudKey};
pub use context::{ExecutionContext, MachineIdentity, ToolHash};
pub use envkey::{env_contribution, EnvDenylist};

/// Hash a string (for command templates, env vars, etc.)
pub fn hash_str(s: &str) -> u64 {
    xxhash_rust::xxh3::xxh3_64(s.as_bytes())
}

/// Helper to resolve a glob pattern into a set of files.
pub fn resolve_glob(root: &Path, pattern: &str) -> BTreeSet<String> {
    let full_pattern = root.join(pattern);
    let prefix = root.to_string_lossy().to_string();

    let paths = match glob::glob(&full_pattern.to_string_lossy()) {
        Ok(p) => p,
        Err(_) => return BTreeSet::new(),
    };

    paths
        .filter_map(Result::ok)
        .filter_map(|p| {
            let path_str = p.to_string_lossy().to_string();
            Some(
                path_str
                    .strip_prefix(&prefix)
                    .unwrap_or(&path_str)
                    .trim_start_matches('/')
                    .to_string(),
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hash_str_deterministic() {
        let h1 = hash_str("hello");
        let h2 = hash_str("hello");
        assert_eq!(h1, h2);
    }

    #[test]
    fn test_hash_str_differs() {
        let h1 = hash_str("hello");
        let h2 = hash_str("world");
        assert_ne!(h1, h2);
    }
}
