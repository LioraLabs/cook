//! Shared kernel types for the Cook build system.
//!
//! This crate defines Cook's shared language as plain data and pure rules.
//! Runtime mechanisms and policy belong in the crates that consume these
//! contracts.

pub mod accessor;
pub mod cache;
pub mod consumes;
pub mod context;
pub mod envkey;
pub mod evict;
pub mod hash;
pub mod layout;
pub mod captured_stream;
pub mod command_failure;
pub mod lua_error;
pub mod lua_string;
pub mod member;
pub mod naming;
pub mod output;
pub mod pathlaw;
pub mod probe;
pub mod probe_key;
pub mod quoting;
pub mod recipe;
pub mod render;
pub mod shell_block;
pub mod sigil;
pub mod registration;
pub mod step;
pub mod unit;

pub use accessor::ACCESSORS;
pub use hash::hash_str;
pub use cache::{CacheMeta, DiscoveredInputs, Sharing};
pub use captured_stream::CapturedStream;
pub use command_failure::CommandFailure;
pub use output::{OutputChunk, OutputStream};
pub use probe::value as probe_value;
pub use probe::{ProbeInputs, ProbeUnit};
pub use recipe::RecipeUnits;
pub use registration::{REGISTER_SURFACE_CHORE_NAME, REGISTER_SURFACE_NAME};
pub use step::StepKind;
pub use unit::{CapturedUnit, DepKind, WorkPayload};
