Pins CS-0062: `use rockmod` + `require("rockmod")` resolves
`cook_modules/share/lua/5.4/rockmod.lua` (the LuaRocks-style path).
The parse.txt records the AST shape (one `use` declaration, one
recipe with a single Lua step). The runtime end-to-end check that
require() actually resolves to the file at the LuaRocks-style path
is verified by the unit tests in `cli/crates/cook-luaotp/src/pool.rs`
(`refresh_sets_path_and_cpath_with_rock_tree_entries`,
`refresh_is_idempotent`) and by the gate-m2 chore (`cook chore
gate-m2`).
