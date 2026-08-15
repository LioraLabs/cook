Pins CS-0233 (§{chores.keyword-initial}): `!"glob"` is an exclude input
(App. A.4, `input ::= STRING | "!" STRING`), so a `gather` line opening with it
is gather-shaped and stays rejected by the chore step-kind ban.

This is the one spelling where misclassifying the line would be worse than the
error it replaced. `gather` is a real program name, so a chore step
`gather !"build/*"` parses cleanly and then dies at run time with
`gather: command not found` — an error about the wrong thing, at the wrong
phase, after the chore has already started. Every other keyword-initial line
CS-0233 reclassifies was rejected before and simply runs now.

The positive counterpart is `positive/chore-keyword-initial-shell`, which pins
`gather -x logs` as a `shell_command`: a remainder that is neither a glob, an
exclude, nor a lone bare identifier.
