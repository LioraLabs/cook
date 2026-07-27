use super::*;
use cook_contracts::CommandFailure;

#[test]
fn command_failure_uses_shared_json_contract() {
    let command = "printf 'key:value\\n\"quoted\"'\nexit 7";
    let dir = tempfile::tempdir().unwrap();
    let error = run_shell_command(command, dir.path(), &HashMap::new(), 23, "json_failure")
        .expect_err("command should fail");
    let wire = error.to_string();
    let failure = CommandFailure::from_wire(&wire).expect("canonical command failure JSON");

    assert_eq!(failure.line(), 23);
    assert_eq!(failure.exit_code(), 7);
    assert_eq!(failure.command(), command);
    assert_eq!(failure.stdout().as_str(), "key:value\n\"quoted\"");
    assert_eq!(failure.stderr().as_str(), "");
}
