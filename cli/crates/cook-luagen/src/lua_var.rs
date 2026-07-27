//! Static scanner for `var.<NAME>` reads inside `using >{ ... }` Lua
//! bodies (Standard §17.1).
//!
//! A `var.NAME` read in a Lua body is the same cache determinant that a
//! `$<NAME>` sigil is in a shell body, so the scanned keys are folded into the
//! unit's `consulted_env_keys` at codegen time and a value change invalidates
//! the unit exactly as a shell-body read would.
//!
//! The module header used to say this "mirrors the shell-side `$<KEY>` sigil
//! scanner in `crate::sigil`", which implied a sync obligation that does not
//! exist: the two read different languages for different shapes and share
//! nothing but a purpose. What they DID share was the walk that skips strings
//! and comments, copied three ways; that now lives in [`crate::lua_scan`]
//! (COOK-357).
//!
//! # Matching rules
//!
//! Three patterns are recognised, each scoped outside strings and comments
//! (the scanner skips `"..."`, `'...'`, `[[...]]` long strings, `--` line
//! comments, and `--[[...]]` long comments before testing for matches):
//!
//! 1. **Dot access** — `var.IDENT` for any Lua identifier. No case
//!    constraint: declared variables are routinely lower-case
//!    (`var.optimize`), and under-recording is the unsafe direction.
//! 2. **String index, double-quoted** — `var["NAME"]` where `KEY` is
//!    any non-empty string literal. No case constraint — the author has
//!    explicitly named the key.
//! 3. **String index, single-quoted** — `var['NAME']`. Same as (2).
//!
//! # Skipped (by design)
//!
//! - **Dynamic-key reads** — `var[k]`, `var[NAME_VAR]`,
//!   `var[string.upper(x)]`, etc. The key isn't statically resolvable
//!   without evaluating Lua. Authors who need cache invalidation on a
//!   dynamic-key read MUST surface the key statically (e.g., assign to a
//!   local first: `local k = var["NAME"]`).
//! - **Writes** — `var.X = …` and `var["X"] = …` (rejected at runtime by the
//!   read-only proxy, but skipped here too so a write is never keyed). The pattern
//!   appears identical to a read up to the LHS; the scanner checks the
//!   token immediately following the match and skips the key when it sees
//!   a `=` that is not part of `==`, `~=`, `<=`, `>=`.
//!
//! # False positives
//!
//! Conservative on false positives is the safe direction — over-recording
//! an env key only wastes a cache lookup; under-recording silently serves
//! stale output (the bug this scanner exists to close). The scanner does
//! NOT try to disambiguate `var.X` appearing in:
//!
//! - a function-call argument that aliases `var` away (e.g.
//!   `local v = var; v.FOO`);
//! - reflective `_G.var.X` access (the scanner anchors on the literal
//!   `var` byte sequence, which `_G.var.X` happens to contain —
//!   acceptable false positive).
//!
//! These limitations are documented in §17.1 of the Cook Standard.

use std::collections::BTreeSet;

use crate::lua_scan;

/// Scan `source` for static reads of `var.<NAME>` and return the set of
/// keys found (sorted, deduplicated).
///
/// See module docs for the matching rules and skipped patterns.
pub fn scan_var_reads(source: &str) -> BTreeSet<String> {
    let mut keys: BTreeSet<String> = BTreeSet::new();
    let bytes = source.as_bytes();
    let mut i = 0usize;

    while i < bytes.len() {
        let b = bytes[i];

        // Strings and comments are not code (see `crate::lua_scan`).
        match lua_scan::skip_non_code(source, bytes, i) {
            lua_scan::Skip::Ended(next) => {
                i = next;
                continue;
            }
            lua_scan::Skip::Unterminated => return keys,
            lua_scan::Skip::Code => {}
        }

        // ── Try to match `var` here.
        const PREFIX: &[u8] = b"var";
        if bytes_starts_with(bytes, i, PREFIX)
            && !is_part_of_larger_identifier(bytes, i, PREFIX.len())
        {
            let after = i + PREFIX.len();

            // Dot access: `var.IDENT`
            if after < bytes.len() && bytes[after] == b'.' {
                let id_start = after + 1;
                let id_end = lua_scan::ident_end(bytes, id_start);
                if id_end > id_start {
                    let key = &source[id_start..id_end];
                    // CS-0172: any Lua identifier counts. The pre-CS-0172
                    // scanner required an upper-case `[A-Z_][A-Z0-9_]*` shape;
                    // declared variables are routinely lower-case
                    // (`var.optimize`), and under-recording is the unsafe
                    // direction — a missed key serves stale output.
                    if !is_assignment_target(bytes, id_end) {
                        keys.insert(key.to_string());
                    }
                    i = id_end;
                    continue;
                }
            }

            // String-indexed access: `var["NAME"]` / `var['NAME']`
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
                // `var[` and resume scanning so a later `var.X`
                // in an expression on the same line still matches.
                i = after + 1;
                continue;
            }

            // `var` followed by something else (e.g. assignment to the
            // whole table, end of expression). Advance past the prefix.
            i = after;
            continue;
        }

        // ── Skip any identifier we encounter so we don't re-test for the
        // `var` prefix inside an identifier (e.g. `variadic`).
        if lua_scan::is_ident_start(b) {
            i = lua_scan::ident_end(bytes, i);
            continue;
        }

        i += 1;
    }

    keys
}

/// True if `bytes[i..]` starts with `needle`.
fn bytes_starts_with(bytes: &[u8], i: usize, needle: &[u8]) -> bool {
    i + needle.len() <= bytes.len() && &bytes[i..i + needle.len()] == needle
}

/// True if the byte immediately preceding position `i` (if any) is a Lua
/// identifier-continuation byte, OR if the byte at `i + prefix_len` (if any)
/// is also a Lua identifier-continuation byte. In either case the prefix is
/// embedded in a larger identifier (e.g. `variadic` is not the `var` prefix
/// we're after; nor is `myvar`).
fn is_part_of_larger_identifier(bytes: &[u8], i: usize, prefix_len: usize) -> bool {
    if i > 0 && lua_scan::is_ident_cont(bytes[i - 1]) {
        return true;
    }
    let after = i + prefix_len;
    after < bytes.len() && lua_scan::is_ident_cont(bytes[after]) && bytes[after] != b'.'
}

/// True if the byte position `pos` is immediately followed by an assignment
/// operator (`=` that is not part of `==`, `<=`, `>=`, `~=`, or a `=>`-style
/// future syntax). Used to skip `var.X = …` writes.
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

/// Scan `source` for static reads of `cook.probes.get("<KEY>")` (or the
/// single-quoted form) and return the set of literal keys found (sorted,
/// deduplicated).
///
/// # Why this exists
///
/// A shell-body unit's `probes` field is populated by scanning `$<key>`
/// sigils in its command string (CS-0074), which demand-schedules the probe
/// ahead of the unit and folds its key into the fingerprint. A Lua-body unit
/// (`lua_code = "..."`, e.g. from a native `>{ … }` cook-step body) has no
/// equivalent sigil surface: a literal `cook.probes.get("key")` call inside
/// the body was previously invisible to the register-phase capture, so the
/// probe was never demand-scheduled and the call read nil at execute time.
/// The keys this scanner finds are unioned into the unit's `probes` field at
/// the `cook.add_unit` capture site (see `unit_api.rs`), which routes them
/// through the existing end-of-pass §22.5.6 machinery exactly like a key
/// that arrived via the `probes = {...}` table or a `$<key>` sigil: an
/// unknown key still produces the usual register-phase diagnostic, a known
/// key gets a DAG edge, a fingerprint fold, and demand-driven scheduling.
///
/// # Matching rule
///
/// `cook.probes.get(` — optional whitespace — a double- or single-quoted
/// short-string literal — optional whitespace — immediately followed by `)`
/// or `,`. Requiring `)`/`,` right after the literal is what excludes
/// concatenation: `cook.probes.get("a" .. b)` has a string literal as the
/// first token, but that literal is not the WHOLE argument — the real key is
/// dynamic, so collecting `"a"` would be wrong. Matching is scoped outside
/// strings and comments exactly like [`scan_var_reads`] (see that function's
/// docs for the comment/string-skipping approach, reused verbatim here).
///
/// # Skipped (by design)
///
/// - **Non-literal first argument** — `cook.probes.get(k)`,
///   `cook.probes.get(some_fn())`. Not statically resolvable; falls through
///   to the execute-phase hard error on an undeclared probe read.
/// - **Concatenation** — `cook.probes.get("a" .. b)` (see above).
/// - **`cook.probes.scope(...)` chains** — out of scope by design; only the
///   literal `cook.probes.get` accessor is scanned.
/// - Text inside comments or unrelated string literals (mirrors
///   [`scan_var_reads`]).
pub fn scan_probe_reads(source: &str) -> BTreeSet<String> {
    let mut keys: BTreeSet<String> = BTreeSet::new();
    let bytes = source.as_bytes();
    let mut i = 0usize;

    while i < bytes.len() {
        let b = bytes[i];

        // Strings and comments are not code (see `crate::lua_scan`).
        match lua_scan::skip_non_code(source, bytes, i) {
            lua_scan::Skip::Ended(next) => {
                i = next;
                continue;
            }
            lua_scan::Skip::Unterminated => return keys,
            lua_scan::Skip::Code => {}
        }

        // ── Try to match `cook.probes.get` here.
        const PREFIX: &[u8] = b"cook.probes.get";
        if bytes_starts_with(bytes, i, PREFIX)
            && !is_part_of_larger_identifier(bytes, i, PREFIX.len())
        {
            let after = i + PREFIX.len();

            // Optional whitespace before `(`.
            let mut j = after;
            while j < bytes.len() && is_lua_space(bytes[j]) {
                j += 1;
            }

            if j < bytes.len() && bytes[j] == b'(' {
                // Optional whitespace before the argument.
                let mut k = j + 1;
                while k < bytes.len() && is_lua_space(bytes[k]) {
                    k += 1;
                }

                if k < bytes.len() && (bytes[k] == b'"' || bytes[k] == b'\'') {
                    let quote = bytes[k];
                    let key_start = k + 1;
                    let mut m = key_start;
                    while m < bytes.len() && bytes[m] != quote {
                        if bytes[m] == b'\\' && m + 1 < bytes.len() {
                            m += 2;
                        } else {
                            m += 1;
                        }
                    }
                    if m < bytes.len() && bytes[m] == quote {
                        // Closing quote at `m`. Only collect when the
                        // literal is immediately (modulo whitespace) the
                        // WHOLE argument — i.e. followed by `)` or `,` —
                        // which is what rules out `"a" .. b` concatenation.
                        let mut n = m + 1;
                        while n < bytes.len() && is_lua_space(bytes[n]) {
                            n += 1;
                        }
                        if n < bytes.len() && (bytes[n] == b')' || bytes[n] == b',') {
                            let key_raw = &source[key_start..m];
                            let key = simple_unescape(key_raw);
                            if !key.is_empty() {
                                keys.insert(key);
                            }
                        }
                        // Resume right after the closing quote (not after
                        // `)`/`,`) regardless of whether we collected, so a
                        // concatenation's trailing expression is still
                        // scanned normally by the outer loop.
                        i = m + 1;
                        continue;
                    }
                    // Unterminated string literal in the call argument —
                    // resume right after the opening quote so we don't spin.
                    i = key_start;
                    continue;
                }
                // Non-literal first argument (identifier, nested call,
                // number, ...) — not statically resolvable. Resume right
                // after `(` so anything later on the same line still scans.
                i = j + 1;
                continue;
            }
            // `cook.probes.get` not immediately followed by a call — e.g.
            // passed around as a bare reference. Resume after the prefix.
            i = after;
            continue;
        }

        // ── Skip any identifier we encounter so we don't re-test for the
        // `cook.probes.get` prefix inside an identifier.
        if lua_scan::is_ident_start(b) {
            i = lua_scan::ident_end(bytes, i);
            continue;
        }

        i += 1;
    }

    keys
}

/// True if `b` is Lua-insignificant horizontal-or-vertical whitespace
/// (space, tab, CR, LF). Used to tolerate whitespace around `(` and around
/// the string-literal argument in [`scan_probe_reads`].
fn is_lua_space(b: u8) -> bool {
    matches!(b, b' ' | b'\t' | b'\r' | b'\n')
}

#[cfg(test)]
#[path = "tests/lua_var_tests.rs"]
mod tests;
