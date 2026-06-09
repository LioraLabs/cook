# 036 — inline awk script in cook-step shell body

Pins that a single-quoted awk program (`'{print $1}'`) inside a
`cook ... { ... }` body passes verbatim to the shell as literal
text. Pre-CS-0033, the `{TOKEN}` scanner would attempt to interpret the
braces inside the awk script as a placeholder, producing a parse error
or silent corruption. The strict `$<IDENT>` lexer (§{lexical.placeholders})
is unambiguous: single-quoted text is literal shell, and no form
beginning with `{` or `'` can be a Cook placeholder.
