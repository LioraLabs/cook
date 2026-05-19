Pins the Cookfile surface of the PRIVATE-by-default rule (CS-0080). A
bare `defines` field on cc.lib MUST NOT propagate to a consumer that
`links` it. The parse-level check is that the Cookfile is well-formed
and the AST records the cook_cc.lib(...) and cook_cc.bin(...) calls as
multi-line `Shell` step sequences in each recipe body;
the runtime contract (FOO_INTERNAL is absent from the consumer's compile
command) is verified by
`cook_cc/spec/targets_spec.lua::"bare defines on cc.lib does NOT propagate to consumer compile (PRIVATE)"`.
