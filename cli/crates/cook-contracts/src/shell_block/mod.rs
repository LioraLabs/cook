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

#[cfg(test)]
#[path = "tests/shell_block_tests.rs"]
mod tests;
