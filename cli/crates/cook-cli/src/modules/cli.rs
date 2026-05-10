//! `cook modules` clap subcommand surface.
//!
//! Wires `install`, `remove`, `update`, `list`, `search` into the cook
//! binary's subcommand dispatch (one variant per subcommand).
//!
//! This module is the orchestration layer: it reads cook.toml via the
//! manifest module, reads/writes cook.lock via the lockfile module, and
//! drives ~/.cook/bin/luarocks via the driver module.

use std::path::PathBuf;

use anyhow::{anyhow, Context, Result};
use clap::{Args, Subcommand};

use crate::modules::driver::RocksDriver;
use crate::modules::lockfile::{self, Lockfile};
use crate::modules::manifest::{self, ManifestModules, ManifestRegistry};

#[derive(Args, Debug, Clone)]
pub struct ModulesArgs {
    #[command(subcommand)]
    pub cmd: ModulesCmd,

    /// One-shot prefix to `[registry].indexes` for this invocation.
    #[arg(long = "registry", global = true)]
    pub registry: Option<String>,

    /// Error on any prompt instead of asking.
    #[arg(long = "non-interactive", global = true)]
    pub non_interactive: bool,

    /// Non-interactive TOFU consent (CI).
    #[arg(long = "accept-trust", global = true)]
    pub accept_trust: bool,
}

#[derive(Subcommand, Debug, Clone)]
pub enum ModulesCmd {
    /// Realise cook.toml + cook.lock into ./cook_modules. With names, add and install them.
    Install {
        /// Optional rock names. With no args, installs the locked closure.
        names: Vec<String>,
    },
    /// Drop modules from cook.toml and prune cook_modules.
    Remove {
        names: Vec<String>,
    },
    /// Bump every dep within manifest constraints, or one named dep.
    Update {
        /// Optional rock name. With no arg, updates every dep.
        name: Option<String>,
    },
    /// Read cook.lock; print installed rocks.
    List,
    /// Search configured indexes for matching rocks.
    Search { query: String },
}

/// Public entry. Returns the process exit code.
pub fn run(args: ModulesArgs) -> i32 {
    match run_inner(args) {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("cook modules: {e:#}");
            1
        }
    }
}

fn run_inner(args: ModulesArgs) -> Result<()> {
    let project_dir = std::env::current_dir().context("cwd")?;
    let cook_toml = project_dir.join("cook.toml");
    let lockfile_path = project_dir.join("cook.lock");

    let (manifest, registry) = if cook_toml.exists() {
        manifest::parse_cook_toml(&cook_toml)?
    } else {
        (ManifestModules::default(), ManifestRegistry::default())
    };

    let mut indexes = registry.indexes.clone();
    if indexes.is_empty() {
        indexes = ManifestRegistry::default().indexes;
    }
    if let Some(override_url) = args.registry.clone() {
        indexes.insert(0, override_url);
    }

    let prefix = cook_prefix()?;
    let driver = RocksDriver::new(prefix, indexes, project_dir.clone());

    match args.cmd {
        ModulesCmd::Install { names } if names.is_empty() => {
            install_locked_closure(&driver, &manifest, &lockfile_path)
        }
        ModulesCmd::Install { names } => {
            install_named(&driver, &manifest, &cook_toml, &lockfile_path, &names)
        }
        ModulesCmd::Remove { names } => {
            remove_named(&driver, &manifest, &cook_toml, &lockfile_path, &names)
        }
        ModulesCmd::Update { name } => {
            update_one_or_all(&driver, &manifest, &lockfile_path, name)
        }
        ModulesCmd::List => list_installed(&lockfile_path),
        ModulesCmd::Search { query } => {
            for hit in driver.search(&query)? {
                println!("{}\t{}\t{}", hit.name, hit.version, hit.index);
            }
            Ok(())
        }
    }
}

fn cook_prefix() -> Result<PathBuf> {
    // ~/.cook/ — same convention as Phase 1's install layout.
    let home = dirs::home_dir().ok_or_else(|| anyhow!("HOME not set"))?;
    Ok(home.join(".cook"))
}

fn install_locked_closure(
    driver: &RocksDriver,
    manifest: &ManifestModules,
    lockfile_path: &std::path::Path,
) -> Result<()> {
    if !lockfile_path.exists() {
        // No lockfile and no positional args: do a fresh install of the manifest.
        for (name, constraint) in &manifest.modules {
            driver.install(name, constraint)?;
        }
        let lock = lockfile::introspect_closure(
            &lockfile_path
                .parent()
                .unwrap_or(std::path::Path::new("."))
                .join("cook_modules"),
            manifest,
        )?;
        lockfile::write(lockfile_path, &lock)?;
        return Ok(());
    }
    let lock = lockfile::read(lockfile_path)?;
    validate_lockfile_consistent(&lock, manifest)?;
    for locked in &lock.modules {
        driver.install_locked(locked)?;
    }
    Ok(())
}

fn install_named(
    driver: &RocksDriver,
    manifest: &ManifestModules,
    cook_toml: &std::path::Path,
    lockfile_path: &std::path::Path,
    names: &[String],
) -> Result<()> {
    let mut updated = manifest.clone();
    for name in names {
        let (rock, constraint) = parse_name_at_version(name);
        updated.modules.insert(rock.clone(), constraint.clone());
        driver.install(&rock, &constraint)?;
    }
    write_manifest_modules(cook_toml, &updated)?;
    let lock = lockfile::introspect_closure(
        &cook_toml
            .parent()
            .unwrap_or(std::path::Path::new("."))
            .join("cook_modules"),
        &updated,
    )?;
    lockfile::write(lockfile_path, &lock)?;
    Ok(())
}

fn remove_named(
    driver: &RocksDriver,
    manifest: &ManifestModules,
    cook_toml: &std::path::Path,
    lockfile_path: &std::path::Path,
    names: &[String],
) -> Result<()> {
    let mut updated = manifest.clone();
    for name in names {
        updated.modules.remove(name);
        driver.remove(name)?;
    }
    write_manifest_modules(cook_toml, &updated)?;
    let lock = lockfile::introspect_closure(
        &cook_toml
            .parent()
            .unwrap_or(std::path::Path::new("."))
            .join("cook_modules"),
        &updated,
    )?;
    lockfile::write(lockfile_path, &lock)?;
    Ok(())
}

fn update_one_or_all(
    driver: &RocksDriver,
    manifest: &ManifestModules,
    lockfile_path: &std::path::Path,
    name: Option<String>,
) -> Result<()> {
    let names: Vec<String> = match name {
        Some(n) => vec![n],
        None => manifest.modules.keys().cloned().collect(),
    };
    for n in &names {
        let constraint = manifest.modules.get(n).cloned().unwrap_or_else(|| "*".into());
        driver.install(n, &constraint)?;
    }
    let lock = lockfile::introspect_closure(
        &lockfile_path
            .parent()
            .unwrap_or(std::path::Path::new("."))
            .join("cook_modules"),
        manifest,
    )?;
    lockfile::write(lockfile_path, &lock)?;
    Ok(())
}

fn list_installed(lockfile_path: &std::path::Path) -> Result<()> {
    if !lockfile_path.exists() {
        eprintln!("no cook.lock found; nothing installed");
        return Ok(());
    }
    let lock = lockfile::read(lockfile_path)?;
    for m in &lock.modules {
        let kind = if m.direct { "direct" } else { "transitive" };
        println!("{}\t{}\t{}", m.name, m.version, kind);
    }
    Ok(())
}

fn validate_lockfile_consistent(lock: &Lockfile, manifest: &ManifestModules) -> Result<()> {
    let direct_in_lock: std::collections::BTreeSet<&str> = lock
        .modules
        .iter()
        .filter(|m| m.direct)
        .map(|m| m.name.as_str())
        .collect();
    let manifest_names: std::collections::BTreeSet<&str> =
        manifest.modules.keys().map(String::as_str).collect();
    if direct_in_lock != manifest_names {
        return Err(anyhow!(
            "cook.lock direct deps ({:?}) disagree with [modules] ({:?}); \
             run `cook modules install <name>` or `cook modules update`",
            direct_in_lock,
            manifest_names,
        ));
    }
    Ok(())
}

fn parse_name_at_version(spec: &str) -> (String, String) {
    if let Some((name, ver)) = spec.split_once('@') {
        (name.to_string(), ver.to_string())
    } else {
        (spec.to_string(), "*".to_string())
    }
}

fn write_manifest_modules(path: &std::path::Path, manifest: &ManifestModules) -> Result<()> {
    let mut existing = if path.exists() {
        std::fs::read_to_string(path).context("read cook.toml")?
    } else {
        String::new()
    };
    // Preserve [registry] block; replace [modules] section.
    if let Some(pos) = existing.find("[modules]") {
        // Trim from [modules] to next [...] or EOF.
        let after = &existing[pos..];
        let end_rel = after[1..]
            .find("\n[")
            .map(|p| pos + 1 + p)
            .unwrap_or(existing.len());
        existing.replace_range(pos..end_rel, "");
    }
    let mut block = String::from("[modules]\n");
    for (name, constraint) in &manifest.modules {
        let key = if name.contains('-') || name.contains('.') {
            format!("\"{}\"", name)
        } else {
            name.clone()
        };
        block.push_str(&format!("{} = \"{}\"\n", key, constraint));
    }
    if !existing.is_empty() && !existing.ends_with('\n') {
        existing.push('\n');
    }
    existing.push_str(&block);
    std::fs::write(path, existing).context("write cook.toml")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_name_at_version_separates_name_and_constraint() {
        assert_eq!(
            parse_name_at_version("cook_smoke@0.1.0-1"),
            ("cook_smoke".into(), "0.1.0-1".into())
        );
        assert_eq!(
            parse_name_at_version("cook_smoke"),
            ("cook_smoke".into(), "*".into())
        );
    }

    #[test]
    fn validate_lockfile_consistent_passes_on_match() {
        let mut manifest = ManifestModules::default();
        manifest.modules.insert("cook_smoke".into(), "*".into());
        let lock = Lockfile::new(vec![lockfile::LockedModule {
            name: "cook_smoke".into(),
            version: "0.1.0-1".into(),
            source: "https://rocks.usecook.com/cook_smoke-0.1.0-1.src.rock".into(),
            integrity: "sha256-x".into(),
            direct: true,
        }]);
        validate_lockfile_consistent(&lock, &manifest).expect("ok");
    }

    #[test]
    fn validate_lockfile_consistent_errors_on_drift() {
        let mut manifest = ManifestModules::default();
        manifest.modules.insert("cook_smoke".into(), "*".into());
        let lock = Lockfile::new(Vec::new());
        let err = validate_lockfile_consistent(&lock, &manifest).expect_err("must fail");
        assert!(format!("{:#}", err).contains("disagree"));
    }
}
