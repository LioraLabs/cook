Pins the negative case for multi-line `ingredients`: any non-quote, non-`!"`
content trailing a quoted pattern (including on a continuation line) must
produce a clear diagnostic. The parser collects quoted patterns and
`!"`-excludes across continuation lines, then requires the leftover token
to be empty.

Exercises App. A.4 (ingredients_step production, multi-line whitespace rule).
