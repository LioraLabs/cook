//! cook — the user-facing binary for the Cook build system.

mod cli;
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
    cmd_serve, cmd_test, cmd_why, resolve_project_root,
};

fn main() {
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
    let partitioned = partition_argv(rest, &recipe)?;

    // Merge post-recipe `--affected`/`--since` flags into globals so that
    // `cook build --affected --since=main` (Turborepo-style, flag after the
    // recipe name) is honoured the same as `cook --affected --since=main build`.
    // clap's external_subcommand captures post-recipe flags into the Vec
    // verbatim; partition_argv pulls --affected/--since back out.
    let mut merged = globals.clone();
    if partitioned.affected {
        merged.affected = true;
    }
    if merged.since.is_none() {
        merged.since = partitioned.since;
    }

    cmd_run(&merged, &recipe, &partitioned.argv, partitioned.preset.as_deref())
}

/// Result of partitioning a recipe's positional argv into the runtime-meaningful
/// pieces. `argv` is the user-facing remainder (chore params etc.); the other
/// fields are flags clap couldn't intercept because they appeared after the
/// `external_subcommand` catch-all.
struct PartitionedArgv {
    argv: Vec<String>,
    preset: Option<String>,
    affected: bool,
    since: Option<String>,
}

/// COOK-36 Task 9 + COOK-58: partition the positionals after the recipe name
/// into argv, preset, and the `--affected`/`--since=<ref>` pair.
///
/// Preset can come from `@TOKEN` sigil or `--config NAME` / `-c NAME` /
/// `--config=NAME` flag forms. `--affected` is a bool. `--since=<ref>` and
/// `--since <ref>` both accepted. The `--` end-of-options separator switches
/// off sigil/flag interpretation for the rest of the line. At most one preset
/// is permitted across all forms.
fn partition_argv(rest: &[String], recipe: &str) -> Result<PartitionedArgv, CookError> {
    let mut argv: Vec<String> = Vec::new();
    let mut preset: Option<String> = None;
    let mut affected = false;
    let mut since: Option<String> = None;
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
        // --affected (bool, no value)
        if tok == "--affected" {
            affected = true;
            continue;
        }
        // --since <ref> (two-token form)
        if tok == "--since" {
            let next = iter.next().ok_or_else(|| {
                CookError::Other("'--since' requires a git ref".to_string())
            })?;
            since = Some(next.clone());
            continue;
        }
        // --since=<ref> (single-token form)
        if let Some(value) = tok.strip_prefix("--since=") {
            since = Some(value.to_string());
            continue;
        }
        argv.push(tok.clone());
    }
    Ok(PartitionedArgv { argv, preset, affected, since })
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
