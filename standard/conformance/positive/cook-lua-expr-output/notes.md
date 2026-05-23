CS-0089 — Lua-expression output for `cook` step (§8.4.2).

Pins the canonical §8.4.2 surface: a one-to-one `cook_step` whose output
slot is a parenthesised Lua expression evaluated per-ingredient with
`input` bound to the current ingredient's path.

**Status (COOK-59 Task 2 landed).** This fixture now parses cleanly. Task
2 added the `OutputPattern::Quoted | OutputPattern::LuaExpr` AST split in
`cli/crates/cook-lang/src/ast.rs` and the `(EXPR)`-recognising branch in
`cli/crates/cook-lang/src/cook_line.rs::parse_cook_line`, mirroring the
balanced-paren scan used by chore default-param Lua expressions (§7.1.1).
Codegen still rejects the fixture pending COOK-59 Task 3 — the v0.13
codegen treats a LuaExpr output as a literal-mode pattern and then bails
when the recipe body uses `$<in>` placeholders. Task 3 will route the
LuaExpr form through one-to-one over own inputs codegen.

**`parse.txt` shape (informative).** The expected AST line is

```
Cook outputs=[LuaExpr("input:gsub(\"/en/\", \"/fr/\")")] using=ShellBlock(["cp $<in> $<out>"])
```

The `LuaExpr(...)` discriminator is rendered by `format_output_patterns`
in `cli/crates/cook-lang/tests/conformance.rs`; Quoted patterns keep the
bare-string `"..."` shape so every pre-COOK-59 positive fixture's
`parse.txt` continues to match unchanged.
