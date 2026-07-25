use super::*;
use mlua::Lua;

/// Returns the VM, the declared-variable STORE (the table config blocks write,
/// reached here the way the engine reaches it — through the registry), and the
/// keyset the surface checks against.
fn setup() -> (Lua, LuaTable, VarKeyset) {
    let lua = Lua::new();
    let cook: LuaTable = lua.create_table().unwrap();
    let store: LuaTable = lua.create_table().unwrap();
    lua.set_named_registry_value(crate::VAR_STORE_REGISTRY_KEY, store.clone())
        .unwrap();
    let ks = VarKeyset::new();
    install_var_api(&lua, &cook, ks.clone()).unwrap();
    lua.globals().set("cook", cook.clone()).unwrap();
    (lua, store, ks)
}

#[test]
fn returns_value_for_declared_key() {
    let (lua, env, ks) = setup();
    env.set("HOME", "/home/alex").unwrap();
    ks.freeze(&env).unwrap();
    let v: String = lua
        .load(r#"return cook.require_var("HOME")"#)
        .eval()
        .unwrap();
    assert_eq!(v, "/home/alex");
}

#[test]
fn returns_empty_string_for_declared_but_empty() {
    let (lua, env, ks) = setup();
    env.set("EMPTY", "").unwrap();
    ks.freeze(&env).unwrap();
    let v: String = lua
        .load(r#"return cook.require_var("EMPTY")"#)
        .eval()
        .unwrap();
    assert_eq!(v, "");
}

#[test]
fn errors_for_undeclared_key() {
    let (lua, env, ks) = setup();
    env.set("HOME", "x").unwrap();
    ks.freeze(&env).unwrap();
    let res: mlua::Result<String> =
        lua.load(r#"return cook.require_var("HOEM")"#).eval();
    assert!(res.is_err());
    let msg = format!("{}", res.unwrap_err());
    assert!(msg.contains("HOEM"), "expected HOEM in: {msg}");
    assert!(msg.contains("declared"), "expected 'declared' in: {msg}");
    assert!(msg.contains("HOME"), "expected HOME in: {msg}");
}

#[test]
fn errors_for_undeclared_key_suggests_closest_matches_only() {
    let (lua, env, ks) = setup();
    for name in [
        "HOMEDIR", "HOME", "PATH", "CC", "CXX", "LANG", "SHELL", "TERM", "USER", "PWD",
    ] {
        env.set(name, "x").unwrap();
    }
    ks.freeze(&env).unwrap();
    let res: mlua::Result<String> =
        lua.load(r#"return cook.require_var("HOMDIR")"#).eval();
    assert!(res.is_err());
    let msg = format!("{}", res.unwrap_err());
    assert!(msg.contains("HOMEDIR"), "expected HOMEDIR in: {msg}");
    assert!(
        msg.contains("var.HOMDIR = "),
        "expected 'var.HOMDIR = ' in: {msg}"
    );
    assert!(
        !msg.contains("PATH"),
        "PATH is far from HOMDIR and should not appear in: {msg}"
    );
    assert!(
        !msg.contains("Declared env vars:"),
        "must not dump the full declared/host env set: {msg}"
    );
}

#[test]
fn edit_distance_is_symmetric_and_case_insensitive() {
    assert_eq!(edit_distance("HOME", "HOME"), 0);
    assert_eq!(edit_distance("HOME", "home"), 0);
    assert_eq!(edit_distance("HOMDIR", "HOMEDIR"), 1);
    assert_eq!(
        edit_distance("HOMDIR", "HOMEDIR"),
        edit_distance("HOMEDIR", "HOMDIR")
    );
    assert_eq!(edit_distance("abc", "xyz"), edit_distance("xyz", "abc"));
}

#[test]
fn closest_declared_picks_top_n_by_distance() {
    let declared: Vec<String> = [
        "HOMEDIR", "HOME", "PATH", "CC", "CXX", "LANG", "SHELL", "TERM", "USER", "PWD",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    let closest = closest_declared("HOMDIR", &declared, 3);
    assert_eq!(closest.len(), 3);
    assert!(
        closest.contains(&"HOMEDIR".to_string()),
        "expected HOMEDIR among closest: {closest:?}"
    );
    assert!(
        !closest.contains(&"PATH".to_string()),
        "PATH should not be among closest: {closest:?}"
    );
}

#[test]
fn errors_when_no_declarations_at_all() {
    let (lua, env, ks) = setup();
    ks.freeze(&env).unwrap();
    let res: mlua::Result<String> = lua.load(r#"return cook.require_var("X")"#).eval();
    assert!(res.is_err());
    let msg = format!("{}", res.unwrap_err());
    assert!(
        msg.contains("no config block declares"),
        "expected 'no config block declares' in: {msg}"
    );
}

#[test]
fn post_freeze_write_does_not_make_key_declared() {
    let (lua, env, ks) = setup();
    // Freeze with empty env (no config-block declarations)
    ks.freeze(&env).unwrap();
    // Simulate a recipe-time write to the live env table after the freeze
    env.set("LATE", "value").unwrap();
    // The post-freeze write makes the store's "LATE" visible, but require_var
    // must still error — LATE was not in scope at freeze time, so it does
    // not satisfy the "declared" contract from §xref.resolution step 3.
    let res: mlua::Result<String> = lua.load(r#"return cook.require_var("LATE")"#).eval();
    assert!(res.is_err(), "post-freeze write must not declare key");
    let msg = format!("{}", res.unwrap_err());
    assert!(msg.contains("LATE") && msg.contains("no config block declares"),
        "diagnostic must name LATE and mention it is not declared; got: {}", msg);
}
