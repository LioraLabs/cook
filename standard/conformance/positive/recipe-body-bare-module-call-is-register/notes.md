Pins CS-0134 §3.9: a bare `<id>.<id>(...)` line inside a recipe body
auto-classifies as a register-phase `InlineLua` step (not a `Shell` step).
The recipe body is purely declarative, so a module-call line runs at register
time. This supersedes the pre-CS-0134 behavior (which classified the bare call
as an interactive `Shell` step and required the now-removed `>>` sigil for
register-phase Lua).
