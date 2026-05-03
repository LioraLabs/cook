# 041 — Leveled Lua long string `[==[ … ]==]` (CS-0035)

Pins level-aware close matching for Lua long strings per the Lua 5.4 lexical
rules: `[==[ … ]==]` opens a level-2 long string; a level-0 closer `]]` (or
any closer with a different number of `=`) does not close it.

In particular, the body line `with } and ]] inside` contains a literal `]]`
which is **not** the closer of the `[==[` open. The brace counter MUST
remain inside the long string until the matching `]==]` is seen, and the
`}` byte on that line MUST be ignored as data.
