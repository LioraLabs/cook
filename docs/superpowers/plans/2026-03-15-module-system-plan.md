# Module System Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a plugin system so language-specific Lua modules (e.g., `cook.cpp`) can be loaded via `use "name"` and manipulate Cook's DAG directly.

**Architecture:** New `use` statement in parser → codegen emits `cook.load_module()` call → runtime loads Lua file from `cook_modules/`, registers module-scoped cache and export/import APIs → module functions called from recipe Lua blocks use `cook.add_unit()` to create `CapturedUnit`s that feed into the existing scheduler pipeline unchanged.

**Tech Stack:** Rust, mlua (Lua 5.4), serde_json (new dependency for module cache + export/import serialization)

**Spec:** `docs/superpowers/specs/2026-03-15-module-system-design.md`

---

## File Structure

### New files

| File | Responsibility |
|---|---|
| `src/runtime/module_cache.rs` | Module-scoped persistent JSON cache (`cook.cache.*` API) |
| `src/runtime/module_loader.rs` | `cook.load_module()` — path resolution, dofile, init, source hash check |
| `src/runtime/platform_api.rs` | `cook.platform` table (os, arch) |
| `src/runtime/unit_api.rs` | `cook.add_unit()`, `cook.step_group()` capture-mode APIs |
| `src/runtime/export_api.rs` | `cook.export()` / `cook.import()` APIs + `ExportStore` type |

### Modified files

| File | Change |
|---|---|
| `Cargo.toml` | Add `serde_json` dependency |
| `src/parser/lexer.rs` | Add `"use"` to blocked keywords, add `Token::UseDecl` variant |
| `src/parser/ast.rs` | Add `UseStatement` struct, add `uses` field to `Cookfile` |
| `src/parser/mod.rs` | Handle `UseDecl` token at top level, populate `cookfile.uses` |
| `src/codegen/recipe.rs` | Emit `cook.load_module()` calls for each `use` before recipe definitions |
| `src/runtime/mod.rs` | Re-export new submodules |
| `src/runtime/engine.rs` | Pass `ExportStore` into API registration, register new APIs on the cook table |
| `src/runtime/capture.rs` | No structural changes — new APIs push to same `CaptureState.units` |
| `src/contracts/mod.rs` | No changes needed — `CapturedUnit`/`CacheMeta`/`DepKind` are sufficient |

---

## Chunk 1: Parser — `use` statement

### Task 1: Lexer — add `Token::UseDecl`

**Files:**
- Modify: `src/parser/lexer.rs:3-14` (Token enum)
- Modify: `src/parser/lexer.rs:47-68` (try_parse_var_decl)
- Modify: `src/parser/lexer.rs:70-146` (tokenize)
- Test: `src/parser/lexer.rs` (test module at bottom)

- [ ] **Step 1: Write failing tests for `use` lexing**

Add to the `mod tests` block at the bottom of `src/parser/lexer.rs`:

```rust
#[test]
fn test_use_decl() {
    let tokens = tokenize(r#"use "cpp""#).unwrap();
    assert_eq!(tokens.len(), 1);
    assert_eq!(
        tokens[0].value,
        Token::UseDecl {
            name: "cpp".to_string(),
        }
    );
}

#[test]
fn test_use_decl_not_var() {
    // "use" should not be parsed as a VarDecl
    let tokens = tokenize(r#"use "cpp""#).unwrap();
    assert!(!matches!(tokens[0].value, Token::VarDecl { .. }));
}

#[test]
fn test_use_prefix_is_content() {
    // "useful" should be Content, not UseDecl
    let tokens = tokenize("useful").unwrap();
    assert_eq!(tokens[0].value, Token::Content("useful".to_string()));
}

#[test]
fn test_use_missing_name() {
    let result = tokenize("use foo");
    assert!(result.is_err());
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --lib -p cook -- lexer::tests::test_use`
Expected: compilation error — `Token::UseDecl` doesn't exist yet

- [ ] **Step 3: Add `Token::UseDecl` variant and parsing**

In `src/parser/lexer.rs`:

1. Add variant to `Token` enum:
```rust
UseDecl { name: String },
```

2. Add `"use"` to the blocked keywords in `try_parse_var_decl` (line 53):
```rust
if matches!(name, "recipe" | "config" | "end" | "ingredients" | "cook" | "plate" | "using" | "use") {
```

3. In `tokenize()`, add a branch before the `try_parse_var_decl` fallback (after the config block, before the `!trimmed.starts_with('@')` branch). Pattern it like `config`:
```rust
} else if trimmed.starts_with("use")
    && trimmed.len() > 3
    && (trimmed.as_bytes()[3] == b' '
        || trimmed.as_bytes()[3] == b'\t'
        || trimmed.as_bytes()[3] == b'"')
{
    let rest = trimmed["use".len()..].trim();
    if !rest.starts_with('"') {
        return Err(LexError::MissingRecipeName { line: line_num });
    }
    let rest = &rest[1..];
    let end = rest
        .find('"')
        .ok_or(LexError::UnterminatedString { line: line_num })?;
    let name = rest[..end].to_string();
    Token::UseDecl { name }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --lib -p cook -- lexer::tests`
Expected: all pass including new `test_use_*` tests

- [ ] **Step 5: Commit**

```bash
git add src/parser/lexer.rs
git commit -m "feat(parser): add Token::UseDecl for use statements"
```

### Task 2: AST — add `UseStatement` and `Cookfile.uses`

**Files:**
- Modify: `src/parser/ast.rs:1-6` (Cookfile struct)
- Test: `src/parser/ast.rs` (test module)

- [ ] **Step 1: Write failing test**

Add to `mod tests` in `src/parser/ast.rs`:

```rust
#[test]
fn test_cookfile_with_uses() {
    let cookfile = Cookfile {
        vars: vec![],
        configs: std::collections::HashMap::new(),
        recipes: vec![],
        uses: vec![
            UseStatement { module_name: "cpp".to_string(), line: 1 },
        ],
    };
    assert_eq!(cookfile.uses.len(), 1);
    assert_eq!(cookfile.uses[0].module_name, "cpp");
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --lib -p cook -- ast::tests::test_cookfile_with_uses`
Expected: compilation error — `UseStatement` and `uses` field don't exist

- [ ] **Step 3: Add `UseStatement` and update `Cookfile`**

In `src/parser/ast.rs`:

Add the struct before `Cookfile`:
```rust
#[derive(Debug, Clone, PartialEq)]
pub struct UseStatement {
    pub module_name: String,
    pub line: usize,
}
```

Add `uses` field to `Cookfile`:
```rust
pub struct Cookfile {
    pub vars: Vec<(String, String)>,
    pub configs: std::collections::HashMap<String, Vec<(String, String)>>,
    pub recipes: Vec<Recipe>,
    pub uses: Vec<UseStatement>,
}
```

- [ ] **Step 4: Fix compilation errors**

The `Cookfile` struct is constructed in several places. Add `uses: vec![]` to each:
- `src/parser/mod.rs:90` — the `parse()` function return
- `src/codegen/tests.rs:8` — `make_cookfile()` helper
- `src/parser/ast.rs:131` — existing `test_cookfile_with_vars_and_configs` test

Run: `cargo test --lib -p cook`
Expected: all existing tests pass, new test passes

- [ ] **Step 5: Commit**

```bash
git add src/parser/ast.rs src/parser/mod.rs src/codegen/tests.rs
git commit -m "feat(parser): add UseStatement AST node and Cookfile.uses field"
```

### Task 3: Parser — handle `use` at top level

**Files:**
- Modify: `src/parser/mod.rs:20-91` (parse function)
- Test: `src/parser/tests.rs`

- [ ] **Step 1: Write failing tests**

Add to `src/parser/tests.rs`:

```rust
#[test]
fn test_parse_use_statement() {
    let source = r#"use "cpp"

recipe "build"
    echo hello
end
"#;
    let cookfile = crate::parser::parse(source).unwrap();
    assert_eq!(cookfile.uses.len(), 1);
    assert_eq!(cookfile.uses[0].module_name, "cpp");
    assert_eq!(cookfile.uses[0].line, 1);
    assert_eq!(cookfile.recipes.len(), 1);
}

#[test]
fn test_parse_multiple_use_statements() {
    let source = r#"use "cpp"
use "proto"

recipe "build"
    echo hello
end
"#;
    let cookfile = crate::parser::parse(source).unwrap();
    assert_eq!(cookfile.uses.len(), 2);
    assert_eq!(cookfile.uses[0].module_name, "cpp");
    assert_eq!(cookfile.uses[1].module_name, "proto");
}

#[test]
fn test_parse_use_with_vars_and_configs() {
    let source = r#"use "cpp"
CC "gcc"

config "debug"
    CFLAGS "-g"
end

recipe "build"
    echo hello
end
"#;
    let cookfile = crate::parser::parse(source).unwrap();
    assert_eq!(cookfile.uses.len(), 1);
    assert_eq!(cookfile.vars.len(), 1);
    assert_eq!(cookfile.configs.len(), 1);
}

#[test]
fn test_parse_use_after_recipe_fails() {
    let source = r#"recipe "build"
    echo hello
end

use "cpp"
"#;
    let result = crate::parser::parse(source);
    assert!(result.is_err());
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --lib -p cook -- parser::tests::test_parse_use`
Expected: FAIL — `UseDecl` not handled in `parse()`

- [ ] **Step 3: Handle `Token::UseDecl` in the parser**

In `src/parser/mod.rs`, inside the `parse()` function's match block (around line 31), add a branch for `UseDecl` alongside `VarDecl` and `ConfigHeader`:

```rust
Token::UseDecl { name } => {
    if seen_recipe {
        return Err(ParseError::Parse {
            line: tok.line,
            message: "use statements must appear before recipes".to_string(),
        });
    }
    uses.push(ast::UseStatement {
        module_name: name.clone(),
        line: tok.line,
    });
    pos += 1;
}
```

Also add `let mut uses = Vec::new();` near the other `let mut` declarations, and include `uses` in the returned `Cookfile`.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --lib -p cook -- parser::tests`
Expected: all pass

- [ ] **Step 5: Commit**

```bash
git add src/parser/mod.rs src/parser/tests.rs
git commit -m "feat(parser): handle use statements at top level"
```

### Task 4: Codegen — emit `cook.load_module()` calls

**Files:**
- Modify: `src/codegen/recipe.rs:7-69` (generate function)
- Test: `src/codegen/tests.rs`

- [ ] **Step 1: Write failing tests**

Add to `src/codegen/tests.rs`:

```rust
#[test]
fn test_use_generates_load_module() {
    let cookfile = Cookfile {
        vars: vec![],
        configs: std::collections::HashMap::new(),
        recipes: vec![make_recipe("build", vec![], vec![], vec![
            Step::Shell { command: "echo hi".to_string(), line: 2, interactive: false },
        ])],
        uses: vec![
            crate::parser::ast::UseStatement { module_name: "cpp".to_string(), line: 1 },
        ],
    };
    let output = generate(&cookfile);
    assert!(output.contains(r#"local cpp = cook.load_module("cpp")"#));
    // load_module should appear before recipe definitions
    let load_pos = output.find("cook.load_module").unwrap();
    let recipe_pos = output.find("cook.recipe").unwrap();
    assert!(load_pos < recipe_pos);
}

#[test]
fn test_multiple_uses_generate_in_order() {
    let cookfile = Cookfile {
        vars: vec![],
        configs: std::collections::HashMap::new(),
        recipes: vec![],
        uses: vec![
            crate::parser::ast::UseStatement { module_name: "cpp".to_string(), line: 1 },
            crate::parser::ast::UseStatement { module_name: "proto".to_string(), line: 2 },
        ],
    };
    let output = generate(&cookfile);
    let cpp_pos = output.find(r#"local cpp = cook.load_module("cpp")"#).unwrap();
    let proto_pos = output.find(r#"local proto = cook.load_module("proto")"#).unwrap();
    assert!(cpp_pos < proto_pos);
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --lib -p cook -- codegen::tests::test_use`
Expected: FAIL — `generate()` doesn't emit load_module

- [ ] **Step 3: Emit `cook.load_module()` in codegen**

In `src/codegen/recipe.rs`, update the `generate()` function. After the header comment line, emit load_module calls for each use statement before the recipe loop:

```rust
pub fn generate(cookfile: &Cookfile) -> String {
    let mut out = String::from("-- Generated by Cook\n");

    // Emit module loading for use statements
    // Module names must be valid Lua identifiers (alphanumeric + underscore, not starting with digit)
    for use_stmt in &cookfile.uses {
        let lua_name = use_stmt.module_name.replace('-', "_");
        out.push_str(&format!(
            "local {} = cook.load_module(\"{}\")\n",
            lua_name,
            escape_lua_string(&use_stmt.module_name),
        ));
    }
    if !cookfile.uses.is_empty() {
        out.push('\n');
    }

    for recipe in &cookfile.recipes {
        // ... existing recipe generation ...
    }

    out
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --lib -p cook -- codegen::tests`
Expected: all pass

- [ ] **Step 5: Commit**

```bash
git add src/codegen/recipe.rs src/codegen/tests.rs
git commit -m "feat(codegen): emit cook.load_module() for use statements"
```

---

## Chunk 2: Runtime — `cook.platform` and `cook.cache`

### Task 5: Add `serde_json` dependency

**Files:**
- Modify: `Cargo.toml`

- [ ] **Step 1: Add dependency**

Add `serde_json = "1"` to `[dependencies]` in `Cargo.toml`.

- [ ] **Step 2: Verify it compiles**

Run: `cargo check`
Expected: OK

- [ ] **Step 3: Commit**

```bash
git add Cargo.toml
git commit -m "chore: add serde_json dependency for module system"
```

### Task 6: `cook.platform` API

**Files:**
- Create: `src/runtime/platform_api.rs`
- Modify: `src/runtime/mod.rs` (add `pub mod platform_api;`)
- Test: `src/runtime/platform_api.rs` (inline tests)

- [ ] **Step 1: Write the test**

Create `src/runtime/platform_api.rs` with:

```rust
use mlua::prelude::*;

/// Register `cook.platform` table with `os` and `arch` fields.
/// Must be called after the `cook` table exists in globals.
pub fn register_platform_api(lua: &Lua) -> LuaResult<()> {
    todo!()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_platform_os_is_set() {
        let lua = Lua::new();
        lua.globals().set("cook", lua.create_table().unwrap()).unwrap();
        register_platform_api(&lua).unwrap();
        let os: String = lua.load("return cook.platform.os").eval().unwrap();
        assert_eq!(os, std::env::consts::OS);
    }

    #[test]
    fn test_platform_arch_is_set() {
        let lua = Lua::new();
        lua.globals().set("cook", lua.create_table().unwrap()).unwrap();
        register_platform_api(&lua).unwrap();
        let arch: String = lua.load("return cook.platform.arch").eval().unwrap();
        assert_eq!(arch, std::env::consts::ARCH);
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --lib -p cook -- platform_api::tests`
Expected: FAIL — `todo!()` panics

- [ ] **Step 3: Implement `register_platform_api`**

Replace the `todo!()`:

```rust
pub fn register_platform_api(lua: &Lua) -> LuaResult<()> {
    let cook: LuaTable = lua.globals().get("cook")?;
    let platform = lua.create_table()?;
    platform.set("os", std::env::consts::OS)?;
    platform.set("arch", std::env::consts::ARCH)?;
    cook.set("platform", platform)?;
    Ok(())
}
```

- [ ] **Step 4: Add module declaration**

In `src/runtime/mod.rs`, add: `pub mod platform_api;`

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test --lib -p cook -- platform_api::tests`
Expected: PASS

- [ ] **Step 6: Commit**

```bash
git add src/runtime/platform_api.rs src/runtime/mod.rs
git commit -m "feat(runtime): add cook.platform API (os, arch)"
```

### Task 7: Module cache (`cook.cache.*`)

**Files:**
- Create: `src/runtime/module_cache.rs`
- Modify: `src/runtime/mod.rs` (add module declaration)

- [ ] **Step 1: Write tests**

Create `src/runtime/module_cache.rs` with the full type and tests:

```rust
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Persistent JSON key-value cache scoped to a single module.
pub struct ModuleCache {
    module_name: String,
    cache_dir: PathBuf,
    data: BTreeMap<String, serde_json::Value>,
    dirty: bool,
}

impl ModuleCache {
    pub fn load(cache_dir: &Path, module_name: &str, source_hash: u64) -> Self {
        todo!()
    }

    pub fn get(&self, key: &str) -> Option<&serde_json::Value> {
        todo!()
    }

    pub fn set(&mut self, key: &str, value: serde_json::Value) {
        todo!()
    }

    pub fn invalidate(&mut self, key: &str) {
        todo!()
    }

    pub fn clear(&mut self) {
        todo!()
    }

    pub fn flush(&self) -> std::io::Result<()> {
        todo!()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn hash(s: &str) -> u64 {
        crate::contracts::hash_str(s)
    }

    #[test]
    fn test_set_and_get() {
        let dir = TempDir::new().unwrap();
        let mut cache = ModuleCache::load(dir.path(), "cpp", hash("v1"));
        cache.set("compiler", serde_json::json!({"cc": "gcc"}));
        let val = cache.get("compiler").unwrap();
        assert_eq!(val, &serde_json::json!({"cc": "gcc"}));
    }

    #[test]
    fn test_flush_and_reload() {
        let dir = TempDir::new().unwrap();
        let mut cache = ModuleCache::load(dir.path(), "cpp", hash("v1"));
        cache.set("compiler", serde_json::json!("gcc"));
        cache.flush().unwrap();

        let cache2 = ModuleCache::load(dir.path(), "cpp", hash("v1"));
        assert_eq!(cache2.get("compiler").unwrap(), &serde_json::json!("gcc"));
    }

    #[test]
    fn test_source_hash_change_invalidates() {
        let dir = TempDir::new().unwrap();
        let mut cache = ModuleCache::load(dir.path(), "cpp", hash("v1"));
        cache.set("compiler", serde_json::json!("gcc"));
        cache.flush().unwrap();

        // Load with different source hash — should be empty
        let cache2 = ModuleCache::load(dir.path(), "cpp", hash("v2"));
        assert!(cache2.get("compiler").is_none());
    }

    #[test]
    fn test_invalidate_key() {
        let dir = TempDir::new().unwrap();
        let mut cache = ModuleCache::load(dir.path(), "cpp", hash("v1"));
        cache.set("compiler", serde_json::json!("gcc"));
        cache.invalidate("compiler");
        assert!(cache.get("compiler").is_none());
    }

    #[test]
    fn test_clear() {
        let dir = TempDir::new().unwrap();
        let mut cache = ModuleCache::load(dir.path(), "cpp", hash("v1"));
        cache.set("a", serde_json::json!(1));
        cache.set("b", serde_json::json!(2));
        cache.clear();
        assert!(cache.get("a").is_none());
        assert!(cache.get("b").is_none());
    }

    #[test]
    fn test_modules_have_separate_caches() {
        let dir = TempDir::new().unwrap();
        let mut cpp = ModuleCache::load(dir.path(), "cpp", hash("v1"));
        let mut rust = ModuleCache::load(dir.path(), "rust", hash("v1"));
        cpp.set("compiler", serde_json::json!("gcc"));
        rust.set("compiler", serde_json::json!("rustc"));
        cpp.flush().unwrap();
        rust.flush().unwrap();

        let cpp2 = ModuleCache::load(dir.path(), "cpp", hash("v1"));
        let rust2 = ModuleCache::load(dir.path(), "rust", hash("v1"));
        assert_eq!(cpp2.get("compiler").unwrap(), &serde_json::json!("gcc"));
        assert_eq!(rust2.get("compiler").unwrap(), &serde_json::json!("rustc"));
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --lib -p cook -- module_cache::tests`
Expected: FAIL — `todo!()` panics

- [ ] **Step 3: Implement `ModuleCache`**

Replace all `todo!()` implementations:

```rust
impl ModuleCache {
    pub fn load(cache_dir: &Path, module_name: &str, source_hash: u64) -> Self {
        let path = cache_dir.join(format!("{}.json", module_name));
        let data = if path.exists() {
            match std::fs::read_to_string(&path) {
                Ok(contents) => {
                    match serde_json::from_str::<BTreeMap<String, serde_json::Value>>(&contents) {
                        Ok(mut map) => {
                            // Check source hash
                            let stored_hash = map
                                .get("_source_hash")
                                .and_then(|v| v.as_u64())
                                .unwrap_or(0);
                            if stored_hash != source_hash {
                                // Module source changed — wipe cache
                                map.clear();
                            }
                            map
                        }
                        Err(_) => BTreeMap::new(),
                    }
                }
                Err(_) => BTreeMap::new(),
            }
        } else {
            BTreeMap::new()
        };

        Self {
            module_name: module_name.to_string(),
            cache_dir: cache_dir.to_path_buf(),
            data,
            dirty: false,
        }
    }

    pub fn get(&self, key: &str) -> Option<&serde_json::Value> {
        if key == "_source_hash" {
            return None;
        }
        self.data.get(key)
    }

    pub fn set(&mut self, key: &str, value: serde_json::Value) {
        self.data.insert(key.to_string(), value);
        self.dirty = true;
    }

    pub fn invalidate(&mut self, key: &str) {
        self.data.remove(key);
        self.dirty = true;
    }

    pub fn clear(&mut self) {
        self.data.clear();
        self.dirty = true;
    }

    pub fn flush(&self) -> std::io::Result<()> {
        if !self.dirty {
            return Ok(());
        }
        std::fs::create_dir_all(&self.cache_dir)?;
        let path = self.cache_dir.join(format!("{}.json", self.module_name));
        let json = serde_json::to_string_pretty(&self.data)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
        std::fs::write(path, json)
    }
}
```

Note: `_source_hash` is stored by `cook.load_module` after loading. `ModuleCache` itself doesn't set it — the loader does via `cache.set("_source_hash", hash)` after creating the cache.

Actually — the `load` method should handle the hash comparison, and the loader should write the new hash after invalidation. Let's keep the hash comparison in `load()` and add a method:

```rust
pub fn set_source_hash(&mut self, hash: u64) {
    self.data.insert(
        "_source_hash".to_string(),
        serde_json::Value::Number(serde_json::Number::from(hash)),
    );
    self.dirty = true;
}
```

- [ ] **Step 4: Add module declaration**

In `src/runtime/mod.rs`, add: `pub mod module_cache;`

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test --lib -p cook -- module_cache::tests`
Expected: all PASS

- [ ] **Step 6: Commit**

```bash
git add src/runtime/module_cache.rs src/runtime/mod.rs
git commit -m "feat(runtime): add ModuleCache for persistent module-scoped caching"
```

---

## Chunk 3: Runtime — `cook.export/import`, `cook.add_unit`, `cook.step_group`

### Task 8: Export/Import store

**Files:**
- Create: `src/runtime/export_api.rs`
- Modify: `src/runtime/mod.rs`

- [ ] **Step 1: Write tests**

Create `src/runtime/export_api.rs`:

```rust
use std::collections::BTreeMap;
use std::cell::RefCell;
use std::rc::Rc;

/// Stores exported metadata from modules, keyed by name.
/// Shared across per-recipe VMs via `Runtime`.
#[derive(Debug, Default, Clone)]
pub struct ExportStore {
    data: BTreeMap<String, serde_json::Value>,
}

pub type SharedExportStore = Rc<RefCell<ExportStore>>;

impl ExportStore {
    pub fn new() -> Self {
        Self { data: BTreeMap::new() }
    }

    pub fn export(&mut self, name: &str, value: serde_json::Value) {
        self.data.insert(name.to_string(), value);
    }

    pub fn import(&self, name: &str) -> Option<&serde_json::Value> {
        self.data.get(name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_export_and_import() {
        let mut store = ExportStore::new();
        store.export("mylib", serde_json::json!({
            "includes": ["include/"],
            "lib_path": "build/libmylib.a"
        }));
        let val = store.import("mylib").unwrap();
        assert_eq!(val["lib_path"], "build/libmylib.a");
    }

    #[test]
    fn test_import_missing_returns_none() {
        let store = ExportStore::new();
        assert!(store.import("noexist").is_none());
    }

    #[test]
    fn test_export_overwrites() {
        let mut store = ExportStore::new();
        store.export("lib", serde_json::json!("v1"));
        store.export("lib", serde_json::json!("v2"));
        assert_eq!(store.import("lib").unwrap(), &serde_json::json!("v2"));
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --lib -p cook -- export_api::tests`
Expected: may need module declaration first

- [ ] **Step 3: Add module declaration**

In `src/runtime/mod.rs`, add: `pub mod export_api;`

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --lib -p cook -- export_api::tests`
Expected: all PASS

- [ ] **Step 5: Commit**

```bash
git add src/runtime/export_api.rs src/runtime/mod.rs
git commit -m "feat(runtime): add ExportStore for cross-recipe module metadata"
```

### Task 9: `cook.add_unit()` and `cook.step_group()` APIs

**Files:**
- Create: `src/runtime/unit_api.rs`
- Modify: `src/runtime/mod.rs`

- [ ] **Step 1: Write tests**

Create `src/runtime/unit_api.rs`:

```rust
use mlua::prelude::*;
use std::cell::RefCell;
use std::rc::Rc;

use crate::contracts::{
    CacheMeta, CapturedUnit, DepKind, SharedCaptureState, WorkPayload, hash_str,
};

/// Register `cook.add_unit(table)` and `cook.step_group(fn)` on the cook table.
/// Must be called after the `cook` table exists in globals.
pub fn register_unit_api(
    lua: &Lua,
    capture_state: SharedCaptureState,
    recipe_name: &str,
) -> LuaResult<()> {
    todo!()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contracts::CaptureState;

    fn setup() -> (Lua, SharedCaptureState) {
        let lua = Lua::new();
        let cook = lua.create_table().unwrap();
        lua.globals().set("cook", cook).unwrap();
        let cs = Rc::new(RefCell::new(CaptureState::new()));
        register_unit_api(&lua, cs.clone(), "test_recipe").unwrap();
        (lua, cs)
    }

    #[test]
    fn test_add_unit_basic() {
        let (lua, cs) = setup();
        lua.load(r#"
            cook.add_unit({
                inputs = { "src/main.cpp" },
                output = "build/main.o",
                command = "g++ -c src/main.cpp -o build/main.o",
            })
        "#).exec().unwrap();

        let state = cs.borrow();
        assert_eq!(state.units.len(), 1);
        match &state.units[0].payload {
            WorkPayload::Shell { cmd, .. } => {
                assert_eq!(cmd, "g++ -c src/main.cpp -o build/main.o");
            }
            other => panic!("expected Shell, got: {:?}", other),
        }
        assert!(state.units[0].cache_meta.is_some());
        let meta = state.units[0].cache_meta.as_ref().unwrap();
        assert_eq!(meta.input_paths, vec!["src/main.cpp"]);
        assert_eq!(meta.output_path, Some("build/main.o".to_string()));
        assert_eq!(meta.recipe_name, "test_recipe");
    }

    #[test]
    fn test_add_unit_no_cache() {
        let (lua, cs) = setup();
        lua.load(r#"
            cook.add_unit({
                command = "echo hello",
                cache = false,
            })
        "#).exec().unwrap();

        let state = cs.borrow();
        assert_eq!(state.units.len(), 1);
        assert!(state.units[0].cache_meta.is_none());
    }

    #[test]
    fn test_add_unit_sequential_by_default() {
        let (lua, cs) = setup();
        lua.load(r#"
            cook.add_unit({ command = "echo 1" })
            cook.add_unit({ command = "echo 2" })
        "#).exec().unwrap();

        let state = cs.borrow();
        assert!(matches!(state.units[0].dep_kind, DepKind::Sequential));
        assert!(matches!(state.units[1].dep_kind, DepKind::Sequential));
    }

    #[test]
    fn test_step_group_makes_parallel() {
        let (lua, cs) = setup();
        lua.load(r#"
            cook.step_group(function()
                cook.add_unit({ command = "echo 1" })
                cook.add_unit({ command = "echo 2" })
            end)
        "#).exec().unwrap();

        let state = cs.borrow();
        assert_eq!(state.units.len(), 2);
        assert!(matches!(state.units[0].dep_kind, DepKind::StepGroup(0)));
        assert!(matches!(state.units[1].dep_kind, DepKind::StepGroup(0)));
        assert_eq!(state.step_groups.len(), 1);
        assert_eq!(state.step_groups[0].len(), 2);
    }

    #[test]
    fn test_step_group_sequential_after() {
        let (lua, cs) = setup();
        lua.load(r#"
            cook.step_group(function()
                cook.add_unit({ command = "echo parallel" })
            end)
            cook.add_unit({ command = "echo sequential" })
        "#).exec().unwrap();

        let state = cs.borrow();
        assert!(matches!(state.units[0].dep_kind, DepKind::StepGroup(0)));
        assert!(matches!(state.units[1].dep_kind, DepKind::Sequential));
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --lib -p cook -- unit_api::tests`
Expected: FAIL — `todo!()` panics

- [ ] **Step 3: Implement `register_unit_api`**

```rust
pub fn register_unit_api(
    lua: &Lua,
    capture_state: SharedCaptureState,
    recipe_name: &str,
) -> LuaResult<()> {
    let cook: LuaTable = lua.globals().get("cook")?;

    // cook.add_unit(table)
    let cs = capture_state.clone();
    let rname = recipe_name.to_string();
    let add_unit_fn = lua.create_function(move |_, tbl: LuaTable| {
        let command: String = tbl.get::<String>("command")
            .unwrap_or_default();
        let cache_enabled: bool = tbl.get::<bool>("cache")
            .unwrap_or(true);

        let inputs: Vec<String> = match tbl.get::<LuaTable>("inputs") {
            Ok(t) => t.sequence_values::<String>()
                .filter_map(Result::ok)
                .collect(),
            Err(_) => vec![],
        };

        let output: Option<String> = tbl.get::<String>("output").ok();

        let cache_meta = if cache_enabled {
            let cache_key = if let Some(ref out) = output {
                out.clone()
            } else {
                format!("{}@{:x}", inputs.first().map(|s| s.as_str()).unwrap_or(""), hash_str(&command))
            };
            Some(CacheMeta {
                recipe_name: rname.clone(),
                cache_key,
                input_paths: inputs,
                output_path: output,
                command_hash: hash_str(&command),
            })
        } else {
            None
        };

        let mut state = cs.borrow_mut();
        let dep_kind = if let Some(group_idx) = state.current_group {
            DepKind::StepGroup(group_idx)
        } else {
            DepKind::Sequential
        };
        let unit_idx = state.units.len();
        state.units.push(CapturedUnit {
            payload: WorkPayload::Shell { cmd: command, line: 0 },
            cache_meta,
            dep_kind: dep_kind.clone(),
        });
        if let DepKind::StepGroup(gi) = &dep_kind {
            state.step_groups[*gi].push(unit_idx);
        }

        Ok(())
    })?;
    cook.set("add_unit", add_unit_fn)?;

    // cook.step_group(fn)
    let cs2 = capture_state.clone();
    let step_group_fn = lua.create_function(move |_, func: LuaFunction| {
        {
            let mut state = cs2.borrow_mut();
            let group_idx = state.step_groups.len();
            state.step_groups.push(Vec::new());
            state.current_group = Some(group_idx);
        }
        let result = func.call::<()>(());
        {
            let mut state = cs2.borrow_mut();
            state.current_group = None;
        }
        result
    })?;
    cook.set("step_group", step_group_fn)?;

    Ok(())
}
```

- [ ] **Step 4: Add module declaration**

In `src/runtime/mod.rs`, add: `pub mod unit_api;`

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test --lib -p cook -- unit_api::tests`
Expected: all PASS

- [ ] **Step 6: Commit**

```bash
git add src/runtime/unit_api.rs src/runtime/mod.rs
git commit -m "feat(runtime): add cook.add_unit() and cook.step_group() APIs"
```

---

## Chunk 4: Runtime — `cook.load_module()` and integration

### Task 10: Module loader (`cook.load_module`)

**Files:**
- Create: `src/runtime/module_loader.rs`
- Modify: `src/runtime/mod.rs`

- [ ] **Step 1: Write tests**

Create `src/runtime/module_loader.rs`:

```rust
use mlua::prelude::*;
use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use crate::contracts::hash_str;
use super::module_cache::ModuleCache;

/// State tracking which module is currently being loaded (for cache scoping).
pub struct ModuleLoaderState {
    pub working_dir: PathBuf,
    pub cache_dir: PathBuf,
    pub current_module: Option<String>,
    pub caches: std::collections::HashMap<String, ModuleCache>,
}

pub type SharedModuleLoaderState = Rc<RefCell<ModuleLoaderState>>;

impl ModuleLoaderState {
    pub fn new(working_dir: PathBuf) -> Self {
        let cache_dir = working_dir.join(".cook").join("cache");
        Self {
            working_dir,
            cache_dir,
            current_module: None,
            caches: std::collections::HashMap::new(),
        }
    }

    pub fn flush_all(&self) {
        for cache in self.caches.values() {
            let _ = cache.flush();
        }
    }
}

/// Register `cook.load_module(name)` on the cook table.
pub fn register_module_loader(
    lua: &Lua,
    state: SharedModuleLoaderState,
) -> LuaResult<()> {
    todo!()
}

/// Register `cook.cache.*` APIs scoped to the current module.
pub fn register_cache_api(
    lua: &Lua,
    state: SharedModuleLoaderState,
) -> LuaResult<()> {
    todo!()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn setup_with_module(module_name: &str, module_code: &str) -> (Lua, TempDir, SharedModuleLoaderState) {
        let dir = TempDir::new().unwrap();
        let modules_dir = dir.path().join("cook_modules");
        std::fs::create_dir_all(&modules_dir).unwrap();
        std::fs::write(
            modules_dir.join(format!("{}.lua", module_name)),
            module_code,
        ).unwrap();

        let lua = Lua::new();
        let cook = lua.create_table().unwrap();
        lua.globals().set("cook", cook).unwrap();

        let state = Rc::new(RefCell::new(ModuleLoaderState::new(dir.path().to_path_buf())));
        register_module_loader(&lua, state.clone()).unwrap();
        register_cache_api(&lua, state.clone()).unwrap();

        (lua, dir, state)
    }

    #[test]
    fn test_load_module_returns_table() {
        let (lua, _dir, _state) = setup_with_module("test_mod", r#"
            local m = {}
            m.value = 42
            return m
        "#);

        let result: i32 = lua.load(r#"
            local m = cook.load_module("test_mod")
            return m.value
        "#).eval().unwrap();
        assert_eq!(result, 42);
    }

    #[test]
    fn test_load_module_calls_init() {
        let (lua, _dir, _state) = setup_with_module("test_mod", r#"
            local m = {}
            m.initialized = false
            function m.init()
                m.initialized = true
            end
            return m
        "#);

        let result: bool = lua.load(r#"
            local m = cook.load_module("test_mod")
            return m.initialized
        "#).eval().unwrap();
        assert!(result);
    }

    #[test]
    fn test_load_module_not_found() {
        let dir = TempDir::new().unwrap();
        let lua = Lua::new();
        let cook = lua.create_table().unwrap();
        lua.globals().set("cook", cook).unwrap();
        let state = Rc::new(RefCell::new(ModuleLoaderState::new(dir.path().to_path_buf())));
        register_module_loader(&lua, state).unwrap();

        let result = lua.load(r#"cook.load_module("nonexistent")"#).exec();
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("not found"), "error should mention not found: {}", err);
    }

    #[test]
    fn test_load_module_init_lua() {
        let dir = TempDir::new().unwrap();
        let modules_dir = dir.path().join("cook_modules").join("mymod");
        std::fs::create_dir_all(&modules_dir).unwrap();
        std::fs::write(
            modules_dir.join("init.lua"),
            "local m = {} m.from_init = true return m",
        ).unwrap();

        let lua = Lua::new();
        let cook = lua.create_table().unwrap();
        lua.globals().set("cook", cook).unwrap();
        let state = Rc::new(RefCell::new(ModuleLoaderState::new(dir.path().to_path_buf())));
        register_module_loader(&lua, state).unwrap();

        let result: bool = lua.load(r#"
            local m = cook.load_module("mymod")
            return m.from_init
        "#).eval().unwrap();
        assert!(result);
    }

    #[test]
    fn test_cache_api_in_module() {
        let (lua, _dir, state) = setup_with_module("test_mod", r#"
            local m = {}
            function m.init()
                cook.cache.set("greeting", "hello")
            end
            function m.get_greeting()
                return cook.cache.get("greeting")
            end
            return m
        "#);

        let result: String = lua.load(r#"
            local m = cook.load_module("test_mod")
            return m.get_greeting()
        "#).eval().unwrap();
        assert_eq!(result, "hello");

        // Flush and verify persistence
        state.borrow().flush_all();
        let cache_file = _dir.path().join(".cook/cache/test_mod.json");
        assert!(cache_file.exists());
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --lib -p cook -- module_loader::tests`
Expected: FAIL — `todo!()` panics

- [ ] **Step 3: Implement `register_module_loader`**

```rust
pub fn register_module_loader(
    lua: &Lua,
    state: SharedModuleLoaderState,
) -> LuaResult<()> {
    let cook: LuaTable = lua.globals().get("cook")?;

    let loader_state = state.clone();
    let load_fn = lua.create_function(move |lua, name: String| {
        let mut ls = loader_state.borrow_mut();

        // Resolve path: cook_modules/name.lua or cook_modules/name/init.lua
        let modules_dir = ls.working_dir.join("cook_modules");
        let file_path = modules_dir.join(format!("{}.lua", &name));
        let dir_path = modules_dir.join(&name).join("init.lua");

        let resolved = if file_path.exists() {
            file_path
        } else if dir_path.exists() {
            dir_path
        } else {
            return Err(mlua::Error::runtime(format!(
                "module '{}' not found: expected {} or {}",
                name,
                file_path.display(),
                dir_path.display(),
            )));
        };

        // Read and hash module source
        let source = std::fs::read_to_string(&resolved)
            .map_err(|e| mlua::Error::runtime(format!("failed to read module '{}': {}", name, e)))?;
        let source_hash = hash_str(&source);

        // Load or create cache for this module
        if !ls.caches.contains_key(&name) {
            let cache = ModuleCache::load(&ls.cache_dir, &name, source_hash);
            ls.caches.insert(name.clone(), cache);
        }
        // Set source hash in cache
        ls.caches.get_mut(&name).unwrap().set_source_hash(source_hash);

        // Set current module name (for cook.cache scoping)
        ls.current_module = Some(name.clone());
        drop(ls);

        // dofile — load and execute the module
        let module_table: LuaValue = lua.load(&source)
            .set_name(format!("cook_modules/{}", name))
            .eval()
            .map_err(|e| mlua::Error::runtime(format!("module '{}' load error: {}", name, e)))?;

        // Call init() if present
        if let LuaValue::Table(ref tbl) = module_table {
            if let Ok(init_fn) = tbl.get::<LuaFunction>("init") {
                init_fn.call::<()>(()).map_err(|e| {
                    mlua::Error::runtime(format!("module '{}' init() failed: {}", name, e))
                })?;
            }
        }

        // Flush cache after init
        let mut ls = loader_state.borrow_mut();
        if let Some(cache) = ls.caches.get(&name) {
            let _ = cache.flush();
        }
        ls.current_module = None;

        Ok(module_table)
    })?;

    cook.set("load_module", load_fn)?;
    Ok(())
}
```

- [ ] **Step 4: Implement `register_cache_api`**

```rust
pub fn register_cache_api(
    lua: &Lua,
    state: SharedModuleLoaderState,
) -> LuaResult<()> {
    let cook: LuaTable = lua.globals().get("cook")?;
    let cache_table = lua.create_table()?;

    // cook.cache.get(key)
    let s = state.clone();
    let get_fn = lua.create_function(move |lua, key: String| {
        let ls = s.borrow();
        let module = ls.current_module.as_deref()
            .ok_or_else(|| mlua::Error::runtime("cook.cache.get: no module context"))?;
        let val = ls.caches.get(module)
            .and_then(|c| c.get(&key))
            .cloned();
        match val {
            Some(v) => json_to_lua_value(lua, v),
            None => Ok(LuaValue::Nil),
        }
    })?;
    cache_table.set("get", get_fn)?;

    // cook.cache.set(key, value)
    let s2 = state.clone();
    let set_fn = lua.create_function(move |_, (key, value): (String, LuaValue)| {
        let mut ls = s2.borrow_mut();
        let module = ls.current_module.clone()
            .ok_or_else(|| mlua::Error::runtime("cook.cache.set: no module context"))?;
        let json_val = lua_value_to_json(value);
        ls.caches.get_mut(&module).unwrap().set(&key, json_val);
        Ok(())
    })?;
    cache_table.set("set", set_fn)?;

    // cook.cache.invalidate(key)
    let s3 = state.clone();
    let inv_fn = lua.create_function(move |_, key: String| {
        let mut ls = s3.borrow_mut();
        let module = ls.current_module.clone()
            .ok_or_else(|| mlua::Error::runtime("cook.cache.invalidate: no module context"))?;
        ls.caches.get_mut(&module).unwrap().invalidate(&key);
        Ok(())
    })?;
    cache_table.set("invalidate", inv_fn)?;

    // cook.cache.clear()
    let s4 = state.clone();
    let clear_fn = lua.create_function(move |_, ()| {
        let mut ls = s4.borrow_mut();
        let module = ls.current_module.clone()
            .ok_or_else(|| mlua::Error::runtime("cook.cache.clear: no module context"))?;
        ls.caches.get_mut(&module).unwrap().clear();
        Ok(())
    })?;
    cache_table.set("clear", clear_fn)?;

    cook.set("cache", cache_table)?;
    Ok(())
}

/// Convert serde_json::Value to mlua LuaValue. Needs `&Lua` to create strings and tables.
pub(crate) fn json_to_lua_value(lua: &Lua, val: serde_json::Value) -> LuaResult<LuaValue> {
    match val {
        serde_json::Value::Null => Ok(LuaValue::Nil),
        serde_json::Value::Bool(b) => Ok(LuaValue::Boolean(b)),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Ok(LuaValue::Integer(i))
            } else {
                Ok(LuaValue::Number(n.as_f64().unwrap_or(0.0)))
            }
        }
        serde_json::Value::String(s) => Ok(LuaValue::String(lua.create_string(&s)?)),
        serde_json::Value::Array(arr) => {
            let tbl = lua.create_table()?;
            for (i, v) in arr.into_iter().enumerate() {
                tbl.set(i + 1, json_to_lua_value(lua, v)?)?;
            }
            Ok(LuaValue::Table(tbl))
        }
        serde_json::Value::Object(map) => {
            let tbl = lua.create_table()?;
            for (k, v) in map {
                tbl.set(k, json_to_lua_value(lua, v)?)?;
            }
            Ok(LuaValue::Table(tbl))
        }
    }
}

pub(crate) fn lua_value_to_json(val: LuaValue) -> serde_json::Value {
    match val {
        LuaValue::Nil => serde_json::Value::Null,
        LuaValue::Boolean(b) => serde_json::json!(b),
        LuaValue::Integer(i) => serde_json::json!(i),
        LuaValue::Number(n) => serde_json::json!(n),
        LuaValue::String(s) => serde_json::json!(s.to_string_lossy()),
        LuaValue::Table(t) => {
            // Try as array first, fall back to object
            let mut arr = Vec::new();
            let mut is_array = true;
            for pair in t.clone().pairs::<LuaValue, LuaValue>() {
                if let Ok((k, v)) = pair {
                    if let LuaValue::Integer(_) = k {
                        arr.push(lua_value_to_json(v));
                    } else {
                        is_array = false;
                        break;
                    }
                }
            }
            if is_array && !arr.is_empty() {
                serde_json::Value::Array(arr)
            } else {
                let mut map = serde_json::Map::new();
                for pair in t.pairs::<String, LuaValue>() {
                    if let Ok((k, v)) = pair {
                        map.insert(k, lua_value_to_json(v));
                    }
                }
                serde_json::Value::Object(map)
            }
        }
        _ => serde_json::Value::Null,
    }
}
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test --lib -p cook -- module_loader::tests`
Expected: all PASS

- [ ] **Step 6: Commit**

```bash
git add src/runtime/module_loader.rs src/runtime/mod.rs
git commit -m "feat(runtime): add cook.load_module() and cook.cache.* APIs"
```

### Task 11: Wire export/import into Lua

**Files:**
- Modify: `src/runtime/export_api.rs` — add Lua registration functions

- [ ] **Step 1: Write tests**

Add to `src/runtime/export_api.rs`:

```rust
/// Register `cook.export(name, table)` and `cook.import(name)` on the cook table.
pub fn register_export_api(
    lua: &Lua,
    store: SharedExportStore,
) -> LuaResult<()> {
    todo!()
}

#[cfg(test)]
mod lua_tests {
    use super::*;

    fn setup() -> (Lua, SharedExportStore) {
        let lua = Lua::new();
        let cook = lua.create_table().unwrap();
        lua.globals().set("cook", cook).unwrap();
        let store = Rc::new(RefCell::new(ExportStore::new()));
        register_export_api(&lua, store.clone()).unwrap();
        (lua, store)
    }

    #[test]
    fn test_export_and_import_lua() {
        let (lua, _store) = setup();
        lua.load(r#"
            cook.export("mylib", { includes = { "include/" }, lib_path = "build/libmylib.a" })
        "#).exec().unwrap();

        let result: String = lua.load(r#"
            local info = cook.import("mylib")
            return info.lib_path
        "#).eval().unwrap();
        assert_eq!(result, "build/libmylib.a");
    }

    #[test]
    fn test_import_missing_returns_nil() {
        let (lua, _store) = setup();
        let result: LuaValue = lua.load(r#"
            return cook.import("nonexistent")
        "#).eval().unwrap();
        assert!(matches!(result, LuaValue::Nil));
    }

    #[test]
    fn test_export_survives_across_store_borrows() {
        let (lua, store) = setup();
        lua.load(r#"
            cook.export("lib", { path = "build/lib.a" })
        "#).exec().unwrap();

        // Simulate a second recipe's VM reading from the same store
        let lua2 = Lua::new();
        let cook2 = lua2.create_table().unwrap();
        lua2.globals().set("cook", cook2).unwrap();
        register_export_api(&lua2, store.clone()).unwrap();

        let result: String = lua2.load(r#"
            local info = cook.import("lib")
            return info.path
        "#).eval().unwrap();
        assert_eq!(result, "build/lib.a");
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --lib -p cook -- export_api::lua_tests`
Expected: FAIL — `todo!()` panics

- [ ] **Step 3: Implement `register_export_api`**

```rust
use mlua::prelude::*;

pub fn register_export_api(
    lua: &Lua,
    store: SharedExportStore,
) -> LuaResult<()> {
    let cook: LuaTable = lua.globals().get("cook")?;

    // cook.export(name, table)
    let s = store.clone();
    let export_fn = lua.create_function(move |_, (name, value): (String, LuaValue)| {
        let json_val = super::module_loader::lua_value_to_json(value);
        s.borrow_mut().export(&name, json_val);
        Ok(())
    })?;
    cook.set("export", export_fn)?;

    // cook.import(name)
    let s2 = store.clone();
    let import_fn = lua.create_function(move |lua, name: String| {
        let store = s2.borrow();
        match store.import(&name) {
            Some(val) => super::module_loader::json_to_lua_value(lua, val.clone()),
            None => Ok(LuaValue::Nil),
        }
    })?;
    cook.set("import", import_fn)?;

    Ok(())
}
```

Note: The `lua_value_to_json` and `json_to_lua_value` functions need to be `pub(crate)` in `module_loader.rs` so `export_api.rs` can use them.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --lib -p cook -- export_api`
Expected: all PASS

- [ ] **Step 5: Commit**

```bash
git add src/runtime/export_api.rs src/runtime/module_loader.rs
git commit -m "feat(runtime): add cook.export() and cook.import() Lua APIs"
```

---

## Chunk 5: Integration — wire everything into `Runtime`

### Task 12: Update `Runtime` and `register_recipe` to register all new APIs

**Files:**
- Modify: `src/runtime/engine.rs`
- Modify: `src/runtime/mod.rs`

- [ ] **Step 1: Write an integration test**

Add to `src/runtime/tests.rs`:

```rust
#[test]
fn test_module_loads_and_adds_units() {
    let dir = TempDir::new().unwrap();

    // Create a module
    let modules_dir = dir.path().join("cook_modules");
    fs::create_dir_all(&modules_dir).unwrap();
    fs::write(modules_dir.join("test_mod.lua"), r#"
        local m = {}
        function m.add_steps()
            cook.step_group(function()
                cook.add_unit({
                    inputs = { "a.txt" },
                    output = "b.txt",
                    command = "cp a.txt b.txt",
                })
                cook.add_unit({
                    inputs = { "c.txt" },
                    output = "d.txt",
                    command = "cp c.txt d.txt",
                })
            end)
        end
        return m
    "#).unwrap();

    let rt = make_runtime(dir.path());
    let lua_src = r#"
local test_mod = cook.load_module("test_mod")
cook.recipe("build", {}, function()
    test_mod.add_steps()
end)
"#;
    let result = rt.register_recipe(lua_src, "build").unwrap();
    assert_eq!(result.units.len(), 2);
    assert!(matches!(result.units[0].dep_kind, DepKind::StepGroup(0)));
    assert!(matches!(result.units[1].dep_kind, DepKind::StepGroup(0)));
    assert!(result.units[0].cache_meta.is_some());
}

#[test]
fn test_export_import_across_recipes() {
    let dir = TempDir::new().unwrap();

    let modules_dir = dir.path().join("cook_modules");
    fs::create_dir_all(&modules_dir).unwrap();
    fs::write(modules_dir.join("test_mod.lua"), r#"
        local m = {}
        function m.export_lib()
            cook.export("mylib", { lib_path = "build/libmylib.a" })
        end
        function m.use_lib()
            local info = cook.import("mylib")
            cook.add_unit({
                inputs = { info.lib_path },
                output = "bin/app",
                command = "gcc " .. info.lib_path .. " -o bin/app",
            })
        end
        return m
    "#).unwrap();

    let rt = make_runtime(dir.path());

    let lua_src = r#"
local test_mod = cook.load_module("test_mod")
cook.recipe("lib", {}, function()
    test_mod.export_lib()
end)
cook.recipe("app", {requires = {"lib"}}, function()
    test_mod.use_lib()
end)
"#;

    // Register "lib" first — it exports
    let lib_result = rt.register_recipe(lua_src, "lib").unwrap();
    assert_eq!(lib_result.units.len(), 0);

    // Register "app" — it imports
    let app_result = rt.register_recipe(lua_src, "app").unwrap();
    assert_eq!(app_result.units.len(), 1);
    match &app_result.units[0].payload {
        WorkPayload::Shell { cmd, .. } => {
            assert!(cmd.contains("libmylib.a"));
        }
        other => panic!("expected Shell, got: {:?}", other),
    }
}

#[test]
fn test_platform_available_in_module() {
    let dir = TempDir::new().unwrap();
    let modules_dir = dir.path().join("cook_modules");
    fs::create_dir_all(&modules_dir).unwrap();
    fs::write(modules_dir.join("test_mod.lua"), r#"
        local m = {}
        m.detected_os = cook.platform.os
        return m
    "#).unwrap();

    let rt = make_runtime(dir.path());
    let lua_src = r#"
local test_mod = cook.load_module("test_mod")
cook.recipe("check", {}, function()
    cook.add_unit({ command = test_mod.detected_os, cache = false })
end)
"#;
    let result = rt.register_recipe(lua_src, "check").unwrap();
    match &result.units[0].payload {
        WorkPayload::Shell { cmd, .. } => {
            assert_eq!(cmd, std::env::consts::OS);
        }
        other => panic!("expected Shell, got: {:?}", other),
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --lib -p cook -- runtime::tests::test_module`
Expected: FAIL — `cook.load_module` not registered in `register_recipe`

- [ ] **Step 3: Update `Runtime` to include `ExportStore` and register new APIs**

In `src/runtime/engine.rs`:

1. Add `ExportStore` field to `Runtime`:
```rust
use super::export_api::{ExportStore, SharedExportStore};
use super::module_loader::{ModuleLoaderState, SharedModuleLoaderState};

pub struct Runtime {
    working_dir: PathBuf,
    env_vars: HashMap<String, String>,
    quiet: bool,
    export_store: SharedExportStore,
}
```

2. Update `Runtime::new`:
```rust
pub fn new(working_dir: PathBuf, env_vars: HashMap<String, String>) -> Self {
    Self {
        working_dir,
        env_vars,
        quiet: false,
        export_store: Rc::new(RefCell::new(ExportStore::new())),
    }
}
```

3. In `register_recipe`, after existing API registration, add the new APIs:
```rust
// After register_path_api
super::platform_api::register_platform_api(&lua)?;

let module_state: SharedModuleLoaderState = Rc::new(RefCell::new(
    ModuleLoaderState::new(self.working_dir.clone())
));
super::module_loader::register_module_loader(&lua, module_state.clone())?;
super::module_loader::register_cache_api(&lua, module_state.clone())?;
super::unit_api::register_unit_api(&lua, capture_state.clone(), recipe_name)?;
super::export_api::register_export_api(&lua, self.export_store.clone())?;
```

Add `use std::rc::Rc;` to imports if not already present.

- [ ] **Step 4: Run all tests**

Run: `cargo test --lib -p cook`
Expected: all pass, including new integration tests

- [ ] **Step 5: Commit**

```bash
git add src/runtime/engine.rs src/runtime/tests.rs src/runtime/mod.rs
git commit -m "feat(runtime): wire module system APIs into Runtime.register_recipe"
```

### Task 13: End-to-end test with Cookfile parsing

**Files:**
- Modify: `tests/integration.rs`

- [ ] **Step 1: Write integration test**

Add to `tests/integration.rs` (or create a new test):

```rust
#[test]
fn test_use_statement_end_to_end() {
    let dir = tempfile::TempDir::new().unwrap();

    // Create module
    let modules_dir = dir.path().join("cook_modules");
    std::fs::create_dir_all(&modules_dir).unwrap();
    std::fs::write(modules_dir.join("hello.lua"), r#"
        local m = {}
        function m.greet()
            cook.add_unit({ command = "echo hello from module", cache = false })
        end
        return m
    "#).unwrap();

    // Create Cookfile
    let cookfile_content = r#"use "hello"

recipe "build"
    >{
        hello.greet()
    }
end
"#;
    std::fs::write(dir.path().join("Cookfile"), cookfile_content).unwrap();

    // Parse
    let cookfile = cook::parser::parse(cookfile_content).unwrap();
    assert_eq!(cookfile.uses.len(), 1);
    assert_eq!(cookfile.uses[0].module_name, "hello");

    // Codegen
    let lua_source = cook::codegen::generate(&cookfile);
    assert!(lua_source.contains(r#"cook.load_module("hello")"#));

    // Register recipe
    let env_vars = std::collections::HashMap::new();
    let rt = cook::runtime::Runtime::new(dir.path().to_path_buf(), env_vars);
    let result = rt.register_recipe(&lua_source, "build").unwrap();

    assert_eq!(result.units.len(), 1);
    match &result.units[0].payload {
        cook::contracts::WorkPayload::Shell { cmd, .. } => {
            assert_eq!(cmd, "echo hello from module");
        }
        other => panic!("expected Shell, got: {:?}", other),
    }
}
```

- [ ] **Step 2: Run test**

Run: `cargo test --test integration test_use_statement_end_to_end`
Expected: PASS

- [ ] **Step 3: Commit**

```bash
git add tests/integration.rs
git commit -m "test: add end-to-end integration test for module system"
```

### Task 14: Final verification

- [ ] **Step 1: Run full test suite**

Run: `cargo test`
Expected: ALL tests pass

- [ ] **Step 2: Run clippy**

Run: `cargo clippy -- -D warnings`
Expected: no warnings

- [ ] **Step 3: Verify example Cookfile parses**

Create a temporary test:
```bash
cd /tmp && mkdir -p cook_test/cook_modules
cat > cook_test/Cookfile << 'EOF'
use "cpp"

recipe "build"
    >{
        cpp.hello()
    }
end

recipe "clean"
    rm -rf build
end
EOF
cat > cook_test/cook_modules/cpp.lua << 'EOF'
local cpp = {}
function cpp.hello()
    cook.add_unit({ command = "echo hello from cpp module", cache = false })
end
return cpp
EOF
```

Run: `cargo run -- run build -f /tmp/cook_test/Cookfile`
Expected: prints "hello from cpp module"

- [ ] **Step 4: Clean up and final commit if needed**

Run: `cargo test && cargo clippy -- -D warnings`
Expected: all green
