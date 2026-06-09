# 038 — JSON object literal in cook-step shell body

Pins that a single-quoted JSON object literal (`'{"key": "value"}'`)
inside a `cook ... { ... }` body passes verbatim to the shell
as literal text. Pre-CS-0033, the `{TOKEN}` scanner would interpret the
opening `{` as the start of a placeholder, producing an error or
corrupted output. The strict `$<IDENT>` lexer (§{lexical.placeholders})
is unambiguous: the single-quoted string is opaque to the brace-depth
tracker and carries no `$<` prefix, so it is unconditionally literal
shell text.
