//! cook-xtask — Cargo xtask entry point for the Cook workspace.
//!
//! Invoked via `cargo xtask <subcommand>` (alias defined in cli/.cargo/config.toml).

use clap::Parser;
use cook_xtask::package::PackageArgs;
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
                .ok_or_else(|| {
                    anyhow::anyhow!(
                        "cook-xtask must live at cli/crates/cook-xtask/; got: {}",
                        manifest_dir.display()
                    )
                })?;

            let (out_path, sha256) = cook_xtask::package::run(&args, workspace_root)?;
            let filename = out_path
                .file_name()
                .and_then(|n| n.to_str())
                .ok_or_else(|| {
                    anyhow::anyhow!("output path has no UTF-8 filename: {}", out_path.display())
                })?;

            println!("{sha256}  {filename}");
        }
    }

    Ok(())
}
