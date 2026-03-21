# Incremental Build Cache Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add step-level incremental caching so Cook only re-executes layers whose inputs have changed.

**Architecture:** New `cache/` module with three files: `store.rs` (data types + bincode persistence), `check.rs` (rebuild-decision algorithm), `mod.rs` (cook.layer() Lua API). Codegen wraps `cook`/`plate` steps in `cook.layer()` calls. Runtime intercepts layers and skips unchanged ones.

**Tech Stack:** Rust, bincode (binary serde), xxhash-rust (xxh3_64 content hashing), mlua (existing)

**Spec:** `docs/superpowers/specs/2026-03-13-incremental-cache-design.md`

---

## Chunk 1: Cache Data Model + Persistence

### Task 1: Add dependencies

**Files:**
- Modify: `Cargo.toml`

- [ ] **Step 1: Add bincode, xxhash-rust, serde to Cargo.toml**

```toml
[dependencies]
mlua = { version = "0.10", features = ["lua54", "vendored"] }
clap = { version = "4", features = ["derive"] }
notify = "7"
glob = "0.3"
dotenvy = "0.15"
thiserror = "2"
serde = { version = "1", features = ["derive"] }
bincode = "1"
xxhash-rust = { version = "0.8", features = ["xxh3"] }
```

- [ ] **Step 2: Verify it compiles**

Run: `cargo check`
Expected: compiles with no errors

- [ ] **Step 3: Commit**

```bash
git add Cargo.toml Cargo.lock
git commit -m "deps: add serde, bincode, xxhash-rust for incremental cache"
```

---

### Task 2: Cache store data types

**Files:**
- Create: `src/cache/store.rs`
- Create: `src/cache/mod.rs`
- Modify: `src/lib.rs`

- [ ] **Step 1: Write tests for RecipeCache serialization round-trip**

In `src/cache/store.rs`:

```rust
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

pub const CACHE_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RecipeCache {
    pub version: u32,
    pub globs: HashMap<String, Vec<String>>,
    pub secondary_inputs_hash: u64,
    pub env_hash: u64,
    pub steps: HashMap<String, StepEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct StepEntry {
    pub inputs: Vec<FileRecord>,
    pub output: Option<FileRecord>,
    pub command_hash: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FileRecord {
    pub path: String,
    pub mtime: u64,
    pub hash: u64,
}

impl RecipeCache {
    pub fn new() -> Self {
        Self {
            version: CACHE_VERSION,
            globs: HashMap::new(),
            secondary_inputs_hash: 0,
            env_hash: 0,
            steps: HashMap::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_recipe_cache_round_trip() {
        let mut cache = RecipeCache::new();
        cache.globs.insert("src/*.c".to_string(), vec!["src/main.c".to_string()]);
        cache.secondary_inputs_hash = 0xdeadbeef;
        cache.env_hash = 0xcafebabe;
        cache.steps.insert(
            "build/main.o".to_string(),
            StepEntry {
                inputs: vec![FileRecord {
                    path: "src/main.c".to_string(),
                    mtime: 1000,
                    hash: 0xaabb,
                }],
                output: Some(FileRecord {
                    path: "build/main.o".to_string(),
                    mtime: 1001,
                    hash: 0xccdd,
                }),
                command_hash: 0x1234,
            },
        );

        let bytes = bincode::serialize(&cache).unwrap();
        let restored: RecipeCache = bincode::deserialize(&bytes).unwrap();
        assert_eq!(cache, restored);
    }

    #[test]
    fn test_empty_cache_round_trip() {
        let cache = RecipeCache::new();
        let bytes = bincode::serialize(&cache).unwrap();
        let restored: RecipeCache = bincode::deserialize(&bytes).unwrap();
        assert_eq!(cache, restored);
    }

    #[test]
    fn test_plate_step_no_output() {
        let entry = StepEntry {
            inputs: vec![FileRecord {
                path: "test/a.bin".to_string(),
                mtime: 500,
                hash: 0x1111,
            }],
            output: None,
            command_hash: 0x9999,
        };
        let bytes = bincode::serialize(&entry).unwrap();
        let restored: StepEntry = bincode::deserialize(&bytes).unwrap();
        assert_eq!(entry, restored);
        assert!(restored.output.is_none());
    }
}
```

- [ ] **Step 2: Create cache/mod.rs and register the module**

In `src/cache/mod.rs`:

```rust
pub mod store;
```

In `src/lib.rs`, add:

```rust
pub mod cache;
```

- [ ] **Step 3: Run tests to verify they pass**

Run: `cargo test cache::store`
Expected: 3 tests pass

- [ ] **Step 4: Commit**

```bash
git add src/cache/ src/lib.rs
git commit -m "feat(cache): add RecipeCache data types with bincode serialization"
```

---

### Task 3: Cache file load/save with atomic writes

**Files:**
- Modify: `src/cache/store.rs`

- [ ] **Step 1: Write tests for file persistence**

Add to `src/cache/store.rs`:

```rust
use std::path::Path;

impl RecipeCache {
    /// Load a recipe cache from `.cook/cache/<name>.bin`.
    /// Returns None if missing, corrupted, or version mismatch.
    pub fn load(cache_dir: &Path, recipe_name: &str) -> Option<Self> {
        let path = cache_dir.join(format!("{}.bin", recipe_name));
        let bytes = std::fs::read(&path).ok()?;
        let cache: Self = bincode::deserialize(&bytes).ok()?;
        if cache.version != CACHE_VERSION {
            return None;
        }
        Some(cache)
    }

    /// Save the cache atomically (write to temp, rename).
    pub fn save(&self, cache_dir: &Path, recipe_name: &str) -> std::io::Result<()> {
        std::fs::create_dir_all(cache_dir)?;
        let target = cache_dir.join(format!("{}.bin", recipe_name));
        let tmp = cache_dir.join(format!("{}.bin.tmp", recipe_name));
        let bytes = bincode::serialize(self)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
        std::fs::write(&tmp, &bytes)?;
        std::fs::rename(&tmp, &target)?;
        Ok(())
    }
}
```

Add tests:

```rust
#[test]
fn test_save_and_load() {
    let dir = tempfile::tempdir().unwrap();
    let cache_dir = dir.path().join(".cook/cache");

    let mut cache = RecipeCache::new();
    cache.steps.insert(
        "out.o".to_string(),
        StepEntry {
            inputs: vec![FileRecord { path: "in.c".to_string(), mtime: 1, hash: 2 }],
            output: Some(FileRecord { path: "out.o".to_string(), mtime: 3, hash: 4 }),
            command_hash: 5,
        },
    );

    cache.save(&cache_dir, "build").unwrap();
    let loaded = RecipeCache::load(&cache_dir, "build").unwrap();
    assert_eq!(cache, loaded);
}

#[test]
fn test_load_missing_returns_none() {
    let dir = tempfile::tempdir().unwrap();
    assert!(RecipeCache::load(dir.path(), "nonexistent").is_none());
}

#[test]
fn test_load_corrupted_returns_none() {
    let dir = tempfile::tempdir().unwrap();
    let cache_dir = dir.path();
    std::fs::create_dir_all(cache_dir).unwrap();
    std::fs::write(cache_dir.join("bad.bin"), b"not valid bincode").unwrap();
    assert!(RecipeCache::load(cache_dir, "bad").is_none());
}

#[test]
fn test_load_wrong_version_returns_none() {
    let dir = tempfile::tempdir().unwrap();
    let cache_dir = dir.path();

    let mut cache = RecipeCache::new();
    cache.version = 999;
    let bytes = bincode::serialize(&cache).unwrap();
    std::fs::create_dir_all(cache_dir).unwrap();
    std::fs::write(cache_dir.join("old.bin"), &bytes).unwrap();

    assert!(RecipeCache::load(cache_dir, "old").is_none());
}
```

- [ ] **Step 2: Add tempfile to dev-dependencies if not already present**

Already in `Cargo.toml` under `[dev-dependencies]` — no change needed.

- [ ] **Step 3: Run tests**

Run: `cargo test cache::store`
Expected: 7 tests pass (3 from Task 2 + 4 new)

- [ ] **Step 4: Commit**

```bash
git add src/cache/store.rs
git commit -m "feat(cache): add atomic load/save for recipe cache files"
```

---

## Chunk 2: Hashing Utilities + Rebuild-Check Algorithm

### Task 4: File hashing utilities

**Files:**
- Create: `src/cache/check.rs`
- Modify: `src/cache/mod.rs`

- [ ] **Step 1: Write tests for hash_file and stat_mtime**

In `src/cache/check.rs`:

```rust
use std::path::Path;

/// Get mtime as epoch milliseconds. Returns None if file doesn't exist.
/// Uses millisecond resolution to catch rapid modifications.
pub fn stat_mtime(path: &Path) -> Option<u64> {
    let meta = std::fs::metadata(path).ok()?;
    let mtime = meta.modified().ok()?;
    Some(
        mtime
            .duration_since(std::time::UNIX_EPOCH)
            .ok()?
            .as_millis() as u64,
    )
}

/// Hash file contents with xxh3_64. Returns None if file can't be read.
pub fn hash_file(path: &Path) -> Option<u64> {
    let bytes = std::fs::read(path).ok()?;
    Some(xxhash_rust::xxh3::xxh3_64(&bytes))
}

/// Hash a string (for command templates, env vars, etc.)
/// NOTE: This is also used by codegen for command hashing.
/// It lives here rather than in a separate util module since cache
/// is the only consumer besides codegen, and the function is trivial.
pub fn hash_str(s: &str) -> u64 {
    xxhash_rust::xxh3::xxh3_64(s.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn test_hash_file_deterministic() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.txt");
        fs::write(&path, "hello world").unwrap();

        let h1 = hash_file(&path).unwrap();
        let h2 = hash_file(&path).unwrap();
        assert_eq!(h1, h2);
    }

    #[test]
    fn test_hash_file_differs_on_content() {
        let dir = tempfile::tempdir().unwrap();
        let p1 = dir.path().join("a.txt");
        let p2 = dir.path().join("b.txt");
        fs::write(&p1, "hello").unwrap();
        fs::write(&p2, "world").unwrap();

        assert_ne!(hash_file(&p1), hash_file(&p2));
    }

    #[test]
    fn test_hash_file_missing_returns_none() {
        let dir = tempfile::tempdir().unwrap();
        assert!(hash_file(&dir.path().join("nope.txt")).is_none());
    }

    #[test]
    fn test_stat_mtime_returns_positive() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.txt");
        fs::write(&path, "data").unwrap();

        let mtime = stat_mtime(&path).unwrap();
        assert!(mtime > 0);
    }

    #[test]
    fn test_stat_mtime_missing_returns_none() {
        let dir = tempfile::tempdir().unwrap();
        assert!(stat_mtime(&dir.path().join("nope.txt")).is_none());
    }

    #[test]
    fn test_hash_str_deterministic() {
        let h1 = hash_str("gcc -c {in} -o {out}");
        let h2 = hash_str("gcc -c {in} -o {out}");
        assert_eq!(h1, h2);
    }

    #[test]
    fn test_hash_str_differs() {
        let h1 = hash_str("gcc -c {in} -o {out}");
        let h2 = hash_str("gcc -O2 -c {in} -o {out}");
        assert_ne!(h1, h2);
    }
}
```

- [ ] **Step 2: Add check module to cache/mod.rs**

```rust
pub mod store;
pub mod check;
```

- [ ] **Step 3: Run tests**

Run: `cargo test cache::check`
Expected: 7 tests pass

- [ ] **Step 4: Commit**

```bash
git add src/cache/check.rs src/cache/mod.rs
git commit -m "feat(cache): add file hashing and mtime utilities"
```

---

### Task 5: Rebuild-check algorithm

**Files:**
- Modify: `src/cache/check.rs`

- [ ] **Step 1: Write the RebuildResult enum and needs_rebuild function**

Add to `src/cache/check.rs`:

```rust
use crate::cache::store::{FileRecord, StepEntry};

#[derive(Debug, PartialEq)]
pub enum RebuildResult {
    Skip,
    Rebuild(RebuildReason),
}

#[derive(Debug, PartialEq)]
pub enum RebuildReason {
    NoCacheEntry,
    CommandHashChanged,
    OutputMissing,
    OutputChanged,
    InputSetChanged,
    InputChanged(String),
}

/// Check inputs against cached state. Shared logic for cook and plate layers.
/// `current_input_paths` are **relative** paths (matching cached paths).
/// `working_dir` is used to resolve to absolute paths for stat/hash on disk.
fn check_inputs(
    cached_inputs: &[FileRecord],
    current_input_paths: &[&str],
    working_dir: &Path,
) -> Result<Vec<FileRecord>, RebuildReason> {
    // Check input set changed
    let cached_paths: Vec<&str> = cached_inputs.iter().map(|f| f.path.as_str()).collect();
    if cached_paths != current_input_paths {
        return Err(RebuildReason::InputSetChanged);
    }

    let mut updated = cached_inputs.to_vec();
    for (i, (cached, rel_path)) in cached_inputs.iter().zip(current_input_paths.iter()).enumerate() {
        let abs_path = working_dir.join(rel_path);
        let disk_mtime = match stat_mtime(&abs_path) {
            Some(m) => m,
            None => return Err(RebuildReason::InputChanged(cached.path.clone())),
        };

        if disk_mtime != cached.mtime {
            let disk_hash = match hash_file(&abs_path) {
                Some(h) => h,
                None => return Err(RebuildReason::InputChanged(cached.path.clone())),
            };
            if disk_hash != cached.hash {
                return Err(RebuildReason::InputChanged(cached.path.clone()));
            }
            // mtime changed but content same — update mtime in cache
            updated[i].mtime = disk_mtime;
        }
    }
    Ok(updated)
}

/// Check if a cook layer (with output) needs to rebuild.
/// All paths are **relative** to working_dir (matching what's stored in cache).
/// `working_dir` resolves them to absolute paths for disk I/O.
///
/// INVARIANT: cook.layer() calls must NOT be nested. The Rc<RefCell<CacheState>>
/// will panic on double borrow if layers are nested.
pub fn needs_rebuild_cook(
    entry: Option<&StepEntry>,
    current_inputs: &[&str],
    current_output: &str,
    command_hash: u64,
    working_dir: &Path,
) -> (RebuildResult, Option<StepEntry>) {
    let entry = match entry {
        None => return (RebuildResult::Rebuild(RebuildReason::NoCacheEntry), None),
        Some(e) => e,
    };

    if entry.command_hash != command_hash {
        return (RebuildResult::Rebuild(RebuildReason::CommandHashChanged), None);
    }

    let abs_output = working_dir.join(current_output);

    // Check output exists
    if !abs_output.exists() {
        return (RebuildResult::Rebuild(RebuildReason::OutputMissing), None);
    }

    // Check output not tampered
    if let Some(ref cached_out) = entry.output {
        if let Some(disk_mtime) = stat_mtime(&abs_output) {
            if disk_mtime != cached_out.mtime {
                if let Some(disk_hash) = hash_file(&abs_output) {
                    if disk_hash != cached_out.hash {
                        return (RebuildResult::Rebuild(RebuildReason::OutputChanged), None);
                    }
                }
            }
        }
    }

    // Check inputs
    match check_inputs(&entry.inputs, current_inputs, working_dir) {
        Err(reason) => (RebuildResult::Rebuild(reason), None),
        Ok(updated_inputs) => {
            let updated_entry = StepEntry {
                inputs: updated_inputs,
                output: entry.output.clone(),
                command_hash: entry.command_hash,
            };
            (RebuildResult::Skip, Some(updated_entry))
        }
    }
}

/// Check if a plate layer (no output) needs to re-run.
/// All paths are **relative** to working_dir.
pub fn needs_rebuild_plate(
    entry: Option<&StepEntry>,
    current_inputs: &[&str],
    command_hash: u64,
    working_dir: &Path,
) -> (RebuildResult, Option<StepEntry>) {
    let entry = match entry {
        None => return (RebuildResult::Rebuild(RebuildReason::NoCacheEntry), None),
        Some(e) => e,
    };

    if entry.command_hash != command_hash {
        return (RebuildResult::Rebuild(RebuildReason::CommandHashChanged), None);
    }

    match check_inputs(&entry.inputs, current_inputs, working_dir) {
        Err(reason) => (RebuildResult::Rebuild(reason), None),
        Ok(updated_inputs) => {
            let updated_entry = StepEntry {
                inputs: updated_inputs,
                output: None,
                command_hash: entry.command_hash,
            };
            (RebuildResult::Skip, Some(updated_entry))
        }
    }
}
```

- [ ] **Step 2: Write tests for the rebuild-check algorithm**

Add to `src/cache/check.rs` tests module:

```rust
#[test]
fn test_no_cache_entry_rebuilds() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("in.c"), "code").unwrap();
    fs::write(dir.path().join("out.o"), "obj").unwrap();

    let (result, _) = needs_rebuild_cook(None, &["in.c"], "out.o", 0x1234, dir.path());
    assert_eq!(result, RebuildResult::Rebuild(RebuildReason::NoCacheEntry));
}

#[test]
fn test_command_hash_changed_rebuilds() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("in.c"), "code").unwrap();
    fs::write(dir.path().join("out.o"), "obj").unwrap();

    let entry = StepEntry {
        inputs: vec![FileRecord {
            path: "in.c".to_string(),
            mtime: stat_mtime(&dir.path().join("in.c")).unwrap(),
            hash: hash_file(&dir.path().join("in.c")).unwrap(),
        }],
        output: Some(FileRecord {
            path: "out.o".to_string(),
            mtime: stat_mtime(&dir.path().join("out.o")).unwrap(),
            hash: hash_file(&dir.path().join("out.o")).unwrap(),
        }),
        command_hash: 0x1234,
    };

    let (result, _) = needs_rebuild_cook(
        Some(&entry), &["in.c"], "out.o", 0x5678, dir.path(),
    );
    assert_eq!(result, RebuildResult::Rebuild(RebuildReason::CommandHashChanged));
}

#[test]
fn test_output_missing_rebuilds() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("in.c"), "code").unwrap();
    // out.o does NOT exist on disk

    let entry = StepEntry {
        inputs: vec![FileRecord {
            path: "in.c".to_string(),
            mtime: stat_mtime(&dir.path().join("in.c")).unwrap(),
            hash: hash_file(&dir.path().join("in.c")).unwrap(),
        }],
        output: Some(FileRecord {
            path: "out.o".to_string(),
            mtime: 1000,
            hash: 0xaaaa,
        }),
        command_hash: 0x1234,
    };

    let (result, _) = needs_rebuild_cook(Some(&entry), &["in.c"], "out.o", 0x1234, dir.path());
    assert_eq!(result, RebuildResult::Rebuild(RebuildReason::OutputMissing));
}

#[test]
fn test_nothing_changed_skips() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("in.c"), "code").unwrap();
    fs::write(dir.path().join("out.o"), "obj").unwrap();

    let entry = StepEntry {
        inputs: vec![FileRecord {
            path: "in.c".to_string(),
            mtime: stat_mtime(&dir.path().join("in.c")).unwrap(),
            hash: hash_file(&dir.path().join("in.c")).unwrap(),
        }],
        output: Some(FileRecord {
            path: "out.o".to_string(),
            mtime: stat_mtime(&dir.path().join("out.o")).unwrap(),
            hash: hash_file(&dir.path().join("out.o")).unwrap(),
        }),
        command_hash: 0x1234,
    };

    let (result, _) = needs_rebuild_cook(Some(&entry), &["in.c"], "out.o", 0x1234, dir.path());
    assert_eq!(result, RebuildResult::Skip);
}

#[test]
fn test_input_content_changed_rebuilds() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("in.c"), "old code").unwrap();
    fs::write(dir.path().join("out.o"), "obj").unwrap();

    let old_hash = hash_file(&dir.path().join("in.c")).unwrap();
    let old_mtime = stat_mtime(&dir.path().join("in.c")).unwrap();

    // Change input content
    std::thread::sleep(std::time::Duration::from_millis(50));
    fs::write(dir.path().join("in.c"), "new code").unwrap();

    let entry = StepEntry {
        inputs: vec![FileRecord {
            path: "in.c".to_string(),
            mtime: old_mtime,
            hash: old_hash,
        }],
        output: Some(FileRecord {
            path: "out.o".to_string(),
            mtime: stat_mtime(&dir.path().join("out.o")).unwrap(),
            hash: hash_file(&dir.path().join("out.o")).unwrap(),
        }),
        command_hash: 0x1234,
    };

    let (result, _) = needs_rebuild_cook(Some(&entry), &["in.c"], "out.o", 0x1234, dir.path());
    assert!(matches!(result, RebuildResult::Rebuild(RebuildReason::InputChanged(_))));
}

#[test]
fn test_plate_no_cache_entry_runs() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("test.bin"), "binary").unwrap();

    let (result, _) = needs_rebuild_plate(None, &["test.bin"], 0x1234, dir.path());
    assert_eq!(result, RebuildResult::Rebuild(RebuildReason::NoCacheEntry));
}

#[test]
fn test_plate_nothing_changed_skips() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("test.bin"), "binary").unwrap();

    let entry = StepEntry {
        inputs: vec![FileRecord {
            path: "test.bin".to_string(),
            mtime: stat_mtime(&dir.path().join("test.bin")).unwrap(),
            hash: hash_file(&dir.path().join("test.bin")).unwrap(),
        }],
        output: None,
        command_hash: 0x1234,
    };

    let (result, _) = needs_rebuild_plate(Some(&entry), &["test.bin"], 0x1234, dir.path());
    assert_eq!(result, RebuildResult::Skip);
}
```

- [ ] **Step 3: Run tests**

Run: `cargo test cache::check`
Expected: all 14 tests pass (7 utility + 7 algorithm)

- [ ] **Step 4: Commit**

```bash
git add src/cache/check.rs
git commit -m "feat(cache): implement rebuild-check algorithm with mtime+hash detection"
```

---

## Chunk 3: Codegen Layer Wrapping

### Task 6: Add command hash computation to codegen

**Files:**
- Modify: `src/codegen/mod.rs`

- [ ] **Step 1: Write tests for command hash generation**

Add to `src/codegen/mod.rs` tests module:

```rust
#[test]
fn test_cook_step_one_to_one_has_layer() {
    let cookfile = make_cookfile(vec![make_recipe(
        "build",
        vec![],
        vec!["src/*.c"],
        vec![Step::Cook {
            step: CookStep {
                output_pattern: "build/{stem}.o".to_string(),
                using_clause: Some(UsingClause::Shell(
                    "gcc -c {in} -o {out}".to_string(),
                )),
            },
            line: 3,
        }],
    )]);
    let output = generate(&cookfile);
    assert!(output.contains("cook.layer("), "should contain cook.layer call");
    assert!(output.contains("_cook_in, _cook_out"), "should pass input and output");
    assert!(output.contains("function()"), "should wrap body in closure");
}

#[test]
fn test_cook_step_many_to_one_has_layer() {
    let cookfile = make_cookfile(vec![make_recipe(
        "build",
        vec![],
        vec!["src/*.c"],
        vec![
            Step::Cook {
                step: CookStep {
                    output_pattern: "build/{stem}.o".to_string(),
                    using_clause: Some(UsingClause::Shell(
                        "gcc -c {in} -o {out}".to_string(),
                    )),
                },
                line: 3,
            },
            Step::Cook {
                step: CookStep {
                    output_pattern: "build/lib.a".to_string(),
                    using_clause: Some(UsingClause::Shell(
                        "ar rcs {out} {all}".to_string(),
                    )),
                },
                line: 4,
            },
        ],
    )]);
    let output = generate(&cookfile);
    // Many-to-one should pass input table
    assert!(output.contains("cook.layer(_cook_outputs_1, _cook_out"), "many-to-one should pass input table");
}

#[test]
fn test_cook_step_declaration_no_layer() {
    let cookfile = make_cookfile(vec![make_recipe(
        "build",
        vec![],
        vec![],
        vec![Step::Cook {
            step: CookStep {
                output_pattern: "bin/app".to_string(),
                using_clause: None,
            },
            line: 3,
        }],
    )]);
    let output = generate(&cookfile);
    assert!(!output.contains("cook.layer"), "declaration-only should not use cook.layer");
}

#[test]
fn test_plate_step_has_layer_nil_output() {
    let cookfile = make_cookfile(vec![make_recipe(
        "run",
        vec![],
        vec![],
        vec![
            Step::Cook {
                step: CookStep {
                    output_pattern: "bin/app".to_string(),
                    using_clause: None,
                },
                line: 2,
            },
            Step::Plate {
                step: PlateStep {
                    command: "./{out}".to_string(),
                },
                line: 3,
            },
        ],
    )]);
    let output = generate(&cookfile);
    assert!(output.contains("cook.layer("), "plate should use cook.layer");
    assert!(output.contains(", nil,"), "plate should pass nil as output");
}

#[test]
fn test_shell_step_no_layer() {
    let cookfile = make_cookfile(vec![make_recipe(
        "build",
        vec![],
        vec![],
        vec![Step::Shell {
            command: "echo hello".to_string(),
            line: 2,
        }],
    )]);
    let output = generate(&cookfile);
    assert!(!output.contains("cook.layer"), "shell steps should not use cook.layer");
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test codegen::tests::test_cook_step_one_to_one_has_layer -- --nocapture`
Expected: FAIL (current codegen doesn't emit `cook.layer`)

- [ ] **Step 3: Modify codegen to wrap cook steps in cook.layer**

Update `generate_cook_step` in `src/codegen/mod.rs`. At the top of the file, add the import:

```rust
use crate::cache::check::hash_str;
```

Modify the `OneToOne` arm of `generate_cook_step` (lines 135-180):

```rust
CookMode::OneToOne => {
    out.push_str(&format!("    local _cook_outputs_{} = {{}}\n", index));
    out.push_str(&format!(
        "    for _, _cook_in in ipairs({}) do\n",
        input_source
    ));
    out.push_str("        local _cook_stem = path.stem(_cook_in)\n");
    out.push_str("        local _cook_name = path.name(_cook_in)\n");
    out.push_str("        local _cook_ext = path.ext(_cook_in)\n");
    out.push_str("        local _cook_dir = path.dir(_cook_in)\n");

    let out_expr = expand_output_pattern(&cook_step.output_pattern);
    out.push_str(&format!("        local _cook_out = {}\n", out_expr));

    // Compute command hash at codegen time
    let cmd_hash = match &cook_step.using_clause {
        Some(UsingClause::Shell(cmd)) => hash_str(cmd),
        Some(UsingClause::LuaBlock(code)) => hash_str(code),
        None => 0,
    };

    out.push_str(&format!(
        "        cook.layer(_cook_in, _cook_out, {}, function()\n",
        cmd_hash
    ));

    match &cook_step.using_clause {
        Some(UsingClause::Shell(cmd)) => {
            let lua_expr = expand_template_to_lua(cmd);
            out.push_str(&format!(
                "            cook.exec({}, {})\n",
                lua_expr, line
            ));
        }
        Some(UsingClause::LuaBlock(code)) => {
            out.push_str("            local input = _cook_in\n");
            out.push_str("            local output = _cook_out\n");
            for (i, _) in ingredients.iter().enumerate() {
                out.push_str(&format!(
                    "            local input_{} = recipe.ingredients[{}]\n",
                    i + 1,
                    i + 1
                ));
            }
            for code_line in code.lines() {
                out.push_str(&format!("            {}\n", code_line));
            }
        }
        None => {}
    }

    out.push_str("        end)\n");
    out.push_str(&format!(
        "        table.insert(_cook_outputs_{}, _cook_out)\n",
        index
    ));
    out.push_str("    end\n");
}
```

Modify the `ManyToOne` arm (lines 182-201):

```rust
CookMode::ManyToOne => {
    out.push_str(&format!("    local _cook_outputs_{} = {{}}\n", index));
    out.push_str(&format!(
        "    local _cook_all = table.concat({}, \" \")\n",
        input_source
    ));

    let out_expr = expand_output_pattern(&cook_step.output_pattern);
    out.push_str(&format!("    local _cook_out = {}\n", out_expr));

    let cmd_hash = match &cook_step.using_clause {
        Some(UsingClause::Shell(cmd)) => hash_str(cmd),
        Some(UsingClause::LuaBlock(code)) => hash_str(code),
        None => 0,
    };

    out.push_str(&format!(
        "    cook.layer({}, _cook_out, {}, function()\n",
        input_source, cmd_hash
    ));

    if let Some(UsingClause::Shell(cmd)) = &cook_step.using_clause {
        let lua_expr = expand_template_to_lua(cmd);
        out.push_str(&format!("        cook.exec({}, {})\n", lua_expr, line));
    }

    out.push_str("    end)\n");
    out.push_str(&format!(
        "    table.insert(_cook_outputs_{}, _cook_out)\n",
        index
    ));
}
```

Modify `generate_plate_step` (lines 205-227):

```rust
fn generate_plate_step(
    out: &mut String,
    plate_step: &PlateStep,
    line: usize,
    last_cook_index: Option<usize>,
) {
    let source = if let Some(idx) = last_cook_index {
        format!("_cook_outputs_{}", idx)
    } else {
        "recipe.ingredients[1]".to_string()
    };

    let cmd_hash = hash_str(&plate_step.command);
    let cmd_expr = expand_plate_cmd(&plate_step.command);

    out.push_str(&format!(
        "    for _, _plate_out in ipairs({}) do\n",
        source
    ));
    out.push_str(&format!(
        "        cook.layer(_plate_out, nil, {}, function()\n",
        cmd_hash
    ));
    out.push_str(&format!(
        "            cook.exec({}, {})\n",
        cmd_expr, line
    ));
    out.push_str("        end)\n");
    out.push_str("    end\n");
}
```

- [ ] **Step 4: Update existing codegen tests that check exact output**

Several existing tests assert on exact `cook.exec(` lines that are now indented differently due to the layer wrapper. Update the assertions in:
- `test_cook_step_one_to_one`: add indentation to expected strings
- `test_cook_step_many_to_one`: add indentation to expected strings
- `test_plate_step`: add layer wrapper expectations

- [ ] **Step 5: Run all codegen tests**

Run: `cargo test codegen`
Expected: all tests pass

- [ ] **Step 6: Commit**

```bash
git add src/codegen/mod.rs
git commit -m "feat(codegen): wrap cook/plate steps in cook.layer() with command hash"
```

---

## Chunk 4: Runtime cook.layer() API

### Task 7: Register cook.layer in the Lua runtime

**Files:**
- Modify: `src/cache/mod.rs`
- Modify: `src/runtime/api.rs`
- Modify: `src/runtime/mod.rs`

- [ ] **Step 1: Add RecipeCacheState to cache/mod.rs**

This is the mutable state shared between the Lua runtime and the cache system during a recipe execution:

```rust
pub mod store;
pub mod check;

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use store::RecipeCache;

/// Mutable cache state shared with the Lua runtime during recipe execution.
pub struct CacheState {
    pub cache: RecipeCache,
    pub cache_dir: PathBuf,
    pub recipe_name: String,
    pub dirty: bool,
}

impl CacheState {
    pub fn new(cache: RecipeCache, cache_dir: PathBuf, recipe_name: String) -> Self {
        Self {
            cache,
            cache_dir,
            recipe_name,
            dirty: false,
        }
    }

    /// Flush cache to disk if dirty. Resets dirty flag on success.
    pub fn flush(&mut self) -> std::io::Result<()> {
        if self.dirty {
            self.cache.save(&self.cache_dir, &self.recipe_name)?;
            self.dirty = false;
        }
        Ok(())
    }
}

pub type SharedCacheState = Rc<RefCell<CacheState>>;
```

- [ ] **Step 2: Register cook.layer in api.rs**

Add a new function `register_layer_api` in `src/runtime/api.rs`:

```rust
pub fn register_layer_api(
    lua: &Lua,
    cache_state: SharedCacheState,
    working_dir: &Path,
) -> LuaResult<()> {
    let cook: LuaTable = lua.globals().get("cook")?;
    let wd = working_dir.to_path_buf();

    let layer_fn = lua.create_function(
        move |_, (inputs, output, command_hash, body): (LuaValue, LuaValue, u64, LuaFunction)| {
            // Collect inputs as RELATIVE path strings (matching cache storage)
            let input_strs: Vec<String> = match &inputs {
                LuaValue::String(s) => vec![s.to_str().unwrap_or("").to_string()],
                LuaValue::Table(t) => {
                    let mut strs = Vec::new();
                    for val in t.clone().sequence_values::<String>() {
                        if let Ok(s) = val { strs.push(s); }
                    }
                    strs
                }
                _ => vec![],
            };

            // Output as relative path string (nil for plate)
            let output_str: Option<String> = match &output {
                LuaValue::String(s) => Some(s.to_str().unwrap_or("").to_string()),
                LuaValue::Nil => None,
                _ => None,
            };

            let input_refs: Vec<&str> = input_strs.iter().map(|s| s.as_str()).collect();

            // Cache key: output path for cook, first input path for plate
            let cache_key = output_str.as_deref()
                .or(input_strs.first().map(|s| s.as_str()))
                .unwrap_or("")
                .to_string();

            let mut state = cache_state.borrow_mut();
            let existing = state.cache.steps.get(&cache_key);

            let (result, updated_entry) = if let Some(ref out) = output_str {
                crate::cache::check::needs_rebuild_cook(
                    existing, &input_refs, out, command_hash, &wd,
                )
            } else {
                crate::cache::check::needs_rebuild_plate(
                    existing, &input_refs, command_hash, &wd,
                )
            };

            // If skip, update cache entry if mtimes changed, flush per-layer
            if let crate::cache::check::RebuildResult::Skip = &result {
                if let Some(entry) = updated_entry {
                    state.cache.steps.insert(cache_key, entry);
                    state.dirty = true;
                    let _ = state.flush(); // incremental persistence
                }
                drop(state);
                return Ok(());
            }

            // Need to rebuild — drop borrow before calling Lua body
            drop(state);

            // Execute the body
            body.call::<()>(())?;

            // After successful execution, record new cache entry
            let mut state = cache_state.borrow_mut();
            let new_inputs: Vec<crate::cache::store::FileRecord> = input_strs
                .iter()
                .map(|rel| {
                    let abs = wd.join(rel);
                    crate::cache::store::FileRecord {
                        path: rel.clone(),
                        mtime: crate::cache::check::stat_mtime(&abs).unwrap_or(0),
                        hash: crate::cache::check::hash_file(&abs).unwrap_or(0),
                    }
                })
                .collect();

            let new_output = output_str.as_ref().map(|rel| {
                let abs = wd.join(rel);
                crate::cache::store::FileRecord {
                    path: rel.clone(),
                    mtime: crate::cache::check::stat_mtime(&abs).unwrap_or(0),
                    hash: crate::cache::check::hash_file(&abs).unwrap_or(0),
                }
            });

            state.cache.steps.insert(
                cache_key,
                crate::cache::store::StepEntry {
                    inputs: new_inputs,
                    output: new_output,
                    command_hash,
                },
            );
            state.dirty = true;
            let _ = state.flush(); // incremental persistence per layer

            Ok(())
        },
    )?;

    cook.set("layer", layer_fn)?;
    Ok(())
}
```

Note: `cache_state` is cloned into the closure via `Rc`. The `register_layer_api` needs to import `use crate::cache::SharedCacheState;` at the top.

- [ ] **Step 3: Modify Runtime to load/save cache per recipe**

In `src/runtime/mod.rs`, update `execute_recipe` to create cache state and register `cook.layer`:

```rust
use crate::cache::{CacheState, SharedCacheState};
use crate::cache::store::RecipeCache;
```

In `execute_recipe`, the ordering must be:
1. Create Lua VM, register cook/fs/path APIs (existing)
2. **Register cook.layer API** (new — must happen before lua.load since generated Lua calls cook.layer)
3. `lua.load(lua_source).exec()` (existing — registers recipes)
4. **Recipe-level invalidation** (Task 8 — needs registered recipe metadata)
5. **Glob result tracking** (Task 9 — needs recipe metadata)
6. Build recipe context / resolve ingredients (existing)
7. Call recipe function (existing)
8. Final cache flush (new)

After creating the Lua VM and registering existing APIs, add cache setup:

```rust
// Load or create recipe cache
let cache_dir = self.working_dir.join(".cook").join("cache");
let cache = RecipeCache::load(&cache_dir, recipe_name).unwrap_or_else(RecipeCache::new);
let cache_state: SharedCacheState = Rc::new(RefCell::new(
    CacheState::new(cache, cache_dir, recipe_name.to_string())
));

api::register_layer_api(&lua, cache_state.clone(), &self.working_dir)?;
```

After successful recipe execution (before the `Ok(())` return), do a final flush in case any mtime updates haven't been written yet:

```rust
// Final cache flush
cache_state.borrow_mut().flush().map_err(|e| {
    RuntimeError::Lua(mlua::Error::runtime(format!("cache flush failed: {e}")))
})?;
```

Note: most cache writes happen per-layer inside `cook.layer()`. This final flush catches any remaining dirty state.

Add the necessary imports at the top of `runtime/mod.rs`:

```rust
use std::cell::RefCell;
use std::rc::Rc;
```

- [ ] **Step 4: Run existing tests to ensure nothing breaks**

Run: `cargo test`
Expected: all existing tests pass. Note: unit tests in runtime/mod.rs use hand-written Lua (no cook.layer calls), so they're unaffected. Integration tests use codegen output (which now emits cook.layer), but cook.layer is registered by the runtime, so they work end-to-end. **Important:** Between Task 6 (codegen changes) and this task, integration tests will fail because cook.layer is referenced in generated Lua but not yet registered. Both tasks should be completed together.

- [ ] **Step 5: Commit**

```bash
git add src/cache/mod.rs src/runtime/api.rs src/runtime/mod.rs
git commit -m "feat(runtime): register cook.layer() API with cache integration"
```

---

## Chunk 5: Recipe-Level Invalidation + env/secondary inputs hashing

### Task 8: Env hash and secondary ingredients hash

**Files:**
- Modify: `src/runtime/mod.rs`
- Modify: `src/cache/check.rs`

- [ ] **Step 1: Write hash_env and hash_secondary_inputs functions**

Add to `src/cache/check.rs`:

```rust
/// Hash a sorted env var map into a single u64.
pub fn hash_env(env: &std::collections::HashMap<String, String>) -> u64 {
    let mut pairs: Vec<(&str, &str)> = env.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
    pairs.sort();
    let combined: String = pairs.iter().map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>().join("\n");
    hash_str(&combined)
}

/// Hash all files matching secondary ingredient globs (index > 0).
/// Returns a combined hash of all file contents.
/// Sorts paths for deterministic ordering regardless of filesystem.
pub fn hash_secondary_inputs(working_dir: &Path, ingredients: &[String]) -> u64 {
    if ingredients.len() <= 1 {
        return 0;
    }

    let mut entries: Vec<(String, u64)> = Vec::new();
    for pattern in &ingredients[1..] {
        let full = working_dir.join(pattern).to_string_lossy().to_string();
        if let Ok(paths) = glob::glob(&full) {
            let mut sorted_paths: Vec<_> = paths.filter_map(|p| p.ok()).collect();
            sorted_paths.sort();
            for entry in sorted_paths {
                if let Some(h) = hash_file(&entry) {
                    entries.push((entry.to_string_lossy().to_string(), h));
                }
            }
        }
    }

    // Combine hashes deterministically
    let bytes: Vec<u8> = entries.iter().flat_map(|(_, h)| h.to_le_bytes()).collect();
    xxhash_rust::xxh3::xxh3_64(&bytes)
}
```

Add tests:

```rust
#[test]
fn test_hash_env_deterministic() {
    let mut env = std::collections::HashMap::new();
    env.insert("A".to_string(), "1".to_string());
    env.insert("B".to_string(), "2".to_string());
    assert_eq!(hash_env(&env), hash_env(&env));
}

#[test]
fn test_hash_env_order_independent() {
    let mut env1 = std::collections::HashMap::new();
    env1.insert("B".to_string(), "2".to_string());
    env1.insert("A".to_string(), "1".to_string());

    let mut env2 = std::collections::HashMap::new();
    env2.insert("A".to_string(), "1".to_string());
    env2.insert("B".to_string(), "2".to_string());

    assert_eq!(hash_env(&env1), hash_env(&env2));
}

#[test]
fn test_hash_env_differs_on_value_change() {
    let mut env1 = std::collections::HashMap::new();
    env1.insert("A".to_string(), "1".to_string());

    let mut env2 = std::collections::HashMap::new();
    env2.insert("A".to_string(), "2".to_string());

    assert_ne!(hash_env(&env1), hash_env(&env2));
}

#[test]
fn test_hash_secondary_no_secondary() {
    let dir = tempfile::tempdir().unwrap();
    let ingredients = vec!["src/*.c".to_string()];
    assert_eq!(hash_secondary_inputs(dir.path(), &ingredients), 0);
}

#[test]
fn test_hash_secondary_detects_change() {
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir_all(dir.path().join("include")).unwrap();
    fs::write(dir.path().join("include/a.h"), "v1").unwrap();

    let ingredients = vec!["src/*.c".to_string(), "include/*.h".to_string()];
    let h1 = hash_secondary_inputs(dir.path(), &ingredients);

    fs::write(dir.path().join("include/a.h"), "v2").unwrap();
    let h2 = hash_secondary_inputs(dir.path(), &ingredients);

    assert_ne!(h1, h2);
}
```

- [ ] **Step 2: Integrate into execute_recipe**

In `src/runtime/mod.rs`, after loading the cache and before executing the recipe, add recipe-level invalidation checks:

```rust
// Recipe-level invalidation: check env hash and secondary ingredients hash
let current_env_hash = crate::cache::check::hash_env(&self.env_vars);
let ingredients: Vec<String> = {
    let registry = recipes.borrow();
    registry.iter()
        .find(|r| r.name == recipe_name)
        .map(|r| r.metadata.ingredients.clone())
        .unwrap_or_default()
};
let current_secondary_hash = crate::cache::check::hash_secondary_inputs(
    &self.working_dir,
    &ingredients,
);

{
    let mut state = cache_state.borrow_mut();
    if state.cache.env_hash != current_env_hash
        || state.cache.secondary_inputs_hash != current_secondary_hash
    {
        // Full invalidation
        state.cache = RecipeCache::new();
        state.dirty = true;
    }
    state.cache.env_hash = current_env_hash;
    state.cache.secondary_inputs_hash = current_secondary_hash;
}
```

Note: This code runs after `lua.load(lua_source).exec()` (which registers recipes) and after the `recipes` are available, but before `func.call()`.

- [ ] **Step 3: Run tests**

Run: `cargo test`
Expected: all tests pass

- [ ] **Step 4: Commit**

```bash
git add src/cache/check.rs src/runtime/mod.rs
git commit -m "feat(cache): add recipe-level invalidation for env vars and secondary ingredients"
```

---

## Chunk 6: Glob Tracking + .gitignore

### Task 9: Populate and check RecipeCache.globs

**Files:**
- Modify: `src/runtime/mod.rs`

- [ ] **Step 1: Record glob results into cache after ingredient resolution**

After resolving ingredient globs in `execute_recipe`, store the results in the cache. Add this after the ingredients_table loop:

```rust
// Record glob results for new-file detection
{
    let mut state = cache_state.borrow_mut();
    for (i, pattern) in recipe.metadata.ingredients.iter().enumerate() {
        let full_pattern = self.working_dir.join(pattern).to_string_lossy().to_string();
        let prefix = self.working_dir.to_string_lossy().to_string();
        let mut files: Vec<String> = Vec::new();
        if let Ok(paths) = glob::glob(&full_pattern) {
            for entry in paths.filter_map(|p| p.ok()) {
                let path_str = entry.to_string_lossy().to_string();
                let relative = path_str
                    .strip_prefix(&prefix)
                    .unwrap_or(&path_str)
                    .trim_start_matches('/')
                    .to_string();
                files.push(relative);
            }
        }
        files.sort();
        let cached = state.cache.globs.get(pattern);
        if cached != Some(&files) {
            // Glob results changed — prune stale step entries for removed files
            if let Some(old_files) = cached {
                for old_file in old_files {
                    if !files.contains(old_file) {
                        // Remove any step entries keyed on this file
                        state.cache.steps.retain(|_, entry| {
                            !entry.inputs.iter().any(|f| &f.path == old_file)
                        });
                    }
                }
            }
            state.cache.globs.insert(pattern.clone(), files);
            state.dirty = true;
        }
    }
}
```

- [ ] **Step 2: Run tests**

Run: `cargo test`
Expected: all tests pass

- [ ] **Step 3: Commit**

```bash
git add src/runtime/mod.rs
git commit -m "feat(cache): track glob results for new/removed file detection"
```

---

### Task 10: Add .cook/ to .gitignore

**Files:**
- Modify: `.gitignore` (create if not exists)

- [ ] **Step 1: Add .cook/ to .gitignore**

Append to `.gitignore`:

```
.cook/
```

- [ ] **Step 2: Commit**

```bash
git add .gitignore
git commit -m "chore: add .cook/ to .gitignore"
```

---

## Chunk 7: Integration Tests

### Task 11: End-to-end cache integration tests

**Files:**
- Modify: `tests/integration.rs`

- [ ] **Step 1: Write integration test for basic cache skip**

```rust
#[test]
fn test_cache_skips_unchanged_cook_step() {
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir_all(dir.path().join("src")).unwrap();
    fs::write(dir.path().join("src/a.c"), "int main() {}").unwrap();
    fs::write(
        dir.path().join("Cookfile"),
        r#"recipe "build"
    ingredients "src/*.c"
    cook "build/{stem}.o" using "cp {in} {out}"
end"#,
    )
    .unwrap();
    fs::create_dir_all(dir.path().join("build")).unwrap();

    // First build
    let output = cook_cmd().current_dir(dir.path()).output().unwrap();
    assert!(output.status.success(), "first build failed: {}", String::from_utf8_lossy(&output.stderr));
    assert!(dir.path().join("build/a.o").exists());

    // Second build — should skip (output exists, input unchanged)
    let output2 = cook_cmd().current_dir(dir.path()).output().unwrap();
    assert!(output2.status.success(), "second build failed: {}", String::from_utf8_lossy(&output2.stderr));

    // Verify cache directory was created
    assert!(dir.path().join(".cook/cache").exists());
}

#[test]
fn test_cache_rebuilds_on_input_change() {
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir_all(dir.path().join("src")).unwrap();
    fs::write(dir.path().join("src/a.c"), "v1").unwrap();
    fs::write(
        dir.path().join("Cookfile"),
        r#"recipe "build"
    ingredients "src/*.c"
    cook "build/{stem}.txt" using "cp {in} {out}"
end"#,
    )
    .unwrap();
    fs::create_dir_all(dir.path().join("build")).unwrap();

    // First build
    let output = cook_cmd().current_dir(dir.path()).output().unwrap();
    assert!(output.status.success());
    let content1 = fs::read_to_string(dir.path().join("build/a.txt")).unwrap();
    assert_eq!(content1, "v1");

    // Modify input
    std::thread::sleep(std::time::Duration::from_millis(50));
    fs::write(dir.path().join("src/a.c"), "v2").unwrap();

    // Second build — should rebuild
    let output2 = cook_cmd().current_dir(dir.path()).output().unwrap();
    assert!(output2.status.success());
    let content2 = fs::read_to_string(dir.path().join("build/a.txt")).unwrap();
    assert_eq!(content2, "v2");
}

#[test]
fn test_cache_rebuilds_on_output_deleted() {
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir_all(dir.path().join("src")).unwrap();
    fs::write(dir.path().join("src/a.c"), "content").unwrap();
    fs::write(
        dir.path().join("Cookfile"),
        r#"recipe "build"
    ingredients "src/*.c"
    cook "build/{stem}.txt" using "cp {in} {out}"
end"#,
    )
    .unwrap();
    fs::create_dir_all(dir.path().join("build")).unwrap();

    // First build
    cook_cmd().current_dir(dir.path()).output().unwrap();
    assert!(dir.path().join("build/a.txt").exists());

    // Delete output
    fs::remove_file(dir.path().join("build/a.txt")).unwrap();

    // Second build — should rebuild since output missing
    cook_cmd().current_dir(dir.path()).output().unwrap();
    assert!(dir.path().join("build/a.txt").exists());
}

#[test]
fn test_cache_invalidates_on_env_change() {
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir_all(dir.path().join("src")).unwrap();
    fs::write(dir.path().join("src/a.c"), "content").unwrap();
    fs::write(dir.path().join(".env"), "MY_VAR=v1").unwrap();
    fs::write(
        dir.path().join("Cookfile"),
        r#"recipe "build"
    ingredients "src/*.c"
    cook "build/{stem}.txt" using "echo {in} > {out}"
end"#,
    )
    .unwrap();
    fs::create_dir_all(dir.path().join("build")).unwrap();

    // First build
    cook_cmd().current_dir(dir.path()).output().unwrap();

    // Change env
    fs::write(dir.path().join(".env"), "MY_VAR=v2").unwrap();

    // Second build — should invalidate entire cache
    let output = cook_cmd().current_dir(dir.path()).output().unwrap();
    assert!(output.status.success());
}
```

- [ ] **Step 2: Run integration tests**

Run: `cargo test --test integration`
Expected: all tests pass (new and existing)

- [ ] **Step 3: Commit**

```bash
git add tests/integration.rs
git commit -m "test: add integration tests for incremental build cache"
```

---

### Task 12: Clean up and final verification

**Files:** None (verification only)

- [ ] **Step 1: Run full test suite**

Run: `cargo test`
Expected: all tests pass

- [ ] **Step 2: Run with --emit-lua to verify generated code looks correct**

Create a test Cookfile and run:

```bash
cd examples && cook --emit-lua
```

Verify the output contains `cook.layer(` calls wrapping cook steps.

- [ ] **Step 3: Run a real build twice to verify caching**

```bash
cd examples && cook build && cook build
```

Second run should be noticeably faster (shell commands skipped).

- [ ] **Step 4: Commit any final fixes**

```bash
git add -A
git commit -m "chore: final cleanup for incremental cache implementation"
```
