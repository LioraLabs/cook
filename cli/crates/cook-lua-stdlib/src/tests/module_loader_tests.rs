//! The shared `cook.load_module` core, exercised once for both phases.
//!
//! These cover the sequence itself — memoisation, cycle detection with
//! §12.3.3's error-survival rule, resolution order, init(), hook ordering.
//! Phase-specific obligations (module caches, current-module tracking, the
//! register scope surface) are covered where they live, in
//! `cook-register`'s and `cook-luaotp`'s own suites.

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use mlua::prelude::*;

use crate::module_loader::{install_module_loader, ModuleLoadHooks, NoHooks};
use crate::WorkingDirSource;

fn vm_with_loader(working_dir: PathBuf) -> Lua {
    let lua = Lua::new();
    let cook = lua.create_table().unwrap();
    install_module_loader(
        &lua,
        &cook,
        WorkingDirSource::Static(working_dir),
        NoHooks,
    )
    .unwrap();
    lua.globals().set("cook", cook).unwrap();
    lua
}

fn write_module(dir: &std::path::Path, name: &str, source: &str) {
    let modules = dir.join("cook_modules");
    std::fs::create_dir_all(&modules).unwrap();
    std::fs::write(modules.join(format!("{name}.lua")), source).unwrap();
}

#[test]
fn loads_a_module_and_memoizes_the_value() {
    let tmp = tempfile::tempdir().unwrap();
    write_module(
        tmp.path(),
        "counter",
        "counted = (counted or 0) + 1\nreturn { n = counted }",
    );
    let lua = vm_with_loader(tmp.path().to_path_buf());

    let (first, second, evals): (LuaTable, LuaTable, i64) = lua
        .load(
            r#"
            local a = cook.load_module("counter")
            local b = cook.load_module("counter")
            return a, b, counted
        "#,
        )
        .eval()
        .unwrap();
    assert_eq!(evals, 1, "top-level chunk runs once per VM");
    assert_eq!(first.get::<i64>("n").unwrap(), 1);
    // Same Lua value, not an equal copy (§12.3.2).
    let same: bool = lua
        .load(r#"return cook.load_module("counter") == cook.load_module("counter")"#)
        .eval()
        .unwrap();
    assert!(same, "memo returns the identical table");
    drop(second);
}

#[test]
fn init_runs_once_immediately_after_the_chunk() {
    let tmp = tempfile::tempdir().unwrap();
    write_module(
        tmp.path(),
        "with_init",
        "inits = (inits or 0)\nlocal m = {}\nfunction m.init() inits = inits + 1 end\nreturn m",
    );
    let lua = vm_with_loader(tmp.path().to_path_buf());
    let inits: i64 = lua
        .load(
            r#"
            cook.load_module("with_init")
            cook.load_module("with_init")
            return inits
        "#,
        )
        .eval()
        .unwrap();
    assert_eq!(inits, 1);
}

#[test]
fn cycle_is_detected_with_the_rendered_path() {
    let tmp = tempfile::tempdir().unwrap();
    write_module(tmp.path(), "a", r#"cook.load_module("b")"#);
    write_module(tmp.path(), "b", r#"cook.load_module("a")"#);
    let lua = vm_with_loader(tmp.path().to_path_buf());
    let err = lua
        .load(r#"cook.load_module("a")"#)
        .exec()
        .expect_err("cycle must raise");
    let msg = err.to_string();
    assert!(
        msg.contains("module cycle detected: a -> b -> a"),
        "renders the full path: {msg}"
    );
}

#[test]
fn self_cycle_is_detected() {
    let tmp = tempfile::tempdir().unwrap();
    write_module(tmp.path(), "solo", r#"cook.load_module("solo")"#);
    let lua = vm_with_loader(tmp.path().to_path_buf());
    let msg = lua
        .load(r#"cook.load_module("solo")"#)
        .exec()
        .expect_err("self-cycle must raise")
        .to_string();
    assert!(msg.contains("module cycle detected: solo -> solo"), "{msg}");
}

/// §12.3.3: detection survives recoverable errors — a module whose body
/// raised leaves the in-flight set, so a retry proceeds (and can succeed if
/// the failure was transient state, here a flag the test flips).
#[test]
fn inflight_marker_is_dropped_when_the_body_raises() {
    let tmp = tempfile::tempdir().unwrap();
    write_module(
        tmp.path(),
        "flaky",
        "if not please_work then error('not yet') end\nreturn { ok = true }",
    );
    let lua = vm_with_loader(tmp.path().to_path_buf());
    let first = lua.load(r#"cook.load_module("flaky")"#).exec();
    assert!(first.is_err());
    let msg = first.unwrap_err().to_string();
    assert!(
        !msg.contains("module cycle detected"),
        "a failed load is not a cycle: {msg}"
    );
    lua.globals().set("please_work", true).unwrap();
    let ok: bool = lua
        .load(r#"return cook.load_module("flaky").ok"#)
        .eval()
        .unwrap();
    assert!(ok, "retry after a body error proceeds");
}

#[test]
fn inflight_marker_is_dropped_when_init_raises() {
    let tmp = tempfile::tempdir().unwrap();
    write_module(
        tmp.path(),
        "bad_init",
        "local m = {}\nfunction m.init() error('init boom') end\nreturn m",
    );
    let lua = vm_with_loader(tmp.path().to_path_buf());
    assert!(lua.load(r#"cook.load_module("bad_init")"#).exec().is_err());
    let msg = lua
        .load(r#"cook.load_module("bad_init")"#)
        .exec()
        .expect_err("still failing")
        .to_string();
    assert!(
        msg.contains("init boom") && !msg.contains("module cycle detected"),
        "retry re-raises the real error, not a phantom cycle: {msg}"
    );
}

#[test]
fn missing_module_raises_the_shared_diagnostic() {
    let tmp = tempfile::tempdir().unwrap();
    let lua = vm_with_loader(tmp.path().to_path_buf());
    let msg = lua
        .load(r#"cook.load_module("ghost")"#)
        .exec()
        .expect_err("missing module must raise")
        .to_string();
    let expected =
        cook_contracts::layout::module_not_found_message(tmp.path(), "ghost");
    assert!(msg.contains(&expected), "got: {msg}\nwant: {expected}");
}

/// Hand-vendored `<name>.lua` wins over `<name>/init.lua` and the LuaRocks
/// tree (§7 / CS-0069 order, via the one candidate list).
#[test]
fn resolution_prefers_hand_vendored_flat_file() {
    let tmp = tempfile::tempdir().unwrap();
    write_module(tmp.path(), "dual", "return { where = 'flat' }");
    let dir = tmp.path().join("cook_modules/dual");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("init.lua"), "return { where = 'dir' }").unwrap();
    let lua = vm_with_loader(tmp.path().to_path_buf());
    let w: String = lua
        .load(r#"return cook.load_module("dual").where"#)
        .eval()
        .unwrap();
    assert_eq!(w, "flat");
}

/// A `Live` working dir keys memoisation per cwd: one worker VM serving two
/// Cookfiles must not hand Cookfile B the module memoized for Cookfile A
/// under the same name (§12.3.2's `(working_dir, name)` key).
#[test]
fn live_cwd_keys_memoisation_per_cookfile() {
    use std::sync::{Arc, Mutex};
    let tmp_a = tempfile::tempdir().unwrap();
    let tmp_b = tempfile::tempdir().unwrap();
    write_module(tmp_a.path(), "who", "return { name = 'a' }");
    write_module(tmp_b.path(), "who", "return { name = 'b' }");

    let slot = Arc::new(Mutex::new(tmp_a.path().to_path_buf()));
    let lua = Lua::new();
    let cook = lua.create_table().unwrap();
    install_module_loader(
        &lua,
        &cook,
        WorkingDirSource::Live(Arc::clone(&slot)),
        NoHooks,
    )
    .unwrap();
    lua.globals().set("cook", cook).unwrap();

    let a: String = lua
        .load(r#"return cook.load_module("who").name"#)
        .eval()
        .unwrap();
    *slot.lock().unwrap() = tmp_b.path().to_path_buf();
    let b: String = lua
        .load(r#"return cook.load_module("who").name"#)
        .eval()
        .unwrap();
    assert_eq!((a.as_str(), b.as_str()), ("a", "b"));
}

/// Hook ordering: before_eval sees the source ahead of evaluation,
/// after_load reports the outcome on success and failure, memo hits fire
/// on_memo_hit instead.
#[test]
fn hooks_fire_in_sequence() {
    #[derive(Default)]
    struct Recording(Rc<RefCell<Vec<String>>>);
    impl ModuleLoadHooks for Recording {
        fn on_memo_hit(&self, name: &str) {
            self.0.borrow_mut().push(format!("memo:{name}"));
        }
        fn before_eval(&self, name: &str, _source: &str) -> LuaResult<()> {
            self.0.borrow_mut().push(format!("before:{name}"));
            Ok(())
        }
        fn after_load(&self, name: &str, success: bool) {
            self.0.borrow_mut().push(format!("after:{name}:{success}"));
        }
    }

    let tmp = tempfile::tempdir().unwrap();
    write_module(tmp.path(), "ok", "return {}");
    write_module(tmp.path(), "boom", "error('no')");

    let log: Rc<RefCell<Vec<String>>> = Rc::default();
    let lua = Lua::new();
    let cook = lua.create_table().unwrap();
    install_module_loader(
        &lua,
        &cook,
        WorkingDirSource::Static(tmp.path().to_path_buf()),
        Recording(Rc::clone(&log)),
    )
    .unwrap();
    lua.globals().set("cook", cook).unwrap();

    lua.load(r#"cook.load_module("ok")"#).exec().unwrap();
    lua.load(r#"cook.load_module("ok")"#).exec().unwrap();
    let _ = lua.load(r#"cook.load_module("boom")"#).exec();

    assert_eq!(
        *log.borrow(),
        vec![
            "before:ok",
            "after:ok:true",
            "memo:ok",
            "before:boom",
            "after:boom:false",
        ]
    );
}

#[test]
fn renamed_cache_stub_errors_with_did_you_mean() {
    let lua = Lua::new();
    let cook = lua.create_table().unwrap();
    crate::module_loader::install_renamed_cache_stub(&lua, &cook).unwrap();
    lua.globals().set("cook", cook).unwrap();
    let msg = lua
        .load(r#"return cook.cache.get("k")"#)
        .exec()
        .expect_err("stub must raise")
        .to_string();
    assert!(
        msg.contains("'cook.cache' was renamed to 'cook.probes' in v1.0 (use cook.probes.get)"),
        "{msg}"
    );
}
