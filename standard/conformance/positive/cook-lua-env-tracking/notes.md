CS-0090 — Lua reads of `cook.env` track to `consulted_env_keys` (§17.1).

Pins the canonical §17.1 surface: a `cook` step whose `using >{ ... }` Lua
body statically reads `cook.env.<KEY>` (or `cook.env["<KEY>"]`). Per
CS-0090, an implementation MUST scan the Lua source for these reads at
register time and fold the matched keys into the unit's
`consulted_env_keys` set, so a value change in any of those keys
invalidates the unit's cache fingerprint.

The fixture exercises both static-read shapes:

- **Dot access** — `cook.env.FOO` (the canonical, conventionally-cased
  form). The scanner accepts identifiers matching
  `[A-Z_][A-Z0-9_]*`.
- **String index, double-quoted** — `cook.env["BAR"]`. The scanner
  accepts any non-empty quoted literal as the key (no case constraint).

Conformance check (informative). The reference implementation's
codegen test
`cli/crates/cook-luagen/src/tests.rs::lua_block_step_records_static_cook_env_reads`
asserts that this fixture's emitted Lua surfaces

```lua
cook.add_unit({…, consulted_env_keys = {"BAR", "FOO"}})
```

The keys are sorted (BTreeSet) and deduplicated. CS-0090 documents the
behaviour normatively; this fixture's `parse.txt` pins only the AST shape
the codegen depends on.
