Pins the modifier-free `cook.add_test` lowering (CS-0135): the `test` step
no longer accepts `as`/`timeout`/`should_fail`, and codegen must not emit
`name`/`timeout`/`should_fail` fields in the generated `cook.add_test({...})`
table. Covers both surviving forms — a one-to-one shell test (`$<in>` over
the preceding `cook` step's outputs) and a single-unit many-to-one Lua test
(`inputs`).
