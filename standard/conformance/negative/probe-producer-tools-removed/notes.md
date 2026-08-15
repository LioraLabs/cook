Pins §22.5.2 and A-grammar §A.5 (CS-0222): the `tools { … }` probe producer is
removed, and the rejection MUST name the top-level `tools NAME` declaration as its
replacement.

The twin of `probe-producer-files-removed`; see its notes for why both spellings
carry a fixture. Distinct from `tools-lua-block`, `probe-as-tools-empty` and
`probe-as-tools-bad-name`, which constrain the surviving top-level `tools NAME`
declaration rather than the position it may no longer occupy.

COOK-485.
