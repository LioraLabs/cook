# Cook — Architecture Overview

## What Cook Is

Cook is a modern build system that combines Make's dependency-tracking power, Just's recipe clarity, and an embedded Lua scripting layer. It reads a `Cookfile` written in a custom DSL, transpiles it to Lua, and runs the result through a Lua VM backed by a parallel task scheduler with hash-based incremental caching. The result is a hybrid task runner and build system: you can describe file dependencies and get incremental rebuilds, or you can write pure task pipelines and get parallelism for free.

> **Language definition.** This directory documents how the implementation works. For the definition of the Cookfile language itself — syntax, semantics, Cook Lua API, modules — see `docs/standard/`.

---

## System Pipeline

```
Cookfile (text)
  → Lexer (src/parser/lexer.rs)
    → Tokens
      → Parser (src/parser/recipe.rs, cook_line.rs, lua_block.rs)
        → AST (src/parser/ast.rs)
          → Codegen (src/codegen/recipe.rs, cook_step.rs, plate_step.rs)
            → Lua source
              → Runtime (src/runtime/engine.rs) — register phase
                → RecipeUnits (src/contracts/)
                  → DAG Builder (src/scheduler/builder.rs)
                    → ExecutionDag
                      → Scheduler (src/scheduler/executor.rs) — execute phase
                        → Cache checks (src/cache/)
                          → Shell commands / Lua chunks
```

Every `cook` invocation follows this path from left to right. The split between the **register phase** and the **execute phase** is the key architectural seam: the runtime runs Lua once to discover what work exists, then hands the captured work to the scheduler which runs it in dependency order.

---

## Entry Files

**`src/main.rs`** (8 lines) — the binary entry point. It calls `cli::run()` and exits non-zero on error. Nothing else lives here.

**`src/lib.rs`** — declares the public modules that make up the library crate:

```
parser  analyzer  codegen  runtime  watcher  env  cli  engine  cache  scheduler  contracts
```

All application logic lives in those modules. The binary just calls into `cli`.

---

## Module Map

| Module | Location | What it does | Key dependencies |
|---|---|---|---|
| `cli` | `cli/crates/cook-cli/` | Thin shell: clap arg parsing, dispatches to engine, bridges `EngineEvent` → `cook_progress::ProgressEvent` for the renderer | `engine`, `cook-progress` |
| `engine::pipeline` | `cli/crates/cook-engine/src/pipeline/` | Pipeline orchestration: parse Cookfile, walk imports (`Workspace`), assemble `RegistryEntry` map, compute `{NAME}` inferred deps. Owns `PipelineError`. | `cook-lang`, `cook-luagen`, `cook-register` |
| `engine` (run + executor) | `cli/crates/cook-engine/src/{run,executor,...}.rs` | Wave-parallel DAG execution: registers each wave, builds work-unit DAG, schedules through `cook-luaotp`, evaluates cache | All other engine submodules |
| `parser` | `src/parser/` | Converts Cookfile text → tokens → AST | None |
| `codegen` | `src/codegen/` | Walks the AST and emits a Lua source string | `parser`, `contracts` |
| `runtime` | `src/runtime/` | Hosts the Lua VM, registers the Cook API, runs two-phase execution | `contracts`, `cache`, `parser` |
| `scheduler` | `src/scheduler/` | Builds the execution DAG, manages the thread pool, runs steps in parallel | `contracts`, `cache`, `runtime` |
| `contracts` | `src/contracts/mod.rs` | Shared types between runtime and scheduler (WorkPayload, CapturedUnit, etc.) | None |
| `analyzer` | `src/analyzer/` | Resolves implicit dependencies, performs topological sort | `parser` |
| `cache` | `src/cache/` | Hash-based incremental rebuild with mtime fast-path | None |
| `watcher` | `src/watcher/mod.rs` | File system monitoring for `cook --serve` | `parser` |
| `env` | `src/env/mod.rs` | Loads `.env` files into the process environment | None |

---

## Key Design Decisions

- **Two-phase execution (register then execute)** — The Lua script is run once in capture mode; Cook API calls record work rather than performing it. The scheduler then executes the captured work. This is what makes parallel scheduling possible. → see [runtime.md](runtime.md)

- **Step groups for parallelism within recipes** — Steps inside a recipe are grouped; steps in the same group can run in parallel, steps in different groups are ordered. This gives per-recipe parallelism without requiring explicit dependency declarations. → see [scheduler.md](scheduler.md)

- **Capture-mode API semantics** — During the register phase, Lua calls like `shell()` and `lua()` return immediately after recording their arguments. No side effects happen until the execute phase. → see [runtime.md](runtime.md)

- **Hash-based caching with mtime fast-path** — Cook checks file mtimes first (cheap); only if mtimes suggest a change does it compute content hashes. This keeps incremental rebuilds fast even on large trees. → see [cache.md](cache.md)

- **Interactive steps drain the thread pool** — Steps marked interactive (with `@`) cannot share the process with other concurrent work. Before running one, the scheduler drains all in-flight work and runs the interactive step on the main thread. → see [scheduler.md](scheduler.md)

- **Implicit dependencies via ingredient-serves matching** — A recipe that `serves` a string automatically becomes a dependency of any recipe that lists that string as an `ingredient`. Matching is exact string equality, not glob. → see [supporting-modules.md](supporting-modules.md)

---

## Reading Order

If you are new to the codebase, the following order lets each document build on the last:

1. **This file** — the map; read it first to orient yourself
2. [**execution-flow.md**](execution-flow.md) — end-to-end trace of what happens when you run `cook build`
3. [**parser.md**](parser.md) — the Cookfile language: tokens, grammar, and AST shape; everything else consumes these types
4. [**codegen.md**](codegen.md) — how the AST becomes Lua; short but central to understanding what the runtime actually executes
5. [**runtime.md**](runtime.md) — the Lua VM, the Cook API, and the two-phase execution model; the heart of the system
6. [**scheduler.md**](scheduler.md) — DAG construction, step groups, the thread pool, and interactive step handling
7. [**cache.md**](cache.md) — incremental rebuild: what gets hashed, how the fast-path works, when cache entries are invalidated
8. [**supporting-modules.md**](supporting-modules.md) — `analyzer`, `watcher`, and `env`; smaller modules that fill specific roles
