Pins Standard §{lua.add-unit-after} / CS-0219: the forward-reference
diagnostic.

This is the rejection that makes acyclicity structural. Because an `after`
entry may name only an earlier unit, a cyclic edge set cannot be written down,
and this message is what enforces that — it is not a lesser stand-in for a
cycle search, it is the reason no cycle search is needed.

It must read differently from the unknown-path case, and does: a path some
later unit DOES declare is an emission order the author's own data does not
admit, while a path nobody declares is a typo. Inside the `cook.add_unit` call
the two are indistinguishable — the later unit has not been registered — which
is why §22.1.3 requires the check at the close of the recipe body.

A register-phase rejection over a Cookfile that parses and generates cleanly,
so the tree-sitter harness carries it on the semantic-only skip list.
