//! `cook pull` — fetch module subtrees from a configured HTTPS registry into
//! the project-local `cook_modules/` directory.

mod archive;
mod args;
mod config;
mod errors;
mod fetch;
mod install;
mod prompt;
mod trust;

pub use errors::PullError;

use std::collections::BTreeSet;
use std::io::{self, IsTerminal};
use std::path::{Path, PathBuf};

use args::PullArgs;
use prompt::{ConflictAnswer, ConflictPrompter, StdinPrompter};
use trust::TrustMode;

/// Public entry. Returns the process exit code.
pub fn run_from_argv(argv: &[String]) -> i32 {
    let pull_argv: Vec<String> = argv.iter().skip(1).cloned().collect();
    let args = match args::parse(&pull_argv) {
        Ok(a) => a,
        Err(e) => {
            // BadArgs with empty reason means clap already printed a diagnostic
            // (Task 3 suppression — keep this).
            if !matches!(&e, PullError::BadArgs { reason } if reason.is_empty()) {
                eprintln!("cook pull: {e}");
            }
            return e.exit_code();
        }
    };

    match run(args) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("cook pull: {e}");
            e.exit_code()
        }
    }
}

fn run(args: PullArgs) -> Result<i32, PullError> {
    let cwd = std::env::current_dir().map_err(|e| PullError::Io {
        context: "current_dir".into(),
        source: e,
    })?;
    let dest_root = cwd.join("cook_modules");

    let cfg_dir = config_dir()?;
    let cook_toml = cfg_dir.join("cook.toml");
    let trust_toml = cfg_dir.join("trust.toml");

    let env_url = std::env::var("COOK_REGISTRY_URL").ok();
    let registry_url = config::resolve_registry_url(
        args.registry.as_deref(),
        env_url.as_deref(),
        &cook_toml,
    )?;

    let is_tty = io::stdin().is_terminal();
    let trust_mode = resolve_trust_mode(&args, is_tty);
    let mut stdin_for_trust = io::stdin().lock();
    let mut stderr_for_trust = io::stderr().lock();
    trust::ensure_trusted(
        &registry_url,
        trust_mode,
        &trust_toml,
        &mut stdin_for_trust,
        &mut stderr_for_trust,
    )?;
    drop(stderr_for_trust);
    drop(stdin_for_trust);

    let archive_url = fetch::archive_url(&registry_url);
    let reader = fetch::fetch_archive(&archive_url)?;
    let plan = archive::parse_archive(reader)?;

    if args.list {
        for name in plan.module_names() {
            println!("{name}");
        }
        return Ok(0);
    }

    let names: Vec<String> = if args.all {
        plan.module_names()
    } else {
        let mut seen = BTreeSet::new();
        let mut deduped: Vec<String> = Vec::new();
        for n in &args.names {
            if seen.insert(n.clone()) {
                deduped.push(n.clone());
            }
        }
        deduped
    };

    // Pre-validate: every requested name must exist in the plan, before any writes.
    for name in &names {
        if !plan.modules.contains_key(name) {
            return Err(PullError::ModuleNotFound {
                name: name.clone(),
                available: plan.module_names(),
            });
        }
    }

    let interactive = !args.non_interactive && is_tty;

    if !args.force && !interactive {
        // Pre-scan for conflicts; non-interactive without --force cannot prompt.
        let conflicts = scan_conflicts(&plan, &names, &dest_root);
        if !conflicts.is_empty() {
            return Err(PullError::ConflictNonInteractive { paths: conflicts });
        }
    }

    // One prompter shared across modules so that "all-yes" sticks.
    // In non-interactive mode without --force we already scanned for conflicts
    // above and would have returned early if any existed.  A ForceYesPrompter
    // is safe here because install_module short-circuits before calling prompt()
    // when there is nothing to overwrite.
    let mut prompter: Box<dyn ConflictPrompter> = if args.force || !interactive {
        Box::new(ForceYesPrompter)
    } else {
        Box::new(StdinPrompter::new(io::stdin().lock(), io::stderr()))
    };

    for name in &names {
        let stats =
            install::install_module(&plan, name, &dest_root, prompter.as_mut(), args.force)?;
        eprintln!(
            "pulled {name} ({} written, {} overwritten, {} skipped)",
            stats.written, stats.overwritten, stats.skipped
        );
    }

    Ok(0)
}

fn config_dir() -> Result<PathBuf, PullError> {
    let base = dirs::config_dir().ok_or_else(|| PullError::Io {
        context: "locate user config dir".into(),
        source: io::Error::other("dirs::config_dir() returned None"),
    })?;
    Ok(base.join("cook"))
}

fn resolve_trust_mode(args: &PullArgs, is_tty: bool) -> TrustMode {
    if args.accept_trust {
        TrustMode::Accept
    } else if args.non_interactive || !is_tty {
        TrustMode::NonInteractive
    } else {
        TrustMode::Interactive
    }
}

fn scan_conflicts(
    plan: &archive::ArchivePlan,
    names: &[String],
    dest_root: &Path,
) -> Vec<PathBuf> {
    let mut hits = Vec::new();
    for name in names {
        if let Some(entries) = plan.modules.get(name) {
            for e in entries {
                let target = dest_root.join(name).join(&e.rel_path);
                if target.exists() {
                    hits.push(target);
                }
            }
        }
    }
    hits
}

/// Prompter used on the `--force` path. `install_module` short-circuits before
/// calling prompt() when `force = true`, so this is defensive.
struct ForceYesPrompter;
impl ConflictPrompter for ForceYesPrompter {
    fn prompt(&mut self, _path: &Path) -> ConflictAnswer {
        ConflictAnswer::Yes
    }
}
