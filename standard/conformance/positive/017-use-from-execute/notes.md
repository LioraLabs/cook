Pins CS-0017: `use greet` brings the alias into scope for both phases. The `test >{ greet.say("world") }` step is an execute-phase Lua body that calls a function on the `use`-d module's table.

The parse.txt records the AST shape (one test step with a Lua block body). This corpus entry pins the parser and nothing else.

**Where the rest of this contract is pinned, and why it says so here.** Between 2026-07-06 and CS-0205 this note claimed the runtime half was covered twice over — by the codegen harness, and by a runtime check that "belongs to the surface e2e harness". Neither was true. `cook-luagen/tests/conformance.rs` walks this corpus asserting only that lowering does not *error*; it never reads the emitted Lua. The runtime check had lived in the `v03-phase-split` example, was deleted in the 2026-07-06 examples overhaul (COOK-194), and was never rebuilt. So this fixture's own Cookfile failed at run time — `attempt to index a nil value (global 'greet')` — for a year, with every suite green (COOK-433).

The two halves now have named, executed homes:

- **Codegen**: `cli/crates/cook-luagen/tests/use_alias_execute_binding.rs` asserts the prelude is present in the `lua_code` payload of every execute-phase body kind (cook step, test step, chore, probe `produce`), and absent when the body does not name the alias (CS-0205).
- **Runtime**: `cli/e2e-fixtures/surface/38-use-alias-in-execute-bodies/` runs the real binary over all four body kinds, and proves that editing a module reached only through the alias rebuilds the unit (the CS-0204 keying).

Deferring coverage to a place is not coverage. If a future change moves either home, change this note in the same commit.
