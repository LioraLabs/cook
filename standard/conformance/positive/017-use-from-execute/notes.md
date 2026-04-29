Pins CS-0017: `use greet` brings the alias into scope for both phases. The `> greet.say("world")` line is a `lua_line` step (execute-phase) that calls a function on the `use`-d module's table.

The parse.txt records the AST shape (one Lua step). The codegen prepends `local greet = cook.load_module("greet")` to the body unit's lua_code so the alias is bound on the worker VM at execute time. Verifying that side of the contract is the codegen harness's concern (`cook-luagen/tests/conformance.rs`); this corpus entry only pins the parser.

A runtime end-to-end check (the recipe runs and prints `hello, world`) lives in `examples/v03-phase-split/Cookfile` under recipe `module-call-execute`.
