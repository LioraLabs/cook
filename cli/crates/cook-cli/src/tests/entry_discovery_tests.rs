use super::*;
use clap::CommandFactory;

fn matches_for(argv: &[&str]) -> clap::ArgMatches {
    let mut full = vec!["cook"];
    full.extend_from_slice(argv);
    <cli::Cli as CommandFactory>::command()
        .try_get_matches_from(full)
        .expect("parse")
}

#[test]
fn default_file_is_not_explicit() {
    assert!(!cookfile_flag_was_explicit(&matches_for(&["build"])));
    assert!(!cookfile_flag_was_explicit(&matches_for(&["menu"])));
    assert!(!cookfile_flag_was_explicit(&matches_for(&[])));
}

#[test]
fn pre_subcommand_flag_is_explicit() {
    assert!(cookfile_flag_was_explicit(&matches_for(&[
        "-f", "sub/Cookfile", "build"
    ])));
}

#[test]
fn post_subcommand_global_flag_is_explicit() {
    // global=true args given after a named subcommand propagate up to the
    // top-level matches (pinned by cli.rs::globals_apply_after_subcommand);
    // value_source must see them as CommandLine too.
    assert!(cookfile_flag_was_explicit(&matches_for(&[
        "test", "-f", "sub/Cookfile"
    ])));
}
