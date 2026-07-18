§22.4, CS-0153. Pins register-phase success (`register_ok.txt`) for the
`inputs` field on `cook.add_test`: an array of file paths whose content
contributes to the test's cache key. The reference implementation has
accepted the field since COOK-84-era caching work (unioned with the
enclosing step group's dependency-output paths, deduplicated,
order-preserving — mirroring `cook.add_unit`'s `inputs`), but §22.4's
field table omitted it until CS-0153. A sibling `data.txt` accompanies
the fixture so the declared input exists on disk.

The fixture registers a Lua-body test through a `register` block (the
same hand-authored module-call path a blessed module's target maker
uses); it must parse, codegen, and register end-to-end without error.
