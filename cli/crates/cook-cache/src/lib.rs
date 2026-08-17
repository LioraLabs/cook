//! Build artifact cache backend for the Cook build system.
//!
//! This crate is the cache's IO half: everything that must ASK THE FILESYSTEM
//! to answer a cache question. `LocalBackend` implements `crate::CacheBackend`
//! against the filesystem, `RecipeCache` is the on-disk recipe-cache file
//! format, `ThreadSafeCacheManager` manages writes during a build, and `check`,
//! `probe`, `resolve` and `statmemo` are the rest of what reads the tree.
//!
//! The effect-free half is `cook-contracts`: cache-key composition, the env
//! denylist, the rebuild decision, eviction planning. The split is the one the
//! constitution draws. A decision that can be made from values alone lives
//! there; what needs a syscall to make it lives here. This crate re-exports the
//! most-used of those items so existing `cook_cache::*` call sites keep working.
//!
//! COOK-418 dissolved `cook-fingerprint` along exactly that line and gave this
//! crate its IO half. The crate is gone from the workspace manifest; prose
//! elsewhere still names it, which is stale rather than a second home.

pub mod backend;
pub mod cap;
// COOK-418: cook-fingerprint's IO half. `cas_backend` is the CacheBackend
// trait and its config; `check` is cache validity and artifact restore;
// `probe`, `statmemo` and `resolve` are the rest of what asks the filesystem.
// COOK-414: the crate's two file hashes are `check::hash_file` (xxh3, local
// content identity) and `probe::hash_file_sha256` (identity that leaves the
// machine); both are pinned to golden vectors, and changing what either
// computes invalidates every cache in existence.
pub mod cache_ctx;
pub mod cas_backend;
pub mod check;
pub mod cloud_backend;
pub mod cloud_config;
pub mod depfile;
pub mod index_bin;
pub mod manager;
pub mod probe;
pub mod resolve;
pub mod statmemo;
pub mod store;

pub use depfile::{DepfileError, parse_make_depfile};

// COOK-418: what cook-fingerprint used to re-export, now owned here or
// forwarded from contracts. The crate is gone; these are the paths its
// consumers used.
pub use cas_backend::{BackendConfig, BackendError, BackendResult, CacheBackend};
pub use check::{
    FetchOutcome, RebuildReason, RebuildResult, RestoreCtx, fetch_by_key, fetch_observation,
    hash_env, hash_file, hash_input_paths, hash_reader, needs_rebuild_cook,
    read_discovered_input_sets, read_module_input_sets, shared_observation, stat_mtime,
};
pub use cook_contracts::cache::cas::{
    ArtifactMeta, CloudKey, CloudKeyInputs, DISCOVERED_INPUT_SETS_CAP, DISCOVERED_INPUT_SETS_INDEX,
    DISCOVERED_INPUT_SETS_PATH, DISCOVERED_INPUTS_MANIFEST_INDEX, DISCOVERED_INPUTS_MANIFEST_PATH,
    DeterminantManifest, EvictCandidate, MODULE_INPUT_SETS_INDEX, MODULE_INPUT_SETS_PATH,
    OBSERVATION_INDEX, OBSERVATION_PATH, artifact_key, cloud_key, recipe_namespace,
};
pub use cook_contracts::cache::cas::{
    decode_path_sets, encode_path_sets, merge_path_set, path_set_candidates,
};
pub use cook_contracts::cache::step::CACHE_VERSION as STEP_CACHE_VERSION;
pub use cook_contracts::consumes::ConsumesFilter;
pub use cook_contracts::envkey::{EnvDenylist, env_contribution};
pub use cook_contracts::evict::{
    DEFAULT_LOW_WATER, EvictPlan, EvictPolicy, SIZE_SWEEP_EXEMPT_KINDS, is_size_sweep_exempt,
    plan_eviction,
};
pub use cook_contracts::pathlaw::{has_glob_meta, is_dir_output, is_terminal_output};
pub use cook_contracts::{consumes, envkey, evict, hash_str};
pub use probe::{
    ProbeInputDigests, hash_file_sha256, resolve_probe_input_digests, resolve_tool_path,
    tool_identity,
};
pub use resolve::{
    empty_dirs_under, normalize_glob_pattern, reconcile_dir_output, resolve_declared_inputs,
    resolve_gather_glob, resolve_glob,
};
pub use statmemo::{stat_mtime_memo, tool_hash_memo};

pub use backend::LocalBackend;
pub use cache_ctx::CacheContext;
pub use cloud_backend::CloudBackend;
pub use cloud_config::{CloudConfig, CloudConfigError};
pub use manager::{RecordError, ThreadSafeCacheManager, collect_records};
pub use store::{CACHE_VERSION, FileRecord, RecipeCache, StepEntry};
