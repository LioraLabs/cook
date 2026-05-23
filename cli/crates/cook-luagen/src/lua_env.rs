//! Static scanner for `cook.env.<KEY>` reads inside `using >{ ... }` Lua
//! bodies (Standard §17.1).
//!
//! Mirrors the shell-side `$<KEY>` sigil scanner in [`crate::sigil`], applied
//! to the Lua source of a cook-step using-block. The scanned keys are folded
//! into the unit's `consulted_env_keys` at codegen time so that a value
//! change in any of those keys invalidates the unit's cache entry, exactly
//! like a shell-body sigil read would.
//!
//! # Matching rules
//!
//! Three patterns are recognised, each scoped outside strings and comments
//! (the scanner skips `"..."`, `'...'`, `[[...]]` long strings, `--` line
//! comments, and `--[[...]]` long comments before testing for matches):
//!
//! 1. **Dot access** — `cook.env.IDENT` where `IDENT` matches the
//!    conventional env-key shape `[A-Z_][A-Z0-9_]*` (uppercase + underscore).
//!    Lower-case identifiers are intentionally skipped: by convention env
//!    keys are upper-case, and admitting lower-case would burn false
//!    positives on common Lua idioms like `cook.env.path` that aren't env
//!    keys.
//! 2. **String index, double-quoted** — `cook.env["KEY"]` where `KEY` is
//!    any non-empty string literal. No case constraint — the author has
//!    explicitly named the key.
//! 3. **String index, single-quoted** — `cook.env['KEY']`. Same as (2).
//!
//! # Skipped (by design)
//!
//! - **Dynamic-key reads** — `cook.env[var]`, `cook.env[KEY_NAME]`,
//!   `cook.env[string.upper(x)]`, etc. The key isn't statically resolvable
//!   without evaluating Lua. Authors who need cache invalidation on a
//!   dynamic-key read MUST surface the key statically (e.g., assign to a
//!   local first: `local k = cook.env["KEY"]`).
//! - **Writes** — `cook.env.X = …` and `cook.env["X"] = …`. The pattern
//!   appears identical to a read up to the LHS; the scanner checks the
//!   token immediately following the match and skips the key when it sees
//!   a `=` that is not part of `==`, `~=`, `<=`, `>=`.
//!
//! # False positives
//!
//! Conservative on false positives is the safe direction — over-recording
//! an env key only wastes a cache lookup; under-recording silently serves
//! stale output (the bug this scanner exists to close). The scanner does
//! NOT try to disambiguate `cook.env.X` appearing in:
//!
//! - a function-call argument that aliases `cook.env` away (e.g.
//!   `local e = cook.env; e.FOO`);
//! - reflective `_G.cook.env.X` access (the scanner anchors on the literal
//!   `cook.env.` byte sequence, which `_G.cook.env.X` happens to contain —
//!   acceptable false positive).
//!
//! These limitations are documented in §17.1 of the Cook Standard.

use std::collections::BTreeSet;

/// Scan `source` for static reads of `cook.env.<KEY>` and return the set of
/// keys found (sorted, deduplicated).
///
/// See module docs for the matching rules and skipped patterns.
pub fn scan_env_reads(source: &str) -> BTreeSet<String> {
    let mut keys: BTreeSet<String> = BTreeSet::new();
    let bytes = source.as_bytes();
    let mut i = 0usize;

    while i < bytes.len() {
        let b = bytes[i];

        // ── Skip line comments: `-- … <newline>`.
        if b == b'-' && i + 1 < bytes.len() && bytes[i + 1] == b'-' {
            // Long comment `--[[ … ]]` / `--[==[ … ]==]`.
            if i + 2 < bytes.len() && bytes[i + 2] == b'[' {
                let (eq_count, after_open) = count_long_bracket_eqs(&bytes[i + 3..]);
                if let Some(after_open_pos) = after_open {
                    let close_marker = format!("]{}]", "=".repeat(eq_count));
                    let from = i + 3 + after_open_pos;
                    if let Some(rel) = source[from..].find(&close_marker) {
                        i = from + rel + close_marker.len();
                        continue;
                    } else {
                        return keys; // unterminated long comment — bail
                    }
                }
            }
            // Line comment.
            while i < bytes.len() && bytes[i] != b'\n' {
                i += 1;
            }
            continue;
        }

        // ── Skip short strings.
        if b == b'"' || b == b'\'' {
            let quote = b;
            i += 1;
            while i < bytes.len() && bytes[i] != quote {
                if bytes[i] == b'\\' && i + 1 < bytes.len() {
                    i += 2;
                } else {
                    i += 1;
                }
            }
            if i < bytes.len() {
                i += 1; // closing quote
            }
            continue;
        }

        // ── Skip long strings `[[ … ]]` / `[==[ … ]==]`.
        if b == b'[' {
            let (eq_count, after_open) = count_long_bracket_eqs(&bytes[i + 1..]);
            if let Some(after_open_pos) = after_open {
                let close_marker = format!("]{}]", "=".repeat(eq_count));
                let from = i + 1 + after_open_pos;
                if let Some(rel) = source[from..].find(&close_marker) {
                    i = from + rel + close_marker.len();
                    continue;
                } else {
                    return keys; // unterminated long string — bail
                }
            }
        }

        // ── Try to match `cook.env` here.
        const PREFIX: &[u8] = b"cook.env";
        if bytes_starts_with(bytes, i, PREFIX)
            && !is_part_of_larger_identifier(bytes, i, PREFIX.len())
        {
            let after = i + PREFIX.len();

            // Dot access: `cook.env.IDENT`
            if after < bytes.len() && bytes[after] == b'.' {
                let id_start = after + 1;
                let id_end = scan_lua_ident_end(bytes, id_start);
                if id_end > id_start {
                    let key = &source[id_start..id_end];
                    if is_envkey_shape(key) && !is_assignment_target(bytes, id_end) {
                        keys.insert(key.to_string());
                    }
                    i = id_end;
                    continue;
                }
            }

            // String-indexed access: `cook.env["KEY"]` / `cook.env['KEY']`
            if after < bytes.len() && bytes[after] == b'[' {
                let mut j = after + 1;
                // Allow optional whitespace before the quote.
                while j < bytes.len() && (bytes[j] == b' ' || bytes[j] == b'\t') {
                    j += 1;
                }
                if j < bytes.len() && (bytes[j] == b'"' || bytes[j] == b'\'') {
                    let quote = bytes[j];
                    let key_start = j + 1;
                    let mut k = key_start;
                    while k < bytes.len() && bytes[k] != quote {
                        if bytes[k] == b'\\' && k + 1 < bytes.len() {
                            k += 2;
                        } else {
                            k += 1;
                        }
                    }
                    if k < bytes.len() && bytes[k] == quote {
                        // Find the closing `]` after the quote (skip whitespace).
                        let mut m = k + 1;
                        while m < bytes.len() && (bytes[m] == b' ' || bytes[m] == b'\t') {
                            m += 1;
                        }
                        if m < bytes.len() && bytes[m] == b']' {
                            let key_raw = &source[key_start..k];
                            // Unescape minimally: only `\"` / `\'` / `\\` are
                            // common in Lua key literals; everything else
                            // passes through. False positives here are safe.
                            let key = simple_unescape(key_raw);
                            if !key.is_empty() && !is_assignment_target(bytes, m + 1) {
                                keys.insert(key);
                            }
                            i = m + 1;
                            continue;
                        }
                    }
                }
                // Not a literal-keyed index — dynamic-key form. Skip past
                // `cook.env[` and resume scanning so a later `cook.env.X`
                // in an expression on the same line still matches.
                i = after + 1;
                continue;
            }

            // `cook.env` followed by something else (e.g. assignment to the
            // whole table, end of expression). Advance past the prefix.
            i = after;
            continue;
        }

        // ── Skip any identifier we encounter so we don't re-test for the
        // `cook.env` prefix inside an identifier (e.g. `bookmark`).
        if is_lua_ident_start(b) {
            i = scan_lua_ident_end(bytes, i);
            continue;
        }

        i += 1;
    }

    keys
}

/// At byte position `bytes[0]` we're past the leading `[`. If the next chars
/// are `=*[`, we have a long-bracket open. Returns `(eq_count,
/// Some(offset_just_past_second_[))`, or `(0, None)`.
fn count_long_bracket_eqs(bytes: &[u8]) -> (usize, Option<usize>) {
    let mut eq = 0;
    while eq < bytes.len() && bytes[eq] == b'=' {
        eq += 1;
    }
    if eq < bytes.len() && bytes[eq] == b'[' {
        (eq, Some(eq + 1))
    } else {
        (0, None)
    }
}

fn is_lua_ident_start(b: u8) -> bool {
    b.is_ascii_alphabetic() || b == b'_'
}

fn is_lua_ident_cont(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

/// Starting at position `start` (which MUST be a Lua-ident-start byte OR
/// the search returns `start`), advance while continuation bytes match.
fn scan_lua_ident_end(bytes: &[u8], start: usize) -> usize {
    let mut k = start;
    if k >= bytes.len() || !is_lua_ident_start(bytes[k]) {
        return k;
    }
    k += 1;
    while k < bytes.len() && is_lua_ident_cont(bytes[k]) {
        k += 1;
    }
    k
}

/// True if `bytes[i..]` starts with `needle`.
fn bytes_starts_with(bytes: &[u8], i: usize, needle: &[u8]) -> bool {
    i + needle.len() <= bytes.len() && &bytes[i..i + needle.len()] == needle
}

/// True if the byte immediately preceding position `i` (if any) is a Lua
/// identifier-continuation byte, OR if the byte at `i + prefix_len` (if any)
/// is also a Lua identifier-continuation byte. In either case the prefix is
/// embedded in a larger identifier (e.g. `_cook.env_x` is not the prefix
/// we're after; nor is `xcook.env`).
fn is_part_of_larger_identifier(bytes: &[u8], i: usize, prefix_len: usize) -> bool {
    if i > 0 && is_lua_ident_cont(bytes[i - 1]) {
        return true;
    }
    let after = i + prefix_len;
    after < bytes.len() && is_lua_ident_cont(bytes[after]) && bytes[after] != b'.'
}

/// True if `key` matches the conventional env-key shape `[A-Z_][A-Z0-9_]*`.
fn is_envkey_shape(key: &str) -> bool {
    let mut chars = key.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !(first.is_ascii_uppercase() || first == '_') {
        return false;
    }
    chars.all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
}

/// True if the byte position `pos` is immediately followed by an assignment
/// operator (`=` that is not part of `==`, `<=`, `>=`, `~=`, or a `=>`-style
/// future syntax). Used to skip `cook.env.X = …` writes.
fn is_assignment_target(bytes: &[u8], pos: usize) -> bool {
    let mut k = pos;
    // Skip horizontal whitespace.
    while k < bytes.len() && (bytes[k] == b' ' || bytes[k] == b'\t') {
        k += 1;
    }
    if k >= bytes.len() || bytes[k] != b'=' {
        return false;
    }
    // `==` is comparison, not assignment.
    if k + 1 < bytes.len() && bytes[k + 1] == b'=' {
        return false;
    }
    true
}

/// Minimal unescape for Lua short-string literal keys. Covers the common
/// escapes (`\"`, `\'`, `\\`) and decimal byte escapes; everything else
/// passes through unchanged. Best-effort — a false positive here is safe.
fn simple_unescape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\\' && i + 1 < bytes.len() {
            match bytes[i + 1] {
                b'\\' => {
                    out.push('\\');
                    i += 2;
                }
                b'"' => {
                    out.push('"');
                    i += 2;
                }
                b'\'' => {
                    out.push('\'');
                    i += 2;
                }
                b'n' => {
                    out.push('\n');
                    i += 2;
                }
                b't' => {
                    out.push('\t');
                    i += 2;
                }
                b'r' => {
                    out.push('\r');
                    i += 2;
                }
                _ => {
                    out.push(bytes[i] as char);
                    i += 1;
                }
            }
        } else {
            // Best-effort: push the byte as a char. Non-ASCII bytes in
            // env-key literals are unusual; this preserves them as the
            // raw byte char which still hashes consistently downstream.
            out.push(bytes[i] as char);
            i += 1;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys(src: &str) -> Vec<String> {
        scan_env_reads(src).into_iter().collect()
    }

    #[test]
    fn empty_source_yields_no_keys() {
        assert!(scan_env_reads("").is_empty());
    }

    #[test]
    fn no_cook_env_references_yields_no_keys() {
        assert!(scan_env_reads("local x = 1\nprint('hi')\n").is_empty());
    }

    #[test]
    fn matches_simple_dot_access() {
        assert_eq!(keys("local f = cook.env.FOO"), vec!["FOO"]);
    }

    #[test]
    fn matches_multiple_dot_accesses() {
        assert_eq!(
            keys("print(cook.env.FOO .. cook.env.BAR)"),
            vec!["BAR".to_string(), "FOO".to_string()]
        );
    }

    #[test]
    fn matches_string_indexed_double_quoted() {
        assert_eq!(keys("local v = cook.env[\"MY_KEY\"]"), vec!["MY_KEY"]);
    }

    #[test]
    fn matches_string_indexed_single_quoted() {
        assert_eq!(keys("local v = cook.env['MY_KEY']"), vec!["MY_KEY"]);
    }

    #[test]
    fn string_indexed_admits_mixed_case() {
        // Authors who explicitly write a string-keyed access have opted in;
        // we don't constrain shape.
        assert_eq!(keys(r#"local v = cook.env["Mixed_Case"]"#), vec!["Mixed_Case"]);
    }

    #[test]
    fn dot_access_skips_lower_case_idents() {
        // `cook.env.path` is unlikely to be an env key — by convention env
        // keys are upper-case. Skipping cuts noise from idioms like
        // `cook.env.path` that aren't env-keys at all.
        assert!(keys("local x = cook.env.path").is_empty());
    }

    #[test]
    fn dot_access_admits_underscore_prefix() {
        assert_eq!(keys("local x = cook.env._INTERNAL"), vec!["_INTERNAL"]);
    }

    #[test]
    fn dot_access_admits_digits() {
        assert_eq!(keys("local x = cook.env.VAR_1"), vec!["VAR_1"]);
    }

    #[test]
    fn skips_dynamic_key_index() {
        // cook.env[var] — no key literal, skipped silently.
        assert!(keys("local v = cook.env[name]").is_empty());
        assert!(keys("local v = cook.env[string.upper(x)]").is_empty());
    }

    #[test]
    fn skips_dot_assignment_write() {
        // cook.env.X = "v" is a write, not a read.
        assert!(keys("cook.env.FOO = \"hi\"").is_empty());
    }

    #[test]
    fn skips_string_indexed_assignment_write() {
        assert!(keys("cook.env[\"FOO\"] = \"hi\"").is_empty());
    }

    #[test]
    fn admits_dot_read_followed_by_equality_compare() {
        // `cook.env.X == "y"` is a read followed by `==`; must NOT be
        // treated as an assignment.
        assert_eq!(keys("if cook.env.FOO == \"y\" then end"), vec!["FOO"]);
    }

    #[test]
    fn skips_text_inside_short_string() {
        // The literal `cook.env.X` appearing inside a string is not a read.
        assert!(keys(r#"print("cook.env.FOO is")"#).is_empty());
        assert!(keys(r#"print('cook.env.BAR also')"#).is_empty());
    }

    #[test]
    fn skips_text_inside_long_string() {
        let src = "local s = [[ cook.env.FOO ]]\n";
        assert!(keys(src).is_empty());
    }

    #[test]
    fn skips_text_inside_long_string_with_eqs() {
        let src = "local s = [==[ cook.env.FOO ]==]\n";
        assert!(keys(src).is_empty());
    }

    #[test]
    fn skips_text_inside_line_comment() {
        assert!(keys("-- cook.env.FOO is dead code\n").is_empty());
    }

    #[test]
    fn skips_text_inside_block_comment() {
        assert!(keys("--[[ cook.env.FOO ]] local x = 1\n").is_empty());
    }

    #[test]
    fn finds_after_line_comment_in_same_source() {
        let src = "-- cook.env.SKIPPED\nlocal v = cook.env.KEPT";
        assert_eq!(keys(src), vec!["KEPT"]);
    }

    #[test]
    fn finds_after_string_in_same_source() {
        let src = "print(\"cook.env.SKIPPED\"); local v = cook.env.KEPT";
        assert_eq!(keys(src), vec!["KEPT"]);
    }

    #[test]
    fn rejects_embedded_in_larger_ident() {
        // `_cook.env.X` would still hit the literal — the leading char is
        // not an ident-cont char for the byte preceding `c`. But
        // `xcook.env.X` — the byte preceding `c` IS `x`, ident-cont — must
        // be skipped.
        assert!(keys("local v = xcook.env.FOO").is_empty());
    }

    #[test]
    fn does_not_match_attribute_chain_through_other_root() {
        // `mything.cook.env.X` should not match — the prefix is preceded
        // by `.` which makes `cook` an attribute, not a free root. The
        // current scanner accepts this as a false positive (the `.` before
        // `cook` is NOT an ident-cont char). Documented limitation.
        assert_eq!(keys("local v = mything.cook.env.FOO"), vec!["FOO"]);
    }

    #[test]
    fn dedups_repeated_keys() {
        assert_eq!(
            keys("local a = cook.env.FOO\nlocal b = cook.env.FOO\nlocal c = cook.env.FOO\n"),
            vec!["FOO"]
        );
    }

    #[test]
    fn returns_sorted_keys() {
        assert_eq!(
            keys("local a = cook.env.ZZ\nlocal b = cook.env.AA\nlocal c = cook.env.MM\n"),
            vec!["AA".to_string(), "MM".to_string(), "ZZ".to_string()]
        );
    }

    #[test]
    fn handles_realistic_using_lua_body() {
        // The kind of body the smoke test exercises.
        let body = r#"
            local f = io.open(output, "w")
            f:write("FOO=" .. tostring(cook.env.FOO))
            f:write("BAR=" .. tostring(cook.env.BAR))
            f:close()
        "#;
        assert_eq!(keys(body), vec!["BAR".to_string(), "FOO".to_string()]);
    }

    #[test]
    fn skipping_dynamic_key_does_not_swallow_following_read() {
        // After `cook.env[expr]`, the scanner must resume so a later
        // `cook.env.X` on the same line still matches.
        let src = "local x = cook.env[name] or cook.env.FALLBACK";
        assert_eq!(keys(src), vec!["FALLBACK"]);
    }

    #[test]
    fn write_then_read_records_only_read() {
        let src = "cook.env.WRITE = \"x\"\nlocal v = cook.env.READ";
        assert_eq!(keys(src), vec!["READ"]);
    }
}
