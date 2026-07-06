# 06 — Lua recipes

Cookfiles lower to Lua; two doors let you in at two different times.

## `>{ ... }` — execute-time Lua

The step's action is Lua instead of shell. It runs sandboxed, per unit,
and caches exactly like a shell step. Pair it with a **computed output
path** — `cook (LUA_EXPR)` evaluates a Lua expression per input:

```
recipe rot13
    ingredients "docs/en/*.txt"
    cook (input:gsub("^docs/en/", "out/"):gsub("%.txt$", ".rot")) >{
        -- `input` and `output` are bound for the current unit
        ...
    }
```

Use this when the input→output mapping is a *rewrite* (subtree moves,
extension swaps) rather than a `$<in.stem>` template, or when the action
needs real logic — parsing, sorting, string surgery — that shell makes
miserable.

## `>>{ ... }` — register-time Lua

Runs **once, while the DAG is being built**. This is the escape hatch to
the low-level API — `cook.add_unit{inputs, outputs, command}` builds a unit
by hand with the same fields the surface syntax fills in for you:

```
recipe stamp
    >>{
        cook.add_unit({ name = "stamp", outputs = {"out/stamp.txt"}, command = "..." })
    }
```

Prefer the surface syntax; reach for `>>{ }` when a shape can't be
expressed yet. Everything the surface does lowers to these calls —
`cook emit-lua` shows the translation for this very file.
