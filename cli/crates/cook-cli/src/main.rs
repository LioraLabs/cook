//! cook — the user-facing binary for the Cook build system.

mod cli;
mod cmd_logs;
mod color;
mod dag_data;
mod dag_server;
mod env;
mod error;
mod pipeline;
mod progress;
mod test_output;
mod watcher;
mod workspace;

use clap::CommandFactory;

use cli::{Cli, Command};
use pipeline::{cmd_dag, cmd_init, cmd_menu, cmd_run, cmd_serve, cmd_test};

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

    let result = if cli.dag {
        match &cli.command {
            Some(Command::External(args)) => {
                let recipe = args.first().map(|s| s.as_str()).unwrap_or("build");
                let config = args.get(1).map(|s| s.as_str());
                cmd_dag(&cli, recipe, config)
            }
            None => cmd_dag(&cli, "build", None),
            _ => {
                eprintln!("cook: --dag can only be used with a recipe target");
                std::process::exit(1);
            }
        }
    } else {
        match &cli.command {
            Some(Command::Init) => cmd_init(),
            Some(Command::Menu) => cmd_menu(&cli),
            Some(Command::Serve { recipe, config }) => {
                let recipe = recipe.clone();
                cmd_serve(&cli, &recipe, config.as_deref())
            }
            Some(Command::Test {
                filter,
                verbose,
                timeout_multiplier,
                wrapper,
                list,
            }) => cmd_test(
                &cli,
                filter.clone(),
                *verbose,
                *timeout_multiplier,
                wrapper.clone(),
                *list,
            ),
            Some(Command::Logs { selector, build, failed }) => {
                crate::cmd_logs::cmd_logs(selector.as_deref(), build.as_deref(), *failed)
            }
            Some(Command::External(args)) => {
                let recipe = args.first().map(|s| s.as_str()).unwrap_or("build");
                let config = args.get(1).map(|s| s.as_str());
                cmd_run(&cli, recipe, config)
            }
            None => cmd_run(&cli, "build", None),
        }
    };

    if let Err(e) = result {
        eprintln!("cook: {e}");
        std::process::exit(e.exit_code());
    }
}
