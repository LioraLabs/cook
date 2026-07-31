//! CS-0128: the shell quoting law — one law, both halves (COOK-389).
//!
//! A `$<param>` sigil expands per the shell quoting context it sits in.
//! The law has two halves that must agree: the CLASSIFIER (scan the command
//! prefix, decide bare / double / single — run by cook-luagen at codegen
//! time) and the QUOTER (render the bound value for that context — run by
//! cook-register's `cook.__quote_param` at register time, on text luagen
//! generated). Until COOK-389 they lived in their two crates joined by a
//! bare string ("bare"/"dquote"/"squote") with a silent `_ =>` fallback on
//! the consumer side, so a fourth context or a tag rename compiled clean
//! and emitted literal quote characters into the user's command at runtime.
//!
//! Here the seam is typed: [`QCtx`] round-trips through [`QCtx::tag`] /
//! [`QCtx::from_tag`], the generated-Lua tag strings exist in exactly one
//! place, and an unknown tag is the CONSUMER'S diagnostic, never bare.
//!
//! Pure `&str` → enum/String, no mlua, no IO — the sigil/subst shape.

/// The shell quoting context a sigil span sits in (CS-0128).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QCtx {
    /// Outside any quote — the value must be single-quoted for word-safety.
    Bare,
    /// Inside a double-quoted region — the value is escaped for that context.
    Double,
    /// Inside a single-quoted region — the value is emitted verbatim.
    Single,
}

impl QCtx {
    /// The tag spelled into generated Lua (`cook.__quote_param(..., TAG)`).
    /// The inverse of [`QCtx::from_tag`]; these two are the whole wire
    /// format of the seam.
    pub fn tag(self) -> &'static str {
        match self {
            QCtx::Bare => "bare",
            QCtx::Double => "dquote",
            QCtx::Single => "squote",
        }
    }

    /// Parse a generated-Lua tag back to its context. `None` for anything
    /// else — the consumer MUST treat that as a diagnostic (emitter/consumer
    /// version drift), never as `Bare`.
    pub fn from_tag(tag: &str) -> Option<QCtx> {
        match tag {
            "bare" => Some(QCtx::Bare),
            "dquote" => Some(QCtx::Double),
            "squote" => Some(QCtx::Single),
            _ => None,
        }
    }
}

/// Classify the shell quoting context at the end of `prefix` by scanning the
/// command text left-to-right, tracking single/double quote state and
/// backslash escapes (POSIX: backslash is inert inside single quotes).
pub fn quote_context(prefix: &str) -> QCtx {
    let (mut sq, mut dq, mut esc) = (false, false, false);
    for c in prefix.chars() {
        if esc {
            esc = false;
            continue;
        }
        match c {
            '\\' if !sq => esc = true,
            '\'' if !dq => sq = !sq,
            '"' if !sq => dq = !dq,
            _ => {}
        }
    }
    if sq {
        QCtx::Single
    } else if dq {
        QCtx::Double
    } else {
        QCtx::Bare
    }
}

/// CS-0128: quote `s` for the shell context `ctx` that a `$<param>` sigil
/// occupies in the step text.
///
/// * [`QCtx::Bare`] — single-quote the whole value for word-safety
///   ([`shell_quote`]).
/// * [`QCtx::Double`] — the sigil sits inside an author-supplied
///   double-quoted region, so emit the value with double-quote-context
///   escaping (backslash-escape `\`, `"`, `$`, and backtick); the
///   surrounding `"..."` already provides the quoting.
/// * [`QCtx::Single`] — the sigil sits inside an author-supplied
///   single-quoted region, so emit the raw value verbatim; a value
///   containing `'` is the author's responsibility (documented edge).
pub fn quote_for_ctx(s: &str, ctx: QCtx) -> String {
    match ctx {
        QCtx::Double => {
            let mut o = String::with_capacity(s.len());
            for ch in s.chars() {
                if matches!(ch, '\\' | '"' | '$' | '`') {
                    o.push('\\');
                }
                o.push(ch);
            }
            o
        }
        QCtx::Single => s.to_string(),
        QCtx::Bare => shell_quote(s),
    }
}

/// POSIX-safe single-quote escaping for shell arguments.
///
/// Wraps the whole string in single quotes; any literal `'` becomes `'\''`
/// (close-quote, escaped-quote, re-open-quote). This is the canonical
/// sh-portable form and handles every character including spaces,
/// backslashes, and dollar signs. The only POSIX single-quote escaper in
/// the workspace; it moves with the law.
pub fn shell_quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('\'');
    for ch in s.chars() {
        if ch == '\'' {
            out.push_str("'\\''");
        } else {
            out.push(ch);
        }
    }
    out.push('\'');
    out
}

#[cfg(test)]
#[path = "tests/quoting_tests.rs"]
mod tests;
