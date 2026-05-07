//! Build artifact cache backend for the Cook build system.
//!
//! This crate is the persistence layer: `LocalBackend` implements
//! `cook_fingerprint::CacheBackend` against the filesystem, `RecipeCache`
//! is the on-disk recipe-cache file format, and `ThreadSafeCacheManager`
//! manages writes during a build.
//!
//! Fingerprint computation (hashing, env denylist, machine identity, cloud
//! key derivation, rebuild-decision logic) lives in `cook-fingerprint`. This
//! crate re-exports the most-used items for back-compat with existing
//! `cook_cache::*` call sites.

pub mod backend;
pub mod cache_ctx;
pub mod cloud_backend;
pub mod cloud_config;
pub mod depfile;
pub mod manager;
pub mod store;
pub mod test_cache;

pub use depfile::{parse_make_depfile, DepfileError};

// Re-exports of fingerprint-side items for back-compat with existing call sites.
// New code should import from `cook_fingerprint` directly.
pub use cook_fingerprint::check;
pub use cook_fingerprint::context;
pub use cook_fingerprint::envkey;
pub use cook_fingerprint::{
    hash_env, hash_file, hash_str, needs_rebuild_cook, needs_rebuild_plate, resolve_glob,
    stat_mtime, CacheBackend, RebuildReason, RebuildResult, RestoreCtx,
};

pub use backend::LocalBackend;
pub use cache_ctx::CacheContext;
pub use cloud_backend::CloudBackend;
pub use cloud_config::{CloudConfig, CloudConfigError};
pub use manager::{collect_records_public, CacheState, RecordError, SharedCacheState, ThreadSafeCacheManager};
pub use store::{FileRecord, RecipeCache, StepEntry, CACHE_VERSION};
pub use test_cache::{TestCache, TestCacheEntry, TestCacheOutcome};
