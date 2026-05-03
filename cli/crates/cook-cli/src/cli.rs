//! Clap argument parsing for the `cook` binary.
//!
//! Built-in commands (`--menu`, `--init`, `--serve`, `--test`, `--logs`) are
//! flag-form so the bare positional namespace stays free for recipe names.
//! Anything that isn't a built-in flag is treated as a recipe target.

use clap::Parser;
use std::path::PathBuf;

#[derive(Parser)]
#[command(
    name = "cook",
    about = "A modern build system with Lua",
    override_usage = "cook [OPTIONS] [RECIPE] [CONFIG]",
    after_help = "Run `cook <recipe>` to execute a recipe (defaults to 'build').\n\
                  Built-in commands: --menu, --init, --serve, --test, --logs."
)]
pub struct Cli {
    /// Recipe to run (default: 'build'). Also used as the target for --serve
    /// and as the selector for --logs.
    pub recipe: Option<String>,

    /// Config preset to use
    pub config: Option<String>,

    // ===== built-in commands =====
    /// List all recipes
    #[arg(
        long = "menu",
        help_heading = "Built-in commands",
        conflicts_with_all = ["init", "serve", "test", "logs"]
    )]
    pub menu: bool,

    /// Generate a starter Cookfile
    #[arg(
        long = "init",
        help_heading = "Built-in commands",
        conflicts_with_all = ["menu", "serve", "test", "logs"]
    )]
    pub init: bool,

    /// Watch ingredients and re-run on change. Uses RECIPE/CONFIG positionals.
    #[arg(
        long = "serve",
        help_heading = "Built-in commands",
        conflicts_with_all = ["menu", "init", "test", "logs"]
    )]
    pub serve: bool,

    /// Run all test recipes
    #[arg(
        long = "test",
        help_heading = "Built-in commands",
        conflicts_with_all = ["menu", "init", "serve", "logs"]
    )]
    pub test: bool,

    /// Show logs for past builds. Uses RECIPE positional as selector.
    #[arg(
        long = "logs",
        help_heading = "Built-in commands",
        conflicts_with_all = ["menu", "init", "serve", "test"]
    )]
    pub logs: bool,

    // ===== --test sub-args =====
    /// Filter tests by name substring
    #[arg(long, help_heading = "Test options (with --test)", requires = "test")]
    pub filter: Option<String>,

    /// Show all test output (don't capture)
    #[arg(long, help_heading = "Test options (with --test)", requires = "test")]
    pub verbose: bool,

    /// Multiply all timeouts by this factor
    #[arg(
        long = "timeout-multiplier",
        help_heading = "Test options (with --test)",
        default_value = "1",
        requires = "test"
    )]
    pub timeout_multiplier: u64,

    /// Run every test through this wrapper command
    #[arg(long, help_heading = "Test options (with --test)", requires = "test")]
    pub wrapper: Option<String>,

    /// List tests without running them
    #[arg(long = "list", help_heading = "Test options (with --test)", requires = "test")]
    pub list: bool,

    // ===== --logs sub-args =====
    /// Specific build id
    #[arg(long, help_heading = "Logs options (with --logs)", requires = "logs")]
    pub build: Option<String>,

    /// Dump failed nodes from the most recent build
    #[arg(long, help_heading = "Logs options (with --logs)", requires = "logs")]
    pub failed: bool,

    // ===== global options =====
    /// Path to Cookfile
    #[arg(short = 'f', long = "file", default_value = "Cookfile")]
    pub file: PathBuf,

    /// Override workspace root resolution. When supplied, the workspace root is
    /// taken to be this directory; the invoked Cookfile MUST be at or below it.
    /// When omitted, the workspace root is determined per §7.6 (marker file →
    /// tree-import inference → self-root or reject).
    #[arg(long = "root")]
    pub root: Option<PathBuf>,

    /// Print transpiled Lua instead of executing
    #[arg(long = "emit-lua")]
    pub emit_lua: bool,

    /// Visualize the build DAG in a browser
    #[arg(
        long = "dag",
        conflicts_with_all = ["menu", "init", "serve", "test", "logs"]
    )]
    pub dag: bool,

    /// Suppress Cook output
    #[arg(short, long)]
    pub quiet: bool,

    /// Number of parallel jobs (default: number of CPU cores)
    #[arg(short = 'j', long = "jobs")]
    pub jobs: Option<usize>,

    /// Color output mode
    #[arg(long, default_value = "auto")]
    pub color: String,

    /// Output mode: auto (default), plain, json
    #[arg(long = "output", default_value = "auto")]
    pub output: String,

    /// Force plain output even on a TTY (synonym for --output=plain)
    #[arg(long = "no-ui")]
    pub no_ui: bool,

    /// Override a variable (KEY=VALUE), repeatable.
    #[arg(long = "set", num_args = 1)]
    pub set: Vec<String>,
}
