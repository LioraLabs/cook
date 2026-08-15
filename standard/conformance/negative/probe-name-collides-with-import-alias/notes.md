Pins the import-alias half of CS-0240's one-name-one-kind rule (App. A.2).
`$<lib.build>` would be both the qualified cross-Cookfile reference of
§{comp.qualified-refs} and member `build` of probe `lib`; rejecting the
collision is what keeps §{xref.dotted-names} deciding the reading by the base
name's KIND rather than adding a third heuristic to its two.

A `files` declaration rather than a `probe` block, because the rule binds every
declaration form that mints a key.

Semantic, not syntactic — see the sibling collision fixture.
