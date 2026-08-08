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
use crate::module_observer::ModuleObserver;
use crate::WorkingDirSource;

fn vm_with_loader(working_dir: PathBuf) -> Lua {
    let lua = Lua::new();
    let cook = lua.create_table().unwrap();
    install_module_loader(
        &lua,
        &cook,
        WorkingDirSource::Static(working_dir),
        NoHooks,
        ModuleObserver::new(),
    )
    .unwrap();
    lua.globals().set("cook", cook).unwrap();
    lua
}

/// Install `<name>.lua` where a rock installs it, so `cook.load_module(name)`
/// resolves by NAME. Routed through `cook_contracts::layout` rather than
/// spelling the subtree, so the next move of the tree root does not have to
/// touch this suite (CS-0207 moved it from `cook_modules/` to
/// `.cook/modules/`).
fn installed_share_dir(dir: &std::path::Path) -> PathBuf {
    cook_contracts::layout::modules_dir(dir)
        .join(cook_contracts::layout::MODULES_SHARE_LUA_SUBDIR)
}

fn write_module(dir: &std::path::Path, name: &str, source: &str) {
    let modules = installed_share_dir(dir);
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
    // CS-0206 split this: the SENTENCE is law in `cook-contracts`, the two
    // directory stats are this crate's (the contracts admission bar excludes
    // the filesystem). Composing the expectation through the same observer the
    // loader uses is what keeps the split from drifting into two messages.
    let expected = cook_contracts::layout::module_not_found_message(
        tmp.path(),
        "ghost",
        crate::module_loader::observe_module_tree(tmp.path()),
    );
    assert!(msg.contains(&expected), "got: {msg}\nwant: {expected}");
}

/// The conditional halves of that diagnostic, executed against a real tree.
/// The sentences are pinned in `cook-contracts`; what is pinned HERE is that
/// this crate observes the right two directories, which is the half a pure
/// test cannot reach.
#[test]
fn a_stale_package_directory_is_observed_and_reported() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tmp.path().join("cook_modules")).unwrap();
    let lua = vm_with_loader(tmp.path().to_path_buf());
    let msg = lua
        .load(r#"cook.load_module("ghost")"#)
        .exec()
        .expect_err("missing module must raise")
        .to_string();
    assert!(msg.contains("still exists"), "{msg}");
    assert!(msg.contains("cook modules install"), "{msg}");
    assert!(msg.contains("use ./path/to/it.lua"), "{msg}");
}

#[test]
fn a_wiped_dot_cook_is_observed_and_reported() {
    let tmp = tempfile::tempdir().unwrap();
    let lua = vm_with_loader(tmp.path().to_path_buf());
    let msg = lua
        .load(r#"cook.load_module("ghost")"#)
        .exec()
        .expect_err("missing module must raise")
        .to_string();
    assert!(msg.contains("no module tree here"), "{msg}");
    assert!(!msg.contains("still exists"), "{msg}");
}

/// The flat `<name>.lua` candidate wins over `<name>/init.lua` — the whole of
/// the CS-0207 candidate list, in order, via the one shared list. (Pre-CS-0207
/// this also pinned the hand-vendored top level ahead of both; that level was
/// withdrawn, and a project patches a module by pointing a path-form `use` at
/// its own file.)
#[test]
fn resolution_prefers_the_flat_file_over_the_init_directory() {
    let tmp = tempfile::tempdir().unwrap();
    write_module(tmp.path(), "dual", "return { where = 'flat' }");
    let dir = installed_share_dir(tmp.path()).join("dual");
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
        ModuleObserver::new(),
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
        ModuleObserver::new(),
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

// ---------------------------------------------------------------------------
// refresh_package_search_paths (relocated from cook-luaotp with the fn)
// ---------------------------------------------------------------------------

#[test]
fn refresh_sets_path_and_cpath_with_rock_tree_entries() {
    use crate::module_loader::refresh_package_search_paths;
    let lua = Lua::new();
    let cwd = PathBuf::from("/tmp/fake-project");
    refresh_package_search_paths(&lua, &cwd).expect("refresh");
    let pkg: LuaTable = lua.globals().get("package").unwrap();
    let path: String = pkg.get("path").unwrap();
    let cpath: String = pkg.get("cpath").unwrap();

    assert!(path.contains("/tmp/fake-project/.cook/modules/share/lua/5.4/?.lua"));
    assert!(path.contains("/tmp/fake-project/.cook/modules/share/lua/5.4/?/init.lua"));
    assert!(cpath.contains("/tmp/fake-project/.cook/modules/lib/lua/5.4/?."));

    // CS-0207: the top-level `?.lua` / `?.<ext>` entries that used to lead each
    // list went with the hand-vendored candidates they served, and the search
    // paths must stay in step with `module_candidates` — a name that resolves
    // through `require` but not through `cook.load_module` is exactly the
    // register/execute split this module exists to prevent.
    assert!(!path.contains("/tmp/fake-project/.cook/modules/?.lua"), "{path}");
    assert!(!path.contains("/tmp/fake-project/.cook/modules/?/init.lua"), "{path}");
    assert!(!path.contains("cook_modules"), "the old tree root must not be searched: {path}");
    assert!(!cpath.contains("/tmp/fake-project/.cook/modules/?."), "{cpath}");
    assert!(!cpath.contains("cook_modules"), "the old tree root must not be searched: {cpath}");
}

#[test]
fn refresh_is_idempotent() {
    use crate::module_loader::refresh_package_search_paths;
    let lua = Lua::new();
    let cwd = PathBuf::from("/tmp/fake-project");
    refresh_package_search_paths(&lua, &cwd).expect("first");
    let pkg: LuaTable = lua.globals().get("package").unwrap();
    let first_path: String = pkg.get("path").unwrap();
    let first_cpath: String = pkg.get("cpath").unwrap();
    refresh_package_search_paths(&lua, &cwd).expect("second");
    let second_path: String = pkg.get("path").unwrap();
    let second_cpath: String = pkg.get("cpath").unwrap();
    assert_eq!(first_path, second_path, "path must not grow on repeated refresh");
    assert_eq!(first_cpath, second_cpath, "cpath must not grow on repeated refresh");
}

// ---------------------------------------------------------------------------
// The door agreement (CS-0205)
// ---------------------------------------------------------------------------
//
// `cook-luagen` composes the text of a `use` binding; this crate installs the
// function that text calls. Neither crate can see the other, and the only
// consequence of a rename is that the alias starts binding through a door the
// CS-0204 observer does not watch — a silently unkeyed module, which is a wrong
// shared-cache answer rather than a failure. These tests execute the composed
// text against the installed door, so changing one side alone goes red.

#[test]
fn the_composed_use_binding_executes_against_the_installed_loader() {
    let tmp = tempfile::tempdir().unwrap();
    write_module(tmp.path(), "greet", "return { value = function() return \"bound\" end }");
    let lua = vm_with_loader(tmp.path().to_path_buf());

    // Byte-for-byte what codegen puts at the top of a register chunk and in
    // front of every execute-phase body that names the alias.
    let chunk = format!(
        "{}\nreturn greet.value()",
        cook_contracts::module_binding::binding("greet", "greet")
    );
    let got: String = lua.load(&chunk).eval().unwrap();
    assert_eq!(got, "bound");
}

#[test]
fn a_hyphenated_module_binds_its_underscore_alias_through_the_same_door() {
    let tmp = tempfile::tempdir().unwrap();
    write_module(tmp.path(), "my-mod", "return { value = 7 }");
    let lua = vm_with_loader(tmp.path().to_path_buf());

    // §12.1 both ways at once: the composed alias must be a legal Lua local
    // AND the composed lookup must be the un-rewritten disk name.
    let chunk = format!(
        "{}\nreturn my_mod.value",
        cook_contracts::module_binding::binding(&cook_contracts::module_binding::alias_of("my-mod"), "my-mod")
    );
    let got: i64 = lua.load(&chunk).eval().unwrap();
    assert_eq!(got, 7);
}

#[test]
fn the_binding_goes_through_the_observed_door() {
    // The CS-0204 point, stated as a test: a unit whose alias was bound by the
    // composed prelude must end up keyed on the module file. If the binding
    // ever stops routing through `cook.load_module`, this records nothing.
    let tmp = tempfile::tempdir().unwrap();
    write_module(tmp.path(), "greet", "return { value = 1 }");
    let observer = ModuleObserver::new();
    let lua = Lua::new();
    let cook = lua.create_table().unwrap();
    install_module_loader(
        &lua,
        &cook,
        WorkingDirSource::Static(tmp.path().to_path_buf()),
        NoHooks,
        observer.clone(),
    )
    .unwrap();
    lua.globals().set("cook", cook).unwrap();

    lua.load(&format!(
        "{}\nreturn greet.value",
        cook_contracts::module_binding::binding("greet", "greet")
    ))
    .eval::<i64>()
    .unwrap();

    let seen = observer.take();
    assert_eq!(seen.len(), 1, "the composed binding must be observed: {seen:?}");
    assert!(seen[0].ends_with("greet.lua"), "{seen:?}");
}

// ---------------------------------------------------------------------------
// CS-0206: a path target resolves, keys, and is refused at THIS door
// ---------------------------------------------------------------------------
//
// §12.2.1 puts the containment rule on `cook.load_module`, not only on the
// `use` surface, because this function takes a string a body computed. A rule
// checked only where the parser can see it constrains spellings rather than
// loads.

fn write_file(dir: &std::path::Path, rel: &str, source: &str) {
    let path = dir.join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, source).unwrap();
}

#[test]
fn a_path_target_resolves_against_the_working_directory() {
    let tmp = tempfile::tempdir().unwrap();
    write_file(tmp.path(), "lua/helpers.lua", "return { value = 42 }");
    let lua = vm_with_loader(tmp.path().to_path_buf());

    let got: i64 = lua
        .load(r#"return cook.load_module("lua/helpers.lua").value"#)
        .eval()
        .unwrap();
    assert_eq!(got, 42);
}

#[test]
fn two_spellings_of_one_path_are_one_module_instance() {
    // §12.3.2. Keying the raw string would run the file's top-level chunk
    // twice and hand out two tables with independent state.
    let tmp = tempfile::tempdir().unwrap();
    write_file(
        tmp.path(),
        "lua/counter.lua",
        "counted = (counted or 0) + 1\nreturn { n = counted }",
    );
    let lua = vm_with_loader(tmp.path().to_path_buf());

    let (same, evals): (bool, i64) = lua
        .load(
            r#"
            local a = cook.load_module("./lua/counter.lua")
            local b = cook.load_module("lua/counter.lua")
            local c = cook.load_module("lua/./counter.lua")
            return a == b and b == c, counted
        "#,
        )
        .eval()
        .unwrap();
    assert!(same, "three spellings of one file must be one table");
    assert_eq!(evals, 1, "the top-level chunk runs once");
}

#[test]
fn a_path_target_is_recorded_as_a_determinant() {
    // The whole reason §24.2 forbids the path form a resolver of its own: it
    // goes through the door CS-0204 observes, or it is run without being keyed.
    let tmp = tempfile::tempdir().unwrap();
    write_file(tmp.path(), "lua/helpers.lua", "return { value = 1 }");
    let observer = ModuleObserver::new();
    let lua = Lua::new();
    let cook = lua.create_table().unwrap();
    install_module_loader(
        &lua,
        &cook,
        WorkingDirSource::Static(tmp.path().to_path_buf()),
        NoHooks,
        observer.clone(),
    )
    .unwrap();
    lua.globals().set("cook", cook).unwrap();

    lua.load(&format!(
        "{}\nreturn helpers.value",
        cook_contracts::module_binding::binding("helpers", "lua/helpers.lua")
    ))
    .eval::<i64>()
    .unwrap();

    let seen = observer.take();
    assert_eq!(seen.len(), 1, "{seen:?}");
    assert!(seen[0].ends_with("lua/helpers.lua"), "{seen:?}");
    // Recorded under the JOINED path, so CS-0204's working-directory prefix
    // still strips. A canonicalised path can carry a different prefix
    // (`/tmp` -> `/private/tmp`) that does not.
    assert!(
        cook_contracts::layout::relative_module_paths(tmp.path(), &seen)
            == vec!["lua/helpers.lua".to_string()],
        "path-loaded module dropped out of the recorded determinant set: {seen:?}"
    );
}

#[test]
fn the_door_refuses_an_escaping_path_a_parser_never_saw() {
    let tmp = tempfile::tempdir().unwrap();
    write_file(tmp.path(), "lua/helpers.lua", "return {}");
    let lua = vm_with_loader(tmp.path().to_path_buf());

    for target in ["../outside.lua", "//lua/helpers.lua", "/opt/x.lua"] {
        let err = lua
            .load(&format!(r#"return cook.load_module("{target}")"#))
            .eval::<LuaValue>()
            .expect_err(&format!("`{target}` must be refused at the door"));
        let msg = err.to_string();
        assert!(msg.contains(target), "{target}: {msg}");
        assert!(
            msg.contains("§12.2.1") || msg.contains("absolute paths are not permitted"),
            "{target}: diagnostic does not name the rule: {msg}"
        );
    }
}

#[cfg(unix)]
#[test]
fn a_symlink_out_of_the_tree_is_refused_and_the_reason_is_the_cache() {
    // The one containment check a pure path rule cannot make.
    let tmp = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    std::fs::write(outside.path().join("escaped.lua"), "return { value = 9 }").unwrap();
    std::fs::create_dir_all(tmp.path().join("lua")).unwrap();
    std::os::unix::fs::symlink(
        outside.path().join("escaped.lua"),
        tmp.path().join("lua/link.lua"),
    )
    .unwrap();
    let lua = vm_with_loader(tmp.path().to_path_buf());

    let err = lua
        .load(r#"return cook.load_module("lua/link.lua")"#)
        .eval::<LuaValue>()
        .expect_err("a symlink out of the tree must be refused");
    let msg = err.to_string();
    assert!(msg.contains("lua/link.lua"), "{msg}");
    assert!(
        msg.contains("determinant"),
        "the diagnostic must say WHY, not just that it refused: {msg}"
    );
}

#[test]
fn a_missing_path_reports_the_file_not_a_candidate_list() {
    let tmp = tempfile::tempdir().unwrap();
    let lua = vm_with_loader(tmp.path().to_path_buf());

    let err = lua
        .load(r#"return cook.load_module("lua/absent.lua")"#)
        .eval::<LuaValue>()
        .expect_err("must fail");
    let msg = err.to_string();
    assert!(msg.contains("lua/absent.lua"), "{msg}");
    // §12.5: a path-addressed module has no candidate list. Reporting one
    // would invite the author to go looking in directories nothing searched.
    assert!(!msg.contains("tried"), "{msg}");
    assert!(!msg.contains("init.lua"), "{msg}");
}

#[test]
fn a_path_module_gets_init_and_cycle_detection_like_any_other() {
    let tmp = tempfile::tempdir().unwrap();
    write_file(
        tmp.path(),
        "lua/inited.lua",
        "local m = { ran = false }\nfunction m.init() m.ran = true end\nreturn m",
    );
    write_file(
        tmp.path(),
        "lua/cyclic.lua",
        r#"cook.load_module("lua/cyclic.lua")
return {}"#,
    );
    let lua = vm_with_loader(tmp.path().to_path_buf());

    let ran: bool = lua
        .load(r#"return cook.load_module("lua/inited.lua").ran"#)
        .eval()
        .unwrap();
    assert!(ran, "§12.3.1: init() runs for a path-addressed module too");

    let err = lua
        .load(r#"return cook.load_module("lua/cyclic.lua")"#)
        .eval::<LuaValue>()
        .expect_err("a self-referential path module must be detected");
    assert!(
        err.to_string().contains("module cycle detected:"),
        "{err}"
    );
}
