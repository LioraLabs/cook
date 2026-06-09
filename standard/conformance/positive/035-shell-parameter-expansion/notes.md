# 035 — shell parameter expansion in cook-step shell body

Pins that POSIX shell parameter expansion forms (`${HOME:-fallback}`,
`${VAR%suffix}`) inside a `cook ... { ... }` body pass verbatim
to the shell as literal text. Pre-CS-0033, the `{TOKEN}` placeholder
scanner would attempt to resolve `HOME:-fallback` as an env-var key,
silently producing wrong output. The strict `$<IDENT>` lexer
(§{lexical.placeholders}) makes the discrimination unambiguous: the
byte sequence `${` does not begin with `$<`, so the substitution layer
is never entered; the text is literal shell.
