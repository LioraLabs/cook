use std::collections::HashMap;
use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::mpsc;
use std::sync::{Arc, Condvar, Mutex};

use cook_contracts::WorkPayload;

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

pub struct WorkItem {
    pub id: usize,
    pub payload: WorkPayload,
    pub recipe_name: String,
    pub working_dir: PathBuf,
    pub env_vars: HashMap<String, String>,
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
}

pub struct WorkResult {
    pub id: usize,
    pub success: bool,
    pub error: Option<String>,
    pub test_output: Option<TestOutput>,
    pub node_name: String,
    pub output_lines: Vec<String>,
}

pub struct WorkerPool {
    threads: Vec<std::thread::JoinHandle<()>>,
    queue: Arc<SharedQueue>,
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
    /// Spawn `n` worker threads.  Each thread creates its own `mlua::Lua` VM
    /// and pulls work items from the shared queue.  Results are sent back
    /// through the returned `mpsc::Receiver`.
    pub fn spawn(n: usize) -> (Self, mpsc::Receiver<WorkResult>) {
        let shared = Arc::new(SharedQueue {
            queue: Mutex::new(VecDeque::new()),
            condvar: Condvar::new(),
        });

        let (tx, rx) = mpsc::channel();

        let mut threads = Vec::with_capacity(n);

        for _ in 0..n {
            let q = Arc::clone(&shared);
            let tx = tx.clone();

            let handle = std::thread::spawn(move || {
                worker_loop(q, tx);
            });
            threads.push(handle);
        }

        (WorkerPool { threads, queue: shared }, rx)
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
) {
    // Each worker creates its own Lua VM.  The VM is `!Send` but never
    // leaves this thread, so this is safe.
    let lua = unsafe { mlua::Lua::unsafe_new() };
    crate::path_api::register_path_api(&lua).expect("failed to register path API");

    // Shared mutable state for per-item context (single-threaded within
    // this worker, but needs interior mutability for closures).
    let current_recipe: Arc<Mutex<String>> = Arc::new(Mutex::new(String::new()));
    let current_working_dir: Arc<Mutex<PathBuf>> = Arc::new(Mutex::new(PathBuf::new()));
    let current_env_vars: Arc<Mutex<HashMap<String, String>>> = Arc::new(Mutex::new(HashMap::new()));

    // Register the `cook` table once with closures that capture shared state.
    register_worker_cook_table(&lua, &current_working_dir, &current_env_vars, &current_recipe)
        .expect("failed to register cook table");

    // Register the `fs` table once at startup. Closures read the cwd
    // through `Arc<Mutex<PathBuf>>` so each call sees the *current* work
    // item's working_dir, not the one in effect at registration time.
    crate::fs_api::register_fs_api(&lua, &current_working_dir)
        .expect("failed to register fs API");

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

                // Refresh package.path so `require` resolves cook_modules/
                // relative to this unit's source Cookfile (CS-0017).
                let _ = refresh_package_path(&lua, &work.working_dir);

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
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    execute_work_item(&lua, &work, &work.working_dir, &work.env_vars)
                }));
                let result = match result {
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
                        }
                    }
                };
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
    current_recipe: &Arc<Mutex<String>>,
) -> mlua::Result<()> {
    let cook = lua.create_table()?;

    // cook.sh(cmd) -> stdout string
    let wd = Arc::clone(current_working_dir);
    let env = Arc::clone(current_env_vars);
    let recipe = Arc::clone(current_recipe);
    let sh_fn = lua.create_function(move |_, cmd: String| {
        let recipe_name = recipe.lock().expect("recipe name lock").clone();
        let working_dir = wd.lock().expect("working_dir lock").clone();
        let env_vars = env.lock().expect("env_vars lock").clone();
        run_shell_in_worker(&cmd, &working_dir, &env_vars, &recipe_name, 0)
    })?;
    cook.set("sh", sh_fn)?;

    // cook.exec(cmd, line) -> stdout string
    let wd2 = Arc::clone(current_working_dir);
    let env2 = Arc::clone(current_env_vars);
    let recipe2 = Arc::clone(current_recipe);
    let exec_fn = lua.create_function(move |_, (cmd, line): (String, usize)| {
        let recipe_name = recipe2.lock().expect("recipe name lock").clone();
        let working_dir = wd2.lock().expect("working_dir lock").clone();
        let env_vars = env2.lock().expect("env_vars lock").clone();
        run_shell_in_worker(&cmd, &working_dir, &env_vars, &recipe_name, line)
    })?;
    cook.set("exec", exec_fn)?;

    // cook.env — use a metatable __index so reads always reflect current env_vars
    let env_table = lua.create_table()?;
    let env_for_index = Arc::clone(current_env_vars);
    let meta = lua.create_table()?;
    meta.set("__index", lua.create_function(move |_, (_tbl, key): (mlua::Value, String)| {
        let env_vars = env_for_index.lock().expect("env_vars lock");
        Ok(env_vars.get(&key).cloned())
    })?)?;
    env_table.set_metatable(Some(meta));
    cook.set("env", env_table)?;

    // cook.platform
    crate::platform_api::register_platform_api(lua, &cook)?;

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

        // Resolve cook_modules/<name>.lua or cook_modules/<name>/init.lua
        let modules_dir = cwd.join("cook_modules");
        let flat_path = modules_dir.join(format!("{}.lua", name));
        let init_path = modules_dir.join(&name).join("init.lua");
        let module_path = if flat_path.exists() {
            flat_path
        } else if init_path.exists() {
            init_path
        } else {
            return Err(mlua::Error::runtime(format!(
                "cook.load_module: module '{}' not found in {}/cook_modules/ \
                 (tried {}.lua and {}/init.lua)",
                name, cwd.display(), name, name,
            )));
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

    lua.globals().set("cook", cook)?;
    Ok(())
}

/// Refresh `package.path` for the upcoming work unit so `require("foo")`
/// finds `<cwd>/cook_modules/foo.lua` (CS-0017). Called per-unit from the
/// worker loop because `cwd` is per-Cookfile and each body unit may come
/// from a different one.
fn refresh_package_path(lua: &mlua::Lua, cwd: &PathBuf) -> mlua::Result<()> {
    let cook_modules = cwd.join("cook_modules");
    let pkg: mlua::Table = match lua.globals().get::<mlua::Value>("package")? {
        mlua::Value::Table(t) => t,
        _ => return Ok(()),
    };
    // Hold onto the original (OS-default) suffix so we don't grow it
    // per-unit. We stash it on the package table itself the first time we
    // see it.
    let original: String = match pkg.get::<mlua::Value>("_cook_original_path")? {
        mlua::Value::String(s) => s.to_str()?.to_string(),
        _ => {
            let cur: String = pkg.get::<String>("path").unwrap_or_default();
            pkg.set("_cook_original_path", cur.clone())?;
            cur
        }
    };
    let new_path = format!(
        "{cm}/?.lua;{cm}/?/init.lua;{orig}",
        cm = cook_modules.display(),
        orig = original,
    );
    pkg.set("path", new_path)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Shell execution (worker variant with prefixed output)
// ---------------------------------------------------------------------------

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

    let output = std::process::Command::new("/bin/sh")
        .arg("-c")
        .arg(cmd)
        .current_dir(wd)
        .envs(&child_env)
        .output()
        .map_err(|e| mlua::Error::runtime(format!("failed to execute: {e}")))?;

    if !output.status.success() {
        let code = output.status.code().unwrap_or(1);
        return Err(mlua::Error::runtime(format!(
            "COOK_CMD_FAILED:{}:{}:{}",
            line, code, cmd
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
        } => execute_lua_chunk(
            lua,
            work.id,
            code,
            inputs,
            outputs,
            ingredient_groups,
            &work.recipe_name,
            node_name,
        ),
        WorkPayload::Interactive { .. } => {
            WorkResult {
                id: work.id,
                success: false,
                error: Some("BUG: interactive step dispatched to worker pool".to_string()),
                test_output: None,
                node_name,
                output_lines: Vec::new(),
            }
        }
        WorkPayload::Test { cmd, line, timeout, should_fail, suite_name, test_name } => {
            execute_test(work.id, cmd, *line, *timeout, *should_fail, suite_name, test_name, working_dir, env_vars, node_name)
        }
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
        },
        Ok(output) => {
            let mut output_lines = Vec::new();

            // Accumulate stderr lines
            if !output.stderr.is_empty() {
                let stderr_str = String::from_utf8_lossy(&output.stderr);
                for l in stderr_str.lines() {
                    output_lines.push(l.to_string());
                }
            }

            // Accumulate stdout lines
            if !output.stdout.is_empty() {
                let stdout_str = String::from_utf8_lossy(&output.stdout);
                for l in stdout_str.lines() {
                    output_lines.push(l.to_string());
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
                }
            } else {
                let code = output.status.code().unwrap_or(1);
                WorkResult {
                    id,
                    success: false,
                    error: Some(format!("COOK_CMD_FAILED:{}:{}:{}", line, code, cmd)),
                    test_output: None,
                    node_name,
                    output_lines,
                }
            }
        }
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

        lua.load(code).exec()?;
        Ok(())
    };

    match setup() {
        Ok(()) => WorkResult {
            id,
            success: true,
            error: None,
            test_output: None,
            node_name,
            output_lines: Vec::new(),
        },
        Err(e) => WorkResult {
            id,
            success: false,
            error: Some(format!("[{recipe_name}] lua error: {e}")),
            test_output: None,
            node_name,
            output_lines: Vec::new(),
        },
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
                }),
                node_name,
                output_lines: Vec::new(),
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
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                timed_out = false;
                exit_success = status.success();
                break;
            }
            Ok(None) => {
                if start.elapsed() >= timeout {
                    let _ = child.kill();
                    let _ = child.wait();
                    timed_out = true;
                    exit_success = false;
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            Err(_) => {
                timed_out = false;
                exit_success = false;
                break;
            }
        }
    }

    let stdout = stdout_handle.and_then(|h| h.join().ok()).unwrap_or_default();
    let stderr = stderr_handle.and_then(|h| h.join().ok()).unwrap_or_default();
    let duration = start.elapsed().as_secs_f64();

    let success = if should_fail { !exit_success } else { exit_success };

    // Populate output_lines from captured test output
    let mut output_lines = Vec::new();
    for line in stdout.lines() {
        output_lines.push(line.to_string());
    }
    for line in stderr.lines() {
        output_lines.push(line.to_string());
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
        }),
        node_name,
        output_lines,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn make_pool(n: usize) -> (WorkerPool, mpsc::Receiver<WorkResult>, TempDir) {
        let dir = TempDir::new().unwrap();
        let (pool, rx) = WorkerPool::spawn(n);
        (pool, rx, dir)
    }

    #[test]
    fn test_pool_executes_shell_command() {
        let (pool, rx, dir) = make_pool(1);

        pool.submit(WorkItem {
            id: 0,
            payload: WorkPayload::Shell {
                cmd: "echo hello".to_string(),
                line: 1,
            },
            recipe_name: "test_recipe".to_string(),
            working_dir: dir.path().to_path_buf(),
            env_vars: HashMap::new(),
        });

        let result = rx.recv().unwrap();
        assert!(result.success, "expected success, got error: {:?}", result.error);
        assert_eq!(result.id, 0);
        assert!(result.error.is_none());

        pool.shutdown();
    }

    #[test]
    fn test_pool_multiple_workers() {
        let (pool, rx, dir) = make_pool(4);

        for i in 0..8 {
            pool.submit(WorkItem {
                id: i,
                payload: WorkPayload::Shell {
                    cmd: "true".to_string(),
                    line: 1,
                },
                recipe_name: format!("recipe_{i}"),
                working_dir: dir.path().to_path_buf(),
                env_vars: HashMap::new(),
            });
        }

        let mut results = Vec::new();
        for _ in 0..8 {
            results.push(rx.recv().unwrap());
        }

        assert_eq!(results.len(), 8);
        for r in &results {
            assert!(r.success, "work item {} failed: {:?}", r.id, r.error);
        }

        // Verify all IDs are present (order may vary)
        let mut ids: Vec<usize> = results.iter().map(|r| r.id).collect();
        ids.sort();
        assert_eq!(ids, (0..8).collect::<Vec<_>>());

        pool.shutdown();
    }

    #[test]
    fn test_pool_reports_shell_failure() {
        let (pool, rx, dir) = make_pool(1);

        pool.submit(WorkItem {
            id: 42,
            payload: WorkPayload::Shell {
                cmd: "false".to_string(),
                line: 7,
            },
            recipe_name: "fail_recipe".to_string(),
            working_dir: dir.path().to_path_buf(),
            env_vars: HashMap::new(),
        });

        let result = rx.recv().unwrap();
        assert!(!result.success);
        assert_eq!(result.id, 42);
        let err = result.error.as_ref().expect("expected error message");
        assert!(
            err.contains("COOK_CMD_FAILED:7:1:false"),
            "unexpected error format: {err}"
        );

        pool.shutdown();
    }

    #[test]
    fn test_pool_working_dir() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("file.txt"), "contents").unwrap();

        let (pool, rx) = WorkerPool::spawn(1);

        pool.submit(WorkItem {
            id: 0,
            payload: WorkPayload::Shell {
                cmd: "cat file.txt".to_string(),
                line: 1,
            },
            recipe_name: "dir_test".to_string(),
            working_dir: dir.path().to_path_buf(),
            env_vars: HashMap::new(),
        });

        let result = rx.recv().unwrap();
        assert!(result.success, "expected success, got error: {:?}", result.error);

        pool.shutdown();
    }

    #[test]
    fn test_pool_executes_lua_chunk_writing_multiple_outputs() {
        let dir = TempDir::new().unwrap();
        let (pool, rx) = WorkerPool::spawn(1);

        let code = r#"
            local f = io.open(outputs[1], "w")
            f:write("a")
            f:close()
            local g = io.open(outputs[2], "w")
            g:write("b")
            g:close()
        "#;

        pool.submit(WorkItem {
            id: 0,
            payload: WorkPayload::LuaChunk {
                code: code.to_string(),
                inputs: vec!["src.rs".to_string()],
                outputs: vec![
                    dir.path().join("a.txt").to_string_lossy().into_owned(),
                    dir.path().join("b.txt").to_string_lossy().into_owned(),
                ],
                ingredient_groups: vec![vec!["src.rs".to_string()]],
            },
            recipe_name: "multi".to_string(),
            working_dir: dir.path().to_path_buf(),
            env_vars: HashMap::new(),
        });

        let result = rx.recv().unwrap();
        assert!(
            result.success,
            "expected success, got error: {:?}",
            result.error
        );
        assert_eq!(fs::read_to_string(dir.path().join("a.txt")).unwrap(), "a");
        assert_eq!(fs::read_to_string(dir.path().join("b.txt")).unwrap(), "b");

        pool.shutdown();
    }

    #[test]
    fn test_pool_lua_chunk_sees_input_output_globals() {
        let dir = TempDir::new().unwrap();
        let out_path = dir.path().join("out.txt");
        let (pool, rx) = WorkerPool::spawn(1);

        // Use singular `input`/`output` convention (single input/output case).
        let code = r#"
            local f = io.open(output, "w")
            f:write(input)
            f:close()
        "#;

        pool.submit(WorkItem {
            id: 0,
            payload: WorkPayload::LuaChunk {
                code: code.to_string(),
                inputs: vec!["hello".to_string()],
                outputs: vec![out_path.to_string_lossy().into_owned()],
                ingredient_groups: vec![],
            },
            recipe_name: "r".to_string(),
            working_dir: dir.path().to_path_buf(),
            env_vars: HashMap::new(),
        });

        let result = rx.recv().unwrap();
        assert!(
            result.success,
            "expected success, got error: {:?}",
            result.error
        );
        assert_eq!(fs::read_to_string(&out_path).unwrap(), "hello");

        pool.shutdown();
    }

    #[test]
    fn test_pool_env_vars() {
        let dir = TempDir::new().unwrap();
        let mut env = HashMap::new();
        env.insert("MY_VAR".to_string(), "hello_from_pool".to_string());

        let (pool, rx) = WorkerPool::spawn(1);

        pool.submit(WorkItem {
            id: 0,
            payload: WorkPayload::Shell {
                cmd: "echo $MY_VAR".to_string(),
                line: 1,
            },
            recipe_name: "env_test".to_string(),
            working_dir: dir.path().to_path_buf(),
            env_vars: env,
        });

        let result = rx.recv().unwrap();
        assert!(result.success, "expected success, got error: {:?}", result.error);

        pool.shutdown();
    }

    /// CS-0017 multi-Cookfile imports route work items with different
    /// `working_dir`s through the same worker. `fs.*` must resolve relative
    /// paths against each item's cwd, not the cwd of the first item the
    /// worker happened to see.
    #[test]
    fn test_pool_fs_api_uses_per_item_working_dir() {
        let dir1 = TempDir::new().unwrap();
        let dir2 = TempDir::new().unwrap();
        fs::write(dir1.path().join("data.txt"), "from-dir1").unwrap();
        fs::write(dir2.path().join("data.txt"), "from-dir2").unwrap();

        let out1 = dir1.path().join("out.txt");
        let out2 = dir2.path().join("out.txt");

        // Single worker so both items hit the same VM and exercise refresh.
        let (pool, rx) = WorkerPool::spawn(1);

        let code = r#"
            local content = fs.read("data.txt")
            local f = io.open(output, "w")
            f:write(content)
            f:close()
        "#;

        pool.submit(WorkItem {
            id: 0,
            payload: WorkPayload::LuaChunk {
                code: code.to_string(),
                inputs: vec![],
                outputs: vec![out1.to_string_lossy().into_owned()],
                ingredient_groups: vec![],
            },
            recipe_name: "r".to_string(),
            working_dir: dir1.path().to_path_buf(),
            env_vars: HashMap::new(),
        });
        let r1 = rx.recv().unwrap();
        assert!(r1.success, "first item failed: {:?}", r1.error);
        assert_eq!(fs::read_to_string(&out1).unwrap(), "from-dir1");

        pool.submit(WorkItem {
            id: 1,
            payload: WorkPayload::LuaChunk {
                code: code.to_string(),
                inputs: vec![],
                outputs: vec![out2.to_string_lossy().into_owned()],
                ingredient_groups: vec![],
            },
            recipe_name: "r".to_string(),
            working_dir: dir2.path().to_path_buf(),
            env_vars: HashMap::new(),
        });
        let r2 = rx.recv().unwrap();
        assert!(r2.success, "second item failed: {:?}", r2.error);
        assert_eq!(
            fs::read_to_string(&out2).unwrap(),
            "from-dir2",
            "fs.read must resolve against item 2's working_dir, \
             not the first item's"
        );

        pool.shutdown();
    }

    /// A Rust panic inside work-item processing must not hang the engine.
    /// The worker should surface a failure `WorkResult` and keep processing
    /// subsequent items. The magic recipe name `"__cook_test_panic__"` is
    /// recognized by `execute_work_item` under `#[cfg(test)]` and panics
    /// before the work payload runs — this exercises the panic boundary
    /// directly (mlua catches panics raised from inside Lua callbacks, so
    /// a Lua-side trigger wouldn't reach `catch_unwind`).
    #[test]
    fn test_pool_recovers_from_worker_panic() {
        let dir = TempDir::new().unwrap();
        let (pool, rx) = WorkerPool::spawn(1);

        // First item: triggers a panic in execute_work_item.
        pool.submit(WorkItem {
            id: 7,
            payload: WorkPayload::Shell {
                cmd: "true".to_string(),
                line: 1,
            },
            recipe_name: "__cook_test_panic__".to_string(),
            working_dir: dir.path().to_path_buf(),
            env_vars: HashMap::new(),
        });
        let r1 = rx.recv().unwrap();
        assert_eq!(r1.id, 7);
        assert!(!r1.success, "expected failure result, got success");
        let err = r1.error.as_ref().expect("expected error message");
        assert!(
            err.to_lowercase().contains("panic"),
            "error should mention the panic: {err}"
        );

        // Second item: same worker pool should still process this.
        pool.submit(WorkItem {
            id: 8,
            payload: WorkPayload::Shell {
                cmd: "echo recovered".to_string(),
                line: 1,
            },
            recipe_name: "recovery".to_string(),
            working_dir: dir.path().to_path_buf(),
            env_vars: HashMap::new(),
        });
        let r2 = rx.recv().unwrap();
        assert_eq!(r2.id, 8);
        assert!(r2.success, "post-panic item failed: {:?}", r2.error);

        pool.shutdown();
    }

    /// Dropping `WorkerPool` without an explicit `shutdown()` must signal
    /// the workers and join them. Otherwise the queue `Arc` outlives the
    /// pool — the workers leak, blocked on the condvar forever.
    #[test]
    fn test_pool_drop_cleans_up_workers() {
        let weak;
        {
            let (pool, _rx) = WorkerPool::spawn(2);
            weak = Arc::downgrade(&pool.queue);
        } // pool dropped here without shutdown()

        // After Drop, all worker threads should have exited and released
        // their `Arc<SharedQueue>` clones, so the only remaining strong
        // ref (the pool's) is also gone.
        assert!(
            weak.upgrade().is_none(),
            "queue Arc still alive after pool drop — workers were not joined"
        );
    }
}
