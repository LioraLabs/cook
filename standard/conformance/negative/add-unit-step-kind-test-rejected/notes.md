§22.1, CS-0153. `cook.add_unit({command = "true", step_kind = "test"})`
parses cleanly — `step_kind` is just a spec-table field, syntactically
indistinguishable from an accepted value — but the register pass rejects
it: a test work unit is registrable only through `cook.add_test` (§22.4).
Before CS-0153 the value was accepted as sandbox/diagnostics metadata
(CS-0135) while the unit was constructed as a plain non-test payload,
invisible to `cook test`'s payload-variant discovery — a silent no-op
that could turn a test gate green over a failing check. The diagnostic
names `step_kind` and directs the author to `cook.add_test`; the field's
surviving accepted values are `"cook"` and `"chore"`.

Because this is a register-phase-only rejection over a syntactically valid
Cookfile, it is on the tree-sitter conformance harness's skip list
(`SEMANTIC_ONLY_NEGATIVES` in `tree-sitter-cook/scripts/conformance.mjs`)
and enumerated in Appendix F §F.2.
