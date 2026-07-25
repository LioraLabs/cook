use std::collections::BTreeMap;
use std::collections::HashMap;
use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use cook_contracts::{OutputStream, StepKind, WorkPayload};
use crate::store::ProbeValueStore;

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// Read-only snapshot of the register session's terminal-outputs map
/// (recipe qualified-name → terminal output paths), shared across all worker
/// VMs so execute-phase `cook.dep_output` / `cook.dep_output_list` (§24.7)
/// resolve without re-entering the register session. `Arc` because every
/// worker thread's VM captures the same immutable map.
pub type WorkerDepOutputs = Arc<BTreeMap<String, Vec<String>>>;

pub struct WorkItem {
    pub id: usize,
    pub payload: WorkPayload,
    pub recipe_name: String,
    pub working_dir: PathBuf,
    /// Full env lookup map: seeds `cook.env` and backs `$<NAME>` /
    /// consulted-value resolution. NOT the child-process environment — a
    /// config `var.*` value here is `$<NAME>`-only (R1 / CS-0164).
    pub env_vars: HashMap<String, String>,
    /// The subset of `env_vars` actually placed in a spawned step's process
    /// environment: per-unit exports only (chore parameters). Config `var.*`
    /// values are excluded so a shell `$NAME` read sees them unset (R1).
    pub process_env_vars: HashMap<String, String>,
    /// Project root for the CS-0045 sandbox. The worker installs the
    /// per-item sandbox policy by combining this root with the
    /// payload's `step_kind` (Cook/Test/Chore → Confined; there is no
    /// unsandboxed step kind — CS-0135 retired `plate`, the prior
    /// exception). One worker VM may serve items from multiple projects in
    /// the cross-Cookfile-import case (CS-0017), so the root must
    /// travel with the item rather than being captured at pool spawn.
    pub project_root: PathBuf,
}

#[derive(Clone, Debug)]
pub struct TestOutput {
    pub suite_name: String,
    pub test_name: String,
    pub stdout: String,
    pub stderr: String,
    pub duration: f64,
    pub timed_out: bool,
    pub should_fail: bool,
    pub exit_success: bool,
    pub exit_code: Option<i32>,
}

/// Payload returned by a completed probe unit (§22.5). `bytes` contains the
/// canonical-JSON-serialised return value of the `produce` Lua function
/// (§22.5.5, CS-0102).
/// Handled by Task G; present as `None` on all non-probe WorkResults.
#[derive(Clone, Debug)]
pub struct ProbeOutput {
    pub key: String,
    pub bytes: Vec<u8>,
}

pub struct WorkResult {
    pub id: usize,
    pub success: bool,
    pub error: Option<String>,
    pub test_output: Option<TestOutput>,
    pub node_name: String,
    /// Captured child output, in emission order.  Each entry is paired with
    /// the file descriptor it came from so downstream observers can preserve
    /// stdout/stderr provenance (CS-0035).  Pre-CS-0035 this was `Vec<String>`
    /// and the engine attributed every line to stdout in the JSON event stream.
    pub output_lines: Vec<(OutputStream, String)>,
    /// Set when this result comes from a `WorkPayload::Probe` unit (§22.5).
    /// `None` for all non-probe units. Wired end-to-end by Task G.
    pub probe_output: Option<ProbeOutput>,
    /// Wall-clock span of the actual work-item execution, measured by the
    /// worker around the `execute_work_item` dispatch (queue wait
    /// excluded). Mirrors the existing `TestOutput.duration` measurement
    /// approach, generalised to every payload kind so a plain (non-test)
    /// unit's completion line can report real elapsed time instead of a
    /// hardcoded zero. Individual `execute_*` helpers set this to
    /// `Duration::ZERO` in their returned literals; `worker_loop`
    /// overwrites it with the measured span for every outcome (success,
    /// failure, and the worker-panic recovery path) before sending the
    /// result, so the placeholder value never reaches the engine.
    pub duration: Duration,
}

pub struct WorkerPool {
    threads: Vec<std::thread::JoinHandle<()>>,
    queue: Arc<SharedQueue>,
    /// Per-run probe-value store. Owned here so `probe_value_store()` returns
    /// a clone that the engine scheduler can write probe outputs into after
    /// workers complete their `WorkPayload::Probe` units (§22.5.7).
    probe_store: ProbeValueStore,
}

// ---------------------------------------------------------------------------
// Internal types
// ---------------------------------------------------------------------------

enum QueueItem {
    Work(WorkItem),
    Shutdown,
}

struct SharedQueue {
    queue: Mutex<VecDeque<QueueItem>>,
    condvar: Condvar,
}

// ---------------------------------------------------------------------------
// WorkerPool implementation
// ---------------------------------------------------------------------------

impl WorkerPool {
    /// Spawn `n` worker threads with no dep-output snapshot (empty map).
    /// Convenience wrapper preserved for the crate's unit tests, which never
    /// exercise `cook.dep_output`.
    pub fn spawn(n: usize) -> (Self, mpsc::Receiver<WorkResult>) {
        Self::spawn_with_dep_outputs(n, Arc::new(BTreeMap::new()))
    }

    /// Spawn `n` worker threads, threading a read-only terminal-outputs
    /// snapshot into every worker VM so execute-phase `cook.dep_output` /
    /// `cook.dep_output_list` (§24.7) resolve. Each thread creates its own
    /// `mlua::Lua` VM and pulls work items from the shared queue.  Results
    /// are sent back through the returned `mpsc::Receiver`.
    pub fn spawn_with_dep_outputs(
        n: usize,
        dep_outputs: WorkerDepOutputs,
    ) -> (Self, mpsc::Receiver<WorkResult>) {
        let shared = Arc::new(SharedQueue {
            queue: Mutex::new(VecDeque::new()),
            condvar: Condvar::new(),
        });

        // Per-run probe-value store: shared across all workers so that
        // `cook.probes.get` on any worker VM sees the same store (§22.5.7).
        let probe_store = ProbeValueStore::new();

        let (tx, rx) = mpsc::channel();

        let mut threads = Vec::with_capacity(n);

        for _ in 0..n {
            let q = Arc::clone(&shared);
            let tx = tx.clone();
            let store = probe_store.clone();
            let deps = Arc::clone(&dep_outputs);

            let handle = std::thread::spawn(move || {
                worker_loop(q, tx, store, deps);
            });
            threads.push(handle);
        }

        (WorkerPool { threads, queue: shared, probe_store }, rx)
    }

    /// Return a clone of the `ProbeValueStore` so the engine scheduler
    /// can write probe outputs into it after each `WorkPayload::Probe` unit
    /// completes (§22.5.7).
    pub fn probe_value_store(&self) -> ProbeValueStore {
        self.probe_store.clone()
    }

    /// Push a work item into the shared queue.
    pub fn submit(&self, item: WorkItem) {
        let mut q = self.queue.queue.lock().expect("queue lock poisoned");
        q.push_back(QueueItem::Work(item));
        self.queue.condvar.notify_one();
    }

    /// Send a shutdown sentinel for every worker and join all threads.
    pub fn shutdown(mut self) {
        self.signal_and_join();
    }

    /// Idempotent shutdown used by both explicit `shutdown()` and `Drop`.
    /// Recovers a poisoned queue mutex so a panicking worker can't strand
    /// the rest of the pool.
    fn signal_and_join(&mut self) {
        if self.threads.is_empty() {
            return;
        }
        {
            let mut q = match self.queue.queue.lock() {
                Ok(g) => g,
                Err(poisoned) => poisoned.into_inner(),
            };
            for _ in &self.threads {
                q.push_back(QueueItem::Shutdown);
            }
            self.queue.condvar.notify_all();
        }
        for handle in self.threads.drain(..) {
            let _ = handle.join();
        }
    }
}

impl Drop for WorkerPool {
    /// Implicit shutdown: if a `WorkerPool` is dropped without an explicit
    /// `shutdown()` call, signal the workers and join them. Without this,
    /// the workers' `Arc<SharedQueue>` clones keep the queue alive forever
    /// and the threads leak, blocked on the condvar.
    fn drop(&mut self) {
        self.signal_and_join();
    }
}

// ---------------------------------------------------------------------------
// Worker loop (runs on each thread)
// ---------------------------------------------------------------------------

fn worker_loop(
    queue: Arc<SharedQueue>,
    tx: mpsc::Sender<WorkResult>,
    probe_store: ProbeValueStore,
    dep_outputs: WorkerDepOutputs,
) {
    // Each worker creates its own Lua VM.  The VM is `!Send` but never
    // leaves this thread, so this is safe.
    let lua = unsafe { mlua::Lua::unsafe_new() };

    // `path.*` is pure string manipulation — install once.
    cook_lua_stdlib::register_path_api(&lua).expect("failed to register path API");

    // Shared mutable state for per-item context (single-threaded within
    // this worker, but needs interior mutability for closures).
    let current_recipe: Arc<Mutex<String>> = Arc::new(Mutex::new(String::new()));
    let current_working_dir: Arc<Mutex<PathBuf>> = Arc::new(Mutex::new(PathBuf::new()));
    let current_env_vars: Arc<Mutex<HashMap<String, String>>> = Arc::new(Mutex::new(HashMap::new()));
    // R1 (CS-0164): the child-process env subset (chore-param exports). Kept
    // separate from `current_env_vars` (the full `cook.env` lookup map) so a
    // config `var.*` value never reaches a spawned step's environment.
    let current_process_env_vars: Arc<Mutex<HashMap<String, String>>> =
        Arc::new(Mutex::new(HashMap::new()));

    // CS-0045 sandbox slot. Updated per work item before the body
    // runs: Cook/Test/Chore → Confined { project_root }. There is no
    // unsandboxed step kind (CS-0135 retired `plate`, the prior
    // exception). Default is `Off` — the slot is overwritten before
    // the first body executes, but if a future code path somehow
    // runs Lua before the first slot update, `Off` is the safe
    // fallback (no false positives on legitimate I/O).
    let current_sandbox: Arc<Mutex<cook_lua_stdlib::SandboxPolicy>> =
        Arc::new(Mutex::new(cook_lua_stdlib::SandboxPolicy::Off));

    // Register the `cook` table once with closures that capture shared state.
    register_worker_cook_table(&lua, &current_working_dir, &current_env_vars, &current_process_env_vars, &current_recipe, &probe_store, &dep_outputs)
        .expect("failed to register cook table");

    // Register the `fs` table once at startup with the Live cwd source
    // so each call sees the *current* work item's working_dir, not the
    // one in effect at registration time. This is the CS-0017
    // multi-Cookfile imports contract: one worker VM may serve items
    // from many Cookfiles (cwds), and `fs.*` resolves against the
    // active item's cwd at call time.
    //
    // CS-0045: pair the live cwd source with a live sandbox source so
    // each call also sees the active item's policy (cook = confined,
    // plate = off).
    cook_lua_stdlib::register_fs_api_with_sandbox(
        &lua,
        cook_lua_stdlib::WorkingDirSource::Live(Arc::clone(&current_working_dir)),
        cook_lua_stdlib::SandboxSource::Live(Arc::clone(&current_sandbox)),
    )
    .expect("failed to register fs API");

    // CS-0045: install Lua-side shell escape-hatch guards on
    // `os.execute` and `io.popen`. Same Live source so the per-item
    // policy applies.
    cook_lua_stdlib::install_shell_escape_guards(
        &lua,
        cook_lua_stdlib::SandboxSource::Live(Arc::clone(&current_sandbox)),
    )
    .expect("failed to install shell escape guards");

    loop {
        let item = {
            let mut q = match queue.queue.lock() {
                Ok(g) => g,
                Err(p) => p.into_inner(),
            };
            loop {
                if let Some(front) = q.pop_front() {
                    break front;
                }
                q = match queue.condvar.wait(q) {
                    Ok(g) => g,
                    Err(p) => p.into_inner(),
                };
            }
        };

        match item {
            QueueItem::Shutdown => break,
            QueueItem::Work(work) => {
                // Update per-item context before executing
                {
                    let mut name = current_recipe.lock().expect("recipe name lock");
                    *name = work.recipe_name.clone();
                }
                {
                    let mut wd = current_working_dir.lock().expect("working_dir lock");
                    *wd = work.working_dir.clone();
                }
                {
                    let mut env = current_env_vars.lock().expect("env_vars lock");
                    *env = work.env_vars.clone();
                }
                {
                    let mut penv =
                        current_process_env_vars.lock().expect("process_env_vars lock");
                    *penv = work.process_env_vars.clone();
                }
                // CS-0045: pick the per-item sandbox policy. Cook, Test,
                // Chore, and any non-LuaChunk payload all run confined to
                // `project_root` — there is no unsandboxed step kind
                // (CS-0135 retired `plate`, the prior exception). For
                // Shell/Test/Interactive payloads the policy is
                // irrelevant — the worker doesn't run user Lua for
                // those — but setting it consistently means a stray
                // `lua.load()` in a future code path can't accidentally
                // land Off.
                {
                    let kind = match &work.payload {
                        WorkPayload::LuaChunk { step_kind, .. } => *step_kind,
                        _ => StepKind::Cook,
                    };
                    let policy = match kind {
                        StepKind::Cook | StepKind::Test | StepKind::Chore => {
                            cook_lua_stdlib::SandboxPolicy::Confined {
                                project_root: work.project_root.clone(),
                            }
                        }
                        // CS-0049: `StepKind` is `#[non_exhaustive]`. Future
                        // variants default to the strictest policy (Confined)
                        // until a CS classifies them explicitly.
                        _ => cook_lua_stdlib::SandboxPolicy::Confined {
                            project_root: work.project_root.clone(),
                        },
                    };
                    let mut sb = current_sandbox.lock().expect("sandbox slot lock");
                    *sb = policy;
                }

                // Refresh package.path and package.cpath so `require` resolves
                // cook_modules/ relative to this unit's source Cookfile (CS-0062).
                let _ = refresh_package_search_paths(&lua, &work.working_dir);

                // Run the work item under `catch_unwind`. A Rust panic
                // anywhere in execute_work_item (e.g. an unexpected
                // upstream invariant violation) is converted into a
                // failure `WorkResult` so the engine never hangs on
                // `rx.recv()`. The Lua VM is reused — mlua wraps panics
                // raised from inside Lua callbacks and converts them to
                // Lua errors, so the VM state stays sane.
                let work_id = work.id;
                let recipe_name = work.recipe_name.clone();
                let node_name = work.payload.display_name();
                // Measured span = actual execution only. The queue wait
                // already ended when this item was popped above, and the
                // per-item context setup just above (recipe/cwd/env/sandbox,
                // package-path refresh) is worker bookkeeping, not queued
                // idle time, so starting the clock here — immediately
                // around the dispatch — is the honest per-unit number
                // (same intent as `TestOutput.duration`'s `start.elapsed()`).
                let exec_start = Instant::now();
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    // R1: shell/test steps spawn with the process-env subset,
                    // NOT the full config lookup map. `cook.env` reads (Lua
                    // bodies) still see the full map via `current_env_vars`.
                    execute_work_item(&lua, &work, &work.working_dir, &work.process_env_vars)
                }));
                let mut result = match result {
                    Ok(r) => r,
                    Err(panic_payload) => {
                        let msg = panic_payload_to_string(&panic_payload);
                        WorkResult {
                            id: work_id,
                            success: false,
                            error: Some(format!(
                                "[{recipe_name}] worker panic: {msg}"
                            )),
                            test_output: None,
                            node_name,
                            output_lines: Vec::new(),
                            probe_output: None,
                            duration: Duration::ZERO,
                        }
                    }
                };
                result.duration = exec_start.elapsed();
                let _ = tx.send(result);
            }
        }
    }
}

/// Best-effort extraction of a panic payload's message. Panics raised via
/// `panic!("…")` carry either a `&'static str` or `String`; anything else
/// gets a generic placeholder.
fn panic_payload_to_string(payload: &Box<dyn std::any::Any + Send>) -> String {
    if let Some(s) = payload.downcast_ref::<&'static str>() {
        s.to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "unknown panic payload".to_string()
    }
}

// ---------------------------------------------------------------------------
// Per-worker cook table registration
// ---------------------------------------------------------------------------

fn register_worker_cook_table(
    lua: &mlua::Lua,
    current_working_dir: &Arc<Mutex<PathBuf>>,
    current_env_vars: &Arc<Mutex<HashMap<String, String>>>,
    current_process_env_vars: &Arc<Mutex<HashMap<String, String>>>,
    current_recipe: &Arc<Mutex<String>>,
    probe_store: &ProbeValueStore,
    dep_outputs: &WorkerDepOutputs,
) -> mlua::Result<()> {
    let cook = lua.create_table()?;

    // cook.sh(cmd) -> stdout string. R1 (CS-0164): a `cook.sh` child spawns
    // with the process-env subset (chore-param exports), not the full config
    // lookup map, so a config `var.*` value is never in its environment.
    let wd = Arc::clone(current_working_dir);
    let penv = Arc::clone(current_process_env_vars);
    let recipe = Arc::clone(current_recipe);
    let sh_fn = lua.create_function(move |_, cmd: String| {
        let recipe_name = recipe.lock().expect("recipe name lock").clone();
        let working_dir = wd.lock().expect("working_dir lock").clone();
        let env_vars = penv.lock().expect("process_env_vars lock").clone();
        run_shell_in_worker(&cmd, &working_dir, &env_vars, &recipe_name, 0)
    })?;
    cook.set("sh", sh_fn)?;

    // Note: `cook.exec`, `cook.interactive`, `cook.add_unit`,
    // `cook.step_group`, and `cook.recipe` are register-only API
    // (Standard §6.3.2). On the worker (execute-phase) VM they are
    // installed as error-raising guards near the bottom of this
    // function — see `install_register_only_guard`.

    // CS-0172: the read-only `var` global — the execute-phase half of the
    // declared-variable surface (§5.3.1). A `__index` metamethod so reads
    // always reflect the current unit's resolved variables. Values arrive in
    // string form: the register phase rejects a non-string variable read from
    // an execute-phase Lua body precisely so this surface never has to coerce
    // one. Writes are refused, as at register phase — a step cannot redefine a
    // determinant it was keyed on.
    let var_table = lua.create_table()?;
    let vars_for_index = Arc::clone(current_env_vars);
    let meta = lua.create_table()?;
    meta.set("__index", lua.create_function(move |_, (_tbl, key): (mlua::Value, String)| {
        let vars = vars_for_index.lock().expect("var store lock");
        Ok(vars.get(&key).cloned())
    })?)?;
    meta.set("__newindex", lua.create_function(|_, (_tbl, key, _v): (mlua::Value, String, mlua::Value)| -> mlua::Result<()> {
        Err(mlua::Error::RuntimeError(format!(
            "var.{key} is read-only: a declared variable is a cache determinant \
             of the unit reading it (Standard §5.3.1)"
        )))
    })?)?;
    meta.set("__metatable", false)?;
    var_table.set_metatable(Some(meta));
    lua.globals().set("var", var_table)?;

    // cook.platform — installed via the shared cook-lua-stdlib so the
    // execute-phase string values are byte-identical to the
    // register-phase ones (CS-0044).
    cook_lua_stdlib::register_platform_api(lua, &cook)?;

    // CS-0123: cook.json_decode / cook.yaml_decode are both-phase (§24.8).
    // Same shared implementation as the register VM so a probe produce
    // body behaves identically on the pre-pass and demand-driven paths.
    cook_lua_stdlib::register_codec_api(lua, &cook)?;

    // CS-0158: cook.tools.id — canonical tool identity (content hash +
    // fresh path). Both-phase; shared implementation, same rationale.
    cook_lua_stdlib::register_tools_api(lua, &cook)?;

    // cook.load_module(name) — execute-phase counterpart of the register-phase
    // resolver in cook-register/src/module_loader.rs (CS-0017, CS-0035,
    // §{lua.cook-load-module}). Lookup uses the unit's current_working_dir,
    // so an imported Cookfile's body unit resolves against its own
    // cook_modules/ directory (lexical per Cookfile, §{modules.use-scope}).
    //
    // Caches loaded modules in `_cook_module_cache` (a per-VM table keyed
    // by `<cwd>::<name>`) so repeated calls within one body unit don't
    // re-read the file. Module top-level and `init()` run once per
    // (cwd, name, worker VM).
    //
    // CS-0035: cycle detection. Tracks an in-flight set in
    // `_cook_module_loading` keyed the same way as the cache. If a
    // re-entrant `cook.load_module(name)` would try to evaluate a module
    // already in flight, we raise a diagnostic naming the cycle path so
    // module authors can locate the offending edge.
    let wd_load = Arc::clone(current_working_dir);
    lua.globals().set("_cook_module_cache", lua.create_table()?)?;
    lua.globals().set("_cook_module_loading", lua.create_table()?)?;
    lua.globals().set("_cook_module_loading_stack", lua.create_table()?)?;
    let load_module_fn = lua.create_function(move |lua, name: String| {
        let cwd = wd_load.lock().expect("working_dir lock").clone();
        let cache_key = format!("{}::{}", cwd.display(), name);

        // Memoization (§6.3.4): a second cook.load_module(name) returns the
        // cached value without re-reading or re-evaluating the module file.
        let cache: mlua::Table = lua.globals().get("_cook_module_cache")?;
        if let Ok(cached) = cache.get::<mlua::Value>(cache_key.clone()) {
            if !matches!(cached, mlua::Value::Nil) {
                return Ok(cached);
            }
        }

        // Cycle detection (CS-0035): if `name` (under this cwd) is already in
        // flight, raise a diagnostic that lists the cycle path.
        let loading: mlua::Table = lua.globals().get("_cook_module_loading")?;
        let stack: mlua::Table = lua.globals().get("_cook_module_loading_stack")?;
        if let Ok(in_flight) = loading.get::<bool>(cache_key.clone()) {
            if in_flight {
                let mut path: Vec<String> = Vec::new();
                let len = stack.raw_len();
                for i in 1..=len {
                    if let Ok(s) = stack.get::<String>(i) {
                        path.push(s);
                    }
                }
                path.push(name.clone());
                return Err(mlua::Error::runtime(format!(
                    "module cycle detected: {}",
                    path.join(" -> ")
                )));
            }
        }

        // Resolve in §7's 4-path order [CS-0069]: hand-vendored wins over
        // LuaRocks-installed. Mirrors cook-register/src/module_loader.rs.
        let modules_dir = cwd.join("cook_modules");
        let share_dir = modules_dir.join("share/lua/5.4");
        let candidates = [
            modules_dir.join(format!("{}.lua", name)),
            modules_dir.join(&name).join("init.lua"),
            share_dir.join(format!("{}.lua", name)),
            share_dir.join(&name).join("init.lua"),
        ];
        let module_path = match candidates.iter().find(|p| p.exists()) {
            Some(p) => p.clone(),
            None => {
                return Err(mlua::Error::runtime(format!(
                    "cook.load_module: module '{}' not found in {}/cook_modules/ \
                     (tried {}.lua, {}/init.lua, share/lua/5.4/{}.lua, \
                     share/lua/5.4/{}/init.lua)",
                    name, cwd.display(), name, name, name, name,
                )));
            }
        };

        let source = std::fs::read_to_string(&module_path).map_err(|e| {
            mlua::Error::runtime(format!(
                "cook.load_module: failed to read {}: {}",
                module_path.display(),
                e
            ))
        })?;

        // Mark this (cwd, name) as in-flight before eval so a re-entrant call
        // can detect the cycle. Cleanup on every exit path keeps detection
        // sane after recoverable errors.
        loading.set(cache_key.clone(), true)?;
        let stack_idx = stack.raw_len() + 1;
        stack.set(stack_idx, name.clone())?;

        let chunk_name = format!("@{}", module_path.display());
        let result: mlua::Value = match lua.load(&source).set_name(&chunk_name).eval() {
            Ok(v) => v,
            Err(e) => {
                let _ = loading.set(cache_key, mlua::Value::Nil);
                let _ = stack.set(stack_idx, mlua::Value::Nil);
                return Err(e);
            }
        };

        // Run init() if the returned table has one (§7.5).
        if let mlua::Value::Table(ref tbl) = result {
            if let Ok(mlua::Value::Function(init_fn)) = tbl.get::<mlua::Value>("init") {
                if let Err(e) = init_fn.call::<()>(()) {
                    let _ = loading.set(cache_key, mlua::Value::Nil);
                    let _ = stack.set(stack_idx, mlua::Value::Nil);
                    return Err(e);
                }
            }
        }

        loading.set(cache_key.clone(), mlua::Value::Nil)?;
        stack.set(stack_idx, mlua::Value::Nil)?;
        cache.set(cache_key, result.clone())?;
        Ok(result)
    })?;
    cook.set("load_module", load_module_fn)?;

    // CS-0070 / CS-0074: cook.probes on the execute-phase VM (Standard §6.3.4).
    //
    // `cook.probes.get(key)` reads from the per-run SharedProbeValueStore so
    // that consumer units see probe values produced by upstream probe units
    // (§22.5.7). `cook.probes.set` is deprecated and raises an error on the
    // execute-phase VM (CS-0074).
    //
    // `cook.probes.scope(label)` is still supported for backwards compat with
    // modules that use the scoped sub-table pattern.
    install_execute_phase_cook_probes(lua, &cook, probe_store)?;

    // COOK-64 §8.3: cook.member_to_string(value) renders a for_each data
    // member to its canonical string form (key-sorted JSON for a table, the
    // scalar's bare string otherwise). Used by the `$<in>` placeholder.
    let member_fn = lua.create_function(|_, value: mlua::Value| {
        let jv = crate::probe_value::lua_to_json(&value)
            .map_err(|e| mlua::Error::runtime(format!("cook.member_to_string: {e}")))?;
        Ok(cook_contracts::member::member_to_string(&jv))
    })?;
    cook.set("member_to_string", member_fn)?;

    // CS-0071: cook.export / cook.import on the execute-phase VM
    // (Standard §6.3.4). Per-worker in-memory store; no cross-invocation
    // persistence. The register-phase implementation
    // (cook-register/src/export_api.rs `register_export_api`) backs a
    // shared store the engine consumes for transitive-link recording.
    // The execute-phase side only needs to satisfy the both-phase API
    // surface so that target makers like cook_cc's `cc.bin` (whose
    // recipe-body Lua calls `cook.export(name, {...})` to publish
    // transitive info) do not raise `attempt to call a nil value
    // (field 'export')` when their body runs on the worker VM.
    //
    // Cross-worker visibility is intentionally out of scope: each
    // recipe is a self-contained producer/consumer pair within one
    // worker, so a per-worker scratch table satisfies CS-0071.
    install_execute_phase_cook_export(lua, &cook)?;

    // cook.dep_output / cook.dep_output_list on the execute-phase VM
    // (Standard §24.7, "Both"). Read-only resolution against the register
    // session's terminal-outputs snapshot; no DAG-edge recording (the DAG is
    // closed before execute phase).
    install_worker_dep_output_api(lua, &cook, Arc::clone(dep_outputs), current_recipe)?;

    // Register-only API guards (Standard §6.3.2).
    //
    // `cook.exec`, `cook.interactive`, `cook.add_unit`, `cook.step_group`,
    // and `cook.recipe` are register-phase-only (§6.3.2, §6.3.3, §6.3.6,
    // and §B.4.12 rationale). A conforming implementation MUST raise a Lua
    // runtime error when any of them is called from execute-phase Lua (a
    // `lua_line`, a `lua_block`, or a `using >{ … }` payload).
    //
    // The worker VM is the execute-phase VM, so we install error-raising
    // stubs that supersede the partial-implementation `cook.exec` set
    // above (which silently aliased to a shell-out — non-conformant with
    // §6.3.2) and the entirely-absent `cook.interactive` / `cook.add_unit`
    // / `cook.step_group` / `cook.recipe` (which previously surfaced as
    // `attempt to call a nil value`, an incidentally-compliant but
    // shape-wrong diagnostic).
    //
    // The register-phase VM is built separately by cook-register
    // (`cook-register/src/{capture,unit_api}.rs`) — those call sites set
    // up the real recording implementations on a different VM, so this
    // guard does not affect them.
    install_register_only_guard(
        lua,
        &cook,
        "exec",
        "cook.exec: register-only API called from execute-phase Lua. \
         Use cook.sh(cmd) to shell out from a lua_line / lua_block / cook-body >{ … } payload. \
         Use `>>` instead of `>` to record this at register phase, or move the call to a \
         top-level `register` block.",
    )?;
    install_register_only_guard(
        lua,
        &cook,
        "interactive",
        "cook.interactive: register-only API called from execute-phase Lua. \
         Interactive steps must be recorded during the register phase; they cannot be \
         scheduled from a lua_line / lua_block / cook-body >{ … } payload. \
         Use `>>` instead of `>` to record this at register phase, or move the call to a \
         top-level `register` block.",
    )?;
    install_register_only_guard(
        lua,
        &cook,
        "add_unit",
        "cook.add_unit: register-only API called from execute-phase Lua. \
         Work units are recorded during the register phase; the DAG is closed before \
         execute-phase Lua runs. \
         Use `>>` instead of `>` to record this at register phase, or move the call to a \
         top-level `register` block.",
    )?;
    install_register_only_guard(
        lua,
        &cook,
        "step_group",
        "cook.step_group: register-only API called from execute-phase Lua. \
         Step groups are recorded during the register phase; they cannot be opened from a \
         lua_line / lua_block / cook-body >{ … } payload. \
         Use `>>` instead of `>` to record this at register phase, or move the call to a \
         top-level `register` block.",
    )?;
    install_register_only_guard(
        lua,
        &cook,
        "recipe",
        "cook.recipe: register-only API called from execute-phase Lua. \
         Recipes are registered during the register phase; they cannot be declared from a \
         lua_line / lua_block / cook-body >{ … } payload. \
         Use `>>` instead of `>` to record this at register phase, or move the call to a \
         top-level `register` block.",
    )?;
    install_register_only_guard(
        lua,
        &cook,
        "probe",
        "cook.probe: register-only API called from execute-phase Lua. \
         Probe units are declared during the register phase; they cannot be created from a \
         lua_line / lua_block / cook-body >{ … } payload. \
         Use `>>` instead of `>` to record this at register phase, or move the call to a \
         top-level `register` block.",
    )?;

    lua.globals().set("cook", cook)?;
    Ok(())
}

/// CS-0152: build the runtime error `cook.probes.get`/scoped `get` raise
/// when `key` (already the full, scope-prefixed key if applicable) has
/// never been materialised in the probe-value store. Shared by the
/// unscoped and scoped `get` implementations so the two diagnostics stay
/// in lockstep.
fn probe_not_materialised_error(key: &str) -> mlua::Error {
    mlua::Error::runtime(format!(
        "cook.probes.get(\"{key}\"): probe value not materialised — this step never \
         demanded probe '{key}'. Reference the probe in this step (a $<key> sigil in a \
         shell body, or probes = {{...}} on cook.add_unit), declare it in the probe's \
         `inputs.requires` (when reading from a probe produce body), or seal it \
         (seal {{ \"{key}\" }}) so it is scheduled before this step runs."
    ))
}

/// Install `cook.probes.{get,set,scope}` on the execute-phase VM
/// (Standard §6.3.4, CS-0070, CS-0074, CS-0152).
///
/// `cook.probes.get(key)` reads from the `SharedProbeValueStore` — the same
/// store the engine writes into when a probe unit completes (§22.5.8
/// `[#cat.probes.exec]`). A key that was never materialised (the step never
/// demanded the probe) is a hard error (CS-0152): silently returning `nil`
/// let real misses masquerade as legitimate probe-absent results. A key that
/// IS present whose canonical JSON payload is `null` still decodes to Lua
/// `nil` with no error — that boundary is load-bearing for probe produce
/// bodies (`cook.probes.get(KEY) or { ... }`).
///
/// CS-0157: merge the per-run tool-path metadata into a probe value's READ
/// VIEW. The canonical value of a `tools { }` producer carries identity only
/// (`{ NAME = { hash } }`); the resolved path is location, recorded fresh
/// each run by the engine (ProbeValueStore::set_tool_paths) so a Lua
/// consumer invoking `$<probe.NAME.path>` always gets where the tool
/// resolves NOW — never a stale path replayed from a cached value. The merge
/// is shape-scoped: only a table entry under the tool's own name that has a
/// `hash` field and no author-provided `path` is annotated, so custom-body
/// probes that happen to declare `inputs.tools` keep their values untouched.
fn merge_tool_paths(
    value: &mlua::Value,
    store: &ProbeValueStore,
    key: &str,
) -> mlua::Result<()> {
    let Some(paths) = store.tool_paths(key) else {
        return Ok(());
    };
    let mlua::Value::Table(t) = value else {
        return Ok(());
    };
    for (tool, path) in paths {
        if let Ok(mlua::Value::Table(entry)) = t.get::<mlua::Value>(tool.as_str()) {
            let has_hash =
                matches!(entry.get::<mlua::Value>("hash"), Ok(mlua::Value::String(_)));
            let path_absent =
                matches!(entry.get::<mlua::Value>("path"), Ok(mlua::Value::Nil));
            if has_hash && path_absent {
                entry.set("path", path)?;
            }
        }
    }
    Ok(())
}

/// `cook.probes.set` is deprecated on the execute-phase VM (CS-0074): calling
/// it raises a runtime error directing the author to use `cook.probe` instead.
///
/// `cook.probes.scope(label)` returns a sub-table whose `get` prefixes keys
/// with `"<label>:"` for backwards compat with modules that use scoped cache.
fn install_execute_phase_cook_probes(
    lua: &mlua::Lua,
    cook: &mlua::Table,
    probe_store: &ProbeValueStore,
) -> mlua::Result<()> {
    let cache_tbl = lua.create_table()?;

    // cook.probes.get(key) → value | hard error on unmaterialised key (CS-0152)
    let store_for_get = probe_store.clone();
    let get_fn = lua.create_function(move |lua, key: String| {
        match store_for_get.get(&key) {
            Some(bytes) => {
                let jv = cook_contracts::probe_value::decode_json(&bytes)
                    .map_err(|e| mlua::Error::runtime(format!(
                        "cook.probes.get('{}'): decode failed: {}", key, e
                    )))?;
                let v = crate::probe_value::json_to_lua(lua, &jv)?;
                merge_tool_paths(&v, &store_for_get, &key)?;
                Ok(v)
            }
            None => Err(probe_not_materialised_error(&key)),
        }
    })?;
    cache_tbl.set("get", get_fn)?;

    // cook.probes.set — deprecated and disabled on the execute-phase VM (CS-0074).
    let set_fn = lua.create_function(|_, (_key, _val): (String, mlua::Value)| -> mlua::Result<()> {
        Err(mlua::Error::runtime(
            "cook.probes.set: deprecated and not available on execute-phase VM (CS-0074). \
             Use cook.probe to declare memoised probe values."
        ))
    })?;
    cache_tbl.set("set", set_fn)?;

    // cook.probes.scope(label) → { get } — scoped get still works for
    // backwards compat; scoped set is also disabled.
    let store_for_scope = probe_store.clone();
    let scope_fn = lua.create_function(move |lua, label: String| {
        let scoped = lua.create_table()?;
        let prefix = format!("{}:", label);

        let store_for_scoped_get = store_for_scope.clone();
        let prefix_for_get = prefix.clone();
        let scoped_get = lua.create_function(move |lua, key: String| {
            let full = format!("{}{}", prefix_for_get, key);
            match store_for_scoped_get.get(&full) {
                Some(bytes) => {
                    let jv = cook_contracts::probe_value::decode_json(&bytes)
                        .map_err(|e| mlua::Error::runtime(format!(
                            "cook.probes.get('{}'): decode failed: {}", full, e
                        )))?;
                    let v = crate::probe_value::json_to_lua(lua, &jv)?;
                    merge_tool_paths(&v, &store_for_scoped_get, &full)?;
                    Ok(v)
                }
                None => Err(probe_not_materialised_error(&full)),
            }
        })?;
        scoped.set("get", scoped_get)?;

        let scoped_set = lua.create_function(|_, (_k, _v): (String, mlua::Value)| -> mlua::Result<()> {
            Err(mlua::Error::runtime(
                "cook.probes.set: deprecated and not available on execute-phase VM (CS-0074). \
                 Use cook.probe to declare memoised probe values."
            ))
        })?;
        scoped.set("set", scoped_set)?;

        Ok(scoped)
    })?;
    cache_tbl.set("scope", scope_fn)?;

    cook.set("probes", cache_tbl)?;

    // `cook.cache.*` renamed to `cook.probes.*` in v1.0 (CS-0136).
    let stub = lua.create_table()?;
    let mt = lua.create_table()?;
    let index_fn = lua.create_function(|_, (_t, key): (mlua::Table, String)| -> mlua::Result<mlua::Value> {
        Err(mlua::Error::runtime(format!(
            "'cook.cache' was renamed to 'cook.probes' in v1.0 (use cook.probes.{key})"
        )))
    })?;
    mt.set("__index", index_fn)?;
    stub.set_metatable(Some(mt));
    cook.set("cache", stub)?;
    Ok(())
}

/// Install `cook.export(name, info)` and `cook.import(name) -> table?`
/// on the execute-phase VM (Standard §6.3.4, CS-0071). Storage is an
/// in-memory Lua table held in the globals under `_cook_execute_exports`
/// — keyed by name (string), valued by arbitrary Lua values (typically
/// the info table that `cc.bin`/`cc.lib` publish). Per-worker; no
/// cross-run persistence and no cross-VM visibility.
///
/// The register-phase implementation lives in
/// `cook-register/src/export_api.rs` and persists into a serde-JSON
/// store the engine reads for transitive-link recording. The
/// execute-phase shape mirrors the contract from the module author's
/// POV — same signatures, same nil-on-miss semantics — without
/// inheriting the JSON round-trip; recipe bodies pass Lua tables
/// directly to each other within the worker.
fn install_execute_phase_cook_export(
    lua: &mlua::Lua,
    cook: &mlua::Table,
) -> mlua::Result<()> {
    let export_store = lua.create_table()?;
    lua.globals().set("_cook_execute_exports", export_store)?;

    let export_fn =
        lua.create_function(|lua, (name, info): (String, mlua::Value)| {
            let store: mlua::Table =
                lua.globals().get("_cook_execute_exports")?;
            store.set(name, info)?;
            Ok(())
        })?;
    cook.set("export", export_fn)?;

    let import_fn = lua.create_function(|lua, name: String| {
        let store: mlua::Table =
            lua.globals().get("_cook_execute_exports")?;
        store.get::<mlua::Value>(name)
    })?;
    cook.set("import", import_fn)?;

    Ok(())
}

/// Resolve a `cook.dep_output(name)` reference against the worker's
/// terminal-outputs snapshot (§24.7). `self_fqn` is the consumer recipe's
/// fully-qualified name; its Cookfile prefix (everything up to the last `.`)
/// qualifies a bare `name`. Looks up the single deterministic key
/// `<prefix>.<name>` (or bare `<name>` for a root consumer with empty prefix),
/// mirroring the register-phase `resolve_global_key`
/// (`cook-register/src/dep_output_api.rs`): a bare name resolves against the
/// consumer's *own* Cookfile and nowhere else. Returns `Some(paths)` on a hit
/// (possibly empty), `None` when the key is absent. A nested consumer's bare
/// ref does NOT fall back to a same-named root recipe — an absent local key is
/// an unknown referent, raising a Lua error rather than mis-resolving.
/// Cross-Cookfile `alias.recipe` refs are likewise not resolved here (that
/// needs the register session's alias-qualified-prefix map) — they miss and
/// raise a Lua error rather than mis-resolve.
fn resolve_worker_dep_output<'a>(
    dep_outputs: &'a BTreeMap<String, Vec<String>>,
    self_fqn: &str,
    name: &str,
) -> Option<&'a Vec<String>> {
    let self_prefix = self_fqn.rsplit_once('.').map(|(p, _)| p).unwrap_or("");
    if self_prefix.is_empty() {
        dep_outputs.get(name)
    } else {
        dep_outputs.get(&format!("{self_prefix}.{name}"))
    }
}

/// Install read-only `cook.dep_output` / `cook.dep_output_list` on the
/// execute-phase (worker) VM's `cook` table (Standard §24.7, "Both").
/// Read-only: unlike the register-phase implementation
/// (`cook-register/src/dep_output_api.rs`) it records no DAG edge — the DAG is
/// closed before execute phase. Error model (§24.7): unknown name → Lua error;
/// empty output list → empty string / empty table + a stderr warning.
fn install_worker_dep_output_api(
    lua: &mlua::Lua,
    cook: &mlua::Table,
    dep_outputs: WorkerDepOutputs,
    current_recipe: &Arc<Mutex<String>>,
) -> mlua::Result<()> {
    let deps = Arc::clone(&dep_outputs);
    let recipe = Arc::clone(current_recipe);
    let f = lua.create_function(move |_, name: String| {
        let fqn = recipe.lock().expect("recipe name lock").clone();
        match resolve_worker_dep_output(&deps, &fqn, &name) {
            Some(paths) if paths.is_empty() => {
                eprintln!(
                    "cook: warning: [{fqn}] cook.dep_output(\"{name}\"): referent has an empty output list"
                );
                Ok(String::new())
            }
            Some(paths) => Ok(paths.join(" ")),
            None => Err(mlua::Error::RuntimeError(format!(
                "recipe '{name}' has no terminal output (not registered or has no cook steps)"
            ))),
        }
    })?;
    cook.set("dep_output", f)?;

    let deps2 = Arc::clone(&dep_outputs);
    let recipe2 = Arc::clone(current_recipe);
    let g = lua.create_function(move |lua, name: String| {
        let fqn = recipe2.lock().expect("recipe name lock").clone();
        match resolve_worker_dep_output(&deps2, &fqn, &name) {
            Some(paths) => {
                if paths.is_empty() {
                    eprintln!(
                        "cook: warning: [{fqn}] cook.dep_output_list(\"{name}\"): referent has an empty output list"
                    );
                }
                let t = lua.create_table()?;
                for (i, p) in paths.iter().enumerate() {
                    t.set(i + 1, p.as_str())?;
                }
                Ok(t)
            }
            None => Err(mlua::Error::RuntimeError(format!(
                "recipe '{name}' has no terminal output (not registered or has no cook steps)"
            ))),
        }
    })?;
    cook.set("dep_output_list", g)?;
    Ok(())
}

/// Install a Lua function under `cook.<field>` that raises
/// `mlua::Error::RuntimeError(message)` when called. Used to surface
/// register-only Cook Lua API helpers as Standard §6.3.2 diagnostics on
/// the worker (execute-phase) VM.
fn install_register_only_guard(
    lua: &mlua::Lua,
    cook: &mlua::Table,
    field: &'static str,
    message: &'static str,
) -> mlua::Result<()> {
    let f = lua.create_function(move |_, _: mlua::MultiValue| -> mlua::Result<()> {
        Err(mlua::Error::RuntimeError(message.to_string()))
    })?;
    cook.set(field, f)?;
    Ok(())
}

/// Refresh `package.path` and `package.cpath` for the upcoming work unit so
/// `require("foo")` finds rocks under `<cwd>/cook_modules/`. Called per-unit
/// from the worker loop because `cwd` is per-Cookfile and each body unit may
/// come from a different one.
///
/// Search-path order (Standard §7):
///
///   package.path:
///     <cwd>/cook_modules/?.lua                          hand-vendored, single file
///     <cwd>/cook_modules/?/init.lua                     hand-vendored, dir module
///     <cwd>/cook_modules/share/lua/5.4/?.lua            LuaRocks pure Lua
///     <cwd>/cook_modules/share/lua/5.4/?/init.lua       LuaRocks pure Lua
///     <original>
///
///   package.cpath:
///     <cwd>/cook_modules/?.<so-ext>                     hand-vendored, top level
///     <cwd>/cook_modules/lib/lua/5.4/?.<so-ext>         LuaRocks-installed C
///     <original>
///
/// `<so-ext>` is `.so` on Linux/macOS (Lua's loader convention; LuaRocks emits
/// `.so` on macOS too) and `.dll` on Windows. The original suffixes are stashed
/// once so per-unit refresh is idempotent across calls.
fn refresh_package_search_paths(lua: &mlua::Lua, cwd: &PathBuf) -> mlua::Result<()> {
    let cook_modules = cwd.join("cook_modules");
    let pkg: mlua::Table = match lua.globals().get::<mlua::Value>("package")? {
        mlua::Value::Table(t) => t,
        _ => return Ok(()),
    };

    // Stash originals on first call so subsequent calls don't grow the suffix.
    let original_path: String = match pkg.get::<mlua::Value>("_cook_original_path")? {
        mlua::Value::String(s) => s.to_str()?.to_string(),
        _ => {
            let cur: String = pkg.get::<String>("path").unwrap_or_default();
            pkg.set("_cook_original_path", cur.clone())?;
            cur
        }
    };
    let original_cpath: String = match pkg.get::<mlua::Value>("_cook_original_cpath")? {
        mlua::Value::String(s) => s.to_str()?.to_string(),
        _ => {
            let cur: String = pkg.get::<String>("cpath").unwrap_or_default();
            pkg.set("_cook_original_cpath", cur.clone())?;
            cur
        }
    };

    let cm = cook_modules.display().to_string();
    let so_ext = if cfg!(target_os = "windows") { "dll" } else { "so" };

    let new_path = format!(
        "{cm}/?.lua;{cm}/?/init.lua;{cm}/share/lua/5.4/?.lua;{cm}/share/lua/5.4/?/init.lua;{orig}",
        cm = cm,
        orig = original_path,
    );
    let new_cpath = format!(
        "{cm}/?.{ext};{cm}/lib/lua/5.4/?.{ext};{orig}",
        cm = cm,
        ext = so_ext,
        orig = original_cpath,
    );

    pkg.set("path", new_path)?;
    pkg.set("cpath", new_cpath)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Shell execution (worker variant with prefixed output)
// ---------------------------------------------------------------------------

/// Maximum bytes per captured stream included in a COOK_CMD_FAILED error
/// message. Larger outputs are truncated with a marker so a chatty failure
/// (e.g., a verbose linker spew) doesn't blow up the error string.
const COOK_CMD_FAIL_STREAM_CAP: usize = 64 * 1024;

/// Lossy-decode a captured stream and apply the cap. Returns an empty
/// string for empty input so callers can suppress the corresponding
/// section header.
fn truncate_captured_stream(stream: &[u8]) -> String {
    if stream.is_empty() {
        return String::new();
    }
    let head_slice = if stream.len() > COOK_CMD_FAIL_STREAM_CAP {
        &stream[..COOK_CMD_FAIL_STREAM_CAP]
    } else {
        stream
    };
    let mut head = String::from_utf8_lossy(head_slice).into_owned();
    if stream.len() > COOK_CMD_FAIL_STREAM_CAP {
        if !head.ends_with('\n') {
            head.push('\n');
        }
        head.push_str(&format!(
            "... ({} bytes truncated)\n",
            stream.len() - COOK_CMD_FAIL_STREAM_CAP
        ));
    }
    head
}

/// Twin of cook-cli/src/diagnostics.rs::sanitize_error — keep in sync.
/// Cuts the mlua traceback (unless COOK_BACKTRACE=1) and drops the
/// "lua error: " / "runtime error: " wrapper prefixes.
fn sanitize_lua_error(msg: &str) -> String {
    let keep_traceback = std::env::var("COOK_BACKTRACE").map(|v| v == "1").unwrap_or(false);
    let mut m = msg.to_string();
    if !keep_traceback {
        if let Some(pos) = m.find("\nstack traceback:") {
            m.truncate(pos);
        }
    }
    let rest = m.as_str();
    let rest = rest.strip_prefix("lua error: ").unwrap_or(rest);
    let rest = rest.strip_prefix("runtime error: ").unwrap_or(rest);
    rest.to_string()
}

/// Build the canonical COOK_CMD_FAILED error string with captured streams
/// appended on subsequent lines. The first line keeps the pre-existing
/// `COOK_CMD_FAILED:<line>:<code>:<cmd>` shape so the parser at
/// `cook-cli/src/pipeline.rs:348` continues to extract line/code (and
/// flows the trailing captured streams through to the user via the
/// `command` field of the displayed error).
pub fn format_cmd_failed(
    line: usize,
    code: i32,
    cmd: &str,
    stdout: &[u8],
    stderr: &[u8],
) -> String {
    let mut msg = format!("COOK_CMD_FAILED:{line}:{code}:{cmd}");
    let stdout_str = truncate_captured_stream(stdout);
    if !stdout_str.is_empty() {
        msg.push_str("\n--- stdout ---\n");
        msg.push_str(&stdout_str);
        if !msg.ends_with('\n') {
            msg.push('\n');
        }
    }
    let stderr_str = truncate_captured_stream(stderr);
    if !stderr_str.is_empty() {
        msg.push_str("--- stderr ---\n");
        msg.push_str(&stderr_str);
    }
    msg
}

fn run_shell_in_worker(
    cmd: &str,
    wd: &std::path::Path,
    env_vars: &HashMap<String, String>,
    _recipe_name: &str,
    line: usize,
) -> mlua::Result<String> {
    let mut child_env: HashMap<String, String> = std::env::vars().collect();
    for (k, v) in env_vars {
        child_env.insert(k.clone(), v.clone());
    }

    // COOK-306: an executed command may write anywhere in the tree.
    cook_fingerprint::statmemo::disarm();
    let output = std::process::Command::new("/bin/sh")
        .arg("-c")
        .arg(cmd)
        .current_dir(wd)
        .envs(&child_env)
        .output()
        .map_err(|e| mlua::Error::runtime(format!("failed to execute: {e}")))?;

    if !output.status.success() {
        let code = output.status.code().unwrap_or(1);
        return Err(mlua::Error::runtime(format_cmd_failed(
            line,
            code,
            cmd,
            &output.stdout,
            &output.stderr,
        )));
    }

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    Ok(stdout)
}

// ---------------------------------------------------------------------------
// Execute a single WorkItem
// ---------------------------------------------------------------------------

fn execute_work_item(
    lua: &mlua::Lua,
    work: &WorkItem,
    working_dir: &PathBuf,
    env_vars: &HashMap<String, String>,
) -> WorkResult {
    // Test-only panic injection: lets `test_pool_recovers_from_worker_panic`
    // exercise the `catch_unwind` boundary in worker_loop without depending
    // on a panic path that's hard to trigger from the public API. mlua
    // catches panics raised from inside Lua callbacks, so a Lua-side
    // trigger would never reach `catch_unwind`.
    #[cfg(test)]
    if work.recipe_name == "__cook_test_panic__" {
        panic!("forced test panic");
    }

    let node_name = work.payload.display_name();

    match &work.payload {
        WorkPayload::Shell { cmd, line } => {
            execute_shell(work.id, cmd, *line, working_dir, env_vars, node_name)
        }
        WorkPayload::LuaChunk {
            code,
            inputs,
            outputs,
            ingredient_groups,
            step_kind: _,
            // is_chore is consumed by the engine's chore-window dispatch
            // before the item ever reaches the worker pool.
            is_chore: _,
            line,
        } => execute_lua_chunk(
            lua,
            work.id,
            code,
            inputs,
            outputs,
            ingredient_groups,
            &work.recipe_name,
            node_name,
            *line,
        ),
        WorkPayload::Interactive { .. } => {
            WorkResult {
                id: work.id,
                success: false,
                error: Some("BUG: interactive step dispatched to worker pool".to_string()),
                test_output: None,
                node_name,
                output_lines: Vec::new(),
                probe_output: None,
                duration: Duration::ZERO,
            }
        }
        WorkPayload::Test { cmd, line, timeout, should_fail, suite_name, test_name, lua_code, .. } => {
            match lua_code {
                Some(code) => execute_lua_test(
                    lua,
                    work.id,
                    code,
                    *timeout,
                    *should_fail,
                    suite_name,
                    test_name,
                    node_name,
                ),
                None => execute_test(work.id, cmd, *line, *timeout, *should_fail, suite_name, test_name, working_dir, env_vars, node_name),
            }
        }
        WorkPayload::Probe { key, produce, line } => {
            execute_probe(lua, work.id, key, produce, *line, node_name)
        }
        // `WorkPayload` is `#[non_exhaustive]` so the reference implementation
        // can introduce new payload kinds without an immediate breaking change.
        // Treat any unknown variant as a worker-side bug — the dispatcher
        // upstream of this fn is responsible for routing only known kinds.
        _ => WorkResult {
            id: work.id,
            success: false,
            error: Some(format!("BUG: unknown WorkPayload variant dispatched to worker pool: {:?}", work.payload)),
            test_output: None,
            node_name,
            output_lines: Vec::new(),
            probe_output: None,
            duration: Duration::ZERO,
        },
    }
}

fn execute_shell(
    id: usize,
    cmd: &str,
    line: usize,
    working_dir: &PathBuf,
    env_vars: &HashMap<String, String>,
    node_name: String,
) -> WorkResult {
    let mut child_env: HashMap<String, String> = std::env::vars().collect();
    for (k, v) in env_vars {
        child_env.insert(k.clone(), v.clone());
    }

    // COOK-306: an executed command may write anywhere in the tree.
    cook_fingerprint::statmemo::disarm();
    let result = std::process::Command::new("/bin/sh")
        .arg("-c")
        .arg(cmd)
        .current_dir(working_dir)
        .envs(&child_env)
        .output();

    match result {
        Err(e) => WorkResult {
            id,
            success: false,
            error: Some(format!("failed to execute: {e}")),
            test_output: None,
            node_name,
            output_lines: Vec::new(),
            probe_output: None,
            duration: Duration::ZERO,
        },
        Ok(output) => {
            let mut output_lines: Vec<(OutputStream, String)> = Vec::new();

            // Accumulate stderr lines (tagged so downstream renderers can
            // preserve fd-of-origin — CS-0035).
            if !output.stderr.is_empty() {
                let stderr_str = String::from_utf8_lossy(&output.stderr);
                for l in stderr_str.lines() {
                    output_lines.push((OutputStream::Stderr, l.to_string()));
                }
            }

            // Accumulate stdout lines.
            if !output.stdout.is_empty() {
                let stdout_str = String::from_utf8_lossy(&output.stdout);
                for l in stdout_str.lines() {
                    output_lines.push((OutputStream::Stdout, l.to_string()));
                }
            }

            if output.status.success() {
                WorkResult {
                    id,
                    success: true,
                    error: None,
                    test_output: None,
                    node_name,
                    output_lines,
                    probe_output: None,
                    duration: Duration::ZERO,
                }
            } else {
                let code = output.status.code().unwrap_or(1);
                WorkResult {
                    id,
                    success: false,
                    error: Some(format_cmd_failed(
                        line,
                        code,
                        cmd,
                        &output.stdout,
                        &output.stderr,
                    )),
                    test_output: None,
                    node_name,
                    output_lines,
                    probe_output: None,
                    duration: Duration::ZERO,
                }
            }
        }
    }
}

/// Execute a `WorkPayload::Probe` unit on the worker Lua VM (§22.5.6).
///
/// Wraps `produce` in `function() ... end` and invokes it, captures the return
/// value, renders it to canonical JSON (§22.5.5, CS-0102), and returns a
/// `WorkResult` with the `probe_output` field populated. Errors in the Lua
/// source or in the JSON conversion propagate as a normal unit failure.
fn execute_probe(
    lua: &mlua::Lua,
    id: usize,
    key: &str,
    produce: &str,
    _line: usize,
    node_name: String,
) -> WorkResult {
    let chunk_name = format!("@probe:{}", key);
    let wrapped = format!("return (function()\n{}\nend)()", produce);

    let value: mlua::Value = match lua.load(&wrapped).set_name(&chunk_name).eval() {
        Ok(v) => v,
        Err(e) => {
            return WorkResult {
                id,
                success: false,
                error: Some(format!(
                    "probe '{}' produce raised: {}",
                    key,
                    sanitize_lua_error(&e.to_string())
                )),
                test_output: None,
                node_name,
                output_lines: Vec::new(),
                probe_output: None,
                duration: Duration::ZERO,
            };
        }
    };

    let jv = match crate::probe_value::lua_to_json(&value) {
        Ok(v) => v,
        Err(e) => {
            return WorkResult {
                id,
                success: false,
                error: Some(format!("probe '{}': {}", key, e)),
                test_output: None,
                node_name,
                output_lines: Vec::new(),
                probe_output: None,
                duration: Duration::ZERO,
            };
        }
    };

    let bytes = cook_contracts::probe_value::encode_canonical_json(&jv);

    WorkResult {
        id,
        success: true,
        error: None,
        test_output: None,
        node_name,
        output_lines: Vec::new(),
        probe_output: Some(ProbeOutput {
            key: key.to_string(),
            bytes,
        }),
        duration: Duration::ZERO,
    }
}

fn execute_lua_chunk(
    lua: &mlua::Lua,
    id: usize,
    code: &str,
    inputs: &[String],
    outputs: &[String],
    ingredient_groups: &[Vec<String>],
    recipe_name: &str,
    node_name: String,
    line: usize,
) -> WorkResult {
    let setup = || -> mlua::Result<()> {
        let globals = lua.globals();

        let inputs_tbl = lua.create_table()?;
        for (i, s) in inputs.iter().enumerate() {
            inputs_tbl.set(i + 1, s.as_str())?;
        }
        globals.set("inputs", inputs_tbl)?;

        let outputs_tbl = lua.create_table()?;
        for (i, s) in outputs.iter().enumerate() {
            outputs_tbl.set(i + 1, s.as_str())?;
        }
        globals.set("outputs", outputs_tbl)?;

        globals.set("input", inputs.first().map(|s| s.as_str()).unwrap_or(""))?;
        globals.set("output", outputs.first().map(|s| s.as_str()).unwrap_or(""))?;

        // Set input_1, input_2, ... for each ingredient group
        for (i, group) in ingredient_groups.iter().enumerate() {
            let table = lua.create_table()?;
            for (j, path) in group.iter().enumerate() {
                table.set(j + 1, path.as_str())?;
            }
            globals.set(format!("input_{}", i + 1), table)?;
        }

        // COOK-191/CS-0126: newline-pad the chunk so line 1 of `code` lands
        // at the originating step's Cookfile line, then name the chunk
        // `@Cookfile` so mlua treats it as a file source. Together these
        // make an execute-phase Lua error read `Cookfile:LINE: msg`
        // instead of the opaque `[string "..."]:1: msg` produced by an
        // unnamed/unpadded `load`. A multi-line `>{ }` block's internal
        // lines resolve correctly too, since `code` is spliced in verbatim
        // after the padding — line k of the block reports as line+k-1.
        //
        // Known imprecision: in a multi-Cookfile workspace the worker has
        // no way to know which imported Cookfile a step came from, so
        // `@Cookfile` is only exactly right for the entry file. This is a
        // follow-up concern, not addressed here.
        let padded;
        let src: &str = if line > 1 {
            let mut s = String::with_capacity(code.len() + line);
            for _ in 1..line {
                s.push('\n');
            }
            s.push_str(code);
            padded = s;
            &padded
        } else {
            code
        };
        lua.load(src).set_name("@Cookfile").exec()?;
        Ok(())
    };

    let result = setup();

    // Flush this worker VM's stdout so recipe output (io.write/print) reaches
    // fd 1 now, before the completion event. Otherwise libc block-buffers it
    // when stdout isn't a TTY and it prints AFTER the `cook done` summary.
    // Runs on both the success and chunk-error paths so partial output isn't
    // stranded in the C stdio buffer.
    let _ = lua.load("io.stdout:flush()").exec();

    match result {
        Ok(()) => WorkResult {
            id,
            success: true,
            error: None,
            test_output: None,
            node_name,
            output_lines: Vec::new(),
            probe_output: None,
            duration: Duration::ZERO,
        },
        Err(e) => WorkResult {
            id,
            success: false,
            error: Some(format!("[{recipe_name}] {}", sanitize_lua_error(&e.to_string()))),
            test_output: None,
            node_name,
            output_lines: Vec::new(),
            probe_output: None,
            duration: Duration::ZERO,
        },
    }
}

/// Execute a `WorkPayload::Test` unit whose body is a Lua chunk (`lua_code`,
/// CS-0127 §22.4) on the worker Lua VM — the sibling of `execute_test` for
/// the shell-command path. Pass/fail is whether the chunk completes without
/// raising a Lua error; `should_fail` inverts the result exactly as it does
/// for shell tests (mirrors `execute_test`'s `success` computation).
///
/// Timeout is enforced best-effort via an instruction-count VM hook: every
/// 100_000 executed instructions, the hook checks wall-clock elapsed time
/// against `timeout_secs` and raises a Lua runtime error once exceeded. This
/// only interrupts *Lua bytecode* execution — a blocking `cook.sh` (or other
/// long-running foreign call) invoked from the test body runs to completion
/// unobserved by the hook, since the hook fires between VM instructions and
/// cannot preempt a call already in flight. A test body that shells out to
/// something that hangs can therefore exceed `timeout_secs` before this
/// function returns.
fn execute_lua_test(
    lua: &mlua::Lua,
    id: usize,
    code: &str,
    timeout_secs: u64,
    should_fail: bool,
    suite_name: &str,
    test_name: &str,
    node_name: String,
) -> WorkResult {
    let start = std::time::Instant::now();
    let timeout_dur = std::time::Duration::from_secs(timeout_secs);
    let timed_out_flag = Arc::new(AtomicBool::new(false));

    let hook_timed_out = Arc::clone(&timed_out_flag);
    let hook_test_name = test_name.to_string();
    lua.set_hook(
        mlua::HookTriggers::new().every_nth_instruction(100_000),
        move |_lua, _debug| {
            if start.elapsed() > timeout_dur {
                hook_timed_out.store(true, Ordering::Relaxed);
                return Err(mlua::Error::runtime(format!(
                    "test '{hook_test_name}' exceeded timeout of {timeout_secs}s"
                )));
            }
            Ok(mlua::VmState::Continue)
        },
    );

    let chunk_name = format!("@test:{suite_name}:{test_name}");
    let exec_result = lua.load(code).set_name(&chunk_name).exec();

    // Always remove the hook before returning — it captures `start` and
    // `timed_out_flag` by move and must not outlive this call; leaving it
    // installed would fire on whatever the next work item's Lua does.
    lua.remove_hook();

    let duration = start.elapsed().as_secs_f64();
    let chunk_ok = exec_result.is_ok();
    let timed_out = timed_out_flag.load(Ordering::Relaxed);
    let stderr = match &exec_result {
        Ok(()) => String::new(),
        Err(e) => e.to_string(),
    };

    let success = if should_fail { !chunk_ok } else { chunk_ok };

    // Mirror execute_test's CS-0035 stream-tagged output_lines so a failing
    // lua test's error text reaches the runner's live output the same way a
    // failing shell test's stderr does — otherwise only the terse
    // "test failed: <name>" summary would ever reach the terminal.
    let mut output_lines: Vec<(OutputStream, String)> = Vec::new();
    for line in stderr.lines() {
        output_lines.push((OutputStream::Stderr, line.to_string()));
    }

    WorkResult {
        id,
        success,
        error: if success { None } else { Some(format!("test failed: {test_name}")) },
        test_output: Some(TestOutput {
            suite_name: suite_name.to_string(),
            test_name: test_name.to_string(),
            stdout: String::new(),
            stderr,
            duration,
            timed_out,
            should_fail,
            exit_success: chunk_ok,
            exit_code: None,
        }),
        node_name,
        output_lines,
        probe_output: None,
        duration: Duration::ZERO,
    }
}

fn execute_test(
    id: usize,
    cmd: &str,
    _line: usize,
    timeout_secs: u64,
    should_fail: bool,
    suite_name: &str,
    test_name: &str,
    working_dir: &PathBuf,
    env_vars: &HashMap<String, String>,
    node_name: String,
) -> WorkResult {
    use std::io::Read;

    let start = std::time::Instant::now();

    let mut child_env: HashMap<String, String> = std::env::vars().collect();
    for (k, v) in env_vars {
        child_env.insert(k.clone(), v.clone());
    }

    // COOK-306: an executed command may write anywhere in the tree.
    cook_fingerprint::statmemo::disarm();
    let child = std::process::Command::new("/bin/sh")
        .args(["-c", cmd])
        .current_dir(working_dir)
        .envs(&child_env)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn();

    let mut child = match child {
        Ok(c) => c,
        Err(e) => {
            return WorkResult {
                id,
                success: false,
                error: Some(format!("failed to spawn test: {e}")),
                test_output: Some(TestOutput {
                    suite_name: suite_name.to_string(),
                    test_name: test_name.to_string(),
                    stdout: String::new(),
                    stderr: format!("failed to spawn: {e}"),
                    duration: 0.0,
                    timed_out: false,
                    should_fail,
                    exit_success: false,
                    exit_code: None,
                }),
                node_name,
                output_lines: Vec::new(),
                probe_output: None,
                duration: Duration::ZERO,
            };
        }
    };

    // Drain stdout/stderr in separate threads to prevent pipe-buffer deadlocks
    let stdout_handle = child.stdout.take().map(|s| {
        std::thread::spawn(move || {
            let mut buf = String::new();
            let mut reader = std::io::BufReader::new(s);
            reader.read_to_string(&mut buf).ok();
            buf
        })
    });
    let stderr_handle = child.stderr.take().map(|s| {
        std::thread::spawn(move || {
            let mut buf = String::new();
            let mut reader = std::io::BufReader::new(s);
            reader.read_to_string(&mut buf).ok();
            buf
        })
    });

    // Wait with timeout
    let timeout = std::time::Duration::from_secs(timeout_secs);
    let timed_out;
    let exit_success;
    let exit_code: Option<i32>;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                timed_out = false;
                exit_success = status.success();
                exit_code = status.code();
                break;
            }
            Ok(None) => {
                if start.elapsed() >= timeout {
                    let _ = child.kill();
                    let _ = child.wait();
                    timed_out = true;
                    exit_success = false;
                    exit_code = None;
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            Err(_) => {
                timed_out = false;
                exit_success = false;
                exit_code = None;
                break;
            }
        }
    }

    let stdout = stdout_handle.and_then(|h| h.join().ok()).unwrap_or_default();
    let stderr = stderr_handle.and_then(|h| h.join().ok()).unwrap_or_default();
    let duration = start.elapsed().as_secs_f64();

    let success = if should_fail { !exit_success } else { exit_success };

    // Populate output_lines from captured test output, tagging by fd
    // origin so the engine event stream can carry true stdout/stderr
    // provenance (CS-0035).
    let mut output_lines: Vec<(OutputStream, String)> = Vec::new();
    for line in stdout.lines() {
        output_lines.push((OutputStream::Stdout, line.to_string()));
    }
    for line in stderr.lines() {
        output_lines.push((OutputStream::Stderr, line.to_string()));
    }

    WorkResult {
        id,
        success,
        error: if success { None } else { Some(format!("test failed: {test_name}")) },
        test_output: Some(TestOutput {
            suite_name: suite_name.to_string(),
            test_name: test_name.to_string(),
            stdout,
            stderr,
            duration,
            timed_out,
            should_fail,
            exit_success,
            exit_code,
        }),
        node_name,
        output_lines,
        probe_output: None,
        duration: Duration::ZERO,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[path = "tests/pool_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "tests/search_path_tests.rs"]
mod search_path_tests;
