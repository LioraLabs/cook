use super::*;

#[test]
fn partition_peels_trailing_global_bool_flag() {
    let mut g = crate::cli::Globals::default();
    let p = partition_argv(&["-v".to_string()], "report", &mut g).unwrap();
    assert!(p.argv.is_empty());
    assert!(g.verbose);
}

#[test]
fn partition_peels_trailing_global_value_flag() {
    let mut g = crate::cli::Globals::default();
    let p = partition_argv(
        &[
            "--output".to_string(),
            "plain".to_string(),
            "-j".to_string(),
            "4".to_string(),
        ],
        "report",
        &mut g,
    )
    .unwrap();
    assert!(p.argv.is_empty());
    assert_eq!(g.output, "plain");
    assert_eq!(g.jobs, Some(4));
}

#[test]
fn partition_keeps_chore_params_as_argv() {
    let mut g = crate::cli::Globals::default();
    let p = partition_argv(&["who=world".to_string()], "greet", &mut g).unwrap();
    assert_eq!(p.argv, vec!["who=world".to_string()]);
}

/// COOK-405: `cook build --replay-logs` used to fail with "recipes do not take
/// parameters; received 1 positional argument" while `cook --replay-logs build`
/// worked. It was the one global clap declares that this table forgot.
#[test]
fn partition_extracts_replay_logs_from_post_recipe_position() {
    let mut g = crate::cli::Globals::default();
    let p = partition_argv(&["--replay-logs".to_string()], "build", &mut g).unwrap();
    assert!(p.argv.is_empty());
    assert!(g.replay_logs);
}

/// The real defect behind COOK-405 is that `partition_argv` hand-mirrors a flag
/// table clap already owns, so the two can disagree silently. Deriving the list
/// at runtime is not possible through clap's API, so this derives it from the
/// declaration instead: every `long = "…"` marked `global = true` in `cli.rs`
/// must appear as a match arm in `partition_argv`.
///
/// Source-scanning rather than behavioural because the point is to catch a flag
/// nobody thought to write a test for. Same shape as `cook-contracts`'
/// `tests/layout.rs`.
#[test]
fn every_global_flag_clap_declares_is_peeled_by_partition_argv() {
    let cli_src = include_str!("../cli.rs");
    let main_src = include_str!("../main.rs");

    // `partition_argv`'s body, so an unrelated mention elsewhere in main.rs
    // cannot satisfy the assertion.
    let body = main_src
        .split_once("fn partition_argv")
        .expect("partition_argv not found in main.rs")
        .1;

    let mut declared: Vec<String> = Vec::new();
    let mut pending: Option<String> = None;
    for line in cli_src.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("#[arg(") || pending.is_some() {
            let attr = pending
                .take()
                .map_or_else(|| trimmed.to_string(), |prev| format!("{prev} {trimmed}"));
            // An `#[arg(...)]` may wrap across lines; accumulate until balanced.
            if attr.matches('(').count() > attr.matches(')').count() {
                pending = Some(attr);
                continue;
            }
            if !attr.contains("global = true") {
                continue;
            }
            if let Some(rest) = attr.split_once("long = \"") {
                if let Some((name, _)) = rest.1.split_once('"') {
                    declared.push(name.to_string());
                }
            }
        }
    }

    assert!(
        declared.len() > 5,
        "scanner found only {} global long flags in cli.rs; the attribute shape \
         probably changed and this test has stopped checking anything",
        declared.len()
    );

    let missing: Vec<&String> = declared
        .iter()
        .filter(|name| !body.contains(&format!("\"--{name}\"")))
        .collect();

    assert!(
        missing.is_empty(),
        "clap declares these as global flags but partition_argv does not peel \
         them, so they are rejected in trailing position (`cook build --flag`) \
         while working in leading position (`cook --flag build`): {missing:?}"
    );
}

#[test]
fn partition_extracts_no_auto_gc_from_post_recipe_position() {
    // The recipe name is a clap `external_subcommand`, which swallows every
    // trailing flag (including globals) into its positional vec — so
    // `cook build --no-auto-gc` must have `--no-auto-gc` re-extracted here,
    // exactly like `--no-prune` already is.
    let mut g = crate::cli::Globals::default();
    let p = partition_argv(&["--no-auto-gc".to_string()], "build", &mut g).unwrap();
    assert!(p.argv.is_empty());
    assert!(g.no_auto_gc);
}

/// A preset selector must have the shape a preset DECLARATION can have.
///
/// COOK-421 consolidated the character class this and `cook-lang`'s lexer
/// share, and the first pass got it half right: it tested the continue class
/// against every character and never asked for a start character, so
/// `cook build @9x` was taken as a preset selector while `config 9x` cannot be
/// declared. The asymmetry ran the opposite way from the one the
/// consolidation set out to fix, and is why the whole name is tested against
/// one function rather than one predicate per character.
#[test]
fn a_preset_selector_needs_a_declarable_name() {
    let mut g = crate::cli::Globals::default();
    for token in ["@9x", "@-x", "@.x", "@"] {
        let p = partition_argv(&[token.to_string()], "build", &mut g).unwrap();
        assert!(
            p.preset.is_none(),
            "{token} is not a name a Cookfile can declare, so it is a literal argv value"
        );
        assert_eq!(p.argv, vec![token.to_string()]);
    }

    let mut g = crate::cli::Globals::default();
    let p = partition_argv(&["@fast.v2".to_string()], "build", &mut g).unwrap();
    assert_eq!(p.preset.as_deref(), Some("fast.v2"), "a declarable name selects a preset");
    assert!(p.argv.is_empty());
}

/// `cook why` does not flow through `partition_argv`, so its sigil strip is a
/// second reader of the same shape and must agree with the first.
#[test]
fn the_why_path_strips_the_same_shapes_and_no_others() {
    assert_eq!(strip_preset_sigil("@fast.v2"), "fast.v2");
    assert_eq!(strip_preset_sigil("@_x-1"), "_x-1");
    for verbatim in ["@9x", "@-x", "@", "@a/b", "plain"] {
        assert_eq!(strip_preset_sigil(verbatim), verbatim, "{verbatim} passes through");
    }
}
