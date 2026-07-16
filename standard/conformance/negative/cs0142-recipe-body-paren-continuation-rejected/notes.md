Pins the boundary of the recipe-body `module_call` continuation rule
(COOK-246): App. A.4's `module_call` production states "Braces may span
subsequent lines" — braces only. There is no analogous rule for a
paren-continued argument list. `collect_module_call`
(`cli/crates/cook-lang/src/recipe.rs`) brace-balances via `LuaScanner`
(`cli/crates/cook-lang/src/brace_scan.rs`) and never looks at parenthesis
depth, so a call whose first line ends mid-argument-list on an open `(`
(`mymod.lib("idlib",`) is treated as balanced (zero open braces) and
collected as a complete one-line `module_call`. The next source line
(`{ sources = {"a.c"} })`) is then dispatched independently: it does not
match any of the Step-dispatch priority forms (§{steps.dispatch} rules 1-6 —
in particular it is not itself a `module_call`, since it opens with `{`, not
`BARE_IDENTIFIER "." BARE_IDENTIFIER(`), so it falls through to rule 7 and is
rejected as a loose shell command (CS-0134).

IMPORTANT: the diagnostic this fixture asserts on —
"loose shell commands are not allowed in a recipe body (CS-0134)" — is the
SAME substring asserted by the pre-existing
`cs0134-loose-shell-under-recipe-rejected` fixture. That diagnostic is
literally true of the *second* line in isolation but is misleading as an
explanation of *why* the overall paren-continued call was rejected: a reader
seeing this message would not learn that the real cause is "module calls
only continue across brace-unbalanced lines, not paren-unbalanced ones." This
fixture's purpose is to pin the paren-continuation boundary itself — the
input is rejected, and rejected via this fall-through path — not to bless
the message as an accurate or final diagnostic for this shape. A future
improvement could give paren-continuation a dedicated, more precise
diagnostic without invalidating this fixture's assertion (rejection still
occurs); see COOK-246.

Parse-only scope: no runtime module resolution is exercised.
