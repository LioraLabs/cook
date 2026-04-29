# v0.3 phase-split walkthrough

Each recipe in this directory exercises one piece of the Cook Standard v0.3 surface — the `>` (execute-phase) / `>>` (register-phase) split, the recipe-body region rule, and the body-unit bundling rule. Run from this directory:

```bash
cd examples/v03-phase-split
cook <recipe-name>
```

## Positive recipes

| Recipe | Demonstrates | Expected output (key lines) |
|---|---|---|
| `register-line` | `>>` runs at register; produces no work units | `[register] selected kind: default`, `done (0/0)` |
| `register-block` | `>>{ }` runs at register; shares scope with `>>` | `[register-block] 1: step-a`, `… 2: step-b`, `… 3: step-c`, `done (0/0)` |
| `module-call` | `m.fn(...)` desugars to register-phase | `[register, via module_call] hello, world`, `done (0/0)` |
| `module-call-execute` | `> m.fn(...)` calls the same module from execute (CS-0017) | print after `queued`, `done (1/1)` (one body unit) |
| `module-via-require` | `> require("m").fn(...)` works because `package.path` is set per-unit | print after `queued`, `done (1/1)` |
| `execute-line` | `>` runs at execute (one body unit) | `queued`, then `[execute] hello from a body unit`, then `done (1/1)` |
| `execute-block` | `>{ }` runs at execute | `[execute] the answer is 42`, `done (1/1)` |
| `shell-bundle` | Adjacent shell lines coalesce; `cd` persists | First `pwd` prints `/tmp`; the `>` line breaks the coalescence; the second `pwd` prints the recipe's cwd |
| `lua-scope` | `>` lines share one Lua VM | `[execute] count: 2` (two `count = count + 1` lines saw the same local) |
| `lua-shell-mix` | Lua + shell interleave in one body unit | Three lines, in source order: shell, lua, shell |
| `interactive-breaks` | `@` ends the bundle; the next `>` gets a fresh VM | Two body units (`a = before`, `b = after`) bracketing one interactive unit |
| `full-mixed` | Declarative region (`>>` + `cook`), then imperative region (`>` + `cat`) | Register prints first, cook step runs, execute prints last |

`KIND` is read from the environment by `register-line`; try `cook --set KIND=staging register-line`.

## Negative recipes

The three reject cases are commented out at the bottom of `Cookfile`. Uncomment one at a time, run `cook bad-<thing>`, and observe the diagnostic. The expected diagnostics are:

| Recipe | Expected diagnostic |
|---|---|
| `bad-region` | `cook step on line N is not allowed after the imperative region began on line M` (Note 4.4.2) |
| `bad-using` | `` cook using: `>>{ … }` (register-phase Lua block) is not a valid using-clause payload — use `>{ … }` for an execute-phase Lua block `` (App. A.4) |
| `bad-triple-arrow` | `` a run of three or more `>` characters at line start is reserved (§{lexical.line-prefixes}) `` |

## What to read alongside this

- Standard §4.9 — the four Lua surface forms (`>` / `>{` / `>>` / `>>{`) and the body-bundling rule.
- Standard §4.4 Note 4.4.2 — the recipe-body region rule.
- Standard §8.1.2 — the phase classification table.
- Standard App. B.4.8–B.4.12 — the rationale for each design decision.
- `standard/specs/2026-04-28-recipe-body-phase-split-design.md` — the design doc that landed as CS-0015.
