//! Clap argument parsing for the `cook` binary.
//!
//! Built-in commands (`--menu`, `--init`, `--serve`, `--logs`) are flag-form so
//! the bare positional namespace stays free for recipe names. Anything that
//! isn't a built-in flag is treated as a recipe target.

use clap::Parser;
use std::path::PathBuf;

#[derive(Parser)]
#[command(
    name = "cook",
    about = "A modern build system with Lua",
    override_usage = "cook [OPTIONS] [RECIPE] [CONFIG]",
    after_help = "Run `cook <recipe>` to execute a recipe (defaults to 'build').\n\
                  Built-in commands: --menu, --init, --serve, --logs."
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
        conflicts_with_all = ["init", "serve", "logs"]
    )]
    pub menu: bool,

    /// Generate a starter Cookfile
    #[arg(
        long = "init",
        help_heading = "Built-in commands",
        conflicts_with_all = ["menu", "serve", "logs"]
    )]
    pub init: bool,

    /// Watch ingredients and re-run on change. Uses RECIPE/CONFIG positionals.
    #[arg(
        long = "serve",
        help_heading = "Built-in commands",
        conflicts_with_all = ["menu", "init", "logs"]
    )]
    pub serve: bool,

    /// Show logs for past builds. Uses RECIPE positional as selector.
    #[arg(
        long = "logs",
        help_heading = "Built-in commands",
        conflicts_with_all = ["menu", "init", "serve"]
    )]
    pub logs: bool,

    /// Run tests in the workspace (or scoped to a recipe/namespace).
    #[arg(
        long = "test",
        help_heading = "Built-in commands",
        conflicts_with_all = ["menu", "init", "serve", "logs", "dag", "emit_lua"]
    )]
    pub test: bool,

    // ===== --test sub-args =====
    /// Filter tests by glob against `<namespace>.<recipe>:<name>`. Repeatable.
    #[arg(long = "filter", num_args = 1, requires = "test")]
    pub filter: Vec<String>,

    /// Cancel queued tests on first failure.
    #[arg(long = "fail-fast", requires = "test")]
    pub fail_fast: bool,

    /// Force re-run of tests matching glob (or all if no pattern).
    #[arg(long = "rerun", num_args = 0..=1, default_missing_value = "*", requires = "test")]
    pub rerun: Option<Vec<String>>,

    /// Re-run only tests that failed (or were blocked / timed out) last run.
    #[arg(long = "rerun-failed", requires = "test")]
    pub rerun_failed: bool,

    /// Write JSON test report to the given path (default: .cook/test-report.json).
    #[arg(long = "report-json", num_args = 1, requires = "test")]
    pub report_json: Option<std::path::PathBuf>,

    /// Write JUnit XML test report to the given path.
    #[arg(long = "report-junit", num_args = 1, requires = "test")]
    pub report_junit: Option<std::path::PathBuf>,

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

    /// Visualize the build DAG in a TUI viewer
    #[arg(
        long = "dag",
        conflicts_with_all = ["menu", "init", "serve", "logs"]
    )]
    pub dag: bool,

    /// Suppress Cook output
    #[arg(short, long)]
    pub quiet: bool,

    /// Stream per-node output (stdout + stderr) inline with [recipe/node] prefix.
    #[arg(short = 'v', long)]
    pub verbose: bool,

    /// Number of parallel jobs (default: number of CPU cores)
    #[arg(short = 'j', long = "jobs")]
    pub jobs: Option<usize>,

    /// Color output mode
    #[arg(long, default_value = "auto")]
    pub color: String,

    /// DAG TUI theme: auto (default) or mono.
    #[arg(long = "theme", default_value = "auto")]
    pub theme: String,

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
