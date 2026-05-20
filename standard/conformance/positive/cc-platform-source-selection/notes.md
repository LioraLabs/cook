Pins the Cookfile surface of the canonical per-platform source-selection
pattern blessed by CS-0081 / §28.7. The parse-level assertion is that a
top-level `register` block containing a `cook.platform.os == "linux"`
guard, an `fs.glob({...})` array-form call inside the guard, and a
`cook_cc.bin(name, { sources = ... })` target declaration is well-formed
and yields the expected sequence of `Lua` step nodes.

The runtime semantics — `cook.platform.os` returns the host OS, the
`if` branch is taken on Linux, `cook_cc.bin` records the union of
common-and-Linux sources, no per-platform-table API is consulted — are
covered by the `cook_cc` busted suite (`cook_cc/spec/targets_spec.lua`)
and by `cook-lua-stdlib`'s `cook.platform` + `fs.glob` unit tests; they
do not require a new harness entry point.

Cross-references: §28.7.1 (canonical pattern), §28.7.2 (non-pattern),
§24.6 (cook.platform), §25.7 (fs.glob).
