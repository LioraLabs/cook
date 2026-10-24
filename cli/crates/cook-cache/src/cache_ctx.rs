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

/// The one policy for puts the shared store refused or never received: one
/// record per invocation. The first failure (any writer) is named in full, the
/// rest are counted per writer for one summary line each, and none of them
/// fails the build. The store is unreachable or refusing, so a line per
/// failure would only repeat the first.
#[derive(Debug, Default)]
pub struct FailedPuts {
    state: std::sync::Mutex<FailedPutsState>,
}

#[derive(Debug, Default)]
struct FailedPutsState {
    told: bool,
    /// Untold failures per writer noun, in first-seen order.
    untold: Vec<(&'static str, u64)>,
    /// Everything `record_once` has seen.
    seen: std::collections::HashSet<String>,
}

impl FailedPuts {
    /// Records one failed upload of `what` by the writer `noun` (e.g. `unit(s)`);
    /// only the invocation's first is told.
    pub fn record(&self, noun: &'static str, what: &str, err: &dyn std::fmt::Display) {
        let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
        st.record(noun, what, err);
    }

    /// Like `record`, but a `what` already recorded is ignored entirely: for a
    /// writer whose put is attempted again by a later pass of the same invocation.
    pub fn record_once(&self, noun: &'static str, what: &str, err: &dyn std::fmt::Display) {
        let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if st.seen.insert(what.to_string()) {
            st.record(noun, what, err);
        }
    }

    /// Tells how many of each writer's failures went untold, then forgets them,
    /// so a second call prints nothing.
    pub fn summarize(&self) {
        let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
        for (noun, more) in std::mem::take(&mut st.untold) {
            tracing::warn!("cache upload failed for {more} more {noun}; not published");
        }
    }
}

impl FailedPutsState {
    fn record(&mut self, noun: &'static str, what: &str, err: &dyn std::fmt::Display) {
        if !self.told {
            self.told = true;
            tracing::warn!("cache upload for {what} failed: {err}; not published");
        } else if let Some(e) = self.untold.iter_mut().find(|e| e.0 == noun) {
            e.1 += 1;
        } else {
            self.untold.push((noun, 1));
        }
    }
}
