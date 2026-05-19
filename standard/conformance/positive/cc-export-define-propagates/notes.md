Pins the Cookfile surface of the explicit-public rule (CS-0080). An
`export_defines` field on cc.lib MUST propagate to consumers that
`links` it. Parse-level check; runtime locked by
`cook_cc/spec/targets_spec.lua::"export_defines on cc.lib DOES propagate to consumer compile (PUBLIC)"`.
