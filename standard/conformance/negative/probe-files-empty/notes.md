Pins §22.5.2 (CS-0148): `files { }` (empty list) is rejected — a glob list
MUST name at least one glob. The `files` twin of `probe-as-tools-empty` /
`probe-as-env-empty`; a probe whose fingerprint set is empty seals nothing
and would be a silently inert determinant.
