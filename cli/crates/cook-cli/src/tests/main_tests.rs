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
