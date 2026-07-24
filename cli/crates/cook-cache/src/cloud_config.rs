//! Parse `.cook/cloud.toml` — the project-level cloud config.
//!
//! Spec §9. The file is optional; if missing or empty, defaults apply.

use std::path::Path;
use std::time::Duration;

use cook_fingerprint::backend::BackendConfig;
use serde::Deserialize;

/// serde default for `CloudSection::publish` — absent key means "publish".
fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct CloudConfig {
    #[serde(default)]
    pub cloud: CloudSection,
    #[serde(default)]
    pub cache: CacheSection,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CloudSection {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub endpoint: Option<String>,
    #[serde(default)]
    pub project: Option<String>,

    // CS-0057 backend tunables. All optional; absent values fall back to
    // `BackendConfig::default()`. Honoured by `LocalBackend` for
    // `max_artifact_bytes` only; the timeout / retry / backoff knobs are
    // honoured by `CloudBackend` (CS-0058).
    #[serde(default)]
    pub timeout_secs: Option<u64>,
    #[serde(default)]
    pub max_retries: Option<u32>,
    #[serde(default)]
    pub backoff_initial_ms: Option<u64>,
    #[serde(default)]
    pub backoff_max_ms: Option<u64>,
    #[serde(default)]
    pub max_artifact_mib: Option<u64>,
    /// COOK-168. When `false`, this client operates in read-only mode:
    /// cache lookups still work, but no artifacts are uploaded to the
    /// shared store. Defaults to `true` (publish enabled) when absent.
    #[serde(default = "default_true")]
    pub publish: bool,
}

impl Default for CloudSection {
    fn default() -> Self {
        Self {
            enabled: false,
            endpoint: None,
            project: None,
            timeout_secs: None,
            max_retries: None,
            backoff_initial_ms: None,
            backoff_max_ms: None,
            max_artifact_mib: None,
            publish: true,
        }
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct CacheSection {
    #[serde(default)]
    pub ignore_env: Vec<String>,
    #[serde(default)]
    pub cache_dir: Option<String>,
}

#[derive(Debug)]
pub enum CloudConfigError {
    Io(std::io::Error),
    Parse(toml::de::Error),
    MissingProject,
    /// CS-0058. `cloud.enabled = true` but no endpoint configured. The
    /// engine cannot construct a `CloudBackend` without one. Distinct from
    /// `MissingProject` so the diagnostic names the right field.
    MissingEndpoint,
    /// CS-0058 / CS-0059. `cloud.enabled = true` but no API key was
    /// resolved from `COOK_CLOUD_API_KEY`. The engine refuses to construct a
    /// `CloudBackend` without a bearer token; no HTTP request is ever sent
    /// unauthenticated. Pre-CS-0059 a `[cloud] api_key` TOML field was a
    /// secondary source; that field was removed to close the
    /// secret-in-checked-in-config foot-gun.
    MissingApiKey,
}

impl std::fmt::Display for CloudConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "reading .cook/cloud.toml: {e}"),
            Self::Parse(e) => write!(f, "parsing .cook/cloud.toml: {e}"),
            Self::MissingProject => write!(
                f,
                "[cloud] enabled=true but [cloud] project is missing — \
                 set `project = \"...\"` in .cook/cloud.toml or set `enabled = false`"
            ),
            Self::MissingEndpoint => write!(
                f,
                "[cloud] enabled=true but [cloud] endpoint is missing — \
                 set `endpoint = \"https://...\"` in .cook/cloud.toml or set `enabled = false`"
            ),
            Self::MissingApiKey => write!(
                f,
                "[cloud] enabled=true but no API key resolved — \
                 export COOK_CLOUD_API_KEY=<your-token> \
                 (interactive `cook cloud login` is planned in a future release)"
            ),
        }
    }
}

impl std::error::Error for CloudConfigError {}

impl CloudConfig {
    /// Load `.cook/cloud.toml` from `project_root`. Returns `Default` if absent.
    /// Validates that `project` is set when `cloud.enabled = true`.
    pub fn load_or_default(project_root: &Path) -> Result<Self, CloudConfigError> {
        let path = project_root.join(".cook").join("cloud.toml");
        let cfg = if !path.exists() {
            Self::default()
        } else {
            let bytes = std::fs::read_to_string(&path).map_err(CloudConfigError::Io)?;
            toml::from_str::<Self>(&bytes).map_err(CloudConfigError::Parse)?
        };

        if cfg.cloud.enabled {
            if cfg.cloud.project.is_none() {
                return Err(CloudConfigError::MissingProject);
            }
            // CS-0058: cloud-enabled must have an endpoint and a resolvable
            // API key. Endpoint check is path-only (no URL parse) — keeping
            // the validator dependency-free; URL validity is the
            // CloudBackend constructor's concern (it surfaces a Transient
            // health-check failure if unreachable).
            if cfg.cloud.endpoint.is_none() {
                return Err(CloudConfigError::MissingEndpoint);
            }
            if cfg.resolved_api_key().is_none() {
                return Err(CloudConfigError::MissingApiKey);
            }
        }
        Ok(cfg)
    }

    /// CS-0058 / CS-0059. Resolve the bearer-token API key for
    /// `CloudBackend` requests from `COOK_CLOUD_API_KEY`. An empty env var
    /// (`COOK_CLOUD_API_KEY=""`) is treated as unset. Returns `None` when
    /// no key is available — `load_or_default` then surfaces
    /// `CloudConfigError::MissingApiKey` when `cloud.enabled = true`.
    /// CS-0059 dropped the secondary `[cloud] api_key` TOML form because
    /// that field encouraged committing secrets in shared repositories.
    pub fn resolved_api_key(&self) -> Option<String> {
        std::env::var("COOK_CLOUD_API_KEY")
            .ok()
            .filter(|v| !v.is_empty())
    }

    /// Returns the configured project_id, or the project root directory name
    /// as a fallback (only valid when cloud is disabled).
    pub fn project_id_or_fallback(&self, project_root: &Path) -> String {
        if let Some(p) = &self.cloud.project {
            return p.clone();
        }
        project_root
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "unknown".to_string())
    }

    pub fn cache_ignore_env(&self) -> &[String] {
        &self.cache.ignore_env
    }

    pub fn cache_dir(&self) -> Option<&str> {
        self.cache.cache_dir.as_deref()
    }

    /// Whether this client publishes produced artifacts to the shared store.
    /// Defaults to `true`; a `[cloud] publish = false` makes the client
    /// read-only (fetch-by-key still works). Honoured globally by the
    /// executor's upload paths (COOK-168).
    pub fn publish(&self) -> bool {
        self.cloud.publish
    }

    /// Build a `BackendConfig` for this project (CS-0057). Starts from
    /// `BackendConfig::default()` and overrides each field that the
    /// `[cloud]` section in `.cook/cloud.toml` set. Unset fields keep
    /// their default; this is the cloud-toml-empty-or-absent identity.
    pub fn backend_config(&self) -> BackendConfig {
        let mut cfg = BackendConfig::default();
        if let Some(secs) = self.cloud.timeout_secs {
            cfg.timeout = Duration::from_secs(secs);
        }
        if let Some(n) = self.cloud.max_retries {
            cfg.max_retries = n;
        }
        if let Some(ms) = self.cloud.backoff_initial_ms {
            cfg.backoff_initial = Duration::from_millis(ms);
        }
        if let Some(ms) = self.cloud.backoff_max_ms {
            cfg.backoff_max = Duration::from_millis(ms);
        }
        if let Some(mib) = self.cloud.max_artifact_mib {
            cfg.max_artifact_bytes = mib.saturating_mul(1024 * 1024);
        }
        cfg
    }
}

#[cfg(test)]
#[path = "tests/cloud_config_tests.rs"]
mod tests;
