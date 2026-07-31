//! cook-luaotp — a pool of worker threads, each with its own Lua VM,
//! that executes work items (shell commands, Lua chunks, tests).
//!
//! The shared `fs.*`, `path.*`, and `cook.platform.*` Lua API tables
//! come from `cook-lua-stdlib` (CS-0044) so register-phase and
//! execute-phase VMs see byte-identical behaviour for these surfaces.

mod pool;
pub(crate) mod probe_value;
mod store;

pub use pool::{resolve_probe_sigils, WorkerPool, WorkItem, WorkResult, ProbeOutput, WorkerDepOutputs};
pub use store::ProbeValueStore;
