Exercises §28.3.15 missing-var behaviour: template @MISSING@ →
empty substitution with a renderer diagnostic. The register step
succeeds (config_header has no register-time validation of template
content); execute-time renders successfully with the empty
substitution. The 'negative' framing documents the at-execute
warning surface.

The parser-only conformance harness in
`cli/crates/cook-lang/tests/conformance.rs` SKIPS fixtures carrying
only an `execute_error.txt`. A future execute-phase runner consumes
the baseline — the renderer emits an informational diagnostic when
a template variable is missing from `vars`. The baseline below is
the stable prefix.
