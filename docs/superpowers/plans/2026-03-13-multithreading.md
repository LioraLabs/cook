# Multithreading Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add fine-grained parallel execution to Cook via a DAG scheduler and Lua VM worker pool.

**Architecture:** Recipes are registered on the main thread to build a fine-grained DAG of work units. A scheduler feeds ready units to a pool of N worker threads (each owning a Lua VM). Workers execute shell commands or self-contained Lua chunks and report completion, which unblocks downstream units.

**Tech Stack:** Rust std::thread, std::sync::{mpsc, Arc, Mutex, Condvar}. Zero new crate dependencies.

**Spec:** `docs/superpowers/specs/2026-03-13-multithreading-design.md`

---

## File Structure

### New Files

| File | Responsibility |
|------|---------------|
| `src/scheduler/mod.rs` | Public API: `build_and_execute()`. Scheduler loop. Module declarations. |
| `src/scheduler/dag.rs` | `WorkPayload`, `CacheMeta`, `DagNode`, `ExecutionDag` types. DAG manipulation. |
| `src/scheduler/pool.rs` | `WorkerPool`, ready queue, worker thread loop, Lua VM setup. |
| `src/scheduler/output.rs` | `PrefixedWriter` — line-buffered output with `[recipe]` prefix. |
| `src/scheduler/builder.rs` | `DagBuilder` — takes per-recipe registration results, wires cross-recipe deps, produces `ExecutionDag`. |

### Modified Files

| File | Changes |
|------|---------|
| `src/lib.rs` | Add `pub mod scheduler;` |
| `src/cli/mod.rs` | Add `-j`/`--jobs` flag. Replace sequential recipe loop with `scheduler::build_and_execute()`. |
| `src/runtime/mod.rs` | Add `register_recipe()` method that runs registration mode and returns captured work units. Keep `execute_recipe()` working for backward compat during transition. |
| `src/runtime/api.rs` | Add `register_layer_api_capture()` — registration-mode `cook.layer` that captures WorkUnits instead of executing. Modify `run_shell_command()` to accept a `PrefixedWriter`. |
| `src/codegen/mod.rs` | Pass raw Lua block source as 5th arg to `cook.layer()` for Lua block cook steps. Emit `cook.begin_step()`/`cook.end_step()` markers around cook step loops. Skip `Step::Taste`. |
| `src/cache/mod.rs` | Add `ThreadSafeCacheManager` — per-recipe `Mutex<CacheState>` for worker-safe cache updates. |

---

## Chunk 1: DAG Foundation

### Task 1: DAG Data Structures

**Files:**
- Create: `src/scheduler/dag.rs`
- Create: `src/scheduler/mod.rs` (minimal, just `pub mod dag;`)
- Modify: `src/lib.rs` — add `pub mod scheduler;`

- [ ] **Step 1: Write failing test for ExecutionDag::add_node**

```rust
// src/scheduler/dag.rs
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_add_single_node() {
        let mut dag = ExecutionDag::new();
        let id = dag.add_node(
            WorkPayload::Shell { cmd: "echo hi".into(), line: 1 },
            "build".into(),
            None,
            vec![],
        );
        assert_eq!(id, 0);
        assert_eq!(dag.len(), 1);
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --lib scheduler::dag::tests::test_add_single_node`
Expected: FAIL — module doesn't exist

- [ ] **Step 3: Implement DAG types and add_node**

```rust
// src/scheduler/dag.rs
use std::sync::atomic::{AtomicUsize, Ordering};

#[derive(Debug, Clone)]
pub enum WorkPayload {
    Shell { cmd: String, line: usize },
    LuaChunk {
        code: String,
        input: String,
        output: String,
        ingredient_groups: Vec<Vec<String>>,
    },
}

#[derive(Debug, Clone)]
pub struct CacheMeta {
    pub recipe_name: String,
    pub cache_key: String,
    pub input_paths: Vec<String>,
    pub output_path: Option<String>,
    pub command_hash: u64,
}

#[derive(Debug)]
pub struct DagNode {
    pub id: usize,
    pub payload: Option<WorkPayload>, // None = pre-satisfied (cached)
    pub recipe_name: String,
    pub cache_meta: Option<CacheMeta>,
    pub dependents: Vec<usize>,
    pub remaining_deps: AtomicUsize,
}

pub struct ExecutionDag {
    nodes: Vec<DagNode>,
}

impl ExecutionDag {
    pub fn new() -> Self {
        Self { nodes: Vec::new() }
    }

    pub fn add_node(
        &mut self,
        payload: WorkPayload,
        recipe_name: String,
        cache_meta: Option<CacheMeta>,
        dep_ids: Vec<usize>,
    ) -> usize {
        let id = self.nodes.len();
        let dep_count = dep_ids.len();
        for &dep_id in &dep_ids {
            self.nodes[dep_id].dependents.push(id);
        }
        self.nodes.push(DagNode {
            id,
            payload: Some(payload),
            recipe_name,
            cache_meta,
            dependents: Vec::new(),
            remaining_deps: AtomicUsize::new(dep_count),
        });
        id
    }

    /// Add a pre-satisfied node (cached, no work to do).
    /// Dependents can start immediately.
    pub fn add_presatisfied(
        &mut self,
        recipe_name: String,
        dep_ids: Vec<usize>,
    ) -> usize {
        let id = self.nodes.len();
        let dep_count = dep_ids.len();
        for &dep_id in &dep_ids {
            self.nodes[dep_id].dependents.push(id);
        }
        self.nodes.push(DagNode {
            id,
            payload: None,
            recipe_name,
            cache_meta: None,
            dependents: Vec::new(),
            remaining_deps: AtomicUsize::new(dep_count),
        });
        id
    }

    pub fn node(&self, id: usize) -> &DagNode {
        &self.nodes[id]
    }

    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    /// Find all nodes with zero remaining deps.
    pub fn initial_ready(&self) -> Vec<usize> {
        self.nodes
            .iter()
            .filter(|n| n.remaining_deps.load(Ordering::Relaxed) == 0)
            .map(|n| n.id)
            .collect()
    }

    /// Mark a node complete, return newly-ready node IDs.
    pub fn complete(&self, id: usize) -> Vec<usize> {
        let mut newly_ready = Vec::new();
        for &dep_id in &self.nodes[id].dependents {
            let prev = self.nodes[dep_id]
                .remaining_deps
                .fetch_sub(1, Ordering::AcqRel);
            if prev == 1 {
                newly_ready.push(dep_id);
            }
        }
        newly_ready
    }
}
```

Also create `src/scheduler/mod.rs`:
```rust
pub mod dag;
```

And add to `src/lib.rs`:
```rust
pub mod scheduler;
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test --lib scheduler::dag::tests::test_add_single_node`
Expected: PASS

- [ ] **Step 5: Write tests for dependency wiring and completion**

```rust
#[test]
fn test_dependency_wiring() {
    let mut dag = ExecutionDag::new();
    let a = dag.add_node(
        WorkPayload::Shell { cmd: "echo a".into(), line: 1 },
        "build".into(), None, vec![],
    );
    let b = dag.add_node(
        WorkPayload::Shell { cmd: "echo b".into(), line: 2 },
        "build".into(), None, vec![a],
    );
    assert_eq!(dag.node(b).remaining_deps.load(Ordering::Relaxed), 1);
    assert_eq!(dag.node(a).dependents, vec![b]);
}

#[test]
fn test_initial_ready_finds_roots() {
    let mut dag = ExecutionDag::new();
    let a = dag.add_node(shell("echo a", 1), "r".into(), None, vec![]);
    let b = dag.add_node(shell("echo b", 2), "r".into(), None, vec![]);
    let _c = dag.add_node(shell("echo c", 3), "r".into(), None, vec![a, b]);
    let ready = dag.initial_ready();
    assert_eq!(ready.len(), 2);
    assert!(ready.contains(&a));
    assert!(ready.contains(&b));
}

#[test]
fn test_complete_unblocks_dependents() {
    let mut dag = ExecutionDag::new();
    let a = dag.add_node(shell("echo a", 1), "r".into(), None, vec![]);
    let b = dag.add_node(shell("echo b", 2), "r".into(), None, vec![]);
    let c = dag.add_node(shell("echo c", 3), "r".into(), None, vec![a, b]);

    let ready = dag.complete(a);
    assert!(ready.is_empty()); // c still blocked by b

    let ready = dag.complete(b);
    assert_eq!(ready, vec![c]); // c now unblocked
}

#[test]
fn test_presatisfied_nodes_start_ready() {
    let mut dag = ExecutionDag::new();
    let cached = dag.add_presatisfied("r".into(), vec![]);
    let work = dag.add_node(shell("echo work", 1), "r".into(), None, vec![cached]);

    // cached is ready (no deps), but it has no payload (no work)
    assert!(dag.node(cached).payload.is_none());

    // Complete cached node → work becomes ready
    let ready = dag.complete(cached);
    assert_eq!(ready, vec![work]);
}

#[test]
fn test_diamond_dag() {
    let mut dag = ExecutionDag::new();
    let a = dag.add_node(shell("a", 1), "r".into(), None, vec![]);
    let b = dag.add_node(shell("b", 2), "r".into(), None, vec![a]);
    let c = dag.add_node(shell("c", 3), "r".into(), None, vec![a]);
    let d = dag.add_node(shell("d", 4), "r".into(), None, vec![b, c]);

    let ready = dag.initial_ready();
    assert_eq!(ready, vec![a]);

    let ready = dag.complete(a);
    assert!(ready.contains(&b));
    assert!(ready.contains(&c));

    assert!(dag.complete(b).is_empty());
    let ready = dag.complete(c);
    assert_eq!(ready, vec![d]);
}
```

- [ ] **Step 6: Run all DAG tests**

Run: `cargo test --lib scheduler::dag`
Expected: PASS

- [ ] **Step 7: Commit**

```bash
git add src/scheduler/ src/lib.rs
git commit -m "feat(scheduler): add DAG data structures"
```

### Task 2: Prefixed Output Writer

**Files:**
- Create: `src/scheduler/output.rs`
- Modify: `src/scheduler/mod.rs` — add `pub mod output;`

- [ ] **Step 1: Write failing test for PrefixedWriter**

```rust
// src/scheduler/output.rs
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_prefix_single_line() {
        let mut buf = Vec::new();
        let mut writer = PrefixedWriter::new("build", &mut buf);
        writer.write_bytes(b"hello world\n");
        assert_eq!(String::from_utf8(buf).unwrap(), "[build] hello world\n");
    }
}
```

- [ ] **Step 2: Run test — fails (module doesn't exist)**

Run: `cargo test --lib scheduler::output`

- [ ] **Step 3: Implement PrefixedWriter**

`PrefixedWriter` buffers bytes and writes complete lines with a `[recipe_name] ` prefix. It needs a `Mutex<io::Stderr>` or `Mutex<io::Stdout>` to write atomically.

```rust
// src/scheduler/output.rs
use std::io::{self, Write};
use std::sync::{Arc, Mutex};

/// Shared writer handle that serializes line output across threads.
#[derive(Clone)]
pub struct SharedWriter {
    stdout: Arc<Mutex<io::Stdout>>,
    stderr: Arc<Mutex<io::Stderr>>,
}

impl SharedWriter {
    pub fn new() -> Self {
        Self {
            stdout: Arc::new(Mutex::new(io::stdout())),
            stderr: Arc::new(Mutex::new(io::stderr())),
        }
    }

    pub fn write_stdout_line(&self, prefix: &str, line: &str) {
        if let Ok(mut out) = self.stdout.lock() {
            let _ = writeln!(out, "[{prefix}] {line}");
        }
    }

    pub fn write_stderr_line(&self, prefix: &str, line: &str) {
        if let Ok(mut err) = self.stderr.lock() {
            let _ = writeln!(err, "[{prefix}] {line}");
        }
    }
}

/// Buffers bytes and flushes complete lines with a prefix.
pub struct PrefixedWriter<'a> {
    prefix: &'a str,
    buffer: Vec<u8>,
    target: PrefixTarget<'a>,
}

enum PrefixTarget<'a> {
    Shared { writer: &'a SharedWriter, is_stderr: bool },
    Vec(&'a mut Vec<u8>),
}

impl<'a> PrefixedWriter<'a> {
    pub fn stdout(prefix: &'a str, writer: &'a SharedWriter) -> Self {
        Self {
            prefix,
            buffer: Vec::new(),
            target: PrefixTarget::Shared { writer, is_stderr: false },
        }
    }

    pub fn stderr(prefix: &'a str, writer: &'a SharedWriter) -> Self {
        Self {
            prefix,
            buffer: Vec::new(),
            target: PrefixTarget::Shared { writer, is_stderr: true },
        }
    }

    #[cfg(test)]
    pub fn new(prefix: &'a str, buf: &'a mut Vec<u8>) -> Self {
        Self {
            prefix,
            buffer: Vec::new(),
            target: PrefixTarget::Vec(buf),
        }
    }

    pub fn write_bytes(&mut self, data: &[u8]) {
        for &byte in data {
            self.buffer.push(byte);
            if byte == b'\n' {
                self.flush_line();
            }
        }
    }

    /// Flush any remaining partial line.
    pub fn flush_remaining(&mut self) {
        if !self.buffer.is_empty() {
            self.flush_line();
        }
    }

    fn flush_line(&mut self) {
        let line = String::from_utf8_lossy(&self.buffer)
            .trim_end_matches('\n')
            .to_string();
        match &mut self.target {
            PrefixTarget::Shared { writer, is_stderr } => {
                if *is_stderr {
                    writer.write_stderr_line(self.prefix, &line);
                } else {
                    writer.write_stdout_line(self.prefix, &line);
                }
            }
            PrefixTarget::Vec(buf) => {
                buf.extend_from_slice(format!("[{}] {}\n", self.prefix, line).as_bytes());
            }
        }
        self.buffer.clear();
    }
}
```

- [ ] **Step 4: Write more tests and verify**

```rust
#[test]
fn test_prefix_multiple_lines() {
    let mut buf = Vec::new();
    let mut writer = PrefixedWriter::new("lib", &mut buf);
    writer.write_bytes(b"line 1\nline 2\n");
    let output = String::from_utf8(buf).unwrap();
    assert_eq!(output, "[lib] line 1\n[lib] line 2\n");
}

#[test]
fn test_prefix_partial_then_complete() {
    let mut buf = Vec::new();
    let mut writer = PrefixedWriter::new("test", &mut buf);
    writer.write_bytes(b"hel");
    writer.write_bytes(b"lo\n");
    assert_eq!(String::from_utf8(buf).unwrap(), "[test] hello\n");
}

#[test]
fn test_flush_remaining_partial_line() {
    let mut buf = Vec::new();
    let mut writer = PrefixedWriter::new("x", &mut buf);
    writer.write_bytes(b"no newline");
    writer.flush_remaining();
    assert_eq!(String::from_utf8(buf).unwrap(), "[x] no newline\n");
}
```

Run: `cargo test --lib scheduler::output`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add src/scheduler/output.rs src/scheduler/mod.rs
git commit -m "feat(scheduler): add prefixed line-buffered output writer"
```

---

## Chunk 2: Worker Pool & Scheduler

### Task 3: Worker Pool

**Files:**
- Create: `src/scheduler/pool.rs`
- Modify: `src/scheduler/mod.rs` — add `pub mod pool;`

The worker pool manages N threads, each with a Lua VM. Workers pull work from a shared queue and send results back.

- [ ] **Step 1: Write failing test for WorkerPool basic execution**

```rust
// src/scheduler/pool.rs
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pool_executes_shell_command() {
        let dir = tempfile::tempdir().unwrap();
        let writer = SharedWriter::new();
        let (result_tx, result_rx) = std::sync::mpsc::channel();
        let pool = WorkerPool::new(
            1,
            dir.path().to_path_buf(),
            std::collections::HashMap::new(),
            writer,
            result_tx,
        );
        pool.submit(WorkItem {
            id: 0,
            payload: WorkPayload::Shell { cmd: "echo hello".into(), line: 1 },
            recipe_name: "test".into(),
        });
        let result = result_rx.recv_timeout(std::time::Duration::from_secs(5)).unwrap();
        assert_eq!(result.id, 0);
        assert!(result.success);
        pool.shutdown();
    }
}
```

- [ ] **Step 2: Run test — fails**

Run: `cargo test --lib scheduler::pool::tests::test_pool_executes_shell_command`

- [ ] **Step 3: Implement WorkerPool**

Key types and implementation:

```rust
// src/scheduler/pool.rs
use crate::scheduler::dag::WorkPayload;
use crate::scheduler::output::SharedWriter;
use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::{Arc, Condvar, Mutex, mpsc};
use std::thread;

pub struct WorkItem {
    pub id: usize,
    pub payload: WorkPayload,
    pub recipe_name: String,
}

pub struct WorkResult {
    pub id: usize,
    pub success: bool,
    pub error: Option<String>,
}

enum QueueItem {
    Work(WorkItem),
    Shutdown,
}

struct SharedQueue {
    queue: Mutex<VecDeque<QueueItem>>,
    condvar: Condvar,
}

impl SharedQueue {
    fn new() -> Self {
        Self {
            queue: Mutex::new(VecDeque::new()),
            condvar: Condvar::new(),
        }
    }

    fn push(&self, item: QueueItem) {
        self.queue.lock().unwrap().push_back(item);
        self.condvar.notify_one();
    }

    fn pop(&self) -> QueueItem {
        let mut queue = self.queue.lock().unwrap();
        loop {
            if let Some(item) = queue.pop_front() {
                return item;
            }
            queue = self.condvar.wait(queue).unwrap();
        }
    }
}

pub struct WorkerPool {
    threads: Vec<thread::JoinHandle<()>>,
    queue: Arc<SharedQueue>,
}

impl WorkerPool {
    pub fn new(
        num_workers: usize,
        working_dir: PathBuf,
        env_vars: HashMap<String, String>,
        writer: SharedWriter,
        result_tx: mpsc::Sender<WorkResult>,
    ) -> Self {
        let queue = Arc::new(SharedQueue::new());
        let mut threads = Vec::new();

        for _ in 0..num_workers {
            let q = queue.clone();
            let wd = working_dir.clone();
            let env = env_vars.clone();
            let tx = result_tx.clone();
            let w = writer.clone();

            threads.push(thread::spawn(move || {
                // Create Lua VM for this worker
                let lua = mlua::Lua::new();
                // Register fs.* and path.* APIs (stateless, safe for workers)
                crate::runtime::api::register_fs_api(&lua).unwrap();
                crate::runtime::api::register_path_api(&lua).unwrap();
                // Register worker-mode cook.* APIs (exec/sh with prefixed output)
                // (implemented in step below)
                worker_loop(&lua, &q, &wd, &env, &w, &tx);
            }));
        }

        Self { threads, queue }
    }

    pub fn submit(&self, item: WorkItem) {
        self.queue.push(QueueItem::Work(item));
    }

    pub fn shutdown(self) {
        for _ in &self.threads {
            self.queue.push(QueueItem::Shutdown);
        }
        for t in self.threads {
            let _ = t.join();
        }
    }
}

fn worker_loop(
    lua: &mlua::Lua,
    queue: &SharedQueue,
    working_dir: &PathBuf,
    env_vars: &HashMap<String, String>,
    writer: &SharedWriter,
    result_tx: &mpsc::Sender<WorkResult>,
) {
    loop {
        match queue.pop() {
            QueueItem::Shutdown => break,
            QueueItem::Work(item) => {
                let result = execute_work_item(
                    lua, &item, working_dir, env_vars, writer,
                );
                let _ = result_tx.send(WorkResult {
                    id: item.id,
                    success: result.is_ok(),
                    error: result.err(),
                });
            }
        }
    }
}

fn execute_work_item(
    lua: &mlua::Lua,
    item: &WorkItem,
    working_dir: &PathBuf,
    env_vars: &HashMap<String, String>,
    writer: &SharedWriter,
) -> Result<(), String> {
    match &item.payload {
        WorkPayload::Shell { cmd, line } => {
            run_prefixed_shell(cmd, *line, working_dir, env_vars, &item.recipe_name, writer)
        }
        WorkPayload::LuaChunk { code, input, output, ingredient_groups } => {
            // Set up variables on Lua VM, execute chunk
            setup_lua_context(lua, input, output, ingredient_groups, &item.recipe_name, working_dir, env_vars, writer)
                .map_err(|e| e.to_string())?;
            lua.load(code.as_str()).exec().map_err(|e| e.to_string())
        }
    }
}

fn run_prefixed_shell(
    cmd: &str,
    line: usize,
    working_dir: &PathBuf,
    env_vars: &HashMap<String, String>,
    recipe_name: &str,
    writer: &SharedWriter,
) -> Result<(), String> {
    let mut child_env: HashMap<String, String> = std::env::vars().collect();
    for (k, v) in env_vars {
        child_env.insert(k.clone(), v.clone());
    }

    let output = std::process::Command::new("/bin/sh")
        .arg("-c")
        .arg(cmd)
        .current_dir(working_dir)
        .envs(&child_env)
        .output()
        .map_err(|e| format!("failed to execute: {e}"))?;

    // Write stderr lines with prefix
    if !output.stderr.is_empty() {
        let stderr_str = String::from_utf8_lossy(&output.stderr);
        for line in stderr_str.lines() {
            writer.write_stderr_line(recipe_name, line);
        }
    }

    // Write stdout lines with prefix
    if !output.stdout.is_empty() {
        let stdout_str = String::from_utf8_lossy(&output.stdout);
        for line in stdout_str.lines() {
            writer.write_stdout_line(recipe_name, line);
        }
    }

    if !output.status.success() {
        let code = output.status.code().unwrap_or(1);
        return Err(format!("COOK_CMD_FAILED:{}:{}:{}", line, code, cmd));
    }

    Ok(())
}

fn setup_lua_context(
    lua: &mlua::Lua,
    input: &str,
    output: &str,
    ingredient_groups: &[Vec<String>],
    recipe_name: &str,
    working_dir: &PathBuf,
    env_vars: &HashMap<String, String>,
    writer: &SharedWriter,
) -> mlua::Result<()> {
    // Set input/output globals
    lua.globals().set("input", input)?;
    lua.globals().set("output", output)?;

    // Set ingredient groups: input_1, input_2, ...
    for (i, group) in ingredient_groups.iter().enumerate() {
        let table = lua.create_table()?;
        for (j, path) in group.iter().enumerate() {
            table.set(j + 1, path.as_str())?;
        }
        lua.globals().set(format!("input_{}", i + 1), table)?;
    }

    // Register cook.sh and cook.exec for this execution context
    let cook = lua.create_table()?;
    let wd = working_dir.clone();
    let env = env_vars.clone();
    let rn = recipe_name.to_string();
    let w = writer.clone();
    let sh_fn = lua.create_function(move |_, cmd: String| {
        run_prefixed_shell(&cmd, 0, &wd, &env, &rn, &w)
            .map_err(|e| mlua::Error::runtime(e))?;
        Ok(()) // cook.sh in worker mode doesn't return stdout (simplification)
    })?;
    cook.set("sh", sh_fn)?;

    // cook.env table
    let env_table = lua.create_table()?;
    for (key, value) in env_vars {
        env_table.set(key.as_str(), value.as_str())?;
    }
    cook.set("env", env_table)?;

    lua.globals().set("cook", cook)?;
    Ok(())
}
```

**Important:** `cook.sh` in worker mode returns `()` instead of stdout. This is a simplification — Lua blocks in worker VMs that depend on cook.sh return values won't work. Document this as a known limitation.

- [ ] **Step 4: Run tests, verify they pass**

Run: `cargo test --lib scheduler::pool`
Expected: PASS

- [ ] **Step 5: Write more pool tests**

```rust
#[test]
fn test_pool_multiple_workers() {
    let dir = tempfile::tempdir().unwrap();
    let writer = SharedWriter::new();
    let (tx, rx) = mpsc::channel();
    let pool = WorkerPool::new(4, dir.path().to_path_buf(), HashMap::new(), writer, tx);

    for i in 0..8 {
        pool.submit(WorkItem {
            id: i,
            payload: WorkPayload::Shell { cmd: "true".into(), line: 1 },
            recipe_name: "test".into(),
        });
    }

    let mut completed = Vec::new();
    for _ in 0..8 {
        let r = rx.recv_timeout(Duration::from_secs(10)).unwrap();
        assert!(r.success);
        completed.push(r.id);
    }
    assert_eq!(completed.len(), 8);
    pool.shutdown();
}

#[test]
fn test_pool_reports_shell_failure() {
    let dir = tempfile::tempdir().unwrap();
    let writer = SharedWriter::new();
    let (tx, rx) = mpsc::channel();
    let pool = WorkerPool::new(1, dir.path().to_path_buf(), HashMap::new(), writer, tx);

    pool.submit(WorkItem {
        id: 0,
        payload: WorkPayload::Shell { cmd: "false".into(), line: 5 },
        recipe_name: "fail".into(),
    });

    let result = rx.recv_timeout(Duration::from_secs(5)).unwrap();
    assert!(!result.success);
    assert!(result.error.unwrap().contains("COOK_CMD_FAILED"));
    pool.shutdown();
}
```

Run: `cargo test --lib scheduler::pool`
Expected: PASS

- [ ] **Step 6: Commit**

```bash
git add src/scheduler/pool.rs src/scheduler/output.rs src/scheduler/mod.rs
git commit -m "feat(scheduler): add worker pool with Lua VM threads"
```

### Task 4: Scheduler Loop

**Files:**
- Modify: `src/scheduler/mod.rs` — add scheduler logic

The scheduler owns the DAG. It seeds the ready queue, processes completion messages, and propagates failures.

- [ ] **Step 1: Write failing test for basic scheduler execution**

```rust
// src/scheduler/mod.rs
#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_scheduler_runs_single_node() {
        let dir = TempDir::new().unwrap();
        let mut dag = ExecutionDag::new();
        dag.add_node(
            WorkPayload::Shell { cmd: "true".into(), line: 1 },
            "test".into(), None, vec![],
        );
        let result = execute_dag(dag, 1, dir.path(), &HashMap::new(), false);
        assert!(result.is_ok());
    }
}
```

- [ ] **Step 2: Run test — fails**

Run: `cargo test --lib scheduler::tests::test_scheduler_runs_single_node`

- [ ] **Step 3: Implement execute_dag**

```rust
// In src/scheduler/mod.rs
pub mod dag;
pub mod output;
pub mod pool;

use dag::ExecutionDag;
use output::SharedWriter;
use pool::{WorkItem, WorkerPool};
use std::collections::HashMap;
use std::path::Path;

pub struct SchedulerError {
    pub failures: Vec<(usize, String)>, // (node_id, error_message)
}

impl std::fmt::Display for SchedulerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for (id, msg) in &self.failures {
            writeln!(f, "  node {id}: {msg}")?;
        }
        Ok(())
    }
}

pub fn execute_dag(
    dag: ExecutionDag,
    num_workers: usize,
    working_dir: &Path,
    env_vars: &HashMap<String, String>,
    quiet: bool,
) -> Result<(), SchedulerError> {
    if dag.is_empty() {
        return Ok(());
    }

    let writer = SharedWriter::new();
    let (result_tx, result_rx) = std::sync::mpsc::channel();
    let pool = WorkerPool::new(
        num_workers,
        working_dir.to_path_buf(),
        env_vars.clone(),
        writer.clone(),
        result_tx,
    );

    // Seed: enqueue pre-satisfied nodes and initial ready work nodes
    let mut pending = dag.len();
    let mut failures: Vec<(usize, String)> = Vec::new();
    let mut cancelled: Vec<bool> = vec![false; dag.len()];

    for id in dag.initial_ready() {
        let node = dag.node(id);
        if node.payload.is_none() {
            // Pre-satisfied: immediately complete
            pending -= 1;
            for newly_ready in dag.complete(id) {
                enqueue_or_skip(&dag, newly_ready, &pool, &mut pending, &cancelled);
            }
        } else {
            pool.submit(WorkItem {
                id: node.id,
                payload: node.payload.clone().unwrap(),
                recipe_name: node.recipe_name.clone(),
            });
        }
    }

    // Main scheduler loop
    while pending > 0 {
        let result = result_rx.recv().unwrap();
        pending -= 1;

        if result.success {
            // Notify scheduler about completed cache writes here (future)
            for newly_ready in dag.complete(result.id) {
                if cancelled[newly_ready] {
                    pending -= 1;
                    continue;
                }
                enqueue_or_skip(&dag, newly_ready, &pool, &mut pending, &cancelled);
            }
        } else {
            let error_msg = result.error.unwrap_or_else(|| "unknown error".into());
            failures.push((result.id, error_msg));
            // Cancel downstream subgraph
            cancel_downstream(&dag, result.id, &mut cancelled, &mut pending);
        }
    }

    pool.shutdown();

    if failures.is_empty() {
        Ok(())
    } else {
        Err(SchedulerError { failures })
    }
}

fn enqueue_or_skip(
    dag: &ExecutionDag,
    id: usize,
    pool: &WorkerPool,
    pending: &mut usize,
    cancelled: &[bool],
) {
    let node = dag.node(id);
    if node.payload.is_none() {
        // Pre-satisfied, immediately complete
        *pending -= 1;
        for sub_id in dag.complete(id) {
            if !cancelled[sub_id] {
                enqueue_or_skip(dag, sub_id, pool, pending, cancelled);
            }
        }
    } else {
        pool.submit(WorkItem {
            id: node.id,
            payload: node.payload.clone().unwrap(),
            recipe_name: node.recipe_name.clone(),
        });
    }
}

fn cancel_downstream(
    dag: &ExecutionDag,
    id: usize,
    cancelled: &mut Vec<bool>,
    pending: &mut usize,
) {
    for &dep_id in &dag.node(id).dependents {
        if !cancelled[dep_id] {
            cancelled[dep_id] = true;
            // This node will never be submitted, so decrement pending
            // But only if it hasn't been submitted yet
            // Actually: it can't have been submitted because its dep just failed
            // and remaining_deps > 0
            cancel_downstream(dag, dep_id, cancelled, pending);
        }
    }
}
```

- [ ] **Step 4: Run test — passes**

Run: `cargo test --lib scheduler::tests::test_scheduler_runs_single_node`

- [ ] **Step 5: Write comprehensive scheduler tests**

```rust
#[test]
fn test_scheduler_respects_dependencies() {
    let dir = TempDir::new().unwrap();
    let marker = dir.path().join("marker.txt");
    let mut dag = ExecutionDag::new();
    let a = dag.add_node(
        WorkPayload::Shell {
            cmd: format!("echo first > {}", marker.display()),
            line: 1,
        },
        "test".into(), None, vec![],
    );
    dag.add_node(
        WorkPayload::Shell {
            cmd: format!("cat {} && echo second >> {}", marker.display(), marker.display()),
            line: 2,
        },
        "test".into(), None, vec![a],
    );
    let result = execute_dag(dag, 1, dir.path(), &HashMap::new(), false);
    assert!(result.is_ok());
    let content = std::fs::read_to_string(&marker).unwrap();
    assert!(content.contains("first"));
    assert!(content.contains("second"));
}

#[test]
fn test_scheduler_failure_cancels_downstream() {
    let dir = TempDir::new().unwrap();
    let mut dag = ExecutionDag::new();
    let a = dag.add_node(
        WorkPayload::Shell { cmd: "false".into(), line: 1 },
        "test".into(), None, vec![],
    );
    dag.add_node(
        WorkPayload::Shell { cmd: "echo should-not-run".into(), line: 2 },
        "test".into(), None, vec![a],
    );
    let result = execute_dag(dag, 1, dir.path(), &HashMap::new(), false);
    assert!(result.is_err());
}

#[test]
fn test_scheduler_parallel_independent_nodes() {
    let dir = TempDir::new().unwrap();
    let mut dag = ExecutionDag::new();
    // 4 independent sleep commands — should complete in ~1s with 4 workers, not ~4s
    for i in 0..4 {
        dag.add_node(
            WorkPayload::Shell { cmd: "sleep 0.2".into(), line: i + 1 },
            "test".into(), None, vec![],
        );
    }
    let start = std::time::Instant::now();
    let result = execute_dag(dag, 4, dir.path(), &HashMap::new(), false);
    let elapsed = start.elapsed();
    assert!(result.is_ok());
    // With 4 workers, 4 × 0.2s sleeps should finish in ~0.2s, not ~0.8s
    assert!(elapsed.as_millis() < 600, "took too long: {:?}", elapsed);
}

#[test]
fn test_scheduler_empty_dag() {
    let dir = TempDir::new().unwrap();
    let dag = ExecutionDag::new();
    let result = execute_dag(dag, 1, dir.path(), &HashMap::new(), false);
    assert!(result.is_ok());
}
```

Run: `cargo test --lib scheduler::tests`
Expected: PASS

- [ ] **Step 6: Commit**

```bash
git add src/scheduler/mod.rs
git commit -m "feat(scheduler): add DAG scheduler loop with failure propagation"
```

---

## Chunk 3: Registration Mode & Codegen

### Task 5: Registration-Mode Runtime

**Files:**
- Modify: `src/runtime/mod.rs` — add `register_recipe()` method
- Modify: `src/runtime/api.rs` — add `register_layer_api_capture()` and `register_cook_api_capture()`

The key change: a new registration mode where `cook.layer()` captures work units instead of executing, and `cook.exec()`/`cook.sh()` record commands instead of running them.

- [ ] **Step 1: Define RegistrationState and RecipeUnits types**

Add to `src/runtime/mod.rs`:

```rust
use crate::scheduler::dag::{WorkPayload, CacheMeta};

/// A captured work unit from registration.
pub struct CapturedUnit {
    pub payload: WorkPayload,
    pub cache_meta: Option<CacheMeta>,
    pub dep_kind: DepKind,
}

/// How this unit relates to others in the recipe.
pub enum DepKind {
    /// Part of a cook step group (can run parallel with siblings)
    StepGroup(usize), // group index
    /// Sequential barrier (depends on all prior units)
    Sequential,
}

/// Result of registering a single recipe.
pub struct RecipeUnits {
    pub recipe_name: String,
    pub units: Vec<CapturedUnit>,
    pub step_groups: Vec<Vec<usize>>, // group_index -> unit indices within `units`
}
```

- [ ] **Step 2: Write failing test for register_recipe**

```rust
#[test]
fn test_register_captures_shell_step() {
    let dir = TempDir::new().unwrap();
    let rt = make_runtime(dir.path());
    let lua_src = r#"
cook.recipe("build", {}, function()
    cook.exec([[echo hello]], 1)
end)
"#;
    let result = rt.register_recipe(lua_src, "build").unwrap();
    assert_eq!(result.units.len(), 1);
    match &result.units[0].payload {
        WorkPayload::Shell { cmd, line } => {
            assert_eq!(cmd, "echo hello");
            assert_eq!(*line, 1);
        }
        _ => panic!("expected Shell payload"),
    }
}
```

- [ ] **Step 3: Run test — fails**

Run: `cargo test --lib runtime::tests::test_register_captures_shell_step`

- [ ] **Step 4: Implement register_recipe and capture APIs**

This is the most complex change. The approach:

1. Create a Lua VM on the main thread (same as today)
2. Register a capture-mode `cook.exec` that records commands into a shared state
3. Register a capture-mode `cook.layer` that does cache checks, then dry-runs the body to capture inner cook.exec calls
4. Add `cook.begin_step()` / `cook.end_step()` handlers that track step group boundaries
5. Run the recipe Lua — all cook.layer/cook.exec calls get captured
6. Return `RecipeUnits` with the captured units

In `src/runtime/api.rs`, add:

```rust
use std::cell::RefCell;
use std::rc::Rc;
use crate::scheduler::dag::{WorkPayload, CacheMeta};

pub struct CaptureState {
    /// Whether we're inside a cook.layer dry-run
    pub inside_layer: bool,
    /// Commands captured during a layer dry-run
    pub layer_commands: Vec<(String, usize)>,
    /// All captured units from this recipe
    pub units: Vec<super::CapturedUnit>,
    /// Current step group index (incremented by begin_step/end_step)
    pub current_group: Option<usize>,
    /// Step groups: group_index -> unit indices
    pub step_groups: Vec<Vec<usize>>,
}

impl CaptureState {
    pub fn new() -> Self {
        Self {
            inside_layer: false,
            layer_commands: Vec::new(),
            units: Vec::new(),
            current_group: None,
            step_groups: Vec::new(),
        }
    }
}

pub type SharedCaptureState = Rc<RefCell<CaptureState>>;
```

Register capture-mode APIs:

```rust
pub fn register_cook_api_capture(
    lua: &Lua,
    env_vars: &HashMap<String, String>,
    working_dir: &PathBuf,
    capture_state: SharedCaptureState,
) -> LuaResult<Rc<RefCell<Vec<RegisteredRecipe>>>> {
    let recipes: Rc<RefCell<Vec<RegisteredRecipe>>> = Rc::new(RefCell::new(Vec::new()));
    let cook = lua.create_table()?;

    // cook.recipe — same as normal mode
    // (same code as register_cook_api's recipe_fn)
    let recipes_clone = recipes.clone();
    let recipe_fn = lua.create_function(move |lua, (name, meta, func): (String, LuaTable, LuaFunction)| {
        // ... same registration logic ...
    })?;
    cook.set("recipe", recipe_fn)?;

    // cook.exec — capture mode
    let cs = capture_state.clone();
    let exec_fn = lua.create_function(move |_, (cmd, line): (String, usize)| {
        let mut state = cs.borrow_mut();
        if state.inside_layer {
            // Inside a dry-run: record for the layer
            state.layer_commands.push((cmd, line));
        } else {
            // Top-level: create a standalone unit
            let unit_idx = state.units.len();
            state.units.push(super::CapturedUnit {
                payload: WorkPayload::Shell { cmd, line },
                cache_meta: None,
                dep_kind: super::DepKind::Sequential,
            });
        }
        Ok("".to_string()) // Return empty string (no real execution)
    })?;
    cook.set("exec", exec_fn)?;

    // cook.sh — capture mode (same as exec but line=0)
    let cs2 = capture_state.clone();
    let sh_fn = lua.create_function(move |_, cmd: String| {
        let mut state = cs2.borrow_mut();
        if state.inside_layer {
            state.layer_commands.push((cmd, 0));
        } else {
            let unit_idx = state.units.len();
            state.units.push(super::CapturedUnit {
                payload: WorkPayload::Shell { cmd, line: 0 },
                cache_meta: None,
                dep_kind: super::DepKind::Sequential,
            });
        }
        Ok("".to_string())
    })?;
    cook.set("sh", sh_fn)?;

    // cook.begin_step() — marks start of a cook step group
    let cs3 = capture_state.clone();
    let begin_fn = lua.create_function(move |_, ()| {
        let mut state = cs3.borrow_mut();
        let group_idx = state.step_groups.len();
        state.step_groups.push(Vec::new());
        state.current_group = Some(group_idx);
        Ok(())
    })?;
    cook.set("begin_step", begin_fn)?;

    // cook.end_step() — marks end of a cook step group
    let cs4 = capture_state.clone();
    let end_fn = lua.create_function(move |_, ()| {
        cs4.borrow_mut().current_group = None;
        Ok(())
    })?;
    cook.set("end_step", end_fn)?;

    // cook.taste — no-op in registration
    cook.set("taste", lua.create_function(|_, _: usize| Ok(()))?)?;

    // cook.env table
    let env_table = lua.create_table()?;
    for (key, value) in env_vars {
        env_table.set(key.as_str(), value.as_str())?;
    }
    cook.set("env", env_table)?;

    lua.globals().set("cook", cook)?;
    Ok(recipes)
}
```

Register capture-mode `cook.layer`:

```rust
pub fn register_layer_api_capture(
    lua: &Lua,
    cache_state: SharedCacheState,
    capture_state: SharedCaptureState,
    working_dir: &std::path::Path,
) -> LuaResult<()> {
    let cook: LuaTable = lua.globals().get("cook")?;
    let wd = working_dir.to_path_buf();

    let layer_fn = lua.create_function(
        move |lua, args: mlua::MultiValue| {
            // Parse args: (inputs, output, hash, body_fn, optional_lua_code)
            let mut iter = args.into_iter();
            let inputs = iter.next().unwrap_or(LuaValue::Nil);
            let output = iter.next().unwrap_or(LuaValue::Nil);
            let hash: u64 = match iter.next() {
                Some(LuaValue::Integer(n)) => n as u64,
                Some(LuaValue::Number(n)) => n as u64,
                _ => 0,
            };
            let body: LuaFunction = match iter.next() {
                Some(LuaValue::Function(f)) => f,
                _ => return Err(mlua::Error::runtime("cook.layer: expected function")),
            };
            let lua_block_code: Option<String> = match iter.next() {
                Some(LuaValue::String(s)) => Some(s.to_string_lossy().to_string()),
                _ => None,
            };

            // Parse input/output strings (same as current layer_fn)
            let input_strs = parse_inputs(&inputs);
            let output_str = parse_output(&output);

            // Cache check (same logic as current register_layer_api)
            let cache_key = compute_cache_key(&input_strs, &output_str, hash);
            let mut cstate = cache_state.borrow_mut();
            let existing = cstate.cache.steps.get(&cache_key);

            let (result, updated_entry) = if output_str.is_some() {
                crate::cache::check::needs_rebuild_cook(
                    existing,
                    &input_strs.iter().map(|s| s.as_str()).collect::<Vec<_>>(),
                    output_str.as_deref().unwrap(),
                    hash, &wd,
                )
            } else {
                crate::cache::check::needs_rebuild_plate(
                    existing,
                    &input_strs.iter().map(|s| s.as_str()).collect::<Vec<_>>(),
                    hash, &wd,
                )
            };

            let mut capstate = capture_state.borrow_mut();

            if let crate::cache::check::RebuildResult::Skip = &result {
                // Pre-satisfied: update cache entry, add presatisfied unit
                if let Some(entry) = updated_entry {
                    cstate.cache.steps.insert(cache_key, entry);
                    cstate.dirty = true;
                }
                let unit_idx = capstate.units.len();
                let dep_kind = if let Some(group) = capstate.current_group {
                    capstate.step_groups[group].push(unit_idx);
                    super::DepKind::StepGroup(group)
                } else {
                    super::DepKind::Sequential
                };
                capstate.units.push(super::CapturedUnit {
                    payload: WorkPayload::Shell { cmd: String::new(), line: 0 }, // placeholder
                    cache_meta: None,
                    dep_kind,
                });
                // Mark as presatisfied (payload = None will be set in DAG builder)
                return Ok(());
            }

            drop(cstate); // Release borrow before dry-run

            // Need rebuild — capture the work
            let cache_meta = Some(CacheMeta {
                recipe_name: String::new(), // filled by DAG builder
                cache_key: cache_key.clone(),
                input_paths: input_strs.clone(),
                output_path: output_str.clone(),
                command_hash: hash,
            });

            let payload = if let Some(code) = lua_block_code {
                // Lua block: capture code and variable bindings
                let recipe_table: LuaTable = lua.globals().get("recipe")?;
                let ingredients_table: LuaTable = recipe_table.get("ingredients")?;
                let mut groups = Vec::new();
                let len = ingredients_table.len()?;
                for i in 1..=len {
                    let group: LuaTable = ingredients_table.get(i)?;
                    let mut files = Vec::new();
                    let glen = group.len()?;
                    for j in 1..=glen {
                        files.push(group.get::<String>(j)?);
                    }
                    groups.push(files);
                }
                WorkPayload::LuaChunk {
                    code,
                    input: input_strs.first().cloned().unwrap_or_default(),
                    output: output_str.unwrap_or_default(),
                    ingredient_groups: groups,
                }
            } else {
                // Shell: dry-run to capture cook.exec call
                capstate.inside_layer = true;
                capstate.layer_commands.clear();
                drop(capstate); // Release before calling Lua

                body.call::<()>(())?;

                let mut capstate = capture_state.borrow_mut();
                capstate.inside_layer = false;

                if capstate.layer_commands.len() == 1 {
                    let (cmd, line) = capstate.layer_commands.pop().unwrap();
                    WorkPayload::Shell { cmd, line }
                } else if capstate.layer_commands.is_empty() {
                    // Layer body didn't call cook.exec — unusual but handle it
                    WorkPayload::Shell { cmd: "true".into(), line: 0 }
                } else {
                    // Multiple commands — shouldn't happen for shell cook steps
                    // but handle gracefully: take the last one
                    let (cmd, line) = capstate.layer_commands.pop().unwrap();
                    capstate.layer_commands.clear();
                    WorkPayload::Shell { cmd, line }
                }
            };

            let mut capstate = capture_state.borrow_mut();
            let unit_idx = capstate.units.len();
            let dep_kind = if let Some(group) = capstate.current_group {
                capstate.step_groups[group].push(unit_idx);
                super::DepKind::StepGroup(group)
            } else {
                super::DepKind::Sequential
            };
            capstate.units.push(super::CapturedUnit {
                payload,
                cache_meta,
                dep_kind,
            });

            Ok(())
        },
    )?;

    cook.set("layer", layer_fn)?;
    Ok(())
}
```

Then implement `register_recipe` on `Runtime`:

```rust
// In src/runtime/mod.rs
impl Runtime {
    pub fn register_recipe(&self, lua_source: &str, recipe_name: &str) -> Result<RecipeUnits, RuntimeError> {
        let lua = Lua::new();
        let capture_state: SharedCaptureState = Rc::new(RefCell::new(api::CaptureState::new()));

        let recipes = api::register_cook_api_capture(
            &lua, &self.env_vars, &self.working_dir, capture_state.clone(),
        )?;
        api::register_fs_api(&lua)?;
        api::register_path_api(&lua)?;

        let cache_dir = self.working_dir.join(".cook").join("cache");
        let cache = RecipeCache::load(&cache_dir, recipe_name).unwrap_or_default();
        let cache_state: SharedCacheState = Rc::new(RefCell::new(
            CacheState::new(cache, cache_dir, recipe_name.to_string()),
        ));
        api::register_layer_api_capture(&lua, cache_state.clone(), capture_state.clone(), &self.working_dir)?;

        lua.load(lua_source).exec()?;

        let registry = recipes.borrow();
        let recipe = registry
            .iter()
            .find(|r| r.name == recipe_name)
            .ok_or_else(|| RuntimeError::RecipeNotFound(recipe_name.to_string()))?;

        // Recipe-level invalidation (same as execute_recipe)
        let current_env_hash = crate::cache::check::hash_env(&self.env_vars);
        let current_secondary_hash = crate::cache::check::hash_secondary_inputs(
            &self.working_dir, &recipe.metadata.ingredients,
        );
        {
            let mut state = cache_state.borrow_mut();
            if state.cache.env_hash != current_env_hash
                || state.cache.secondary_inputs_hash != current_secondary_hash
            {
                state.cache = RecipeCache::new();
                state.dirty = true;
            }
            state.cache.env_hash = current_env_hash;
            state.cache.secondary_inputs_hash = current_secondary_hash;
        }

        // Build recipe context and set globals (same as execute_recipe)
        // ... recipe_table setup, ingredient resolution, glob recording ...
        // (extract this into a helper to share with execute_recipe)

        let recipe_table = lua.create_table()?;
        recipe_table.set("name", recipe.name.as_str())?;
        // ... ingredient resolution (same code as execute_recipe:100-158) ...
        lua.globals().set("recipe", recipe_table)?;

        // Execute recipe function — this populates capture_state
        let func: LuaFunction = lua.registry_value(&recipe.function)?;
        func.call::<()>(())?;

        // Flush cache (for any updated entries from skip checks)
        cache_state.borrow_mut().flush().map_err(|e| {
            RuntimeError::Lua(mlua::Error::runtime(format!("cache flush failed: {e}")))
        })?;

        let state = capture_state.borrow();
        Ok(RecipeUnits {
            recipe_name: recipe_name.to_string(),
            units: state.units.drain(..).collect(), // this won't work with borrow, need to take ownership
            step_groups: state.step_groups.clone(),
        })
    }
}
```

**Note to implementor:** The ingredient resolution and glob recording code in `execute_recipe` (lines 96-158 of `runtime/mod.rs`) should be extracted into a shared helper function that both `execute_recipe` and `register_recipe` call. Do not duplicate the code.

- [ ] **Step 5: Run test — passes**

Run: `cargo test --lib runtime::tests::test_register_captures_shell_step`

- [ ] **Step 6: Write more registration tests**

```rust
#[test]
fn test_register_captures_layer_shell() {
    let dir = TempDir::new().unwrap();
    fs::create_dir_all(dir.path().join("src")).unwrap();
    fs::write(dir.path().join("src/a.c"), "int main(){}").unwrap();
    let rt = make_runtime(dir.path());
    let lua_src = r#"
cook.recipe("build", {ingredients = {"src/*.c"}}, function()
    cook.begin_step()
    for _, _cook_in in ipairs(recipe.ingredients[1]) do
        local _cook_out = "build/" .. path.stem(_cook_in) .. ".o"
        cook.layer(_cook_in, _cook_out, 12345, function()
            cook.exec("gcc -c " .. _cook_in .. " -o " .. _cook_out, 3)
        end)
    end
    cook.end_step()
end)
"#;
    let result = rt.register_recipe(lua_src, "build").unwrap();
    assert_eq!(result.units.len(), 1);
    match &result.units[0].payload {
        WorkPayload::Shell { cmd, .. } => {
            assert!(cmd.contains("gcc -c"));
            assert!(cmd.contains("src/a.c"));
        }
        _ => panic!("expected Shell"),
    }
}

#[test]
fn test_register_skips_cached_layer() {
    let dir = TempDir::new().unwrap();
    fs::create_dir_all(dir.path().join("src")).unwrap();
    fs::create_dir_all(dir.path().join("build")).unwrap();
    fs::write(dir.path().join("src/a.c"), "content").unwrap();
    fs::write(dir.path().join("build/a.o"), "output").unwrap();
    let rt = make_runtime(dir.path());

    // First registration: no cache, should capture
    let lua_src = r#"
cook.recipe("build", {ingredients = {"src/*.c"}}, function()
    cook.begin_step()
    for _, _cook_in in ipairs(recipe.ingredients[1]) do
        local _cook_out = "build/" .. path.stem(_cook_in) .. ".o"
        cook.layer(_cook_in, _cook_out, 99, function()
            cook.exec("cp " .. _cook_in .. " " .. _cook_out, 2)
        end)
    end
    cook.end_step()
end)
"#;
    // Run actual execution first to populate cache
    rt.execute_recipe(lua_src, "build").unwrap();

    // Second registration: cache hit, unit should be pre-satisfied
    let result = rt.register_recipe(lua_src, "build").unwrap();
    assert_eq!(result.units.len(), 1);
    // The unit should have no meaningful payload (presatisfied)
    // (check via cache_meta being None for presatisfied units)
    assert!(result.units[0].cache_meta.is_none());
}
```

- [ ] **Step 7: Run all registration tests**

Run: `cargo test --lib runtime::tests`
Expected: PASS (existing tests still pass too)

- [ ] **Step 8: Commit**

```bash
git add src/runtime/mod.rs src/runtime/api.rs
git commit -m "feat(runtime): add registration mode for capturing work units"
```

### Task 6: Codegen Updates

**Files:**
- Modify: `src/codegen/mod.rs`

Changes:
1. Wrap cook step loops with `cook.begin_step()` / `cook.end_step()`
2. Pass raw Lua block source as 5th arg to `cook.layer()` for LuaBlock using clauses
3. Skip `Step::Taste`

- [ ] **Step 1: Write failing test for begin_step/end_step markers**

```rust
#[test]
fn test_cook_step_emits_begin_end_markers() {
    let cookfile = make_cookfile(vec![make_recipe(
        "build", vec![], vec!["src/*.c"],
        vec![Step::Cook {
            step: CookStep {
                output_pattern: "build/{stem}.o".to_string(),
                using_clause: Some(UsingClause::Shell("gcc -c {in} -o {out}".to_string())),
            },
            line: 3,
        }],
    )]);
    let output = generate(&cookfile);
    assert!(output.contains("cook.begin_step()"), "missing begin_step");
    assert!(output.contains("cook.end_step()"), "missing end_step");
}
```

- [ ] **Step 2: Run test — fails**

Run: `cargo test --lib codegen::tests::test_cook_step_emits_begin_end_markers`

- [ ] **Step 3: Add begin_step/end_step to generate_cook_step**

In `src/codegen/mod.rs`, modify the step generation in `generate()`:

```rust
Step::Cook { step: cook_step, line } => {
    cook_index += 1;
    out.push_str("    cook.begin_step()\n");
    generate_cook_step(
        &mut out, cook_step, *line, cook_index,
        prev_cook_index, &recipe.ingredients,
    );
    out.push_str("    cook.end_step()\n");
    prev_cook_index = Some(cook_index);
}
```

Also wrap plate steps:
```rust
Step::Plate { step: plate_step, line } => {
    out.push_str("    cook.begin_step()\n");
    generate_plate_step(&mut out, plate_step, *line, prev_cook_index);
    out.push_str("    cook.end_step()\n");
}
```

Skip taste:
```rust
Step::Taste { .. } => {
    // Taste steps are skipped — will be redesigned for threaded model
}
```

- [ ] **Step 4: Run test — passes**

Run: `cargo test --lib codegen::tests::test_cook_step_emits_begin_end_markers`

- [ ] **Step 5: Write test for Lua block 5th arg**

```rust
#[test]
fn test_lua_block_passes_code_to_layer() {
    let cookfile = make_cookfile(vec![make_recipe(
        "build", vec![], vec!["src/*.c"],
        vec![Step::Cook {
            step: CookStep {
                output_pattern: "build/{stem}.o".to_string(),
                using_clause: Some(UsingClause::LuaBlock(
                    "cook.sh(\"gcc -c \" .. input .. \" -o \" .. output)".to_string(),
                )),
            },
            line: 3,
        }],
    )]);
    let output = generate(&cookfile);
    // Should pass the Lua block source as 5th arg to cook.layer
    assert!(output.contains(", [=["), "missing 5th arg opener");
    assert!(output.contains("]=])"), "missing 5th arg closer");
}
```

- [ ] **Step 6: Implement 5th arg for Lua block cook steps**

In `generate_cook_step`, for `CookMode::OneToOne` with `UsingClause::LuaBlock`:

Change the cook.layer call to include the raw code as a 5th argument:

```rust
Some(UsingClause::LuaBlock(code)) => {
    // ... existing local variable setup ...

    // Close the function with 5th arg: raw Lua block source
    out.push_str("        end, [=[\n");
    for code_line in code.lines() {
        out.push_str(&format!("            {}\n", code_line));
    }
    out.push_str("        ]=])\n");
}
```

Instead of the current:
```rust
out.push_str("        end)\n");
```

- [ ] **Step 7: Run all codegen tests, fix any broken ones**

Run: `cargo test --lib codegen`
Expected: some existing tests may need updated assertions for begin_step/end_step and taste skipping. Fix them.

- [ ] **Step 8: Run full test suite**

Run: `cargo test`
Expected: PASS (existing tests need `cook.begin_step`/`cook.end_step` no-op registered in normal execute_recipe mode too)

**Important:** Update `register_cook_api` in `api.rs` to also register no-op `cook.begin_step` and `cook.end_step` functions so the normal execution path doesn't break:

```rust
// In register_cook_api, after setting up other functions:
cook.set("begin_step", lua.create_function(|_, ()| Ok(()))?)?;
cook.set("end_step", lua.create_function(|_, ()| Ok(()))?)?;
```

- [ ] **Step 9: Commit**

```bash
git add src/codegen/mod.rs src/runtime/api.rs
git commit -m "feat(codegen): add step group markers, lua block 5th arg, skip taste"
```

---

## Chunk 4: DAG Builder, Cache & CLI Integration

### Task 7: DAG Builder

**Files:**
- Create: `src/scheduler/builder.rs`
- Modify: `src/scheduler/mod.rs` — add `pub mod builder;`

The DAG builder takes `RecipeUnits` from each recipe (in topo order) and produces a single `ExecutionDag` with all cross-recipe dependencies wired.

- [ ] **Step 1: Write failing test for single-recipe DAG build**

```rust
// src/scheduler/builder.rs
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_single_recipe() {
        let units = RecipeUnits {
            recipe_name: "build".into(),
            units: vec![
                CapturedUnit {
                    payload: WorkPayload::Shell { cmd: "echo a".into(), line: 1 },
                    cache_meta: None,
                    dep_kind: DepKind::Sequential,
                },
            ],
            step_groups: vec![],
        };
        let dag = build_dag(vec![units]);
        assert_eq!(dag.len(), 1);
    }
}
```

- [ ] **Step 2: Implement build_dag**

```rust
// src/scheduler/builder.rs
use crate::runtime::{RecipeUnits, CapturedUnit, DepKind};
use crate::scheduler::dag::{ExecutionDag, WorkPayload, CacheMeta};

pub fn build_dag(recipe_units: Vec<RecipeUnits>) -> ExecutionDag {
    let mut dag = ExecutionDag::new();

    // Track the "leaf" node IDs of each recipe (last units, for cross-recipe deps)
    let mut recipe_leaves: std::collections::HashMap<String, Vec<usize>> = std::collections::HashMap::new();
    // Track all node IDs per recipe
    let mut recipe_nodes: std::collections::HashMap<String, Vec<usize>> = std::collections::HashMap::new();

    for recipe in recipe_units {
        let mut last_sequential_ids: Vec<usize> = Vec::new();
        let mut current_step_ids: Vec<usize> = Vec::new();
        // Map from local unit index to DAG node ID
        let mut local_to_dag: Vec<usize> = Vec::new();

        // Cross-recipe deps: this recipe depends on all leaves of its prerequisite recipes
        // (The recipe_units vec is in topo order, so prerequisites are already processed)
        // We don't have explicit dep info here — that's handled by the caller
        // For now, we use a simple approach: recipe deps are passed in RecipeUnits
        // TODO: Add recipe_deps field to RecipeUnits

        // Within-recipe dependency wiring
        let mut all_recipe_nodes = Vec::new();

        for (idx, unit) in recipe.units.iter().enumerate() {
            let deps = match &unit.dep_kind {
                DepKind::Sequential => {
                    // Depends on all previously emitted units (barrier)
                    last_sequential_ids.clone()
                }
                DepKind::StepGroup(_) => {
                    // Depends on whatever the last sequential barrier was
                    last_sequential_ids.clone()
                }
            };

            let is_presatisfied = unit.cache_meta.is_none()
                && matches!(&unit.payload, WorkPayload::Shell { cmd, .. } if cmd.is_empty());

            let node_id = if is_presatisfied {
                dag.add_presatisfied(recipe.recipe_name.clone(), deps)
            } else {
                let mut meta = unit.cache_meta.clone();
                if let Some(ref mut m) = meta {
                    m.recipe_name = recipe.recipe_name.clone();
                }
                dag.add_node(
                    unit.payload.clone(),
                    recipe.recipe_name.clone(),
                    meta,
                    deps,
                )
            };

            local_to_dag.push(node_id);
            all_recipe_nodes.push(node_id);

            match &unit.dep_kind {
                DepKind::Sequential => {
                    // Update sequential barrier: just this node
                    last_sequential_ids = vec![node_id];
                }
                DepKind::StepGroup(group_idx) => {
                    // Accumulate step group members
                    current_step_ids.push(node_id);
                    // Check if this is the last member of this group
                    let group = &recipe.step_groups[*group_idx];
                    if group.last() == Some(&idx) {
                        // End of step group: all members become the new sequential barrier
                        last_sequential_ids = current_step_ids.clone();
                        current_step_ids.clear();
                    }
                }
            }
        }

        // If step_ids are pending (shouldn't happen with end_step), add them
        if !current_step_ids.is_empty() {
            last_sequential_ids.extend(current_step_ids);
        }

        recipe_leaves.insert(recipe.recipe_name.clone(), last_sequential_ids);
        recipe_nodes.insert(recipe.recipe_name.clone(), all_recipe_nodes);
    }

    dag
}
```

**Note to implementor:** The cross-recipe dependency wiring needs `RecipeUnits` to carry a `deps: Vec<String>` field listing which recipes this one depends on. During build_dag, when processing recipe B that depends on recipe A, B's root units should depend on A's leaf units. Add this field to `RecipeUnits` and populate it in `register_recipe` from the recipe's metadata requires + implicit deps.

- [ ] **Step 3: Write tests for multi-recipe DAG**

```rust
#[test]
fn test_build_multi_recipe_with_deps() {
    let setup = RecipeUnits {
        recipe_name: "setup".into(),
        deps: vec![],
        units: vec![CapturedUnit {
            payload: WorkPayload::Shell { cmd: "mkdir -p build".into(), line: 1 },
            cache_meta: None,
            dep_kind: DepKind::Sequential,
        }],
        step_groups: vec![],
    };
    let build = RecipeUnits {
        recipe_name: "build".into(),
        deps: vec!["setup".into()],
        units: vec![CapturedUnit {
            payload: WorkPayload::Shell { cmd: "gcc -c a.c".into(), line: 2 },
            cache_meta: None,
            dep_kind: DepKind::StepGroup(0),
        }, CapturedUnit {
            payload: WorkPayload::Shell { cmd: "gcc -c b.c".into(), line: 2 },
            cache_meta: None,
            dep_kind: DepKind::StepGroup(0),
        }],
        step_groups: vec![vec![0, 1]],
    };

    let dag = build_dag(vec![setup, build]);
    // setup(1 node) + build(2 nodes) = 3 nodes
    assert_eq!(dag.len(), 3);
    // build's nodes should depend on setup's node
    assert_eq!(dag.node(1).remaining_deps.load(std::sync::atomic::Ordering::Relaxed), 1);
    assert_eq!(dag.node(2).remaining_deps.load(std::sync::atomic::Ordering::Relaxed), 1);
}
```

- [ ] **Step 4: Run tests**

Run: `cargo test --lib scheduler::builder`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add src/scheduler/builder.rs src/scheduler/mod.rs
git commit -m "feat(scheduler): add DAG builder for multi-recipe dependency wiring"
```

### Task 8: Thread-Safe Cache Updates

**Files:**
- Modify: `src/cache/mod.rs` — add `ThreadSafeCacheManager`

After a work unit completes, the scheduler updates its cache entry. Since multiple workers might complete units from the same recipe concurrently, cache access needs a per-recipe mutex.

- [ ] **Step 1: Write failing test**

```rust
#[test]
fn test_thread_safe_cache_write() {
    let dir = tempfile::tempdir().unwrap();
    let cache_dir = dir.path().join(".cook/cache");
    let manager = ThreadSafeCacheManager::new(cache_dir.clone());

    manager.update_step("build", "step1", StepEntry {
        inputs: vec![],
        output: None,
        command_hash: 123,
    });
    manager.flush_all().unwrap();

    let loaded = RecipeCache::load(&cache_dir, "build").unwrap();
    assert!(loaded.steps.contains_key("step1"));
}
```

- [ ] **Step 2: Implement ThreadSafeCacheManager**

```rust
// In src/cache/mod.rs
use std::sync::{Arc, Mutex};
use std::collections::HashMap;

pub struct ThreadSafeCacheManager {
    caches: Mutex<HashMap<String, RecipeCache>>,
    cache_dir: PathBuf,
    dirty: Mutex<std::collections::HashSet<String>>,
}

impl ThreadSafeCacheManager {
    pub fn new(cache_dir: PathBuf) -> Self {
        Self {
            caches: Mutex::new(HashMap::new()),
            cache_dir,
            dirty: Mutex::new(std::collections::HashSet::new()),
        }
    }

    pub fn load_recipe(&self, recipe_name: &str) {
        let cache = RecipeCache::load(&self.cache_dir, recipe_name).unwrap_or_default();
        self.caches.lock().unwrap().insert(recipe_name.to_string(), cache);
    }

    pub fn update_step(&self, recipe_name: &str, cache_key: &str, entry: store::StepEntry) {
        let mut caches = self.caches.lock().unwrap();
        let cache = caches.entry(recipe_name.to_string()).or_insert_with(RecipeCache::new);
        cache.steps.insert(cache_key.to_string(), entry);
        self.dirty.lock().unwrap().insert(recipe_name.to_string());
    }

    pub fn flush_all(&self) -> std::io::Result<()> {
        let caches = self.caches.lock().unwrap();
        let dirty = self.dirty.lock().unwrap();
        for name in dirty.iter() {
            if let Some(cache) = caches.get(name) {
                cache.save(&self.cache_dir, name)?;
            }
        }
        Ok(())
    }
}
```

- [ ] **Step 3: Run tests**

Run: `cargo test --lib cache`
Expected: PASS

- [ ] **Step 4: Commit**

```bash
git add src/cache/mod.rs
git commit -m "feat(cache): add thread-safe cache manager with per-recipe locking"
```

### Task 9: CLI Integration

**Files:**
- Modify: `src/cli/mod.rs` — add `-j` flag, replace sequential loop

This is where everything comes together.

- [ ] **Step 1: Add -j/--jobs flag to CLI**

```rust
// In Cli struct
/// Number of parallel jobs (default: number of CPU cores)
#[arg(short = 'j', long = "jobs", global = true)]
pub jobs: Option<usize>,
```

- [ ] **Step 2: Write the new cmd_run using the scheduler**

Replace the sequential recipe loop in `cmd_run` with:

```rust
fn cmd_run(cli: &Cli, recipe_name: &str) -> Result<(), CookError> {
    let (cookfile, lua_source) = read_and_parse(cli)?;

    if cli.emit_lua {
        println!("{lua_source}");
        return Ok(());
    }

    let cookfile_dir = cli.file.parent().unwrap_or(std::path::Path::new("."));
    let cookfile_dir = if cookfile_dir.as_os_str().is_empty() {
        std::path::Path::new(".")
    } else {
        cookfile_dir
    };
    let env_vars = load_env(cookfile_dir);

    let order = analyzer::resolve_execution_order(&cookfile, recipe_name)
        .map_err(|e| match e {
            analyzer::graph::GraphError::UnknownRecipe(name) => CookError::RecipeNotFound(name),
            analyzer::graph::GraphError::CycleDetected(name) => {
                CookError::Other(format!("dependency cycle involving: {name}"))
            }
        })?;

    let mut rt = Runtime::new(cookfile_dir.to_path_buf(), env_vars.clone());
    rt.set_no_taste(cli.no_taste);
    rt.set_quiet(cli.quiet);

    let num_jobs = cli.jobs.unwrap_or_else(||
        std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(1)
    );

    // Phase 1: Register all recipes (main thread, sequential)
    let mut all_recipe_units = Vec::new();
    for name in &order {
        if !cli.quiet {
            eprintln!("cook: registering recipe '{name}'");
        }
        let units = rt.register_recipe(&lua_source, name)
            .map_err(|e| match e {
                crate::runtime::RuntimeError::RecipeNotFound(name) => CookError::RecipeNotFound(name),
                crate::runtime::RuntimeError::Lua(e) => CookError::Other(format!("lua error: {e}")),
                crate::runtime::RuntimeError::CommandFailed { command, line, code } => {
                    CookError::CommandFailed(format!("Cookfile:{line}: command failed (exit {code}): {command}"))
                }
            })?;
        all_recipe_units.push(units);
    }

    // Phase 2: Build DAG
    let dag = crate::scheduler::builder::build_dag(all_recipe_units);

    if dag.is_empty() {
        return Ok(());
    }

    // Phase 3: Execute DAG
    crate::scheduler::execute_dag(dag, num_jobs, cookfile_dir, &env_vars, cli.quiet)
        .map_err(|e| {
            // Parse first failure for error reporting
            if let Some((_, msg)) = e.failures.first() {
                if msg.contains("COOK_CMD_FAILED:") {
                    let parts: Vec<&str> = msg
                        .split("COOK_CMD_FAILED:")
                        .nth(1)
                        .unwrap_or("0:1:unknown")
                        .splitn(3, ':')
                        .collect();
                    let line = parts.first().and_then(|s| s.parse().ok()).unwrap_or(0);
                    let code = parts.get(1).and_then(|s| s.parse().ok()).unwrap_or(1);
                    let command = parts.get(2).unwrap_or(&"unknown").to_string();
                    if line == 0 {
                        CookError::CommandFailed(format!("command failed (exit {code}): {command}"))
                    } else {
                        CookError::CommandFailed(format!("Cookfile:{line}: command failed (exit {code}): {command}"))
                    }
                } else {
                    CookError::Other(msg.clone())
                }
            } else {
                CookError::Other("unknown scheduler error".into())
            }
        })?;

    // Phase 4: Flush caches
    // (handled by scheduler or separately)

    Ok(())
}
```

- [ ] **Step 3: Run the full test suite**

Run: `cargo test`
Expected: ALL tests pass — both unit tests and integration tests.

This is the critical validation step. Every existing integration test should continue to work with the new scheduler-based execution. If any fail, debug and fix.

- [ ] **Step 4: Run the stress test specifically**

Run: `cargo test test_cook_stress_all_features -- --nocapture`
Expected: PASS — this exercises every Cook feature

- [ ] **Step 5: Write new integration test for -j flag**

```rust
#[test]
fn test_cook_parallel_flag() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(
        dir.path().join("Cookfile"),
        r#"recipe "build"
    echo "parallel!"
end"#,
    ).unwrap();
    let output = cook_cmd()
        .current_dir(dir.path())
        .args(["-j", "2", "build"])
        .output()
        .unwrap();
    assert!(output.status.success(), "parallel flag failed: {}", String::from_utf8_lossy(&output.stderr));
}

#[test]
fn test_cook_parallel_independent_recipes() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(
        dir.path().join("Cookfile"),
        r#"recipe "a"
    echo "recipe a"
end

recipe "b"
    echo "recipe b"
end

recipe "all": "a" "b"
    echo "done"
end"#,
    ).unwrap();
    let output = cook_cmd()
        .current_dir(dir.path())
        .args(["-j", "4", "all"])
        .output()
        .unwrap();
    assert!(output.status.success(), "parallel recipes failed: {}", String::from_utf8_lossy(&output.stderr));
}
```

- [ ] **Step 6: Run all tests**

Run: `cargo test`
Expected: ALL PASS

- [ ] **Step 7: Commit**

```bash
git add src/cli/mod.rs
git commit -m "feat(cli): add -j flag, wire scheduler-based parallel execution"
```

- [ ] **Step 8: Final integration validation**

Run: `cargo build && cargo test`
Expected: Clean build, all tests pass.

Run manually with the examples:
```bash
cd examples && cargo run --manifest-path ../Cargo.toml -- -j4 build
```

- [ ] **Step 9: Final commit with any remaining fixes**

```bash
git add -A
git commit -m "fix: address remaining issues from parallel execution integration"
```

---

## Known Limitations (Document in README)

1. **Lua blocks depending on cook.sh() return values:** In parallel mode, cook.sh() inside worker Lua blocks returns `()` instead of stdout. Lua blocks that branch on command output won't work correctly.

2. **Lua blocks reading intermediate files during registration:** During the DAG registration phase, intermediate build products don't exist yet. Lua blocks that call `fs.read()` on files produced by previous cook steps will fail during registration. Use shell-based cook steps for these cases.

3. **Taste breakpoints:** Skipped in parallel mode. Will be redesigned separately.

4. **Output ordering:** With `-j > 1`, output from concurrent recipes interleaves (prefixed with `[recipe_name]`). Use `-j1` for sequential output.
