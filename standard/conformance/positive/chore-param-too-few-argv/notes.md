Invocation-time error: chore declares a required parameter but argv supplies none.

The conformance harness check is parse-only (the Cookfile lexes and registers cleanly
so this is a positive/parse-success fixture). The integration test in
`cli/crates/cook-cli/tests/chore_params_test.rs` (`chore_missing_required_argv_errors`)
asserts the runtime diagnostic.

Implementations MUST surface a diagnostic of the form:

    chore 'NAME' requires parameter 'PARAM' (declared at line LINE); supply it as a positional argument

when the chore is dispatched without the required argv element.
