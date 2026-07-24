use super::*;

/// `RESERVED` is hand-maintained but must equal the real reserved set: a
/// name missing here would be completed bare, and the shell would silently
/// dispatch the builtin instead of the user's recipe.
#[test]
fn reserved_names_match_the_parse_tree() {
    let cmd = Cli::command();
    let mut actual: Vec<String> = cmd
        .get_subcommands()
        .map(|s| s.get_name().to_string())
        .collect();
    actual.push("help".to_string());
    actual.sort();

    let mut expected: Vec<String> = RESERVED.iter().map(|s| s.to_string()).collect();
    expected.sort();

    assert_eq!(
        actual, expected,
        "reserved subcommand set drifted; update RESERVED in completion.rs"
    );
}

#[test]
fn module_internal_names_are_recognised_including_when_qualified() {
    assert!(is_module_internal("__cc_config_header__build_config_h"));
    // A workspace member prefix must not hide the `__`.
    assert!(is_module_internal(
        "game.__cc_config_header__build_config_h"
    ));

    assert!(!is_module_internal("build"));
    assert!(!is_module_internal("cli.build"));
    // A single underscore is an ordinary name.
    assert!(!is_module_internal("_private"));
    // `__` must be at the start of the segment, not merely present.
    assert!(!is_module_internal("my__recipe"));
}

#[test]
fn completion_command_does_not_disturb_the_parse_tree() {
    // The augmented command is completion-only; the real tree must still
    // route an unknown positional to the external_subcommand arm.
    let cmd = Cli::command();
    assert!(cmd.find_subcommand("why").is_some());
    assert!(cmd.find_subcommand("build").is_none());
}
