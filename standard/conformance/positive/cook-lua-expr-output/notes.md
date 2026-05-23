CS-0089 — Lua-expression output for `cook` step (§8.4.2).

Pins the canonical §8.4.2 surface: a one-to-one `cook_step` whose output
slot is a parenthesised Lua expression evaluated per-ingredient with
`input` bound to the current ingredient's path.

**Status (COOK-59 Task 1).** This fixture fails today because Task 2
(parser support for `cook (EXPR) using ...`) is not yet implemented. The
v0.12 parser rejects the form with `expected quoted output pattern`. The
fixture is committed to `positive/` per spec-first convention: the
Standard's §8.4.2 grammar is the source of truth, and the reference
implementation catches up via Tasks 2 + 3 of COOK-59.

**`parse.txt` shape (informative).** The expected AST line is

```
Cook outputs=[LuaExpr("input:gsub(\"/en/\", \"/fr/\")")] using=ShellBlock(["cp $<in> $<out>"])
```

The `LuaExpr(...)` discriminator is the natural extension of the existing
`Cook outputs=[...]` renderer in `cli/crates/cook-lang/tests/conformance.rs`
once `outputs` becomes a `Vec<OutputPattern>` (`Quoted` | `LuaExpr`) per
Task 2's AST change. If Task 2 chooses a different rendering (e.g. separate
`output_kinds=[...]` line), this `parse.txt` is the authoritative target —
adjust the renderer rather than rewriting the fixture.
