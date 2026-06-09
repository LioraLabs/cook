# 043 — Quoted heredoc delimiter `<<'END'` (CS-0035)

Pins delimiter-recognition for the quoted heredoc forms `<<'TAG'` and
`<<"TAG"`. The quoted form suppresses shell parameter expansion inside the
heredoc body but is otherwise scanned identically to the bare form for the
purpose of brace-balance: `}` bytes on body lines are data and do not close
the surrounding `{ … }` shell block.
