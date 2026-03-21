//! cook-luaotp — a pool of worker threads, each with its own Lua VM,
//! that executes work items (shell commands, Lua chunks, tests).

mod pool;
mod fs_api;
mod path_api;
mod platform_api;

pub use pool::{WorkerPool, WorkItem, WorkResult, TestOutput};
