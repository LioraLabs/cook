//! cook — the user-facing binary for the Cook build system.

mod cli;
mod cmd_logs;
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
use pipeline::{cmd_dag, cmd_emit_lua, cmd_init, cmd_menu, cmd_run, cmd_serve, cmd_test};

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
        None => cmd_run(&globals, "build", None),
        Some(Cmd::Init) => cmd_init(),
        Some(Cmd::Menu) => cmd_menu(&globals),
        Some(Cmd::Modules(args)) => std::process::exit(modules::run(args)),
        Some(Cmd::Test(args)) => cmd_test(&globals, &args),
        Some(Cmd::Dag(args)) => cmd_dag(&globals, &args),
        Some(Cmd::Logs(args)) => crate::cmd_logs::cmd_logs(
            args.selector.as_deref(),
            args.build.as_deref(),
            args.failed,
        ),
        Some(Cmd::Serve(args)) => cmd_serve(
            &globals,
            args.recipe.as_deref().unwrap_or("build"),
            args.config.as_deref(),
        ),
        Some(Cmd::EmitLua) => cmd_emit_lua(&globals),
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
    let (first, rest) = parts
        .split_first()
        .expect("external_subcommand variant always carries ≥1 element");

    let recipe = first.strip_prefix('+').unwrap_or(first).to_string();
    let config = match rest {
        [] => None,
        [c] => Some(c.as_str()),
        _ => {
            return Err(CookError::Other(format!(
                "too many positional arguments after recipe `{recipe}`"
            )));
        }
    };

    cmd_run(globals, &recipe, config)
}
