Pins §A.3.2 / §22.5.2 (CS-0181): a `tools` name is a `TOOL_NAME`
(= `PROBE_SEG`), so an executable carrying internal `-` or `.` is
spellable — `tree-sitter`, `pkg-config`, `llvm-config`. Before CS-0181
the list took the narrow `IDENT` and none of these parsed, which left
`§22.5.4`'s own worked example (`"pkg-config"`) unwritable in the
native probe surface.

The `[A-Za-z_]` head is unchanged, so `probe-as-tools-bad-name`
(`tools { cc --version }`) stays a rejection. `envs` is NOT widened:
see `cook-disposition-seal-envs-probe`.
