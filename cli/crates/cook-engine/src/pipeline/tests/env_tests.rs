use super::*;

// CS-0172 removed the ambient-process-env and `.env` layers: the declared
// variable namespace is exactly what a Cookfile's `config` blocks write, so
// the only thing left to resolve here is the `--set` override list. The
// declared-name check on those overrides lives in `cook-register`
// (`check_overrides_declared`), which is where the declared set exists.

#[test]
fn test_parse_cli_overrides_splits_on_first_equals() {
    let map = parse_cli_overrides(&[
        "MODE=release".to_string(),
        "FLAGS=-DA=1 -DB=2".to_string(),
    ])
    .unwrap();
    assert_eq!(map.get("MODE").unwrap(), "release");
    // Only the FIRST '=' separates; the rest belongs to the value.
    assert_eq!(map.get("FLAGS").unwrap(), "-DA=1 -DB=2");
}

#[test]
fn test_parse_cli_overrides_accepts_empty_value() {
    let map = parse_cli_overrides(&["EMPTY=".to_string()]).unwrap();
    assert_eq!(map.get("EMPTY").unwrap(), "");
}

#[test]
fn test_parse_cli_overrides_rejects_missing_equals() {
    let result = parse_cli_overrides(&["NOT_A_PAIR".to_string()]);
    assert!(matches!(result, Err(PipelineError::InvalidSet(_))));
}
