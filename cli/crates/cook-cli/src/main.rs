//! cook — the user-facing binary for the Cook build system.

mod cli;
mod color;
mod dag_data;
mod env;
mod error;
mod pipeline;
mod progress;
mod test_output;
mod watcher;
mod workspace;

use clap::Parser;

use cli::{Cli, Command};
use pipeline::{cmd_dag, cmd_init, cmd_menu, cmd_run, cmd_serve, cmd_test};

fn main() {
    let cli = Cli::parse();

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
