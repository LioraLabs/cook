//! cook-xtask — Cargo xtask entry point for the Cook workspace.
//!
//! Invoked via `cargo xtask <subcommand>` (alias defined in cli/.cargo/config.toml).

mod package;

use clap::Parser;
use package::PackageArgs;
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(name = "xtask", about = "Cargo xtask helpers for the Cook workspace")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(clap::Subcommand, Debug)]
enum Cmd {
    /// Assemble a release tarball (VERSION + bin/cook).
    Package(PackageArgs),
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    match cli.cmd {
        Cmd::Package(args) => {
            // Resolve workspace root: the directory containing cli/Cargo.toml.
            // When invoked as `cargo xtask` from inside cli/, CARGO_MANIFEST_DIR
            // is cli/crates/cook-xtask — so we walk up two levels.
            let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
            let workspace_root = manifest_dir
                .parent() // crates/
                .and_then(|p| p.parent()) // cli/
                .expect("unexpected directory layout for cook-xtask");

            let (out_path, sha256) = package::run(&args, workspace_root)?;
            let filename = out_path
                .file_name()
                .and_then(|n| n.to_str())
                .expect("output path always has a UTF-8 filename");

            println!("{sha256}  {filename}");
        }
    }

    Ok(())
}
