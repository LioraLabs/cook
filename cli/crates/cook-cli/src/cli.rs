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

    /// Visualize the build DAG in a TUI viewer.
    Dag(DagArgs),

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

    /// Explain the cache key per unit: every input that shapes the key,
    /// attributed to its source, with hit/miss status — and on a shared miss,
    /// the diff against what the cached artifact was built from. Read-only; runs nothing.
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
            Cmd::Dag(_) => Some("dag"),
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
            Cmd::Dag(a) => a.recipe.as_deref(),
            Cmd::Cache(c) => match &c.cmd {
                CacheCmd::Verify(v) => v.recipe.as_deref(),
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
pub struct WhyArgs {
    /// Recipe to explain (default: 'build').
    pub recipe: Option<String>,
    /// Config preset.
    pub config: Option<String>,
    /// Emit machine-readable JSON instead of the human-readable report.
    #[arg(long = "json")]
    pub json: bool,
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
    fn built_in_name_matches_every_subcommand_spelling() {
        let cases = [
            (vec!["init"], "init"),
            (vec!["menu"], "menu"),
            (vec!["list"], "list"),
            (vec!["modules", "list"], "modules"),
            (vec!["test"], "test"),
            (vec!["dag"], "dag"),
            (vec!["logs"], "logs"),
            (vec!["cache", "verify"], "cache"),
            (vec!["serve"], "serve"),
            (vec!["emit-lua"], "emit-lua"),
            (vec!["affected", "--since=HEAD"], "affected"),
            (vec!["why"], "why"),
        ];

        for (argv, expected) in cases {
            let cmd = parse(&argv).cmd.expect("built-in command");
            assert_eq!(cmd.built_in_name(), Some(expected), "argv={argv:?}");
        }

        let recipe = parse(&["deploy"]).cmd.expect("external recipe");
        assert_eq!(recipe.built_in_name(), None);
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
    fn list_subcommand_takes_no_args() {
        assert!(matches!(parse(&["list"]).cmd, Some(Cmd::List)));
    }

    #[test]
    fn list_subcommand_rejects_removed_filter_flags() {
        for flag in ["--recipes-only", "--chores-only"] {
            assert!(
                Cli::try_parse_from(["cook", "list", flag]).is_err(),
                "{flag} was removed along with the bare-name listing"
            );
        }
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
    fn why_subcommand_defaults() {
        match parse(&["why"]).cmd {
            Some(Cmd::Why(a)) => { assert!(a.recipe.is_none()); assert!(!a.json); }
            other => panic!("expected Cmd::Why, got {other:?}"),
        }
    }

    #[test]
    fn why_subcommand_recipe_and_json() {
        match parse(&["why", "build", "--json"]).cmd {
            Some(Cmd::Why(a)) => { assert_eq!(a.recipe.as_deref(), Some("build")); assert!(a.json); }
            other => panic!("expected Cmd::Why, got {other:?}"),
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
    fn cache_verify_subcommand_defaults() {
        let cli = parse(&["cache", "verify"]);
        match cli.cmd {
            Some(Cmd::Cache(CacheArgs { cmd: CacheCmd::Verify(a) })) => {
                assert!(a.recipe.is_none());
                assert!(!a.json);
            }
            other => panic!("expected Cmd::Cache verify, got {other:?}"),
        }
    }

    #[test]
    fn cache_verify_recipe_and_json() {
        let cli = parse(&["cache", "verify", "build", "--json"]);
        match cli.cmd {
            Some(Cmd::Cache(CacheArgs { cmd: CacheCmd::Verify(a) })) => {
                assert_eq!(a.recipe.as_deref(), Some("build"));
                assert!(a.json);
            }
            other => panic!("expected Cmd::Cache verify, got {other:?}"),
        }
    }

    #[test]
    fn reserved_target_covers_every_target_typed_arm() {
        // The dispatcher's single `//`-rejection chokepoint reads
        // Cmd::reserved_target; pin that every target-typed subcommand
        // surfaces its user-supplied target through it.
        for argv in [
            &["test", "//x"][..],
            &["dag", "//x"],
            &["cache", "verify", "//x"],
            &["serve", "//x"],
            &["affected", "--since=main", "--recipe=//x"],
            &["why", "//x"],
        ] {
            let cli = parse(argv);
            let cmd = cli.cmd.expect("subcommand parses");
            assert_eq!(
                cmd.reserved_target(),
                Some("//x"),
                "target-typed arm must surface its target: {argv:?}"
            );
        }
        // No user-supplied target -> nothing to validate.
        assert_eq!(parse(&["test"]).cmd.unwrap().reserved_target(), None);
        assert_eq!(parse(&["menu"]).cmd.unwrap().reserved_target(), None);
        // The external_subcommand arm validates in dispatch_recipe instead.
        assert_eq!(parse(&["//x"]).cmd.unwrap().reserved_target(), None);
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
