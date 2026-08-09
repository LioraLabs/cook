Pins Standard §{lua.cook-cookfile} / CS-0208: the three ways a field scan edits
bytes the author never pointed at.

The Cookfile carries all three hazards in one call, because they are one bug —
"where does a Lua string or comment begin and end" answered at less than full
fidelity — and a fixture per symptom would suggest three:

  - `-- links = { "stale" },` is a commented-out field ABOVE the real one. A
    scan that matches a key by delimiter-and-`=` alone matches this first and
    splices into the comment, which is the thing this whole layer exists to
    preserve;
  - `opts = { links = { "nested" } }` is a key of the same name in a NESTED
    table. This is the dangerous one: editing it leaves a file that parses,
    reads correctly, and links against a list nobody edited;
  - `sources = { [[src/a}b.cpp]] }` holds a `}` inside a long-bracket string.
    A brace matcher that knows `"…"` and not `[[…]]` closes the list at that
    brace and inserts the new entry into the middle of the author's literal.

The module edits a COPY, for the reason the sibling
`cookfile-splice-preserves-comments/` fixture records: the corpus is run
repeatedly, and a fixture that edits a tracked file in place passes once and
then splices into its own output forever.

Both edits are asserted, then the whole file is checked to have grown by
exactly the two inserted strings. The length check is what makes the two
"untouched" assertions total rather than a spot check: it fails on any byte
that moved anywhere, including in a way no named assertion looks at.
