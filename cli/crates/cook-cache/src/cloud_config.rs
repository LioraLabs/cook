//! Parse `.cook/cloud.toml` — the project-level cloud config.
//!
//! Spec §9. The file is optional; if missing or empty, defaults apply.

use std::path::Path;
use std::time::Duration;

use cook_fingerprint::backend::BackendConfig;
use serde::Deserialize;

#[derive(Debug, Clone, Default, Deserialize)]
pub struct CloudConfig {
    #[serde(default)]
    pub cloud: CloudSection,
    #[serde(default)]
    pub cache: CacheSection,
}

#[derive(Debug, Clone, Default, Deserialize)]
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
    // wired through but no-ops on disk I/O. `CloudBackend` (next ticket)
    // will honour all five.
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
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct CacheSection {
    #[serde(default)]
    pub ignore_env: Vec<String>,
    #[serde(default)]
    pub cache_dir: Option<String>,
    /// Declared toolchain pinning (CS-0052). Each name is resolved via `which`
    /// at build start; the resolved binaries' content hashes fold into every
    /// step's context_hash. An empty list (or absent field) is observationally
    /// inert. Misdeclared names cause a build-start error — see the design at
    /// standard/specs/2026-05-04-cache-declared-tools-design.md.
    #[serde(default)]
    pub tools: Vec<String>,
}

#[derive(Debug)]
pub enum CloudConfigError {
    Io(std::io::Error),
    Parse(toml::de::Error),
    MissingProject,
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

        if cfg.cloud.enabled && cfg.cloud.project.is_none() {
            return Err(CloudConfigError::MissingProject);
        }
        Ok(cfg)
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

    pub fn cache_tools(&self) -> &[String] {
        &self.cache.tools
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
mod tests {
    use super::*;
    use std::io::Write;
    use std::path::PathBuf;

    fn write_toml(dir: &Path, contents: &str) -> PathBuf {
        let cook_dir = dir.join(".cook");
        std::fs::create_dir_all(&cook_dir).expect("mkdir");
        let path = cook_dir.join("cloud.toml");
        let mut f = std::fs::File::create(&path).expect("create");
        f.write_all(contents.as_bytes()).expect("write");
        path
    }

    #[test]
    fn missing_file_returns_default() {
        let dir = tempfile::tempdir().expect("tempdir");
        let cfg = CloudConfig::load_or_default(dir.path()).expect("load");
        assert!(!cfg.cloud.enabled);
        assert_eq!(cfg.project_id_or_fallback(dir.path()), dir.path().file_name().unwrap().to_string_lossy());
        assert!(cfg.cache_ignore_env().is_empty());
    }

    #[test]
    fn cloud_disabled_no_project_required() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_toml(dir.path(), r#"
[cloud]
enabled = false
"#);
        let cfg = CloudConfig::load_or_default(dir.path()).expect("load");
        assert!(!cfg.cloud.enabled);
        // No project required when disabled.
    }

    #[test]
    fn cloud_enabled_requires_project() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_toml(dir.path(), r#"
[cloud]
enabled = true
endpoint = "https://api.cook.dev"
"#);
        let result = CloudConfig::load_or_default(dir.path());
        assert!(result.is_err(), "missing project must error when cloud.enabled=true");
    }

    #[test]
    fn cloud_enabled_with_project_ok() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_toml(dir.path(), r#"
[cloud]
enabled = true
endpoint = "https://api.cook.dev"
project = "cook"
"#);
        let cfg = CloudConfig::load_or_default(dir.path()).expect("load");
        assert!(cfg.cloud.enabled);
        assert_eq!(cfg.cloud.project.as_deref(), Some("cook"));
    }

    #[test]
    fn cache_ignore_env_parsed() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_toml(dir.path(), r#"
[cache]
ignore_env = ["GITHUB_TOKEN", "MY_API_KEY"]
"#);
        let cfg = CloudConfig::load_or_default(dir.path()).expect("load");
        let ignore = cfg.cache_ignore_env();
        assert_eq!(ignore.len(), 2);
        assert!(ignore.contains(&"GITHUB_TOKEN".to_string()));
        assert!(ignore.contains(&"MY_API_KEY".to_string()));
    }

    #[test]
    fn cache_tools_parsed() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_toml(dir.path(), r#"
[cache]
tools = ["gcc", "ld", "strip"]
"#);
        let cfg = CloudConfig::load_or_default(dir.path()).expect("load");
        assert_eq!(cfg.cache_tools(), &["gcc", "ld", "strip"]);
    }

    #[test]
    fn cache_tools_default_empty() {
        let dir = tempfile::tempdir().expect("tempdir");
        let cfg = CloudConfig::load_or_default(dir.path()).expect("load");
        assert!(cfg.cache_tools().is_empty());
    }

    #[test]
    fn malformed_toml_errors() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_toml(dir.path(), "this is not valid toml === ");
        assert!(CloudConfig::load_or_default(dir.path()).is_err());
    }

    #[test]
    fn project_id_or_fallback_uses_dir_name_when_no_project() {
        let dir = tempfile::tempdir().expect("tempdir");
        let project_dir = dir.path().join("my-cool-project");
        std::fs::create_dir_all(&project_dir).expect("mkdir");
        let cfg = CloudConfig::load_or_default(&project_dir).expect("load");
        assert_eq!(cfg.project_id_or_fallback(&project_dir), "my-cool-project");
    }

    // ─── CS-0057: BackendConfig threading ───────────────────────────────────

    /// An empty `[cloud]` section produces a `BackendConfig` exactly equal
    /// to `BackendConfig::default()` — the no-tunables identity.
    #[test]
    fn backend_config_uses_defaults_when_unset() {
        let dir = tempfile::tempdir().expect("tempdir");
        let cfg = CloudConfig::load_or_default(dir.path()).expect("load");
        let bc = cfg.backend_config();
        let def = BackendConfig::default();
        assert_eq!(bc.timeout, def.timeout);
        assert_eq!(bc.max_retries, def.max_retries);
        assert_eq!(bc.backoff_initial, def.backoff_initial);
        assert_eq!(bc.backoff_max, def.backoff_max);
        assert_eq!(bc.max_artifact_bytes, def.max_artifact_bytes);
    }

    /// All five `[cloud]` knobs override the corresponding
    /// `BackendConfig` fields with the user-provided values, including
    /// the `_secs` / `_ms` / `_mib` unit conversions.
    #[test]
    fn backend_config_overrides_from_toml() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_toml(dir.path(), r#"
[cloud]
timeout_secs = 90
max_retries = 7
backoff_initial_ms = 250
backoff_max_ms = 12000
max_artifact_mib = 256
"#);
        let cfg = CloudConfig::load_or_default(dir.path()).expect("load");
        let bc = cfg.backend_config();
        assert_eq!(bc.timeout, Duration::from_secs(90));
        assert_eq!(bc.max_retries, 7);
        assert_eq!(bc.backoff_initial, Duration::from_millis(250));
        assert_eq!(bc.backoff_max, Duration::from_millis(12_000));
        assert_eq!(bc.max_artifact_bytes, 256u64 * 1024 * 1024);
    }
}
