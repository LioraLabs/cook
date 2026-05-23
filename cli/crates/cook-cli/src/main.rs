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
use cook_cli::modules;

use cli::{Cli, Cmd};
use error::CookError;
use pipeline::{cmd_affected, cmd_dag, cmd_emit_lua, cmd_init, cmd_list, cmd_menu, cmd_run, cmd_serve, cmd_test};

fn main() {
    let version_string: &'static str = Box::leak(Box::new(format!(
        "{} (Cook Standard v{})",
        env!("CARGO_PKG_VERSION"),
        cook_lang::COOK_STANDARD_VERSION,
    )));
    let cli_command = <Cli as CommandFactory>::command().version(version_string);
    let matches = cli_command.get_matches();
    let cli = <Cli as clap::FromArgMatches>::from_arg_matches(&matches)
        .expect("clap derive guarantees this conversion");

    let result = dispatch(cli);

    if let Err(e) = result {
        // TestFailure: the summary line already conveys the failure count;
        // printing the error message again would be noise (spec §3.4).
        if !matches!(e, CookError::TestFailure(_)) {
            eprintln!("cook: {e}");
        }
        std::process::exit(e.exit_code());
    }
}

fn dispatch(cli: Cli) -> Result<(), CookError> {
    let Cli { globals, cmd } = cli;
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
            let project_root = std::env::current_dir().map_err(|e| CookError::Other(e.to_string()))?;
            cook_logs::cmd_logs(&project_root, selector, cook_logs::Theme::default())
                .map_err(|e| CookError::Other(e.to_string()))
        }
        Some(Cmd::Serve(args)) => cmd_serve(
            &globals,
            args.recipe.as_deref().unwrap_or("build"),
            args.config.as_deref(),
        ),
        Some(Cmd::EmitLua) => cmd_emit_lua(&globals),
        Some(Cmd::Affected(args)) => cmd_affected(&globals, &args),
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
