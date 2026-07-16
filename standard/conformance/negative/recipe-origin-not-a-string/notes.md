§22.3, CS-0143 (applying the field-typing discipline CS-0127 established
for `cook.add_unit`). `cook.recipe("build", {..., origin = 42}, fn)` parses
cleanly — the `origin` field is just a metadata-table entry, syntactically
indistinguishable from a string-valued one — but the register pass rejects
it: `origin` must be a Lua string, not silently coerced from a number.
`parse_origin_meta` (`cook-register/src/capture.rs`) matches on `LuaValue`
specifically to avoid mlua's `String: FromLua` numeric-coercion path, so
`42` is rejected outright rather than becoming `"42"`.

Because this is a register-phase-only rejection over a syntactically valid
Cookfile, it is on the tree-sitter conformance harness's skip list
(`SEMANTIC_ONLY_NEGATIVES` in `tree-sitter-cook/scripts/conformance.mjs`)
and enumerated in Appendix F §F.2.
