# Scheduler: DAG Builder, Worker Pool, Parallelism

**Source files:** `src/scheduler/mod.rs` (490 lines), `src/scheduler/dag.rs` (249 lines), `src/scheduler/builder.rs` (259 lines), `src/scheduler/pool.rs` (551 lines), `src/scheduler/output.rs` (206 lines)

---

## 1. Overview

The scheduler is the execution engine that takes the work units produced by the runtime and runs them as fast as the dependency graph allows. The pipeline has three stages:

1. **Build** — `build_dag()` in `builder.rs` consumes a topologically-sorted `Vec<RecipeUnits>` and produces an `ExecutionDag` where every node knows how many predecessors it must wait for.
2. **Execute** — `execute_dag()` in `mod.rs` seeds the pool with all initially-ready nodes, then drives a result loop: every time a node finishes, its dependents' counters are decremented and any that reach zero are dispatched immediately.
3. **Output** — `SharedWriter` in `output.rs` serializes line-level output from all worker threads so that lines from different recipes never interleave on the terminal.

Interactive steps (`@`-prefixed shell commands) are special-cased throughout: they cannot share stdout/stdin with worker threads, so they are held in a side queue and only run on the main thread once the worker pool is fully drained.

---

## 2. DAG Data Structures (`src/scheduler/dag.rs`)

### `WorkPayload`

```rust
pub enum WorkPayload {
    Shell { cmd: String, line: usize },
    Interactive { cmd: String, line: usize },
    LuaChunk {
        code: String,
        input: String,
        output: String,
        ingredient_groups: Vec<Vec<String>>,
    },
}
```

- `Shell` — an ordinary shell command captured from a recipe step. `line` is the source line number used in error messages (`COOK_CMD_FAILED:<line>:<code>:<cmd>`).
- `Interactive` — a shell command that needs inherited stdio (the `@` prefix in a cookfile). Cannot run on a worker thread.
- `LuaChunk` — a Lua block step. Carries the full code string plus the `input`/`output` path strings and `ingredient_groups` that the Lua VM will see as globals. The worker thread's isolated `mlua::Lua` VM evaluates this directly.

### `CacheMeta`

```rust
pub struct CacheMeta {
    pub recipe_name: String,
    pub cache_key:   String,
    pub input_paths: Vec<String>,
    pub output_path: Option<String>,
    pub command_hash: u64,
}
```

Attached to a node when the cache subsystem recorded the step during capture. After a node executes successfully, `execute_dag` re-stats the input and output files and calls `cache_manager.update_step()` to record the new fingerprints.

### `DagNode`

```rust
pub struct DagNode {
    pub id:             usize,
    pub payload:        Option<WorkPayload>,  // None = pre-satisfied (cached)
    pub recipe_name:    String,
    pub cache_meta:     Option<CacheMeta>,
    pub dependents:     Vec<usize>,           // node IDs that depend on this one
    pub remaining_deps: AtomicUsize,          // decremented as predecessors complete
}
```

`remaining_deps` is an `AtomicUsize` so `dag.complete(id)` can be called from worker threads without holding a mutex — each `fetch_sub(1, AcqRel)` returns the *previous* value, so the thread that observes `prev == 1` knows it just made that dependent ready.

`payload: None` marks a pre-satisfied node (a cached step that needs no re-execution). These nodes complete immediately and synchronously when first encountered, cascading through any downstream nodes that are also pre-satisfied.

### `ExecutionDag`

```rust
pub struct ExecutionDag {
    nodes: Vec<DagNode>,
}
```

A flat `Vec<DagNode>` indexed by node ID. Key methods:

| Method | Purpose |
|---|---|
| `add_node(payload, recipe, cache_meta, dep_ids)` | Appends a node and wires `dep_ids` as predecessors |
| `add_presatisfied(recipe, dep_ids)` | Same but with `payload: None` |
| `initial_ready() -> Vec<usize>` | All nodes where `remaining_deps == 0` |
| `complete(id) -> Vec<usize>` | Decrement deps on dependents; return newly-ready IDs |

---

## 3. DAG Builder (`src/scheduler/builder.rs`)

`build_dag(recipe_units: Vec<RecipeUnits>) -> ExecutionDag`

The input is a `Vec<RecipeUnits>` already in **topological order** — if recipe B depends on recipe A, A appears first. The builder iterates through them once, maintaining a `recipe_leaves: HashMap<String, Vec<usize>>` that maps each completed recipe to its final barrier nodes.

### Within-recipe wiring: barriers and step groups

The central concept is the **barrier** — a set of DAG node IDs that represents "everything that must have finished before the next sequential unit can begin." The barrier starts empty (meaning "no within-recipe predecessors") and is updated after each unit is processed.

```
DepKind::Sequential  →  depends on current barrier; becomes the new barrier (singleton)
DepKind::StepGroup(gi) →  depends on current barrier (shared); all members collected;
                           when the last member is processed, all members become the new barrier
```

This means step-group members can run in parallel with each other but are still gated behind whatever ran before the group, and everything after the group waits for all members to finish.

Example — compile two files in parallel, then link:

```
Initial barrier: []

unit 0: StepGroup(0), cmd="gcc -c a.c"
  → all_deps = []  (no within-recipe deps, no cross-recipe deps)
  → dag_id = 0, group_dag_ids[0] = [0]
  → not the last in group 0 (size=2), barrier unchanged: []

unit 1: StepGroup(0), cmd="gcc -c b.c"
  → all_deps = []
  → dag_id = 1, group_dag_ids[0] = [0, 1]
  → last in group 0: barrier = [0, 1]

unit 2: Sequential, cmd="ar rcs lib.a a.o b.o"
  → all_deps = [0, 1]  (both compilers)
  → dag_id = 2
  → barrier = [2]
```

Resulting DAG:

```
  [0: gcc -c a.c]   [1: gcc -c b.c]
        \                /
         \              /
          [2: ar rcs lib.a]
```

### Cross-recipe wiring

When a recipe names prerequisites in `deps`, the builder looks up each prerequisite's leaf barrier in `recipe_leaves` and collects those node IDs as `cross_deps`. Cross-recipe deps are applied only to **root nodes** of the dependent recipe — units whose `within_deps` list is empty:

```rust
let all_deps = if within_deps.is_empty() {
    cross_deps.clone()
} else {
    within_deps          // within-recipe chain takes precedence
};
```

This means: "the first thing this recipe does must wait for everything the prerequisite recipes did last."

Example — `build` depends on `setup`:

```
setup recipe leaf: [0: mkdir build]

build, unit 0 (root, Sequential):
  within_deps = []  → all_deps = [0]  (cross-recipe)
  dag_id = 1

  [0: mkdir build]
        |
  [1: gcc main.c]
```

### Cache pre-satisfaction

`is_presatisfied(unit)` returns true when the unit has an empty shell command and no `cache_meta`. In practice, when the runtime detects a cache hit it emits a `CapturedUnit` with an empty `cmd` — the builder calls `dag.add_presatisfied()` for it, which sets `payload: None`. The execution loop completes these nodes immediately and synchronously without dispatching to any thread.

---

## 4. Worker Pool (`src/scheduler/pool.rs`)

### Architecture

```
Main thread                     Worker threads (N)
-----------                     ------------------
WorkerPool::spawn(N, ...)  →    thread 0: worker_loop(queue, tx, lua_vm, ...)
                                thread 1: worker_loop(queue, tx, lua_vm, ...)
                                ...
pool.submit(WorkItem)      →    SharedQueue (Mutex<VecDeque> + Condvar)
                           ←    mpsc::Receiver<WorkResult>
pool.shutdown()            →    N × QueueItem::Shutdown sentinels
```

Each worker thread owns its own `mlua::Lua` VM. The VM is `!Send` (cannot be moved between threads), but because the VM never leaves the thread it was created on, `unsafe { mlua::Lua::unsafe_new() }` is used. This is the only `unsafe` block in the scheduler.

### Per-thread setup (`worker_loop`, line 109)

Each worker calls three registration functions before entering its loop:

1. `register_fs_api(&lua, &working_dir)` — mounts `fs.*` Lua globals (read, write, exists, etc.)
2. `register_path_api(&lua)` — mounts `path.*` Lua globals (join, basename, etc.)
3. `register_worker_cook_table(...)` — creates the `cook` Lua table with:
   - `cook.sh(cmd)` — runs a shell command, returns stdout as a string
   - `cook.exec(cmd, line)` — same, with explicit line number for error messages
   - `cook.env` — table of recipe environment variables

`cook.sh` and `cook.exec` are closures that capture a `Arc<Mutex<String>>` called `current_recipe`. Before each work item is executed, the loop updates this string so that output lines are prefixed with the correct recipe name.

### Shared queue

```rust
struct SharedQueue {
    queue:   Mutex<VecDeque<QueueItem>>,
    condvar: Condvar,
}
```

`pool.submit(item)` locks the queue, pushes `QueueItem::Work(item)`, then calls `condvar.notify_one()`. Workers block on `condvar.wait(q)` when the queue is empty. `pool.shutdown()` pushes one `QueueItem::Shutdown` sentinel per thread and calls `condvar.notify_all()`.

### Result channel

Each worker thread receives a clone of an `mpsc::Sender<WorkResult>`. The main thread holds the sole `mpsc::Receiver<WorkResult>`. This is a standard fan-in: N producers, 1 consumer.

### `WorkItem` and `WorkResult`

```rust
pub struct WorkItem {
    pub id:          usize,
    pub payload:     WorkPayload,
    pub recipe_name: String,
}

pub struct WorkResult {
    pub id:      usize,
    pub success: bool,
    pub error:   Option<String>,
}
```

Note: unlike earlier drafts, `WorkItem` does not carry `env_vars`, `quiet`, or `writer` — those are captured once at thread spawn time and live for the thread's lifetime.

### Executing work items (`execute_work_item`, line 258)

| Payload type | Execution path |
|---|---|
| `Shell` | `execute_shell()` — `std::process::Command`, captures stdout/stderr, writes prefixed lines through `SharedWriter` |
| `LuaChunk` | `execute_lua_chunk()` — sets `input`, `output`, `input_1..N` globals, calls `lua.load(code).exec()` |
| `Interactive` | Returns an immediate error: `"BUG: interactive step dispatched to worker pool"` — this should never occur if the execution loop is correct |

Shell failure produces an error string in the format `COOK_CMD_FAILED:<line>:<exit_code>:<cmd>` — a structured format parsed by the error reporter upstream.

---

## 5. Execution Loop (`src/scheduler/mod.rs`, `execute_dag`)

### Initialization

```rust
let (pool, rx) = WorkerPool::spawn(num_workers, writer, working_dir.clone(), env_vars.clone());
let initial = dag.initial_ready();   // all nodes where remaining_deps == 0
for id in initial {
    pending += process_ready(&dag, id, &pool, ...);
}
```

`pending` counts work items currently in-flight (submitted to pool but no result received yet). `finished` counts all nodes that have been accounted for (completed, failed, or cancelled). The loop ends when `pending == 0 && interactive_queue.is_empty()`.

### `process_ready(id, ...)`

Called whenever a node becomes ready. Returns the number of items submitted to the pool (0 or 1).

```
Node is cancelled?          → finished++, return 0
payload == None             → finished++, dag.complete(id), cascade to dependents
payload == Interactive      → interactive_queue.push(id), return 0
payload == Shell/LuaChunk   → pool.submit(WorkItem { id, payload, recipe_name }), return 1
```

Pre-satisfied nodes cascade synchronously — if a chain of cached nodes precedes a real node, they all resolve in the same call stack before the loop needs to wait for pool results.

### Main loop

```
loop:
  while pending == 0 && !interactive_queue.is_empty():
      run next interactive node on main thread (see §6)

  if pending == 0 && interactive_queue.is_empty(): break

  result = rx.recv()   ← blocks until a worker reports back
  pending -= 1
  finished += 1

  if result.success:
      update cache entry (if node has cache_meta)
      newly_ready = dag.complete(result.id)
      for each newly_ready: pending += process_ready(...)
  else:
      record failure
      cancel_subtree(result.id)   ← marks all transitive dependents cancelled
```

`cancel_subtree` is a recursive helper defined inside `execute_dag` that walks `node.dependents` and sets `cancelled[id] = true`. Cancelled nodes are skipped (and counted as finished) when `process_ready` encounters them.

### Cache update path

After a successful node, `execute_dag` re-stats every input file and the output file (if any) listed in the node's `CacheMeta`, then calls `cache_manager.update_step()`. This happens on the main thread immediately after receiving the `WorkResult`, before dispatching newly-ready dependents. After the loop exits, `cm.flush_all()` persists the in-memory cache to disk.

---

## 6. Interactive Step Handling

Interactive nodes (`WorkPayload::Interactive`) require the terminal's stdin/stdout/stderr to be inherited by the child process. Worker threads cannot do this because their output goes through `SharedWriter`.

**Detection:** When `process_ready` encounters an `Interactive` payload, it pushes the node ID onto `interactive_queue` and returns 0 (no pool submission).

**Execution condition:** Interactive nodes only run when `pending == 0` — all in-flight pool work has completed and results have been received. There is no explicit `drain()` call; the condition is simply the inner `while` in the main loop:

```rust
while pending == 0 && !interactive_queue.is_empty() {
    let id = interactive_queue.remove(0);
    // ... run on main thread ...
}
```

**Running on main thread:** `run_interactive_on_main(cmd, line, working_dir, env_vars)` calls `std::process::Command` with the default stdio (inherited). The worker threads are idle (blocked on `condvar.wait`) during this time.

**Resuming parallel work:** After the interactive node succeeds, `dag.complete(id)` is called and any newly-ready dependents are dispatched to the pool. If those dependents include more interactive nodes, they are queued and will run in the next `pending == 0` window.

**Failure:** If the interactive command exits with a non-zero status, its error is recorded and `cancel_subtree` is called on its dependents. The main loop then returns to waiting for any remaining pool work before breaking.

---

## 7. Failure Handling

When a node fails (pool returns `success: false`, or an interactive command returns `Err`):

1. The failure is appended to `failures: Vec<(node_id, recipe_name, error_message)>`.
2. `cancel_subtree(id, &mut cancelled)` marks `cancelled[id] = true` for every transitive dependent of the failed node.
3. When `process_ready` encounters a cancelled node, it increments `finished` without submitting work — the node is effectively skipped.
4. Independent branches (not downstream of the failed node) continue running normally.

After the loop, if `failures` is non-empty, `execute_dag` returns:

```rust
Err(SchedulerError { failures })
```

`SchedulerError` formats as:

```
scheduler: N task(s) failed:
  node <id> (<recipe_name>): <error_message>
```

---

## 8. Output Serialization (`src/scheduler/output.rs`)

### `SharedWriter`

```rust
pub struct SharedWriter {
    stdout: Arc<Mutex<io::Stdout>>,
    stderr: Arc<Mutex<io::Stderr>>,
}
```

Two mutexes — one for stdout, one for stderr — ensure that no two threads write a partial line at the same time. Both `write_stdout_line` and `write_stderr_line` lock, write `[prefix] line\n`, flush, and unlock atomically.

`SharedWriter` is `Clone` (cloning the `Arc`s) so each worker thread and the main pool initialization all share the same underlying locks.

### `PrefixedWriter`

A stateful byte-level writer that buffers incoming bytes, scans for newlines, and emits complete lines through a `Sink`:

```
Sink::Stdout(&SharedWriter)   → write_stdout_line
Sink::Stderr(&SharedWriter)   → write_stderr_line
Sink::Buffer(Arc<Mutex<Vec<u8>>>) → test-only in-memory sink
```

`PrefixedWriter::write_bytes(data)` appends to an internal `Vec<u8>` and flushes any newline-terminated segments. `flush_remaining()` emits whatever is left (no trailing newline in the source) as a final partial line.

In practice the pool's shell and Lua execution paths call `SharedWriter` methods directly (line-splitting is done on the already-collected output string), so `PrefixedWriter` is available for callers that receive streaming byte chunks from child process pipes.

### Output format

Every line printed by a worker looks like:

```
[recipe_name] <original line content>
```

This makes it immediately obvious which recipe produced each line when multiple recipes run in parallel.
