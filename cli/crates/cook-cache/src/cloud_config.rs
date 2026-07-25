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
    /// COOK-232. `[cache] max_size = "20GB"` — the CAS budget consumed by
    /// `cook cache du`'s over/under-budget line. Absent means "no budget,
    /// no warning" (today's behaviour). Stored as the raw literal; parsed
    /// on demand by `CloudConfig::max_size_bytes` so a bad literal is a
    /// deferred, nameable error rather than a load-time panic.
    #[serde(default)]
    pub max_size: Option<String>,
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
    /// COOK-232. `[cache] max_size` was set but its literal did not parse
    /// as a decimal number with an optional unit suffix. Carries the
    /// offending literal verbatim so the message can name it.
    BadMaxSize(String),
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
            Self::BadMaxSize(lit) => write!(
                f,
                "[cache] max_size = {lit:?} is not a valid size — \
                 expected a decimal number with an optional unit suffix \
                 (B, KB, MB, GB, TB, KiB, MiB, GiB, TiB), e.g. \"20GB\" or \"512 MiB\""
            ),
        }
    }
}

impl std::error::Error for CloudConfigError {}

/// COOK-232. Parse a `[cache] max_size` literal into a byte count.
///
/// Accepts a decimal number (fractional allowed, truncated toward zero)
/// followed by an optional unit suffix, matched case-insensitively, with
/// optional whitespace between the number and the unit. A bare number is
/// bytes. `KB`/`MB`/`GB`/`TB` are powers of 1000; `KIB`/`MIB`/`GIB`/`TIB`
/// are powers of 1024. A leading sign (`+` or `-`) and anything else that
/// doesn't match this shape return `None` — the caller turns that into a
/// `CloudConfigError::BadMaxSize` naming the original literal. A budget
/// literal never needs an explicit sign, so a value never gets far enough
/// to be tested as negative (which would otherwise let a signed zero like
/// `"-0"` slip through, since `-0.0 < 0.0` is false).
///
/// A value that doesn't fit in `u64` also returns `None` rather than
/// silently saturating to `u64::MAX` — a typo like `"999999999TB"` must
/// not read as "effectively no budget".
fn parse_size(literal: &str) -> Option<u64> {
    let s = literal.trim();
    if s.starts_with('-') || s.starts_with('+') {
        return None;
    }
    let bytes = s.as_bytes();
    let mut i = 0;
    let mut saw_digit = false;
    while i < bytes.len() && bytes[i].is_ascii_digit() {
        i += 1;
        saw_digit = true;
    }
    if i < bytes.len() && bytes[i] == b'.' {
        i += 1;
        while i < bytes.len() && bytes[i].is_ascii_digit() {
            i += 1;
            saw_digit = true;
        }
    }
    if !saw_digit {
        return None;
    }
    let (num_part, rest) = s.split_at(i);
    let number: f64 = num_part.parse().ok()?;
    let unit = rest.trim_start();
    let multiplier: f64 = if unit.is_empty() {
        1.0
    } else {
        match unit.to_ascii_uppercase().as_str() {
            "B" => 1.0,
            "KB" => 1_000.0,
            "MB" => 1_000_000.0,
            "GB" => 1_000_000_000.0,
            "TB" => 1_000_000_000_000.0,
            "KIB" => 1024.0,
            "MIB" => 1024.0 * 1024.0,
            "GIB" => 1024.0 * 1024.0 * 1024.0,
            "TIB" => 1024.0 * 1024.0 * 1024.0 * 1024.0,
            _ => return None,
        }
    };
    let total = number * multiplier;
    if !total.is_finite() {
        return None;
    }
    // `u64::MAX` isn't exactly representable as `f64` (it rounds up to
    // `2^64`), so this comparison correctly rejects anything that would
    // otherwise saturate via the `as` cast below rather than erroring.
    if total >= u64::MAX as f64 {
        return None;
    }
    Some(total as u64)
}

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

    /// COOK-232. `[cache] max_size` resolved to a byte budget. `None` when
    /// unset — "no budget, no warning" (today's behaviour). `Err` names the
    /// offending literal when set but unparseable; this never silently
    /// falls back to "no budget" on a typo. The warning that consumes this
    /// value (`cook cache du`'s over/under-budget line) and the `auto_gc`
    /// knob both belong to COOK-235, not here.
    pub fn max_size_bytes(&self) -> Result<Option<u64>, CloudConfigError> {
        match &self.cache.max_size {
            None => Ok(None),
            Some(lit) => parse_size(lit)
                .map(Some)
                .ok_or_else(|| CloudConfigError::BadMaxSize(lit.clone())),
        }
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
