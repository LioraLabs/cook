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

// COOK-489: the closing bracket sits immediately after the content, so a
// trailing `]` in the content pairs with it and closes the literal early.
#[test]
fn wrap_string_ending_in_close_bracket_escalates() {
    // `[[` + `[ -f x ]` + `]]` is `[[[ -f x ]]]`, which Lua closes at the
    // content's own `]`, leaving a stray `]` behind.
    assert_eq!(wrap_lua_string("[ -f x ]"), "[=[[ -f x ]]=]");
}

// COOK-489: the same boundary hazard one level up — a trailing `]=` pairs
// with a level-1 closer chosen from an interior `]]`.
#[test]
fn wrap_string_ending_in_close_run_escalates_past_the_run() {
    assert_eq!(wrap_lua_string("a ]] b ]="), "[==[a ]] b ]=]==]");
}

// COOK-489, the silent half: content that ALREADY wrapped validly, whose level
// the boundary sentinel nonetheless raises. Unlike the two cases above, these
// never produced a Lua syntax error — so a regression here reports nothing. The
// literal stays well-formed and decodes to a DIFFERENT string: a wrong shell
// command that no error names, and that the cache is happy to reuse. Pinning
// the literal text is how that value gets asserted at all; this crate emits Lua
// and never evaluates it.
#[test]
fn wrap_string_ending_in_close_run_equal_to_level_escalates() {
    // k == old level (both 0). `[[foo]=]]` was valid and decoded correctly;
    // the level rises to 2 and the decoded value must not move.
    assert_eq!(wrap_lua_string("foo]="), "[==[foo]=]==]");
}

#[test]
fn wrap_string_ending_in_close_run_above_level_escalates() {
    // k (2) > old level (0) — the other side of the boundary.
    assert_eq!(wrap_lua_string("echo a]=="), "[===[echo a]==]===]");
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
