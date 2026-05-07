//! Trust-on-first-use consent for registry URLs. State lives in a
//! `trust.toml` file under the user's config dir; a missing entry triggers a
//! prompt unless `--accept-trust` was passed.

use std::fs;
use std::io::{BufRead, Write};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use super::errors::PullError;

#[derive(Debug, Default, Serialize, Deserialize)]
struct TrustFile {
    #[serde(default, rename = "trusted")]
    trusted: Vec<TrustedEntry>,
}

#[derive(Debug, Serialize, Deserialize)]
struct TrustedEntry {
    url: String,
    accepted_at: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrustMode {
    /// Interactive: prompt user via stdin if not already trusted.
    Interactive,
    /// Non-interactive: error if not already trusted.
    NonInteractive,
    /// Pre-accepted via `--accept-trust`: record and proceed.
    Accept,
}

/// Ensure `url` is trusted, prompting via stdin/stderr if needed.
///
/// `trust_path` is the path to `trust.toml`. If the file cannot be written,
/// emits a one-line warning to `stderr` and returns Ok (consent applies for
/// this invocation only).
pub fn ensure_trusted<R: BufRead, W: Write>(
    url: &str,
    mode: TrustMode,
    trust_path: &Path,
    stdin: &mut R,
    stderr: &mut W,
) -> Result<(), PullError> {
    let mut trust = load(trust_path)?;
    if trust.trusted.iter().any(|t| t.url == url) {
        return Ok(());
    }

    let accept = match mode {
        TrustMode::Accept => true,
        TrustMode::NonInteractive => {
            return Err(PullError::TrustRefused { url: url.into() });
        }
        TrustMode::Interactive => prompt_user(url, trust_path, stdin, stderr)?,
    };

    if !accept {
        return Err(PullError::TrustRefused { url: url.into() });
    }

    trust.trusted.push(TrustedEntry {
        url: url.into(),
        accepted_at: now_iso8601(),
    });

    if let Err(e) = save(trust_path, &trust) {
        let _ = writeln!(
            stderr,
            "cook: cannot persist trust to {}: {}; consent applies for this invocation only",
            trust_path.display(),
            e
        );
    }

    Ok(())
}

fn load(path: &Path) -> Result<TrustFile, PullError> {
    if !path.exists() {
        return Ok(TrustFile::default());
    }
    let raw = match fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) => {
            return Err(PullError::Io {
                context: format!("read {}", path.display()),
                source: e,
            });
        }
    };
    // Per spec: corrupt file is treated as empty (no entries known) and the
    // user will be re-prompted. We do NOT overwrite the file from this path;
    // save() only runs after explicit consent.
    Ok(toml::from_str(&raw).unwrap_or_default())
}

fn save(path: &Path, trust: &TrustFile) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let body = toml::to_string(trust).map_err(|e| std::io::Error::other(e.to_string()))?;
    let tmp = path.with_extension("toml.cook-pull-tmp");
    fs::write(&tmp, body)?;
    fs::rename(tmp, path)?;
    Ok(())
}

fn prompt_user<R: BufRead, W: Write>(
    url: &str,
    trust_path: &Path,
    stdin: &mut R,
    stderr: &mut W,
) -> Result<bool, PullError> {
    let _ = writeln!(
        stderr,
        "\nThe registry at {url} contains Lua modules that `cook` will execute when"
    );
    let _ = writeln!(
        stderr,
        "your recipes use them. By continuing, you trust this registry and the people"
    );
    let _ = writeln!(stderr, "who publish to it.");
    let _ = writeln!(stderr);
    let _ = writeln!(
        stderr,
        "Cook will record your consent in {} so you won't see this",
        trust_path.display()
    );
    let _ = writeln!(stderr, "prompt again for this URL.");
    let _ = writeln!(stderr);
    let _ = write!(stderr, "Trust this registry? [y/N]: ");
    let _ = stderr.flush();

    let mut line = String::new();
    if stdin.read_line(&mut line).map_err(|e| PullError::Io {
        context: "read consent".into(),
        source: e,
    })? == 0
    {
        return Ok(false);
    }
    Ok(matches!(
        line.trim().to_ascii_lowercase().as_str(),
        "y" | "yes"
    ))
}

fn now_iso8601() -> String {
    // Minimal ISO 8601 (UTC, second precision). Avoids pulling chrono in
    // for one timestamp; we only need a stable, sortable string.
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let datetime = humantime::format_rfc3339_seconds(SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(secs));
    datetime.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;
    use tempfile::TempDir;

    fn paths(dir: &TempDir) -> std::path::PathBuf {
        dir.path().join("trust.toml")
    }

    #[test]
    fn accept_mode_records_entry() {
        let dir = TempDir::new().unwrap();
        let path = paths(&dir);
        let mut stdin = Cursor::new(Vec::<u8>::new());
        let mut stderr = Vec::new();
        ensure_trusted(
            "https://example.test/r",
            TrustMode::Accept,
            &path,
            &mut stdin,
            &mut stderr,
        )
        .unwrap();
        let body = fs::read_to_string(&path).unwrap();
        assert!(body.contains("https://example.test/r"));
    }

    #[test]
    fn already_trusted_is_silent() {
        let dir = TempDir::new().unwrap();
        let path = paths(&dir);
        // First call records.
        let mut stdin = Cursor::new(Vec::<u8>::new());
        let mut stderr = Vec::new();
        ensure_trusted(
            "https://example.test/r",
            TrustMode::Accept,
            &path,
            &mut stdin,
            &mut stderr,
        )
        .unwrap();
        // Second call (interactive) must NOT prompt — passes silently.
        let mut stdin2 = Cursor::new(Vec::<u8>::new()); // empty stdin: would EOF if prompted
        let mut stderr2 = Vec::new();
        ensure_trusted(
            "https://example.test/r",
            TrustMode::Interactive,
            &path,
            &mut stdin2,
            &mut stderr2,
        )
        .unwrap();
        assert!(stderr2.is_empty(), "should not have prompted: {stderr2:?}");
    }

    #[test]
    fn non_interactive_without_record_errors() {
        let dir = TempDir::new().unwrap();
        let path = paths(&dir);
        let mut stdin = Cursor::new(Vec::<u8>::new());
        let mut stderr = Vec::new();
        let err = ensure_trusted(
            "https://example.test/r",
            TrustMode::NonInteractive,
            &path,
            &mut stdin,
            &mut stderr,
        )
        .unwrap_err();
        assert!(matches!(err, PullError::TrustRefused { .. }));
    }

    #[test]
    fn interactive_yes_records() {
        let dir = TempDir::new().unwrap();
        let path = paths(&dir);
        let mut stdin = Cursor::new(b"y\n".to_vec());
        let mut stderr = Vec::new();
        ensure_trusted(
            "https://example.test/r",
            TrustMode::Interactive,
            &path,
            &mut stdin,
            &mut stderr,
        )
        .unwrap();
        assert!(fs::read_to_string(&path).unwrap().contains("example.test"));
    }

    #[test]
    fn interactive_no_refuses() {
        let dir = TempDir::new().unwrap();
        let path = paths(&dir);
        let mut stdin = Cursor::new(b"n\n".to_vec());
        let mut stderr = Vec::new();
        let err = ensure_trusted(
            "https://example.test/r",
            TrustMode::Interactive,
            &path,
            &mut stdin,
            &mut stderr,
        )
        .unwrap_err();
        assert!(matches!(err, PullError::TrustRefused { .. }));
        assert!(!path.exists());
    }

    #[test]
    fn corrupt_file_treated_as_empty() {
        let dir = TempDir::new().unwrap();
        let path = paths(&dir);
        fs::write(&path, b"this is not toml = =").unwrap();
        let mut stdin = Cursor::new(Vec::<u8>::new());
        let mut stderr = Vec::new();
        // Accept mode → records new entry, replacing the corrupt file.
        ensure_trusted(
            "https://example.test/r",
            TrustMode::Accept,
            &path,
            &mut stdin,
            &mut stderr,
        )
        .unwrap();
        let body = fs::read_to_string(&path).unwrap();
        assert!(body.contains("example.test"));
    }

    #[test]
    fn per_url_isolation() {
        let dir = TempDir::new().unwrap();
        let path = paths(&dir);
        let mut stdin = Cursor::new(Vec::<u8>::new());
        let mut stderr = Vec::new();
        ensure_trusted(
            "https://a.test/r",
            TrustMode::Accept,
            &path,
            &mut stdin,
            &mut stderr,
        )
        .unwrap();
        // Different URL — non-interactive must still refuse.
        let err = ensure_trusted(
            "https://b.test/r",
            TrustMode::NonInteractive,
            &path,
            &mut stdin,
            &mut stderr,
        )
        .unwrap_err();
        assert!(matches!(err, PullError::TrustRefused { .. }));
    }
}
