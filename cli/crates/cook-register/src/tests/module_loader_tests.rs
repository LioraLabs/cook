use super::*;
use tempfile::TempDir;

/// Where an installed rock's pure-Lua files land, so `cook.load_module(name)`
/// resolves BY NAME. Through `cook_contracts::layout` so the next move of the
/// tree root does not have to touch this suite (CS-0207 moved it from
/// `cook_modules/` to `.cook/modules/`).
fn installed_share(dir: &std::path::Path) -> std::path::PathBuf {
    cook_contracts::layout::modules_dir(dir)
        .join(cook_contracts::layout::MODULES_SHARE_LUA_SUBDIR)
}

fn setup_with_module(
    module_name: &str,
    module_code: &str,
) -> (Lua, TempDir, SharedModuleLoaderState) {
    let dir = TempDir::new().unwrap();
    let modules_dir = installed_share(dir.path());
    std::fs::create_dir_all(&modules_dir).unwrap();
    std::fs::write(modules_dir.join(format!("{}.lua", module_name)), module_code).unwrap();

    let lua = Lua::new();
    let cook = lua.create_table().unwrap();
    lua.globals().set("cook", cook).unwrap();

    let state = Rc::new(RefCell::new(ModuleLoaderState::new(dir.path().to_path_buf())));
    register_module_loader(&lua, state.clone(), cook_lua_stdlib::ModuleObserver::new()).unwrap();
    register_cache_api(&lua, state.clone(), Rc::new(RefCell::new(BTreeMap::new()))).unwrap();
    (lua, dir, state)
}

#[test]
fn test_load_module_returns_table() {
    let (lua, _dir, _) =
        setup_with_module("test_mod", "local m = {} m.value = 42 return m");
    let result: i32 = lua
        .load(r#"local m = cook.load_module("test_mod") return m.value"#)
        .eval()
        .unwrap();
    assert_eq!(result, 42);
}

#[test]
fn test_load_module_calls_init() {
    let (lua, _dir, _) = setup_with_module(
        "test_mod",
        "local m = {} m.initialized = false function m.init() m.initialized = true end return m",
    );
    let result: bool = lua
        .load(r#"local m = cook.load_module("test_mod") return m.initialized"#)
        .eval()
        .unwrap();
    assert!(result);
}

#[test]
fn test_load_module_not_found() {
    let dir = TempDir::new().unwrap();
    let lua = Lua::new();
    lua.globals()
        .set("cook", lua.create_table().unwrap())
        .unwrap();
    let state = Rc::new(RefCell::new(ModuleLoaderState::new(dir.path().to_path_buf())));
    register_module_loader(&lua, state, cook_lua_stdlib::ModuleObserver::new()).unwrap();
    let result = lua.load(r#"cook.load_module("nonexistent")"#).exec();
    assert!(result.is_err());
}

#[test]
fn test_load_module_init_lua() {
    let dir = TempDir::new().unwrap();
    let modules_dir = installed_share(dir.path()).join("mymod");
    std::fs::create_dir_all(&modules_dir).unwrap();
    std::fs::write(
        modules_dir.join("init.lua"),
        "local m = {} m.from_init = true return m",
    )
    .unwrap();

    let lua = Lua::new();
    lua.globals()
        .set("cook", lua.create_table().unwrap())
        .unwrap();
    let state = Rc::new(RefCell::new(ModuleLoaderState::new(dir.path().to_path_buf())));
    register_module_loader(&lua, state, cook_lua_stdlib::ModuleObserver::new()).unwrap();
    let result: bool = lua
        .load(r#"local m = cook.load_module("mymod") return m.from_init"#)
        .eval()
        .unwrap();
    assert!(result);
}

#[test]
fn test_load_module_memoized_returns_same_table() {
    // §6.3.4: a second cook.load_module(name) MUST return the same Lua
    // value without re-evaluating the module file. We verify by mutating
    // the table after first load and observing the mutation on the second
    // load (which would be reset if the file were re-evaluated).
    let (lua, _dir, _) = setup_with_module(
        "test_mod",
        "local m = {} m.value = 1 return m",
    );
    let result: i32 = lua
        .load(
            r#"local a = cook.load_module("test_mod")
                a.value = 99
                local b = cook.load_module("test_mod")
                return b.value"#,
        )
        .eval()
        .unwrap();
    assert_eq!(result, 99, "memoization must return the same table instance");
}

#[test]
fn test_load_module_init_runs_once_when_memoized() {
    // §6.3.4 corollary: if the module table is reused, init() must not
    // run again either. We track invocation count via a global counter.
    let (lua, _dir, _) = setup_with_module(
        "test_mod",
        r#"local m = {}
            function m.init()
                _G.init_calls = (_G.init_calls or 0) + 1
            end
            return m"#,
    );
    let calls: i32 = lua
        .load(
            r#"cook.load_module("test_mod")
                cook.load_module("test_mod")
                cook.load_module("test_mod")
                return _G.init_calls"#,
        )
        .eval()
        .unwrap();
    assert_eq!(calls, 1, "init must run exactly once across repeated loads");
}

#[test]
fn test_load_module_cycle_two_modules_raises() {
    // §6.3.4 cycle detection: a cycle a -> b -> a MUST raise a diagnostic
    // naming the cycle, not stack-overflow.
    let dir = TempDir::new().unwrap();
    let modules_dir = installed_share(dir.path());
    std::fs::create_dir_all(&modules_dir).unwrap();
    std::fs::write(
        modules_dir.join("a.lua"),
        r#"local m = {}
            cook.load_module("b")
            return m"#,
    )
    .unwrap();
    std::fs::write(
        modules_dir.join("b.lua"),
        r#"local m = {}
            cook.load_module("a")
            return m"#,
    )
    .unwrap();

    let lua = Lua::new();
    lua.globals()
        .set("cook", lua.create_table().unwrap())
        .unwrap();
    let state = Rc::new(RefCell::new(ModuleLoaderState::new(dir.path().to_path_buf())));
    register_module_loader(&lua, state, cook_lua_stdlib::ModuleObserver::new()).unwrap();

    let err = lua
        .load(r#"cook.load_module("a")"#)
        .exec()
        .expect_err("cycle must raise");
    let msg = format!("{}", err);
    assert!(
        msg.contains("module cycle detected"),
        "diagnostic must say `module cycle detected`, got: {}",
        msg
    );
    assert!(
        msg.contains("a -> b -> a"),
        "diagnostic must render the cycle path, got: {}",
        msg
    );
}

#[test]
fn test_load_module_self_cycle_raises() {
    // A module that loads itself must surface the same diagnostic.
    let dir = TempDir::new().unwrap();
    let modules_dir = installed_share(dir.path());
    std::fs::create_dir_all(&modules_dir).unwrap();
    std::fs::write(
        modules_dir.join("solo.lua"),
        r#"local m = {}
            cook.load_module("solo")
            return m"#,
    )
    .unwrap();

    let lua = Lua::new();
    lua.globals()
        .set("cook", lua.create_table().unwrap())
        .unwrap();
    let state = Rc::new(RefCell::new(ModuleLoaderState::new(dir.path().to_path_buf())));
    register_module_loader(&lua, state, cook_lua_stdlib::ModuleObserver::new()).unwrap();

    let err = lua
        .load(r#"cook.load_module("solo")"#)
        .exec()
        .expect_err("self-cycle must raise");
    let msg = format!("{}", err);
    assert!(msg.contains("solo -> solo"), "got: {}", msg);
}

#[test]
fn test_load_module_recovers_after_error() {
    // After a module load fails, the in-flight set must be cleaned up so
    // a subsequent retry can proceed (cycle detection survives recoverable
    // errors).
    let dir = TempDir::new().unwrap();
    let modules_dir = installed_share(dir.path());
    std::fs::create_dir_all(&modules_dir).unwrap();
    std::fs::write(
        modules_dir.join("boom.lua"),
        r#"error("intentional")"#,
    )
    .unwrap();

    let lua = Lua::new();
    lua.globals()
        .set("cook", lua.create_table().unwrap())
        .unwrap();
    let state = Rc::new(RefCell::new(ModuleLoaderState::new(dir.path().to_path_buf())));
    register_module_loader(&lua, state.clone(), cook_lua_stdlib::ModuleObserver::new()).unwrap();

    let _ = lua.load(r#"cook.load_module("boom")"#).exec();
    // After the failure the in-flight marker must be gone: a retry raises
    // the module's own error again, not a phantom cycle diagnostic.
    let msg = lua
        .load(r#"cook.load_module("boom")"#)
        .exec()
        .expect_err("still failing")
        .to_string();
    assert!(
        msg.contains("intentional") && !msg.contains("module cycle detected"),
        "retry must re-raise the real error, got: {msg}"
    );
    // And the failed load must not leave a dangling module context.
    assert!(state.borrow().current_module.is_none());
}

#[test]
fn test_cache_api_in_module() {
    let (lua, dir, state) = setup_with_module(
        "test_mod",
        r#"local m = {}
            function m.init()
                cook.probes.set("greeting", "hello")
            end
            function m.get_greeting()
                return cook.probes.get("greeting")
            end
            return m"#,
    );
    let result: String = lua
        .load(r#"local m = cook.load_module("test_mod") return m.get_greeting()"#)
        .eval()
        .unwrap();
    assert_eq!(result, "hello");
    state.borrow().flush_all();
    let cache_file = dir.path().join(".cook/cache/test_mod.json");
    assert!(cache_file.exists());
}

/// §24.4.3: the register VM carries `cook.probes.scope(label)` — a view
/// whose get/set are the prefixed `label:key` operations on the module
/// store. Until COOK-412 the register VM never installed it, so a module
/// using the scoped pattern died with a nil-index error at register phase
/// while working at execute phase.
#[test]
fn probes_scope_prefixes_get_and_set_on_the_module_store() {
    let (lua, _dir, _state) = setup_with_module(
        "test_mod",
        r#"local m = {}
            function m.init()
                local sc = cook.probes.scope("toolchain")
                sc.set("cc", "clang")
            end
            function m.read_scoped()
                return cook.probes.scope("toolchain").get("cc")
            end
            function m.read_full()
                return cook.probes.get("toolchain:cc")
            end
            return m"#,
    );
    let (scoped, full): (String, String) = lua
        .load(
            r#"local m = cook.load_module("test_mod")
                return m.read_scoped(), m.read_full()"#,
        )
        .eval()
        .unwrap();
    assert_eq!(scoped, "clang");
    assert_eq!(full, "clang", "scoped set stores under the full label:key");
}

/// §24.4.3: a label containing ':' MUST raise.
#[test]
fn probes_scope_refuses_a_colon_label() {
    let (lua, _dir, _state) = setup_with_module(
        "test_mod",
        r#"local m = {}
            function m.init()
                cook.probes.scope("a:b")
            end
            return m"#,
    );
    let msg = lua
        .load(r#"cook.load_module("test_mod")"#)
        .exec()
        .expect_err("colon label must raise")
        .to_string();
    assert!(
        msg.contains("must not contain ':'"),
        "names the rule: {msg}"
    );
}

#[test]
fn cook_cache_is_hard_error_with_did_you_mean() {
    let (lua, _dir, _state) = setup_with_module("test_mod", "return {}");
    let err = lua
        .load(r#"return cook.cache.get("x")"#)
        .exec()
        .expect_err("cook.cache.get must be a hard error");
        let msg = err.to_string();
        assert!(
            msg.contains("cook.cache' was renamed to 'cook.probes'")
            && msg.contains("cook.probes.get"),
        "rename diagnostic must name the new spelling; got: {msg}"
    );
}

#[test]
fn test_load_module_resolves_share_lua_flat() {
    let dir = TempDir::new().unwrap();
    let share_dir = installed_share(dir.path());
    std::fs::create_dir_all(&share_dir).unwrap();
    std::fs::write(
        share_dir.join("rockmod.lua"),
        "local m = {} m.tag = 'share-flat' return m",
    )
    .unwrap();

    let lua = Lua::new();
    lua.globals()
        .set("cook", lua.create_table().unwrap())
        .unwrap();
    let state = Rc::new(RefCell::new(ModuleLoaderState::new(dir.path().to_path_buf())));
    register_module_loader(&lua, state, cook_lua_stdlib::ModuleObserver::new()).unwrap();

    let tag: String = lua
        .load(r#"local m = cook.load_module("rockmod") return m.tag"#)
        .eval()
        .unwrap();
    assert_eq!(tag, "share-flat");
}

#[test]
fn test_load_module_resolves_share_lua_init() {
    let dir = TempDir::new().unwrap();
    let share_dir = installed_share(dir.path()).join("rockmod");
    std::fs::create_dir_all(&share_dir).unwrap();
    std::fs::write(
        share_dir.join("init.lua"),
        "local m = {} m.tag = 'share-init' return m",
    )
    .unwrap();

    let lua = Lua::new();
    lua.globals()
        .set("cook", lua.create_table().unwrap())
        .unwrap();
    let state = Rc::new(RefCell::new(ModuleLoaderState::new(dir.path().to_path_buf())));
    register_module_loader(&lua, state, cook_lua_stdlib::ModuleObserver::new()).unwrap();

    let tag: String = lua
        .load(r#"local m = cook.load_module("rockmod") return m.tag"#)
        .eval()
        .unwrap();
    assert_eq!(tag, "share-init");
}

/// CS-0207 withdrew the hand-vendored top level; this used to assert it WON.
/// Inverted rather than deleted, because the cut is only real if a decoy at
/// either retired location is invisible: shadowing by precedence left no
/// record in the Cookfile that it happened, so a reader saw `use rockmod` and
/// had to know the search order to learn which `rockmod` ran.
#[test]
fn test_retired_top_level_candidates_are_not_resolved() {
    let dir = TempDir::new().unwrap();
    let share_dir = installed_share(dir.path());
    std::fs::create_dir_all(&share_dir).unwrap();
    let tree_root = cook_contracts::layout::modules_dir(dir.path());
    let legacy_root = cook_contracts::layout::legacy_modules_dir(dir.path());
    std::fs::create_dir_all(&legacy_root).unwrap();

    // Decoy 1: the top level of the CURRENT tree root, which is not a candidate.
    std::fs::write(tree_root.join("rockmod.lua"), "return { tag = 'tree-top' }").unwrap();
    // Decoy 2: the pre-CS-0207 root, which is never searched at all.
    std::fs::write(legacy_root.join("rockmod.lua"), "return { tag = 'legacy' }").unwrap();
    // The only candidate.
    std::fs::write(share_dir.join("rockmod.lua"), "return { tag = 'share' }").unwrap();

    let lua = Lua::new();
    lua.globals()
        .set("cook", lua.create_table().unwrap())
        .unwrap();
    let state = Rc::new(RefCell::new(ModuleLoaderState::new(dir.path().to_path_buf())));
    register_module_loader(&lua, state, cook_lua_stdlib::ModuleObserver::new()).unwrap();

    let tag: String = lua
        .load(r#"local m = cook.load_module("rockmod") return m.tag"#)
        .eval()
        .unwrap();
    assert_eq!(tag, "share");
}
