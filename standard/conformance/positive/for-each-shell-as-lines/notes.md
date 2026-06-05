CS-0091 — `for_each` shell-capture source with `as lines` (§8.3).

Pins the `$(cmd)` register-time shell-capture source plus the `as lines`
modifier: stdout is split on newlines into raw-string members (JSON parsing
disabled). The bare `$<in>` placeholder binds the whole member (COOK-63).

**`parse.txt` shape (informative).** The `for_each` step renders as

```
ForEach source=ShellCapture("ls posts/*.md") as_lines=true
```

`ShellCapture` stores the command text without the surrounding `$( )`.
`as lines` is rejected for a `probe_ref` source (a negative parse case); it
is only meaningful for a `$(cmd)` capture, whose members are otherwise
JSON-decoded per line.
