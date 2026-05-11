# Scheduling Layer: Recipe Waves, Work-Unit DAG, Worker Pool

**Source crates:**
- `cli/crates/cook-dag/src/lib.rs` — generic `Dag<T>` (~636 lines incl. tests)
- `cli/crates/cook-engine/src/` — Cook-specific orchestration (~6.6k lines across 9 files)
- `cli/crates/cook-luaotp/src/pool.rs` — `WorkerPool` and worker threads (~1.7k lines)
- `cli/crates/cook-contracts/src/lib.rs` — shared `WorkPayload`, `DepKind`, `CacheMeta`, `RecipeUnits`, `OutputStream`

---

## 1. Overview

There is no single "scheduler" module any more. Scheduling is split across four crates that compose into a pipeline:

```
   targets + recipe_infos
            │
            ▼
   ┌──────────────────────┐
   │ wave_grouper         │  partition recipes into ordered waves
   │   (cook-engine)      │     ──> Vec<Wave>
   └──────────────────────┘
            │
            ▼  for each wave:
   ┌──────────────────────┐
   │ register_recipe(...) │  evaluate registration-phase Lua per recipe
   │   (cook-register     │     ──> RecipeUnits with CapturedUnit list
   │    via RegistryEntry)│
   └──────────────────────┘
            │
            ▼
   ┌──────────────────────┐
   │ dag_builder          │  RecipeUnits ──> Dag<WorkNode>
   │   (cook-engine)      │     barriers + step groups + cross-recipe edges
   └──────────────────────┘
            │
            ▼
   ┌──────────────────────┐
   │ executor::execute_dag│  seed pool, run loop, chore drains, cache updates
   │   (cook-engine)      │     ──> emits EngineEvent through mpsc
   └──────────────────────┘
            │              ▲
            │              │ WorkResult (id, success, output_lines, ...)
            ▼              │
   ┌──────────────────────┐
   │ WorkerPool           │  N threads, each owns a !Send mlua::Lua VM
   │   (cook-luaotp)      │     executes Shell / LuaChunk / Test payloads
   └──────────────────────┘
```

The top-level entry point is `cook_engine::run::run()` (and `run_for_test()` for `cook test`), defined at `cli/crates/cook-engine/src/run.rs:314` and `:46`. Both return `RunResult { test_results }` on success and `EngineError` on failure.

Two architectural points are worth keeping in mind throughout:

1. **The work DAG is generic.** `cook-dag::Dag<T>` knows nothing about Cook. Cook-specific payload lives in `cook_engine::WorkNode` (`cli/crates/cook-engine/src/lib.rs:62`), which is what the engine instantiates `Dag<T>` with.
2. **Waves are the granularity of registration.** Registration-phase Lua needs its own `!Send` `mlua::Lua` VM and is serialised per recipe; the wave abstraction partitions recipes into ordered groups where members of a wave have no recipe-level ordering constraint between them. After a wave's recipes are registered, the work-unit DAG for that wave is built and run to completion before the next wave starts.

---

## 2. Generic `Dag<T>` (`cook-dag`)

`cook_dag::Dag<T>` is a behaviour-free directed-acyclic graph with topological traversal support. It has no knowledge of Cook semantics — it just stores nodes, dependency edges, and `AtomicUsize` counters.

### Core API

| Method | Behaviour | Source |
|---|---|---|
| `Dag::new()` | Empty DAG | `cook-dag/src/lib.rs:147` |
| `add_node(payload: T, depends_on: &[usize]) -> Result<usize, DagError>` | Append a node, dedupe duplicate deps via `BTreeSet`, wire forward edges, return its id | `cook-dag/src/lib.rs:159` |
| `validate() -> Result<(), CycleError>` | Kahn's algorithm; on cycle, walks unconsumed predecessors to surface one concrete cycle path | `cook-dag/src/lib.rs:201` |
| `initial_ready() -> Vec<usize>` | All nodes where `remaining_deps == 0` (the roots) | `cook-dag/src/lib.rs:302` |
| `complete(id) -> Vec<usize>` | Atomic `fetch_sub(1, SeqCst)` on each dependent's `remaining_deps`; returns dependents whose previous value was 1 (i.e. just became ready) | `cook-dag/src/lib.rs:315` |
| `node(id) -> &Node<T>` | Read-only access | `cook-dag/src/lib.rs:334` |

### Concurrency properties

- `Dag<T>` is `Send + Sync` when `T: Send + Sync` (asserted by `dag_is_send_and_sync` in the test module at `cook-dag/src/lib.rs:629`).
- `complete()` uses `Ordering::SeqCst` so multiple worker threads can call it concurrently on different node ids without external locking. The thread that observes `prev == 1` is the unique unlocker of that dependent.
- `add_node()` returns `DagError::DependencyOutOfRange` when a dep id has not yet been inserted; on error the DAG is left unchanged. Self-references and forward references are both caught by the same range check.

### Cycle reporting

`CycleError` (`cook-dag/src/lib.rs:80`) carries:
- `cycle_path: Vec<usize>` — a concrete `[v_0, …, v_k]` with the implicit closing edge `v_k → v_0`, in dependency order (`v_i` depends on `v_{i+1}`).
- `blocked: usize` — number of nodes part of, or transitively downstream of, the cycle.

The engine calls `dag.validate()` defensively at the top of `execute_dag` (`cli/crates/cook-engine/src/executor.rs:312`); the work-DAG builder cannot construct a cycle today (every dep id was emitted earlier in the same pass), but the validation is cheap insurance against a future builder bug.

---

## 3. Recipe DAG and waves (`cook-engine`)

The recipe-level scheduling layer sits above the work-unit DAG. Two structures cooperate:

### `RecipeDag` — wave-by-wave readiness tracking

`cli/crates/cook-engine/src/recipe_dag.rs:22` — a much simpler structure than the work DAG. Each recipe is a node; nodes track `remaining_deps`, `in_flight`, and `done` flags. The API is:

- `RecipeDag::new(dep_edges: &BTreeMap<String, Vec<String>>)`
- `pop_ready() -> Vec<String>` — returns all recipes whose deps are satisfied and which are not yet in-flight or done, and flips them to `in_flight`.
- `mark_done(names: &[String])` — flips `in_flight → done` and decrements `remaining_deps` on dependents.

This struct is the abstract pattern; in practice the unified entry point in `run.rs` does not use `RecipeDag` directly because it pre-computes the full wave list up front via `wave_grouper`.

### `wave_grouper::compute_waves` — two-tier wave assignment

`cli/crates/cook-engine/src/wave_grouper.rs:28` partitions recipes into ordered `Wave`s using two kinds of edge:

| Edge | Semantics |
|---|---|
| **Explicit** (`: dep`) | Wave boundary. If B explicitly depends on A, A's wave is strictly earlier than B's. |
| **Inferred** (`{dep}` references) | Same-wave merging. If B references `{A}`, A and B land in the same wave with A registered first. Transitive inferred deps collapse into one wave (C → B → A all-inferred ⇒ one wave). |

The algorithm:
1. Build inferred-edge connected components (undirected BFS) — each component is a same-wave group.
2. Build inter-group edges from explicit deps that cross group boundaries.
3. Kahn-toposort the groups; a group's wave level is `max(dep_wave_levels) + 1`, or 0 if it has none.
4. Within each wave, concatenate groups in toposorted order, and within each group, toposort recipes by directed inferred edges so dependees come first.

The result is a `Vec<Wave>` where each `Wave { recipes: Vec<String> }` lists recipes in registration order. Wave 0 runs first, wave N runs last; recipes within a wave have no ordering constraint between them other than the intra-group order needed for `{dep}` references to resolve at registration time.

### How `run_inner` consumes waves

`cli/crates/cook-engine/src/run.rs:481` is the wave loop:

```text
for wave in &waves {
    for recipe_name in &wave.recipes {
        units = registry.register_recipe(...);   // ── registration-phase Lua
        wave_units.push(units);
        wave_cache_managers.insert(...);
    }
    dag = dag_builder::build_dag(wave_units)?;   // ── work-unit DAG for this wave
    executor::execute_dag(dag, num_jobs, ...);   // ── run it to completion
}
```

Registration within a wave is currently serial (each `register_recipe` call holds its own `!Send` Lua VM for the duration of registration). The wave abstraction is what makes future per-recipe-thread parallelism a local change rather than a structural one: members of a wave are by construction safe to register concurrently because they have no recipe-level dependency on each other.

Once the wave is registered, **all** of its work runs through one shared `Dag<WorkNode>` and one shared `WorkerPool` invocation — work-unit parallelism across recipes within a wave is the engine's primary source of concurrency.

---

## 4. Work-unit DAG building (`dag_builder.rs`)

`cli/crates/cook-engine/src/dag_builder.rs:35`:

```rust
pub fn build_dag(recipe_units: Vec<RecipeUnits>) -> Result<Dag<WorkNode>, EngineError>
```

The input is a wave's worth of `RecipeUnits` in registration order. The builder iterates once, maintaining a `recipe_leaves: BTreeMap<String, Vec<usize>>` that maps each fully-processed recipe to its terminal barrier nodes (so later recipes can wire cross-recipe edges to them).

### `WorkPayload` variants

`WorkPayload` (`cli/crates/cook-contracts/src/lib.rs:58`) is the per-unit work description; the engine wraps it in `WorkNode { payload: Option<WorkPayload>, recipe_name, cache_meta, working_dir, env_vars }` (`cli/crates/cook-engine/src/lib.rs:62`). When `payload` is `None`, the node is pre-satisfied (a cache hit captured at registration time) and the executor completes it synchronously without dispatching to the pool.

| Variant | Carries | Where it runs |
|---|---|---|
| `Shell { cmd, line }` | A literal shell command | Worker thread: `/bin/sh -c cmd`, captures stdout+stderr |
| `Interactive { cmd, line, is_chore }` | Shell command needing inherited stdio | Main thread, after pool drains; `is_chore=true` joins a chore window |
| `LuaChunk { code, inputs, outputs, ingredient_groups, step_kind, is_chore }` | Cook/test/chore-step Lua body | Worker VM `lua.load(code).exec()`; if `is_chore=true` it routes through the chore-window drain instead |
| `Test { cmd, line, timeout, should_fail, suite_name, test_name, iteration_item }` | One test unit | Worker thread: spawned with a timeout, outcome reported as `TestStarted` / `TestPassed` / `TestFailed` / `TestTimedOut` events |

### `DepKind` and the barrier concept

`DepKind` (`cli/crates/cook-contracts/src/lib.rs:172`) is how the registrar tells the builder how a unit relates to its siblings:

| Variant | Within-recipe semantics |
|---|---|
| `Sequential` | Depends on the current barrier; becomes the new barrier (singleton). |
| `StepGroup(idx)` | Depends on the current barrier (shared with group siblings). When the last group member is processed, all members become the new barrier. |
| `TestSibling(idx)` | Same wiring as `StepGroup`, but failures are scoped: a failing test in the group does **not** cancel its siblings (so every test in a group runs regardless of which ones fail). |

The **barrier** is the central concept: a set of node ids representing "everything that must finish before the next sequential unit can begin." It starts empty and is updated after each unit per the rules above. This is the same mechanism the old single-module scheduler used; the only changes are (a) the barrier now feeds into `Dag<WorkNode>::add_node` rather than a Cook-specific `ExecutionDag`, and (b) `TestSibling` joined the variant list.

### Worked example: compile-then-link

```
unit 0: StepGroup(0), cmd="gcc -c a.c"
unit 1: StepGroup(0), cmd="gcc -c b.c"
unit 2: Sequential,   cmd="ar rcs lib.a a.o b.o"
```

- Initial barrier `= []`.
- Unit 0: within_deps `= []`, dag_id `= 0`. Not the last in group 0; barrier unchanged.
- Unit 1: within_deps `= []`, dag_id `= 1`. Last in group 0: barrier becomes `[0, 1]`.
- Unit 2: within_deps `= [0, 1]`, dag_id `= 2`. Barrier becomes `[2]`.

```
  [0: gcc -c a.c]   [1: gcc -c b.c]
        \                /
         \              /
          [2: ar rcs lib.a]
```

### Cross-recipe edges

Two flavours:

- **Coarse `deps`** (`RecipeUnits.deps: Vec<String>`): each prerequisite's leaves are unioned into `cross_deps`. Cross-recipe deps apply only to **root** units (units whose within-recipe deps are empty) — once a recipe has internal predecessors, those subsume the cross-recipe wait. See `dag_builder.rs:92`.
- **Fine-grained `dep_edges`** (`Vec<(unit_idx, dep_recipe_name)>`): for the exact unit at `unit_idx`, append the named recipe's terminal nodes regardless of whether the unit has within-recipe deps. This is how a single late step in recipe B can wait on recipe A without forcing all of B's earlier steps to. See `dag_builder.rs:100`.

### Pre-satisfaction (cache hits at registration)

`is_presatisfied(unit)` (`dag_builder.rs:181`) is true when the captured payload is `Shell { cmd: "", .. }` or `Test { cmd: "", .. }` with no `cache_meta`. In that case the builder emits a `WorkNode` with `payload: None`; the executor's `process_ready` sees the `None` and completes the node immediately, cascading through any downstream chain of pre-satisfied nodes in the same call stack.

(The artifact cache check that runs at execute time — `check_node_cache` at `executor.rs:507` — is a separate path: it sees real payloads with `cache_meta` and decides node-by-node whether to skip them.)

### Plan-time output-collision check

Before returning the DAG, `build_dag` calls `detect_output_collisions` (`dag_builder.rs:194`) which fails fast with `EngineError::OutputCollision` when two recipes with no dependency edge between them (in either direction) both declare the same canonical output path. This prevents silent races under `--jobs > 1`.

---

## 5. Worker pool (`cook-luaotp`)

`cli/crates/cook-luaotp/src/pool.rs:77`:

```rust
pub fn WorkerPool::spawn(n: usize) -> (WorkerPool, mpsc::Receiver<WorkResult>);
pub fn submit(&self, item: WorkItem);
pub fn shutdown(self);   // + Drop impl that signals + joins
```

### Per-thread Lua VM

Each of the N worker threads creates **its own** `mlua::Lua` VM with `unsafe { mlua::Lua::unsafe_new() }` (`pool.rs:159`). The VM is `!Send` — it cannot move between threads — but because it never leaves the thread that created it, the unsafe constructor's invariants are satisfied. This is the only `unsafe` block in the scheduling layer, and the rationale has not changed since the old single-module scheduler.

Each worker installs:
- `path.*` Lua API (`cook_lua_stdlib::register_path_api`)
- A `cook` table whose closures (`cook.sh`, `cook.exec`, `cook.env`) capture `Arc<Mutex<…>>` slots updated per work item: `current_recipe`, `current_working_dir`, `current_env_vars`, and a CS-0045 sandbox slot (`current_sandbox`).
- `fs.*` API bound to a `WorkingDirSource::Live` reading the live cwd slot (so one VM can serve items from multiple Cookfiles per CS-0017).
- Register-only API guards (`cook.add_unit`, `cook.recipe`, `cook.step_group`, `cook.interactive`) that raise errors when invoked from the execute-phase VM.

### Shared queue + result channel

```text
WorkerPool::queue : Arc<SharedQueue {
    queue:   Mutex<VecDeque<QueueItem>>,   // QueueItem = Work(WorkItem) | Shutdown
    condvar: Condvar,
}>

submit(item):  lock, push_back(Work(item)), notify_one()
shutdown():    lock, push_back(Shutdown) × N, notify_all(), join all
```

The result channel is a standard fan-in `mpsc::channel<WorkResult>`: each worker holds an `mpsc::Sender` clone; the executor on the main thread holds the sole `Receiver`.

### `WorkItem` and `WorkResult`

```rust
pub struct WorkItem {
    pub id: usize,
    pub payload: WorkPayload,
    pub recipe_name: String,
    pub working_dir: PathBuf,
    pub env_vars: HashMap<String, String>,
    pub project_root: PathBuf,    // CS-0045: per-item sandbox root
}

pub struct WorkResult {
    pub id: usize,
    pub success: bool,
    pub error: Option<String>,
    pub test_output: Option<TestOutput>,
    pub node_name: String,
    pub output_lines: Vec<(OutputStream, String)>,   // CS-0035 fd-tagged lines
}
```

The worker dispatch in `execute_work_item` (`pool.rs:735`) routes by payload:

| Payload | Function | Notes |
|---|---|---|
| `Shell` | `execute_shell` (`pool.rs:804`) | `std::process::Command`, captures stdout+stderr, splits into `(Stream, String)` entries |
| `LuaChunk` | `execute_lua_chunk` (`pool.rs:883`) | Sets `input`, `output`, `inputs`, `outputs`, `input_1..N` globals, runs `lua.load(code).exec()` |
| `Test` | `execute_test` (`pool.rs:944`) | Spawns child with timeout, builds `TestOutput { stdout, stderr, exit_code, timed_out, ... }` |
| `Interactive` | Returns a `"BUG: interactive step dispatched to worker pool"` error | Should never occur — the engine routes interactives to the chore-window drain instead |

### Panic safety

Each work item runs inside `std::panic::catch_unwind` (`pool.rs:287`). A Rust panic in `execute_work_item` is converted into a failing `WorkResult` carrying `"worker panic: …"` so the engine never hangs on `rx.recv()`. The Lua VM is reused — mlua wraps Lua-callback panics as Lua errors, keeping the VM state sane.

---

## 6. Execution loop (`executor::execute_dag`)

`cli/crates/cook-engine/src/executor.rs:280`:

```rust
pub fn execute_dag(
    dag: Dag<WorkNode>,
    num_workers: usize,
    cache_managers: BTreeMap<String, Arc<ThreadSafeCacheManager>>,
    event_tx: Option<mpsc::Sender<EngineEvent>>,
    cache_ctx: Arc<CacheContext>,
    test_cache: Option<&TestCache>,
    fingerprint_by_node: &BTreeMap<usize, String>,
    rerun_patterns: &[String],
) -> Result<Vec<TestResult>, EngineError>
```

### Initialisation

```text
1. Install the depfile-parser callback (one-time, via Once).
2. dag.validate() — defensive cycle check.
3. WorkerPool::spawn(num_workers) → (pool, rx).
4. Build per-recipe RecipeTracker { start, total_nodes, completed_nodes, cached_nodes,
                                    has_failure, started, is_chore }.
5. Seed: for id in dag.initial_ready() { pending += process_ready(id, …) }.
```

`pending` counts work items currently in-flight (submitted to the pool but no result yet). `finished` counts every node that has been accounted for (completed, failed, pre-satisfied, or cancelled). The loop exits when `pending == 0 && interactive_queue.is_empty()`.

### `process_ready(id)` — what to do with a newly-ready node

Returns the number of items submitted to the pool (0 or 1). Routing (`executor.rs:559`):

```text
cancelled[id]                                → finished++, return 0
payload == None  (pre-satisfied)             → emit NodeCacheHit, finished++,
                                               dag.complete(id), recurse into newly_ready
payload == Interactive                       → interactive_queue.push(id), return 0
payload == LuaChunk { is_chore: true }       → interactive_queue.push(id), return 0
                                               (CS-0051: chore Lua shares the drain)
payload == Test                              → test-cache lookup first; on hit synthesize
                                               TestStarted + TestPassed(cached=true) and
                                               cascade as a cache hit; on miss, fall through
                                               to artifact-cache check then pool dispatch
payload == Shell / LuaChunk (non-chore) /
           Test (cache miss)                 → check_node_cache(); on hit, cascade; on miss,
                                               ensure_output_parent_dirs() (CS-0050), then
                                               pool.submit(WorkItem { ... }); return 1
```

Pre-satisfied cascades are synchronous and recursive: a chain of cached nodes all resolve in the same call stack before the loop waits for any pool result.

### Main loop

```text
loop {
    while pending == 0 && !interactive_queue.is_empty() {
        run_next_interactive_window();   // see §7
    }
    if pending == 0 && interactive_queue.is_empty() { break }

    result = rx.recv()                    // block on next worker result
    pending -= 1; finished += 1

    if result.success {
        forward result.output_lines as EngineEvent::OutputLine
        emit NodeCompleted
        record_completion in cache_manager (handles depfile augmentation,
            cloud_key + artifact_key computation, backend put_bytes)
        for nid in dag.complete(result.id) { pending += process_ready(nid, …) }
    } else {
        emit NodeFailed
        failures.push((result.id, recipe_name, error))
        for dep in dag.node(result.id).dependents() {
            cancel_subtree(dag, dep, ...)   // see §8
        }
    }
}
```

After the loop, the executor flushes every `cache_manager`, drains the pool (its `Drop` sends `Shutdown` sentinels and joins all threads), and either returns `Ok(test_results)` or wraps the accumulated failures in `EngineError::TaskFailures`.

---

## 7. Interactive steps and chore-window drain

`WorkPayload::Interactive` and `WorkPayload::LuaChunk { is_chore: true, .. }` cannot share stdio with workers — they need the controlling terminal. Both feed `interactive_queue: Vec<usize>` and are drained only when `pending == 0`. The drain is the inner `while` in the main loop; there is no explicit `drain()` call.

When the queue head is reached, the executor branches on whether the head is a **chore-window member** (`is_chore_window_member` at `executor.rs:85`): any `Interactive { is_chore: true, .. }` or `LuaChunk { is_chore: true, .. }`.

### Chore-window path (CS-0051)

`executor.rs:1021`. A chore body is emitted by the registrar as a linear chain of `Interactive`/`LuaChunk` units bracketed by `cook._enter_chore()` / `cook._exit_chore()` with `is_chore = true`. Only the head is initially ready; later steps surface as each predecessor completes. The drain pre-walks `dependents()` from the head while same-recipe + chore-member + single-predecessor invariants hold to discover the **full window** statically, then:

1. Emit one `EngineEvent::InteractiveStart { chore_step_count: n }` so the renderer can freeze progress bars **before** any chore output appears.
2. Run window steps in order:
   - Shell-interactive steps → `run_interactive_on_main` directly on the engine thread.
   - Lua-bundle chore steps → submitted to the worker pool as a single `WorkItem`; the executor blocks on `rx.recv()` for that one result (safe because chore drains only enter when `pending == 0`, so the submitted item is the only in-flight one).
3. On step failure: record `failed_idx`, set `last_err`, break, and cancel the untouched tail of the window.
4. Emit one `EngineEvent::InteractiveEnd { success, is_terminal, failed_step }`. The `is_terminal` flag tells the renderer "no more work is coming — leave progress bars frozen".

### Legacy single-line path

`executor.rs:1357`. `Interactive { is_chore: false, .. }` keeps the pre-CS-0051 per-node `InteractiveStart` / `InteractiveEnd` pair. Same execution model — runs on the main thread via `run_interactive_on_main` — but no window pre-walk and no `chore_step_count`.

The renderer suppresses any in-progress progress bars between `InteractiveStart` and `InteractiveEnd` so the interactive command owns the terminal.

---

## 8. Failure handling

When a node fails (either a worker returns `success: false`, or an interactive command exits non-zero, or `ensure_output_parent_dirs` returns `Err`):

1. The failure is appended to `failures: Vec<(node_id, recipe_name, error_message)>`.
2. `cancel_subtree(dag, dep, &mut cancelled, …)` is called for every direct dependent of the failed node. The helper recurses through `dag.node(id).dependents()`, marking each transitively reachable node as cancelled.
3. For each cancelled `WorkPayload::Test` node, `cancel_subtree` also emits `EngineEvent::TestBlocked { upstream }` and synthesises a `TestResult { outcome: Blocked, blocked_by: Some(upstream_name), … }`. These accumulate in `blocked_results` so the test runner can report them even though they never executed.
4. When `process_ready` later encounters a cancelled node, it ticks `finished` without submitting work — the node is skipped.
5. Failures in independent branches (not downstream of the failed node) continue running normally.

After the loop, if `failures` is non-empty the executor returns:

```rust
Err(EngineError::TaskFailures {
    count: failures.len(),
    failures,                          // Vec<(node_id, recipe_name, error_message)>
    partial_test_results: blocked + cached_test_results + test_results,
})
```

`run_for_test_inner` (`run.rs:74`) catches this variant, extracts `partial_test_results`, and returns `Ok(RunResult { test_results })` — failed cook steps are surfaced through the per-test `Blocked` rows rather than as a hard error from `cook test`.

The old `SchedulerError` type no longer exists; failures propagate as `EngineError` variants throughout.

### `cancel_subtree` and `TestSibling`

`DepKind::TestSibling` produces edges in the DAG just like `StepGroup`, so `cancel_subtree` would naturally cascade through them. The current implementation walks `dependents()` unconditionally; the test-blocked semantics rely on the fact that test-step nodes are leaves in their group (no within-recipe dependent points back at them) plus the upstream cancellation skipping. See `executor.rs:427`.

---

## 9. Output streaming

There is no `SharedWriter`. Output is line-streamed through `EngineEvent`s.

### Worker side

Each `execute_shell` / `execute_test` / `execute_lua_chunk` returns a `WorkResult` whose `output_lines: Vec<(OutputStream, String)>` carries every captured line tagged with its file descriptor of origin — `OutputStream::Stdout` or `OutputStream::Stderr` (`cli/crates/cook-contracts/src/lib.rs:28`). CS-0035 made this distinction load-bearing: prior to the fix the `output_lines` were untagged `Vec<String>` and every line in `events.jsonl` was attributed to stdout.

### Engine side

When the executor receives a successful `WorkResult` (`executor.rs:1697`):

```rust
for (stream, line) in &result.output_lines {
    emit(&event_tx, EngineEvent::OutputLine {
        recipe: recipe_name.clone(),
        line: line.clone(),
        stream: *stream,
    });
}
```

The chore-window Lua path forwards captured lines the same way (`executor.rs:1178`). Downstream consumers (the TTY renderer, the JSONL writer at `events.jsonl`, the per-node log store) decide how to prefix and where to write — the engine is purely an event emitter.

---

## 10. Event flow

`EngineEvent` (`cli/crates/cook-engine/src/lib.rs:117`) is the engine's full observability surface. Variants:

| Lifecycle | Events |
|---|---|
| Build | `BuildStarted { recipes, total_nodes }`, `Finished { elapsed, success }` |
| Recipe | `RecipeQueued`, `RecipeStarted`, `RecipeCompleted { kind: Recipe \| Chore }`, `RecipeFailed` |
| Node | `NodeStarted { kind: NodeKind }`, `NodeCompleted`, `NodeFailed`, `NodeCacheHit`, `NodeSkipped` |
| Interactive | `InteractiveStart { chore_step_count }`, `InteractiveEnd { is_terminal, failed_step }` |
| Output | `OutputLine { recipe, line, stream }` |
| Test | `TestStarted`, `TestPassed { cached, should_fail }`, `TestFailed { reason: ExitStatusMismatch \| SignalKilled \| SpawnError }`, `TestBlocked { upstream }`, `TestTimedOut { timeout }` |

`NodeKind` and `RecipeKind` are engine-side enums isomorphic to `cook_progress::NodeKind` / `cook_progress::event::RecipeKind`; the CLI translates between them so `cook-engine` does not directly depend on `cook-progress` (the renderer is one of several possible event consumers — `events.jsonl`, the test runner, and a future telemetry path can all subscribe to the same stream).

Events are emitted through `Option<mpsc::Sender<EngineEvent>>` (`emit` helper at `executor.rs:44`). `run::run_inner` spans the event stream across a `std::thread::scope` (`run.rs:606`): a bridge thread `recv`s events and forwards them to the user-supplied `on_event` callback while the main thread runs `execute_dag`.

### `ExportStore`

`cli/crates/cook-engine/src/lib.rs:51`:

```rust
pub type ExportStore = BTreeMap<String, serde_json::Value>;
```

The engine owns one `ExportStore` per run and threads it into each registration call so that `cook.export()` calls from one recipe can be observed by later recipes' registration-phase Lua. It is not part of the work-DAG scheduling proper but lives in the same crate because the wave loop is its natural owner.
