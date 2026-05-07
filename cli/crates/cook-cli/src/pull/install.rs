//! Install a parsed module tree into `dest_root/<name>/` with atomic writes
//! and conflict prompting.

use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};

use super::archive::{ArchiveEntry, ArchivePlan};
use super::errors::PullError;
use super::prompt::{ConflictAnswer, ConflictPrompter};

#[derive(Debug, Default, PartialEq, Eq)]
pub struct InstallStats {
    pub written: usize,
    pub overwritten: usize,
    pub skipped: usize,
}

/// Install the module named `name` from `plan` into `dest_root/<name>/`.
///
/// `force = true` skips the prompter and treats every conflict as Yes.
/// Returns `Ok(stats)` on success, `Err(AbortedByUser)` if the prompter answered
/// Quit at any conflict.
pub fn install_module(
    plan: &ArchivePlan,
    name: &str,
    dest_root: &Path,
    prompter: &mut dyn ConflictPrompter,
    force: bool,
) -> Result<InstallStats, PullError> {
    let entries = plan
        .modules
        .get(name)
        .ok_or_else(|| PullError::ModuleNotFound {
            name: name.to_string(),
            available: plan.module_names(),
        })?;

    let module_root = dest_root.join(name);
    let mut stats = InstallStats::default();
    let mut temp_files_written: Vec<PathBuf> = Vec::new();

    for entry in entries {
        let target = module_root.join(&entry.rel_path);

        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent).map_err(|e| PullError::Io {
                context: format!("create_dir_all {}", parent.display()),
                source: e,
            })?;
        }

        let exists = target.exists();
        if exists && !force {
            match prompter.prompt(&target) {
                ConflictAnswer::Yes | ConflictAnswer::All => {} // proceed
                ConflictAnswer::No => {
                    stats.skipped += 1;
                    continue;
                }
                ConflictAnswer::Quit => {
                    // Already-renamed files in earlier iterations stay overwritten by design;
                    // full rollback would require keeping backup copies of every previous
                    // target, which is out of scope for v1. We only clean unflushed temps.
                    cleanup_temps(&temp_files_written);
                    return Err(PullError::AbortedByUser);
                }
            }
        }

        let file_name = target
            .file_name()
            .expect("install target always has a final path component")
            .to_string_lossy();
        let tmp = target.with_file_name(format!(".{}.cook-pull-tmp", file_name));
        if let Err(e) = write_atomic(&tmp, &target, entry, &mut temp_files_written) {
            cleanup_temps(&temp_files_written);
            return Err(e);
        }

        if exists {
            stats.overwritten += 1;
        } else {
            stats.written += 1;
        }
    }

    Ok(stats)
}

fn write_atomic(
    tmp: &Path,
    target: &Path,
    entry: &ArchiveEntry,
    in_flight: &mut Vec<PathBuf>,
) -> Result<(), PullError> {
    {
        let mut f = File::create(tmp).map_err(|e| PullError::Io {
            context: format!("create temp {}", tmp.display()),
            source: e,
        })?;
        f.write_all(&entry.contents).map_err(|e| PullError::Io {
            context: format!("write temp {}", tmp.display()),
            source: e,
        })?;
        f.sync_all().map_err(|e| PullError::Io {
            context: format!("fsync temp {}", tmp.display()),
            source: e,
        })?;
    }
    in_flight.push(tmp.to_path_buf());
    fs::rename(tmp, target).map_err(|e| PullError::Io {
        context: format!("rename {} -> {}", tmp.display(), target.display()),
        source: e,
    })?;
    in_flight.pop();
    Ok(())
}

fn cleanup_temps(temps: &[PathBuf]) {
    for t in temps {
        let _ = fs::remove_file(t);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pull::archive::{ArchiveEntry, ArchivePlan};
    use crate::pull::prompt::{ConflictAnswer, ScriptedPrompter};
    use std::collections::BTreeMap;
    use tempfile::TempDir;

    fn plan_with(name: &str, files: &[(&str, &[u8])]) -> ArchivePlan {
        let mut modules = BTreeMap::new();
        let entries: Vec<ArchiveEntry> = files
            .iter()
            .map(|(p, b)| ArchiveEntry {
                rel_path: PathBuf::from(p),
                contents: b.to_vec(),
            })
            .collect();
        modules.insert(name.to_string(), entries);
        ArchivePlan { modules }
    }

    #[test]
    fn writes_fresh_module() {
        let dir = TempDir::new().unwrap();
        let plan = plan_with(
            "cpp",
            &[
                ("init.lua", b"-- cpp init"),
                ("helpers.lua", b"-- helpers"),
            ],
        );
        let mut prompter = ScriptedPrompter::new(vec![]);
        let stats =
            install_module(&plan, "cpp", dir.path(), &mut prompter, false).unwrap();
        assert_eq!(stats, InstallStats { written: 2, overwritten: 0, skipped: 0 });
        assert_eq!(prompter.asked.len(), 0);

        let init = dir.path().join("cpp/init.lua");
        let helpers = dir.path().join("cpp/helpers.lua");
        assert!(init.exists());
        assert!(helpers.exists());
        assert_eq!(fs::read(&init).unwrap(), b"-- cpp init");
    }

    #[test]
    fn module_not_found_is_error() {
        let dir = TempDir::new().unwrap();
        let plan = plan_with("cpp", &[("init.lua", b"x")]);
        let mut prompter = ScriptedPrompter::new(vec![]);
        let err = install_module(&plan, "rust", dir.path(), &mut prompter, false).unwrap_err();
        match err {
            PullError::ModuleNotFound { name, available } => {
                assert_eq!(name, "rust");
                assert_eq!(available, vec!["cpp"]);
            }
            other => panic!("wrong variant: {other:?}"),
        }
    }

    #[test]
    fn prompts_on_conflict_yes_overwrites() {
        let dir = TempDir::new().unwrap();
        fs::create_dir_all(dir.path().join("cpp")).unwrap();
        fs::write(dir.path().join("cpp/init.lua"), b"OLD").unwrap();

        let plan = plan_with("cpp", &[("init.lua", b"NEW")]);
        let mut prompter = ScriptedPrompter::new(vec![ConflictAnswer::Yes]);
        let stats =
            install_module(&plan, "cpp", dir.path(), &mut prompter, false).unwrap();
        assert_eq!(stats, InstallStats { written: 0, overwritten: 1, skipped: 0 });
        assert_eq!(fs::read(dir.path().join("cpp/init.lua")).unwrap(), b"NEW");
    }

    #[test]
    fn prompts_on_conflict_no_skips() {
        let dir = TempDir::new().unwrap();
        fs::create_dir_all(dir.path().join("cpp")).unwrap();
        fs::write(dir.path().join("cpp/init.lua"), b"OLD").unwrap();

        let plan = plan_with("cpp", &[("init.lua", b"NEW")]);
        let mut prompter = ScriptedPrompter::new(vec![ConflictAnswer::No]);
        let stats =
            install_module(&plan, "cpp", dir.path(), &mut prompter, false).unwrap();
        assert_eq!(stats, InstallStats { written: 0, overwritten: 0, skipped: 1 });
        assert_eq!(fs::read(dir.path().join("cpp/init.lua")).unwrap(), b"OLD");
    }

    #[test]
    fn prompts_on_conflict_quit_returns_aborted() {
        let dir = TempDir::new().unwrap();
        fs::create_dir_all(dir.path().join("cpp")).unwrap();
        fs::write(dir.path().join("cpp/init.lua"), b"OLD").unwrap();

        let plan = plan_with(
            "cpp",
            &[("init.lua", b"NEW"), ("helpers.lua", b"NEW")],
        );
        let mut prompter = ScriptedPrompter::new(vec![ConflictAnswer::Quit]);
        let err =
            install_module(&plan, "cpp", dir.path(), &mut prompter, false).unwrap_err();
        assert!(matches!(err, PullError::AbortedByUser));
        // Original content untouched.
        assert_eq!(fs::read(dir.path().join("cpp/init.lua")).unwrap(), b"OLD");
        // helpers.lua never written.
        assert!(!dir.path().join("cpp/helpers.lua").exists());
    }

    #[test]
    fn force_skips_prompter() {
        let dir = TempDir::new().unwrap();
        fs::create_dir_all(dir.path().join("cpp")).unwrap();
        fs::write(dir.path().join("cpp/init.lua"), b"OLD").unwrap();

        let plan = plan_with("cpp", &[("init.lua", b"NEW")]);
        let mut prompter = ScriptedPrompter::new(vec![]); // no answers — must not be called
        let stats =
            install_module(&plan, "cpp", dir.path(), &mut prompter, true).unwrap();
        assert_eq!(stats, InstallStats { written: 0, overwritten: 1, skipped: 0 });
        assert!(prompter.asked.is_empty());
    }

    #[test]
    fn writes_nested_paths() {
        let dir = TempDir::new().unwrap();
        let plan = plan_with(
            "cpp",
            &[
                ("init.lua", b"x"),
                ("lib/sub/deep.lua", b"y"),
            ],
        );
        let mut prompter = ScriptedPrompter::new(vec![]);
        let stats =
            install_module(&plan, "cpp", dir.path(), &mut prompter, false).unwrap();
        assert_eq!(stats.written, 2);
        assert!(dir.path().join("cpp/lib/sub/deep.lua").exists());
    }

    #[test]
    fn no_temp_files_remain_after_success() {
        let dir = TempDir::new().unwrap();
        let plan = plan_with("cpp", &[("init.lua", b"x")]);
        let mut prompter = ScriptedPrompter::new(vec![]);
        install_module(&plan, "cpp", dir.path(), &mut prompter, false).unwrap();

        let leftover: Vec<_> = fs::read_dir(dir.path().join("cpp"))
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| {
                e.file_name()
                    .to_string_lossy()
                    .ends_with(".cook-pull-tmp")
            })
            .collect();
        assert!(leftover.is_empty(), "found stray temp files: {leftover:?}");
    }

    #[test]
    fn same_stem_different_extension_does_not_collide() {
        let dir = TempDir::new().unwrap();
        let plan = plan_with(
            "cpp",
            &[
                ("lib.lua", b"-- lua"),
                ("lib.toml", b"# toml"),
            ],
        );
        let mut prompter = ScriptedPrompter::new(vec![]);
        let stats =
            install_module(&plan, "cpp", dir.path(), &mut prompter, false).unwrap();
        assert_eq!(stats.written, 2);
        assert_eq!(fs::read(dir.path().join("cpp/lib.lua")).unwrap(), b"-- lua");
        assert_eq!(fs::read(dir.path().join("cpp/lib.toml")).unwrap(), b"# toml");
    }
}
