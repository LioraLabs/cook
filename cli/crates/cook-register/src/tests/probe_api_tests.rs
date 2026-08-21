use super::*;
use crate::BodyCaptureState;

fn setup(source_file: &str) -> (Lua, SharedProbeRegistry, SharedBodySlot) {
    let lua = Lua::new();
    let cook = lua.create_table().unwrap();
    lua.globals().set("cook", cook.clone()).unwrap();
    let reg: SharedProbeRegistry = Rc::new(RefCell::new(ProbeRegistry::default()));
    let body_slot: SharedBodySlot = Rc::new(RefCell::new(Some(BodyCaptureState::new())));
    install_cook_probe(
        &lua,
        &cook,
        reg.clone(),
        body_slot.clone(),
        source_file.to_string(),
    )
    .unwrap();
    (lua, reg, body_slot)
}

#[test]
fn cook_probe_registers_a_unit() {
    let (lua, reg, _cap) = setup("Cookfile");

    lua.load(r#"
            cook.probe("cc:zlib", {
              inputs = { tools = {"pkg-config"} },
              produce = "return { found = true }",
            })
        "#,
    )
    .exec()
    .unwrap();

    let r = reg.borrow();
    let p = r.probes.get("cc:zlib").expect("probe registered");
    assert_eq!(p.probe.key.as_str(), "cc:zlib");
    assert_eq!(p.probe.produce_source, "return { found = true }");
    assert_eq!(p.probe.inputs.tools, vec!["pkg-config"]);
}

#[test]
fn cook_probe_registers_requires_in_inputs() {
    let (lua, reg, _cap) = setup("Cookfile");

    lua.load(r#"
            cook.probe("cc:libfoo", {
              inputs = { requires = {"cc:compiler"} },
              produce = "return true",
            })
        "#,
    )
    .exec()
    .unwrap();

    let r = reg.borrow();
    let p = r.probes.get("cc:libfoo").expect("probe registered");
    assert_eq!(p.probe.inputs.requires, vec!["cc:compiler"]);
}

    #[test]
fn cook_probe_empty_inputs_table_is_ok() {
    let (lua, reg, _cap) = setup("Cookfile");

    lua.load(r#"
            cook.probe("cc:simple", {
              inputs = {},
              produce = "return 1",
            })
        "#,
    )
    .exec()
    .unwrap();

    let r = reg.borrow();
    assert!(r.probes.contains_key("cc:simple"));
}

#[test]
fn cook_probe_omitting_inputs_defaults_to_empty() {
    let (lua, reg, _cap) = setup("Cookfile");

    lua.load(r#"
            cook.probe("cc:noinputs", {
              produce = "return nil",
            })
        "#,
    )
    .exec()
    .unwrap();

    let r = reg.borrow();
    let p = r.probes.get("cc:noinputs").expect("probe registered");
    assert!(p.probe.inputs.tools.is_empty());
    assert!(p.probe.inputs.files.is_empty());
    assert!(p.probe.inputs.requires.is_empty());
}

#[test]
fn duplicate_probe_key_errors_with_both_locations() {
    let (lua, _reg, _cap) = setup("Cookfile");

    let result = lua
        .load(r#"
            cook.probe("cc:zlib", { inputs = {}, produce = "return 1" })
            cook.probe("cc:zlib", { inputs = {}, produce = "return 2" })
        "#,
        )
        .exec();

    let err = result.unwrap_err().to_string();
    assert!(
        err.contains("probe key 'cc:zlib' declared at"),
        "got: {err}"
    );
    assert!(err.contains("previously declared at"), "got: {err}");
}

#[test]
fn produce_must_be_string_not_function() {
    let (lua, _reg, _cap) = setup("Cookfile");

    let result = lua
        .load(r#"
            cook.probe("cc:zlib", {
              inputs = {},
              produce = function() return 1 end,
            })
        "#,
        )
        .exec();

    let err = result.unwrap_err().to_string();
    assert!(err.contains("must be a string"), "got: {err}");
}

#[test]
fn produce_missing_raises_error() {
    let (lua, _reg, _cap) = setup("Cookfile");

    let result = lua
        .load(r#"cook.probe("k", { inputs = {} })"#)
        .exec();

    assert!(result.is_err(), "missing produce must raise an error");
        let err = result.unwrap_err().to_string();
        assert!(err.contains("produce"), "error should mention 'produce'; got: {err}");
}

#[test]
fn empty_key_raises_error() {
    let (lua, _reg, _cap) = setup("Cookfile");

    let result = lua
        .load(r#"cook.probe("", { inputs = {}, produce = "return 1" })"#)
        .exec();

    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(err.contains("non-empty"), "got: {err}");
}

#[test]
fn inline_seal_key_namespace_raises_error() {
    let (lua, _reg, _cap) = setup("Cookfile");

    let err = lua
        .load(r#"cook.probe("@seal:build:5", { inputs = {}, produce = "return 1" })"#)
        .exec()
        .unwrap_err()
        .to_string();

    assert!(err.contains("keys beginning `@seal:` are reserved"), "got: {err}");
}

#[test]
fn internal_inline_seal_probe_accepts_reserved_key() {
    let (lua, reg, _cap) = setup("Cookfile");

    lua.load(r#"cook.__inline_seal_probe("@seal:build:5", { inputs = {}, produce = "return 1" })"#)
        .exec()
        .unwrap();

    assert!(reg.borrow().probes.contains_key("@seal:build:5"));
}

#[test]
fn multiple_distinct_probes_all_registered() {
    let (lua, reg, _cap) = setup("Cookfile");

    lua.load(r#"
            cook.probe("cc:zlib",  { inputs = {}, produce = "return 1" })
            cook.probe("cc:openssl", { inputs = {}, produce = "return 2" })
            cook.probe("cc:lua", { inputs = {}, produce = "return 3" })
        "#,
    )
    .exec()
    .unwrap();

    let r = reg.borrow();
    assert_eq!(r.probes.len(), 3);
    assert!(r.probes.contains_key("cc:zlib"));
    assert!(r.probes.contains_key("cc:openssl"));
    assert!(r.probes.contains_key("cc:lua"));
}

/// CS-0244: `opts.inputs.env` is removed surface. It fed the probe
/// fingerprint alone — never a probe's value — so with the fingerprint gone
/// accepting it would silently promise a determinant that does not exist. The
/// diagnostic names the sub-key, the offending probe key and the entry, and
/// points at the same shell probe CS-0226 already established for the
/// Cookfile-level `envs { }` with its concrete `lines { … }` spelling.
#[test]
fn cs0244_cook_probe_rejects_inputs_env_by_name() {
    let (lua, _reg, _cap) = setup("Cookfile");

    let err = lua
        .load(
            r#"
            cook.probe("host:cflags", {
              inputs = { env = {"HOME"} },
              produce = "return {}",
            })
        "#,
        )
        .exec()
        .expect_err("opts.inputs.env must be rejected");

    let msg = err.to_string();
    assert!(msg.contains("inputs.env"), "the diagnostic must name the removed sub-key; got: {msg}");
    assert!(msg.contains("CS-0244"), "the diagnostic must name the entry; got: {msg}");
    // Without this the author of a module registering probes in a loop gets a
    // traceback into vendored code and no way to tell which probe.
    assert!(
        msg.contains("host:cflags"),
        "the diagnostic must name the offending probe key; got: {msg}",
    );
    assert!(
        msg.contains("shell probe"),
        "the diagnostic must direct the author to the replacement; got: {msg}",
    );
    assert!(
        msg.contains(r#"lines { echo "$NAME" }"#),
        "the diagnostic must carry the concrete replacement spelling; got: {msg}",
    );
}
