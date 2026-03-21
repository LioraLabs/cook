# Onboarding Guide Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Create a comprehensive codebase onboarding guide for the Cook project owner to understand how the code works.

**Architecture:** Trace-first approach — a high-level architecture overview and end-to-end execution trace, followed by deep dives into each module. 8 markdown files in `docs/architecture/`.

**Tech Stack:** Markdown documentation. Source reference: 7,563 lines of Rust across 21 files in `src/`.

**Spec:** `docs/superpowers/specs/2026-03-15-onboarding-guide-design.md`

---

## File Structure

All files are created (none exist yet). `docs/architecture/` directory must be created first.

```
docs/architecture/
  README.md              — Entry point: architecture overview, system diagram, module map
  execution-flow.md      — End-to-end trace of `cook build`
  parser.md              — Lexer, AST, parsing pipeline
  codegen.md             — AST-to-Lua transpilation
  runtime.md             — Lua VM, APIs, two-phase execution
  scheduler.md           — DAG builder, worker pool, parallelism
  cache.md               — Hash-based caching, invalidation
  supporting-modules.md  — Analyzer, watcher, env
```

Tasks follow the recommended reading order so each doc can reference prior docs.

---

## Chunk 1: Architecture Overview and Execution Trace

### Task 1: README.md — Architecture Overview

**Files:**
- Create: `docs/architecture/README.md`
- Read: `src/main.rs` (8 lines), `src/lib.rs` (9 lines), `src/cli/mod.rs:1-50` (CLI struct/args)

- [ ] **Step 1: Create the docs/architecture/ directory**

Run: `mkdir -p docs/architecture`

- [ ] **Step 2: Read entry files and CLI structure**

Read these files for accurate details:
- `src/main.rs` — binary entry point
- `src/lib.rs` — crate root, module declarations
- `src/cli/mod.rs:1-50` — CLI arg definitions

- [ ] **Step 3: Write README.md**

Create `docs/architecture/README.md` with these sections:

1. **What Cook Is** — one paragraph: modern build system combining Make's power, Just's clarity, and embedded Lua. Hybrid task runner and build system.

2. **System Pipeline** — ASCII diagram showing the data flow:
   ```
   Cookfile (text)
     → Lexer (src/parser/lexer.rs)
       → Tokens
         → Parser (src/parser/mod.rs)
           → AST (src/parser/ast.rs)
             → Codegen (src/codegen/mod.rs)
               → Lua source
                 → Runtime (src/runtime/) — register phase
                   → RecipeUnits (captured work)
                     → DAG Builder (src/scheduler/builder.rs)
                       → ExecutionDag
                         → Scheduler (src/scheduler/mod.rs) — execute phase
                           → Cache checks (src/cache/)
                             → Shell commands / Lua chunks
   ```

3. **Entry Files** — `main.rs` calls `cli::run()`, `lib.rs` declares the 9 modules

4. **Module Map** — table with module name, directory, one-sentence description, key dependencies:
   - `cli` (`src/cli/mod.rs`, 385 lines) — entry point, arg parsing, orchestrates the pipeline. Depends on: all other modules
   - `parser` (`src/parser/`, 1495 lines) — Cookfile text → tokens → AST. No internal dependencies
   - `codegen` (`src/codegen/mod.rs`, 864 lines) — AST → Lua source. Depends on: parser (AST types)
   - `runtime` (`src/runtime/`, 1689 lines) — Lua VM, API registration, two-phase execution. Depends on: cache, scheduler types
   - `scheduler` (`src/scheduler/`, 1755 lines) — DAG construction, thread pool, parallel execution. Depends on: runtime types, cache
   - `analyzer` (`src/analyzer/`, 323 lines) — dependency resolution, topological sort. Depends on: parser (AST types)
   - `cache` (`src/cache/`, 876 lines) — hash-based incremental rebuild. No internal dependencies
   - `watcher` (`src/watcher/mod.rs`, 99 lines) — file system monitoring for `cook serve`. No internal dependencies
   - `env` (`src/env/mod.rs`, 60 lines) — .env file loading. No internal dependencies

5. **Key Design Decisions** — bullet list with pointers to deep dive docs:
   - Two-phase execution (register then execute) → see runtime.md
   - Step groups for parallelism within recipes → see scheduler.md
   - Capture-mode API semantics → see runtime.md
   - Hash-based caching with mtime fast-path → see cache.md
   - Interactive steps drain the thread pool → see scheduler.md
   - Implicit dependencies via ingredient-serves matching (exact string, not glob) → see supporting-modules.md

6. **Reading Order** — recommended path through the docs with one-line description of each

- [ ] **Step 4: Commit**

```bash
git add docs/architecture/README.md
git commit -m "docs(architecture): add high-level overview and module map"
```

---

### Task 2: execution-flow.md — End-to-end Trace

**Files:**
- Create: `docs/architecture/execution-flow.md`
- Read: `src/cli/mod.rs` (385 lines — the full orchestration flow)

- [ ] **Step 1: Read the CLI orchestration code**

Read `src/cli/mod.rs` completely. Focus on:
- `read_and_parse()` (~line 121) — Cookfile → AST → Lua
- `resolve_env()` (~line 132) — 5-layer environment resolution
- `cmd_run()` (~line 187) — the main execution path, per-recipe loop with register and execute phases

- [ ] **Step 2: Write execution-flow.md**

Create `docs/architecture/execution-flow.md` tracing a `cook build` invocation through each stage. For each stage, include:
- What happens (narrative)
- Which function(s) are called (with file:line references)
- What data flows in and out
- Key code snippets where helpful

Sections:

1. **Overview** — "This traces what happens when you run `cook build` from CLI to completion"

2. **CLI Dispatch** — `main()` → `cli::run()` → clap parses args → no subcommand defaults to recipe "build" → `cmd_run()`. Reference exact lines in `cli/mod.rs`.

3. **Read & Parse** — `read_and_parse()` reads Cookfile from disk, calls `parser::parse()` which runs the lexer then parser, producing a `Cookfile` AST. Then calls `codegen::generate()` to transpile to Lua source. If `--emit-lua` flag, prints the Lua and exits.

4. **Environment Resolution** — `resolve_env()` merges 5 layers in order:
   - Layer 1: System environment (`std::env::vars()`)
   - Layer 2: Cookfile bare variables
   - Layer 3: Selected config block (if any — from CLI positional arg)
   - Layer 4: `.env` file via `env::load_env()`
   - Layer 5: `--set KEY=VALUE` CLI overrides
   Reference the exact lines where each layer is applied.

5. **Dependency Analysis** — `analyzer::resolve_execution_order()` takes the AST and target recipe, returns recipes in topological order. Only reachable recipes are included. Explain explicit deps (recipe header) vs implicit deps (ingredient-serves matching).

6. **Per-Recipe Execution Loop** — for each recipe in order:
   - **Phase 1 — Register**: `runtime.register_recipe()` runs the generated Lua in capture mode. `cook.exec()` is a no-op. `cook.layer()` records `CapturedUnit` entries. Result: `RecipeUnits` containing work units and step group info.
   - **Phase 2 — Execute**: `scheduler::builder::build_dag()` constructs the `ExecutionDag` from `RecipeUnits`. `scheduler::execute_dag()` spawns worker threads, dispatches ready nodes, runs interactive nodes on main thread, checks/updates cache per step.
   Reference the loop in `cmd_run()` and the key function calls with file:line.

7. **Output & Error Handling** — how errors propagate (line numbers from Cookfile), exit codes, what the user sees on success vs failure.

- [ ] **Step 3: Commit**

```bash
git add docs/architecture/execution-flow.md
git commit -m "docs(architecture): add end-to-end execution flow trace"
```

---

## Chunk 2: Parsing and Codegen Deep Dives

### Task 3: parser.md — Lexer, AST, and Parsing Pipeline

**Files:**
- Create: `docs/architecture/parser.md`
- Read: `src/parser/lexer.rs` (404 lines), `src/parser/ast.rs` (151 lines), `src/parser/mod.rs` (940 lines)

- [ ] **Step 1: Read all parser source files**

Read completely:
- `src/parser/lexer.rs` — tokenization logic
- `src/parser/ast.rs` — data structures
- `src/parser/mod.rs` — parsing logic

- [ ] **Step 2: Write parser.md**

Create `docs/architecture/parser.md` with these sections:

1. **Overview** — the parser turns Cookfile text into a structured AST in two stages: lexing (text → tokens) and parsing (tokens → AST)

2. **Lexer** (`src/parser/lexer.rs`):
   - Line-by-line tokenization via `tokenize()` function
   - Token enum variants with descriptions: `Comment`, `RecipeHeader {name, deps}`, `ConfigHeader {name}`, `VarDecl {name, value}`, `RecipeEnd`, `LuaLine`, `LuaBlockOpen`, `Taste`, `Blank`, `Content`
   - How each line type is recognized (keywords, prefixes like `>`, `>{`, `#`)
   - Important: `@` interactive prefix is NOT a lexer concern — lines with `@` become `Content` tokens
   - Include example: show a Cookfile snippet and the tokens it produces

3. **AST Structures** (`src/parser/ast.rs`):
   - `Cookfile` — top-level: `vars`, `configs`, `recipes`
   - `Recipe` — `name`, `deps`, `ingredients`, `steps`, `line`
   - `Step` enum — `Shell {command, line, interactive}`, `Lua {code, line}`, `LuaBlock {code, line}`, `Taste {line}`, `Cook {step: CookStep, line}`, `Plate {step: PlateStep, line}`
   - `CookStep` — `output_pattern`, `using_clause: Option<UsingClause>`
   - `PlateStep` — `command`, `using_clause`
   - `UsingClause` enum — `Shell(String)`, `LuaBlock(String)`

4. **Parser** (`src/parser/mod.rs`):
   - Four parsing scopes: global (vars + configs), config block, recipe, lua block
   - Global scope: collects `VarDecl` and `ConfigHeader` tokens. Variables and configs must appear before any recipe.
   - Config block scope: collects variable declarations until `RecipeEnd`
   - Recipe scope (`parse_recipe()`): processes `Content` tokens into steps. Validates ordering — ingredients must come before cook/plate steps.
   - Lua block scope (`collect_lua_block()`): collects raw source lines, tracks brace depth. Ignores braces inside strings, comments, and Lua long strings (`[[...]]`). Ends when brace depth returns to 0.
   - How `@` is handled: parser checks if Content line starts with `@`, strips prefix, creates `Step::Shell { interactive: true }`
   - How `ingredients`, `cook`, `plate` lines are parsed from Content tokens
   - Error handling: `CookError` variants for parse failures with line numbers

- [ ] **Step 3: Commit**

```bash
git add docs/architecture/parser.md
git commit -m "docs(architecture): add parser deep dive"
```

---

### Task 4: codegen.md — AST-to-Lua Transpilation

**Files:**
- Create: `docs/architecture/codegen.md`
- Read: `src/codegen/mod.rs` (864 lines)

- [ ] **Step 1: Read the codegen source**

Read `src/codegen/mod.rs` completely. Focus on:
- `generate()` (line 14) — entry point, recipe generation logic is inline here
- `generate_metadata()` (line 82) — builds the metadata table for each recipe
- `generate_cook_step()` (line 121) — the complex cook step codegen
- `expand_template_to_lua()` (line 293) / `expand_template_with_env_fallback()` (line 316) — template expansion
- `escape_lua_string()` (line 369) / `wrap_lua_string()` (line 375) — string escaping

- [ ] **Step 2: Write codegen.md**

Create `docs/architecture/codegen.md` with these sections:

1. **Overview** — codegen transforms the AST into Lua source code that the runtime can execute. Each recipe becomes a `cook.recipe()` call wrapping a function body.

2. **Recipe Wrapping** — the `generate()` function produces per-recipe Lua inline (with `generate_metadata()` for the metadata table):
   ```lua
   cook.recipe("name", {ingredients = {"glob1", "glob2"}, requires = {"dep1"}}, function()
       -- steps
   end)
   ```
   Explain the metadata table structure.

3. **Step Translation Rules** — how each `Step` variant maps to Lua:
   - `Shell` → `cook.exec([[command]], line)`
   - `Shell { interactive: true }` → `cook.interactive([[command]], line)`
   - `Lua` → verbatim single line
   - `LuaBlock` → verbatim multi-line
   - `Taste` → skipped (comment: "will be redesigned for threaded model"). Note: `cook.taste` API still exists in runtime for legacy path.
   - `Cook` → complex, see next section
   - `Plate` → similar to cook but for post-processing

4. **Cook Step Modes** — the `generate_cook_step()` function (line 121) determines mode based on `using_clause`:
   - **DeclarationOnly** (`using_clause: None`) — just declares outputs: `local _cook_outputs_N = {"pattern"}`
   - **OneToOne** (`using_clause` with `{in}` in shell, or any LuaBlock) — loops over inputs, one output per input. Show the generated Lua loop with `cook.layer()` call.
   - **ManyToOne** (`using_clause` shell without `{in}`) — single invocation with all inputs. Show the generated Lua with `{all}` expansion.

   For each mode, include the complete generated Lua as an example.

5. **`cook.begin_step()` / `cook.end_step()` Markers** — these wrap cook steps and create step group boundaries. During capture mode, `begin_step()` opens a new step group and `end_step()` closes it. Units within a group can run in parallel.

6. **Template Expansion** — two phases:
   - Builtin variables (`expand_template_to_lua()`, line 293): `{in}` → `_cook_in`, `{out}` → `_cook_out`, `{stem}` → `_cook_stem`, `{name}` → `_cook_name`, `{ext}` → `_cook_ext`, `{dir}` → `_cook_dir`, `{all}` → `_cook_all`. These become Lua variable references.
   - Environment variables (`expand_template_with_env_fallback()`, line 316): any remaining `{VAR}` → `cook.env["VAR"]` via string concatenation.

7. **String Escaping** — two functions: `escape_lua_string()` (line 369) handles quotes/backslashes/newlines for double-quoted Lua strings. `wrap_lua_string()` (line 375) chooses between `[[...]]` and `[=[...]=]` based on whether the content contains `]]`.

- [ ] **Step 3: Commit**

```bash
git add docs/architecture/codegen.md
git commit -m "docs(architecture): add codegen deep dive"
```

---

## Chunk 3: Runtime and Scheduler Deep Dives

### Task 5: runtime.md — Lua VM, APIs, Two-Phase Execution

**Files:**
- Create: `docs/architecture/runtime.md`
- Read: `src/runtime/mod.rs` (858 lines), `src/runtime/api.rs` (831 lines)

- [ ] **Step 1: Read all runtime source files**

Read completely:
- `src/runtime/mod.rs` — Runtime struct, register/execute modes, recipe context
- `src/runtime/api.rs` — all API registrations

- [ ] **Step 2: Write runtime.md**

Create `docs/architecture/runtime.md` with these sections:

1. **Overview** — the runtime manages the Lua VM, registers Cook's built-in APIs, and implements two execution modes (capture for planning, real for execution).

2. **Runtime Struct** — fields: `working_dir`, `env_vars`, `no_taste`, `quiet`. How it's constructed in `cmd_run()`.

3. **Lua VM Initialization** — how `mlua::Lua` is created, how the runtime registers all API namespaces.

4. **API Reference** — comprehensive list of every registered function:

   **`cook.*` namespace:**
   - `cook.recipe(name, metadata, fn)` — registers a recipe; metadata has `ingredients` and `requires`
   - `cook.exec(cmd, line)` — execute shell command, return stdout. In capture mode: no-op (or captured inside a layer)
   - `cook.interactive(cmd, line)` — execute with inherited stdio (main thread only)
   - `cook.sh(cmd)` — convenience wrapper for exec without line number. Behaves differently in capture mode: inside a layer it captures, outside a layer it executes immediately
   - `cook.taste(line)` — debugger breakpoint placeholder (legacy, skipped in DAG path)
   - `cook.env` — table of resolved environment variables
   - `cook.layer(inputs, output, cmd_hash, fn)` — cache-aware execution wrapper. In capture mode: records a `CapturedUnit`. In execute mode: checks cache, runs fn if needed.
   - `cook.begin_step()` / `cook.end_step()` — step group boundaries for parallelism

   **`fs.*` namespace:**
   - `fs.exists(path)` → bool
   - `fs.size(path)` → u64
   - `fs.read(path)` → string contents
   - `fs.glob(pattern)` → table of matching paths
   - `fs.mtime(path)` → modification time as seconds (f64)

   **`path.*` namespace:**
   - `path.stem(p)` → filename without extension
   - `path.name(p)` → basename (filename with extension)
   - `path.ext(p)` → extension including dot
   - `path.dir(p)` → parent directory
   - `path.replace_ext(p, ext)` → path with new extension
   - `path.join(a, b)` → concatenated path

5. **Two Execution Modes**:
   - **Capture mode** (`register_recipe()`): Lua runs but side effects are suppressed. `cook.exec()` is a no-op. `cook.layer()` records `CapturedUnit` entries instead of executing. Purpose: discover what work needs to be done without doing it.
   - **Execute mode** (`execute_recipe()`): Lua runs with real side effects. `cook.exec()` runs commands. `cook.layer()` checks cache and executes if needed. Used by the legacy single-threaded path.

6. **Capture-Mode Data Structures**:
   - `CapturedUnit` — `payload: WorkPayload`, `cache_meta: Option<CacheMeta>`, `dep_kind: DepKind`
   - `DepKind::StepGroup(usize)` — can run parallel with siblings in same group
   - `DepKind::Sequential` — depends on all prior units
   - `RecipeUnits` — `recipe_name`, `deps`, `units: Vec<CapturedUnit>`, `step_groups: Vec<Vec<usize>>`
   - How step groups are built: `begin_step()` opens a group, each `layer()` call adds to it, `end_step()` closes it

7. **Recipe Context Setup** (`setup_recipe_context()`):
   - Cache invalidation: checks env hash and secondary inputs hash against stored values
   - Ingredient glob resolution: expands glob patterns relative to working dir, populates `recipe.ingredients` table
   - Records glob results for new-file detection

- [ ] **Step 3: Commit**

```bash
git add docs/architecture/runtime.md
git commit -m "docs(architecture): add runtime deep dive"
```

---

### Task 6: scheduler.md — DAG Builder, Worker Pool, Parallelism

**Files:**
- Create: `docs/architecture/scheduler.md`
- Read: `src/scheduler/mod.rs` (490 lines), `src/scheduler/dag.rs` (249 lines), `src/scheduler/builder.rs` (259 lines), `src/scheduler/pool.rs` (551 lines), `src/scheduler/output.rs` (206 lines)

- [ ] **Step 1: Read all scheduler source files**

Read completely:
- `src/scheduler/builder.rs` — DAG construction from RecipeUnits
- `src/scheduler/dag.rs` — DAG data structures and operations
- `src/scheduler/mod.rs` — execution loop, interactive handling
- `src/scheduler/pool.rs` — worker thread pool
- `src/scheduler/output.rs` — SharedWriter/PrefixedWriter

- [ ] **Step 2: Write scheduler.md**

Create `docs/architecture/scheduler.md` with these sections:

1. **Overview** — the scheduler takes captured work units from the runtime, builds a dependency DAG, and executes them in parallel using a thread pool. Interactive steps are special-cased to run on the main thread.

2. **DAG Data Structures** (`src/scheduler/dag.rs`):
   - `ExecutionDag` — vector of `DagNode`
   - `DagNode` — `id`, `payload: Option<WorkPayload>` (None = cached/pre-satisfied), `recipe_name`, `cache_meta`, `dependents: Vec<usize>`, `remaining_deps: AtomicUsize`
   - `WorkPayload` enum:
     - `Shell { cmd, line }` — shell command
     - `Interactive { cmd, line }` — shell command needing inherited stdio
     - `LuaChunk { code, input, output, ingredient_groups }` — Lua block cook step; carries everything needed for a worker thread's isolated Lua VM to execute it

3. **DAG Builder** (`src/scheduler/builder.rs`):
   - `build_dag()` takes a slice of `RecipeUnits` (already in topological order)
   - **Within-recipe wiring**: Sequential units form a chain — each depends on the previous "barrier" node. StepGroup units share the same barrier (can run in parallel). When the last member of a group completes, that group becomes the new barrier.
   - **Cross-recipe wiring**: Root nodes of a recipe (those with no within-recipe deps) additionally depend on leaf nodes of all prerequisite recipes.
   - **Cache pre-satisfaction**: If `cache_meta` indicates the step is cached, the node's payload is set to `None` and it's immediately satisfiable.
   - Include a diagram showing how units wire together.

4. **Worker Pool** (`src/scheduler/pool.rs`):
   - Architecture: N worker threads (configurable via `-j`), each with its own `mlua::Lua` VM
   - Per-thread setup: creates Lua VM, registers `fs.*` and `path.*` APIs, registers `cook` table with closures for exec/interactive/env
   - Shared work queue: `Mutex<VecDeque<WorkItem>>` + `Condvar` for wake-up
   - Result channel: `mpsc::Sender<WorkResult>` per thread → single `mpsc::Receiver` on main thread
   - `WorkItem` contains: node ID, payload, recipe name, env vars, quiet flag, writer
   - `WorkResult` contains: node ID, success/failure, optional error message

5. **Execution Loop** (`src/scheduler/mod.rs`, `execute_dag()`):
   - Initialize: `dag.initial_ready()` returns nodes with `remaining_deps == 0`
   - For each ready node: call `process_ready()` which either dispatches to pool (Shell/LuaChunk) or queues for main thread (Interactive)
   - Main loop: receive `WorkResult` from pool → `dag.complete(id)` decrements dependent counts → newly-ready dependents get dispatched
   - When pool has no pending work and interactive queue is non-empty: drain pool, run interactive nodes sequentially on main thread with inherited stdio, then resume pool
   - Loop ends when all nodes are completed or cancelled

6. **Interactive Step Handling**:
   - Interactive nodes (`WorkPayload::Interactive`) cannot run on worker threads (they need stdin/stdout)
   - They're queued in `interactive_queue` and only run when the pool is idle
   - The execution loop waits until `pending == 0` (all in-flight work has completed), then runs interactive nodes on the main thread while pool threads sit idle — there is no explicit `drain()` method
   - After running, DAG is updated and dependent work resumes

7. **Failure Handling**:
   - When a node fails, all transitive dependents are cancelled via `dag.cancel_subtree()`
   - Independent branches continue executing
   - Final result reports which recipes failed

8. **Output Serialization** (`src/scheduler/output.rs`):
   - `SharedWriter` — thread-safe writer backed by `Arc<Mutex<...>>`
   - `PrefixedWriter` — wraps SharedWriter, prefixes each line with `[recipe_name]`
   - Ensures parallel output from multiple workers doesn't interleave within lines
   - Each worker gets a `PrefixedWriter` with the recipe name of the current node

- [ ] **Step 3: Commit**

```bash
git add docs/architecture/scheduler.md
git commit -m "docs(architecture): add scheduler deep dive"
```

---

## Chunk 4: Cache and Supporting Modules

### Task 7: cache.md — Hash-based Caching and Invalidation

**Files:**
- Create: `docs/architecture/cache.md`
- Read: `src/cache/mod.rs` (181 lines), `src/cache/check.rs` (494 lines), `src/cache/store.rs` (201 lines)

- [ ] **Step 1: Read all cache source files**

Read completely:
- `src/cache/mod.rs` — CacheManager, ThreadSafeCacheManager, RecipeCache structures
- `src/cache/check.rs` — invalidation logic (needs_rebuild_cook, needs_rebuild_plate)
- `src/cache/store.rs` — persistent storage (load/save)

- [ ] **Step 2: Write cache.md**

Create `docs/architecture/cache.md` with these sections:

1. **Overview** — Cook uses content-hash-based caching to skip rebuilding steps whose inputs and outputs haven't changed. Cache is per-recipe, per-step, stored on disk.

2. **Data Structures** (`src/cache/mod.rs`):
   - `RecipeCache` — `version`, `globs: HashMap<String, Vec<String>>`, `secondary_inputs_hash: u64`, `env_hash: u64`, `steps: HashMap<String, StepEntry>`
   - `StepEntry` — `inputs: Vec<FileRecord>`, `output: Option<FileRecord>`, `command_hash: u64`
   - `FileRecord` — `path: String`, `mtime: u64` (millisecond resolution), `hash: u64` (xxh3_64 of file contents)

3. **Cache Keys**:
   - Cook steps: keyed by output path
   - Plate steps: keyed by input paths + command hash

4. **Invalidation Logic** (`src/cache/check.rs`) — checks run in this order, short-circuiting on first rebuild trigger:
   1. No cache entry exists → rebuild
   2. Command hash changed (the build command itself changed) → rebuild
   3. Output file missing from disk → rebuild
   4. Output file content changed (mtime or hash differs from cached) → rebuild
   5. Input set changed (number of inputs differs from cached) → rebuild
   6. Any input file content changed (mtime or hash differs) → rebuild
   7. Input mtime changed but hash is same → update mtime in cache, skip rebuild (fast path: file was touched but not modified)
   8. All checks pass → skip (fully cached)

5. **Recipe-Level Invalidation** — before checking individual steps:
   - Environment hash: if the hash of all env vars changed since last run, clear the entire recipe cache
   - Secondary inputs hash: if hash of secondary ingredient files changed, clear recipe cache
   - New files in ingredient globs: if glob expansion finds files not in the cached glob results, remove entries for deleted files

6. **Persistent Storage** (`src/cache/store.rs`):
   - Location: `.cook/cache/{recipe_name}.bin`
   - Format: bincode serialization of `RecipeCache`
   - Atomic writes: write to `{recipe_name}.bin.tmp`, then `fs::rename` to final path
   - Load: deserialize from disk, return empty cache if file missing or version mismatch

7. **Thread-Safe Manager** (`ThreadSafeCacheManager`):
   - Wraps `HashMap<String, RecipeCache>` in `Mutex`
   - Tracks dirty recipes in `HashSet<String>`
   - `update_step()` — updates a step entry AND internally marks the recipe as dirty (no separate `mark_dirty()` method)
   - `flush_all()` — writes all dirty recipes to disk at end of build

- [ ] **Step 3: Commit**

```bash
git add docs/architecture/cache.md
git commit -m "docs(architecture): add cache deep dive"
```

---

### Task 8: supporting-modules.md — Analyzer, Watcher, Env

**Files:**
- Create: `docs/architecture/supporting-modules.md`
- Read: `src/analyzer/mod.rs` (41 lines), `src/analyzer/graph.rs` (282 lines), `src/watcher/mod.rs` (99 lines), `src/env/mod.rs` (60 lines)

- [ ] **Step 1: Read all supporting module source files**

Read completely:
- `src/analyzer/mod.rs` + `src/analyzer/graph.rs` — dependency resolution
- `src/watcher/mod.rs` — file watching
- `src/env/mod.rs` — .env loading

- [ ] **Step 2: Write supporting-modules.md**

Create `docs/architecture/supporting-modules.md` with these sections:

1. **Analyzer** (`src/analyzer/`):

   - **Purpose**: determines recipe execution order via topological sort. This runs early in the pipeline — before any recipe is executed.

   - **RecipeInfo** struct: `ingredients` (glob input patterns), `serves` (output paths from cook steps), `requires` (explicit dependencies from recipe header)

   - **How `serves` is derived**: the analyzer extracts output patterns from all `Cook` steps in a recipe. E.g., a recipe with `cook "build/{stem}.o"` serves `"build/{stem}.o"`.

   - **Dependency Types**:
     - Explicit: recipe header `recipe "build": "setup"` → "build" requires "setup"
     - Implicit: if recipe A serves `"lib.a"` and recipe B has ingredient `"lib.a"`, B depends on A. Important: this is **exact string matching only**, not glob matching. `"src/*.c"` will never trigger an implicit dependency.

   - **Topological Sort**: DFS-based. Starts from the target recipe, walks dependencies recursively, returns only reachable recipes in execution order.

   - **Error Detection**: `CycleDetected` if a recipe depends on itself (directly or transitively), `UnknownRecipe` if a dependency names a recipe that doesn't exist.

2. **Watcher** (`src/watcher/mod.rs`):

   - **Purpose**: powers `cook serve` — watches files and re-runs recipes when they change.

   - **CookWatcher** struct: `globs` (collected from all recipes in execution order), `cookfile_path`

   - **Setup**: collects all ingredient glob patterns, resolves them to directories, watches those directories recursively using the `notify` crate. Also watches the Cookfile's directory (non-recursive).

   - **Debounce**: 200ms — prevents multiple rapid file changes from triggering multiple rebuilds.

   - **Change Types**:
     - Cookfile changed → full re-parse: read Cookfile again, re-lex, re-parse, re-generate Lua, then rebuild
     - Ingredient file changed → rebuild with existing Cookfile (skip re-parse)

   - **Interactive rejection**: `cook serve` rejects recipes containing `@` interactive steps (added recently, see `cli/mod.rs`).

3. **Env** (`src/env/mod.rs`):

   - **Purpose**: loads `.env` file from the working directory.

   - **Implementation**: uses `dotenvy` crate. `load_dotenv()` returns `HashMap<String, String>` or empty map if no `.env` file exists.

   - **5-Layer Resolution** (handled in `cli/mod.rs` `resolve_env()`, not in this module):
     1. System environment (`std::env::vars()`) — lowest priority
     2. Cookfile bare variables (`CC "gcc"`)
     3. Selected config block (`config "debug" ... end`)
     4. `.env` file
     5. `--set KEY=VALUE` CLI flags — highest priority

   Each layer overrides the previous. The result is a single `HashMap<String, String>` passed to the runtime.

- [ ] **Step 3: Commit**

```bash
git add docs/architecture/supporting-modules.md
git commit -m "docs(architecture): add supporting modules deep dive"
```
