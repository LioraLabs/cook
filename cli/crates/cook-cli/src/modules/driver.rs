//! M3.3 — `~/.cook/bin/luarocks` subprocess wrapper.
//!
//! The driver wraps every state-changing or read-only luarocks invocation
//! cook needs. Every call passes `--tree <project>/.cook/modules` so rocks
//! land in the project's own build-output tree, never in a user-global
//! luarocks tree (§27.1.1, CS-0207).
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
use cook_contracts::CapturedStream;

use crate::modules::lockfile::LockedModule;

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

    /// Where luarocks installs. One law with the resolver: this is the same
    /// `modules_dir` the loader probes, so an install can never land somewhere
    /// a `use` will not look.
    pub fn tree_arg(&self) -> PathBuf {
        cook_contracts::layout::modules_dir(&self.project_dir)
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
                CapturedStream::from_bytes(&out.stdout).as_str(),
                CapturedStream::from_bytes(&out.stderr).as_str(),
                out.status.code().map(|c| c.to_string()).unwrap_or_else(|| "signal".into()),
            ));
        }
        Ok(out)
    }
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
#[path = "tests/driver_tests.rs"]
mod tests;
