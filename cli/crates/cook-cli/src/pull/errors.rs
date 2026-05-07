//! `PullError` — every variant maps to a documented exit code from the design spec.

use std::fmt;
use std::io;
use std::path::PathBuf;

#[derive(Debug)]
pub enum PullError {
    /// HTTP / network failure during archive fetch.
    Network { url: String, source: String },
    /// Trust not established and the caller could not prompt.
    TrustRefused { url: String },
    /// Module name was not present under `modules/<name>/` in the archive.
    ModuleNotFound { name: String, available: Vec<String> },
    /// Conflict on overwrite, no TTY and no `--force`.
    ConflictNonInteractive { paths: Vec<PathBuf> },
    /// User answered `q` at a conflict prompt.
    AbortedByUser,
    /// Archive layout was rejected (symlink entry, traversal, malformed gzip/tar).
    BadArchive { reason: String },
    /// I/O error during install or trust persistence.
    Io { context: String, source: io::Error },
    /// Malformed config or trust file.
    BadConfig { path: PathBuf, reason: String },
    /// Invalid CLI args (clap mapped, plus our cross-arg checks).
    BadArgs { reason: String },
}

impl PullError {
    /// Process exit code per design §Error model.
    pub fn exit_code(&self) -> i32 {
        match self {
            PullError::Network { .. } => 1,
            PullError::TrustRefused { .. } => 2,
            PullError::ModuleNotFound { .. } => 3,
            PullError::ConflictNonInteractive { .. } => 4,
            PullError::AbortedByUser => 5,
            PullError::BadArchive { .. } => 1,
            PullError::Io { .. } => 1,
            PullError::BadConfig { .. } => 1,
            PullError::BadArgs { .. } => 64,
        }
    }
}

impl fmt::Display for PullError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PullError::Network { url, source } => write!(f, "failed to fetch {url}: {source}"),
            PullError::TrustRefused { url } => write!(
                f,
                "registry {url} is not trusted; rerun with --accept-trust"
            ),
            PullError::ModuleNotFound { name, available } => {
                let list = if available.is_empty() {
                    "(registry exposes no modules)".to_string()
                } else {
                    available.join(", ")
                };
                write!(f, "module '{name}' not found; available: {list}")
            }
            PullError::ConflictNonInteractive { paths } => {
                let list = paths
                    .iter()
                    .map(|p| p.display().to_string())
                    .collect::<Vec<_>>()
                    .join(", ");
                write!(
                    f,
                    "would overwrite: {list}; rerun with --force or in a TTY"
                )
            }
            PullError::AbortedByUser => write!(f, "aborted by user"),
            PullError::BadArchive { reason } => write!(f, "bad archive: {reason}"),
            PullError::Io { context, source } => write!(f, "{context}: {source}"),
            PullError::BadConfig { path, reason } => {
                write!(f, "bad config at {}: {reason}", path.display())
            }
            PullError::BadArgs { reason } => write!(f, "{reason}"),
        }
    }
}

impl std::error::Error for PullError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exit_codes_match_spec() {
        assert_eq!(
            PullError::Network {
                url: "u".into(),
                source: "s".into()
            }
            .exit_code(),
            1
        );
        assert_eq!(PullError::TrustRefused { url: "u".into() }.exit_code(), 2);
        assert_eq!(
            PullError::ModuleNotFound {
                name: "x".into(),
                available: vec![]
            }
            .exit_code(),
            3
        );
        assert_eq!(
            PullError::ConflictNonInteractive { paths: vec![] }.exit_code(),
            4
        );
        assert_eq!(PullError::AbortedByUser.exit_code(), 5);
        assert_eq!(
            PullError::BadArgs {
                reason: "x".into()
            }
            .exit_code(),
            64
        );
    }

    #[test]
    fn display_includes_url_in_network_error() {
        let e = PullError::Network {
            url: "https://example.test".into(),
            source: "timeout".into(),
        };
        let s = format!("{e}");
        assert!(s.contains("https://example.test"));
        assert!(s.contains("timeout"));
    }
}
