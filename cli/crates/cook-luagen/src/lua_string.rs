pub(crate) fn escape_lua_string(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
}

/// Pick a long-bracket level high enough to safely wrap `s`.
///
/// A Lua long-bracket wrapper is `[<=>{n}[ ... ]<=>{n}]` for some n >= 0,
/// where `<=>{n}` denotes a run of exactly `n` equals signs. The closing
/// bracket only matches an opener of the same level, so the wrapper is safe
/// iff `s` contains no substring of the form `]<=>{n}]` (an inner closing
/// bracket of the same level).
///
/// Algorithm: scan `s` for every substring matching `]=*]` and record the
/// length of the equals run. The chosen level is `max(run) + 1`, which
/// guarantees no inner close can match. If `s` contains no `]…]` pattern
/// at all we use level 0 (`[[ … ]]`).
///
/// We accept overlapping matches (e.g. `]]]` contains both `]]` at offset 0
/// and `]]` at offset 1, both length-0 runs) by always continuing the scan
/// from the position after the opening `]`, not after the closing one.
fn pick_long_bracket_level(s: &str) -> usize {
    let bytes = s.as_bytes();
    let mut max_run: Option<usize> = None;
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b']' {
            let mut j = i + 1;
            while j < bytes.len() && bytes[j] == b'=' {
                j += 1;
            }
            if j < bytes.len() && bytes[j] == b']' {
                let run = j - i - 1;
                max_run = Some(max_run.map_or(run, |m| m.max(run)));
            }
        }
        i += 1;
    }
    max_run.map_or(0, |m| m + 1)
}

/// Wrap `s` in a Lua long-string literal, picking the smallest safe level.
pub(crate) fn wrap_lua_string(s: &str) -> String {
    let level = pick_long_bracket_level(s);
    let eq = "=".repeat(level);
    format!("[{eq}[{s}]{eq}]")
}

/// Wrap `code` in a Lua long-string literal suitable for embedding a Lua
/// chunk (plate/test bodies). Adds the surrounding newlines that Lua eats
/// after `[…[` so the body's first line is preserved verbatim, then picks
/// a long-bracket level high enough to contain any `]=*]` runs in `code`.
pub(crate) fn lua_chunk_literal(code: &str) -> String {
    let level = pick_long_bracket_level(code);
    let eq = "=".repeat(level);
    format!("[{eq}[\n{code}\n]{eq}]")
}

#[cfg(test)]
mod tests {
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
}
