CS-0185 §22.4. `cook.add_test` is removed — not aliased, not deprecated.

Calling it MUST raise and MUST name the replacement. Leaving the name unbound
would satisfy "removed" but not the diagnostic requirement: Lua's `attempt to
call a nil value (field 'add_test')` reads as a Cook bug rather than a removed
API, and names nothing to move to.

Register-phase, not syntactic — `cook.add_test(...)` inside a `register` block
is ordinary Lua to the parser — so the tree-sitter harness records this as a
semantic skip.
