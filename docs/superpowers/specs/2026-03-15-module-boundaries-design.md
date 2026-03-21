# Module Boundaries & DDD Refactor Design

**Date:** 2026-03-15
**Goal:** Restructure Cook's modules so that each `mod.rs` is a thin facade, cross-module dependencies go through public APIs, and shared types live in a `contracts` module. The result should be an agent-friendly codebase where an agent can work on one domain without loading the entire project into context.

## Boundary Diagram

```
                         main.rs
                            |
                           cli
                     (args + dispatch)
                            |
                         engine
                      (orchestration)
                     /  |  |  |  |  \
                    /   |  |  |  |   \
                 env analyzer runtime scheduler watcher
                       |       |        |         |
                       |    contracts   |         |
                       |   (shared types)         |
                       |       |                  |
                     parser  cache                |
                       ^                          |
                       +--------------------------+
```

### Dependency Rules

- **cli** -> engine (thin shell, clap only)
- **engine** -> everything below (orchestrator)
- **runtime** -> contracts, parser, cache
- **scheduler** -> contracts, cache, runtime (pool.rs registers Lua APIs for worker threads)
- **analyzer** -> parser
- **codegen** -> parser, contracts (hash_str function)
- **watcher** -> parser (Cookfile type)
- **env** -> nothing
- **contracts** -> nothing (pure types, zero logic)
- **parser** -> nothing
- **cache** -> nothing

## Changes

### 1. New module: `contracts/`

Pure types that define the interface between runtime and scheduler. No logic, no dependencies.

```
contracts/
  mod.rs          re-exports
  work.rs         WorkPayload, CacheMeta
  capture.rs      CapturedUnit, DepKind, RecipeUnits, CaptureState, SharedCaptureState
  hash.rs         hash_str() pure function (replaces codegen's direct import of cache::check::hash_str)
```

**Moves from:** `scheduler/dag.rs` (WorkPayload, CacheMeta), `runtime/api.rs` (CapturedUnit, DepKind, RecipeUnits, CaptureState, SharedCaptureState)

### 2. Split `cli/` into `cli/` + `engine/`

```
cli/
  mod.rs          Cli struct (clap), Command enum, run() dispatches to engine

engine/
  mod.rs          re-exports
  pipeline.rs     read_and_parse(), resolve_env(), cmd_run()
  commands.rs     cmd_menu(), cmd_init(), cmd_serve()
  error.rs        CookError enum
```

### 3. Decompose `parser/mod.rs` (929 -> ~30 lines)

```
parser/
  mod.rs          pub mod + re-exports, parse() entry point
  ast.rs          (unchanged)
  lexer.rs        (unchanged)
  recipe.rs       parse_recipe(), parse_config_block()
  cook_line.rs    parse_cook_line(), parse_quoted_strings_parser(),
                  parse_single_quoted_string(), strip_keyword()
  lua_block.rs    collect_lua_block(), count_brace_delta()
  tests.rs        all unit tests
```

### 4. Decompose `codegen/mod.rs` (846 -> ~30 lines)

```
codegen/
  mod.rs          pub mod + re-exports, generate() entry point
  recipe.rs       generate() walk logic, generate_metadata()
  cook_step.rs    generate_cook_step(), cook_step_mode()
  plate_step.rs   generate_plate_step(), expand_plate_cmd()
  template.rs     expand_output_pattern(), expand_template_to_lua(),
                  expand_template_with_env_fallback()
  lua_string.rs   escape_lua_string(), wrap_lua_string()
  tests.rs        all unit tests
```

**Boundary fix:** Replace `cache::check::hash_str` import with `hash_str` from contracts.

### 5. Decompose `runtime/` (mod.rs 360 + api.rs 586 -> ~30 line facade)

```
runtime/
  mod.rs          re-exports
  engine.rs       Runtime struct, new(), set_quiet(), register_recipe()
  context.rs      setup_recipe_context()
  capture.rs      register_cook_api_capture(), register_layer_api_capture()
  fs_api.rs       register_fs_api()
  path_api.rs     register_path_api()
  tests.rs        unit tests
```

**Boundary fixes:**
- CapturedUnit, DepKind, RecipeUnits, CaptureState, SharedCaptureState -> import from contracts
- WorkPayload, CacheMeta -> import from contracts
- Cache invalidation logic -> call cache's new `invalidate_recipe()` method

### 6. Decompose `scheduler/mod.rs` (490 -> ~30 line facade)

```
scheduler/
  mod.rs          re-exports
  executor.rs     execute_dag(), process_ready(), cancel_subtree()
  builder.rs      (unchanged)
  dag.rs          (slimmed: WorkPayload/CacheMeta moved to contracts)
  pool.rs         (import paths updated: WorkPayload from contracts, Lua APIs from runtime re-exports)
  output.rs       (unchanged)
  tests.rs        all unit tests
```

**Boundary fixes:**
- Duplicated cache-update code replaced with `cache_manager.record_completion()` call
- builder.rs imports CapturedUnit/DepKind/RecipeUnits from contracts instead of runtime::api

### 7. Decompose `cache/mod.rs` (181 -> ~30 line facade)

```
cache/
  mod.rs          re-exports
  manager.rs      CacheState, SharedCacheState, ThreadSafeCacheManager
  check.rs        (unchanged)
  store.rs        (unchanged)
```

**New methods:**
- `record_completion(recipe_name, cache_key, command_hash, input_paths, output_path, working_dir)` on **ThreadSafeCacheManager** — absorbs duplicated scheduler logic
- `invalidate_recipe(env_hash, secondary_inputs_hash, glob_snapshots, working_dir) -> bool` on **CacheState** — absorbs runtime cache invalidation logic (runs in single-threaded registration phase, not the multi-threaded execution phase)

**Note:** `SharedCaptureState` is `Rc<RefCell<CaptureState>>` — it is `!Send + !Sync` and only used during the single-threaded registration phase. Add a doc comment in `contracts/capture.rs` noting this constraint.

### 8. Enforcement by example

No lints or CI checks. Instead:
- Add module structure rules to `CLAUDE.md`
- Every module demonstrates the thin-facade pattern after refactor
- Update `docs/architecture/` to reflect new structure

## Modules NOT changing internal structure

- **env/** (60 lines) — already clean
- **watcher/** (99 lines) — already clean
- **analyzer/** (41 line facade + graph.rs) — already follows the pattern
