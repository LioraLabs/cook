//! Stateful brace-balance scanners for Lua and shell block bodies.
//!
//! Both `>{ … }` Lua blocks and `{ … }` shell blocks are collected by walking
//! the source line-by-line and counting `{`/`}` braces. The naïve form of this
//! algorithm — count braces per line, line-locally — is incorrect when the
//! interior language contains constructs that span multiple source lines and
//! whose interior must be treated as opaque to the brace counter. This module
//! provides two stateful scanners that carry that interior-context state
//! across the lines of a block body:
//!
//! * [`LuaScanner`] tracks Lua **long strings** (`[==[ … ]==]`) and Lua
//!   **block comments** (`--[==[ … ]==]`), each at any `=`-level per the
//!   Lua 5.4 reference manual.
//! * [`ShellScanner`] tracks POSIX **heredocs** (`<<TAG`, `<<-TAG`, with
//!   quoted variants `<<'TAG'`, `<<"TAG"`).
//!
//! When the scanner is inside one of these spans, `{`/`}` characters are
//! ignored (they are data, not brace-block delimiters). The opening marker
//! still counts braces up to the marker's start; the closing marker resumes
//! brace counting on the line after the close.
//!
//! The two scanners are intentionally separate: Lua state is meaningless to
//! shell text and vice versa. Module-call collection (`recipe::collect_module_call`)
//! uses [`LuaScanner`] because module calls are syntactically Lua expressions.
//!
//! See standard § 2.9 (Brace-balanced blocks).

/// Stateful brace-balance scanner for Lua block bodies.
///
/// Carries state across lines so that multi-line Lua long strings (`[[ … ]]`,
/// `[==[ … ]==]`) and multi-line block comments (`--[[ … ]]`,
/// `--[==[ … ]==]`) do not confuse the brace counter. A `}` byte appearing
/// inside such a span is data, not a block-closing delimiter.
#[derive(Debug, Default, Clone)]
pub(crate) struct LuaScanner {
    /// When `Some(level)`, the scanner is inside an open long bracket
    /// (long string or block comment) of the given `=`-level. The closing
    /// bracket must have the same level.
    in_long_bracket: Option<u32>,
}

impl LuaScanner {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Scan a single source line, returning the net brace delta contributed
    /// by characters NOT inside a string, comment, or open long bracket.
    /// Updates internal state so that subsequent calls correctly handle a
    /// long bracket or block comment that was opened on a previous line and
    /// is closed on a later one.
    pub(crate) fn scan_line(&mut self, line: &str) -> i32 {
        let chars: Vec<char> = line.chars().collect();
        let len = chars.len();
        let mut i = 0;
        let mut delta: i32 = 0;

        while i < len {
            // If we are inside an open long bracket, scan only for the
            // matching close bracket `]==…==]` of the same level. Braces in
            // between are data.
            if let Some(level) = self.in_long_bracket {
                if chars[i] == ']' {
                    if let Some(end) = match_close_long_bracket(&chars, i, level) {
                        self.in_long_bracket = None;
                        i = end;
                        continue;
                    }
                }
                i += 1;
                continue;
            }

            let c = chars[i];

            // Lua block comment `--[==[ … ]==]` or line comment `-- …`.
            if c == '-' && i + 1 < len && chars[i + 1] == '-' {
                // Could be `--[==[` block comment opener, or a line comment.
                if let Some((level, after)) = match_open_long_bracket(&chars, i + 2) {
                    // Block comment opener.
                    self.in_long_bracket = Some(level);
                    i = after;
                    continue;
                }
                // Line comment — rest of this line is opaque.
                break;
            }

            // Lua long string `[==[ … ]==]`.
            if c == '[' {
                if let Some((level, after)) = match_open_long_bracket(&chars, i) {
                    self.in_long_bracket = Some(level);
                    i = after;
                    continue;
                }
            }

            // Double-quoted string. Lua strings do not span newlines unless
            // continued with `\`; we do not model that subtlety because
            // mid-line unterminated strings are themselves a Lua syntax
            // error and out of scope for the Cookfile-layer brace scan.
            if c == '"' {
                i += 1;
                while i < len && chars[i] != '"' {
                    if chars[i] == '\\' && i + 1 < len {
                        i += 2;
                        continue;
                    }
                    i += 1;
                }
                if i < len {
                    i += 1; // closing quote
                }
                continue;
            }

            // Single-quoted string — same shape as double-quoted in Lua.
            if c == '\'' {
                i += 1;
                while i < len && chars[i] != '\'' {
                    if chars[i] == '\\' && i + 1 < len {
                        i += 2;
                        continue;
                    }
                    i += 1;
                }
                if i < len {
                    i += 1;
                }
                continue;
            }

            if c == '{' {
                delta += 1;
            } else if c == '}' {
                delta -= 1;
            }

            i += 1;
        }

        delta
    }
}

/// If `chars[i..]` begins with `[` then a run of `=` of length `level` then
/// another `[`, returns `Some((level, position-after-second-[))`.
/// Otherwise returns `None`.
fn match_open_long_bracket(chars: &[char], i: usize) -> Option<(u32, usize)> {
    if i >= chars.len() || chars[i] != '[' {
        return None;
    }
    let mut j = i + 1;
    let mut level: u32 = 0;
    while j < chars.len() && chars[j] == '=' {
        level += 1;
        j += 1;
    }
    if j < chars.len() && chars[j] == '[' {
        Some((level, j + 1))
    } else {
        None
    }
}

/// If `chars[i..]` begins with `]` then a run of `=` of length `level` then
/// another `]`, returns `Some(position-after-second-])`. Otherwise `None`.
fn match_close_long_bracket(chars: &[char], i: usize, level: u32) -> Option<usize> {
    if i >= chars.len() || chars[i] != ']' {
        return None;
    }
    let mut j = i + 1;
    let mut found: u32 = 0;
    while j < chars.len() && chars[j] == '=' {
        found += 1;
        j += 1;
    }
    if found == level && j < chars.len() && chars[j] == ']' {
        Some(j + 1)
    } else {
        None
    }
}

/// Stateful brace-balance scanner for shell block bodies.
///
/// Carries state across lines so that POSIX heredocs (`<<TAG`, `<<-TAG`,
/// with quoted variants `<<'TAG'` and `<<"TAG"`) do not confuse the brace
/// counter. While inside an open heredoc, `{`/`}` characters are data; the
/// heredoc closes when a line equal to the delimiter (with leading tabs
/// stripped if and only if `<<-` was used) is encountered.
///
/// Multiple heredocs may queue on the same source line (`cmd <<A <<B`); they
/// are consumed in order.
#[derive(Debug, Default, Clone)]
pub(crate) struct ShellScanner {
    /// FIFO queue of heredoc closers introduced on the current or earlier
    /// lines but not yet closed. Each entry is `(delimiter, allow_tab_indent)`.
    pending_heredocs: Vec<(String, bool)>,
}

impl ShellScanner {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Scan a single source line, returning the net brace delta contributed
    /// by characters NOT inside an open heredoc. Updates internal state so
    /// that a heredoc opened on this line (or a previous one) consumes
    /// subsequent lines until its delimiter is matched.
    ///
    /// The delimiter match is performed against the trimmed line. This
    /// matches Cookfile shell-block runtime semantics: `collect_shell_block`
    /// trims each line before passing the assembled script to the shell, so
    /// the closer sees only the trimmed text. Authors writing heredocs
    /// inside an indented `{ … }` body therefore do not need to dedent the
    /// closing delimiter to column 0 in the source. The `<<-` (tab-strip)
    /// form is still recognised; under trim-first matching the tab-strip
    /// distinction collapses, but we preserve the parsed flag for clarity.
    pub(crate) fn scan_line(&mut self, line: &str) -> i32 {
        // If we are inside any open heredoc, the entire line is heredoc
        // content unless it is the delimiter line.
        if !self.pending_heredocs.is_empty() {
            let (delim, _allow_tab) = self.pending_heredocs[0].clone();
            if line.trim() == delim {
                // This line closes the front-of-queue heredoc.
                self.pending_heredocs.remove(0);
            }
            return 0;
        }

        // Not inside a heredoc — scan the line normally, counting braces and
        // queueing any new heredoc openers.
        let bytes = line.as_bytes();
        let len = bytes.len();
        let mut i = 0;
        let mut delta: i32 = 0;

        while i < len {
            let b = bytes[i];

            // Shell line comment: `#` outside of quotes/brace-words.
            // We are not running a full shell tokenizer; for the brace-scan
            // purpose, a `#` after whitespace begins a comment. This matches
            // common heredoc-bearing recipes and avoids accidentally treating
            // `cmd #count: 3` as containing `}` — which it does not, but
            // safety first.
            if b == b'#' && (i == 0 || is_shell_word_break(bytes[i - 1])) {
                break;
            }

            // Single-quoted: opaque, no escapes.
            if b == b'\'' {
                i += 1;
                while i < len && bytes[i] != b'\'' {
                    i += 1;
                }
                if i < len {
                    i += 1;
                }
                continue;
            }

            // Double-quoted: `\` escapes a few characters.
            if b == b'"' {
                i += 1;
                while i < len && bytes[i] != b'"' {
                    if bytes[i] == b'\\' && i + 1 < len {
                        i += 2;
                        continue;
                    }
                    i += 1;
                }
                if i < len {
                    i += 1;
                }
                continue;
            }

            // Heredoc opener: `<<` or `<<-`, optionally followed by whitespace
            // and then a delimiter (bare, single-quoted, or double-quoted).
            if b == b'<' && i + 1 < len && bytes[i + 1] == b'<' {
                let mut j = i + 2;
                let allow_tab = if j < len && bytes[j] == b'-' {
                    j += 1;
                    true
                } else {
                    false
                };
                // Optional whitespace between `<<` and the delimiter.
                while j < len && (bytes[j] == b' ' || bytes[j] == b'\t') {
                    j += 1;
                }
                if let Some((delim, after)) = read_heredoc_delim(bytes, j) {
                    self.pending_heredocs.push((delim, allow_tab));
                    i = after;
                    continue;
                }
                // Not a heredoc opener (e.g. `<<<` here-string or `<<` with
                // no following delimiter). Fall through to normal handling.
            }

            // Backslash-escape next byte (covers `\{`, `\}`, etc.).
            if b == b'\\' && i + 1 < len {
                i += 2;
                continue;
            }

            if b == b'{' {
                delta += 1;
            } else if b == b'}' {
                delta -= 1;
            }

            i += 1;
        }

        delta
    }

    /// Returns true if any heredoc opened on or before the most recently
    /// scanned line is still pending a closing delimiter.
    #[cfg(test)]
    pub(crate) fn has_pending_heredoc(&self) -> bool {
        !self.pending_heredocs.is_empty()
    }
}

fn is_shell_word_break(b: u8) -> bool {
    matches!(b, b' ' | b'\t' | b';' | b'&' | b'|' | b'(' | b')')
}

/// Read a heredoc delimiter at `bytes[i..]`. The delimiter may be:
/// * bare:   one or more `[A-Za-z0-9_]` characters
/// * single: `'…'`
/// * double: `"…"`
///
/// Returns `Some((delimiter_text, position-after-delimiter))` or `None` if
/// no valid delimiter is present.
fn read_heredoc_delim(bytes: &[u8], i: usize) -> Option<(String, usize)> {
    if i >= bytes.len() {
        return None;
    }
    let b = bytes[i];
    if b == b'\'' || b == b'"' {
        let quote = b;
        let mut j = i + 1;
        let start = j;
        while j < bytes.len() && bytes[j] != quote {
            j += 1;
        }
        if j >= bytes.len() {
            return None; // unterminated
        }
        let delim = String::from_utf8_lossy(&bytes[start..j]).into_owned();
        Some((delim, j + 1))
    } else if is_bare_delim_char(b) {
        let mut j = i;
        while j < bytes.len() && is_bare_delim_char(bytes[j]) {
            j += 1;
        }
        let delim = String::from_utf8_lossy(&bytes[i..j]).into_owned();
        Some((delim, j))
    } else {
        None
    }
}

fn is_bare_delim_char(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

#[cfg(test)]
mod tests {
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
}
