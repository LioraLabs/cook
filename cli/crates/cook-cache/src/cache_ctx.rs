//! CacheContext — single struct aggregating everything the cache layer needs:
//! env denylist, backend, and project config. Built once per `cook build`
//! invocation in cook-engine's run.rs and threaded down.

use std::path::PathBuf;
use std::sync::Arc;

use crate::{CacheBackend, EnvDenylist};

use crate::cloud_config::CloudConfig;

#[derive(Clone)]
pub struct CacheContext {
    pub denylist: Arc<EnvDenylist>,
    pub backend: Arc<dyn CacheBackend>,
    pub cloud_config: Arc<CloudConfig>,
    pub project_root: PathBuf,
    pub project_id: String,
    /// When false, the executor suppresses ALL shared-store uploads for this
    /// invocation (read-only / publish-off client mode, COOK-168). Fetch,
    /// drift-restore, and `pinned` cold-fetch are unaffected. Resolved from
    /// `[cloud] publish` (default true) AND `--no-publish` / `COOK_NO_PUBLISH`.
    pub publish_enabled: bool,
    pub replay_logs: bool,
}

/// The one policy for puts the shared store refused or never received: the
/// first failure of an invocation is named in full, the rest are counted for
/// one summary line per writer, and none of them fails the build. The store is
/// unreachable or refusing, so a line per failure would only repeat the first.
#[derive(Default)]
pub struct FailedPuts {
    told: std::sync::atomic::AtomicBool,
    untold: std::sync::atomic::AtomicU64,
}

impl FailedPuts {
    /// For a writer that runs after another in the same invocation (the run
    /// after the register pass): when that one has told its first failure,
    /// this one only counts.
    pub fn after(earlier_told: bool) -> Self {
        Self { told: earlier_told.into(), untold: 0.into() }
    }

    /// Whether a failure has been told.
    pub fn told(&self) -> bool {
        self.told.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Records one failed upload of `what`; only the invocation's first is told.
    pub fn record(&self, what: &str, err: &dyn std::fmt::Display) {
        if self.told.swap(true, std::sync::atomic::Ordering::Relaxed) {
            self.untold.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        } else {
            tracing::warn!("cache upload for {what} failed: {err}; not published");
        }
    }

    /// Tells how many of this writer's failures went untold, as `noun` (e.g. `unit(s)`).
    pub fn summarize(&self, noun: &str) {
        let more = self.untold.load(std::sync::atomic::Ordering::Relaxed);
        if more > 0 {
            tracing::warn!("cache upload failed for {more} more {noun}; not published");
        }
    }
}
