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
    about = "A modern build system with Lua",
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

    /// Disable stale-output reconciliation (§17.7): keep orphaned outputs
    /// instead of sweeping them. Also settable via `COOK_NO_PRUNE=1`.
    #[arg(long = "no-prune", global = true)]
    pub no_prune: bool,
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

    /// Print recipe and chore names, one per line, for shell pipelines.
    ///
    /// Machine-readable counterpart of `cook menu`: no decoration, no kind
    /// prefix, no column padding — each line is exactly the name you'd type
    /// on the CLI. Designed for `cook list | fzf | xargs -r cook` and
    /// similar shell pipelines.
    List(ListArgs),

    /// Manage cook modules — install, remove, update, list, search rocks.
    Modules(ModulesArgs),

    /// Run tests in the workspace (or scoped to a recipe/namespace).
    Test(TestArgs),

    /// Visualize the build DAG in a TUI viewer.
    Dag(DagArgs),

    /// Show logs for past builds.
    Logs(LogsArgs),

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

    /// Run a recipe by name. Captured for any first positional that does not
    /// match a reserved subcommand. The first element is the recipe name
    /// (with a leading `+` stripped if present); the optional second element
    /// is the config preset.
    #[command(external_subcommand)]
    Recipe(Vec<String>),
}

#[derive(clap::Args, Debug, Clone)]
pub struct ListArgs {
    /// Restrict output to recipes only (mutually exclusive with --chores-only).
    #[arg(long = "recipes-only", conflicts_with = "chores_only")]
    pub recipes_only: bool,

    /// Restrict output to chores only (mutually exclusive with --recipes-only).
    #[arg(long = "chores-only", conflicts_with = "recipes_only")]
    pub chores_only: bool,
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
pub struct DagArgs {
    /// Recipe to visualize (default: 'build').
    pub recipe: Option<String>,

    /// Config preset.
    pub config: Option<String>,

    /// DAG TUI theme: auto (default) or mono.
    #[arg(long = "theme", default_value = "auto")]
    pub theme: String,
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
pub struct ServeArgs {
    /// Recipe to watch (default: 'build').
    pub recipe: Option<String>,

    /// Config preset.
    pub config: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(argv: &[&str]) -> Cli {
        let mut full = vec!["cook"];
        full.extend_from_slice(argv);
        Cli::try_parse_from(full).expect("parse should succeed")
    }

    #[test]
    fn no_args_yields_no_subcommand() {
        let cli = parse(&[]);
        assert!(cli.cmd.is_none());
    }

    #[test]
    fn bare_recipe_captured_as_external() {
        let cli = parse(&["deploy"]);
        match cli.cmd {
            Some(Cmd::Recipe(parts)) => assert_eq!(parts, vec!["deploy".to_string()]),
            other => panic!("expected Cmd::Recipe, got {other:?}"),
        }
    }

    #[test]
    fn recipe_with_config_captured_as_external() {
        let cli = parse(&["deploy", "prod"]);
        match cli.cmd {
            Some(Cmd::Recipe(parts)) => {
                assert_eq!(parts, vec!["deploy".to_string(), "prod".to_string()])
            }
            other => panic!("expected Cmd::Recipe, got {other:?}"),
        }
    }

    #[test]
    fn plus_escape_captured_as_external() {
        let cli = parse(&["+test"]);
        match cli.cmd {
            Some(Cmd::Recipe(parts)) => assert_eq!(parts, vec!["+test".to_string()]),
            other => panic!("expected Cmd::Recipe, got {other:?}"),
        }
    }

    #[test]
    fn init_subcommand() {
        assert!(matches!(parse(&["init"]).cmd, Some(Cmd::Init)));
    }

    #[test]
    fn menu_subcommand() {
        assert!(matches!(parse(&["menu"]).cmd, Some(Cmd::Menu)));
    }

    #[test]
    fn list_subcommand_no_flags() {
        match parse(&["list"]).cmd {
            Some(Cmd::List(args)) => {
                assert!(!args.recipes_only);
                assert!(!args.chores_only);
            }
            other => panic!("expected Cmd::List, got {other:?}"),
        }
    }

    #[test]
    fn list_subcommand_recipes_only() {
        match parse(&["list", "--recipes-only"]).cmd {
            Some(Cmd::List(args)) => {
                assert!(args.recipes_only);
                assert!(!args.chores_only);
            }
            other => panic!("expected Cmd::List, got {other:?}"),
        }
    }

    #[test]
    fn list_subcommand_chores_only() {
        match parse(&["list", "--chores-only"]).cmd {
            Some(Cmd::List(args)) => {
                assert!(!args.recipes_only);
                assert!(args.chores_only);
            }
            other => panic!("expected Cmd::List, got {other:?}"),
        }
    }

    #[test]
    fn list_subcommand_rejects_both_flags() {
        let result = Cli::try_parse_from(["cook", "list", "--recipes-only", "--chores-only"]);
        assert!(
            result.is_err(),
            "--recipes-only and --chores-only must be mutually exclusive"
        );
    }

    #[test]
    fn emit_lua_subcommand() {
        assert!(matches!(parse(&["emit-lua"]).cmd, Some(Cmd::EmitLua)));
    }

    #[test]
    fn test_subcommand_with_filter() {
        let cli = parse(&["test", "--filter", "alpha:*"]);
        match cli.cmd {
            Some(Cmd::Test(args)) => {
                assert!(args.scope.is_none());
                assert_eq!(args.filter, vec!["alpha:*".to_string()]);
            }
            other => panic!("expected Cmd::Test, got {other:?}"),
        }
    }

    #[test]
    fn test_subcommand_with_scope() {
        let cli = parse(&["test", "sub.pass"]);
        match cli.cmd {
            Some(Cmd::Test(args)) => assert_eq!(args.scope.as_deref(), Some("sub.pass")),
            other => panic!("expected Cmd::Test, got {other:?}"),
        }
    }

    #[test]
    fn dag_subcommand_with_theme() {
        let cli = parse(&["dag", "host", "--theme", "mono"]);
        match cli.cmd {
            Some(Cmd::Dag(args)) => {
                assert_eq!(args.recipe.as_deref(), Some("host"));
                assert_eq!(args.theme, "mono");
            }
            other => panic!("expected Cmd::Dag, got {other:?}"),
        }
    }

    #[test]
    fn serve_subcommand_with_recipe() {
        let cli = parse(&["serve", "host", "prod"]);
        match cli.cmd {
            Some(Cmd::Serve(args)) => {
                assert_eq!(args.recipe.as_deref(), Some("host"));
                assert_eq!(args.config.as_deref(), Some("prod"));
            }
            other => panic!("expected Cmd::Serve, got {other:?}"),
        }
    }

    #[test]
    fn globals_apply_with_subcommand() {
        let cli = parse(&["-v", "test"]);
        assert!(cli.globals.verbose);
        assert!(matches!(cli.cmd, Some(Cmd::Test(_))));
    }

    #[test]
    fn globals_apply_after_subcommand() {
        // Symmetric to globals_apply_with_subcommand: when a subcommand
        // is present, a `global = true` flag attached to it must still
        // populate Globals via the flatten propagation.
        let cli = parse(&["test", "-v"]);
        assert!(cli.globals.verbose);
        assert!(matches!(cli.cmd, Some(Cmd::Test(_))));
    }

    #[test]
    fn globals_apply_without_subcommand() {
        let cli = parse(&["-v", "deploy"]);
        assert!(cli.globals.verbose);
        assert!(matches!(cli.cmd, Some(Cmd::Recipe(_))));
    }

    #[test]
    fn old_flag_form_rejected() {
        // Sanity: --test should no longer parse as a built-in invocation.
        let result = Cli::try_parse_from(["cook", "--test"]);
        assert!(result.is_err(), "--test should be rejected after the redesign");
    }

    #[test]
    fn logs_no_args_means_latest() {
        let cli = parse(&["logs"]);
        let Some(Cmd::Logs(a)) = &cli.cmd else { panic!("expected Logs command") };
        assert!(a.build_id.is_none());
        assert!(!a.last_failed);
        assert!(a.nth.is_none());
    }

    #[test]
    fn logs_build_id_positional() {
        let cli = parse(&["logs", "2026-05-10-abc"]);
        let Some(Cmd::Logs(a)) = &cli.cmd else { panic!() };
        assert_eq!(a.build_id.as_deref(), Some("2026-05-10-abc"));
    }

    #[test]
    fn logs_nth_flag() {
        let cli = parse(&["logs", "-n", "3"]);
        let Some(Cmd::Logs(a)) = &cli.cmd else { panic!() };
        assert_eq!(a.nth, Some(3));
    }

    #[test]
    fn logs_last_failed_flag() {
        let cli = parse(&["logs", "--last-failed"]);
        let Some(Cmd::Logs(a)) = &cli.cmd else { panic!() };
        assert!(a.last_failed);
    }

    #[test]
    fn logs_conflicting_selectors_fail_to_parse() {
        let res = Cli::try_parse_from(["cook", "logs", "--last-failed", "-n", "2"]);
        assert!(res.is_err());
    }

    #[test]
    fn parses_affected_subcommand_with_since() {
        let cli = parse(&["affected", "--since=main"]);
        match cli.cmd {
            Some(Cmd::Affected(args)) => {
                assert!(args.recipe.is_none());
                assert!(!args.json);
            }
            other => panic!("expected Cmd::Affected, got {other:?}"),
        }
        assert_eq!(cli.globals.since.as_deref(), Some("main"));
        assert!(!cli.globals.affected);
    }

    #[test]
    fn parses_affected_subcommand_with_recipe_and_json() {
        let cli = parse(&["affected", "--since=origin/main", "--recipe=build", "--json"]);
        match cli.cmd {
            Some(Cmd::Affected(args)) => {
                assert_eq!(args.recipe.as_deref(), Some("build"));
                assert!(args.json);
            }
            other => panic!("expected Cmd::Affected, got {other:?}"),
        }
    }

    #[test]
    fn parses_recipe_with_affected_flag_globals_first() {
        // Globals-first form: clap intercepts --affected/--since because they
        // appear before the external_subcommand's first positional.
        let cli = parse(&["--affected", "--since=main", "build"]);
        assert!(matches!(cli.cmd, Some(Cmd::Recipe(_))));
        assert!(cli.globals.affected);
        assert_eq!(cli.globals.since.as_deref(), Some("main"));
    }

    #[test]
    fn parses_recipe_with_affected_flag_post_recipe_raw() {
        // Post-recipe Turborepo-style form (`cook build --affected --since=main`):
        // clap captures the flags raw into the Recipe vec because they appear
        // after the external_subcommand catch-all. partition_argv in main.rs
        // re-extracts them before dispatch (see PartitionedArgv). At the clap
        // layer therefore globals stay defaulted; this test pins that contract.
        let cli = parse(&["build", "--affected", "--since=main"]);
        match &cli.cmd {
            Some(Cmd::Recipe(parts)) => {
                assert_eq!(
                    parts,
                    &vec![
                        "build".to_string(),
                        "--affected".to_string(),
                        "--since=main".to_string()
                    ]
                );
            }
            other => panic!("expected Cmd::Recipe, got {other:?}"),
        }
        assert!(!cli.globals.affected);
        assert!(cli.globals.since.is_none());
    }
}
