Pins the Cookfile surface of the array form of `fs.glob` (CS-0079). The
parse-level assertion is that a Lua step body containing an array-literal
argument to `fs.glob` is well-formed; runtime behavior (each pattern is
matched, results concatenated in call order, per-pattern CS-0064 filter)
is verified by the unit tests in `cli/crates/cook-lua-stdlib/src/fs_api.rs`
under the `static_glob_array_*` and `confined_fs_glob_array_*` cases.

Mirrors the documented signature change in Standard §25.7.
