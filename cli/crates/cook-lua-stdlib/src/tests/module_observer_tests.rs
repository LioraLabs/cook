//! What the sink sees, through both doors (CS-0204).

use std::path::PathBuf;

use mlua::prelude::*;

use crate::module_loader::{install_module_loader, NoHooks};
use crate::module_observer::{install_require_observer, ModuleObserver};
use crate::WorkingDirSource;

fn write_module(dir: &std::path::Path, rel: &str, source: &str) -> PathBuf {
    let path = dir.join("cook_modules").join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, source).unwrap();
    path
}

fn vm(working_dir: PathBuf, observer: &ModuleObserver) -> Lua {
    let lua = Lua::new();
    let cook = lua.create_table().unwrap();
    install_module_loader(
        &lua,
        &cook,
        WorkingDirSource::Static(working_dir.clone()),
        NoHooks,
        observer.clone(),
    )
    .unwrap();
    lua.globals().set("cook", cook).unwrap();
    crate::module_loader::refresh_package_search_paths(&lua, &working_dir).unwrap();
    install_require_observer(&lua, observer.clone()).unwrap();
    lua
}

#[test]
fn a_load_records_the_resolved_path() {
    let tmp = tempfile::tempdir().unwrap();
    let expected = write_module(tmp.path(), "helper.lua", "return { v = 1 }");
    let observer = ModuleObserver::new();
    let lua = vm(tmp.path().to_path_buf(), &observer);

    lua.load(r#"cook.load_module("helper")"#).exec().unwrap();

    assert_eq!(observer.take(), vec![expected]);
}

/// The memo (§12.3.2) suppresses re-evaluation, not the dependency. One
/// worker VM serves many units; the second unit to ask for a module gets the
/// memoized table and must still be keyed on the file that produced it.
#[test]
fn a_memo_hit_still_records() {
    let tmp = tempfile::tempdir().unwrap();
    let expected = write_module(tmp.path(), "helper.lua", "return { v = 1 }");
    let observer = ModuleObserver::new();
    let lua = vm(tmp.path().to_path_buf(), &observer);

    lua.load(r#"cook.load_module("helper")"#).exec().unwrap();
    observer.take();
    lua.load(r#"cook.load_module("helper")"#).exec().unwrap();

    assert_eq!(observer.take(), vec![expected]);
}

/// §7 resolution order: hand-vendored beats LuaRocks-installed. The recorded
/// path must be the candidate that WON, not the first one probed.
#[test]
fn records_the_winning_candidate_not_the_first_probed() {
    let tmp = tempfile::tempdir().unwrap();
    let installed = write_module(tmp.path(), "share/lua/5.4/dual.lua", "return { v = 'rock' }");
    let observer = ModuleObserver::new();
    let lua = vm(tmp.path().to_path_buf(), &observer);

    lua.load(r#"cook.load_module("dual")"#).exec().unwrap();

    assert_eq!(observer.take(), vec![installed]);
}

#[test]
fn a_failed_load_records_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    let observer = ModuleObserver::new();
    let lua = vm(tmp.path().to_path_buf(), &observer);

    assert!(lua.load(r#"cook.load_module("absent")"#).exec().is_err());

    assert!(observer.take().is_empty());
}

/// The other door. A multi-file rock's internal `require` is how most module
/// source actually reaches a body, and it never passes through
/// `cook.load_module`.
#[test]
fn a_sub_require_records_its_own_file() {
    let tmp = tempfile::tempdir().unwrap();
    let init = write_module(
        tmp.path(),
        "rock/init.lua",
        r#"local sub = require("rock.sub"); return { v = sub.v }"#,
    );
    let sub = write_module(tmp.path(), "rock/sub.lua", "return { v = 7 }");
    let observer = ModuleObserver::new();
    let lua = vm(tmp.path().to_path_buf(), &observer);

    lua.load(r#"assert(cook.load_module("rock").v == 7)"#).exec().unwrap();

    let mut seen = observer.take();
    seen.sort();
    let mut want = vec![init, sub];
    want.sort();
    assert_eq!(seen, want);
}

/// A stdlib name has no file whose content could move, so it contributes
/// nothing to a key.
#[test]
fn requiring_a_stdlib_name_records_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    let observer = ModuleObserver::new();
    let lua = vm(tmp.path().to_path_buf(), &observer);

    lua.load(r#"require("string")"#).exec().unwrap();

    assert!(observer.take().is_empty());
}

/// The VM outlives the work item. A set left standing would key the next unit
/// on a module it never loaded.
#[test]
fn take_drains_and_clear_discards() {
    let tmp = tempfile::tempdir().unwrap();
    write_module(tmp.path(), "helper.lua", "return {}");
    let observer = ModuleObserver::new();
    let lua = vm(tmp.path().to_path_buf(), &observer);

    lua.load(r#"cook.load_module("helper")"#).exec().unwrap();
    assert_eq!(observer.take().len(), 1);
    assert!(observer.take().is_empty());

    lua.load(r#"cook.load_module("helper")"#).exec().unwrap();
    observer.clear();
    assert!(observer.take().is_empty());
}

/// Installing twice must not nest wrappers on a long-lived worker VM.
#[test]
fn require_observer_install_is_idempotent() {
    let tmp = tempfile::tempdir().unwrap();
    let sub = write_module(tmp.path(), "solo.lua", "return { v = 1 }");
    let observer = ModuleObserver::new();
    let lua = vm(tmp.path().to_path_buf(), &observer);
    install_require_observer(&lua, observer.clone()).unwrap();
    install_require_observer(&lua, observer.clone()).unwrap();

    lua.load(r#"require("solo")"#).exec().unwrap();

    assert_eq!(observer.take(), vec![sub]);
}
