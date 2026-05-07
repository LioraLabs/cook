//! cook — the user-facing binary for the Cook build system.

mod cli;
mod cmd_logs;
mod error;
mod pipeline;
mod progress;
mod watcher;

use clap::CommandFactory;
use cook_cli::pull;

use cli::Cli;
use pipeline::{cmd_dag, cmd_init, cmd_menu, cmd_run, cmd_serve};

fn main() {
    let raw_argv: Vec<String> = std::env::args().collect();
    if raw_argv.get(1).map(String::as_str) == Some("pull") {
        let pull_argv: Vec<String> = raw_argv.iter().skip(1).cloned().collect();
        std::process::exit(pull::run_from_argv(&pull_argv));
    }

    let version_string: &'static str = Box::leak(Box::new(format!(
        "{} (Cook Standard v{})",
        env!("CARGO_PKG_VERSION"),
        cook_lang::COOK_STANDARD_VERSION,
    )));
    let cli_command = <Cli as CommandFactory>::command().version(version_string);
    let matches = cli_command.get_matches();
    let cli = <Cli as clap::FromArgMatches>::from_arg_matches(&matches)
        .expect("clap derive guarantees this conversion");

    let recipe = cli.recipe.clone().unwrap_or_else(|| "build".to_string());
    let config = cli.config.clone();

    let result = if cli.dag {
        cmd_dag(&cli, &recipe, config.as_deref())
    } else if cli.init {
        cmd_init()
    } else if cli.menu {
        cmd_menu(&cli)
    } else if cli.serve {
        cmd_serve(&cli, &recipe, config.as_deref())
    } else if cli.logs {
        crate::cmd_logs::cmd_logs(cli.recipe.as_deref(), cli.build.as_deref(), cli.failed)
    } else {
        cmd_run(&cli, &recipe, config.as_deref())
    };

    if let Err(e) = result {
        eprintln!("cook: {e}");
        std::process::exit(e.exit_code());
    }
}
