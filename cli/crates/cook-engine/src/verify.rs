//! COOK-167 — `cook cache verify`: determinant-fidelity verifier.
//!
//! Re-runs each cacheable unit in a throwaway sandbox and byte-compares its
//! produced outputs against what the local cache recorded for the SAME key K
//! (populated by a normal `run()` against this very workspace, so K matches by
//! construction). A non-`record` unit whose re-run produces different bytes is a
//! divergence (§17.1.1 byte-equivalence broken — an under-keyed determinant or a
//! non-reproducible producer). A `record` unit (§17.1.4) is byte-exempt and gets
//! a weaker exists (re-produced) check. This is a fidelity/diagnostic tool, NOT a
//! trust gate (COOK-167).

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use crate::RegisteredWorkspace;
use cook_contracts::WorkPayload;

/// Per-unit verdict from the re-run-and-compare pass.
#[derive(Debug, Clone, PartialEq)]
pub enum UnitVerdict {
    /// Re-run reproduced byte-identical outputs under the matching key.
    Pass,
    /// `record` unit: byte-equivalence waived (§17.1.4); re-run re-produced
    /// the declared outputs (presence check; bytes intentionally not compared).
    RecordExempt,
    /// Re-run produced different bytes under a matching key — the cache would
    /// serve a non-byte-equivalent artifact. Under-keyed determinant or
    /// non-reproducible producer.
    Divergence { detail: String },
    /// The unit could not be verified (re-run failed, missing recorded entry,
    /// sandbox error). Distinct from a divergence — the producer broke.
    Error { detail: String },
}

impl UnitVerdict {
    pub fn is_ok(&self) -> bool {
        matches!(self, UnitVerdict::Pass | UnitVerdict::RecordExempt)
    }
    pub fn label(&self) -> &'static str {
        match self {
            UnitVerdict::Pass => "pass",
            UnitVerdict::RecordExempt => "pass (record: byte-check waived)",
            UnitVerdict::Divergence { .. } => "DIVERGENCE",
            UnitVerdict::Error { .. } => "ERROR",
        }
    }
}

#[derive(Debug, Clone)]
pub struct UnitReport {
    pub recipe: String,
    pub unit: String,
    pub key: String,
    pub verdict: UnitVerdict,
}

#[derive(Debug, Clone, Default)]
pub struct VerifyReport {
    pub units: Vec<UnitReport>,
}

impl VerifyReport {
    /// 0 iff every unit verdict is_ok().
    pub fn exit_code(&self) -> i32 {
        if self.units.iter().all(|u| u.verdict.is_ok()) { 0 } else { 1 }
    }
    pub fn divergences(&self) -> usize {
        self.units.iter().filter(|u| matches!(u.verdict, UnitVerdict::Divergence { .. })).count()
    }
    pub fn errors(&self) -> usize {
        self.units.iter().filter(|u| matches!(u.verdict, UnitVerdict::Error { .. })).count()
    }
}

/// Recursively copy `src` directory contents into `dst` (which must exist).
/// Skips the `.cook` cache dir and any `.git` dir to keep sandboxes cheap.
fn copy_dir_shallow_filtered(src: &Path, dst: &Path) -> std::io::Result<()> {
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let name = entry.file_name();
        if name == ".cook" || name == ".git" {
            continue;
        }
        let from = entry.path();
        let to = dst.join(&name);
        let ft = entry.file_type()?;
        if ft.is_dir() {
            std::fs::create_dir_all(&to)?;
            copy_dir_shallow_filtered(&from, &to)?;
        } else if ft.is_file() {
            std::fs::copy(&from, &to)?;
        }
        // symlinks: best-effort skip
    }
    Ok(())
}

/// Pure verdict: compare a unit's recorded output hashes against the re-run's.
/// `record` => byte-equivalence waived (§17.1.4): a re-produced, present output
/// passes regardless of bytes; an absent recorded output is an Error (producer
/// failed to make the artifact at all). Non-`record` => any hash mismatch or a
/// recorded output absent from the re-run is a Divergence.
pub fn classify(
    record: bool,
    recorded: &BTreeMap<String, u64>,
    rerun: &BTreeMap<String, u64>,
) -> UnitVerdict {
    if record {
        for path in recorded.keys() {
            if !rerun.contains_key(path) {
                return UnitVerdict::Error {
                    detail: format!("record unit failed to re-produce output `{path}`"),
                };
            }
        }
        return UnitVerdict::RecordExempt;
    }
    for (path, recorded_hash) in recorded {
        match rerun.get(path) {
            None => {
                return UnitVerdict::Divergence {
                    detail: format!("output `{path}` absent after re-run under matching key"),
                };
            }
            Some(rerun_hash) if rerun_hash != recorded_hash => {
                return UnitVerdict::Divergence {
                    detail: format!(
                        "output `{path}` bytes differ under matching key (cache {recorded_hash:016x} != re-run {rerun_hash:016x})"
                    ),
                };
            }
            Some(_) => {}
        }
    }
    UnitVerdict::Pass
}

/// Re-execute `cmd` in a throwaway copy of `working_dir`, then return the
/// content hash of each declared output (glob-resolved) that exists after the
/// run. Errors if the command exits non-zero.
pub fn rerun_outputs_in_sandbox(
    cmd: &str,
    working_dir: &Path,
    env_vars: &BTreeMap<String, String>,
    declared_outputs: &[String],
) -> Result<BTreeMap<String, u64>, String> {
    let sandbox = tempfile::tempdir().map_err(|e| format!("sandbox: {e}"))?;
    copy_dir_shallow_filtered(working_dir, sandbox.path())
        .map_err(|e| format!("sandbox copy: {e}"))?;

    let mut child_env: std::collections::HashMap<String, String> = std::env::vars().collect();
    for (k, v) in env_vars {
        child_env.insert(k.clone(), v.clone());
    }
    let status = std::process::Command::new("/bin/sh")
        .arg("-c")
        .arg(cmd)
        .current_dir(sandbox.path())
        .envs(&child_env)
        .status()
        .map_err(|e| format!("spawn: {e}"))?;
    if !status.success() {
        return Err(format!("re-run exited {}", status.code().unwrap_or(-1)));
    }

    let mut out = BTreeMap::new();
    for decl in declared_outputs {
        let resolved: Vec<String> = if cook_fingerprint::has_glob_meta(decl) {
            cook_fingerprint::resolve_glob(sandbox.path(), decl).into_iter().collect()
        } else {
            vec![decl.clone()]
        };
        for rel in resolved {
            let abs = sandbox.path().join(&rel);
            if let Some(h) = cook_fingerprint::hash_file(&abs) {
                out.insert(rel, h);
            }
        }
    }
    Ok(out)
}

/// Run the recipe to populate the cache, then re-run every cacheable unit in a
/// sandbox and compare against the recorded outputs under the matching key.
///
/// `edges` / `reachable` are the same maps `run()` consumes. K matches by
/// construction because the populate run and the re-run see the same workspace,
/// so any byte divergence is purely a producer/determinant property.
pub fn verify_cache(
    project_root: &Path,
    workspace: &RegisteredWorkspace,
    edges: &BTreeMap<String, Vec<String>>,
    reachable: &BTreeSet<String>,
    num_jobs: usize,
    _recipe: &str,
) -> Result<VerifyReport, String> {
    // 1. Populate: a normal run records StepEntry.outputs (path,hash) per unit.
    crate::run::run(
        project_root,
        workspace,
        edges,
        reachable,
        num_jobs,
        &[],
        /*no_prune*/ false,
        /*no_publish*/ true,
        |_e| {},
    )
    .map_err(|e| format!("populate run failed: {e}"))?;

    let mut report = VerifyReport::default();
    for (recipe_name, units) in &workspace.units_by_recipe {
        // Only verify recipes that were part of this run's reachable closure.
        if !reachable.contains(recipe_name) {
            continue;
        }
        // Per-recipe cache manager, anchored exactly like the executor.
        let prefix = crate::run::split_recipe_name(recipe_name).0;
        let wd = workspace
            .working_dir_by_prefix
            .get(&prefix)
            .cloned()
            .unwrap_or_else(|| std::path::PathBuf::from("."));
        let cache_dir = wd.join(".cook").join("cache");
        let mgr = cook_cache::ThreadSafeCacheManager::new(cache_dir);
        let index_name = crate::run::recipe_cache_index_name(units, recipe_name);
        let recipe_cache = mgr.get_or_load(&index_name);

        for unit in &units.units {
            let Some(meta) = &unit.cache_meta else {
                continue;
            };
            if meta.output_paths.is_empty() {
                continue;
            }
            let cmd = match &unit.payload {
                WorkPayload::Shell { cmd, .. } => cmd.clone(),
                _ => continue, // only shell units are byte-verifiable here
            };
            let Some(entry) = recipe_cache.steps.get(&meta.cache_key) else {
                continue; // not cached this run — nothing to verify
            };
            let recorded: BTreeMap<String, u64> =
                entry.outputs.iter().map(|f| (f.path.clone(), f.hash)).collect();

            let verdict = match rerun_outputs_in_sandbox(
                &cmd,
                &units.working_dir,
                &units.env_vars,
                &meta.output_paths,
            ) {
                Ok(rerun) => classify(meta.record, &recorded, &rerun),
                Err(e) => UnitVerdict::Error { detail: e },
            };
            report.units.push(UnitReport {
                recipe: recipe_name.clone(),
                unit: meta.output_paths.first().cloned().unwrap_or_default(),
                key: meta.cache_key.clone(),
                verdict,
            });
        }
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verify_cache_entry_exists() {
        let _f: fn(
            &std::path::Path,
            &crate::RegisteredWorkspace,
            &std::collections::BTreeMap<String, Vec<String>>,
            &std::collections::BTreeSet<String>,
            usize,
            &str,
        ) -> Result<VerifyReport, String> = verify_cache;
        let _ = _f;
    }

    #[test]
    fn rerun_in_sandbox_hashes_declared_outputs() {
        let dir = tempfile::tempdir().unwrap();
        let env = std::collections::BTreeMap::new();
        let res = rerun_outputs_in_sandbox(
            "printf 'hello' > out.txt",
            dir.path(),
            &env,
            &["out.txt".to_string()],
        )
        .expect("rerun should succeed");
        assert_eq!(res.len(), 1);
        let h = res.get("out.txt").copied().expect("out.txt hashed");
        let res2 = rerun_outputs_in_sandbox(
            "printf 'hello' > out.txt",
            dir.path(),
            &env,
            &["out.txt".to_string()],
        )
        .unwrap();
        assert_eq!(res2.get("out.txt").copied(), Some(h));
    }

    #[test]
    fn rerun_nondeterministic_producer_changes_hash() {
        let dir = tempfile::tempdir().unwrap();
        let env = std::collections::BTreeMap::new();
        let cmd = "date +%s%N > out.txt";
        let a = rerun_outputs_in_sandbox(cmd, dir.path(), &env, &["out.txt".to_string()]).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(5));
        let b = rerun_outputs_in_sandbox(cmd, dir.path(), &env, &["out.txt".to_string()]).unwrap();
        assert_ne!(a.get("out.txt"), b.get("out.txt"), "nondeterministic producer must differ");
    }

    #[test]
    fn rerun_failed_command_is_err() {
        let dir = tempfile::tempdir().unwrap();
        let env = std::collections::BTreeMap::new();
        let r = rerun_outputs_in_sandbox("exit 7", dir.path(), &env, &["out.txt".to_string()]);
        assert!(r.is_err());
    }

    #[test]
    fn verdict_pass_is_ok_and_record_exempt_is_ok() {
        assert!(UnitVerdict::Pass.is_ok());
        assert!(UnitVerdict::RecordExempt.is_ok());
        assert!(!UnitVerdict::Divergence { detail: "x".into() }.is_ok());
        assert!(!UnitVerdict::Error { detail: "y".into() }.is_ok());
    }

    #[test]
    fn report_exit_code_zero_iff_all_ok() {
        let mut r = VerifyReport::default();
        r.units.push(UnitReport { recipe: "build".into(), unit: "a.o".into(), key: "k".into(), verdict: UnitVerdict::Pass });
        assert_eq!(r.exit_code(), 0);
        r.units.push(UnitReport { recipe: "build".into(), unit: "b.o".into(), key: "k2".into(), verdict: UnitVerdict::Divergence { detail: "bytes differ".into() } });
        assert_ne!(r.exit_code(), 0);
    }

    #[test]
    fn matching_bytes_pass() {
        let recorded: BTreeMap<String, u64> = [("a.o".to_string(), 42u64)].into();
        let rerun: BTreeMap<String, u64> = [("a.o".to_string(), 42u64)].into();
        assert_eq!(classify(false, &recorded, &rerun), UnitVerdict::Pass);
    }

    #[test]
    fn differing_bytes_diverge_for_non_record() {
        let recorded: BTreeMap<String, u64> = [("a.o".to_string(), 42u64)].into();
        let rerun: BTreeMap<String, u64> = [("a.o".to_string(), 99u64)].into();
        match classify(false, &recorded, &rerun) {
            UnitVerdict::Divergence { detail } => assert!(detail.contains("a.o")),
            v => panic!("expected divergence, got {v:?}"),
        }
    }

    #[test]
    fn record_unit_byte_difference_is_exempt() {
        let recorded: BTreeMap<String, u64> = [("img.png".to_string(), 42u64)].into();
        let rerun: BTreeMap<String, u64> = [("img.png".to_string(), 99u64)].into();
        assert_eq!(classify(true, &recorded, &rerun), UnitVerdict::RecordExempt);
    }

    #[test]
    fn record_unit_missing_output_is_error() {
        let recorded: BTreeMap<String, u64> = [("img.png".to_string(), 42u64)].into();
        let rerun: BTreeMap<String, u64> = BTreeMap::new();
        match classify(true, &recorded, &rerun) {
            UnitVerdict::Error { .. } => {}
            v => panic!("expected error, got {v:?}"),
        }
    }

    #[test]
    fn non_record_missing_output_is_divergence() {
        let recorded: BTreeMap<String, u64> = [("a.o".to_string(), 42u64)].into();
        let rerun: BTreeMap<String, u64> = BTreeMap::new();
        match classify(false, &recorded, &rerun) {
            UnitVerdict::Divergence { .. } => {}
            v => panic!("expected divergence, got {v:?}"),
        }
    }
}
