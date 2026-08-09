//! cook-luaotp — a pool of worker threads, each with its own Lua VM,
//! that executes work items (shell commands, Lua chunks, tests).
//!
//! The shared `fs.*`, `path.*`, and `cook.platform.*` Lua API tables
//! come from `cook-lua-stdlib` (CS-0044) so register-phase and
//! execute-phase VMs see byte-identical behaviour for these surfaces.
//!
//! The probe-value store and `$<key>` substitution are NOT here: neither
//! touches a VM, and both are the reading half of a file format whose
//! writer is `cook-probe` (COOK-422). A consumer wants
//! `cook_probe::store::ProbeValueStore` and
//! `cook_probe::sigil::resolve_probe_sigils` directly, not a courier's
//! re-export of them.

mod pool;
pub(crate) mod probe_value;

pub use pool::{WorkerPool, WorkItem, WorkResult, ProbeOutput, WorkerDepOutputs};
