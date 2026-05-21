Invocation-time error: chore declares 1 required parameter but more argv elements
are supplied than declared parameters.

The conformance harness check is parse-only (the Cookfile lexes and registers cleanly
so this is a positive/parse-success fixture). The integration test in
`cli/crates/cook-cli/tests/chore_params_test.rs` covers the binding path.

Implementations MUST surface a diagnostic of the form:

    chore 'NAME' takes K parameter(s) but M positional argument(s) were supplied

when the chore is dispatched with more argv elements than declared parameters.
