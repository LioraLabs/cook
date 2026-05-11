Pins CS-0065: `use rockmod` resolves
`cook_modules/share/lua/5.4/rockmod.lua` (the LuaRocks-style path) at
register time. The `chore verify` body reads `rockmod.value` and asserts
42 — proving the register VM successfully loaded the share/lua/5.4-installed
module.

Runtime check is now embedded in this fixture. The cook-luaotp unit tests
(`refresh_sets_path_and_cpath_with_rock_tree_entries`, `refresh_is_idempotent`)
remain for the worker-side path; this fixture is the register-side counterpart.
