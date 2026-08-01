//! Build artifact cache backend for the Cook build system.
//!
//! This crate is the persistence layer: `LocalBackend` implements
//! `crate::CacheBackend` against the filesystem, `RecipeCache`
//! is the on-disk recipe-cache file format, and `ThreadSafeCacheManager`
//! manages writes during a build.
//!
//! Fingerprint computation (hashing, env denylist, machine identity, cloud
//! key derivation, rebuild-decision logic) lives in `cook-fingerprint`. This
//! crate re-exports the most-used items for back-compat with existing
//! `cook_cache::*` call sites.

pub mod backend;
pub mod cap;
// COOK-418: cook-fingerprint's IO half. `cas_backend` is the CacheBackend
// trait and its config; `check` is cache validity and artifact restore;
// `probe`, `statmemo` and `resolve` are the rest of what asks the filesystem.
pub mod cas_backend;
pub mod check;
pub mod probe;
pub mod resolve;
pub mod statmemo;
pub mod cache_ctx;
pub mod cloud_backend;
pub mod cloud_config;
pub mod depfile;
pub mod index_bin;
pub mod manager;
pub mod store;

pub use depfile::{parse_make_depfile, DepfileError};

// COOK-418: what cook-fingerprint used to re-export, now owned here or
// forwarded from contracts. The crate is gone; these are the paths its
// consumers used.
pub use cook_contracts::{consumes, context, envkey, evict, hash_str};
pub use cook_contracts::cache::cas::{
    artifact_key, cloud_key, recipe_namespace, ArtifactMeta, CloudKey, CloudKeyInputs,
    DeterminantManifest, EvictCandidate, DISCOVERED_INPUTS_MANIFEST_INDEX,
    DISCOVERED_INPUTS_MANIFEST_PATH, DISCOVERED_INPUT_SETS_CAP, DISCOVERED_INPUT_SETS_INDEX,
    DISCOVERED_INPUT_SETS_PATH, OBSERVATION_INDEX, OBSERVATION_PATH,
};
pub use cook_contracts::cache::step::{CACHE_VERSION as STEP_CACHE_VERSION};
pub use cook_contracts::consumes::ConsumesFilter;
pub use cook_contracts::context::{compute_probe_fingerprint, ProbeFingerprintInputs};
pub use cook_contracts::envkey::{env_contribution, EnvDenylist};
pub use cook_contracts::evict::{
    is_size_sweep_exempt, plan_eviction, EvictPlan, EvictPolicy, DEFAULT_LOW_WATER,
    SIZE_SWEEP_EXEMPT_KINDS,
};
pub use cook_contracts::pathlaw::{has_glob_meta, is_dir_output, is_terminal_output};
pub use cas_backend::{BackendConfig, BackendError, BackendResult, CacheBackend};
pub use check::{
    fetch_by_key, fetch_observation, hash_env, hash_file, hash_input_paths, hash_reader,
    needs_rebuild_cook, read_discovered_input_sets, shared_observation, stat_mtime, FetchOutcome,
    RebuildReason, RebuildResult, RestoreCtx,
};
pub use probe::{resolve_probe_inputs, resolve_tool_path, tool_identity};
pub use resolve::{
    empty_dirs_under, normalize_glob_pattern, reconcile_dir_output, resolve_declared_inputs,
    resolve_glob, resolve_ingredient_glob,
};
pub use statmemo::stat_mtime_memo;

pub use backend::LocalBackend;
pub use cache_ctx::CacheContext;
pub use cloud_backend::CloudBackend;
pub use cloud_config::{parse_size, CloudConfig, CloudConfigError, SIZE_LITERAL_HELP};
pub use manager::{collect_records, RecordError, ThreadSafeCacheManager};
pub use store::{FileRecord, RecipeCache, StepEntry, CACHE_VERSION};
