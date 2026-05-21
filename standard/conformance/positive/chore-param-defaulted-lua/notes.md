Positive parse fixture: Lua-expression chore parameter default.

The conformance harness verifies that the Cookfile parses successfully and that
`version` is recorded as a `DefaultedLua` parameter with
`default_lua = "cook.git.head_tag() or \"v0\""`.

At invocation time (`cook release`), if no argv is supplied the default Lua
expression is evaluated against the Cookfile-scope VM (§13.2 load phase).
The result MUST be a string; non-string returns raise a diagnostic (§7.1.2).

Integration tests covering runtime invocation are in
`cli/crates/cook-cli/tests/chore_params_test.rs`.
