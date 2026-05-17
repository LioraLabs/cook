Exercises §28.3.14 cc.checks.has_compile_flag with a flag the
compiler will reject. Register-time: probe registration succeeds
(no diagnostic). Execute-time: the probe MUST return false; the
sigil expansion in any consuming command MUST stringify to 'false'.
This is a 'negative' fixture in the sense that the *feature being
tested* fails at execute time, even though the parse succeeds —
documenting the demand-driven failure shape.

The parser-only conformance harness in
`cli/crates/cook-lang/tests/conformance.rs` SKIPS fixtures carrying
only an `execute_error.txt`. A future execute-phase runner consumes
the baseline (matching the precedent set by
`cc-find-missing-on-build`). The baseline below is a stable
sentinel — the probe returns the literal value `false`, surfaced as
the string `"false"` when expanded into a consuming command's
vars-literal argument.
