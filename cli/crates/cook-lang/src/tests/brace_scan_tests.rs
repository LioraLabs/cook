use super::*;

// ── LuaScanner ─────────────────────────────────────────────────────

#[test]
fn lua_simple_braces() {
    let mut s = LuaScanner::new();
    assert_eq!(s.scan_line("local t = {1, 2}"), 0);
    assert_eq!(s.scan_line("local u = {"), 1);
    assert_eq!(s.scan_line("}"), -1);
}

#[test]
fn lua_string_braces_ignored() {
    let mut s = LuaScanner::new();
    assert_eq!(s.scan_line("local s = \"} not closing\""), 0);
    assert_eq!(s.scan_line("local s = '} also not closing'"), 0);
}

#[test]
fn lua_line_comment_braces_ignored() {
    let mut s = LuaScanner::new();
    assert_eq!(s.scan_line("local x = 1 -- } commentary"), 0);
}

#[test]
fn lua_multiline_long_string_carries_state() {
    let mut s = LuaScanner::new();
    // Open long string, level 0
    assert_eq!(s.scan_line("local s = [["), 0);
    // Inside long string — `}` is data
    assert_eq!(s.scan_line("} not a closer }"), 0);
    assert_eq!(s.scan_line("more } data"), 0);
    // Close long string; the `local x = {` after counts.
    assert_eq!(s.scan_line("]] local x = {"), 1);
    assert_eq!(s.scan_line("}"), -1);
}

#[test]
fn lua_multiline_long_string_with_levels() {
    let mut s = LuaScanner::new();
    assert_eq!(s.scan_line("local s = [==["), 0);
    // ]] does not match level-2 open
    assert_eq!(s.scan_line("]] still inside }"), 0);
    assert_eq!(s.scan_line("]==] -- closed"), 0);
}

#[test]
fn lua_multiline_block_comment_carries_state() {
    let mut s = LuaScanner::new();
    assert_eq!(s.scan_line("--[["), 0);
    assert_eq!(s.scan_line("} brace inside block comment"), 0);
    assert_eq!(s.scan_line("more } data"), 0);
    assert_eq!(s.scan_line("]] local t = {"), 1);
    assert_eq!(s.scan_line("}"), -1);
}

#[test]
fn lua_block_comment_with_levels() {
    let mut s = LuaScanner::new();
    assert_eq!(s.scan_line("--[==["), 0);
    assert_eq!(s.scan_line("]] not closing }"), 0);
    assert_eq!(s.scan_line("]==]"), 0);
}

// ── ShellScanner ───────────────────────────────────────────────────

#[test]
fn shell_simple_braces() {
    let mut s = ShellScanner::new();
    assert_eq!(s.scan_line("echo {hello}"), 0);
    assert_eq!(s.scan_line("if true; then {"), 1);
    assert_eq!(s.scan_line("}"), -1);
}

#[test]
fn shell_quoted_braces_ignored() {
    let mut s = ShellScanner::new();
    assert_eq!(s.scan_line("echo '{ literal }'"), 0);
    assert_eq!(s.scan_line("echo \"{ also literal }\""), 0);
}

#[test]
fn shell_heredoc_carries_state() {
    let mut s = ShellScanner::new();
    assert_eq!(s.scan_line("cat <<EOF"), 0);
    // Inside heredoc — `}` is data
    assert_eq!(s.scan_line("} not a closer"), 0);
        assert_eq!(s.scan_line("more } data"), 0);
    assert!(s.has_pending_heredoc());
    assert_eq!(s.scan_line("EOF"), 0);
    assert!(!s.has_pending_heredoc());
    assert_eq!(s.scan_line("echo done }"), -1);
}

#[test]
fn shell_heredoc_dash_form() {
    // Trim-first matching closes on any whitespace-prefixed delimiter
    // line; this matches Cookfile shell-block runtime semantics where
    // each interior line is trimmed before being sent to the shell.
    let mut s = ShellScanner::new();
    assert_eq!(s.scan_line("cat <<-EOF"), 0);
    assert_eq!(s.scan_line("\t} content"), 0);
    assert!(s.has_pending_heredoc());
    assert_eq!(s.scan_line("\tEOF"), 0);
    assert!(!s.has_pending_heredoc());
}

#[test]
fn shell_heredoc_quoted_delim() {
    let mut s = ShellScanner::new();
    assert_eq!(s.scan_line("cat <<'END'"), 0);
    assert_eq!(s.scan_line("} stays literal"), 0);
    assert_eq!(s.scan_line("END"), 0);
    assert!(!s.has_pending_heredoc());
}

#[test]
fn shell_heredoc_does_not_close_inside_outer() {
    // Regression: a brace on the same line as the heredoc opener still
    // counts; only the heredoc body is opaque.
    let mut s = ShellScanner::new();
    assert_eq!(s.scan_line("{ cat <<EOF"), 1);
    assert_eq!(s.scan_line("}"), 0);  // inside heredoc, ignored
    assert_eq!(s.scan_line("EOF"), 0);
    assert_eq!(s.scan_line("}"), -1);
}

#[test]
fn shell_backslash_escapes_brace() {
    let mut s = ShellScanner::new();
    assert_eq!(s.scan_line("echo \\}"), 0);
}

#[test]
fn shell_multiple_heredocs_on_one_line() {
    let mut s = ShellScanner::new();
    assert_eq!(s.scan_line("cat <<A <<B"), 0);
    assert_eq!(s.scan_line("body of A } no close"), 0);
    assert_eq!(s.scan_line("A"), 0);
    // Now consuming B
    assert_eq!(s.scan_line("body of B }"), 0);
    assert_eq!(s.scan_line("B"), 0);
    assert!(!s.has_pending_heredoc());
}
