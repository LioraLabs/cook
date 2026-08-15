use super::*;
use crate::BodyCaptureState;
use std::cell::RefCell;
use std::rc::Rc;

fn body_ref(body_slot: &SharedBodySlot) -> std::cell::Ref<'_, BodyCaptureState> {
    std::cell::Ref::map(body_slot.borrow(), |slot| {
        slot.as_ref().expect("body slot populated for test")
    })
}

fn setup_lua() -> (Lua, SharedTerminalOutputs, SharedBodySlot) {
    let lua = Lua::new();
    lua.globals().set("cook", lua.create_table().unwrap()).unwrap();
    let terminal_outputs: SharedTerminalOutputs = Arc::new(Mutex::new(BTreeMap::new()));
    let body_slot: SharedBodySlot =
        Rc::new(RefCell::new(Some(BodyCaptureState::new())));
    (lua, terminal_outputs, body_slot)
}

#[test]
fn test_dep_output_returns_space_joined() {
    let (lua, outputs, cs) = setup_lua();
    outputs.lock().unwrap().insert(
        "protos".into(),
        vec!["gen/foo.pb.o".into(), "gen/bar.pb.o".into()],
    );
    register_dep_output_api(&lua, outputs, cs, BTreeMap::new(), String::new(), BTreeMap::new()).unwrap();
    let result: String = lua
        .load(r#"return cook.dep_output("protos")"#)
        .eval()
        .unwrap();
    assert_eq!(result, "gen/foo.pb.o gen/bar.pb.o");
}

#[test]
fn test_dep_output_list_returns_table() {
    let (lua, outputs, cs) = setup_lua();
    outputs
        .lock().unwrap()
        .insert("libmath".into(), vec!["build/lib/libmath.a".into()]);
    register_dep_output_api(&lua, outputs, cs, BTreeMap::new(), String::new(), BTreeMap::new()).unwrap();
    let result: Vec<String> = lua
        .load(r#"return cook.dep_output_list("libmath")"#)
        .eval()
        .unwrap();
    assert_eq!(result, vec!["build/lib/libmath.a"]);
}

#[test]
fn test_dep_output_unknown_recipe_errors() {
    let (lua, outputs, cs) = setup_lua();
    register_dep_output_api(&lua, outputs, cs, BTreeMap::new(), String::new(), BTreeMap::new()).unwrap();
    let result = lua
        .load(r#"return cook.dep_output("nonexistent")"#)
        .eval::<String>();
    assert!(result.is_err());
}

#[test]
fn test_dep_output_accumulates_dep_ref() {
    let (lua, outputs, cs) = setup_lua();
    outputs
        .lock().unwrap()
        .insert("libmath".into(), vec!["libmath.a".into()]);
    register_dep_output_api(&lua, outputs, cs.clone(), BTreeMap::new(), String::new(), BTreeMap::new()).unwrap();
    lua.load(r#"cook.dep_output("libmath")"#).exec().unwrap();
    // dep_output accumulates in step_group_dep_refs, not dep_edges directly.
    // Actual edge recording happens in cook.add_unit().
    let state = body_ref(&cs);
    assert_eq!(state.step_group_dep_refs, vec!["libmath".to_string()]);
    // No direct dep_edges yet — add_unit would create them.
    assert!(state.dep_edges.is_empty());
}

#[test]
fn test_dep_output_deduplicates_refs() {
    let (lua, outputs, cs) = setup_lua();
    outputs
        .lock().unwrap()
        .insert("libmath".into(), vec!["libmath.a".into()]);
    register_dep_output_api(&lua, outputs, cs.clone(), BTreeMap::new(), String::new(), BTreeMap::new()).unwrap();
    lua.load(r#"
            cook.dep_output("libmath")
            cook.dep_output("libmath")
        "#).exec().unwrap();
    let state = body_ref(&cs);
    // Should not duplicate
    assert_eq!(state.step_group_dep_refs, vec!["libmath".to_string()]);
}

/// COOK-297: `cook.dep_order` records ONLY the edge ref — no
/// terminal-output lookup (a zero-step meta recipe is a legal target, so
/// none is registered here) and no entry in `step_group_dep_input_paths`
/// (nothing may land in `cache_meta.input_paths`; that is the whole
/// point of the API vs `dep_output`).
#[test]
fn test_dep_order_accumulates_ref_without_input_paths() {
    let (lua, outputs, cs) = setup_lua();
    register_dep_output_api(&lua, outputs, cs.clone(), BTreeMap::new(), String::new(), BTreeMap::new()).unwrap();
    lua.load(r#"cook.dep_order("libmath")"#).exec().unwrap();
    let state = body_ref(&cs);
    assert_eq!(state.step_group_dep_refs, vec!["libmath".to_string()]);
    assert!(
        state.step_group_dep_input_paths.is_empty(),
        "dep_order must not fold any path into the cache-input accumulator"
        );
        // No direct dep_edges yet — add_unit creates them.
        assert!(state.dep_edges.is_empty());
    }

    /// COOK-297: dep_order and dep_output share one ref namespace — naming
    /// the same recipe through both accumulates a single ref.
    #[test]
    fn test_dep_order_dedupes_against_dep_output() {
        let (lua, outputs, cs) = setup_lua();
        outputs
            .lock().unwrap()
            .insert("libmath".into(), vec!["libmath.a".into()]);
    register_dep_output_api(&lua, outputs, cs.clone(), BTreeMap::new(), String::new(), BTreeMap::new()).unwrap();
    lua.load(r#"
            cook.dep_output("libmath")
            cook.dep_order("libmath")
        "#).exec().unwrap();
    let state = body_ref(&cs);
    assert_eq!(state.step_group_dep_refs, vec!["libmath".to_string()]);
}

/// COOK-297: dep_order resolves names exactly like dep_output — a bare
/// same-Cookfile name under `qualified_prefix = "queue"` records the
/// qualified global key.
#[test]
fn test_dep_order_same_cookfile_uses_self_prefix() {
    let (lua, outputs, cs) = setup_lua();
    register_dep_output_api(
        &lua,
        outputs,
        cs.clone(),
        BTreeMap::new(),
        "queue".to_string(),
        BTreeMap::new(),
    )
    .unwrap();
    lua.load(r#"cook.dep_order("local_recipe")"#).exec().unwrap();
    let state = body_ref(&cs);
    assert_eq!(state.step_group_dep_refs, vec!["queue.local_recipe".to_string()]);
}

/// COOK-297: dep_order outside a recipe body raises, mirroring dep_output.
#[test]
fn test_dep_order_outside_body_errors() {
    let (lua, outputs, _) = setup_lua();
    let empty_slot: SharedBodySlot = Rc::new(RefCell::new(None));
    register_dep_output_api(&lua, outputs, empty_slot, BTreeMap::new(), String::new(), BTreeMap::new()).unwrap();
    let res = lua.load(r#"cook.dep_order("libmath")"#).exec();
    assert!(res.is_err(), "dep_order outside a recipe body must raise");
}

#[test]
fn test_dep_output_rewrites_qualified_paths_with_alias_dir() {
    let (lua, outputs, cs) = setup_lua();
    outputs.lock().unwrap().insert(
        "lib.lib_build".into(),
        vec!["build/lib.o".into()],
    );
    let mut alias_dirs = BTreeMap::new();
    alias_dirs.insert("lib".to_string(), PathBuf::from("lib"));

    register_dep_output_api(&lua, outputs, cs, alias_dirs, String::new(), BTreeMap::new()).unwrap();
    let result: String = lua
        .load(r#"return cook.dep_output("lib.lib_build")"#)
        .eval()
        .unwrap();
    assert_eq!(result, "lib/build/lib.o");
}

#[test]
fn test_dep_output_unqualified_no_rewrite() {
    let (lua, outputs, cs) = setup_lua();
    outputs.lock().unwrap().insert(
        "local_recipe".into(),
        vec!["build/local.o".into()],
    );
    register_dep_output_api(&lua, outputs, cs, BTreeMap::new(), String::new(), BTreeMap::new()).unwrap();
    let result: String = lua
        .load(r#"return cook.dep_output("local_recipe")"#)
        .eval()
        .unwrap();
    assert_eq!(result, "build/local.o");
}

#[test]
fn test_dep_output_sigil_alias_with_dotdot() {
    let (lua, outputs, cs) = setup_lua();
    outputs.lock().unwrap().insert(
        "core.core_lib".into(),
        vec!["build/core.o".into()],
    );
    let mut alias_dirs = BTreeMap::new();
    alias_dirs.insert("core".to_string(), PathBuf::from("../../core/lib"));

    register_dep_output_api(&lua, outputs, cs, alias_dirs, String::new(), BTreeMap::new()).unwrap();
    let result: String = lua
        .load(r#"return cook.dep_output("core.core_lib")"#)
        .eval()
        .unwrap();
    assert_eq!(result, "../../core/lib/build/core.o");
}

#[test]
fn test_dep_output_list_rewrites_qualified_paths() {
    let (lua, outputs, cs) = setup_lua();
    outputs.lock().unwrap().insert(
        "lib.lib_build".into(),
        vec!["build/foo.o".into(), "build/bar.o".into()],
    );
    let mut alias_dirs = BTreeMap::new();
    alias_dirs.insert("lib".to_string(), PathBuf::from("lib"));

    register_dep_output_api(&lua, outputs, cs, alias_dirs, String::new(), BTreeMap::new()).unwrap();
    let result: Vec<String> = lua
        .load(r#"return cook.dep_output_list("lib.lib_build")"#)
        .eval()
        .unwrap();
    assert_eq!(result, vec!["lib/build/foo.o", "lib/build/bar.o"]);
}

/// Transitive sigil case: when `apps/server` invokes a recipe whose chain is
/// `server → //libs/queue → //libs/proto`, queue's registry knows that its local
/// alias `"proto"` resolves to the canonical importee prefix `"server.queue.proto"`.
/// Queue's Lua calls `cook.dep_output("proto.proto_lib")` and the lookup must
/// reach `"server.queue.proto.proto_lib"`.
#[test]
fn test_dep_output_resolves_via_alias_qualified_prefix() {
    let (lua, outputs, cs) = setup_lua();
    outputs.lock().unwrap().insert(
        "server.queue.proto.proto_lib".into(),
        vec!["build/proto.bin".into()],
    );
    let mut alias_dirs = BTreeMap::new();
    alias_dirs.insert("proto".to_string(), PathBuf::from("../proto"));
    let mut alias_qp = BTreeMap::new();
    alias_qp.insert("proto".to_string(), "server.queue.proto".to_string());

    register_dep_output_api(
        &lua,
        outputs,
        cs.clone(),
        alias_dirs,
        "server.queue".to_string(),
        alias_qp,
    )
    .unwrap();
    let result: String = lua
        .load(r#"return cook.dep_output("proto.proto_lib")"#)
        .eval()
        .unwrap();
    assert_eq!(result, "../proto/build/proto.bin");
    // The dep ref recorded must be the GLOBAL key so DAG edge wiring lines up
    // with recipe_leaves (which uses globally-qualified names).
    let state = body_ref(&cs);
    assert_eq!(
        state.step_group_dep_refs,
        vec!["server.queue.proto.proto_lib".to_string()]
    );
}

/// Diamond case: `apps/cli` and `apps/server → libs/queue` both reach `libs/proto`.
/// proto has ONE canonical storage prefix (e.g. `"server.queue.proto"`). cli's local
/// alias `"proto"` MUST also resolve to the same canonical prefix — not to
/// `"cli.proto.proto_lib"` (which doesn't exist). The alias_qualified_prefixes map
/// is what makes this work: pipeline.rs supplies cli's map as
/// `{"proto" → "server.queue.proto"}`, the canonical importee prefix.
#[test]
fn test_dep_output_diamond_resolves_to_canonical_importee_prefix() {
    let (lua, outputs, cs) = setup_lua();
    // proto's canonical storage key (from server's chain winning find_full_prefix).
    outputs.lock().unwrap().insert(
        "server.queue.proto.proto_lib".into(),
        vec!["build/proto.bin".into()],
    );
    let mut alias_dirs = BTreeMap::new();
    // cli's importer-relative path to proto: ../../libs/proto.
    alias_dirs.insert("proto".to_string(), PathBuf::from("../../libs/proto"));
    let mut alias_qp = BTreeMap::new();
    // CLI's alias "proto" → proto's canonical workspace prefix.
    alias_qp.insert("proto".to_string(), "server.queue.proto".to_string());

    // CLI's own qualified_prefix is "cli". Without the alias-map indirection,
    // CS-0028's prefix-prepend would look up "cli.proto.proto_lib" → fail.
    register_dep_output_api(
        &lua,
        outputs,
        cs.clone(),
        alias_dirs,
        "cli".to_string(),
        alias_qp,
    )
    .unwrap();
    let result: String = lua
        .load(r#"return cook.dep_output("proto.proto_lib")"#)
        .eval()
        .unwrap();
    assert_eq!(result, "../../libs/proto/build/proto.bin");
    let state = body_ref(&cs);
    assert_eq!(
        state.step_group_dep_refs,
        vec!["server.queue.proto.proto_lib".to_string()]
    );
}

/// Same-Cookfile reference (no dot) falls back to self-prefix qualification.
/// A registry with `qualified_prefix = "queue"` calling
/// `cook.dep_output("local_recipe")` must look up `"queue.local_recipe"`.
#[test]
fn test_dep_output_same_cookfile_uses_self_prefix() {
    let (lua, outputs, cs) = setup_lua();
    outputs.lock().unwrap().insert(
        "queue.local_recipe".into(),
        vec!["build/local.bin".into()],
    );

    register_dep_output_api(
        &lua,
        outputs,
        cs.clone(),
        BTreeMap::new(),
        "queue".to_string(),
        BTreeMap::new(),
    )
    .unwrap();
    let result: String = lua
        .load(r#"return cook.dep_output("local_recipe")"#)
        .eval()
        .unwrap();
    assert_eq!(result, "build/local.bin");
    let state = body_ref(&cs);
    assert_eq!(state.step_group_dep_refs, vec!["queue.local_recipe".to_string()]);
}

#[test]
fn test_dep_output_member_returns_member_output() {
    let lua = Lua::new();
    lua.globals().set("cook", lua.create_table().unwrap()).unwrap();
    let member_outputs: SharedMemberOutputs = Arc::new(Mutex::new(BTreeMap::new()));
    {
        let mut m = member_outputs.lock().unwrap();
        let mut render = BTreeMap::new();
        render.insert("{\"id\":\"s1\"}".to_string(), vec!["build/s1.silent.mp4".to_string()]);
        m.insert("render".to_string(), render);
        }
        let body_slot: SharedBodySlot = Rc::new(RefCell::new(Some(BodyCaptureState::new())));
        register_member_output_api(&lua, member_outputs, body_slot, String::new(), BTreeMap::new()).unwrap();
        let got: String = lua
            .load(r#"return cook.dep_output_member("render", "{\"id\":\"s1\"}")"#)
        .eval()
        .unwrap();
    assert_eq!(got, "build/s1.silent.mp4");
}

#[test]
fn test_dep_output_member_missing_member_errors() {
    let lua = Lua::new();
    lua.globals().set("cook", lua.create_table().unwrap()).unwrap();
    let member_outputs: SharedMemberOutputs = Arc::new(Mutex::new(BTreeMap::new()));
    {
        let mut m = member_outputs.lock().unwrap();
        m.insert("render".to_string(), BTreeMap::new()); // recipe known, no members
        }
        let body_slot: SharedBodySlot = Rc::new(RefCell::new(Some(BodyCaptureState::new())));
        register_member_output_api(&lua, member_outputs, body_slot, String::new(), BTreeMap::new()).unwrap();
        let res = lua.load(r#"return cook.dep_output_member("render", "{\"id\":\"nope\"}")"#).eval::<String>();
    assert!(res.is_err());
}

/// The recording is the load-bearing part of COOK-96: it is what makes the
/// per-member DAG edge and the fingerprint fold fire (mirrors `dep_output`).
/// Pin it so a regression in the body-slot writes is caught here, not only
/// end-to-end in the integration test.
#[test]
fn test_dep_output_member_records_dep_ref_and_input_path() {
    let lua = Lua::new();
    lua.globals().set("cook", lua.create_table().unwrap()).unwrap();
    let member_outputs: SharedMemberOutputs = Arc::new(Mutex::new(BTreeMap::new()));
    {
        let mut m = member_outputs.lock().unwrap();
        let mut render = BTreeMap::new();
        render.insert("{\"id\":\"s1\"}".to_string(), vec!["build/s1.silent.mp4".to_string()]);
        m.insert("render".to_string(), render);
        }
        let body_slot: SharedBodySlot = Rc::new(RefCell::new(Some(BodyCaptureState::new())));
        register_member_output_api(&lua, member_outputs, body_slot.clone(), String::new(), BTreeMap::new()).unwrap();
        lua.load(r#"return cook.dep_output_member("render", "{\"id\":\"s1\"}")"#)
        .eval::<String>()
        .unwrap();
    let state = body_ref(&body_slot);
    assert_eq!(state.step_group_dep_refs, vec!["render".to_string()]);
        // COOK-96: the member's path lands in the per-unit buffer (drained by the
        // next add_unit), NOT the step-group-wide accumulator — this is what keeps
        // each fan-out member's fingerprint isolated. The recipe-level ref above
        // stays step-group-wide (the ordering edge IS shared across members).
        assert_eq!(
            state.pending_member_dep_input_paths,
            vec!["build/s1.silent.mp4".to_string()]
    );
    assert!(
        state.step_group_dep_input_paths.is_empty(),
        "member paths must not leak into the step-group-wide accumulator"
        );
    }

    /// Empty self-prefix and empty alias map (entry-point Cookfile, no imports):
    /// the local name is the global key directly.
    #[test]
    fn test_dep_output_empty_qualified_prefix_no_translation() {
        let (lua, outputs, cs) = setup_lua();
        outputs.lock().unwrap().insert(
            "local_recipe".into(),
        vec!["build/local.bin".into()],
    );

    register_dep_output_api(
        &lua,
        outputs,
        cs.clone(),
        BTreeMap::new(),
        String::new(),
        BTreeMap::new(),
    )
    .unwrap();
    let result: String = lua
        .load(r#"return cook.dep_output("local_recipe")"#)
        .eval()
        .unwrap();
    assert_eq!(result, "build/local.bin");
    let state = body_ref(&cs);
    assert_eq!(state.step_group_dep_refs, vec!["local_recipe".to_string()]);
}
