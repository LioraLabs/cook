Pins CS-0184's "positions are not literal" clause.

The output pattern carries a placeholder but no iteration driver (`$<in…>` or
`$<recipe.ACCESSOR>`). The kind classifier therefore reports `Literal`, which
the reference implementation read as "contains no placeholders" and emitted the
pattern verbatim: the unit declared the output path `out/all-$<suffix>.o`,
/bin/sh parsed the raw sigil as a redirection and wrote a file named
`out/all-$`, no diagnostic was raised, and `suffix` never entered the unit's
key — so changing it rebuilt nothing.

Classification says which iteration a pattern declares. It never said whether
the pattern needs substituting, and § 10.2.4 forbids conflating the two: the
pattern MUST lower through the same resolution a step body would apply.

Positive fixture, so the corpus asserts it parses AND lowers without error.
The substituted value and the resulting cache determinant are pinned by the
reference implementation's `sigil_agreement` tests, which the corpus cannot
express.
