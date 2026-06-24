//! CacheContext — single struct aggregating everything the cache layer needs:
//! env denylist, backend, and project config. Built once per `cook build`
//! invocation in cook-engine's run.rs and threaded down.

use std::path::PathBuf;
use std::sync::Arc;

use cook_fingerprint::{CacheBackend, EnvDenylist};

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
}
