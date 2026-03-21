# Onboarding Guide Design

**Date:** 2026-03-15
**Purpose:** A comprehensive codebase onboarding guide for the Cook project creator to understand how the code works.

## Audience

The project owner, who vibe-coded Cook and needs a ground-up understanding of the codebase internals.

## Approach

Trace-first, then deep dives (Approach B). Start with a high-level architecture overview and an end-to-end execution trace of `cook build`, then provide deep dives into each module. This gives the reader a mental model of the whole system before zooming into specifics.

## Location

`docs/architecture/` with the following files:

```
docs/architecture/
  README.md              — High-level architecture overview + system diagram
  execution-flow.md      — End-to-end trace of `cook build`
  parser.md              — Lexer, AST, and parsing pipeline
  codegen.md             — AST-to-Lua transpilation
  runtime.md             — Lua VM setup, APIs, two-phase execution
  scheduler.md           — DAG builder, worker pool, interactive handling
  cache.md               — Hash-based caching, invalidation logic
  supporting-modules.md  — Watcher, analyzer, env loading
```

## Depth

Full depth across all modules, including the Lua transpilation pipeline, runtime internals, scheduler/parallelism system, and cache invalidation logic. Even coverage — no module gets light treatment.

## Section Details

### README.md — High-level Architecture Overview

- **What Cook is** — one-paragraph summary
- **System diagram** — pipeline visualization: `Cookfile -> Lexer -> AST -> Codegen -> Lua -> Runtime (register) -> DAG -> Scheduler (execute) -> Cache`
- **Entry files** — `main.rs` (binary entry point) and `lib.rs` (crate root) for orientation
- **Module map** — each of the 9 source modules with a one-sentence description and its dependencies:
  - `cli` — entry point, arg parsing, orchestrates everything
  - `parser` — Cookfile text -> tokens -> AST
  - `codegen` — AST -> Lua source
  - `runtime` — Lua VM setup, API registration, two-phase execution
  - `scheduler` — DAG construction, thread pool, parallel execution
  - `analyzer` — dependency resolution, topological sort
  - `cache` — hash-based incremental rebuild decisions
  - `watcher` — file system monitoring for `cook serve`
  - `env` — .env loading and 5-layer variable resolution
- **Key design decisions** — brief list of non-obvious architectural choices with pointers to relevant deep dives
- **Reading order** — recommended path: README -> execution-flow -> parser -> codegen -> runtime -> scheduler -> cache -> supporting-modules

### execution-flow.md — End-to-end Trace of `cook build`

Traces what happens from CLI invocation to completion:

1. **CLI dispatch** — `main()` calls `cli::run()`, clap parses args, defaults to recipe "build", calls `cmd_run()`
2. **Read & parse** — `read_and_parse()` reads Cookfile, lexer tokenizes line-by-line, parser builds AST
3. **Codegen** — AST transpiled to Lua. Each recipe becomes `cook.recipe(name, metadata, fn)`. Steps become `cook.exec()`, `cook.layer()`, etc.
4. **Environment resolution** — `resolve_env()` merges 5 layers: system env -> bare vars -> config block -> .env -> --set flags
5. **Dependency analysis** — `analyzer::resolve_execution_order()` does DFS topological sort using explicit deps and implicit ingredient-serves matching. Returns reachable recipes in order.
6. **Per-recipe loop** — for each recipe:
   - Phase 1 (Register): Runtime runs Lua in capture mode, `cook.layer()` records work units, produces `RecipeUnits`
   - Phase 2 (Execute): `scheduler::builder::build_dag()` constructs the DAG, then `scheduler::execute_dag()` spawns workers, executes in parallel, checks/updates cache
7. **Output** — success or failure with line-number error reporting

Includes actual function names and file paths for code navigation.

### parser.md — Lexer, AST, and Parsing Pipeline

- **Lexer**: line-by-line tokenization, token types (`RecipeHeader`, `ConfigHeader`, `VarDecl`, `Content`, etc.), `>` and `>{` for Lua mode. Note: `@` interactive prefix is NOT handled at the lexer level — lines starting with `@` are tokenized as `Content`
- **AST structures**: `Cookfile`, `Recipe`, `Step` enum (Shell, Lua, LuaBlock, Taste, Cook, Plate), `CookStep`, `UsingClause`
- **Parser**: four-scope parsing (global, config block, recipe, lua block), ordering validation (ingredients before cook/plate), brace-depth tracking with string/comment awareness. The parser strips `@` prefix from shell steps and sets `interactive: true`

### codegen.md — AST-to-Lua Transpilation

- **Recipe wrapping**: each recipe becomes `cook.recipe(name, metadata, fn)`
- **Step translation**: Shell -> `cook.exec()`, interactive -> `cook.interactive()`, Lua -> verbatim, Taste -> skipped in DAG codegen path (but `cook.taste` API still registered in runtime for the legacy single-threaded path)
- **Cook step modes**: DeclarationOnly, OneToOne (loop), ManyToOne (single invocation) with generated Lua examples
- **Template expansion**: builtins (`{in}`, `{out}`, `{stem}`, `{name}`, `{ext}`, `{dir}`, `{all}`) -> Lua variables, env vars -> `cook.env["VAR"]`
- **Cook step codegen markers**: `cook.begin_step()` / `cook.end_step()` boundaries and their role in creating parallel step groups during capture mode
- **String escaping**: quotes, backslashes, `[[...]]` vs `[=[...]=]` wrapping

### runtime.md — Lua VM Setup, APIs, Two-Phase Execution

- **Runtime struct** and Lua VM initialization
- **All registered APIs** — enumerate explicitly:
  - `cook.*`: `cook.recipe`, `cook.exec`, `cook.interactive`, `cook.sh`, `cook.taste`, `cook.env`, `cook.layer`, `cook.begin_step`, `cook.end_step`
  - `fs.*`: `fs.exists`, `fs.size`, `fs.read`, `fs.glob`, `fs.mtime`
  - `path.*`: `path.stem`, `path.name`, `path.ext`, `path.dir`, `path.replace_ext`, `path.join`
- **`cook.sh()` vs `cook.exec()`**: `cook.sh` is a convenience wrapper without a line number arg. In capture mode inside a layer it captures; outside a layer it executes immediately. This distinction must be documented.
- **Two execution modes**: capture (register) vs real (execute)
- **Capture-mode mechanics**: `cook.layer()` builds `CapturedUnit` with `DepKind`, assembles `RecipeUnits` and step groups
- **Recipe context setup**: cache invalidation checks, ingredient glob resolution

### scheduler.md — DAG Builder, Worker Pool, Interactive Handling

- **DAG structures**: `ExecutionDag`, `DagNode`, `WorkPayload` (including `WorkPayload::LuaChunk` — how Lua block cook steps carry their code, input, output, and ingredient_groups to worker threads)
- **DAG builder**: within-recipe wiring (sequential chains, parallel step groups, barriers), cross-recipe wiring (root -> leaf dependencies)
- **Worker pool**: per-thread Lua VMs, mutex/condvar queue, mpsc results
- **Execution loop**: ready queue -> dispatch -> complete -> unlock dependents
- **Interactive handling**: drain pool, run on main thread
- **Failure handling**: cancel transitive subtree, continue independent branches
- **Output serialization**: `SharedWriter` / `PrefixedWriter` mechanism for line-buffered, recipe-prefixed output (`[recipe_name] line`) during parallel execution

### cache.md — Hash-based Caching and Invalidation

- **Data structures**: `RecipeCache`, `StepEntry`, `FileRecord`
- **Cache key**: output path (cook) or input+command_hash (plate)
- **Invalidation order**: missing entry -> command hash -> output missing -> output changed -> input set changed -> input content changed -> mtime-only shortcut
- **Recipe-level invalidation**: env hash, secondary inputs hash, new glob files
- **Storage**: `.cook/cache/{recipe}.bin`, bincode, atomic tmp+rename
- **Thread-safe manager**: mutex-wrapped HashMap, dirty tracking, flush

### supporting-modules.md — Watcher, Analyzer, Env

- **Analyzer**: `RecipeInfo` (with `serves` derived from cook step output patterns), DFS topological sort, explicit vs implicit deps (exact string match, not glob), cycle/unknown detection
- **Watcher**: glob-based directory watching, 200ms debounce, Cookfile change -> full re-parse vs ingredient change -> rebuild
- **Env**: dotenvy .env loading, 5-layer resolution order
