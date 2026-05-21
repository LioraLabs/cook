Invocation-time error: Lua-expression default raises a Lua error.

The conformance harness check is parse-only (the Cookfile lexes and parses
cleanly — this is a positive/parse-success fixture). The integration test in
`cli/crates/cook-cli/tests/chore_params_test.rs`
(`chore_lua_default_error_surfaces_diagnostic`) asserts the runtime diagnostic.

Implementations MUST surface a diagnostic of the form:

    chore 'NAME': default for parameter 'PARAM' raised a Lua error (defined at line LINE): MESSAGE

when the chore is dispatched without argv and the default expression raises.
