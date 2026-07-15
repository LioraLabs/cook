//! cook — the user-facing binary for the Cook build system.

mod cli;
mod completion;
mod error;
mod iso8601;
mod pipeline;
mod progress;
mod test_reporter;
mod test_state;
mod watcher;

use clap::CommandFactory;
use cook_cli::diagnostics;
use cook_cli::modules;

use cli::{Cli, Cmd};
use error::CookError;
use pipeline::{
    cmd_affected, cmd_cache_verify, cmd_dag, cmd_emit_lua, cmd_init, cmd_list, cmd_menu, cmd_run,
    cmd_serve, cmd_test, cmd_why, resolve_project_root, set_invoked_builtin,
    warn_if_builtin_shadows_recipe,
};

fn main() {
    // Must precede argument parsing: when the shell is asking for candidates
    // the argv is a completion request, not a command, and this never returns.
    completion::complete();

    let version_string: &'static str = Box::leak(Box::new(format!(
        "{} (Cook Standard v{})",
        env!("CARGO_PKG_VERSION"),
        cook_lang::COOK_STANDARD_VERSION,
    )));
    let cli_command = <Cli as CommandFactory>::command().version(version_string);
    let matches = cli_command.get_matches();
    let mut cli = <Cli as clap::FromArgMatches>::from_arg_matches(&matches)
        .expect("clap derive guarantees this conversion");
    if cli.globals.verbose {
        std::env::set_var("COOK_BACKTRACE", "1");
    }
    let output_json = cli.globals.output == "json";
    let file_explicit = cookfile_flag_was_explicit(&matches);
    let result = apply_entry_discovery(&mut cli, file_explicit).and_then(|()| dispatch(cli));

    if let Err(e) = result {
        let msg = diagnostics::sanitize_error(&e.to_string(), diagnostics::backtrace_enabled());
        if output_json {
            eprintln!("{}", diagnostics::json_diagnostic(e.code(), &msg));
        } else if !matches!(e, CookError::TestFailure(_)) {
            // TestFailure: the summary line already conveys the failure count;
            // printing the error message again would be noise.
            eprintln!("cook: {msg}");
        }
        std::process::exit(e.exit_code());
    }
}

/// True when `-f/--file` was given on the command line (any position —
/// clap propagates `global = true` args to the top-level matches).
fn cookfile_flag_was_explicit(matches: &clap::ArgMatches) -> bool {
    matches.value_source("file") == Some(clap::parser::ValueSource::CommandLine)
}

/// Upward Cookfile discovery (§20.2 / CS-0120): when no `-f/--file` was
/// given and the default `Cookfile` is absent in cwd, walk up to the nearest
/// Cookfile and make it the entry point. `cook init` (creates a Cookfile
/// here) and `cook modules` (cwd-scoped cook.toml management) are exempt.
fn apply_entry_discovery(cli: &mut Cli, file_explicit: bool) -> Result<(), CookError> {
    if file_explicit || matches!(cli.cmd, Some(Cmd::Init) | Some(Cmd::Modules(_))) {
        return Ok(());
    }
    if cli.globals.file.is_file() {
        return Ok(()); // nearest Cookfile is cwd — identical to today
    }
    let cwd = std::env::current_dir().map_err(|e| CookError::Other(e.to_string()))?;
    let found = cook_engine::pipeline::discover_entry_cookfile(
        &cwd,
        cli.globals.root.as_deref(),
    )
    .map_err(|e| CookError::Other(e.to_string()))?;
    cli.globals.file = found;
    Ok(())
}

fn dispatch(cli: Cli) -> Result<(), CookError> {
    let Cli { globals, cmd } = cli;
    // Single chokepoint for the reserved `//<name>` root-anchored target
    // syntax (§20.2.4 / CS-0120): every target-typed subcommand field routes
    // through `Cmd::reserved_target`. The external_subcommand `Recipe` arm
    // validates in `dispatch_recipe` after stripping the `+` escape.
    if let Some(target) = cmd.as_ref().and_then(|c| c.reserved_target()) {
        reject_reserved_root_target(target)?;
    }
    if let Some(name) = cmd.as_ref().and_then(Cmd::built_in_name) {
        set_invoked_builtin(name);
    }
    if matches!(
        cmd.as_ref(),
        Some(Cmd::Init | Cmd::Modules(_) | Cmd::Logs(_) | Cmd::EmitLua)
    ) {
        warn_if_builtin_shadows_recipe(&globals);
    }
    match cmd {
        None => cmd_run(&globals, "build", &[], None),
        Some(Cmd::Init) => cmd_init(),
        Some(Cmd::Menu) => cmd_menu(&globals),
        Some(Cmd::List(args)) => cmd_list(&globals, &args),
        Some(Cmd::Modules(args)) => std::process::exit(modules::run(args)),
        Some(Cmd::Test(args)) => cmd_test(&globals, &args),
        Some(Cmd::Dag(args)) => cmd_dag(&globals, &args),
        Some(Cmd::Logs(args)) => {
            let selector = if args.last_failed {
                cook_logs::BuildSelector::LastFailed
            } else if let Some(id) = args.build_id.clone() {
                cook_logs::BuildSelector::ByBuildId(id)
            } else if let Some(n) = args.nth {
                cook_logs::BuildSelector::Nth(n)
            } else {
                cook_logs::BuildSelector::Latest
            };
            let project_root = resolve_project_root(&globals)?;
            cook_logs::cmd_logs(&project_root, selector, cook_logs::Theme::default())
                .map_err(|e| CookError::Other(e.to_string()))
        }
        Some(Cmd::Cache(args)) => match args.cmd {
            crate::cli::CacheCmd::Verify(v) => cmd_cache_verify(&globals, &v),
        },
        Some(Cmd::Serve(args)) => {
            let recipe = args.recipe.as_deref().unwrap_or("build");
            cmd_serve(&globals, recipe, args.config.as_deref())
        }
        Some(Cmd::EmitLua) => cmd_emit_lua(&globals),
        Some(Cmd::Affected(args)) => cmd_affected(&globals, &args),
        Some(Cmd::Why(args)) => {
            let recipe = args.recipe.as_deref().unwrap_or("build");
            cmd_why(&globals, recipe, args.config.as_deref(), args.json)
        }
        Some(Cmd::Recipe(parts)) => dispatch_recipe(&globals, &parts),
    }
}

fn dispatch_recipe(globals: &cli::Globals, parts: &[String]) -> Result<(), CookError> {
    // clap's #[command(external_subcommand)] guarantees `parts` is non-empty
    // when this variant matches; an empty-args invocation goes through the
    // `None` arm of the outer dispatch match (default recipe "build").
    //
    // The `+` sigil escapes a recipe name that would otherwise dispatch to a
    // built-in subcommand (spec §"Recipe escape syntax"). We strip a single
    // leading `+`; `cook ++foo` therefore runs a recipe literally named
    // `+foo`, which is defensible and consistent with the spec's "leading
    // `+`" wording.
    //
    // COOK-36 Task 9: partition_argv splits out @PRESET / --config / -c
    // markers and the `--` end-of-options separator from the positional argv.
    let (first, rest) = parts
        .split_first()
        .expect("external_subcommand variant always carries ≥1 element");

    let recipe = first.strip_prefix('+').unwrap_or(first).to_string();
    reject_reserved_root_target(&recipe)?;

    // clap's external_subcommand captures every post-recipe token into the
    // Vec verbatim (it can't know which are global flags vs. chore params);
    // partition_argv re-extracts the global flags and applies them onto
    // `merged` in place so that `cook build -v` / `cook build --affected
    // --since=main` (Turborepo-style, flags after the recipe name) are
    // honoured the same as `cook -v build` / `cook --affected --since=main build`.
    let mut merged = globals.clone();
    let partitioned = partition_argv(rest, &recipe, &mut merged)?;

    cmd_run(&merged, &recipe, &partitioned.argv, partitioned.preset.as_deref())
}

/// Result of partitioning a recipe's positional argv into the runtime-meaningful
/// pieces. `argv` is the user-facing remainder (chore params etc.); `preset`
/// is the config preset pulled from `@TOKEN` / `--config` forms. Every other
/// `Globals` flag that clap couldn't intercept (because it appeared after the
/// `external_subcommand` catch-all) is applied directly onto the caller's
/// `Globals` in place — see `partition_argv`.
struct PartitionedArgv {
    argv: Vec<String>,
    preset: Option<String>,
}

/// COOK-36 Task 9 + COOK-58 + COOK-193 Task 1: partition the positionals
/// after the recipe name into argv and preset, applying every trailing
/// global flag (`--affected`, `--since`, `-v`, `-q`, `-j`, `--color`,
/// `--output`, `--set`, `-f`, `--root`, `--no-prune`, `--no-publish`) onto
/// `globals` in place.
///
/// Preset can come from `@TOKEN` sigil or `--config NAME` / `-c NAME` /
/// `--config=NAME` flag forms. `--since=<ref>` and `--since <ref>` both
/// accepted. The `--` end-of-options separator switches off sigil/flag
/// interpretation for the rest of the line (everything after it is a literal
/// chore param). At most one preset is permitted across all forms.
///
/// This peel is safe because chore params are always `key=value` or bare
/// words — never `-`/`--`-shaped — so any `-flag` token appearing after the
/// recipe name unambiguously belongs to `Globals`, not to the recipe.
fn partition_argv(
    rest: &[String],
    recipe: &str,
    globals: &mut cli::Globals,
) -> Result<PartitionedArgv, CookError> {
    let mut argv: Vec<String> = Vec::new();
    let mut preset: Option<String> = None;
    let mut end_of_options = false;
    let mut iter = rest.iter();
    while let Some(tok) = iter.next() {
        if end_of_options {
            argv.push(tok.clone());
            continue;
        }
        if tok == "--" {
            end_of_options = true;
            continue;
        }
        // @PRESET sigil — but only when the token is `@<bare-ident-shape>`.
        // A token like `@something/else` is treated as a literal param value.
        if let Some(name) = tok.strip_prefix('@') {
            if !name.is_empty() && name.chars().all(is_preset_char) {
                if preset.is_some() {
                    return Err(CookError::Other(format!(
                        "chore '{recipe}': multiple config presets supplied; use only one of '@PRESET' or '--config PRESET'"
                    )));
                }
                preset = Some(name.to_string());
                continue;
            }
        }
        // --config NAME / -c NAME (two-token form)
        if tok == "--config" || tok == "-c" {
            let next = iter.next().ok_or_else(|| {
                CookError::Other(format!("'{tok}' requires an argument"))
            })?;
            if preset.is_some() {
                return Err(CookError::Other(format!(
                    "chore '{recipe}': multiple config presets supplied; use only one of '@PRESET' or '--config PRESET'"
                )));
            }
            preset = Some(next.clone());
            continue;
        }
        // --config=NAME single-token form
        if let Some(name) = tok.strip_prefix("--config=") {
            if preset.is_some() {
                return Err(CookError::Other(format!(
                    "chore '{recipe}': multiple config presets supplied; use only one of '@PRESET' or '--config PRESET'"
                )));
            }
            preset = Some(name.to_string());
            continue;
        }
        match tok.as_str() {
            "-v" | "--verbose" => {
                globals.verbose = true;
                continue;
            }
            "-q" | "--quiet" => {
                globals.quiet = true;
                continue;
            }
            "--no-prune" => {
                globals.no_prune = true;
                continue;
            }
            "--no-publish" => {
                globals.no_publish = true;
                continue;
            }
            "--affected" => {
                globals.affected = true;
                continue;
            }
            "-j" | "--jobs" => {
                let n = iter
                    .next()
                    .ok_or_else(|| CookError::Other(format!("'{tok}' requires a number")))?;
                globals.jobs = Some(n.parse().map_err(|_| {
                    CookError::Other(format!("'{tok}' expects an integer, got '{n}'"))
                })?);
                continue;
            }
            "--color" => {
                globals.color = iter
                    .next()
                    .ok_or_else(|| CookError::Other("'--color' requires a value".into()))?
                    .clone();
                continue;
            }
            "--output" => {
                globals.output = iter
                    .next()
                    .ok_or_else(|| CookError::Other("'--output' requires a value".into()))?
                    .clone();
                continue;
            }
            "-f" | "--file" => {
                globals.file = iter
                    .next()
                    .ok_or_else(|| CookError::Other(format!("'{tok}' requires a path")))?
                    .into();
                continue;
            }
            "--root" => {
                globals.root = Some(
                    iter.next()
                        .ok_or_else(|| CookError::Other("'--root' requires a path".into()))?
                        .into(),
                );
                continue;
            }
            "--set" => {
                globals.set.push(
                    iter.next()
                        .ok_or_else(|| CookError::Other("'--set' requires KEY=VALUE".into()))?
                        .clone(),
                );
                continue;
            }
            "--since" => {
                globals.since = Some(
                    iter.next()
                        .ok_or_else(|| CookError::Other("'--since' requires a git ref".into()))?
                        .clone(),
                );
                continue;
            }
            other => {
                if let Some(v) = other.strip_prefix("--jobs=") {
                    globals.jobs = Some(v.parse().map_err(|_| {
                        CookError::Other(format!("'--jobs' expects an integer, got '{v}'"))
                    })?);
                    continue;
                }
                if let Some(v) = other.strip_prefix("--color=") {
                    globals.color = v.to_string();
                    continue;
                }
                if let Some(v) = other.strip_prefix("--output=") {
                    globals.output = v.to_string();
                    continue;
                }
                if let Some(v) = other.strip_prefix("--file=") {
                    globals.file = v.into();
                    continue;
                }
                if let Some(v) = other.strip_prefix("--root=") {
                    globals.root = Some(v.into());
                    continue;
                }
                if let Some(v) = other.strip_prefix("--set=") {
                    globals.set.push(v.to_string());
                    continue;
                }
                if let Some(v) = other.strip_prefix("--since=") {
                    globals.since = Some(v.to_string());
                    continue;
                }
            }
        }
        argv.push(tok.clone());
    }
    Ok(PartitionedArgv { argv, preset })
}

fn is_preset_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.'
}

/// §20.2.4 / CS-0120 reserved syntax: a `//`-prefixed CLI target names a
/// workspace-root-anchored target. v1 reserves the syntax without
/// implementing resolution — reject with a clear diagnostic instead of
/// misparsing the name as a recipe literal.
fn reject_reserved_root_target(target: &str) -> Result<(), CookError> {
    if let Some(rest) = target.strip_prefix("//") {
        return Err(CookError::Other(format!(
            "'//{rest}': root-anchored targets ('//<name>') are reserved syntax and not yet supported; \
             run `cook {rest}` from the workspace root instead"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod entry_discovery_tests {
    use super::*;
    use clap::CommandFactory;

    fn matches_for(argv: &[&str]) -> clap::ArgMatches {
        let mut full = vec!["cook"];
        full.extend_from_slice(argv);
        <cli::Cli as CommandFactory>::command()
            .try_get_matches_from(full)
            .expect("parse")
    }

    #[test]
    fn default_file_is_not_explicit() {
        assert!(!cookfile_flag_was_explicit(&matches_for(&["build"])));
        assert!(!cookfile_flag_was_explicit(&matches_for(&["menu"])));
        assert!(!cookfile_flag_was_explicit(&matches_for(&[])));
    }

    #[test]
    fn pre_subcommand_flag_is_explicit() {
        assert!(cookfile_flag_was_explicit(&matches_for(&[
            "-f", "sub/Cookfile", "build"
        ])));
    }

    #[test]
    fn post_subcommand_global_flag_is_explicit() {
        // global=true args given after a named subcommand propagate up to the
        // top-level matches (pinned by cli.rs::globals_apply_after_subcommand);
        // value_source must see them as CommandLine too.
        assert!(cookfile_flag_was_explicit(&matches_for(&[
            "test", "-f", "sub/Cookfile"
        ])));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn partition_peels_trailing_global_bool_flag() {
        let mut g = crate::cli::Globals::default();
        let p = partition_argv(&["-v".to_string()], "report", &mut g).unwrap();
        assert!(p.argv.is_empty());
        assert!(g.verbose);
    }

    #[test]
    fn partition_peels_trailing_global_value_flag() {
        let mut g = crate::cli::Globals::default();
        let p = partition_argv(
            &[
                "--output".to_string(),
                "plain".to_string(),
                "-j".to_string(),
                "4".to_string(),
            ],
            "report",
            &mut g,
        )
        .unwrap();
        assert!(p.argv.is_empty());
        assert_eq!(g.output, "plain");
        assert_eq!(g.jobs, Some(4));
    }

    #[test]
    fn partition_keeps_chore_params_as_argv() {
        let mut g = crate::cli::Globals::default();
        let p = partition_argv(&["who=world".to_string()], "greet", &mut g).unwrap();
        assert_eq!(p.argv, vec!["who=world".to_string()]);
    }
}

#[cfg(test)]
mod reserved_target_tests {
    use super::*;

    #[test]
    fn double_slash_target_is_rejected() {
        let err = reject_reserved_root_target("//check").unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("reserved"), "msg: {msg}");
        assert!(msg.contains("not yet supported"), "msg: {msg}");
        assert!(msg.contains("check"), "msg: {msg}");
    }

    #[test]
    fn normal_and_qualified_targets_pass() {
        assert!(reject_reserved_root_target("build").is_ok());
        assert!(reject_reserved_root_target("rust.build").is_ok());
        // single slash is not the reserved syntax
        assert!(reject_reserved_root_target("/x").is_ok());
    }
}
