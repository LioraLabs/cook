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

#[test]
fn child_command_requires_active_chore_and_explicit_context() {
    let lua = Lua::new();
    lua.globals()
        .set("cook", lua.create_table().unwrap())
        .unwrap();
    let slot = Rc::new(RefCell::new(Some(crate::BodyCaptureState::default())));
    install_child_command_api(
        &lua,
        slot.clone(),
        None,
        String::new(),
        Default::default(),
        Default::default(),
    )
    .unwrap();
    let error = lua
        .load("return cook.child_command('target')")
        .eval::<String>()
        .unwrap_err();
    assert!(error.to_string().contains("active chore"));
    slot.borrow_mut().as_mut().unwrap().current_chore_active = true;
    let error = lua
        .load("return cook.child_command('target')")
        .eval::<String>()
        .unwrap_err();
    assert!(error.to_string().contains("embedding caller must supply"));
    for value in ["42", "''", "'a' .. string.char(0)"] {
        assert!(lua
            .load(format!("return cook.child_command({value})"))
            .eval::<String>()
            .is_err());
    }
}
