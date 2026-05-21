The `=( LUA_EXPR )` form for a defaulted parameter is reserved syntax. The
grammar (App. A.3.1) accepts it, but the current implementation rejects it
with a transient diagnostic. Task 6 of COOK-36 replaces this rejection with
a brace-balanced Lua-expression scan and execute-time evaluation against the
Cookfile-scope VM (§13.2 load phase). See §7.1.1.
