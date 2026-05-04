//! Fingerprint and cache-key computation for the Cook build system.
//!
//! This crate is the "what changed?" surface: pure functions that compute
//! content hashes, env contributions, machine/tool identity, and the
//! SHA-256 cache keys that address artifacts in any backend. It also defines
//! the `CacheBackend` trait — the seam the persistence layer (filesystem,
//! Cook Cloud, etc.) implements.
//!
//! `cook-cache` provides the v3 filesystem backend and the recipe-cache
//! manager built on top of these primitives.

pub mod backend;
pub mod check;
pub mod context;
pub mod envkey;
pub mod record;

use std::collections::BTreeSet;
use std::path::Path;

pub use backend::{
    artifact_key, cloud_key, ArtifactMeta, BackendError, BackendResult, CacheBackend, CloudKey,
    CloudKeyInputs,
};
pub use check::{
    hash_env, hash_file, needs_rebuild_cook, needs_rebuild_plate, stat_mtime, RebuildReason,
    RebuildResult, RestoreCtx,
};
pub use context::{ExecutionContext, MachineIdentity, ToolHash};
pub use envkey::{env_contribution, EnvDenylist};
pub use record::{FileRecord, StepEntry, CACHE_VERSION};

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
