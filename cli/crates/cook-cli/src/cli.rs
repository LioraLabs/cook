//! Clap argument parsing for the `cook` binary.
//!
//! Top-level shape:
//!     cook [GLOBALS]
//!     cook [GLOBALS] [+]<RECIPE> [CONFIG]
//!     cook [GLOBALS] <SUBCOMMAND> [SUBCOMMAND-ARGS...]
//!
//! Bare positional that doesn't match a reserved subcommand name dispatches
//! to a recipe via `Cmd::Recipe`. A leading `+` on the first positional
//! forces recipe lookup (escape hatch for recipes whose names collide with
//! reserved subcommands).

use clap::{Parser, Subcommand};
use std::path::PathBuf;

use crate::modules::cli::ModulesArgs;

#[derive(Parser, Debug)]
#[command(
    name = "cook",
    about = "A declarative build system for polyglot projects",
    after_help = "Run `cook <recipe>` to execute a recipe (defaults to 'build').\n\
                  Use `cook +<recipe>` to invoke a recipe whose name collides with a built-in subcommand."
)]
pub struct Cli {
    #[command(flatten)]
    pub globals: Globals,

    #[command(subcommand)]
    pub cmd: Option<Cmd>,
}

#[derive(clap::Args, Debug, Default, Clone)]
pub struct Globals {
    /// Path to Cookfile
    #[arg(short = 'f', long = "file", default_value = "Cookfile", global = true)]
    pub file: PathBuf,

    /// Override workspace root resolution. When supplied, the workspace root is
    /// taken to be this directory; the invoked Cookfile MUST be at or below it.
    #[arg(long = "root", global = true)]
    pub root: Option<PathBuf>,

    /// Suppress Cook output
    #[arg(short, long, global = true)]
    pub quiet: bool,

    /// Stream per-node output (stdout + stderr) inline with [recipe/node] prefix.
    #[arg(short = 'v', long, global = true)]
    pub verbose: bool,

    /// Replay captured stdout/stderr for cache hits, marked as replayed.
    #[arg(long = "replay-logs", global = true)]
    pub replay_logs: bool,

    /// Number of parallel jobs (default: number of CPU cores)
    #[arg(short = 'j', long = "jobs", global = true)]
    pub jobs: Option<usize>,

    /// Color output mode
    #[arg(long, default_value = "auto", global = true)]
    pub color: String,

    /// Output mode: auto (default), plain, json
    #[arg(long = "output", default_value = "auto", global = true)]
    pub output: String,

    /// Override a variable (KEY=VALUE), repeatable.
    #[arg(long = "set", num_args = 1, global = true)]
    pub set: Vec<String>,

    /// Restrict the recipe runner to recipes whose declared file inputs (or
    /// transitive downstream consumers) intersect the git diff against
    /// `--since=<ref>`. See `cook affected --help` for semantics. Requires
    /// `--since=<ref>`.
    #[arg(long = "affected", global = true)]
    pub affected: bool,

    /// Git ref to diff against for `--affected` / `cook affected`. Uses
    /// three-dot merge-base semantics and includes working-tree changes
    /// (staged, unstaged, and untracked-non-ignored).
    #[arg(long = "since", global = true)]
    pub since: Option<String>,

    /// Keep stale outputs instead of sweeping files no longer declared by a recipe.
    /// instead of sweeping them. Also settable via `COOK_NO_PRUNE=1`.
    #[arg(long = "no-prune", global = true)]
    pub no_prune: bool,

    /// Skip the automatic `[cache] auto_gc` sweep for this run. The budget
    /// warning still prints; only the deletion is suppressed. Also settable via
    /// `COOK_NO_AUTO_GC=1`.
    #[arg(long = "no-auto-gc", global = true)]
    pub no_auto_gc: bool,

    /// Read-only / publish-off client mode: fetch cached artifacts by key but
    /// never publish locally-produced artifacts to the shared store. Also
    /// settable via `COOK_NO_PUBLISH=1` or `[cloud] publish = false` in
    /// `.cook/cloud.toml`. Write-authorization itself is backend IAM.
    #[arg(long = "no-publish", global = true)]
    pub no_publish: bool,
}

#[derive(clap::Args, Debug, Clone)]
pub struct AffectedArgs {
    /// Restrict the listed recipes to those whose name equals this argument
    /// (after the qualified prefix). In a pnpm workspace `--recipe=build`
    /// filters to all `*:build` recipes.
    #[arg(long = "recipe")]
    pub recipe: Option<String>,

    /// Emit machine-readable JSON instead of one-name-per-line plain text.
    #[arg(long = "json")]
    pub json: bool,
}

#[derive(Subcommand, Debug, Clone)]
pub enum Cmd {
    /// Generate a starter Cookfile in the current directory.
    Init,

    /// List all recipes (and chores) in the workspace.
    Menu,

    /// Alias for `menu`: list all recipes (and chores) in the workspace.
    ///
    /// Kept as its own variant rather than a clap alias so that `built_in_name`
    /// can report the spelling the user actually typed — a recipe named `list`
    /// must be named as `list` in the shadowing notice, not as `menu`.
    List,

    /// Manage cook modules — install, remove, update, list, search rocks.
    Modules(ModulesArgs),

    /// Run tests in the workspace (or scoped to a recipe/namespace).
    Test(TestArgs),

    /// Show logs for past builds.
    Logs(LogsArgs),

    /// Cache fidelity tooling. `cook cache verify` re-runs cached steps and
    /// reports byte-divergence under a matching key.
    Cache(CacheArgs),

    /// Watch ingredients and re-run on change.
    Serve(ServeArgs),

    /// Print transpiled Lua for the current Cookfile (file-level, not recipe-scoped).
    #[command(name = "emit-lua")]
    EmitLua,

    /// List the recipes whose declared file inputs (or transitive downstream
    /// consumers) would be invalidated by the diff since `--since=<ref>`.
    /// Uses three-dot merge-base semantics and includes working-tree state.
    /// Requires `--since=<ref>`.
    Affected(AffectedArgs),

    /// Explain what a run would do, and why: the build graph with per-node
    /// hit/rebuild counts and what each rebuild forces downstream, plus (at
    /// `--level unit`) every determinant behind each unit's cache key and, on
    /// a shared miss, the diff against what the cached artifact was built
    /// from. Read-only; runs nothing.
    Why(WhyArgs),

    /// Run a recipe by name. Captured for any first positional that does not
    /// match a reserved subcommand. The first element is the recipe name
    /// (with a leading `+` stripped if present); the optional second element
    /// is the config preset.
    #[command(external_subcommand)]
    Recipe(Vec<String>),
}

impl Cmd {
    pub fn built_in_name(&self) -> Option<&'static str> {
        match self {
            Cmd::Init => Some("init"),
            Cmd::Menu => Some("menu"),
            Cmd::List => Some("list"),
            Cmd::Modules(_) => Some("modules"),
            Cmd::Test(_) => Some("test"),
            Cmd::Logs(_) => Some("logs"),
            Cmd::Cache(_) => Some("cache"),
            Cmd::Serve(_) => Some("serve"),
            Cmd::EmitLua => Some("emit-lua"),
            Cmd::Affected(_) => Some("affected"),
            Cmd::Why(_) => Some("why"),
            Cmd::Recipe(_) => None,
        }
    }

    /// The user-supplied CLI target (recipe name / test scope) for
    /// subcommands that accept one, or `None` where no target was given or
    /// the subcommand has no target-typed field.
    ///
    /// This is the single chokepoint the dispatcher validates for the
    /// reserved `//<name>` root-anchored syntax (§20.2.4 / CS-0120) — a new
    /// target-typed field must be wired in here to inherit the rejection.
    /// The `external_subcommand` `Recipe` arm is the one exception: its
    /// recipe name needs the `+` escape stripped first, so `dispatch_recipe`
    /// keeps its own validation call.
    ///
    /// Note this stays a post-parse check (not a clap value_parser): the
    /// rejection must exit 1 with the exact `cook: '<target>': ...`
    /// diagnostic, while clap's error path exits 2 and wraps messages in its
    /// own `error: invalid value ...` formatting.
    pub fn reserved_target(&self) -> Option<&str> {
        match self {
            Cmd::Test(a) => a.scope.as_deref(),
            Cmd::Cache(c) => match &c.cmd {
                CacheCmd::Verify(v) => v.recipe.as_deref(),
                CacheCmd::Dump(d) => d.recipe.as_deref(),
                CacheCmd::Du => None,
                CacheCmd::Gc(_) => None,
            },
            Cmd::Serve(a) => a.recipe.as_deref(),
            Cmd::Affected(a) => a.recipe.as_deref(),
            Cmd::Why(a) => a.recipe.as_deref(),
            Cmd::Init
            | Cmd::Menu
            | Cmd::List
            | Cmd::Modules(_)
            | Cmd::Logs(_)
            | Cmd::EmitLua
            | Cmd::Recipe(_) => None,
        }
    }
}

#[derive(clap::Args, Debug, Clone)]
pub struct TestArgs {
    /// Optional recipe scope (e.g. `apps.web` or `apps.web.unit`).
    pub scope: Option<String>,

    /// Filter tests by glob against `<namespace>.<recipe>:<name>`. Repeatable.
    #[arg(long = "filter", num_args = 1)]
    pub filter: Vec<String>,

    /// Cancel queued tests on first failure.
    #[arg(long = "fail-fast")]
    pub fail_fast: bool,

    /// Force re-run of tests matching glob (or all if no pattern).
    #[arg(long = "rerun", num_args = 0..=1, default_missing_value = "*")]
    pub rerun: Option<Vec<String>>,

    /// Re-run only tests that failed (or were blocked / timed out) last run.
    #[arg(long = "rerun-failed")]
    pub rerun_failed: bool,

    /// Write JSON test report to the given path (default: .cook/test-report.json).
    #[arg(long = "report-json", num_args = 1)]
    pub report_json: Option<PathBuf>,

    /// Write JUnit XML test report to the given path.
    #[arg(long = "report-junit", num_args = 1)]
    pub report_junit: Option<PathBuf>,
}

#[derive(clap::Args, Debug, Clone)]
pub struct WhyArgs {
    /// Recipe to explain (default: 'build').
    pub recipe: Option<String>,

    /// Config preset.
    pub config: Option<String>,

    /// How much to collapse: recipe (default), group, or unit. A real C/C++
    /// target registers thousands of units, so the default is coarse.
    #[arg(long = "level", default_value = "recipe")]
    pub level: String,

    /// Output syntax: text (default), mermaid, dot, or json.
    ///
    /// CS-0171: this subsumes the former `--json` flag. One spelling for one
    /// thing — `--json` and `--format json` meaning the same thing was surface
    /// worth removing while the command had no users.
    #[arg(long = "format", default_value = "text")]
    pub format: String,

    /// Node ceiling for `--level unit`; above it the command refuses rather
    /// than emitting a graph nobody can read.
    #[arg(long = "max-nodes", default_value_t = 200)]
    pub max_nodes: usize,

    /// Report full determinants for units whose recipe name, cache key, or
    /// output path contains this substring, instead of rendering the graph.
    #[arg(long = "unit", num_args = 1)]
    pub unit: Option<String>,
}

#[derive(clap::Args, Debug, Clone)]
pub struct LogsArgs {
    /// Specific build id (directory name under .cook/logs/).
    pub build_id: Option<String>,

    /// Open the Nth most recent build (1 = latest).
    #[arg(short = 'n', long, conflicts_with_all = ["build_id", "last_failed"])]
    pub nth: Option<usize>,

    /// Open the most recent build with a non-zero exit code.
    #[arg(long, conflicts_with_all = ["build_id", "nth"])]
    pub last_failed: bool,

    /// Color theme: auto (default) or mono.
    #[arg(long, default_value = "auto")]
    pub theme: String,
}

#[derive(clap::Args, Debug, Clone)]
pub struct CacheArgs {
    #[command(subcommand)]
    pub cmd: CacheCmd,
}

#[derive(Subcommand, Debug, Clone)]
pub enum CacheCmd {
    /// Re-run cached steps and report byte-divergence under a matching key.
    /// Opt-in CI fidelity tool: exits non-zero on any divergence. `record`
    /// record steps are byte-exempt. NOT a trust gate.
    Verify(CacheVerifyArgs),
    /// Print a recipe's cache index as readable TOML (CS-0166).
    ///
    /// The index itself is binary since v7; this renders it on demand, with
    /// each input/output record inlined on one line. Read-only. For the
    /// question "why did this rebuild", prefer `cook why`.
    Dump(CacheDumpArgs),
    /// Report local artifact-store disk usage (COOK-232).
    ///
    /// Walks the on-disk CAS and prints a byte/object total, a breakdown by
    /// artifact kind and by recipe namespace, the oldest/newest artifact,
    /// and (when `[cache] max_size` is configured) budget usage. When
    /// `[cloud] enabled = true` the cloud cache is server-managed and this
    /// prints a note instead of walking anything local.
    Du,
    /// Evict least-recently-used or stale artifacts from the local store.
    ///
    /// Manual sweep: requires `--max-size` and/or `--older-than`. Neither
    /// given is a usage error. When `[cloud] enabled = true` the cloud
    /// cache is server-managed and this prints a note instead of touching
    /// anything local.
    Gc(CacheGcArgs),
}

#[derive(clap::Args, Debug, Clone)]
pub struct CacheGcArgs {
    /// Evict least-recently-used artifacts until the store fits (e.g. 10GB, 500MB).
    #[arg(long = "max-size")]
    pub max_size: Option<String>,
    /// Evict artifacts untouched for longer than this (e.g. 30d, 720h).
    #[arg(long = "older-than")]
    pub older_than: Option<String>,
    /// Report what would be freed without deleting anything.
    #[arg(long = "dry-run")]
    pub dry_run: bool,
}

#[derive(clap::Args, Debug, Clone)]
pub struct CacheDumpArgs {
    /// Recipe whose index to dump (default: 'build').
    pub recipe: Option<String>,
}

#[derive(clap::Args, Debug, Clone)]
pub struct CacheVerifyArgs {
    /// Recipe to verify (default: 'build').
    pub recipe: Option<String>,
    /// Config preset.
    pub config: Option<String>,
    /// Emit machine-readable JSON instead of a human summary.
    #[arg(long = "json")]
    pub json: bool,
}

#[derive(clap::Args, Debug, Clone)]
pub struct ServeArgs {
    /// Recipe to watch (default: 'build').
    pub recipe: Option<String>,

    /// Config preset.
    pub config: Option<String>,
}

#[cfg(test)]
#[path = "tests/cli_tests.rs"]
mod tests;
