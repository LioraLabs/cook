Pins Standard §5.5: a `$<NAME>` bare reference in a bare
`shell_command` body MUST substitute by the space-joined concatenation
of the named recipe's output list, identical to its meaning in `cook`
`using` blocks and `plate`/`test` shell bodies.

Codegen used to lower the bare-shell substitution as a `cook.sh(...)`
expression embedded in a long-string `lua_code` payload, with
`cook.dep_output("NAME")` left as a runtime call. The worker VM has no
`cook.dep_output` (only the register VM does — see
`cli/crates/cook-register/src/dep_output_api.rs`), so the worker
crashed at execute time with
`attempt to call a nil value (field 'dep_output')`.

The fix splits the bundled-body chunk into Static and
RegisterTimeShellCmd pieces. Pieces with `cook.dep_output(...)` /
`cook.require_env(...)` are wrapped via Lua-string concat with
`string.format("%q", ...)` so register-time evaluation produces a
literal `io.write(cook.sh("..."))` line for the worker. See
`cli/crates/cook-luagen/src/recipe.rs` (`emit_body_unit_with_names`,
`render_chunk_pieces`).

This fixture pins parse + codegen success for the surface form;
behavioural confirmation lives in the cook-luagen unit tests
`test_bare_shell_dep_ref_lowers_to_register_time_eval` and
`test_bare_shell_env_ref_lowers_to_register_time_eval`.
