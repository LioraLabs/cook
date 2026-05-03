# 042 — Shell heredoc body with `}` brace (CS-0035)

Pins the stateful brace-balance algorithm for `using { … }` shell blocks:
when the body opens a POSIX heredoc (`cat <<EOF`), `}` bytes appearing on
heredoc-body lines are data, not the closing delimiter of the surrounding
shell block. The block closes only on a brace at depth 1 outside any open
heredoc.

The heredoc closer is matched against the **trimmed** line: this matches
the runtime semantics of `collect_shell_block`, which trims each interior
line before assembling the shell script. Authors writing heredocs inside an
indented `{ … }` body therefore do not need to dedent the closing
delimiter to column 0.
