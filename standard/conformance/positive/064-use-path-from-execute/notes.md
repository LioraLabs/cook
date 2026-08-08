Pins CS-0206 against CS-0205: a path-form `use` brings its alias into an
execute-phase body exactly as a name form does. This is `017-use-from-execute`
over the new form, and it exists as its own entry because the two forms reach
the same binding by different derivations — the name IS its alias, a path's is
derived from its basename — and a codegen that got one right could get the
other wrong.

Parse-only, like `017`. The executed halves are named in CS-0206's conformance
section and in `017`'s notes: `cli/crates/cook-luagen/tests/use_alias_execute_binding.rs`
for the emitted prelude across every execute-phase body kind, and the surface
fixture `cli/e2e-fixtures/surface/39-use-path-form/` for the real binary,
including the CS-0204 keying — editing this module must rebuild the unit that
loaded it. Deferring coverage to a place is not coverage; if either home moves,
change this note in the same commit.
