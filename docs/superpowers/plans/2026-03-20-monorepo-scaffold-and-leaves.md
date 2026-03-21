# Monorepo Scaffold + Leaf Crates Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Create the `~/dev/cook/` monorepo structure and extract the three leaf crates (cook-contracts, cook-lang, cook-progress) that have no internal dependencies.

**Architecture:** Fresh git repo at `~/dev/cook/`. The `cli/` subdirectory holds a Cargo workspace. We extract leaf crates first because they have zero internal dependencies — each can be built and tested in isolation. The old repo at `~/dev/remote/github.com/Alex-Gilbert/cook` is read-only reference material.

**Tech Stack:** Rust 2024 edition, Cargo workspace

**Spec:** `docs/superpowers/specs/2026-03-20-monorepo-ddd-rewrite-design.md`

---

## File Structure

### New files to create:

```
~/dev/cook/
├── cli/
│   ├── Cargo.toml                          # Workspace manifest
│   └── crates/
│       ├── cook-contracts/
│       │   ├── Cargo.toml
│       │   └── src/
│       │       └── lib.rs                  # WorkPayload, CacheMeta, CapturedUnit, DepKind, RecipeUnits
│       ├── cook-lang/
│       │   ├── Cargo.toml
│       │   └── src/
│       │       ├── lib.rs                  # Public API: parse(), AST types re-export
│       │       ├── ast.rs                  # Cookfile, Recipe, Step, etc.
│       │       ├── lexer.rs                # Tokenizer
│       │       ├── recipe.rs               # Recipe block parsing
│       │       ├── cook_line.rs            # cook step parsing
│       │       └── lua_block.rs            # Lua block parsing
│       └── cook-progress/
│           ├── Cargo.toml
│           └── src/
│               ├── lib.rs
│               ├── bar.rs
│               ├── frame.rs
│               ├── renderer.rs
│               ├── output.rs
│               └── symbols.rs
├── examples/                               # Copied from old repo
│   ├── Cookfile
│   ├── .env
│   ├── src/
│   ├── lib/
│   ├── include/
│   ├── tests/
│   ├── cpp-project/
│   └── monorepo/
├── marketing/                              # Copied from old repo (excluding node_modules, dist)
│   ├── package.json
│   ├── vite.config.js
│   ├── index.html
│   ├── main.js
│   ├── style.css
│   ├── fire-shader.js
│   ├── public/
│   └── shaders/
├── docs/                                   # Copied from old repo
│   ├── architecture/
│   └── superpowers/
├── CLAUDE.md
├── .gitignore
└── README.md
```

---

## Chunk 1: Monorepo Scaffold

### Task 1: Create monorepo and initialize git

**Files:**
- Create: `~/dev/cook/.gitignore`
- Create: `~/dev/cook/CLAUDE.md`
- Create: `~/dev/cook/cli/Cargo.toml`

- [ ] **Step 1: Create directory and init git**

```bash
mkdir -p ~/dev/cook
cd ~/dev/cook
git init
```

- [ ] **Step 2: Create .gitignore**

```gitignore
# Rust
target/
Cargo.lock
*.swp

# Node
node_modules/
dist/

# Cook artifacts
.cook/
build/
bin/

# IDE
.idea/
.vscode/

# OS
.DS_Store
```

Note: `Cargo.lock` is gitignored at root level since this is a monorepo. The `cli/Cargo.lock` will be committed separately (see workspace Cargo.toml step).

- [ ] **Step 3: Create workspace Cargo.toml**

Create `cli/Cargo.toml`:

```toml
[workspace]
members = [
    "crates/cook-contracts",
    "crates/cook-lang",
    "crates/cook-progress",
]
resolver = "2"
```

- [ ] **Step 4: Create CLAUDE.md**

Copy the existing CLAUDE.md from the old repo and update the module structure section to reflect the new monorepo layout. Keep the testing and architecture sections. Update paths to reflect `cli/crates/` structure.

- [ ] **Step 5: Commit scaffold**

```bash
cd ~/dev/cook
git add .gitignore CLAUDE.md cli/Cargo.toml
git commit -m "chore: scaffold monorepo with workspace manifest"
```

### Task 2: Copy non-Rust assets

**Files:**
- Create: `~/dev/cook/examples/` (copy from old repo)
- Create: `~/dev/cook/marketing/` (copy from old repo, exclude node_modules/dist)
- Create: `~/dev/cook/docs/` (copy from old repo)

- [ ] **Step 1: Copy examples directory**

```bash
cp -r ~/dev/remote/github.com/Alex-Gilbert/cook/examples ~/dev/cook/examples
# Remove build artifacts
rm -rf ~/dev/cook/examples/bin ~/dev/cook/examples/build
rm -rf ~/dev/cook/examples/cpp-project/build ~/dev/cook/examples/cpp-project/bin
```

- [ ] **Step 2: Copy marketing directory (without node_modules and dist)**

```bash
cp -r ~/dev/remote/github.com/Alex-Gilbert/cook/marketing ~/dev/cook/marketing
rm -rf ~/dev/cook/marketing/node_modules ~/dev/cook/marketing/dist
```

- [ ] **Step 3: Copy docs directory**

```bash
cp -r ~/dev/remote/github.com/Alex-Gilbert/cook/docs ~/dev/cook/docs
```

- [ ] **Step 4: Commit assets**

```bash
cd ~/dev/cook
git add examples/ marketing/ docs/
git commit -m "chore: add examples, marketing, and docs from original repo"
```

---

## Chunk 2: cook-contracts

### Task 3: Create cook-contracts crate with behavior-free types

**Files:**
- Create: `cli/crates/cook-contracts/Cargo.toml`
- Create: `cli/crates/cook-contracts/src/lib.rs`

**Source reference:** `~/dev/remote/github.com/Alex-Gilbert/cook/src/contracts/mod.rs` (247 lines)

Per the DDD review, cook-contracts contains ONLY behavior-free structs. The following types stay:
- `WorkPayload` (enum — Shell, Interactive, LuaChunk, Test)
- `CacheMeta` (struct)
- `CapturedUnit` (struct)
- `DepKind` (enum)
- `RecipeUnits` (struct)

The following are EXCLUDED (they move to other crates in later phases):
- `CaptureState`, `SharedCaptureState` → cook-register (Phase 3)
- `hash_str`, `resolve_glob` → cook-cache (Phase 2)
- `TestResults`, `TestCaseResult`, `TestSuiteResult`, `TestStatus`, `SharedTestResults` → cook-cli (Phase 5)
- `WorkPayload::display_name()` → cook-cli (Phase 5)

- [ ] **Step 1: Create Cargo.toml**

Create `cli/crates/cook-contracts/Cargo.toml`:

```toml
[package]
name = "cook-contracts"
version = "0.1.0"
edition = "2024"
description = "Shared types for the Cook build system — behavior-free structs only"

[dependencies]
serde = { version = "1", features = ["derive"] }
```

- [ ] **Step 2: Write tests for type construction**

Add to `cli/crates/cook-contracts/src/lib.rs`:

```rust
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub enum WorkPayload {
    Shell {
        cmd: String,
        line: usize,
    },
    Interactive {
        cmd: String,
        line: usize,
    },
    LuaChunk {
        code: String,
        input: String,
        output: String,
        ingredient_groups: Vec<Vec<String>>,
    },
    Test {
        cmd: String,
        line: usize,
        timeout: u64,
        should_fail: bool,
        suite_name: String,
        test_name: String,
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

#[derive(Debug, Clone)]
pub struct CapturedUnit {
    pub payload: WorkPayload,
    pub cache_meta: Option<CacheMeta>,
    pub dep_kind: DepKind,
}

/// How a captured unit relates to others in the recipe.
#[derive(Debug, Clone)]
pub enum DepKind {
    /// Part of a cook step group (can run parallel with siblings in the group)
    StepGroup(usize),
    /// Sequential barrier (depends on all prior units in recipe)
    Sequential,
    /// Part of a test step group — like StepGroup but failures don't cancel siblings
    TestSibling(usize),
}

/// Result of registering a single recipe.
pub struct RecipeUnits {
    pub recipe_name: String,
    pub deps: Vec<String>,
    pub units: Vec<CapturedUnit>,
    pub step_groups: Vec<Vec<usize>>,
    pub working_dir: PathBuf,
    pub env_vars: BTreeMap<String, String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_work_payload_shell() {
        let p = WorkPayload::Shell { cmd: "gcc -c foo.c".into(), line: 1 };
        assert!(matches!(p, WorkPayload::Shell { .. }));
    }

    #[test]
    fn test_dep_kind_variants() {
        let sg = DepKind::StepGroup(0);
        let seq = DepKind::Sequential;
        let ts = DepKind::TestSibling(1);
        assert!(matches!(sg, DepKind::StepGroup(0)));
        assert!(matches!(seq, DepKind::Sequential));
        assert!(matches!(ts, DepKind::TestSibling(1)));
    }

    #[test]
    fn test_recipe_units_construction() {
        let units = RecipeUnits {
            recipe_name: "lib".into(),
            deps: vec!["setup".into()],
            units: vec![],
            step_groups: vec![],
            working_dir: PathBuf::from("."),
            env_vars: BTreeMap::new(),
        };
        assert_eq!(units.recipe_name, "lib");
        assert_eq!(units.deps, vec!["setup"]);
    }

    #[test]
    fn test_cache_meta_construction() {
        let meta = CacheMeta {
            recipe_name: "lib".into(),
            cache_key: "build/obj/foo.o".into(),
            input_paths: vec!["lib/foo.c".into()],
            output_path: Some("build/obj/foo.o".into()),
            command_hash: 12345,
        };
        assert_eq!(meta.cache_key, "build/obj/foo.o");
    }
}
```

- [ ] **Step 3: Run tests**

```bash
cd ~/dev/cook/cli
cargo test -p cook-contracts
```

Expected: all 4 tests pass.

- [ ] **Step 4: Commit**

```bash
cd ~/dev/cook
git add cli/crates/cook-contracts/
git commit -m "feat: add cook-contracts crate with shared behavior-free types"
```

---

## Chunk 3: cook-lang

### Task 4: Create cook-lang crate

**Files:**
- Create: `cli/crates/cook-lang/Cargo.toml`
- Create: `cli/crates/cook-lang/src/lib.rs`
- Create: `cli/crates/cook-lang/src/ast.rs`
- Create: `cli/crates/cook-lang/src/lexer.rs`
- Create: `cli/crates/cook-lang/src/recipe.rs`
- Create: `cli/crates/cook-lang/src/cook_line.rs`
- Create: `cli/crates/cook-lang/src/lua_block.rs`

**Source reference:** `~/dev/remote/github.com/Alex-Gilbert/cook/src/parser/` (1,879 lines total)

cook-lang is a pure parser crate. It takes Cookfile text and produces an AST. No dependencies on any other cook crate.

- [ ] **Step 1: Create Cargo.toml**

Create `cli/crates/cook-lang/Cargo.toml`:

```toml
[package]
name = "cook-lang"
version = "0.1.0"
edition = "2024"
description = "Cookfile parser — lexer, parser, and AST types"

[dependencies]
glob = "0.3"

[dev-dependencies]
```

- [ ] **Step 2: Copy parser source files**

Copy each file from the old repo's `src/parser/` into `cli/crates/cook-lang/src/`:

```bash
OLD=~/dev/remote/github.com/Alex-Gilbert/cook/src/parser
NEW=~/dev/cook/cli/crates/cook-lang/src

mkdir -p $NEW
cp $OLD/ast.rs $NEW/ast.rs
cp $OLD/lexer.rs $NEW/lexer.rs
cp $OLD/recipe.rs $NEW/recipe.rs
cp $OLD/cook_line.rs $NEW/cook_line.rs
cp $OLD/lua_block.rs $NEW/lua_block.rs
```

- [ ] **Step 3: Create lib.rs as the public API facade**

Create `cli/crates/cook-lang/src/lib.rs`:

```rust
pub mod ast;
mod lexer;
mod recipe;
mod cook_line;
mod lua_block;

pub use ast::Cookfile;

/// Parse a Cookfile source string into a Cookfile AST.
pub fn parse(source: &str) -> Result<Cookfile, ParseError> {
    let tokens = lexer::tokenize(source)?;
    recipe::parse_tokens(tokens)
}

#[derive(Debug, thiserror::Error)]
pub enum ParseError {
    #[error("lex error: {0}")]
    Lex(String),
    #[error("parse error: {0}")]
    Parse(String),
}
```

Note: The exact error type and parse function signature should be adapted to match the current `parser/mod.rs` implementation. The old `mod.rs` has the `parse()` function — use its logic.

- [ ] **Step 4: Update internal imports**

All files in the old parser module use `use crate::parser::...` or `use super::...` for cross-references. Update these to use crate-internal paths:

- `use crate::parser::ast::*` → `use crate::ast::*`
- `use crate::parser::lexer::*` → `use crate::lexer::*`
- `use super::*` paths should work as-is if module structure is preserved

Check each file for `use crate::` imports that reference anything outside parser — these would indicate unexpected dependencies. The parser should be self-contained.

- [ ] **Step 5: Copy and adapt tests**

Copy `src/parser/tests.rs` into `cli/crates/cook-lang/src/tests.rs` and add `mod tests;` to `lib.rs` (behind `#[cfg(test)]`). Update imports to use the new crate paths.

Alternatively, if the tests in `tests.rs` are already `#[cfg(test)]` module tests within the parser, integrate them into the relevant source files.

- [ ] **Step 6: Verify it compiles and tests pass**

```bash
cd ~/dev/cook/cli
cargo test -p cook-lang
```

Expected: all parser tests pass. Fix any import issues discovered.

- [ ] **Step 7: Commit**

```bash
cd ~/dev/cook
git add cli/crates/cook-lang/
git commit -m "feat: add cook-lang crate — Cookfile lexer, parser, and AST"
```

---

## Chunk 4: cook-progress

### Task 5: Move cook-progress crate

**Files:**
- Create: `cli/crates/cook-progress/` (copy from old repo's `crates/cook-progress/`)

**Source reference:** `~/dev/remote/github.com/Alex-Gilbert/cook/crates/cook-progress/` (already a standalone crate)

cook-progress is already a standalone crate in the old repo. This is a straight copy.

- [ ] **Step 1: Copy the crate**

```bash
cp -r ~/dev/remote/github.com/Alex-Gilbert/cook/crates/cook-progress ~/dev/cook/cli/crates/cook-progress
```

- [ ] **Step 2: Verify it compiles and tests pass**

```bash
cd ~/dev/cook/cli
cargo test -p cook-progress
```

Expected: all tests pass (this crate is already standalone).

- [ ] **Step 3: Commit**

```bash
cd ~/dev/cook
git add cli/crates/cook-progress/
git commit -m "feat: add cook-progress crate — terminal progress rendering primitives"
```

---

## Chunk 5: Workspace Verification

### Task 6: Full workspace compilation and test

- [ ] **Step 1: Verify full workspace compiles**

```bash
cd ~/dev/cook/cli
cargo build
```

Expected: all three crates compile successfully.

- [ ] **Step 2: Run all workspace tests**

```bash
cd ~/dev/cook/cli
cargo test
```

Expected: all tests across cook-contracts, cook-lang, and cook-progress pass.

- [ ] **Step 3: Verify crate independence**

Each crate should compile independently:

```bash
cd ~/dev/cook/cli
cargo build -p cook-contracts
cargo build -p cook-lang
cargo build -p cook-progress
```

Expected: each compiles with zero warnings about unused dependencies.

- [ ] **Step 4: Final commit if any fixes were needed**

```bash
cd ~/dev/cook
git add -A
git status  # verify only expected files
git commit -m "fix: resolve workspace compilation issues"
```

Only commit if fixes were needed. Skip if step 1-3 passed cleanly.

---

## Completion Criteria

- [ ] `~/dev/cook/` git repo exists with clean history
- [ ] `cli/` workspace compiles with 3 crates: cook-contracts, cook-lang, cook-progress
- [ ] `cargo test` passes for all 3 crates
- [ ] Each crate compiles independently (no hidden cross-dependencies)
- [ ] `examples/`, `marketing/`, `docs/` are present at top level
- [ ] No code from other modules (cache, runtime, scheduler, etc.) has leaked into leaf crates
- [ ] cook-contracts contains ONLY behavior-free structs (no methods, no utility functions)
- [ ] cook-lang has zero dependencies on other cook crates
- [ ] cook-progress has zero dependencies on other cook crates

---

## Review Errata (from plan review)

These issues were identified during plan review. The executing agent MUST address them:

### cook-lang

1. **DO NOT use the lib.rs code snippet in Task 4 Step 3.** The `parse()` function shown is invented and does not match the real implementation. Instead, copy the contents of `src/parser/mod.rs` directly into `lib.rs`, changing only module paths (`pub(crate)` → `mod`, `use crate::parser::` → `use crate::`). The real `parse()` function (lines 20-126 of `mod.rs`) handles var decls, config blocks, use statements, import decls, and recipes — it is NOT a simple `tokenize → parse_tokens` pipeline.

2. **`ParseError` definition is wrong in the plan.** The real definition (in `mod.rs`) is:
   ```rust
   pub enum ParseError {
       Lex(#[from] LexError),
       Parse { line: usize, message: String },
   }
   ```
   NOT `Lex(String)` and `Parse(String)`.

3. **Missing `thiserror` dependency.** Add `thiserror = "2"` to cook-lang's `Cargo.toml`. Both `lexer.rs` and `mod.rs` use `#[derive(thiserror::Error)]`.

4. **Fix test paths.** In `tests.rs`, replace all `crate::parser::parse(` with `crate::parse(`. Affected tests: `test_parse_use_statement`, `test_parse_multiple_use_statements`, `test_parse_use_with_vars_and_configs`, `test_parse_use_after_recipe_fails`, `test_parse_import_decl`, `test_parse_import_after_recipe_fails`, `test_parse_duplicate_import_names_fails`.

5. **`ast.rs` uses `HashMap` for `configs`.** Change to `BTreeMap` per the spec's deterministic output convention.

### cook-contracts

6. **`RecipeUnits.env_vars` change from `HashMap` to `BTreeMap` is intentional.** Document this in the commit message: "Changed env_vars from HashMap to BTreeMap per spec constraint for deterministic output."

7. **Remove `serde` from Cargo.toml** unless you add `#[derive(Serialize, Deserialize)]` to the types. The current source does not derive serde on these types.

### .gitignore

8. **Change `Cargo.lock` to `/Cargo.lock`** (matches only root) or add `!cli/Cargo.lock` to unignore the workspace lock file. The bare pattern `Cargo.lock` would also match `cli/Cargo.lock` which should be committed.
