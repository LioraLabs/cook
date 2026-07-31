//! What a `shell_block` body means as shell text (§{steps.shell-block-invocation}).
//!
//! A `{ … }` body is ONE shell invocation, not one per line: the block's lines
//! are joined with LF in source order under a `set -e` prefix, and the result
//! goes to a single `/bin/sh -c`. That single sentence decides three things a
//! Cookfile author can observe — whether a failing line stops the block,
//! whether a `cd` on one line is still in effect on the next, and whether the
//! block counts as one spawn or several when its output is attributed — which
//! is why it is stated in the Standard and implemented here rather than being
//! whatever each caller happens to write.
//!
//! It was written twice, identically, in `cook-luagen`: once for `cook` step
//! bodies and once for `test` step bodies, differing only by a parameter one of
//! them ignored. Two copies of a rule this load-bearing are two chances for a
//! build to stop meaning what its author wrote (CS-0190).

/// The halt-on-failure prelude `compose` puts before a non-empty body, as
/// it appears in the composed text. Callers that must spell the prefix in
/// another encoding (e.g. inside a generated Lua string literal) derive it
/// from this constant rather than re-typing it (COOK-391).
pub const SET_E_PREFIX: &str = "set -e\n";

/// Compose a `shell_block`'s lines into the single shell text that runs them.
///
/// The `set -e` prefix costs no source line: line *k* of `lines` is line *k* of
/// the author's body, so a diagnostic citing a line number cites theirs. An
/// empty block composes to the bare prefix, which is a well-formed no-op.
pub fn compose(lines: &[String]) -> String {
    let mut out = String::from("set -e");
    for line in lines {
        out.push('\n');
        out.push_str(line);
    }
    out
}

/// `compose`'s inverse for display: strip the `set -e` prelude, leaving the
/// author's body text. Not-composed text passes through unchanged.
///
/// The empty-block edge has ONE answer here (COOK-391): `compose(&[])`
/// yields the bare `"set -e"` with no trailing LF, and stripping it yields
/// the empty body the author wrote — the two former per-crate strippers
/// (`strip_prefix("set -e\n")` twins) left that case unstripped, and a
/// third site filtered any `set -e` LINE anywhere in the body, which is
/// not this law's inverse at all.
pub fn strip_set_e(cmd: &str) -> &str {
    match cmd.strip_prefix(SET_E_PREFIX) {
        Some(body) => body,
        None if cmd == "set -e" => "",
        None => cmd,
    }
}

#[cfg(test)]
#[path = "tests/shell_block_tests.rs"]
mod tests;
