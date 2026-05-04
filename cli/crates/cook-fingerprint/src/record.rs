//! Persistent fingerprint state types.
//!
//! `FileRecord` and `StepEntry` describe the recorded fingerprint of inputs,
//! outputs, command, context, and env for a single step. The `CACHE_VERSION`
//! constant tags every persisted RecipeCache so a schema change is rejected
//! on load (see `cook-cache::store`).

use serde::{Deserialize, Serialize};

/// Fingerprint schema version. Bump on any breaking change to `StepEntry` /
/// `FileRecord` / cache key composition.
pub const CACHE_VERSION: u32 = 3;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct StepEntry {
    pub inputs: Vec<FileRecord>,
    pub outputs: Vec<FileRecord>,
    pub command_hash: u64,
    pub context_hash: u64,
    pub env_contribution: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FileRecord {
    pub path: String,
    pub mtime: u64,
    pub hash: u64,
}
