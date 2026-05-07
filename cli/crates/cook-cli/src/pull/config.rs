//! Registry URL resolution. Precedence: --registry flag > COOK_REGISTRY_URL env
//! > [registry].url in cook.toml > built-in default.

use std::fs;
use std::path::Path;

use serde::Deserialize;

use super::errors::PullError;

/// Compile-time built-in default. Replaced at v1 release with the public URL.
pub const DEFAULT_REGISTRY_URL: &str = "https://gilberthouse.story-pike.ts.net/cook/registry";

#[derive(Debug, Default, Deserialize)]
struct CookConfig {
    #[serde(default)]
    registry: Option<RegistrySection>,
}

#[derive(Debug, Deserialize)]
struct RegistrySection {
    url: Option<String>,
}

/// Resolve the registry URL to use for this invocation.
///
/// `flag_url` is `--registry`'s value; `env_value` is `COOK_REGISTRY_URL`'s
/// value (passed in for testability); `config_path` points at a `cook.toml`
/// (may be absent).
pub fn resolve_registry_url(
    flag_url: Option<&str>,
    env_value: Option<&str>,
    config_path: &Path,
) -> Result<String, PullError> {
    let raw = match (flag_url, env_value) {
        (Some(s), _) => {
            validate_url(s).map_err(|reason| PullError::BadArgs { reason })?;
            s.to_string()
        }
        (None, Some(s)) if !s.is_empty() => {
            validate_url(s).map_err(|reason| PullError::BadArgs { reason })?;
            s.to_string()
        }
        _ => match read_config(config_path)? {
            Some(s) => {
                validate_url(&s).map_err(|reason| PullError::BadConfig {
                    path: config_path.to_path_buf(),
                    reason,
                })?;
                s
            }
            None => DEFAULT_REGISTRY_URL.to_string(),
        },
    };
    Ok(strip_trailing_slash(&raw))
}

fn read_config(path: &Path) -> Result<Option<String>, PullError> {
    if !path.exists() {
        return Ok(None);
    }
    let raw = fs::read_to_string(path).map_err(|e| PullError::Io {
        context: format!("read {}", path.display()),
        source: e,
    })?;
    let parsed: CookConfig = toml::from_str(&raw).map_err(|e| PullError::BadConfig {
        path: path.to_path_buf(),
        reason: format!("invalid TOML: {e}"),
    })?;
    Ok(parsed.registry.and_then(|r| r.url))
}

fn validate_url(s: &str) -> Result<(), String> {
    if !(s.starts_with("https://") || s.starts_with("http://")) {
        return Err(format!("must be http(s):// URL, got: {s}"));
    }
    Ok(())
}

fn strip_trailing_slash(s: &str) -> String {
    s.trim_end_matches('/').to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn flag_wins() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("cook.toml");
        fs::write(&path, "[registry]\nurl = \"https://from-config.test\"").unwrap();
        let url = resolve_registry_url(
            Some("https://from-flag.test"),
            Some("https://from-env.test"),
            &path,
        )
        .unwrap();
        assert_eq!(url, "https://from-flag.test");
    }

    #[test]
    fn env_beats_config() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("cook.toml");
        fs::write(&path, "[registry]\nurl = \"https://from-config.test\"").unwrap();
        let url =
            resolve_registry_url(None, Some("https://from-env.test"), &path).unwrap();
        assert_eq!(url, "https://from-env.test");
    }

    #[test]
    fn config_beats_default() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("cook.toml");
        fs::write(&path, "[registry]\nurl = \"https://from-config.test\"").unwrap();
        let url = resolve_registry_url(None, None, &path).unwrap();
        assert_eq!(url, "https://from-config.test");
    }

    #[test]
    fn missing_config_falls_through_to_default() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("does-not-exist.toml");
        let url = resolve_registry_url(None, None, &path).unwrap();
        assert_eq!(url, strip_trailing_slash(DEFAULT_REGISTRY_URL));
    }

    #[test]
    fn empty_env_is_ignored() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("does-not-exist.toml");
        let url = resolve_registry_url(None, Some(""), &path).unwrap();
        assert_eq!(url, strip_trailing_slash(DEFAULT_REGISTRY_URL));
    }

    #[test]
    fn malformed_toml_is_error() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("cook.toml");
        fs::write(&path, "this is not = = toml").unwrap();
        let err = resolve_registry_url(None, None, &path).unwrap_err();
        assert!(matches!(err, PullError::BadConfig { .. }));
    }

    #[test]
    fn non_http_url_from_flag_is_bad_args() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("cook.toml");
        let err = resolve_registry_url(Some("file:///etc/passwd"), None, &path).unwrap_err();
        assert!(matches!(err, PullError::BadArgs { .. }));
    }

    #[test]
    fn non_http_url_from_config_is_bad_config() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("cook.toml");
        fs::write(&path, "[registry]\nurl = \"file:///etc/passwd\"").unwrap();
        let err = resolve_registry_url(None, None, &path).unwrap_err();
        match err {
            PullError::BadConfig { path: p, .. } => {
                assert_eq!(p, path); // real path, not "<registry url>" placeholder
            }
            other => panic!("wrong variant: {other:?}"),
        }
    }

    #[test]
    fn config_without_url_falls_through_to_default() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("cook.toml");
        fs::write(&path, "[registry]\n").unwrap();
        let url = resolve_registry_url(None, None, &path).unwrap();
        assert_eq!(url, strip_trailing_slash(DEFAULT_REGISTRY_URL));
    }

    #[test]
    fn trailing_slash_stripped() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("cook.toml");
        let url =
            resolve_registry_url(Some("https://example.test/r/"), None, &path).unwrap();
        assert_eq!(url, "https://example.test/r");
    }
}
