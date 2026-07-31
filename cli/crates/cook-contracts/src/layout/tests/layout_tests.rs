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
