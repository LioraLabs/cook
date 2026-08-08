Pins CS-0206, and it exists because review found the reference implementation
had been silently accepting this line for as long as `use` has existed.

App. A.2's `use_declaration` ends at `NEWLINE` and admits no trailing text, and
`tree-sitter-cook` has always rejected `use cpp # …` accordingly. The Rust
lexer read the first token and discarded the rest of the line, so it accepted
it. CS-0206 gives the declaration a second argument position, which makes that
discarded remainder meaningful — the divergence had to be settled, and it is
settled toward what the grammar always said.

The diagnostic must name the COMMENT rather than count it as a further
argument: `#` opens a comment everywhere else in a Cookfile, and telling an
author their `use` "takes at most two arguments" names nothing they can act on.

The recipe body is a well-formed `cook` step, not a bare `echo`, so the only
thing that can reject this Cookfile is the rule it is here for (see `068`).
