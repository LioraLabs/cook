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
fn dag_subcommand_with_level_and_format() {
    let cli = parse(&["dag", "host", "--level", "group", "--format", "mermaid"]);
    match cli.cmd {
        Some(Cmd::Dag(args)) => {
            assert_eq!(args.recipe.as_deref(), Some("host"));
            assert_eq!(args.level, "group");
            assert_eq!(args.format, "mermaid");
        }
        other => panic!("expected Cmd::Dag, got {other:?}"),
    }
}

#[test]
fn dag_defaults_to_recipe_level_and_text() {
    let cli = parse(&["dag"]);
    match cli.cmd {
        Some(Cmd::Dag(args)) => {
            assert_eq!(args.level, "recipe");
            assert_eq!(args.format, "text");
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
