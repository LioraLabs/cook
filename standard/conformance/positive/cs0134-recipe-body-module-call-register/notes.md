Pins CS-0134's purely-declarative recipe body: a bare register-phase module
call in a recipe body needs no sigil at all — it auto-classifies as
register-phase Lua (`Step::InlineLua`). Covers both the single-line form
(`pnpm.run("build")`) and a multi-line call (`cook.add_unit({ … })`),
confirming the multi-line call's code is joined with `\n` and preserved
verbatim in the AST.
