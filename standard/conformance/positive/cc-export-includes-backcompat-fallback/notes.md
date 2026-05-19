Locks the v0.7 backcompat carve-out (CS-0080 §28.4.1): absence of
`export_includes` MUST fall back to bare `includes` at `cook.export`
time, so consumers continue to see -Iinclude/ from a v0.6-shape
Cookfile. This is the only field where the v0.7 surface preserves
implicit-public propagation. Parse-level check; runtime locked by
`cook_cc/spec/targets_spec.lua::"export_includes absent → falls back to includes"`.
