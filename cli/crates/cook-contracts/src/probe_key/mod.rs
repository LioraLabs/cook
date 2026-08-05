//! The probe-key grammar, once (COOK-408, CS-0201).
//!
//! ```text
//! PROBE_KEY ::= PROBE_SEG (":" PROBE_SEG)*
//! PROBE_SEG ::= [A-Za-z_] [A-Za-z0-9_-]*
//! ```
//!
//! A probe key is spelled two ways and every site that names one accepts both:
//! **bare**, which must match `PROBE_KEY`, and **quoted**, which is any
//! non-empty string and is the escape hatch for anything the bare form cannot
//! spell. The Lua API's string argument IS the quoted form, so
//! `cook.probe("g++")` stays legal.
//!
//! # What this replaced
//!
//! Five sites named a probe key and no two agreed. The declaration allowed
//! `-` and `.` and capped at two colon-segments; `cook.probe()` validated
//! nothing; the sigil scanner allowed `- . : [ ]` uncapped; `seal` allowed
//! neither `-` nor `.`, capped at two, and refused the quoted form outright;
//! `ingredients` allowed neither `-` nor `.` but was uncapped and did accept
//! quoting. So `probe cc-version` declared a key that could be referenced by
//! sigil but neither sealed nor consumed, and `cc:find:raylib` — the flagship
//! module's ordinary case — could not be sealed at all, which is exactly the
//! pin a cache-trust story needs.
//!
//! # Why `.` is not in `PROBE_SEG`
//!
//! Because `.` is member access in a sigil. With `.` inside a segment,
//! `$<demo:cc-version.ver>` is either probe `demo:cc-version` field `ver` or
//! probe `demo:cc-version.ver` with no field, and the two readings cannot be
//! told apart from the text. Resolving that by longest-match against the
//! declared set would make an existing reference change meaning when an
//! unrelated probe is declared, which is the class of bug this codebase spent
//! a milestone removing. CS-0131 admitted `.` before probe references had
//! member access; the two features are incompatible and member access is worth
//! more.
//!
//! # Why `TOOL_NAME` is a different production
//!
//! It used to be `PROBE_SEG` (CS-0181), which conflated "a segment of a key"
//! with "the name of an executable". Only the first has the ambiguity above,
//! and only the second needs `.` — `python3.11` is a real tool name.
//! [`is_tool_name`] keeps the dot.

/// True when `c` may start a `PROBE_SEG` or a `TOOL_NAME`.
fn is_head(c: char) -> bool {
    c.is_ascii_alphabetic() || c == '_'
}

/// True when `s` is a single valid `PROBE_SEG`.
pub fn is_segment(s: &str) -> bool {
    let mut chars = s.chars();
    chars.next().is_some_and(is_head)
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// True when `s` is a valid bare probe key: one or more `:`-separated
/// segments, no cap on how many.
///
/// The two-segment cap this used to carry was enforced on the surface
/// declaration and ignored by `cook.probe()`, so modules mint three-segment
/// keys (`cc:find:raylib`, `cc:compiler:auto`) as their ordinary case. A cap
/// one of two declaration paths enforces is not a rule.
pub fn is_valid_bare(key: &str) -> bool {
    !key.is_empty() && key.split(':').all(is_segment)
}

/// True when `s` is a valid bare tool name. As `PROBE_SEG` but `.` is
/// admitted, because an executable name may carry one and a tool name is
/// never member-accessed.
pub fn is_tool_name(s: &str) -> bool {
    let mut chars = s.chars();
    chars.next().is_some_and(is_head)
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.')
}

/// The diagnostic for a bare key that does not match the grammar. One wording,
/// so `seal`, `ingredients` and the declaration cannot blame different things
/// for the same cause.
pub fn bare_key_error(site: &str, key: &str) -> String {
    let hint = if key.contains('.') {
        " ('.' is member access in a probe reference and is not part of a key; \
         use '-' or the quoted form)"
    } else {
        ""
    };
    format!(
        "{site}: malformed probe key '{key}'{hint}. A bare key is one or more \
         ':'-separated segments matching [A-Za-z_][A-Za-z0-9_-]*; the quoted \
         form \"{key}\" accepts any spelling."
    )
}

// ---------------------------------------------------------------------------
// Scoped keys (§24.4.3)
// ---------------------------------------------------------------------------

/// §24.4.3: the key a `cook.probes.scope(label)` view reads and writes —
/// `label .. ":" .. key`. Both phases compose it: the register view over the
/// module store, and the execute view over the per-run probe-value store
/// (whose unmaterialised-read diagnostic MUST report this full form,
/// §22.5.8).
pub fn scoped_key(label: &str, key: &str) -> String {
    format!("{label}:{key}")
}

/// §24.4.3: a scope label MUST NOT contain `:` — a colon in the label would
/// make `scope("a:b").get("c")` and `scope("a").get("b:c")` collide on one
/// stored key. `Some(diagnostic)` when the label is invalid; both phases
/// raise it verbatim.
pub fn scope_label_error(label: &str) -> Option<String> {
    label.contains(':').then(|| {
        format!(
            "cook.probes.scope: label '{label}' must not contain ':' \
             (the scope separator; Standard §24.4.3)"
        )
    })
}

#[cfg(test)]
#[path = "tests/probe_key_tests.rs"]
mod tests;
