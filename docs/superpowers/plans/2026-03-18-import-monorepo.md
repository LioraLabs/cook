# Import & Monorepo Support Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add `import` keyword to Cook so Cookfiles can pull in recipes from other directories, enabling monorepo builds with a single unified DAG.

**Architecture:** Engine-first approach. The parser learns a new `ImportDecl` token. The engine recursively loads imported Cookfiles, creates per-import Runtimes (each with its own working_dir, env, cache), and merges all recipes into one namespaced DAG. The scheduler gains per-node execution context instead of a pool-wide working directory.

**Tech Stack:** Rust, mlua (Lua VM), clap (CLI), notify (file watcher)

**Spec:** `docs/superpowers/specs/2026-03-18-import-monorepo-design.md`

---

## File Structure

### New Files
- `src/engine/workspace.rs` — Recursive Cookfile loading, import resolution, namespace merging, cycle detection, deduplication
- `tests/import_integration.rs` — Integration tests for import feature using temp directory Cookfile structures

### Modified Files
- `src/parser/lexer.rs` — Add `Token::ImportDecl` variant and parsing logic
- `src/parser/ast.rs` — Add `ImportDecl` struct and `imports` field to `Cookfile`
- `src/parser/mod.rs` — Handle `ImportDecl` tokens during parsing
- `src/engine/mod.rs` — Declare `workspace` submodule, re-export
- `src/engine/pipeline.rs` — Refactor `cmd_run` and `cmd_test` to use workspace loading
- `src/engine/commands.rs` — Make `cmd_menu` and `cmd_serve` import-aware
- `src/scheduler/pool.rs` — Change `WorkerPool::spawn` to accept per-item working dirs; change `WorkItem` to carry `working_dir` and `env_vars`
- `src/scheduler/executor.rs` — Pass per-node context through to pool
- `src/analyzer/graph.rs` — Extend `topological_sort` to handle namespaced recipe names
- `src/analyzer/mod.rs` — Update `resolve_execution_order` for multi-Cookfile graphs
- `src/watcher/mod.rs` — Accept multiple watch directories for imported Cookfiles

---

## Chunk 1: Parser — `import` keyword

### Task 1: Add `ImportDecl` to AST

**Files:**
- Modify: `src/parser/ast.rs:1-13`

- [ ] **Step 1: Add `ImportDecl` struct and update `Cookfile`**

In `src/parser/ast.rs`, add the new struct and field:

```rust
// After UseStatement (line 5)
pub struct ImportDecl {
    pub name: String,
    pub path: String,
    pub line: usize,
}
```

And add the `imports` field to `Cookfile`:

```rust
pub struct Cookfile {
    pub vars: Vec<(String, String)>,
    pub configs: std::collections::HashMap<String, Vec<(String, String)>>,
    pub recipes: Vec<Recipe>,
    pub uses: Vec<UseStatement>,
    pub imports: Vec<ImportDecl>,
}
```

- [ ] **Step 2: Fix all compile errors from new field**

Run: `cargo check 2>&1 | head -40`

The `Cookfile` struct is constructed in `src/parser/mod.rs:104`. Add `imports: vec![]` there. Also check `src/engine/commands.rs` (`cmd_init` creates a default Cookfile source string — no struct construction there, so it should be fine).

- [ ] **Step 3: Run tests to verify nothing broke**

Run: `cargo test --lib 2>&1 | tail -5`
Expected: All existing tests pass.

- [ ] **Step 4: Commit**

```bash
git add src/parser/ast.rs src/parser/mod.rs
git commit -m "feat(parser): add ImportDecl to AST"
```

---

### Task 2: Add `Token::ImportDecl` to lexer

**Files:**
- Modify: `src/parser/lexer.rs:4-15` (Token enum), `src/parser/lexer.rs:48-69` (keyword list), `src/parser/lexer.rs:71-163` (tokenize function)
- Test: `src/parser/lexer.rs:165+` (existing test module)

- [ ] **Step 1: Write failing tests for import tokenization**

Add to the `#[cfg(test)] mod tests` block in `src/parser/lexer.rs`:

```rust
#[test]
fn test_import_decl() {
    let tokens = tokenize("import backend ./services/backend").unwrap();
    assert_eq!(tokens.len(), 1);
    assert_eq!(
        tokens[0].value,
        Token::ImportDecl {
            name: "backend".to_string(),
            path: "./services/backend".to_string(),
        }
    );
}

#[test]
fn test_import_decl_relative_parent() {
    let tokens = tokenize("import proto ../../libs/proto").unwrap();
    assert_eq!(tokens.len(), 1);
    assert_eq!(
        tokens[0].value,
        Token::ImportDecl {
            name: "proto".to_string(),
            path: "../../libs/proto".to_string(),
        }
    );
}

#[test]
fn test_import_prefix_is_content() {
    // "important" should NOT match as an import
    let tokens = tokenize("important").unwrap();
    assert_eq!(tokens[0].value, Token::Content("important".to_string()));
}

#[test]
fn test_import_missing_path() {
    let result = tokenize("import backend");
    assert!(result.is_err());
}

#[test]
fn test_import_missing_name_and_path() {
    let result = tokenize("import");
    // bare "import" with nothing after should error or be content
    // Since "import" + space + args is the pattern, bare "import" is Content
    let tokens = result.unwrap();
    assert_eq!(tokens[0].value, Token::Content("import".to_string()));
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --lib lexer::tests 2>&1 | tail -10`
Expected: FAIL — `Token::ImportDecl` variant does not exist.

- [ ] **Step 3: Add `ImportDecl` variant to `Token` enum**

In `src/parser/lexer.rs`, add to the `Token` enum (after `UseDecl`):

```rust
ImportDecl { name: String, path: String },
```

- [ ] **Step 4: Add `"import"` to keyword blocklist in `try_parse_var_decl`**

In `src/parser/lexer.rs:54`, update the `matches!` check:

```rust
if matches!(name, "recipe" | "config" | "end" | "ingredients" | "cook" | "plate" | "using" | "use" | "test" | "import") {
```

- [ ] **Step 5: Add import parsing branch to `tokenize`**

In `src/parser/lexer.rs`, add a new branch in the `tokenize` function. Insert it **before** the `try_parse_var_decl` fallback (before line 146). The pattern follows `use` but parses two bare tokens instead of one quoted string:

```rust
} else if trimmed.starts_with("import")
    && trimmed.len() > 6
    && (trimmed.as_bytes()[6] == b' ' || trimmed.as_bytes()[6] == b'\t')
{
    let rest = trimmed["import".len()..].trim();
    // Parse: import <name> <path>
    // Both are bare tokens separated by whitespace
    let space_pos = rest.find(|c: char| c == ' ' || c == '\t');
    match space_pos {
        Some(pos) => {
            let name = rest[..pos].to_string();
            let path = rest[pos..].trim().to_string();
            if path.is_empty() {
                return Err(LexError::MissingRecipeName { line: line_num });
            }
            Token::ImportDecl { name, path }
        }
        None => {
            return Err(LexError::MissingRecipeName { line: line_num });
        }
    }
```

- [ ] **Step 6: Run tests to verify they pass**

Run: `cargo test --lib lexer::tests 2>&1 | tail -10`
Expected: All tests pass including new import tests.

- [ ] **Step 7: Commit**

```bash
git add src/parser/lexer.rs
git commit -m "feat(lexer): add import keyword tokenization"
```

---

### Task 3: Handle `ImportDecl` tokens in parser

**Files:**
- Modify: `src/parser/mod.rs:20-105`
- Test: `src/parser/mod.rs:107+` (test module)

- [ ] **Step 1: Write failing test for import parsing**

In `src/parser/mod.rs`, the test module is in a separate file at `src/parser/tests.rs` (declared as `mod tests` at line 108). If it exists, add tests there. Otherwise add inline. Check first:

Run: `ls src/parser/tests.rs 2>/dev/null && echo exists || echo inline`

Add a test (in whichever location):

```rust
#[test]
fn test_parse_import_decl() {
    let source = r#"
import backend ./services/backend
import frontend ./apps/frontend

recipe "all": "backend.build" "frontend.build"
end
"#;
    let cookfile = crate::parser::parse(source).unwrap();
    assert_eq!(cookfile.imports.len(), 2);
    assert_eq!(cookfile.imports[0].name, "backend");
    assert_eq!(cookfile.imports[0].path, "./services/backend");
    assert_eq!(cookfile.imports[1].name, "frontend");
    assert_eq!(cookfile.imports[1].path, "./apps/frontend");
}

#[test]
fn test_parse_import_after_recipe_fails() {
    let source = r#"
recipe "build"
end

import backend ./services/backend
"#;
    let result = crate::parser::parse(source);
    assert!(result.is_err());
}

#[test]
fn test_parse_duplicate_import_names_fails() {
    let source = r#"
import backend ./services/a
import backend ./services/b
"#;
    let result = crate::parser::parse(source);
    assert!(result.is_err());
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --lib parser::tests 2>&1 | tail -10`
Expected: FAIL — `ImportDecl` token not handled in `parse()`.

- [ ] **Step 3: Add `ImportDecl` handling to `parse()` function**

In `src/parser/mod.rs`, add a `mut imports = Vec::new()` alongside the other collections (line 27), and add a match arm for `Token::ImportDecl` inside the `while` loop. Place it after the `UseDecl` arm (after line 99):

```rust
Token::ImportDecl { name, path } => {
    if seen_recipe {
        return Err(ParseError::Parse {
            line: tok.line,
            message: "import declarations must appear before recipes".to_string(),
        });
    }
    // Check for duplicate import names
    if imports.iter().any(|i: &ast::ImportDecl| i.name == *name) {
        return Err(ParseError::Parse {
            line: tok.line,
            message: format!("duplicate import name '{name}' (already declared above)"),
        });
    }
    imports.push(ast::ImportDecl {
        name: name.clone(),
        path: path.clone(),
        line: tok.line,
    });
    pos += 1;
}
```

Update the return to include `imports`:

```rust
Ok(Cookfile { vars, configs, recipes, uses, imports })
```

- [ ] **Step 4: Run all tests**

Run: `cargo test --lib 2>&1 | tail -10`
Expected: All tests pass.

- [ ] **Step 5: Commit**

```bash
git add src/parser/mod.rs src/parser/tests.rs
git commit -m "feat(parser): handle import declarations in parse()"
```

---

## Chunk 2: Scheduler — Per-node execution context

### Task 4: Add execution context to `WorkItem`

**Files:**
- Modify: `src/scheduler/pool.rs:12-16` (`WorkItem` struct)
- Modify: `src/scheduler/pool.rs:66-93` (`WorkerPool::spawn`)
- Modify: `src/scheduler/pool.rs:251-292` (`execute_work_item`)
- Modify: `src/scheduler/pool.rs:294-364` (`execute_shell`)
- Modify: `src/scheduler/pool.rs:366-414` (`execute_lua_chunk`)
- Modify: `src/scheduler/pool.rs:416-547` (`execute_test`)
- Test: `src/scheduler/pool.rs` (existing tests if any), `src/scheduler/executor.rs:119+` (existing tests)

The key change: `WorkItem` carries its own `working_dir` and `env_vars` instead of the pool sharing one set. **Important:** Task 4 and Task 5 must be done together to maintain a working state — between them the system must always have valid working directories on WorkItems.

- [ ] **Step 1: Add `working_dir` and `env_vars` to `WorkItem`**

In `src/scheduler/pool.rs`, update `WorkItem`:

```rust
pub struct WorkItem {
    pub id: usize,
    pub payload: WorkPayload,
    pub recipe_name: String,
    pub working_dir: PathBuf,
    pub env_vars: HashMap<String, String>,
}
```

Add necessary imports at the top: `use std::path::PathBuf;` (check if already imported).

- [ ] **Step 2: Remove pool-wide `working_dir` and `env_vars` from `WorkerPool::spawn`**

Change the signature of `WorkerPool::spawn` from:

```rust
pub fn spawn(n: usize, working_dir: PathBuf, env_vars: HashMap<String, String>) -> (Self, mpsc::Receiver<WorkResult>)
```

to:

```rust
pub fn spawn(n: usize) -> (Self, mpsc::Receiver<WorkResult>)
```

Inside the worker thread loop, update `execute_work_item` calls to use `work.working_dir` and `work.env_vars` from each `WorkItem`:

In the worker loop (around line 154-162), change:

```rust
QueueItem::Work(work) => {
    *current_recipe.lock().unwrap() = work.recipe_name.clone();
    let result = execute_work_item(&lua, &work, &work.working_dir, &work.env_vars);
    let _ = tx.send(result);
}
```

Remove the `working_dir` and `env_vars` clones that were captured into the thread closure. The worker thread no longer needs these — each `WorkItem` carries its own.

- [ ] **Step 3: Update worker cook table registration**

The worker cook table (lines 172-209) currently uses captured `working_dir` and `env_vars`. These need to become per-item. The cook table's `cook.sh`, `cook.exec`, and `cook.env` closures currently capture the pool-wide values.

This is trickier because the Lua VM is created once per thread, but the working_dir changes per item. The current pattern uses `Arc<Mutex<String>>` for `current_recipe` (because the value must be `Send` for mlua closures) — use the same pattern for `working_dir` and `env_vars`:

Add to the worker thread setup (near line 135):

```rust
let current_working_dir: Arc<Mutex<PathBuf>> = Arc::new(Mutex::new(PathBuf::new()));
let current_env_vars: Arc<Mutex<HashMap<String, String>>> = Arc::new(Mutex::new(HashMap::new()));
```

Update the work dispatch to set these before executing:

```rust
QueueItem::Work(work) => {
    *current_recipe.lock().unwrap() = work.recipe_name.clone();
    *current_working_dir.lock().unwrap() = work.working_dir.clone();
    *current_env_vars.lock().unwrap() = work.env_vars.clone();
    let result = execute_work_item(&lua, &work, &work.working_dir, &work.env_vars);
    let _ = tx.send(result);
}
```

Update the cook table closures (`cook.sh`, `cook.exec`) to use `current_working_dir` and `current_env_vars` instead of the captured pool-wide values. Update `cook.env` table similarly. **Note:** These must use `Arc<Mutex<>>`, NOT `Rc<RefCell<>>` — worker threads require `Send`.

- [ ] **Step 4: Fix all call sites — `execute_dag` in `executor.rs`**

In `src/scheduler/executor.rs`, update `execute_dag`'s signature. Remove `working_dir: PathBuf` and `env_vars: HashMap<String, String>` parameters:

Change from:

```rust
pub fn execute_dag(
    dag: ExecutionDag,
    num_workers: usize,
    working_dir: PathBuf,
    env_vars: HashMap<String, String>,
    _quiet: bool,
    cache_manager: Option<Arc<ThreadSafeCacheManager>>,
    event_tx: Option<mpsc::Sender<ProgressEvent>>,
    mut test_outputs: Option<&mut Vec<TestOutput>>,
) -> Result<(), SchedulerError>
```

to:

```rust
pub fn execute_dag(
    dag: ExecutionDag,
    num_workers: usize,
    _quiet: bool,
    cache_manager: Option<Arc<ThreadSafeCacheManager>>,
    event_tx: Option<mpsc::Sender<ProgressEvent>>,
    mut test_outputs: Option<&mut Vec<TestOutput>>,
) -> Result<(), SchedulerError>
```

Update `WorkerPool::spawn` call (line ~119):

```rust
let (pool, rx) = WorkerPool::spawn(num_workers);
```

**Critical:** Where `WorkItem` is constructed in `executor.rs` (in `process_ready` and similar), populate `working_dir` and `env_vars` from the DAG node — NOT with empty defaults. The DAG node already carries these fields after Task 5. **Tasks 4 and 5 must be implemented together** to avoid a broken intermediate state where WorkItems have empty paths.

Also update executor's `record_completion` calls (around lines 335 and 409) to use the per-node `working_dir` from the DAG node instead of the removed pool-wide `working_dir`.

- [ ] **Step 5: Update `run_interactive_on_main`**

The `run_interactive_on_main` function in `executor.rs` (around line 315) also uses the pool-wide `working_dir`. Update it to use the DAG node's `working_dir` and `env_vars`.

- [ ] **Step 6: Fix call sites in `pipeline.rs`**

In `src/engine/pipeline.rs`, update both `cmd_run` (line 201) and `cmd_test` (line 375) calls to `execute_dag` — remove the `working_dir` and `env_vars` arguments.

- [ ] **Step 7: Run all tests**

Run: `cargo test 2>&1 | tail -10`
Expected: All tests pass. Behavior unchanged — every WorkItem gets the same working_dir (from RecipeUnits, which gets it from Runtime).

- [ ] **Step 8: Commit**

```bash
git add src/scheduler/pool.rs src/scheduler/executor.rs src/engine/pipeline.rs
git commit -m "refactor(scheduler): move working_dir and env_vars to per-WorkItem"
```

---

### Task 5: Add execution context to DAG nodes

**Files:**
- Modify: `src/scheduler/dag.rs:6-13` (`DagNode` struct)
- Modify: `src/scheduler/dag.rs:24-46` (`add_node`)
- Modify: `src/scheduler/dag.rs:48-63` (`add_presatisfied`)
- Modify: `src/scheduler/builder.rs:16-121` (`build_dag`)
- Modify: `src/scheduler/executor.rs` (read context from node when submitting to pool)

- [ ] **Step 1: Add `working_dir` and `env_vars` to `DagNode`**

In `src/scheduler/dag.rs`, add to `DagNode`:

```rust
pub struct DagNode {
    pub id: usize,
    pub payload: Option<WorkPayload>,
    pub recipe_name: String,
    pub cache_meta: Option<CacheMeta>,
    pub dependents: Vec<usize>,
    pub remaining_deps: AtomicUsize,
    pub working_dir: PathBuf,
    pub env_vars: HashMap<String, String>,
}
```

Add imports: `use std::path::PathBuf;` and `use std::collections::HashMap;`.

Update `add_node` and `add_presatisfied` to accept and store these fields.

- [ ] **Step 2: Update `build_dag` to pass context through**

In `src/scheduler/builder.rs`, update `build_dag` to accept a default `working_dir` and `env_vars` (or accept them per `RecipeUnits`). For now, add them to `RecipeUnits` is cleanest.

Update `RecipeUnits` in `src/contracts/mod.rs`:

```rust
pub struct RecipeUnits {
    pub recipe_name: String,
    pub deps: Vec<String>,
    pub units: Vec<CapturedUnit>,
    pub step_groups: Vec<Vec<usize>>,
    pub working_dir: PathBuf,
    pub env_vars: HashMap<String, String>,
}
```

Then `build_dag` passes `ru.working_dir.clone()` and `ru.env_vars.clone()` to each `dag.add_node` call.

- [ ] **Step 3: Update `RecipeUnits` construction in `Runtime::register_recipe`**

In `src/runtime/engine.rs:135`, add the working_dir and env_vars to the returned `RecipeUnits`:

```rust
Ok(RecipeUnits {
    recipe_name: recipe_name.to_string(),
    deps,
    units: cap.units.clone(),
    step_groups: cap.step_groups.clone(),
    working_dir: self.working_dir.clone(),
    env_vars: self.env_vars.clone(),
})
```

- [ ] **Step 4: Update executor to read context from DAG node**

In `src/scheduler/executor.rs`, wherever `WorkItem` is constructed (in `process_ready` and similar), set `working_dir` and `env_vars` from the DAG node:

```rust
let node = dag.node(node_id);
let work_item = WorkItem {
    id: node_id,
    payload: node.payload.clone().unwrap(),
    recipe_name: node.recipe_name.clone(),
    working_dir: node.working_dir.clone(),
    env_vars: node.env_vars.clone(),
};
```

- [ ] **Step 5: Fix all existing test constructors**

Update test code in `dag.rs` tests, `builder.rs` tests, and `executor.rs` tests to include the new fields. Use `PathBuf::from(".")` and `HashMap::new()` as defaults.

- [ ] **Step 6: Run all tests**

Run: `cargo test 2>&1 | tail -10`
Expected: All tests pass.

- [ ] **Step 7: Commit**

```bash
git add src/scheduler/dag.rs src/scheduler/builder.rs src/scheduler/executor.rs src/contracts/mod.rs src/runtime/engine.rs
git commit -m "feat(scheduler): per-node working_dir and env_vars in DAG"
```

---

## Chunk 3: Engine — Workspace loading and import resolution

### Task 6: Create workspace module for recursive Cookfile loading

**Files:**
- Create: `src/engine/workspace.rs`
- Modify: `src/engine/mod.rs`

This is the core of import support. The workspace module:
1. Takes a parsed root Cookfile + its directory
2. Recursively loads imported Cookfiles (parse + codegen each)
3. Detects cycles via canonical path set
4. Deduplicates same-path imports
5. Returns a flattened list of all Runtimes + namespaced recipe info

- [ ] **Step 1: Write the workspace types**

Create `src/engine/workspace.rs`:

```rust
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};

use crate::codegen;
use crate::parser;
use crate::parser::ast::Cookfile;
use crate::runtime::Runtime;

use super::error::CookError;

/// A loaded Cookfile with its parsed AST, generated Lua source, and directory.
pub struct LoadedCookfile {
    pub cookfile: Cookfile,
    pub lua_source: String,
    pub dir: PathBuf,
}

/// A resolved workspace: all Cookfiles loaded, imports resolved.
pub struct Workspace {
    /// Root Cookfile entry
    pub root: LoadedCookfile,
    /// All imported Cookfiles keyed by canonical path
    pub imports: BTreeMap<PathBuf, LoadedCookfile>,
    /// Namespace mapping: (parent_canonical_path, import_name) -> imported canonical path
    pub namespace_map: Vec<(PathBuf, String, PathBuf)>,
}
```

- [ ] **Step 2: Implement recursive loading with cycle detection**

Add to `workspace.rs`:

```rust
impl Workspace {
    pub fn load(cookfile_path: &Path, cli_sets: &[String]) -> Result<Self, CookError> {
        let cookfile_path = std::fs::canonicalize(cookfile_path)
            .map_err(|e| CookError::Other(format!("cannot resolve {}: {e}", cookfile_path.display())))?;
        let root_dir = cookfile_path.parent().unwrap_or(Path::new(".")).to_path_buf();

        let source = std::fs::read_to_string(&cookfile_path)
            .map_err(|e| CookError::Other(format!("cannot read {}: {e}", cookfile_path.display())))?;
        let cookfile = parser::parse(&source)
            .map_err(|e| CookError::ParseError(e.to_string()))?;
        let lua_source = codegen::generate(&cookfile);

        let mut imports = BTreeMap::new();
        let mut namespace_map = Vec::new();
        let mut visited = HashSet::new();
        visited.insert(std::fs::canonicalize(&root_dir)
            .unwrap_or_else(|_| root_dir.clone()));

        Self::load_imports(
            &cookfile,
            &root_dir,
            &mut imports,
            &mut namespace_map,
            &mut visited,
        )?;

        Ok(Workspace {
            root: LoadedCookfile { cookfile, lua_source, dir: root_dir },
            imports,
            namespace_map,
        })
    }

    fn load_imports(
        cookfile: &Cookfile,
        cookfile_dir: &Path,
        imports: &mut BTreeMap<PathBuf, LoadedCookfile>,
        namespace_map: &mut Vec<(PathBuf, String, PathBuf)>,
        visited: &mut HashSet<PathBuf>,
    ) -> Result<(), CookError> {
        let parent_canonical = std::fs::canonicalize(cookfile_dir)
            .unwrap_or_else(|_| cookfile_dir.to_path_buf());

        for import_decl in &cookfile.imports {
            let import_dir = cookfile_dir.join(&import_decl.path);
            if !import_dir.exists() {
                return Err(CookError::Other(format!(
                    "Import '{}': directory '{}' not found",
                    import_decl.name, import_decl.path
                )));
            }

            let canonical = std::fs::canonicalize(&import_dir)
                .map_err(|e| CookError::Other(format!(
                    "Import '{}': cannot resolve '{}': {e}",
                    import_decl.name, import_decl.path
                )))?;

            // Record namespace mapping
            namespace_map.push((
                parent_canonical.clone(),
                import_decl.name.clone(),
                canonical.clone(),
            ));

            // Cycle detection
            if !visited.insert(canonical.clone()) {
                // Already loaded — dedup, not a cycle (unless it's in our ancestor chain)
                if imports.contains_key(&canonical) {
                    continue; // Dedup: already loaded
                }
                return Err(CookError::Other(format!(
                    "Circular import detected involving '{}'",
                    import_decl.path
                )));
            }

            // Load the imported Cookfile
            let import_cookfile_path = import_dir.join("Cookfile");
            if !import_cookfile_path.exists() {
                return Err(CookError::Other(format!(
                    "Import '{}': no Cookfile found in '{}'",
                    import_decl.name, import_decl.path
                )));
            }

            let source = std::fs::read_to_string(&import_cookfile_path)
                .map_err(|e| CookError::Other(format!(
                    "Import '{}': cannot read Cookfile: {e}",
                    import_decl.name
                )))?;
            let sub_cookfile = parser::parse(&source)
                .map_err(|e| CookError::ParseError(format!(
                    "Import '{}': {e}", import_decl.name
                )))?;
            let sub_lua = codegen::generate(&sub_cookfile);

            // Recurse into this Cookfile's own imports
            Self::load_imports(
                &sub_cookfile,
                &canonical,
                imports,
                namespace_map,
                visited,
            )?;

            imports.insert(canonical, LoadedCookfile {
                cookfile: sub_cookfile,
                lua_source: sub_lua,
                dir: import_dir,
            });
        }

        Ok(())
    }
}
```

- [ ] **Step 3: Add namespace resolution helper**

Add to `workspace.rs`:

```rust
impl Workspace {
    /// Given a parent Cookfile's canonical dir and a dependency string like "backend.build",
    /// resolve it to (canonical_import_dir, recipe_name).
    /// Returns None if the dep doesn't contain a dot or the import name isn't found.
    pub fn resolve_namespaced_dep(
        &self,
        parent_dir: &Path,
        dep: &str,
    ) -> Option<(PathBuf, String)> {
        let dot_pos = dep.find('.')?;
        let import_name = &dep[..dot_pos];
        let recipe_name = &dep[dot_pos + 1..];

        let parent_canonical = std::fs::canonicalize(parent_dir)
            .unwrap_or_else(|_| parent_dir.to_path_buf());

        // Find the import in our namespace map
        for (parent, name, target) in &self.namespace_map {
            if parent == &parent_canonical && name == import_name {
                return Some((target.clone(), recipe_name.to_string()));
            }
        }
        None
    }
}
```

- [ ] **Step 4: Declare workspace module**

In `src/engine/mod.rs`, add:

```rust
pub mod workspace;
```

And re-export:

```rust
pub use workspace::Workspace;
```

- [ ] **Step 5: Write unit tests for workspace loading**

Add `#[cfg(test)] mod tests` to `src/engine/workspace.rs`. Use `tempfile::TempDir` to create test directory structures:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;
    use std::fs;

    #[test]
    fn test_no_imports_loads_root_only() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("Cookfile"), r#"
recipe "build"
end
"#).unwrap();
        let ws = Workspace::load(&dir.path().join("Cookfile"), &[]).unwrap();
        assert!(ws.imports.is_empty());
        assert!(ws.namespace_map.is_empty());
    }

    #[test]
    fn test_basic_import_loads_child() {
        let dir = TempDir::new().unwrap();
        fs::create_dir_all(dir.path().join("lib")).unwrap();
        fs::write(dir.path().join("lib/Cookfile"), r#"
recipe "build"
end
"#).unwrap();
        fs::write(dir.path().join("Cookfile"), r#"
import lib ./lib

recipe "all": "lib.build"
end
"#).unwrap();
        let ws = Workspace::load(&dir.path().join("Cookfile"), &[]).unwrap();
        assert_eq!(ws.imports.len(), 1);
        assert_eq!(ws.namespace_map.len(), 1);
    }

    #[test]
    fn test_cycle_detection() {
        let dir = TempDir::new().unwrap();
        fs::create_dir_all(dir.path().join("a")).unwrap();
        fs::create_dir_all(dir.path().join("b")).unwrap();
        fs::write(dir.path().join("a/Cookfile"), "import b ../b\nrecipe \"x\"\nend\n").unwrap();
        fs::write(dir.path().join("b/Cookfile"), "import a ../a\nrecipe \"y\"\nend\n").unwrap();
        fs::write(dir.path().join("Cookfile"), "import a ./a\nrecipe \"z\"\nend\n").unwrap();
        let result = Workspace::load(&dir.path().join("Cookfile"), &[]);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("ircular") || err.contains("already"), "expected cycle error: {err}");
    }

    #[test]
    fn test_dedup_same_path() {
        let dir = TempDir::new().unwrap();
        fs::create_dir_all(dir.path().join("shared")).unwrap();
        fs::create_dir_all(dir.path().join("a")).unwrap();
        fs::create_dir_all(dir.path().join("b")).unwrap();
        fs::write(dir.path().join("shared/Cookfile"), "recipe \"s\"\nend\n").unwrap();
        fs::write(dir.path().join("a/Cookfile"), "import shared ../shared\nrecipe \"a\"\nend\n").unwrap();
        fs::write(dir.path().join("b/Cookfile"), "import shared ../shared\nrecipe \"b\"\nend\n").unwrap();
        fs::write(dir.path().join("Cookfile"), "import a ./a\nimport b ./b\nrecipe \"all\"\nend\n").unwrap();
        let ws = Workspace::load(&dir.path().join("Cookfile"), &[]).unwrap();
        // shared should appear only once in imports despite being referenced by both a and b
        let shared_count = ws.imports.keys()
            .filter(|p| p.to_string_lossy().contains("shared"))
            .count();
        assert_eq!(shared_count, 1, "shared should be deduped");
    }

    #[test]
    fn test_missing_import_dir_errors() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("Cookfile"), "import missing ./nonexistent\nrecipe \"x\"\nend\n").unwrap();
        let result = Workspace::load(&dir.path().join("Cookfile"), &[]);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("not found"));
    }

    #[test]
    fn test_missing_cookfile_in_import_dir_errors() {
        let dir = TempDir::new().unwrap();
        fs::create_dir_all(dir.path().join("empty")).unwrap();
        fs::write(dir.path().join("Cookfile"), "import empty ./empty\nrecipe \"x\"\nend\n").unwrap();
        let result = Workspace::load(&dir.path().join("Cookfile"), &[]);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("no Cookfile"));
    }
}
```

- [ ] **Step 6: Run tests**

Run: `cargo test --lib engine::workspace 2>&1 | tail -10`
Expected: All pass.

- [ ] **Step 7: Commit**

```bash
git add src/engine/workspace.rs src/engine/mod.rs
git commit -m "feat(engine): add workspace module for recursive Cookfile loading"
```

---

### Task 7: Extend analyzer for namespaced recipes

**Files:**
- Modify: `src/analyzer/graph.rs`
- Modify: `src/analyzer/mod.rs`

The analyzer needs to build a topological sort across multiple Cookfiles' recipes with namespace prefixes.

- [ ] **Step 1: Write failing test for namespaced topological sort**

Add to `src/analyzer/graph.rs` tests:

```rust
#[test]
fn test_namespaced_deps() {
    let mut recipes = HashMap::new();
    recipes.insert("all".to_string(), RecipeInfo {
        ingredients: vec![],
        serves: vec![],
        requires: vec!["backend.build".to_string()],
    });
    recipes.insert("backend.build".to_string(), RecipeInfo {
        ingredients: vec![],
        serves: vec![],
        requires: vec!["backend.proto.generate".to_string()],
    });
    recipes.insert("backend.proto.generate".to_string(), RecipeInfo {
        ingredients: vec![],
        serves: vec![],
        requires: vec![],
    });
    let order = topological_sort(&recipes, "all").unwrap();
    assert_eq!(order, vec![
        "backend.proto.generate".to_string(),
        "backend.build".to_string(),
        "all".to_string(),
    ]);
}
```

- [ ] **Step 2: Run test to verify it passes**

The existing `topological_sort` already treats recipe names as opaque strings and follows `requires` edges. Namespaced names like `"backend.build"` should work without changes to the sort itself — the dot is just part of the string.

Run: `cargo test --lib analyzer 2>&1 | tail -10`
Expected: PASS — the existing algorithm handles this naturally.

- [ ] **Step 3: Add a workspace-level recipe info builder**

In `src/analyzer/mod.rs`, add a new function that builds a unified `RecipeInfo` map from a `Workspace`:

```rust
use crate::engine::workspace::Workspace;
use std::collections::HashMap;
use std::path::Path;

/// Build a unified recipe info map from all Cookfiles in a workspace.
/// Recipe names are prefixed with their namespace (e.g., "backend.build").
/// Root recipes have no prefix.
pub fn build_workspace_recipe_info(workspace: &Workspace) -> HashMap<String, RecipeInfo> {
    let mut all = HashMap::new();

    // Root recipes (no prefix)
    let root_info = build_recipe_info(&workspace.root.cookfile);
    for (name, mut info) in root_info {
        // Resolve namespaced deps
        info.requires = info.requires.iter().map(|dep| {
            resolve_dep_namespace(workspace, &workspace.root.dir, dep)
        }).collect();
        all.insert(name, info);
    }

    // Imported recipes (prefixed)
    for (canonical_path, loaded) in &workspace.imports {
        let prefix = find_full_prefix(workspace, canonical_path);
        let sub_info = build_recipe_info(&loaded.cookfile);
        for (name, mut info) in sub_info {
            let namespaced = format!("{prefix}.{name}");
            // Resolve deps within this import's scope
            info.requires = info.requires.iter().map(|dep| {
                resolve_dep_namespace(workspace, &loaded.dir, dep)
            }).collect();
            all.insert(namespaced, info);
        }
    }

    all
}

/// Resolve a dependency name to its fully namespaced form.
fn resolve_dep_namespace(workspace: &Workspace, from_dir: &Path, dep: &str) -> String {
    if let Some((target_dir, recipe_name)) = workspace.resolve_namespaced_dep(from_dir, dep) {
        let prefix = find_full_prefix(workspace, &target_dir);
        format!("{prefix}.{recipe_name}")
    } else {
        // Local dep — prefix with this Cookfile's own namespace
        dep.to_string()
    }
}

/// Find the full dotted prefix for a canonical import path by walking
/// the namespace chain from root. For transitive imports (root -> backend -> proto),
/// proto's prefix from root's perspective is "backend.proto".
pub fn find_full_prefix(workspace: &Workspace, canonical_path: &Path) -> String {
    let root_canonical = std::fs::canonicalize(&workspace.root.dir)
        .unwrap_or_else(|_| workspace.root.dir.clone());

    // Build reverse map: child_canonical -> (parent_canonical, name)
    let mut parent_map: HashMap<PathBuf, (PathBuf, String)> = HashMap::new();
    for (parent, name, target) in &workspace.namespace_map {
        parent_map.insert(target.clone(), (parent.clone(), name.clone()));
    }

    // Walk from target back to root, collecting name segments
    let mut segments = Vec::new();
    let mut current = canonical_path.to_path_buf();
    loop {
        if current == root_canonical {
            break;
        }
        match parent_map.get(&current) {
            Some((parent, name)) => {
                segments.push(name.clone());
                current = parent.clone();
            }
            None => break, // orphan — shouldn't happen in a valid workspace
        }
    }

    segments.reverse();
    segments.join(".")
}
```

- [ ] **Step 4: Run all tests**

Run: `cargo test --lib 2>&1 | tail -10`
Expected: All pass.

- [ ] **Step 5: Commit**

```bash
git add src/analyzer/mod.rs src/analyzer/graph.rs
git commit -m "feat(analyzer): add workspace-level recipe info builder with namespacing"
```

---

### Task 8: Integrate workspace loading into `cmd_run`

**Files:**
- Modify: `src/engine/pipeline.rs`

This is where it all comes together. `cmd_run` uses the workspace to load all Cookfiles, build a unified recipe graph, and execute with per-recipe Runtimes.

- [ ] **Step 1: Add workspace-aware path to `cmd_run`**

In `src/engine/pipeline.rs`, refactor `cmd_run` to check if the parsed Cookfile has any imports. If it does, use the workspace path. If not, fall through to the existing single-Cookfile path (preserving backward compatibility).

After `read_and_parse` (line 115), add:

```rust
// If cookfile has imports, use workspace loading
if !cookfile.imports.is_empty() {
    return cmd_run_workspace(cli, recipe_name, config);
}
```

- [ ] **Step 2: Implement `cmd_run_workspace`**

Add a new function in `pipeline.rs`:

```rust
fn cmd_run_workspace(
    cli: &Cli,
    recipe_name: &str,
    config: Option<&str>,
) -> Result<(), CookError> {
    let workspace = crate::engine::workspace::Workspace::load(&cli.file, &cli.set)?;

    // Build env for root
    let dotenv_vars = load_env(&workspace.root.dir);
    let root_env = resolve_env(&workspace.root.cookfile, config, dotenv_vars, &cli.set)?;

    // Build unified recipe info and resolve execution order
    let all_recipes = crate::analyzer::build_workspace_recipe_info(&workspace);
    let order = crate::analyzer::graph::topological_sort(&all_recipes, recipe_name)
        .map_err(|e| match e {
            crate::analyzer::graph::GraphError::UnknownRecipe(name) => CookError::RecipeNotFound(name),
            crate::analyzer::graph::GraphError::CycleDetected(name) => {
                CookError::Other(format!("dependency cycle involving: {name}"))
            }
        })?;

    // Create Runtimes: one for root, one per import
    let mut runtimes: std::collections::BTreeMap<String, (Runtime, String)> = std::collections::BTreeMap::new();

    // Root runtime (empty prefix)
    let root_rt = Runtime::new(workspace.root.dir.clone(), root_env.clone());
    runtimes.insert(String::new(), (root_rt, workspace.root.lua_source.clone()));

    // Import runtimes (prefixed)
    for (canonical_path, loaded) in &workspace.imports {
        let prefix = crate::analyzer::find_full_prefix(&workspace, canonical_path);
        let import_env = resolve_env(&loaded.cookfile, config, std::collections::HashMap::new(), &cli.set)?;
        let rt = Runtime::new(loaded.dir.clone(), import_env);
        runtimes.insert(prefix, (rt, loaded.lua_source.clone()));
    }

    let num_jobs = cli.jobs.unwrap_or_else(||
        std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1)
    );

    // Set up progress renderer
    let color = resolve_color(cli);
    let (event_tx, event_rx) = std::sync::mpsc::channel::<ProgressEvent>();
    let render_thread = spawn_renderer_thread(event_rx, color);

    // Emit RecipeQueued for all recipes
    for name in &order {
        let _ = event_tx.send(ProgressEvent::RecipeQueued {
            name: name.clone(),
            total_nodes: 0,
        });
    }

    let mut run_result: Result<(), CookError> = Ok(());
    for name in &order {
        // Determine which runtime owns this recipe
        let (prefix, local_name) = split_recipe_name(name);
        let (rt, lua_source) = runtimes.get(&prefix)
            .ok_or_else(|| CookError::Other(format!("no runtime for recipe '{name}'")))?;

        let cache_dir = rt.working_dir().join(".cook").join("cache");
        let cache_manager = std::sync::Arc::new(crate::cache::ThreadSafeCacheManager::new(cache_dir));

        // Register recipe with its own runtime
        let units = rt.register_recipe(lua_source, &local_name, Some(event_tx.clone()))
            .map_err(|e| CookError::Other(format!("recipe '{name}': {e}")))?;

        let dag = crate::scheduler::builder::build_dag(vec![units]);
        if dag.is_empty() {
            continue;
        }

        let result = crate::scheduler::execute_dag(
            dag,
            num_jobs,
            cli.quiet,
            Some(cache_manager),
            Some(event_tx.clone()),
            None,
        );

        if let Err(e) = result {
            run_result = Err(CookError::Other(
                e.failures.first()
                    .map(|(_, _, msg)| msg.clone())
                    .unwrap_or_else(|| "unknown error".into())
            ));
            break;
        }
    }

    let _ = event_tx.send(ProgressEvent::Finished);
    drop(event_tx);
    let _ = render_thread.join();

    run_result
}

/// Split a namespaced recipe name into (prefix, local_name).
/// "backend.build" -> ("backend", "build")
/// "backend.proto.generate" -> ("backend.proto", "generate")
/// "build" -> ("", "build")
/// Uses rfind to split on the LAST dot — the prefix is the full
/// namespace path, the local name is always a single recipe name.
fn split_recipe_name(name: &str) -> (String, String) {
    if let Some(dot_pos) = name.rfind('.') {
        (name[..dot_pos].to_string(), name[dot_pos + 1..].to_string())
    } else {
        (String::new(), name.to_string())
    }
}
```

- [ ] **Step 3: Expose `working_dir` from Runtime**

Add a getter to `src/runtime/engine.rs`:

```rust
pub fn working_dir(&self) -> &PathBuf {
    &self.working_dir
}
```

- [ ] **Step 4: Run cargo check**

Run: `cargo check 2>&1 | tail -20`
Expected: Compiles. There may be issues to work through.

- [ ] **Step 5: Commit**

```bash
git add src/engine/pipeline.rs src/runtime/engine.rs
git commit -m "feat(engine): integrate workspace loading into cmd_run"
```

---

### Task 8.5: Make `cmd_test` import-aware and add "reaching through" validation

**Files:**
- Modify: `src/engine/pipeline.rs` (`cmd_test` function)
- Modify: `src/engine/workspace.rs` or `src/analyzer/mod.rs` (validation)

- [ ] **Step 1: Add `cmd_test_workspace` analogous to `cmd_run_workspace`**

In `src/engine/pipeline.rs`, add a workspace-aware test path. At the top of `cmd_test`, after parsing, check for imports:

```rust
if !cookfile.imports.is_empty() {
    return cmd_test_workspace(cli, filter, verbose, timeout_multiplier, wrapper, list);
}
```

Implement `cmd_test_workspace` following the same pattern as `cmd_run_workspace` but:
- Discover test recipes across all imported Cookfiles (any recipe with `Step::Test`)
- Namespace them with their prefix
- Build unified recipe graph and execute in topological order
- Collect `TestOutput` from all imported test recipes
- Use per-import Runtime for each recipe's working_dir

The structure mirrors `cmd_test` but uses the workspace loading path.

- [ ] **Step 2: Add "reaching through" validation**

In `src/analyzer/mod.rs`, inside `build_workspace_recipe_info`, add validation when resolving deps. If a dep contains multiple dots (e.g., `"backend.proto.generate"` from root), check whether only the first segment is a direct import. If the dep resolves through a nested import that isn't directly imported by the current Cookfile, error:

```rust
/// Validate that a dependency doesn't "reach through" a nested import.
/// E.g., root can reference "backend.build" (direct import) but NOT
/// "backend.proto.generate" (proto is backend's import, not root's).
fn validate_dep_not_reaching_through(
    workspace: &Workspace,
    from_dir: &Path,
    dep: &str,
) -> Result<(), String> {
    if let Some(dot_pos) = dep.find('.') {
        let import_name = &dep[..dot_pos];
        let remainder = &dep[dot_pos + 1..];

        // Check if this import exists as a direct import of from_dir
        let from_canonical = std::fs::canonicalize(from_dir)
            .unwrap_or_else(|_| from_dir.to_path_buf());
        let is_direct = workspace.namespace_map.iter()
            .any(|(parent, name, _)| parent == &from_canonical && name == import_name);

        if is_direct && remainder.contains('.') {
            // The remainder itself contains a dot — this is reaching through
            return Err(format!(
                "Cannot reference nested import '{}' through '{}'. \
                Only direct imports are visible. Add an import declaration \
                to use '{}' recipes directly.",
                &remainder[..remainder.find('.').unwrap()],
                import_name,
                &remainder[..remainder.find('.').unwrap()],
            ));
        }
    }
    Ok(())
}
```

Call this validation in `build_workspace_recipe_info` when processing each recipe's deps. Return `CookError` on failure.

- [ ] **Step 3: Write integration test for reaching-through error**

Add to `tests/import_integration.rs`:

```rust
#[test]
fn test_reaching_through_nested_import_fails() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();

    fs::create_dir_all(root.join("libs/proto")).unwrap();
    fs::write(root.join("libs/proto/Cookfile"), r#"
recipe "generate"
    echo "proto"
end
"#).unwrap();

    fs::create_dir_all(root.join("services/backend")).unwrap();
    fs::write(root.join("services/backend/Cookfile"), r#"
import proto ../../libs/proto

recipe "build": "proto.generate"
    echo "backend"
end
"#).unwrap();

    // Root tries to reach through: backend.proto.generate
    fs::write(root.join("Cookfile"), r#"
import backend ./services/backend

recipe "all": "backend.proto.generate"
    echo "root"
end
"#).unwrap();

    let output = Command::new(cook_bin())
        .args(["-f", root.join("Cookfile").to_str().unwrap(), "all"])
        .output()
        .expect("failed to run cook");

    assert!(!output.status.success(), "should fail on reaching through");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("nested import") || stderr.contains("direct imports"),
        "error should mention nested import restriction: {stderr}");
}
```

- [ ] **Step 4: Run all tests**

Run: `cargo test 2>&1 | tail -10`
Expected: All pass.

- [ ] **Step 5: Commit**

```bash
git add src/engine/pipeline.rs src/analyzer/mod.rs
git commit -m "feat(engine): make cmd_test import-aware, add reaching-through validation"
```

---

## Chunk 4: Integration tests and remaining commands

### Task 9: Write integration tests

**Files:**
- Create: `tests/import_integration.rs`

- [ ] **Step 1: Write basic import integration test**

Create `tests/import_integration.rs`:

```rust
use std::fs;
use std::process::Command;
use tempfile::TempDir;

fn cook_bin() -> String {
    // Use cargo to find the binary
    let output = Command::new("cargo")
        .args(["build", "--quiet"])
        .output()
        .expect("failed to build");
    assert!(output.status.success(), "cargo build failed");
    format!("./target/debug/cook")
}

#[test]
fn test_basic_import() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();

    // Create sub-project
    fs::create_dir_all(root.join("libs/greeter")).unwrap();
    fs::write(root.join("libs/greeter/Cookfile"), r#"
recipe "greet"
    echo "hello from greeter"
end
"#).unwrap();

    // Create root Cookfile that imports it
    fs::write(root.join("Cookfile"), r#"
import greeter ./libs/greeter

recipe "all": "greeter.greet"
    echo "root done"
end
"#).unwrap();

    let output = Command::new(cook_bin())
        .args(["-f", root.join("Cookfile").to_str().unwrap(), "all"])
        .output()
        .expect("failed to run cook");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "cook failed: {stderr}");
}

#[test]
fn test_transitive_import() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();

    // Create proto lib
    fs::create_dir_all(root.join("libs/proto")).unwrap();
    fs::write(root.join("libs/proto/Cookfile"), r#"
recipe "generate"
    echo "generating types"
end
"#).unwrap();

    // Create backend that imports proto
    fs::create_dir_all(root.join("services/backend")).unwrap();
    fs::write(root.join("services/backend/Cookfile"), r#"
import proto ../../libs/proto

recipe "build": "proto.generate"
    echo "building backend"
end
"#).unwrap();

    // Root imports only backend — proto is transitive
    fs::write(root.join("Cookfile"), r#"
import backend ./services/backend

recipe "all": "backend.build"
    echo "all done"
end
"#).unwrap();

    let output = Command::new(cook_bin())
        .args(["-f", root.join("Cookfile").to_str().unwrap(), "all"])
        .output()
        .expect("failed to run cook");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "cook failed: {stderr}");
}

#[test]
fn test_circular_import_detected() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();

    fs::create_dir_all(root.join("a")).unwrap();
    fs::create_dir_all(root.join("b")).unwrap();

    fs::write(root.join("a/Cookfile"), r#"
import b ../b

recipe "build"
    echo "a"
end
"#).unwrap();

    fs::write(root.join("b/Cookfile"), r#"
import a ../a

recipe "build"
    echo "b"
end
"#).unwrap();

    fs::write(root.join("Cookfile"), r#"
import a ./a

recipe "all": "a.build"
    echo "root"
end
"#).unwrap();

    let output = Command::new(cook_bin())
        .args(["-f", root.join("Cookfile").to_str().unwrap(), "all"])
        .output()
        .expect("failed to run cook");

    assert!(!output.status.success(), "should fail on circular import");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("ircular") || stderr.contains("already"),
        "error should mention circular import: {stderr}");
}

#[test]
fn test_dedup_same_path() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();

    // Shared lib
    fs::create_dir_all(root.join("libs/shared")).unwrap();
    fs::write(root.join("libs/shared/Cookfile"), r#"
recipe "build"
    echo "shared built"
end
"#).unwrap();

    // Two consumers
    fs::create_dir_all(root.join("a")).unwrap();
    fs::write(root.join("a/Cookfile"), r#"
import shared ../libs/shared

recipe "build": "shared.build"
    echo "a built"
end
"#).unwrap();

    fs::create_dir_all(root.join("b")).unwrap();
    fs::write(root.join("b/Cookfile"), r#"
import shared ../libs/shared

recipe "build": "shared.build"
    echo "b built"
end
"#).unwrap();

    fs::write(root.join("Cookfile"), r#"
import a ./a
import b ./b

recipe "all": "a.build" "b.build"
    echo "all done"
end
"#).unwrap();

    let output = Command::new(cook_bin())
        .args(["-f", root.join("Cookfile").to_str().unwrap(), "all"])
        .output()
        .expect("failed to run cook");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "cook failed: {stderr}");
}
```

- [ ] **Step 2: Run integration tests**

Run: `cargo test --test import_integration 2>&1 | tail -20`
Expected: All pass.

- [ ] **Step 3: Commit**

```bash
git add tests/import_integration.rs
git commit -m "test: add integration tests for import feature"
```

---

### Task 10: Make `cmd_menu` import-aware

**Files:**
- Modify: `src/engine/commands.rs:6-26`

- [ ] **Step 1: Update `cmd_menu` to show imported recipes**

In `src/engine/commands.rs`, after parsing the Cookfile, check for imports and load workspace if present. Show imported recipes with their namespace prefix.

```rust
pub fn cmd_menu(cli: &Cli) -> Result<(), CookError> {
    let (cookfile, _lua_source) = super::pipeline::read_and_parse(cli)?;

    // Show root recipes
    for recipe in &cookfile.recipes {
        println!("  {}", recipe.name);
    }

    // If imports exist, load workspace and show imported recipes
    if !cookfile.imports.is_empty() {
        let workspace = crate::engine::workspace::Workspace::load(&cli.file, &cli.set)?;
        for (canonical_path, loaded) in &workspace.imports {
            let prefix = crate::analyzer::find_full_prefix(&workspace, canonical_path);
            for recipe in &loaded.cookfile.recipes {
                println!("  {}.{}", prefix, recipe.name);
            }
        }
    }

    Ok(())
}
```

- [ ] **Step 2: Run cargo check**

Run: `cargo check 2>&1 | tail -10`
Expected: Compiles.

- [ ] **Step 3: Commit**

```bash
git add src/engine/commands.rs
git commit -m "feat(menu): show imported recipes with namespace prefix"
```

---

### Task 11: Make `cmd_serve` import-aware

**Files:**
- Modify: `src/engine/commands.rs:45-95`
- Modify: `src/watcher/mod.rs`

- [ ] **Step 1: Update watcher to accept multiple directories**

In `src/watcher/mod.rs`, update `CookWatcher` to hold multiple Cookfile paths:

```rust
pub struct CookWatcher {
    globs: Vec<String>,
    cookfile_paths: Vec<PathBuf>,
}
```

Update `new` and `watch` to iterate over all Cookfile paths.

- [ ] **Step 2: Update `cmd_serve` to collect imported Cookfile paths**

After workspace loading, collect all Cookfile paths and pass to watcher:

```rust
let mut cookfile_paths = vec![cli.file.clone()];
if !cookfile.imports.is_empty() {
    let workspace = Workspace::load(&cli.file, &cli.set)?;
    for (_, loaded) in &workspace.imports {
        cookfile_paths.push(loaded.dir.join("Cookfile"));
    }
}
```

- [ ] **Step 3: Run cargo check**

Run: `cargo check 2>&1 | tail -10`
Expected: Compiles.

- [ ] **Step 4: Commit**

```bash
git add src/watcher/mod.rs src/engine/commands.rs
git commit -m "feat(serve): watch imported Cookfile directories"
```

---

## Chunk 5: Example monorepo

### Task 12: Create example monorepo structure

**Files:**
- Create: `examples/monorepo/Cookfile`
- Create: `examples/monorepo/libs/proto/Cookfile`
- Create: `examples/monorepo/libs/proto/api.proto`
- Create: `examples/monorepo/services/backend/Cookfile`
- Create: `examples/monorepo/services/backend/cmd/server/main.go`
- Create: `examples/monorepo/services/backend/go.mod`
- Create: `examples/monorepo/apps/frontend/Cookfile`
- Create: `examples/monorepo/apps/frontend/package.json`
- Create: `examples/monorepo/apps/frontend/src/index.ts`

- [ ] **Step 1: Create directory structure**

```bash
mkdir -p examples/monorepo/libs/proto
mkdir -p examples/monorepo/services/backend/cmd/server
mkdir -p examples/monorepo/apps/frontend/src
```

- [ ] **Step 2: Create proto Cookfile and schema**

`examples/monorepo/libs/proto/Cookfile`:
```
recipe "generate"
    echo "Generating types from api.proto..."
    mkdir -p gen
    echo "// Generated types" > gen/types.txt
end

recipe "clean"
    rm -rf gen
end
```

`examples/monorepo/libs/proto/api.proto`:
```protobuf
syntax = "proto3";

package tasks;

message Task {
    string id = 1;
    string title = 2;
    bool completed = 3;
}

service TaskService {
    rpc ListTasks(ListTasksRequest) returns (ListTasksResponse);
    rpc CreateTask(CreateTaskRequest) returns (Task);
}

message ListTasksRequest {}
message ListTasksResponse { repeated Task tasks = 1; }
message CreateTaskRequest { string title = 1; }
```

- [ ] **Step 3: Create backend Cookfile and source**

`examples/monorepo/services/backend/Cookfile`:
```
import proto ../../libs/proto

recipe "build": "proto.generate"
    echo "Building Go backend..."
    echo "// Built" > server.out
end

recipe "dev": "proto.generate"
    echo "Starting backend dev server..."
end

recipe "test": "build"
    echo "Running backend tests..."
end

recipe "clean"
    rm -f server.out
end
```

`examples/monorepo/services/backend/go.mod`:
```
module example.com/monorepo/backend

go 1.21
```

`examples/monorepo/services/backend/cmd/server/main.go`:
```go
package main

import "fmt"

func main() {
    fmt.Println("Task service running on :8080")
}
```

- [ ] **Step 4: Create frontend Cookfile and source**

`examples/monorepo/apps/frontend/Cookfile`:
```
import proto ../../libs/proto

recipe "build": "proto.generate"
    echo "Building TypeScript frontend..."
    echo "// Built" > dist.out
end

recipe "dev": "proto.generate"
    echo "Starting frontend dev server..."
end

recipe "test": "build"
    echo "Running frontend tests..."
end

recipe "clean"
    rm -f dist.out
end
```

`examples/monorepo/apps/frontend/package.json`:
```json
{
  "name": "monorepo-frontend",
  "version": "0.1.0",
  "scripts": {
    "build": "echo 'tsc build'",
    "dev": "echo 'dev server'"
  }
}
```

`examples/monorepo/apps/frontend/src/index.ts`:
```typescript
console.log("Task Manager UI");
```

- [ ] **Step 5: Create root Cookfile**

`examples/monorepo/Cookfile`:
```
import backend ./services/backend
import frontend ./apps/frontend

recipe "build": "backend.build" "frontend.build"
    echo "Monorepo build complete"
end

recipe "dev": "backend.dev" "frontend.dev"
    echo "Dev servers running"
end

recipe "test": "backend.test" "frontend.test"
    echo "All tests passed"
end

recipe "clean": "backend.clean" "frontend.clean"
    echo "Cleaned"
end
```

- [ ] **Step 6: Test the example manually**

Run: `cargo run -- -f examples/monorepo/Cookfile build 2>&1`
Expected: Should build proto, then backend and frontend, then root "build complete" message.

- [ ] **Step 7: Commit**

```bash
git add examples/monorepo/
git commit -m "feat: add monorepo example with Go backend, TS frontend, shared proto"
```

---

### Task 13: Clean up existing examples

**Files:**
- Modify: `examples/` directory structure

- [ ] **Step 1: Reorganize existing examples**

Move existing examples into named subdirectories for clarity:

```bash
# Current structure:
#   examples/Cookfile (C project)
#   examples/cpp-project/ (C++ project)
#
# Rename to make the pattern clear:
mv examples/Cookfile examples/c-project/Cookfile
# (move associated source files too)
```

Verify each example still works after reorganization.

- [ ] **Step 2: Commit**

```bash
git add examples/
git commit -m "chore: reorganize examples into named subdirectories"
```
