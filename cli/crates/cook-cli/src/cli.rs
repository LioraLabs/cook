//! Clap argument parsing for the `cook` binary.

use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser)]
#[command(
    name = "cook",
    about = "A modern build system with Lua",
    override_usage = "cook [OPTIONS] [RECIPE] [CONFIG]",
    after_help = "Run `cook <recipe>` to execute a recipe (defaults to 'build')"
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Command>,

    /// Path to Cookfile
    #[arg(short = 'f', long = "file", global = true, default_value = "Cookfile")]
    pub file: PathBuf,

    /// Print transpiled Lua instead of executing
    #[arg(long = "emit-lua", global = true)]
    pub emit_lua: bool,

    /// Visualize the build DAG in a browser
    #[arg(long = "dag", global = true)]
    pub dag: bool,

    /// Suppress Cook output
    #[arg(short, long, global = true)]
    pub quiet: bool,

    /// Number of parallel jobs (default: number of CPU cores)
    #[arg(short = 'j', long = "jobs", global = true)]
    pub jobs: Option<usize>,

    /// Color output mode
    #[arg(long, global = true, default_value = "auto")]
    pub color: String,

    /// Output mode: auto (default), plain, json
    #[arg(long = "output", global = true, default_value = "auto")]
    pub output: String,

    /// Force plain output even on a TTY (synonym for --output=plain)
    #[arg(long = "no-ui", global = true)]
    pub no_ui: bool,

    /// Override a variable (KEY=VALUE), repeatable. Must appear before recipe name.
    #[arg(long = "set", global = true, num_args = 1)]
    pub set: Vec<String>,
}

#[derive(Subcommand)]
pub enum Command {
    /// Watch ingredients and re-run on change
    Serve {
        /// Recipe to serve
        #[arg(default_value = "build")]
        recipe: String,
        /// Config preset to use
        config: Option<String>,
    },
    /// List all recipes
    Menu,
    /// Generate a starter Cookfile
    Init,
    /// Run all test recipes
    Test {
        /// Filter tests by name substring
        #[arg(long)]
        filter: Option<String>,

        /// Show all test output (don't capture)
        #[arg(long)]
        verbose: bool,

        /// Multiply all timeouts by this factor
        #[arg(long, default_value = "1")]
        timeout_multiplier: u64,

        /// Run every test through this wrapper command
        #[arg(long)]
        wrapper: Option<String>,

        /// List tests without running them
        #[arg(long)]
        list: bool,
    },
    /// Show logs for past builds
    Logs {
        /// Selector: <recipe>, <recipe>:<node>, or omit to list builds
        selector: Option<String>,
        /// Specific build id
        #[arg(long)]
        build: Option<String>,
        /// Dump failed nodes from the most recent build
        #[arg(long)]
        failed: bool,
    },
    #[command(external_subcommand)]
    External(Vec<String>),
}
