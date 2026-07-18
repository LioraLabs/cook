//! M3.3 — `~/.cook/bin/luarocks` subprocess wrapper.
//!
//! The driver wraps every state-changing or read-only luarocks invocation
//! cook needs. Every call passes `--tree <project>/cook_modules` so rocks
//! land in the project's tree, never in a user-global luarocks tree.
//! Index precedence is realised by passing `--server <url>` repeatedly in
//! left-to-right order.
//!
//! Error handling is passthrough: on non-zero exit, the driver returns an
//! `anyhow::Error` whose Display contains argv + captured stdout + captured
//! stderr (each capped at 64 KiB to match `cook.exec`'s SHI-188 truncation).
//! No structured parsing of luarocks output.

use std::path::PathBuf;
use std::process::{Command, Output};

use anyhow::{anyhow, Context, Result};

use crate::modules::lockfile::LockedModule;

// Per-stream cap matching SHI-188's `cook.exec` failure truncation.
// Keep in sync with COOK_CMD_FAIL_STREAM_CAP in cli/crates/cook-luaotp/src/pool.rs
// and cli/crates/cook-register/src/capture.rs.
const STREAM_CAP_BYTES: usize = 64 * 1024;

#[derive(Debug, Clone)]
pub struct RocksDriver {
    prefix: PathBuf,
    indexes: Vec<String>,
    project_dir: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstalledRock {
    pub name: String,
    pub version: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchHit {
    pub name: String,
    pub version: String,
    /// Approximation: always the first configured index in the driver's
    /// `indexes` list, not the actual index the hit came from. Phase 3
    /// does not parse luarocks search output to per-hit precision; the
    /// user sees luarocks's own output too via the parent command stdout.
    pub index: String,
}

impl RocksDriver {
    pub fn new(prefix: PathBuf, indexes: Vec<String>, project_dir: PathBuf) -> Self {
        Self {
            prefix,
            indexes,
            project_dir,
        }
    }

    pub fn binary(&self) -> PathBuf {
        self.prefix.join("bin/luarocks")
    }

    pub fn tree_arg(&self) -> PathBuf {
        self.project_dir.join("cook_modules")
    }

    /// Build the base argv prefix used by every invocation.
    pub fn base_argv(&self) -> Vec<String> {
        let mut v = vec![
            "--tree".to_string(),
            self.tree_arg().to_string_lossy().into_owned(),
        ];
        // luarocks' `--server` is SINGLE-VALUED (last flag wins, in both the
        // `--server url` and `--server=url` spellings), so emitting one flag
        // per index never worked: only the last index was searched and every
        // blessed-rock install failed with "No results matching query". Found
        // launch night with cook_cc 0.14.0-1 live on rocks.usecook.com but
        // uninstallable through cook. One `--server=` flag PREPENDS to
        // luarocks' built-in default server list (luarocks.org + mirrors),
        // which is exactly the blessed-index-first, public-fallback semantics
        // we want — so emit the first non-default index and let the built-in
        // defaults provide the fallback. A config with several private
        // indexes is not expressible through the CLI flag; that needs a
        // generated luarocks config file (follow-up).
        if let Some(idx) = self
            .indexes
            .iter()
            .find(|i| i.trim_end_matches('/') != "https://luarocks.org")
        {
            v.push(format!("--server={idx}"));
        }
        v
    }

    pub fn install(&self, name: &str, constraint: &str) -> Result<()> {
        let mut argv = vec!["install".to_string()];
        argv.extend(self.base_argv());
        argv.push(name.to_string());
        if !constraint.is_empty() && constraint != "*" {
            argv.push(constraint.to_string());
        }
        self.run(&argv)?;
        Ok(())
    }

    pub fn install_locked(&self, locked: &LockedModule) -> Result<()> {
        // Install by pinned NAME + EXACT VERSION through the resolver. The
        // previous form passed `locked.source` (a git+https or tarball URL)
        // as the package spec, which `luarocks install` cannot resolve at
        // all ("No results matching query") — every locked reinstall from a
        // fresh tree failed. The name@version pair IS the lock (integrity
        // is recorded but not yet enforced); the server list pins where it
        // resolves from.
        let mut argv = vec!["install".to_string()];
        argv.extend(self.base_argv());
        argv.push(locked.name.clone());
        argv.push(locked.version.clone());
        self.run(&argv)?;
        Ok(())
    }

    pub fn remove(&self, name: &str) -> Result<()> {
        let mut argv = vec!["remove".to_string()];
        argv.extend(self.base_argv());
        argv.push(name.to_string());
        self.run(&argv)?;
        Ok(())
    }

    pub fn search(&self, query: &str) -> Result<Vec<SearchHit>> {
        let mut argv = vec!["search".to_string()];
        argv.extend(self.base_argv());
        argv.push(query.to_string());
        let out = self.run(&argv)?;
        Ok(parse_search_output(&out.stdout, &self.indexes))
    }

    pub fn list_installed(&self) -> Result<Vec<InstalledRock>> {
        let mut argv = vec!["list".to_string()];
        argv.extend(self.base_argv());
        argv.push("--porcelain".to_string());
        let out = self.run(&argv)?;
        Ok(parse_list_output(&out.stdout))
    }

    /// Run the luarocks binary with the given argv, return captured Output.
    /// On non-zero exit, return a passthrough error.
    fn run(&self, argv: &[String]) -> Result<Output> {
        let bin = self.binary();
        let out = Command::new(&bin)
            .args(argv)
            .output()
            .with_context(|| format!("spawn {}", bin.display()))?;
        if !out.status.success() {
            let argv_quoted = argv
                .iter()
                .map(|a| {
                    if a.contains(' ') {
                        format!("'{}'", a)
                    } else {
                        a.clone()
                    }
                })
                .collect::<Vec<_>>()
                .join(" ");
            return Err(anyhow!(
                "luarocks failed: {} {}\n--- stdout ---\n{}\n--- stderr ---\n{}\n--- exit {} ---",
                bin.display(),
                argv_quoted,
                truncate_stream(&out.stdout),
                truncate_stream(&out.stderr),
                out.status.code().map(|c| c.to_string()).unwrap_or_else(|| "signal".into()),
            ));
        }
        Ok(out)
    }
}

fn truncate_stream(bytes: &[u8]) -> String {
    let s = String::from_utf8_lossy(bytes);
    if s.len() <= STREAM_CAP_BYTES {
        return s.into_owned();
    }
    let truncated = &s[..STREAM_CAP_BYTES];
    format!(
        "{}... ({} more bytes)",
        truncated,
        s.len() - STREAM_CAP_BYTES
    )
}

fn parse_list_output(stdout: &[u8]) -> Vec<InstalledRock> {
    // luarocks --porcelain `list` output: lines of the form `name\tversion\t...`.
    String::from_utf8_lossy(stdout)
        .lines()
        .filter_map(|line| {
            let mut cols = line.split('\t');
            let name = cols.next()?;
            let version = cols.next()?;
            if name.is_empty() {
                return None;
            }
            Some(InstalledRock {
                name: name.to_string(),
                version: version.to_string(),
            })
        })
        .collect()
}

fn parse_search_output(stdout: &[u8], indexes: &[String]) -> Vec<SearchHit> {
    // luarocks `search` output isn't perfectly stable; we extract `name (version)`
    // pairs and tag them with the first configured index (best effort).
    // Structured search semantics is not a Phase 3 goal; the user sees luarocks's
    // own output too via the parent command stdout.
    let s = String::from_utf8_lossy(stdout);
    let default_index = indexes.first().cloned().unwrap_or_default();
    let mut hits = Vec::new();
    for line in s.lines() {
        let trimmed = line.trim();
        if let Some(idx) = trimmed.find('(') {
            let name = trimmed[..idx].trim();
            let rest = &trimmed[idx + 1..];
            if let Some(end) = rest.find(')') {
                let version = rest[..end].trim();
                if !name.is_empty() && !version.is_empty() {
                    hits.push(SearchHit {
                        name: name.to_string(),
                        version: version.to_string(),
                        index: default_index.clone(),
                    });
                }
            }
        }
    }
    hits
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};

    fn fake_prefix() -> tempfile::TempDir {
        // Set up a fake $prefix where bin/luarocks is a symlink to the
        // tests/fixtures/driver/fake-luarocks.sh script.
        let tmp = tempfile::tempdir().expect("tempdir");
        let bin = tmp.path().join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        let fake = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/driver/fake-luarocks.sh");
        std::os::unix::fs::symlink(&fake, bin.join("luarocks")).expect("symlink");
        tmp
    }

    fn read_argv_log(path: &Path) -> Vec<String> {
        let raw = std::fs::read_to_string(path).expect("read log");
        raw.lines()
            .skip(1) // skip "argv:" header
            .map(|l| l.trim().to_string())
            .collect()
    }

    fn clear_fake_env() {
        for var in ["FAKE_LUAROCKS_LOG", "FAKE_LUAROCKS_EXIT", "FAKE_LUAROCKS_STDOUT", "FAKE_LUAROCKS_STDERR"] {
            std::env::remove_var(var);
        }
    }

    #[test]
    #[serial_test::serial]
    fn install_argv_includes_tree_and_servers() {
        clear_fake_env();
        let prefix = fake_prefix();
        let project = tempfile::tempdir().expect("project");
        let log = project.path().join("argv.log");
        std::env::set_var("FAKE_LUAROCKS_LOG", &log);
        std::env::set_var("FAKE_LUAROCKS_EXIT", "0");

        let driver = RocksDriver::new(
            prefix.path().to_path_buf(),
            vec![
                "https://rocks.usecook.com".to_string(),
                "https://luarocks.org".to_string(),
            ],
            project.path().to_path_buf(),
        );
        driver.install("cook_smoke", "*").expect("install");

        let argv = read_argv_log(&log);
        assert_eq!(argv[0], "install");
        assert!(argv.iter().any(|a| a == "--tree"));
        let tree_idx = argv.iter().position(|a| a == "--tree").unwrap();
        assert!(argv[tree_idx + 1].ends_with("cook_modules"));
        let server_args: Vec<&String> = argv
            .iter()
            .filter(|a| a.starts_with("--server="))
            .collect();
        // Exactly ONE --server flag: luarocks' flag is single-valued
        // (last-wins), and luarocks.org is already in its built-in default
        // server list, so the blessed index is the only flag emitted.
        assert_eq!(
            server_args,
            vec![&"--server=https://rocks.usecook.com".to_string()]
        );
        assert_eq!(argv.last().unwrap(), "cook_smoke");
        // Constraint "*" omitted from argv (passes through as no-constraint).
        assert!(!argv.iter().any(|a| a == "*"));
        clear_fake_env();
    }

    #[test]
    #[serial_test::serial]
    fn install_with_explicit_constraint_passes_through() {
        clear_fake_env();
        let prefix = fake_prefix();
        let project = tempfile::tempdir().expect("project");
        let log = project.path().join("argv.log");
        std::env::set_var("FAKE_LUAROCKS_LOG", &log);
        std::env::set_var("FAKE_LUAROCKS_EXIT", "0");

        let driver = RocksDriver::new(
            prefix.path().to_path_buf(),
            Vec::new(),
            project.path().to_path_buf(),
        );
        driver.install("argparse", ">=0.7").expect("install");
        let argv = read_argv_log(&log);
        assert_eq!(argv[0], "install");
        assert!(argv.iter().any(|a| a == "argparse"));
        assert!(argv.iter().any(|a| a == ">=0.7"));
        clear_fake_env();
    }

    #[test]
    #[serial_test::serial]
    fn install_locked_uses_pinned_name_and_version() {
        clear_fake_env();
        let prefix = fake_prefix();
        let project = tempfile::tempdir().expect("project");
        let log = project.path().join("argv.log");
        std::env::set_var("FAKE_LUAROCKS_LOG", &log);

        let driver = RocksDriver::new(
            prefix.path().to_path_buf(),
            vec!["https://example".into()],
            project.path().to_path_buf(),
        );
        let locked = LockedModule {
            name: "cook_smoke".into(),
            version: "0.1.0-1".into(),
            source: "https://rocks.usecook.com/cook_smoke-0.1.0-1.src.rock".into(),
            integrity: "sha256-x".into(),
            direct: true,
        };
        driver.install_locked(&locked).expect("install_locked");
        let argv = read_argv_log(&log);
        // name + exact version, never the source URL (luarocks cannot
        // resolve a git/tarball URL passed as a package spec).
        assert_eq!(argv[argv.len() - 2], locked.name);
        assert_eq!(argv.last().unwrap(), &locked.version);
        clear_fake_env();
    }

    #[test]
    #[serial_test::serial]
    fn nonzero_exit_passes_through_argv_stdout_stderr() {
        clear_fake_env();
        let prefix = fake_prefix();
        let project = tempfile::tempdir().expect("project");
        let log = project.path().join("argv.log");
        std::env::set_var("FAKE_LUAROCKS_LOG", &log);
        std::env::set_var("FAKE_LUAROCKS_EXIT", "7");
        std::env::set_var("FAKE_LUAROCKS_STDOUT", "stdout-marker");
        std::env::set_var("FAKE_LUAROCKS_STDERR", "stderr-marker");

        let driver = RocksDriver::new(
            prefix.path().to_path_buf(),
            Vec::new(),
            project.path().to_path_buf(),
        );
        let err = driver.remove("cook_smoke").expect_err("must fail");
        let msg = format!("{:#}", err);
        assert!(msg.contains("luarocks failed"));
        assert!(msg.contains("stdout-marker"));
        assert!(msg.contains("stderr-marker"));
        assert!(msg.contains("exit 7"));
        clear_fake_env();
    }

    #[test]
    fn parse_list_output_extracts_porcelain() {
        let stdout = b"cook_smoke\t0.1.0-1\tinstalled\nargparse\t0.7.1-1\tinstalled\n";
        let rocks = parse_list_output(stdout);
        assert_eq!(rocks.len(), 2);
        assert_eq!(rocks[0].name, "cook_smoke");
        assert_eq!(rocks[0].version, "0.1.0-1");
        assert_eq!(rocks[1].name, "argparse");
        assert_eq!(rocks[1].version, "0.7.1-1");
    }

    #[test]
    fn parse_search_output_extracts_name_version() {
        let stdout = b"cook_smoke (0.1.0-1)\nlua-cjson (2.1.0.10-1)\n";
        let hits = parse_search_output(stdout, &["https://rocks.usecook.com".into()]);
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].name, "cook_smoke");
        assert_eq!(hits[0].version, "0.1.0-1");
        assert_eq!(hits[0].index, "https://rocks.usecook.com");
    }

    #[test]
    fn truncate_stream_caps_long_output() {
        let big = vec![b'A'; STREAM_CAP_BYTES + 100];
        let truncated = truncate_stream(&big);
        assert!(truncated.contains("100 more bytes"));
        assert!(truncated.len() < big.len());
    }
}
