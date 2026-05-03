# 040 — Multi-line Lua block comment inside a `>{ … }` block (CS-0035)

Pins the stateful brace-balance algorithm of §{lexical.brace-blocks}: a `}`
byte appearing inside a multi-line Lua block comment `--[[ … ]]` MUST NOT
be counted as the closing delimiter of the surrounding `>{ … }` Lua block.

Companion to fixture 039: same defect class as the long-string carry-across
case, but with the `--[[` introducer.
