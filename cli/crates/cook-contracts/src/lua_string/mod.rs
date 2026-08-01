//! The Lua string-literal escaping law (COOK-398).
//!
//! Cook generates Lua from two places. `cook-luagen` lowers a Cookfile into
//! the register-phase program; `cook-register` builds the chore-param prelude
//! that program runs against. Both embed arbitrary user text (command lines,
//! CLI argv, probe values) inside double-quoted Lua literals, and both used to
//! spell the escape rule themselves.
//!
//! They had already drifted. luagen escaped `\`, `"` and newline; register
//! escaped those plus carriage return and NUL. A value carrying a `\r` was a
//! chore prelude that loaded and a generated command that was a Lua syntax
//! error, because Lua forbids a raw newline or carriage return inside a short
//! literal. Which answer you got depended on which emitter the text reached.
//!
//! # Why `\ddd` is always three digits
//!
//! Lua's decimal escape consumes UP TO three digits, so a one- or two-digit
//! escape is only safe when the next character is not a digit. `"\0"` followed
//! by the text `5` is not NUL then `5`: it is `\05`, the single character 5.
//! Register's hand-rolled version emitted exactly that. Every escape here is
//! zero-padded to three digits, which is unambiguous in every position.
//!
//! # Scope
//!
//! This is the SHORT-literal law: what goes between two `"`. Long-bracket
//! literals (`[[ … ]]`) escape nothing and are a different problem, solved by
//! choosing a bracket level no inner close can match; that stays in
//! `cook-luagen`'s `wrap_lua_string`, which is a lowering choice rather than a
//! rule two crates must agree on.
//!
//! Pure `&str` → `String`. No mlua, no IO.

/// Escape `s` for inclusion between the quotes of a double-quoted Lua string
/// literal.
///
/// Escapes the two characters that would end or continue the literal (`\` and
/// `"`), the two Lua forbids raw inside one (newline and carriage return), and
/// every remaining control character, which is legal but not reliably
/// round-trippable through an editor or a diff.
///
/// The result never contains a raw byte below `0x20`, a `"`, or a lone `\`, so
/// it is safe to concatenate with any surrounding text.
pub fn escape_double_quoted(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            // Legal in a Lua literal, but a raw control byte in generated
            // source survives no round trip a human is involved in. Three
            // digits so a following digit cannot be absorbed.
            c if (c as u32) < 0x20 || c as u32 == 0x7f => {
                out.push_str(&format!("\\{:03}", c as u32));
            }
            other => out.push(other),
        }
    }
    out
}

#[cfg(test)]
#[path = "tests/lua_string_tests.rs"]
mod tests;
