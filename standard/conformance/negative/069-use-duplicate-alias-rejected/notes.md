Pins CS-0206. Two `use` declarations may not bind one identifier to two
different targets, for the reason C.11.1 gives for duplicate `import`:
last-wins would silently redirect every call through the alias, and the
mistake is cheapest to see at the line that made it.

Two declarations naming the SAME target are NOT a conflict — §12.3.2 already
makes the second a memo hit — so this fixture deliberately uses two different
targets under one name.

The recipe body is a well-formed `cook` step for the reason given in
`068`'s notes: a bare `echo unreachable` is itself a CS-0134 rejection and
would have masked the absence of this rule entirely.
