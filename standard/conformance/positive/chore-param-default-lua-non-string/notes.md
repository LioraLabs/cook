Invocation-time error: Lua-expression default returns a non-string value.

The conformance harness check is parse-only (the Cookfile lexes and parses
cleanly — this is a positive/parse-success fixture). The integration test in
`cli/crates/cook-cli/tests/chore_params_test.rs`
(`chore_lua_default_non_string_surfaces_diagnostic`) asserts the runtime
diagnostic.

Implementations MUST surface a diagnostic of the form:

    chore 'NAME': default for parameter 'PARAM' must evaluate to a string; got TYPE (defined at line LINE)

when the chore is dispatched without argv and the default expression returns
a non-string Lua value (e.g. a table, number, boolean).
