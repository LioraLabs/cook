//! cook-lua-stdlib — the shared Cook Lua API surface installed in both
//! the register-phase VM (`cook-register`) and the execute-phase worker
//! VMs (`cook-luaotp`).
//!
//! Exposes three tables:
//!
//! - `fs.*` — filesystem helpers, working-directory rooted (§6.5).
//! - `path.*` — pure string path manipulation (§6.6).
//! - `cook.platform.*` — host OS / architecture identifiers.
//!
//! All three are specified by the Cook Standard (`standard/src/content/
//! docs/06-cook-lua-api.mdx`) as **Phase: Both**. Sharing one
//! implementation across both VMs is what realizes that spec contract —
//! see CS-0044.
//!
//! # Working-directory abstraction
//!
//! `fs.*` resolves relative paths against a working directory. The two
//! callers differ in how that directory is sourced:
//!
//! - `cook-register` knows the cwd at registration time and never
//!   changes it for the lifetime of the VM (one VM per recipe).
//! - `cook-luaotp` reuses one VM across many work items from
//!   potentially different Cookfiles (CS-0017 multi-Cookfile imports),
//!   so the cwd is updated per-item via a shared `Arc<Mutex<PathBuf>>`.
//!
//! [`WorkingDirSource`] abstracts this so a single `fs_api`
//! implementation handles both call patterns.

pub mod fs_api;
pub mod path_api;
pub mod platform_api;
pub mod sandbox;
pub mod shell_guard;

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

pub use fs_api::{register_fs_api, register_fs_api_with_sandbox};
pub use path_api::register_path_api;
pub use platform_api::register_platform_api;
pub use sandbox::{SandboxPolicy, SandboxSource};
pub use shell_guard::install_shell_escape_guards;

/// Source of the working directory used to resolve relative paths in
/// `fs.*` calls.
///
/// `cook-register` constructs `Static` with the Cookfile's directory at
/// VM creation time. `cook-luaotp` constructs `Live` with the
/// `Arc<Mutex<PathBuf>>` it updates per work item.
///
/// `Live` resolves the cwd on every call so a worker VM that processes
/// items from multiple Cookfiles (CS-0017) sees each item's own cwd —
/// not the cwd in effect when the `fs` table was first registered.
#[derive(Clone, Debug)]
pub enum WorkingDirSource {
    /// Captured once at registration; used by `cook-register`.
    Static(PathBuf),
    /// Resolved live per call; used by `cook-luaotp`'s reusable workers.
    Live(Arc<Mutex<PathBuf>>),
}

impl WorkingDirSource {
    /// Resolve the working directory at call time.
    ///
    /// `Live`'s mutex lock is held only for the duration of the clone.
    /// Lock poisoning is treated as a hard error consistent with the
    /// pre-extraction `cook-luaotp` behavior — the slot is shared
    /// per-VM, and a poisoned slot means the worker is in an
    /// unrecoverable state.
    pub fn resolve(&self) -> PathBuf {
        match self {
            WorkingDirSource::Static(p) => p.clone(),
            WorkingDirSource::Live(slot) => slot.lock().expect("working_dir lock").clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn static_returns_captured_path() {
        let src = WorkingDirSource::Static(PathBuf::from("/tmp/static"));
        assert_eq!(src.resolve(), PathBuf::from("/tmp/static"));
        // Repeated calls return the same path.
        assert_eq!(src.resolve(), PathBuf::from("/tmp/static"));
    }

    #[test]
    fn live_reflects_post_registration_mutations() {
        let slot = Arc::new(Mutex::new(PathBuf::from("/tmp/initial")));
        let src = WorkingDirSource::Live(Arc::clone(&slot));
        assert_eq!(src.resolve(), PathBuf::from("/tmp/initial"));

        // Simulate a new work item updating the slot.
        *slot.lock().unwrap() = PathBuf::from("/tmp/updated");
        assert_eq!(
            src.resolve(),
            PathBuf::from("/tmp/updated"),
            "Live source must observe slot mutations made after registration"
        );
    }

    #[test]
    fn live_clone_shares_slot() {
        let slot = Arc::new(Mutex::new(PathBuf::from("/tmp/a")));
        let src1 = WorkingDirSource::Live(Arc::clone(&slot));
        let src2 = src1.clone();

        *slot.lock().unwrap() = PathBuf::from("/tmp/b");
        assert_eq!(src1.resolve(), PathBuf::from("/tmp/b"));
        assert_eq!(src2.resolve(), PathBuf::from("/tmp/b"));
    }
}
