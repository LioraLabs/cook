use super::*;

#[test]
fn wrap_plain_string_uses_level_zero() {
    assert_eq!(wrap_lua_string("hello"), "[[hello]]");
}

#[test]
fn wrap_string_with_double_close_escalates_to_level_one() {
    assert_eq!(wrap_lua_string("a ]] b"), "[=[a ]] b]=]");
}

#[test]
fn wrap_string_with_level_one_close_escalates_to_level_two() {
    // Reproduces the original bug: input `]=]` must NOT close `[=[ … ]=]`.
    let out = wrap_lua_string("a ]=] b");
    assert_eq!(out, "[==[a ]=] b]==]");
}

#[test]
fn wrap_string_with_level_three_close_escalates_to_level_four() {
    let out = wrap_lua_string("x ]===] y");
    assert_eq!(out, "[====[x ]===] y]====]");
}

#[test]
fn wrap_string_with_mixed_runs_picks_max_plus_one() {
    // Contains both `]]` (run 0) and `]==]` (run 2). Must use level 3.
    let out = wrap_lua_string("a ]] b ]==] c");
    assert_eq!(out, "[===[a ]] b ]==] c]===]");
}

#[test]
fn wrap_lone_close_brackets_do_not_escalate() {
    // `]` alone (not paired with another `]`) does not require escalation.
    assert_eq!(wrap_lua_string("a ] b"), "[[a ] b]]");
}

#[test]
fn wrap_three_consecutive_brackets_treated_as_run_zero() {
    // `]]]` contains a `]]` close at level 0; need level 1.
    let out = wrap_lua_string("]]]");
    assert_eq!(out, "[=[]]]]=]");
}

#[test]
fn lua_chunk_literal_wraps_with_newlines_and_escalates() {
    let out = lua_chunk_literal("local x = [==[ y ]==]");
    // Must escalate beyond `]==]` -> level 3.
    assert_eq!(out, "[===[\nlocal x = [==[ y ]==]\n]===]");
}

#[test]
fn lua_chunk_literal_plain_uses_level_zero() {
    assert_eq!(lua_chunk_literal("print(1)"), "[[\nprint(1)\n]]");
}
