# 034 — shell brace expansion in `using` body

Pins that legitimate Bash brace expansion (`{1..3}`, `{c,h}`) inside a
`cook ... using { ... }` body passes verbatim to the shell as literal
text. Pre-CS-0033, the `{TOKEN}` placeholder scanner consumed these
sequences as env-var lookups (`cook.env["1..3"]` etc.), corrupting the
emitted command. The strict `$<IDENT>` lexer (§{lexical.placeholders})
makes the discrimination unambiguous: only `$<...>` triggers the
substitution layer; anything starting with `{` is literal shell.
