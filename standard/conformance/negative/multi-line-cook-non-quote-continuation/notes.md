Pins the negative case for multi-line `cook` outputs: any non-quote, non-`using`
content trailing a quoted output pattern (including on a continuation line)
must produce a clear diagnostic. The parser collects quoted patterns across
continuation lines, then requires the leftover token to be either empty
(declaration-only cook) or `using` (cook with body).

Exercises App. A.4 (cook_step production, multi-line whitespace rule).
