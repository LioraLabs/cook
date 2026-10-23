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
use std::path::{Path, PathBuf};

use crate::RegisteredWorkspace;
use cook_contracts::WorkPayload;
use cook_probe::store::ProbeValueStore;

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

/// Where `output` (relative to `wd`, or absolute; a glob by its literal
/// directory prefix) really lands when that is outside `root`: the nearest
/// existing ancestor is canonicalized, so a symlinked parent is followed. A
/// dangling symlink on the way counts as outside (the kernel would create its
/// target). `root` must be canonical. Lexically outside counts too: an
/// absolute output elsewhere would be written by the populate run.
pub(crate) fn resolves_outside(root: &Path, wd: &Path, output: &str) -> Option<PathBuf> {
    let full = cook_contracts::pathlaw::normalize(&wd.join(output));
    let mut cur: PathBuf = full
        .components()
        .take_while(|c| !cook_contracts::pathlaw::has_glob_meta(&c.as_os_str().to_string_lossy()))
        .collect();
    loop {
        match std::fs::canonicalize(&cur) {
            Ok(real) => return (!real.starts_with(root)).then_some(real),
            Err(_) if std::fs::symlink_metadata(&cur).is_ok() => return Some(cur),
            Err(_) => {
                if !cur.pop() {
                    return None;
                }
            }
        }
    }
}

/// Rebase `p` from under `from` to under `to` by string prefix (not
/// `Path::strip_prefix`, which would drop the `./` segments recorded output
/// names keep). `None` when `p` is not `from` or inside it.
fn rebase(p: &str, from: &str, to: &str) -> Option<String> {
    match p.strip_prefix(from) {
        Some(rest) if rest.is_empty() || rest.starts_with('/') => Some(format!("{to}{rest}")),
        _ => None,
    }
}

/// Recursively copy `src` directory contents into `dst` (which must exist).
/// Skips the `.cook` cache dir and any `.git` dir to keep sandboxes cheap.
/// `real`/`sand` are the working dir and sandbox roots, for rebasing symlinks.
/// Symlinks are skipped on non-unix.
fn copy_dir_shallow_filtered(src: &Path, dst: &Path, real: &str, sand: &str) -> std::io::Result<()> {
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let name = entry.file_name();
        if name == cook_contracts::layout::DOT_COOK_DIR || name == ".git" {
            continue;
        }
        let from = entry.path();
        let to = dst.join(&name);
        let ft = entry.file_type()?;
        if ft.is_dir() {
            std::fs::create_dir_all(&to)?;
            copy_dir_shallow_filtered(&from, &to, real, sand)?;
        } else if ft.is_file() {
            std::fs::copy(&from, &to)?;
        } else if ft.is_symlink() {
            // Never followed. A relative link (pnpm's node_modules) resolves
            // inside the sandbox; an absolute target inside the working dir is
            // rebased into the sandbox so the re-run cannot reach the real
            // tree; an absolute target outside it stays verbatim (a system
            // path is shared, as it is for the real run).
            #[cfg(unix)]
            {
                let target = std::fs::read_link(&from)?;
                let target = target
                    .to_str()
                    .and_then(|t| rebase(t, real, sand))
                    .map(std::path::PathBuf::from)
                    .unwrap_or(target);
                std::os::unix::fs::symlink(target, &to)?;
            }
        }
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

/// What a re-run produced: the declared outputs' hashes and, when traced, the
/// strace log with the sandbox root rewritten to the real working dir.
#[derive(Debug, Clone)]
pub struct Rerun {
    pub outputs: BTreeMap<String, u64>,
    pub trace: Option<String>,
}

/// The tracer a traced re-run wraps the command in: `strace` on PATH.
pub fn tracer() -> Result<String, String> {
    cook_cache::resolve_tool_path("strace").ok_or_else(|| "strace not found on PATH".to_string())
}

/// Re-execute `cmd` outside the scheduler and return the content hash of each
/// declared output (glob-resolved) that exists after the run. Errors if the
/// command exits non-zero.
///
/// This is the one way a unit is re-executed outside the scheduler (verify and
/// audit both call it; nothing rebuilds the command). `cmd` is the payload's
/// raw command: probe sigils survive registration, so it is resolved here
/// against `probe_store` scoped to `recipe_name`, exactly as the worker pool
/// does, before the shell sees it.
///
/// Where it runs: inside an isolated overlay view of the project
/// ([`crate::sandbox::isolated_root`]) the unit runs in place, since writes
/// already land in the overlay; otherwise in a throwaway copy of
/// `working_dir`. A declared output that resolves outside that tree cannot be
/// reproduced and is an error, never read from the real tree.
///
/// In place, the declared outputs are removed first (after the check above, so
/// nothing outside the working directory is ever touched) so an incremental
/// tool redoes what a cold build does; `keep` (absolute, normalized) spares
/// any output that is or contains one of those paths (a declared input that is
/// also an output: an in-place formatter's sources).
/// ponytail: tool state outside the declared outputs (incremental caches) can
/// still make a re-run read less than a cold build, and a failed re-run leaves
/// its outputs gone for the units after it.
///
/// `traced` wraps the command in strace and returns the log.
pub fn rerun_in_sandbox(
    cmd: &str,
    probe_store: &ProbeValueStore,
    recipe_name: &str,
    working_dir: &Path,
    env_vars: &BTreeMap<String, String>,
    declared_outputs: &[String],
    keep: &BTreeSet<PathBuf>,
    traced: bool,
) -> Result<Rerun, String> {
    let cmd = cook_probe::sigil::resolve_probe_sigils(&probe_store.for_recipe(recipe_name), cmd)?;
    // Scratch (copy sandbox, trace file) lives under the user cache dir, not
    // the system temp dir (commonly tmpfs, i.e. RAM, and a real project's tree
    // can exceed it), and never inside the project: cargo/pnpm/git/tsc walk up
    // from the cwd, would find the real workspace, and the re-run would stop
    // being isolated. `dirs` honours XDG_CACHE_HOME. Inside an isolated view
    // the run's own scratch dir is used, which the parent removes on ^C too.
    let verify_root = crate::sandbox::scratch_dir()
        .unwrap_or_else(|| cook_cache::cloud_config::user_cache_root().join("verify"));
    std::fs::create_dir_all(&verify_root).map_err(|e| format!("sandbox: {e}"))?;
    // In place only when the working dir REALLY is inside the overlay: a
    // member reached through a symlink out of the project is the real tree.
    let isolated = crate::sandbox::isolated_root();
    let in_place = isolated
        .as_ref()
        .is_some_and(|root| std::fs::canonicalize(working_dir).is_ok_and(|wd| wd.starts_with(root)));
    let copy = if in_place {
        None
    } else {
        Some(tempfile::tempdir_in(&verify_root).map_err(|e| format!("sandbox: {e}"))?)
    };
    let sandbox_path = copy.as_ref().map_or(working_dir, |d| d.path());
    let (real, sand) = (working_dir.to_string_lossy(), sandbox_path.to_string_lossy());
    let real = match real.trim_end_matches('/') {
        "" => "/",
        t => t,
    };
    // Checked before anything runs. An output declared as an absolute path under the working dir (cook_pnpm
    // declares `<root>/././dist/**`) names the real tree; read it from the
    // sandbox instead, and name what it finds as the real run recorded it.
    let mut declared = Vec::with_capacity(declared_outputs.len());
    for p in declared_outputs {
        let p = rebase(p, real, &sand).unwrap_or_else(|| p.clone());
        // Lexical, so `<wd>/../x` does not pass the prefix check.
        let abs = sandbox_path.join(&p);
        if !cook_contracts::pathlaw::normalize(&abs).starts_with(sandbox_path) {
            return Err(format!(
                "output `{p}` is outside the unit's working directory; the re-run cannot reproduce it"
            ));
        }
        declared.push(p);
    }
    if let Some(overlay) = isolated.as_ref().filter(|_| in_place) {
        // The check above is lexical; the kernel follows a symlinked parent.
        // An output that really lands outside the overlay is refused, never run.
        let outside = |p: &str| {
            resolves_outside(overlay, sandbox_path, p).map(|real| {
                format!("output `{p}` resolves outside the isolated tree ({}); the audit cannot isolate it", real.display())
            })
        };
        if let Some(e) = declared.iter().find_map(|p| outside(p)) {
            return Err(e);
        }
        for found in crate::executor::resolve_output_paths(&declared, sandbox_path) {
            let abs = cook_contracts::pathlaw::normalize(&sandbox_path.join(&found));
            if abs == sandbox_path || keep.iter().any(|i| i.starts_with(&abs)) {
                continue;
            }
            if let Some(e) = outside(&found) {
                return Err(e);
            }
            match std::fs::symlink_metadata(&abs) {
                Ok(m) if m.is_dir() => drop(std::fs::remove_dir_all(&abs)),
                Ok(_) => drop(std::fs::remove_file(&abs)),
                Err(_) => {}
            }
        }
    }
    if copy.is_some() {
        copy_dir_shallow_filtered(working_dir, sandbox_path, real, &sand)
            .map_err(|e| format!("sandbox copy: {e}"))?;
    }

    let trace_file = if traced {
        let strace = tracer()?;
        let file = tempfile::NamedTempFile::new_in(&verify_root).map_err(|e| format!("trace file: {e}"))?;
        let q = cook_contracts::quoting::shell_quote;
        let wrapped = format!(
            "exec {} -f -qq -x -y --seccomp-bpf -e trace=%file,%process,fchdir -o {} /bin/sh -c {}",
            q(&strace),
            q(&file.path().to_string_lossy()),
            q(&cmd)
        );
        Some((file, wrapped))
    } else {
        None
    };
    let command = trace_file.as_ref().map_or(cmd.as_str(), |(_, w)| w.as_str());

    // CS-0188: this used `.status()` and captured nothing, so a shadow re-run
    // that failed could only ever be reported as a bare exit code. `cook cache
    // verify` exists to tell an author why two runs of one unit disagreed, and
    // answering "it exited 1" is the least useful form that answer can take.
    // Capturing costs nothing here — the re-run is already a full build of the
    // unit in a temporary tree — and the command's own diagnostics are usually
    // the whole explanation.
    // PWD names the dir the command runs in: inherited, it would name the
    // invoking shell's dir, which the shell stats (a read no unit declares).
    // OLDPWD likewise (bash stats it); the overlay cannot unset, so it names
    // the same dir.
    let mut env = env_vars.clone();
    for k in ["PWD", "OLDPWD"] {
        env.insert(k.to_string(), sandbox_path.to_string_lossy().into_owned());
    }
    let outcome = cook_shell::run(
        &cook_shell::Spawn {
            command,
            working_dir: sandbox_path,
            stdio: cook_shell::Stdio::Captured,
        },
        &env,
    )
    .map_err(|e| format!("spawn: {}", e.message()))?;
    if !outcome.success() {
        let code = outcome
            .exit_code()
            .map(|c| c.to_string())
            .unwrap_or_else(|| "signal".to_string());
        let detail = {
            let err = outcome.stderr_lossy();
            let text = if err.trim().is_empty() { outcome.stdout_lossy() } else { err };
            let text = text.trim();
            if text.is_empty() {
                String::new()
            } else {
                format!(": {text}")
            }
        };
        return Err(format!("re-run exited {code}{detail}"));
    }
    let trace = match &trace_file {
        Some((file, _)) => {
            let raw = std::fs::read(file.path()).map_err(|e| format!("trace file: {e}"))?;
            Some(String::from_utf8_lossy(&raw).replace(&*sand, real))
        }
        None => None,
    };

    let mut outputs = BTreeMap::new();
    for found in crate::executor::resolve_output_paths(&declared, sandbox_path) {
        if let Some(h) = cook_cache::hash_file(&sandbox_path.join(&found)) {
            outputs.insert(rebase(&found, &sand, real).unwrap_or(found), h);
        }
    }
    Ok(Rerun { outputs, trace })
}

/// `cook cache verify`'s entry to [`rerun_in_sandbox`], the one re-run path;
/// outputs only, untraced.
pub fn rerun_outputs_in_sandbox(
    cmd: &str,
    probe_store: &ProbeValueStore,
    recipe_name: &str,
    working_dir: &Path,
    env_vars: &BTreeMap<String, String>,
    declared_outputs: &[String],
) -> Result<BTreeMap<String, u64>, String> {
    rerun_in_sandbox(cmd, probe_store, recipe_name, working_dir, env_vars, declared_outputs, &BTreeSet::new(), false)
        .map(|r| r.outputs)
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
) -> Result<VerifyReport, String> {
    // 1. Populate: a normal run records StepEntry.outputs (path,hash) per unit.
    let run_result = crate::run::run(
        project_root,
        workspace,
        edges,
        reachable,
        num_jobs,
        &[],
        /*no_prune*/ false,
        /*no_publish*/ true,
        /*replay_logs*/ false,
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
        let prefix = cook_contracts::naming::import_prefix(recipe_name);
        let wd = workspace
            .working_dir_by_prefix
            .get(prefix)
            .cloned()
            .unwrap_or_else(|| std::path::PathBuf::from("."));
        let cache_dir = cook_contracts::layout::cache_dir(&wd);
        let mgr = cook_cache::ThreadSafeCacheManager::new(cache_dir);
        let index_name = cook_contracts::cache::recipe_cache_index_name(units, recipe_name);
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
                entry.outputs.iter().map(|f| (f.path.to_string(), f.hash)).collect();

            // R1 (CS-0164): config `var.*` values are never injected into a
            // step's process environment, so the determinism re-run must not
            // inject them either — it would diverge from the real execution.
            // Byte-verify handles only shell (`cook`) units, which carry no
            // per-unit exports, so an empty process env matches reality.
            let verdict = match rerun_outputs_in_sandbox(
                &cmd,
                &run_result.probe_values,
                recipe_name,
                &units.working_dir,
                &BTreeMap::new(),
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
#[path = "tests/verify_tests.rs"]
mod tests;
