# 039 — Multi-line Lua long string inside a `>{ … }` block (CS-0035)

Pins the stateful brace-balance algorithm of §{lexical.brace-blocks}: a `}`
byte appearing inside a multi-line Lua long string `[[ … ]]` MUST NOT be
counted as the closing delimiter of the surrounding `>{ … }` Lua block.

Pre-CS-0035, the brace counter was line-local: it correctly ignored braces
inside a single-line `[[ … ]]`, but did not carry the long-string state
across lines. A `}` byte on the second line of an open long string would
therefore decrement depth and prematurely close the block. This fixture
locks in the post-CS-0035 behaviour.
