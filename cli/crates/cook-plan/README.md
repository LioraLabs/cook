# cook-plan

`cook-plan` turns an invocation — a cwd, a target, some flags — into a
`RegisteredWorkspace` plus a recipe edge map: everything the engine's walk
needs, before anything is spent producing outputs.

## How it does that well

- **It hands one value across the boundary.** `register_workspace` is the only
  register-phase entry point, and what it returns is the whole plan. The walk
  (`cook-engine`) consumes that value without depending on this crate: the
  handoff type lives in `cook-register`, the stratum both sides already share,
  so neither crate can reach into the other's internals without a `Cargo.toml`
  edge showing up in review (COOK-419).
- **A single Cookfile is a workspace of one member.** There is no separate
  single-Cookfile code path to drift from the imported-workspace one
  (SHI-222 / CS-0077); `RegisterMode` names the dispatch/introspect/enumerate
  target semantics instead of each caller re-deriving them.
- **Diamond imports have one name, deterministically.** `find_full_prefix`
  resolves a directory reachable through several import chains to the shortest
  alias chain from the workspace root, ties broken by declaration order
  (CS-0147) — so a recipe's qualified name cannot change because an unrelated
  import was added elsewhere.
- **Edges come only from what was declared.** The analyzer builds the recipe
  graph from `requires` and from fine-grained per-unit references; path-string
  equality between an ingredient and another recipe's output is opaque and
  creates no edge (§10.6). Reads that race writes are the walk's plan-rejection
  diagnostics, not silently inferred orderings.
- **Recipe metadata comes from registration, not from the AST.** `RecipeInfo`
  assembly reads the `RegisteredWorkspace` — what the register-phase Lua
  actually declared — so module-registered recipes and fan-out members are
  first-class, not a parse-time approximation. (A second, AST-walking assembler
  existed for `cook test` and had quietly died; COOK-419 deleted it.)
- **One error surface.** Everything here fails as `PipelineError`; `cook-cli`
  owns the single mapping onto display text and exit codes.

## What it does not do

It spends no work. Executing planned units, deciding cache hits, restoring
artifacts, and emitting progress are `cook-engine`'s walk. It renders nothing
and owns no exit codes — CLI concerns stay in `cook-cli`. It does not define
the language: parsing is `cook-lang`, codegen is `cook-luagen`, and the
register-phase VM it drives is `cook-register`'s.
