use std::collections::HashMap;
use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::mpsc;
use std::sync::{Arc, Condvar, Mutex};

use cook_contracts::{OutputStream, StepKind, WorkPayload};

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

pub struct WorkItem {
    pub id: usize,
    pub payload: WorkPayload,
    pub recipe_name: String,
    pub working_dir: PathBuf,
    pub env_vars: HashMap<String, String>,
    /// Project root for the CS-0045 sandbox. The worker installs the
    /// per-item sandbox policy by combining this root with the
    /// payload's `step_kind` (Cook/Test/Chore → Confined; Plate →
    /// Off). One worker VM may serve items from multiple projects in
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

    // `path.*` is pure string manipulation — install once.
    cook_lua_stdlib::register_path_api(&lua).expect("failed to register path API");

    // Shared mutable state for per-item context (single-threaded within
    // this worker, but needs interior mutability for closures).
    let current_recipe: Arc<Mutex<String>> = Arc::new(Mutex::new(String::new()));
    let current_working_dir: Arc<Mutex<PathBuf>> = Arc::new(Mutex::new(PathBuf::new()));
    let current_env_vars: Arc<Mutex<HashMap<String, String>>> = Arc::new(Mutex::new(HashMap::new()));

    // CS-0045 sandbox slot. Updated per work item before the body
    // runs: Cook/Test/Chore → Confined { project_root }, Plate → Off.
    // Default is `Off` — the slot is overwritten before the first
    // body executes, but if a future code path somehow runs Lua
    // before the first slot update, `Off` is the safe fallback (no
    // false positives on legitimate I/O).
    let current_sandbox: Arc<Mutex<cook_lua_stdlib::SandboxPolicy>> =
        Arc::new(Mutex::new(cook_lua_stdlib::SandboxPolicy::Off));

    // Register the `cook` table once with closures that capture shared state.
    register_worker_cook_table(&lua, &current_working_dir, &current_env_vars, &current_recipe)
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
                // CS-0045: pick the per-item sandbox policy. Plate
                // bodies run unsandboxed; everything else (Cook, Test,
                // Chore, and any non-LuaChunk payload) runs confined to
                // `project_root`. For Shell/Test/Interactive payloads
                // the policy is irrelevant — the worker doesn't run
                // user Lua for those — but setting it consistently
                // means a stray `lua.load()` in a future code path
                // can't accidentally land Off.
                {
                    let kind = match &work.payload {
                        WorkPayload::LuaChunk { step_kind, .. } => *step_kind,
                        _ => StepKind::Cook,
                    };
                    let policy = match kind {
                        StepKind::Plate => cook_lua_stdlib::SandboxPolicy::Off,
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

    // Note: `cook.exec`, `cook.interactive`, `cook.add_unit`,
    // `cook.step_group`, and `cook.recipe` are register-only API
    // (Standard §6.3.2). On the worker (execute-phase) VM they are
    // installed as error-raising guards near the bottom of this
    // function — see `install_register_only_guard`.

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

    // cook.platform — installed via the shared cook-lua-stdlib so the
    // execute-phase string values are byte-identical to the
    // register-phase ones (CS-0044).
    cook_lua_stdlib::register_platform_api(lua, &cook)?;

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
        "cook.exec: register-only API called from execute-phase Lua (Standard §6.3.2). \
         Use cook.sh(cmd) to shell out from a lua_line / lua_block / using >{ … } payload.",
    )?;
    install_register_only_guard(
        lua,
        &cook,
        "interactive",
        "cook.interactive: register-only API called from execute-phase Lua (Standard §6.3.2). \
         Interactive steps must be recorded during the register phase; they cannot be \
         scheduled from a lua_line / lua_block / using >{ … } payload.",
    )?;
    install_register_only_guard(
        lua,
        &cook,
        "add_unit",
        "cook.add_unit: register-only API called from execute-phase Lua (Standard §6.3.2). \
         Work units are recorded during the register phase; the DAG is closed before \
         execute-phase Lua runs.",
    )?;
    install_register_only_guard(
        lua,
        &cook,
        "step_group",
        "cook.step_group: register-only API called from execute-phase Lua (Standard §6.3.2). \
         Step groups are recorded during the register phase; they cannot be opened from a \
         lua_line / lua_block / using >{ … } payload.",
    )?;
    install_register_only_guard(
        lua,
        &cook,
        "recipe",
        "cook.recipe: register-only API called from execute-phase Lua (Standard §6.3.2). \
         Recipes are registered during the register phase; they cannot be declared from a \
         lua_line / lua_block / using >{ … } payload.",
    )?;

    lua.globals().set("cook", cook)?;
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
            ..
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
        WorkPayload::Test { cmd, line, timeout, should_fail, suite_name, test_name, .. } => {
            execute_test(work.id, cmd, *line, *timeout, *should_fail, suite_name, test_name, working_dir, env_vars, node_name)
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
                    exit_code: None,
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
            project_root: dir.path().to_path_buf(),
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
                project_root: dir.path().to_path_buf(),
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
            project_root: dir.path().to_path_buf(),
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
            project_root: dir.path().to_path_buf(),
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
                step_kind: cook_contracts::StepKind::Cook,
                is_chore: false,
            },
            recipe_name: "multi".to_string(),
            working_dir: dir.path().to_path_buf(),
            env_vars: HashMap::new(),
            project_root: dir.path().to_path_buf(),
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
                step_kind: cook_contracts::StepKind::Cook,
                is_chore: false,
            },
            recipe_name: "r".to_string(),
            working_dir: dir.path().to_path_buf(),
            env_vars: HashMap::new(),
            project_root: dir.path().to_path_buf(),
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
            project_root: dir.path().to_path_buf(),
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
                step_kind: cook_contracts::StepKind::Cook,
                is_chore: false,
            },
            recipe_name: "r".to_string(),
            working_dir: dir1.path().to_path_buf(),
            env_vars: HashMap::new(),
            project_root: dir1.path().to_path_buf(),
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
                step_kind: cook_contracts::StepKind::Cook,
                is_chore: false,
            },
            recipe_name: "r".to_string(),
            working_dir: dir2.path().to_path_buf(),
            env_vars: HashMap::new(),
            project_root: dir2.path().to_path_buf(),
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
            project_root: dir.path().to_path_buf(),
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
            project_root: dir.path().to_path_buf(),
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

    #[test]
    fn test_output_carries_exit_code() {
        let to = TestOutput {
            suite_name: "s".into(),
            test_name: "t".into(),
            stdout: String::new(),
            stderr: String::new(),
            duration: 0.0,
            timed_out: false,
            should_fail: false,
            exit_success: false,
            exit_code: Some(7),
        };
        assert_eq!(to.exit_code, Some(7));
    }

    // SHI-188: format_cmd_failed embeds captured stdout/stderr.
    #[test]
    fn format_cmd_failed_includes_captured_streams() {
        let msg = format_cmd_failed(
            42,
            1,
            "cc bad.c",
            b"compiling bad.c\n",
            b"bad.c:1: error: undeclared identifier\n",
        );
        // Legacy prefix preserved so the pipeline.rs parser still extracts
        // line/code.
        assert!(
            msg.starts_with("COOK_CMD_FAILED:42:1:cc bad.c"),
            "missing legacy prefix: {msg}"
        );
        // Both captured streams flow through.
        assert!(msg.contains("--- stdout ---\ncompiling bad.c"), "stdout missing: {msg}");
        assert!(
            msg.contains("--- stderr ---\nbad.c:1: error: undeclared identifier"),
            "stderr missing: {msg}"
        );
    }

    #[test]
    fn format_cmd_failed_omits_empty_stream_sections() {
        let msg = format_cmd_failed(0, 2, "false", b"", b"");
        assert_eq!(msg, "COOK_CMD_FAILED:0:2:false");
    }

    #[test]
    fn format_cmd_failed_truncates_huge_stream() {
        let huge = vec![b'.'; COOK_CMD_FAIL_STREAM_CAP * 2];
        let msg = format_cmd_failed(0, 1, "noisy", &huge, b"");
        assert!(msg.contains("... ("), "expected truncation marker: {}", &msg[..200]);
        assert!(
            msg.contains("bytes truncated"),
            "expected 'bytes truncated' marker: {}",
            &msg[..200]
        );
        // Cap plus a small fixed overhead — well under twice the cap.
        assert!(msg.len() < COOK_CMD_FAIL_STREAM_CAP + 1024);
    }

    // -----------------------------------------------------------------
    // Standard §6.3.2 regression tests: register-only Cook Lua API
    // helpers must raise a §6.3.2-shaped diagnostic when called from
    // execute-phase Lua (lua_line / lua_block / using >{ … } payload).
    // -----------------------------------------------------------------

    /// Submit a single LuaChunk work item that runs `code` on a worker VM,
    /// then return the resulting `WorkResult` for inspection.
    fn run_lua_chunk_in_worker(code: &str) -> WorkResult {
        let dir = TempDir::new().unwrap();
        let (pool, rx) = WorkerPool::spawn(1);
        pool.submit(WorkItem {
            id: 0,
            payload: WorkPayload::LuaChunk {
                code: code.to_string(),
                inputs: vec![],
                outputs: vec![],
                ingredient_groups: vec![],
                step_kind: cook_contracts::StepKind::Cook,
                is_chore: false,
            },
            recipe_name: "rec".to_string(),
            working_dir: dir.path().to_path_buf(),
            env_vars: HashMap::new(),
            project_root: dir.path().to_path_buf(),
        });
        let result = rx.recv().unwrap();
        pool.shutdown();
        result
    }

    fn assert_register_only_diagnostic(result: &WorkResult, fn_name: &str) {
        assert!(
            !result.success,
            "expected register-only-API call to fail; got success"
        );
        let err = result.error.as_deref().unwrap_or("");
        let needle_fn = format!("cook.{fn_name}");
        assert!(
            err.contains(&needle_fn),
            "diagnostic must name the function `{needle_fn}`; got: {err}"
        );
        assert!(
            err.contains("Standard §6.3.2"),
            "diagnostic must cite Standard §6.3.2; got: {err}"
        );
        assert!(
            err.contains("execute-phase Lua"),
            "diagnostic must identify the calling step kind as execute-phase Lua; got: {err}"
        );
    }

    #[test]
    fn cook_exec_from_execute_phase_raises_section_6_3_2_diagnostic() {
        let result = run_lua_chunk_in_worker(r#"cook.exec("echo hi", 0)"#);
        assert_register_only_diagnostic(&result, "exec");
    }

    #[test]
    fn cook_interactive_from_execute_phase_raises_section_6_3_2_diagnostic() {
        let result = run_lua_chunk_in_worker(r#"cook.interactive("echo hi", 0)"#);
        assert_register_only_diagnostic(&result, "interactive");
    }

    #[test]
    fn cook_add_unit_from_execute_phase_raises_section_6_3_2_diagnostic() {
        let result =
            run_lua_chunk_in_worker(r#"cook.add_unit({command = "echo hi"})"#);
        assert_register_only_diagnostic(&result, "add_unit");
    }

    #[test]
    fn cook_step_group_from_execute_phase_raises_section_6_3_2_diagnostic() {
        let result = run_lua_chunk_in_worker(r#"cook.step_group("g")"#);
        assert_register_only_diagnostic(&result, "step_group");
    }

    #[test]
    fn cook_recipe_from_execute_phase_raises_section_6_3_2_diagnostic() {
        let result =
            run_lua_chunk_in_worker(r#"cook.recipe("inner", {}, function() end)"#);
        assert_register_only_diagnostic(&result, "recipe");
    }

    /// `cook.sh` is the both-phase shell-out helper (§6.3.1) and MUST
    /// continue to work on the worker VM. Guard against accidentally
    /// classifying it as register-only.
    #[test]
    fn cook_sh_from_execute_phase_still_works() {
        let result = run_lua_chunk_in_worker(r#"cook.sh("true")"#);
        assert!(
            result.success,
            "cook.sh must remain callable in execute phase; got error: {:?}",
            result.error
        );
    }

    // -----------------------------------------------------------------
    // CS-0069 regressions: execute-phase `cook.load_module` MUST honour
    // §7's four-path resolution order, not just the top-level two paths.
    // -----------------------------------------------------------------

    /// Submit a single LuaChunk work item that runs `code` on a worker VM
    /// rooted at `cwd`, then return the resulting `WorkResult` for
    /// inspection. Unlike `run_lua_chunk_in_worker`, this lets the caller
    /// pre-populate `cwd` with module-resolution fixtures.
    fn run_lua_chunk_in_worker_at(cwd: &std::path::Path, code: &str) -> WorkResult {
        let (pool, rx) = WorkerPool::spawn(1);
        pool.submit(WorkItem {
            id: 0,
            payload: WorkPayload::LuaChunk {
                code: code.to_string(),
                inputs: vec![],
                outputs: vec![],
                ingredient_groups: vec![],
                step_kind: cook_contracts::StepKind::Cook,
                is_chore: false,
            },
            recipe_name: "rec".to_string(),
            working_dir: cwd.to_path_buf(),
            env_vars: HashMap::new(),
            project_root: cwd.to_path_buf(),
        });
        let result = rx.recv().unwrap();
        pool.shutdown();
        result
    }

    /// CS-0069: a module installed under `cook_modules/share/lua/5.4/<name>/init.lua`
    /// (the canonical LuaRocks share-tree layout for multi-file rocks) MUST
    /// be resolvable by execute-phase `cook.load_module`, not just by the
    /// register phase. Pre-CS-0069 this raised "module not found" because
    /// pool.rs only searched the two top-level paths.
    #[test]
    fn cook_load_module_resolves_share_lua_5_4_init() {
        let dir = TempDir::new().unwrap();
        let share_pkg = dir.path()
            .join("cook_modules/share/lua/5.4/share_only_pkg");
        fs::create_dir_all(&share_pkg).expect("mkdir share path");
        fs::write(
            share_pkg.join("init.lua"),
            "return { from_share = true }",
        ).expect("write init.lua");

        let code = r#"
            local m = cook.load_module("share_only_pkg")
            assert(m.from_share == true, "expected from_share=true, got "..tostring(m.from_share))
        "#;
        let result = run_lua_chunk_in_worker_at(dir.path(), code);
        assert!(
            result.success,
            "cook.load_module must resolve share/lua/5.4/<name>/init.lua; got error: {:?}",
            result.error
        );
    }

    /// CS-0069: a flat module file at `cook_modules/share/lua/5.4/<name>.lua`
    /// (single-file rocks) MUST also be resolvable from the execute phase.
    #[test]
    fn cook_load_module_resolves_share_lua_5_4_flat() {
        let dir = TempDir::new().unwrap();
        let share_dir = dir.path().join("cook_modules/share/lua/5.4");
        fs::create_dir_all(&share_dir).expect("mkdir share path");
        fs::write(
            share_dir.join("share_flat_pkg.lua"),
            "return { kind = 'flat' }",
        ).expect("write flat module");

        let code = r#"
            local m = cook.load_module("share_flat_pkg")
            assert(m.kind == "flat", "expected kind=flat, got "..tostring(m.kind))
        "#;
        let result = run_lua_chunk_in_worker_at(dir.path(), code);
        assert!(
            result.success,
            "cook.load_module must resolve share/lua/5.4/<name>.lua; got error: {:?}",
            result.error
        );
    }

    /// CS-0069: when a module exists at both the top-level and share-tree
    /// paths, the top-level (hand-vendored) copy MUST win. Mirrors the
    /// priority test in cook-register/src/module_loader.rs.
    #[test]
    fn cook_load_module_top_level_wins_over_share_lua() {
        let dir = TempDir::new().unwrap();
        let modules_dir = dir.path().join("cook_modules");
        let share_dir = modules_dir.join("share/lua/5.4");
        fs::create_dir_all(&share_dir).expect("mkdir share path");
        // Top-level (should win): flat <name>.lua under cook_modules/.
        fs::write(
            modules_dir.join("dup_pkg.lua"),
            "return { from = 'top-level' }",
        ).expect("write top-level module");
        // Share-tree (should lose): init.lua under share/lua/5.4/<name>/.
        let share_pkg = share_dir.join("dup_pkg");
        fs::create_dir_all(&share_pkg).expect("mkdir share pkg");
        fs::write(
            share_pkg.join("init.lua"),
            "return { from = 'share' }",
        ).expect("write share module");

        let code = r#"
            local m = cook.load_module("dup_pkg")
            assert(m.from == "top-level", "expected from=top-level, got "..tostring(m.from))
        "#;
        let result = run_lua_chunk_in_worker_at(dir.path(), code);
        assert!(
            result.success,
            "cook.load_module must prefer top-level over share-tree; got error: {:?}",
            result.error
        );
    }

    /// CS-0069: the diagnostic when no candidate path matches MUST list
    /// all four attempted paths.
    #[test]
    fn cook_load_module_miss_diagnostic_lists_all_four_paths() {
        let dir = TempDir::new().unwrap();
        let code = r#"cook.load_module("nonexistent_pkg")"#;
        let result = run_lua_chunk_in_worker_at(dir.path(), code);
        assert!(!result.success, "expected miss to fail");
        let err = result.error.as_deref().unwrap_or("");
        assert!(
            err.contains("nonexistent_pkg.lua"),
            "diagnostic must mention top-level flat path; got: {err}"
        );
        assert!(
            err.contains("nonexistent_pkg/init.lua"),
            "diagnostic must mention top-level init path; got: {err}"
        );
        assert!(
            err.contains("share/lua/5.4/nonexistent_pkg.lua"),
            "diagnostic must mention share-tree flat path; got: {err}"
        );
        assert!(
            err.contains("share/lua/5.4/nonexistent_pkg/init.lua"),
            "diagnostic must mention share-tree init path; got: {err}"
        );
    }
}

#[cfg(test)]
mod search_path_tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn refresh_sets_path_and_cpath_with_rock_tree_entries() {
        let lua = mlua::Lua::new();
        let cwd = PathBuf::from("/tmp/fake-project");
        refresh_package_search_paths(&lua, &cwd).expect("refresh");
        let pkg: mlua::Table = lua.globals().get("package").unwrap();
        let path: String = pkg.get("path").unwrap();
        let cpath: String = pkg.get("cpath").unwrap();

        assert!(path.contains("/tmp/fake-project/cook_modules/?.lua"));
        assert!(path.contains("/tmp/fake-project/cook_modules/?/init.lua"));
        assert!(path.contains("/tmp/fake-project/cook_modules/share/lua/5.4/?.lua"));
        assert!(path.contains("/tmp/fake-project/cook_modules/share/lua/5.4/?/init.lua"));

        assert!(cpath.contains("/tmp/fake-project/cook_modules/?."));
        assert!(cpath.contains("/tmp/fake-project/cook_modules/lib/lua/5.4/?."));
    }

    #[test]
    fn refresh_is_idempotent() {
        let lua = mlua::Lua::new();
        let cwd = PathBuf::from("/tmp/fake-project");
        refresh_package_search_paths(&lua, &cwd).expect("first");
        let pkg: mlua::Table = lua.globals().get("package").unwrap();
        let first_path: String = pkg.get("path").unwrap();
        let first_cpath: String = pkg.get("cpath").unwrap();
        refresh_package_search_paths(&lua, &cwd).expect("second");
        let second_path: String = pkg.get("path").unwrap();
        let second_cpath: String = pkg.get("cpath").unwrap();
        assert_eq!(first_path, second_path, "path must not grow on repeated refresh");
        assert_eq!(first_cpath, second_cpath, "cpath must not grow on repeated refresh");
    }
}
