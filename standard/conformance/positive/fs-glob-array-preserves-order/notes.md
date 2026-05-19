Second positive fixture for CS-0079: documents the "concat in call order"
rule for the array form of `fs.glob` at the Cookfile surface. The
behavioral rule itself is locked by the unit test
`static_glob_array_order_follows_pattern_order` in
`cook-lua-stdlib::fs_api`.

This fixture's value is in the Cookfile shape: it mirrors the Doom 3
demo's platform-source pattern (see
`docs/superpowers/specs/2026-05-19-doom3-demo-roadmap-design.md` §4 and
§1.3) so a future reader can grep for `fs.glob({` in conformance/ to find
the canonical use.
