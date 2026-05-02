//! CacheContext — single struct aggregating everything the cache layer needs:
//! machine identity, env denylist, backend, and project config. Built once
//! per `cook build` invocation in cook-engine's run.rs and threaded down.

use std::path::PathBuf;
use std::sync::Arc;

use crate::backend::CacheBackend;
use crate::context::ExecutionContext;
use crate::envkey::EnvDenylist;
use crate::cloud_config::CloudConfig;

#[derive(Clone)]
pub struct CacheContext {
    pub exec_ctx: Arc<ExecutionContext>,
    pub denylist: Arc<EnvDenylist>,
    pub backend: Arc<dyn CacheBackend>,
    pub cloud_config: Arc<CloudConfig>,
    pub project_root: PathBuf,
    pub project_id: String,
}
