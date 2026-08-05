use super::*;

#[test]
fn candidate_order_is_hand_vendored_first() {
    let wd = Path::new("/proj");
    let c = module_candidates(wd, "cook_cc");
    assert_eq!(c[0], Path::new("/proj/cook_modules/cook_cc.lua"));
    assert_eq!(c[1], Path::new("/proj/cook_modules/cook_cc/init.lua"));
    assert_eq!(c[2], Path::new("/proj/cook_modules/share/lua/5.4/cook_cc.lua"));
    assert_eq!(c[3], Path::new("/proj/cook_modules/share/lua/5.4/cook_cc/init.lua"));
}

#[test]
fn search_paths_compose_all_roots_and_keep_original_suffix() {
    let p = compose_lua_search_paths(Path::new("/proj"), "ORIG_PATH", "ORIG_CPATH");
    let ext = native_lua_ext();
    assert_eq!(
        p.path,
        "/proj/cook_modules/?.lua;/proj/cook_modules/?/init.lua;\
         /proj/cook_modules/share/lua/5.4/?.lua;/proj/cook_modules/share/lua/5.4/?/init.lua;\
         ORIG_PATH"
    );
    assert_eq!(
        p.cpath,
        format!("/proj/cook_modules/?.{ext};/proj/cook_modules/lib/lua/5.4/?.{ext};ORIG_CPATH")
    );
}

#[test]
fn index_basename_round_trips() {
    for name in ["build", "@cap/env:build", "50%/done", "a%2Fb"] {
        assert_eq!(decode_index_basename(&encode_index_basename(name)), name);
    }
    assert_eq!(encode_index_basename("@cap/env:build"), "@cap%2Fenv:build");
}

#[test]
fn dot_cook_tree() {
    let b = Path::new("/p");
    assert_eq!(cache_dir(b), Path::new("/p/.cook/cache"));
    assert_eq!(probes_dir(b), Path::new("/p/.cook/probes"));
    assert_eq!(logs_dir(b), Path::new("/p/.cook/logs"));
}

// ---------------------------------------------------------------------------
// Module-load laws
// ---------------------------------------------------------------------------

mod module_load_laws {
    use crate::layout::*;
    use std::path::Path;

    #[test]
    fn memo_key_is_cwd_and_name() {
        assert_eq!(
            module_memo_key(Path::new("/proj/sub"), "cook_cc"),
            "/proj/sub::cook_cc"
        );
    }

    /// §12.3.3: prefix, ` -> ` joining, re-entered name appended.
    #[test]
    fn cycle_message_renders_the_path() {
        let stack = vec!["a".to_string(), "b".to_string()];
        assert_eq!(
            module_cycle_message(&stack, "a"),
            "module cycle detected: a -> b -> a"
        );
    }

    #[test]
    fn cycle_message_self_cycle() {
        assert_eq!(
            module_cycle_message(&["solo".to_string()], "solo"),
            "module cycle detected: solo -> solo"
        );
    }

    /// §24.2: the diagnostic identifies the name and the searched paths.
    #[test]
    fn not_found_message_names_module_and_paths() {
        let msg = module_not_found_message(Path::new("/proj"), "foo");
        assert_eq!(
            msg,
            "cook.load_module: module 'foo' not found under /proj/cook_modules \
             (tried foo.lua, foo/init.lua, share/lua/5.4/foo.lua, \
             share/lua/5.4/foo/init.lua)"
        );
    }

    #[test]
    fn read_failed_message_names_module_path_and_cause() {
        let msg =
            module_read_failed_message("foo", Path::new("/p/foo.lua"), "permission denied");
        assert_eq!(
            msg,
            "cook.load_module: failed to read module 'foo' at /p/foo.lua: permission denied"
        );
    }

    #[test]
    fn chunk_name_is_at_path() {
        assert_eq!(
            module_chunk_name(Path::new("/p/cook_modules/foo.lua")),
            "@/p/cook_modules/foo.lua"
        );
    }
}
