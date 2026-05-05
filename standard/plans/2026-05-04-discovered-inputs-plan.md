# Discovered Inputs and Depfile-as-Implicit-Output — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Apply the design in `standard/specs/2026-05-04-discovered-inputs-design.md` (commit `c02b4cc`). After this plan executes, `cook.add_unit` accepts `discovered_inputs = { from, format }`; the engine parses Make-format depfiles before checks (to fatten `current_inputs`) and after executes (to augment `StepEntry.inputs` and upload the depfile as an implicit restorable artifact); `cpp.lua` no longer parses depfiles itself; `examples/lua-build`'s warmup collapses from 3 runs to 2.

**Architecture:** Five layers, in dependency order. (1) `cook-contracts::CacheMeta` gains an `Option<DiscoveredInputs>` field. (2) `cook-cache::depfile` is a new module owning Make-format parsing. (3) `cook-fingerprint::check::needs_rebuild_cook` gains an optional `discovered_inputs` parameter and pre-check augmentation. (4) `cook-register::unit_api` reads the new Lua field. (5) `cook-engine::executor` adds post-execution augmentation and depfile artifact upload at both call sites. Three sibling `cpp.lua` copies under `examples/` are slimmed in lockstep. Standard amendments to §6 and §8 close the spec-first hook. TDD throughout: failing test → minimal implementation → green test → commit. Schema unchanged; `CACHE_VERSION` does NOT bump.

**Tech Stack:** Rust (edition 2021, workspace at `cli/`), `cargo test`, `mlua` for Lua 5.4 runtime, MDX (Astro Starlight) for the Standard, Lua Cookfile fixtures under `examples/`.

---

## Working directory and prerequisites

All paths are relative to `/home/alex/dev/cook` unless noted.

Confirm the spec-first hook is installed:

```bash
git -C /home/alex/dev/cook config --get core.hooksPath
# Expected: .githooks
```

If empty: `git -C /home/alex/dev/cook config core.hooksPath .githooks`. The design doc at `standard/specs/2026-05-04-discovered-inputs-design.md` is already committed (`c02b4cc`) — that satisfies the hook's spec-pairing requirement for everything in this plan.

Confirm a clean working tree before starting:

```bash
git -C /home/alex/dev/cook status --short
# Expected: empty output
```

## Per-task verification commands

| Scope | Command | Expected |
|---|---|---|
| Contracts unit tests | `cd cli && cargo test -p cook-contracts` | clean |
| Cache unit tests | `cd cli && cargo test -p cook-cache --lib` | clean |
| Cache integration tests | `cd cli && cargo test -p cook-cache --test '*'` | clean |
| Fingerprint unit tests | `cd cli && cargo test -p cook-fingerprint` | clean |
| Register unit tests | `cd cli && cargo test -p cook-register` | clean |
| Engine unit tests | `cd cli && cargo test -p cook-engine --lib` | clean |
| Whole CLI test suite | `cd cli && cargo test` | clean |
| Spec build | `cd standard && pnpm build` | exit 0 |
| `lua-build` smoke (manual; final) | `cd examples/lua-build && cook clean && cook && cook` | run 2 reports `(N nodes, N cached)` for `.o` nodes |

The spec build is only relevant after Tasks 17–18 (Standard amendments). Tasks 1–16 do not touch `standard/src/`.

## File structure

| File | Responsibility | Tasks |
|---|---|---|
| `cli/crates/cook-contracts/src/lib.rs` | `DiscoveredInputs` struct + `CacheMeta.discovered_inputs: Option<DiscoveredInputs>` | 1 |
| `cli/crates/cook-cache/src/depfile.rs` (new) | Make-format depfile parser; `DepfileError` enum | 2, 3 |
| `cli/crates/cook-cache/src/lib.rs` | Re-export `depfile::parse_make_depfile` and `DepfileError` | 2 |
| `cli/crates/cook-cache/src/manager.rs` | `record_completion` appends depfile to `StepEntry.outputs` when `discovered_inputs.is_some()` | 9 |
| `cli/crates/cook-fingerprint/src/check.rs` | `needs_rebuild_cook` signature change + pre-check augmentation | 5, 6 |
| `cli/crates/cook-fingerprint/src/lib.rs` | Re-export of types if needed | 5 |
| `cli/crates/cook-register/src/unit_api.rs` | Read `discovered_inputs` Lua table; populate `CacheMeta.discovered_inputs`; validate `from` and `format` | 7, 8 |
| `cli/crates/cook-engine/src/executor.rs` | Thread `discovered_inputs` into the cache check; post-execution augmentation; depfile artifact upload | 5, 10, 11 |
| `cli/crates/cook-dag-viewer/src/dag_data.rs` | Update `needs_rebuild_cook` call site (compile-driven) | 5 |
| `cli/crates/cook-cache/tests/integration_restore_on_hit.rs` | Update `needs_rebuild_cook` call sites (4 calls) | 5 |
| `cli/crates/cook-cache/tests/integration_multi_output_restore.rs` | Update `needs_rebuild_cook` call sites (2 calls) | 5 |
| `cli/crates/cook-cache/tests/integration_discovered_inputs_warmup.rs` (new) | Three-run warmup scenario at the cache layer | 13 |
| `cli/crates/cook-cache/tests/integration_discovered_inputs_restore.rs` (new) | Restore depfile and outputs from backend after disk wipe | 14 |
| `examples/lua-build/cook_modules/cpp.lua` | Remove `parse_depfile`; pass `discovered_inputs` at three call sites | 15 |
| `examples/cpp-project/cook_modules/cpp.lua` | Same slim-down | 15 |
| `examples/fzf-picker/cook_modules/cpp.lua` | Same slim-down | 15 |
| `standard/src/content/docs/06-cook-lua-api.mdx` | §6.2 add `discovered_inputs` row + new normative subsection | 17 |
| `standard/src/content/docs/08-execution-model.mdx` | New §{exec.cache.discovered-inputs} subsection | 18 |

No file is fully deleted in this plan; legacy code (`parse_depfile` in `cpp.lua`) is removed in Task 15.

---

## Task 1: Add `DiscoveredInputs` and extend `CacheMeta`

**Files:**
- Modify: `cli/crates/cook-contracts/src/lib.rs`
- Test: `cli/crates/cook-contracts/src/lib.rs` (existing `#[cfg(test)] mod tests`)

- [x] **Step 1.1: Write the failing test**

In `cli/crates/cook-contracts/src/lib.rs`, append to the existing test module (after `cache_meta_no_output`):

```rust
#[test]
fn cache_meta_construction_with_discovered_inputs() {
    let m = CacheMeta {
        recipe_name: "compile".into(),
        project_id: "p".into(),
        cookfile_path: "Cookfile".into(),
        cache_key: "k".into(),
        input_paths: vec!["src/a.c".into()],
        output_paths: vec!["build/a.o".into()],
        command_hash: 0xdead,
        context_hash: 0,
        env_contribution: 0,
        consulted_env: std::collections::BTreeMap::new(),
        discovered_inputs: Some(DiscoveredInputs {
            from: ".cook/deps/a.d".into(),
            format: "make".into(),
        }),
    };
    let di = m.discovered_inputs.as_ref().expect("present");
    assert_eq!(di.from, ".cook/deps/a.d");
    assert_eq!(di.format, "make");
}

#[test]
fn cache_meta_default_discovered_inputs_is_none() {
    let m = CacheMeta {
        recipe_name: "r".into(),
        project_id: "p".into(),
        cookfile_path: "Cookfile".into(),
        cache_key: "k".into(),
        input_paths: vec![],
        output_paths: vec![],
        command_hash: 0,
        context_hash: 0,
        env_contribution: 0,
        consulted_env: std::collections::BTreeMap::new(),
        discovered_inputs: None,
    };
    assert!(m.discovered_inputs.is_none());
}
```

- [x] **Step 1.2: Run the test — verify it fails**

```bash
cd cli && cargo test -p cook-contracts cache_meta_construction_with_discovered_inputs
```

Expected: compile error on `DiscoveredInputs` not in scope and `discovered_inputs` field missing on `CacheMeta`.

- [x] **Step 1.3: Add the `DiscoveredInputs` struct and field**

In `cli/crates/cook-contracts/src/lib.rs`, immediately before the `CacheMeta` struct definition:

```rust
/// Declarative description of post-execution input discovery for a unit.
///
/// When present on a [`CacheMeta`], the engine MUST:
///   - Read the file at [`Self::from`] (relative to the unit's working
///     directory) before composing the cache check's `current_inputs`,
///     parsing it under [`Self::format`].
///   - After successful execution, parse the file again and append its
///     contents to the recorded `StepEntry.inputs`.
///   - Treat the file as an implicit restorable output: uploaded under
///     its own artifact key, restored on a hit-with-drifted-outputs check.
///
/// The only currently supported `format` is `"make"`. See the design at
/// `standard/specs/2026-05-04-discovered-inputs-design.md`.
#[derive(Debug, Clone, PartialEq)]
pub struct DiscoveredInputs {
    pub from: String,
    pub format: String,
}
```

Add the new field to `CacheMeta` (preserving the documented field order):

```rust
pub struct CacheMeta {
    pub recipe_name: String,
    pub project_id: String,
    pub cookfile_path: String,
    pub cache_key: String,
    pub input_paths: Vec<String>,
    pub output_paths: Vec<String>,
    pub command_hash: u64,
    pub context_hash: u64,
    pub env_contribution: u64,
    pub consulted_env: std::collections::BTreeMap<String, String>,
    pub discovered_inputs: Option<DiscoveredInputs>,
}
```

- [x] **Step 1.4: Update existing tests in this file to set the new field**

Search the file for `CacheMeta {` and add `discovered_inputs: None,` to every existing literal. There are five such literals at line offsets approximately: 266 (`cache_meta_construction`), 286 (`cache_meta_no_output`), and three more in the later integration-style tests around line 389. Each gets `discovered_inputs: None,` after `consulted_env`.

- [x] **Step 1.5: Run the tests — verify they pass**

```bash
cd cli && cargo test -p cook-contracts
```

Expected: all green.

- [x] **Step 1.6: Update other crates that construct `CacheMeta` literals**

The contracts crate change cascades. Find every other call site:

```bash
cd cli && cargo build -p cook-contracts
cd cli && cargo build 2>&1 | grep -E 'missing field|error\[E0063\]' | head -40
```

Each error names a file and line. For every `CacheMeta { ... }` literal found, append `discovered_inputs: None,` to the field list. Likely sites: `cli/crates/cook-register/src/unit_api.rs` (the `Some(CacheMeta { ... })` block around line 201), `cli/crates/cook-register/src/tests.rs` (test fixtures), and any other test fixtures.

- [x] **Step 1.7: Verify the whole workspace builds**

```bash
cd cli && cargo build
```

Expected: exit 0.

- [x] **Step 1.8: Commit**

```bash
cd /home/alex/dev/cook
git add cli/crates/cook-contracts/src/lib.rs cli/crates/cook-register/src/unit_api.rs cli/crates/cook-register/src/tests.rs
# add any other files surfaced by the build cascade
git commit -m "feat(contracts): add DiscoveredInputs and CacheMeta.discovered_inputs

Addition is purely structural; no engine consumes the field yet.
Defaults to None at every existing call site.

Spec: standard/specs/2026-05-04-discovered-inputs-design.md §4.1
Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 2: Add `cook-cache::depfile` module skeleton and re-export

**Files:**
- Create: `cli/crates/cook-cache/src/depfile.rs`
- Modify: `cli/crates/cook-cache/src/lib.rs`

- [x] **Step 2.1: Write the failing test (single skeleton)**

Create `cli/crates/cook-cache/src/depfile.rs` with **only** the public surface and a `todo!()` body, plus a module-level test:

```rust
//! Make-format depfile parser. See the design at
//! `standard/specs/2026-05-04-discovered-inputs-design.md` §4.3.

use std::io;
use std::path::Path;

/// Result of attempting to read a Make-format depfile.
#[derive(Debug)]
pub enum DepfileError {
    NotFound,
    Io(io::Error),
    Malformed { byte_offset: usize, reason: String },
}

impl std::fmt::Display for DepfileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DepfileError::NotFound => write!(f, "depfile not found"),
            DepfileError::Io(e) => write!(f, "depfile io error: {e}"),
            DepfileError::Malformed { byte_offset, reason } => {
                write!(f, "depfile malformed at byte {byte_offset}: {reason}")
            }
        }
    }
}

impl std::error::Error for DepfileError {}

/// Parse a Make-format depfile.
///
/// Filter rules (see design §4.3):
///   - Strip the leading target text up to and including the first `:`.
///   - Join continuation lines (`\\\n` and `\\\r\n`).
///   - Skip entries beginning with `/` (absolute paths).
///   - Skip entries equal to `source_path`.
///   - Skip entries whose path does not exist on disk relative to `working_dir`.
///
/// `source_path` may be the empty string (no self-skip).
pub fn parse_make_depfile(
    depfile_path: &Path,
    source_path: &str,
    working_dir: &Path,
) -> Result<Vec<String>, DepfileError> {
    todo!("Task 4")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn returns_not_found_for_missing_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let result = parse_make_depfile(
            &dir.path().join("nonexistent.d"),
            "src/a.c",
            dir.path(),
        );
        assert!(matches!(result, Err(DepfileError::NotFound)));
    }
}
```

In `cli/crates/cook-cache/src/lib.rs`, add the module and re-export. Find the existing `pub mod` declarations at the top and add:

```rust
pub mod depfile;

pub use depfile::{parse_make_depfile, DepfileError};
```

- [x] **Step 2.2: Run the test — verify it fails on `todo!()`**

```bash
cd cli && cargo test -p cook-cache --lib depfile::tests::returns_not_found_for_missing_file
```

Expected: PANIC with "not yet implemented: Task 4". The module compiles; the panic confirms the path through to the function works.

- [x] **Step 2.3: Verify the workspace still builds**

```bash
cd cli && cargo build
```

Expected: exit 0.

- [x] **Step 2.4: Commit**

```bash
cd /home/alex/dev/cook
git add cli/crates/cook-cache/src/depfile.rs cli/crates/cook-cache/src/lib.rs
git commit -m "feat(cook-cache): add depfile module skeleton

Public surface only; parse_make_depfile is todo!() pending Task 4.
Re-exported from the crate root for ergonomic engine and check use.

Spec: standard/specs/2026-05-04-discovered-inputs-design.md §4.3
Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 3: Write parser tests against fixtures (failing)

**Files:**
- Modify: `cli/crates/cook-cache/src/depfile.rs` (test module)

- [x] **Step 3.1: Add the parser fixture tests**

Append to the `mod tests` in `cli/crates/cook-cache/src/depfile.rs`:

```rust
    use std::fs;

    fn write_file(dir: &Path, rel: &str, content: &str) {
        let abs = dir.join(rel);
        if let Some(parent) = abs.parent() {
            fs::create_dir_all(parent).expect("mkdir");
        }
        fs::write(&abs, content).expect("write");
    }

    #[test]
    fn parses_single_line_depfile() {
        let dir = tempfile::tempdir().expect("tempdir");
        let wd = dir.path();
        write_file(wd, "src/a.c", "// source\n");
        write_file(wd, "include/a.h", "#pragma once\n");
        write_file(wd, ".cook/deps/a.d", "build/a.o: src/a.c include/a.h\n");

        let paths = parse_make_depfile(
            &wd.join(".cook/deps/a.d"),
            "src/a.c",
            wd,
        )
        .expect("ok");

        assert_eq!(paths, vec!["include/a.h".to_string()]);
    }

    #[test]
    fn joins_continuation_lines() {
        let dir = tempfile::tempdir().expect("tempdir");
        let wd = dir.path();
        write_file(wd, "src/a.c", "");
        write_file(wd, "include/a.h", "");
        write_file(wd, "include/b.h", "");
        write_file(wd, ".cook/deps/a.d",
            "build/a.o: src/a.c \\\n  include/a.h \\\n  include/b.h\n");

        let paths = parse_make_depfile(
            &wd.join(".cook/deps/a.d"),
            "src/a.c",
            wd,
        )
        .expect("ok");

        assert_eq!(paths, vec!["include/a.h".to_string(), "include/b.h".to_string()]);
    }

    #[test]
    fn skips_absolute_paths() {
        let dir = tempfile::tempdir().expect("tempdir");
        let wd = dir.path();
        write_file(wd, "src/a.c", "");
        write_file(wd, "include/a.h", "");
        write_file(wd, ".cook/deps/a.d",
            "build/a.o: src/a.c /usr/include/stdio.h include/a.h\n");

        let paths = parse_make_depfile(
            &wd.join(".cook/deps/a.d"),
            "src/a.c",
            wd,
        )
        .expect("ok");

        assert_eq!(paths, vec!["include/a.h".to_string()]);
    }

    #[test]
    fn skips_source_self_reference() {
        let dir = tempfile::tempdir().expect("tempdir");
        let wd = dir.path();
        write_file(wd, "src/a.c", "");
        write_file(wd, "include/a.h", "");
        write_file(wd, ".cook/deps/a.d",
            "build/a.o: src/a.c include/a.h src/a.c\n");

        let paths = parse_make_depfile(
            &wd.join(".cook/deps/a.d"),
            "src/a.c",
            wd,
        )
        .expect("ok");

        assert_eq!(paths, vec!["include/a.h".to_string()]);
    }

    #[test]
    fn skips_nonexistent_paths() {
        let dir = tempfile::tempdir().expect("tempdir");
        let wd = dir.path();
        write_file(wd, "src/a.c", "");
        write_file(wd, "include/exists.h", "");
        write_file(wd, ".cook/deps/a.d",
            "build/a.o: src/a.c include/exists.h include/missing.h\n");

        let paths = parse_make_depfile(
            &wd.join(".cook/deps/a.d"),
            "src/a.c",
            wd,
        )
        .expect("ok");

        assert_eq!(paths, vec!["include/exists.h".to_string()]);
    }

    #[test]
    fn empty_source_path_disables_self_skip() {
        let dir = tempfile::tempdir().expect("tempdir");
        let wd = dir.path();
        write_file(wd, "src/a.c", "");
        write_file(wd, ".cook/deps/a.d",
            "build/a.o: src/a.c\n");

        let paths = parse_make_depfile(
            &wd.join(".cook/deps/a.d"),
            "",
            wd,
        )
        .expect("ok");

        assert_eq!(paths, vec!["src/a.c".to_string()]);
    }

    #[test]
    fn malformed_no_colon_returns_error() {
        let dir = tempfile::tempdir().expect("tempdir");
        let wd = dir.path();
        write_file(wd, ".cook/deps/a.d", "no colon here at all\n");

        let result = parse_make_depfile(
            &wd.join(".cook/deps/a.d"),
            "src/a.c",
            wd,
        );

        assert!(matches!(result, Err(DepfileError::Malformed { .. })));
    }

    #[test]
    fn deduplicates_repeated_paths() {
        let dir = tempfile::tempdir().expect("tempdir");
        let wd = dir.path();
        write_file(wd, "src/a.c", "");
        write_file(wd, "include/a.h", "");
        write_file(wd, ".cook/deps/a.d",
            "build/a.o: src/a.c include/a.h include/a.h\n");

        let paths = parse_make_depfile(
            &wd.join(".cook/deps/a.d"),
            "src/a.c",
            wd,
        )
        .expect("ok");

        assert_eq!(paths, vec!["include/a.h".to_string()]);
    }
```

- [x] **Step 3.2: Run the tests — verify they fail on `todo!()`**

```bash
cd cli && cargo test -p cook-cache --lib depfile::tests
```

Expected: every test panics with "not yet implemented: Task 4" except `returns_not_found_for_missing_file` from Task 2 (which still passes because the function returns NotFound before reaching `todo!()` — actually no, it doesn't; the early return is what we'll implement in Task 4. As of now all eight tests panic).

- [x] **Step 3.3: Commit (failing tests as specification)**

```bash
cd /home/alex/dev/cook
git add cli/crates/cook-cache/src/depfile.rs
git commit -m "test(cook-cache): pin depfile parser semantics with fixtures

Eight failing tests cover the filter rules from the design:
single line, continuation joins, absolute-path skip, source self-skip,
nonexistent skip, empty source bypass, malformed input, dedupe.

Spec: standard/specs/2026-05-04-discovered-inputs-design.md §4.3
Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 4: Implement `parse_make_depfile`

**Files:**
- Modify: `cli/crates/cook-cache/src/depfile.rs`

- [x] **Step 4.1: Replace the `todo!()` body with the parser**

In `cli/crates/cook-cache/src/depfile.rs`, replace the `parse_make_depfile` body:

```rust
pub fn parse_make_depfile(
    depfile_path: &Path,
    source_path: &str,
    working_dir: &Path,
) -> Result<Vec<String>, DepfileError> {
    let content = match std::fs::read_to_string(depfile_path) {
        Ok(s) => s,
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            return Err(DepfileError::NotFound);
        }
        Err(e) => return Err(DepfileError::Io(e)),
    };

    // Locate the first ':' separating the target from the prerequisites.
    let colon_pos = match content.find(':') {
        Some(p) => p,
        None => {
            return Err(DepfileError::Malformed {
                byte_offset: 0,
                reason: "no ':' separating target from prerequisites".to_string(),
            });
        }
    };

    // Strip target text and any leading whitespace after the colon.
    let after_colon = &content[colon_pos + 1..];

    // Join continuation lines: '\\\r\n' and '\\\n' both become a single space.
    let joined = after_colon
        .replace("\\\r\n", " ")
        .replace("\\\n", " ");

    // Tokenise on any whitespace and apply filter rules. Preserve first-occurrence order.
    let mut seen = std::collections::HashSet::new();
    let mut out: Vec<String> = Vec::new();

    for token in joined.split_whitespace() {
        if token.is_empty() {
            continue;
        }
        // Filter: skip absolute paths.
        if token.starts_with('/') {
            continue;
        }
        // Filter: skip the source itself.
        if !source_path.is_empty() && token == source_path {
            continue;
        }
        // Filter: skip non-existent paths (relative to working_dir).
        let abs = working_dir.join(token);
        if !abs.exists() {
            continue;
        }
        // Dedupe.
        if seen.insert(token.to_string()) {
            out.push(token.to_string());
        }
    }

    Ok(out)
}
```

- [x] **Step 4.2: Run the parser tests — verify they pass**

```bash
cd cli && cargo test -p cook-cache --lib depfile::tests
```

Expected: all eight tests pass.

- [x] **Step 4.3: Commit**

```bash
cd /home/alex/dev/cook
git add cli/crates/cook-cache/src/depfile.rs
git commit -m "feat(cook-cache): implement Make-format depfile parser

Pure content-only parser: read file, split on first ':', join continuations,
tokenise, apply filters (absolute / source-self / nonexistent), dedupe.
Eight unit tests cover the documented semantics.

Spec: standard/specs/2026-05-04-discovered-inputs-design.md §4.3
Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 5: Plumb `discovered_inputs` parameter through `needs_rebuild_cook`

This task is signature-only — no behaviour change. It updates the function signature and every call site to pass `None`. Pre-check augmentation is implemented in Task 6.

**Files:**
- Modify: `cli/crates/cook-fingerprint/src/check.rs`
- Modify: `cli/crates/cook-engine/src/executor.rs:392`
- Modify: `cli/crates/cook-dag-viewer/src/dag_data.rs:253`
- Modify: `cli/crates/cook-cache/tests/integration_restore_on_hit.rs` (4 calls)
- Modify: `cli/crates/cook-cache/tests/integration_multi_output_restore.rs` (2 calls)

- [x] **Step 5.1: Update the function signature in `check.rs`**

In `cli/crates/cook-fingerprint/src/check.rs`, modify the `pub fn needs_rebuild_cook` signature:

```rust
pub fn needs_rebuild_cook(
    entry: Option<&StepEntry>,
    current_inputs: &[&str],
    current_outputs: &[&str],
    command_hash: u64,
    context_hash: u64,
    env_contribution: u64,
    working_dir: &Path,
    restore_ctx: Option<&RestoreCtx>,
    discovered_inputs: Option<&cook_contracts::DiscoveredInputs>,
) -> (RebuildResult, Option<StepEntry>) {
    // ... existing body unchanged ...
}
```

`cook_contracts` is already a workspace dependency of `cook-fingerprint`. Verify with:

```bash
grep '^cook-contracts' cli/crates/cook-fingerprint/Cargo.toml
```

If missing, add `cook-contracts = { path = "../cook-contracts" }` to `[dependencies]`.

- [x] **Step 5.2: Update all in-file test calls to pass `None`**

Within `check.rs`, the in-module tests around lines 404, 428, 454, 478, 515, 614, 637 each call `needs_rebuild_cook(...)`. Append `, None` (the new ninth argument) to each call. Use this surgical sed-like edit:

```bash
cd /home/alex/dev/cook
# Verify line counts before mass edit
grep -c 'needs_rebuild_cook(' cli/crates/cook-fingerprint/src/check.rs
# Expected: a small integer (~9 — function def + calls)
```

For each occurrence (other than the `pub fn needs_rebuild_cook(` line that is the definition), replace the trailing `None);` (the previous restore_ctx) with `None, None);` and the trailing `Some(&...));` with `Some(&...), None);`. The simplest approach is to open `check.rs` and do this manually, one call site at a time.

- [x] **Step 5.3: Update `cook-engine::executor` call site (line ~392)**

In `cli/crates/cook-engine/src/executor.rs` around line 392, change the call:

```rust
let (result, updated) = needs_rebuild_cook(
    entry,
    &input_refs,
    &current_outputs,
    meta.command_hash,
    meta.context_hash,
    meta.env_contribution,
    &work_node.working_dir,
    Some(&restore_ctx),
    meta.discovered_inputs.as_ref(),  // NEW
);
```

This wires through `meta.discovered_inputs` (currently always `None` since nothing populates it yet — that lands in Task 7).

- [x] **Step 5.4: Update `cook-dag-viewer::dag_data` call site (line ~253)**

In `cli/crates/cook-dag-viewer/src/dag_data.rs` around line 253, append `, None` to the `needs_rebuild_cook(...)` call (the dag-viewer doesn't consume `CacheMeta`, so passing `None` is correct).

- [x] **Step 5.5: Update integration test call sites**

In `cli/crates/cook-cache/tests/integration_restore_on_hit.rs`, append `, None` to each of the four `needs_rebuild_cook(...)` calls (around lines 101, 155, 256, 312).

In `cli/crates/cook-cache/tests/integration_multi_output_restore.rs`, append `, None` to each of the two `needs_rebuild_cook(...)` calls (around lines 110, 193).

- [x] **Step 5.6: Build and run the whole CLI test suite**

```bash
cd cli && cargo test
```

Expected: all green. No behaviour has changed — the new parameter is `None` everywhere.

- [x] **Step 5.7: Commit**

```bash
cd /home/alex/dev/cook
git add cli/crates/cook-fingerprint/src/check.rs \
        cli/crates/cook-engine/src/executor.rs \
        cli/crates/cook-dag-viewer/src/dag_data.rs \
        cli/crates/cook-cache/tests/integration_restore_on_hit.rs \
        cli/crates/cook-cache/tests/integration_multi_output_restore.rs
# also stage the Cargo.toml if dependency was added
git add cli/crates/cook-fingerprint/Cargo.toml || true
git commit -m "refactor(cook-fingerprint): plumb discovered_inputs param through needs_rebuild_cook

Signature-only change. Every call site passes None except the executor,
which threads meta.discovered_inputs.as_ref() so the field flows from
CacheMeta. Augmentation logic lands in Task 6.

Spec: standard/specs/2026-05-04-discovered-inputs-design.md §5.1
Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 6: Implement pre-check augmentation in `needs_rebuild_cook`

**Files:**
- Modify: `cli/crates/cook-fingerprint/src/check.rs`
- Test: `cli/crates/cook-fingerprint/src/check.rs` (existing test module)

- [x] **Step 6.1: Write the failing test for the augmentation path**

In the test module of `cli/crates/cook-fingerprint/src/check.rs`, append:

```rust
    #[test]
    fn augments_current_inputs_from_depfile_and_skips() {
        use cook_contracts::DiscoveredInputs;

        let dir = tempfile::tempdir().expect("tempdir");
        let wd = dir.path();
        // Lay out source, header, and a depfile that references both.
        std::fs::write(wd.join("src.c"), b"src").expect("src");
        std::fs::write(wd.join("hdr.h"), b"hdr").expect("hdr");
        std::fs::create_dir_all(wd.join(".cook/deps")).expect("mkdir");
        std::fs::write(
            wd.join(".cook/deps/src.d"),
            b"build/src.o: src.c hdr.h\n",
        ).expect("d");
        std::fs::write(wd.join("out.o"), b"obj").expect("out");

        // Build a stored entry that already has the fat input set.
        let src_hash = cook_fingerprint::hash_file(&wd.join("src.c")).unwrap();
        let hdr_hash = cook_fingerprint::hash_file(&wd.join("hdr.h")).unwrap();
        let out_hash = cook_fingerprint::hash_file(&wd.join("out.o")).unwrap();

        let entry = StepEntry {
            inputs: vec![
                FileRecord { path: "src.c".into(), mtime: 0, hash: src_hash },
                FileRecord { path: "hdr.h".into(), mtime: 0, hash: hdr_hash },
            ],
            outputs: vec![FileRecord {
                path: "out.o".into(),
                mtime: cook_fingerprint::stat_mtime(&wd.join("out.o")).unwrap_or(0),
                hash: out_hash,
            }],
            command_hash: 0xc0de,
            context_hash: 0,
            env_contribution: 0,
        };

        let di = DiscoveredInputs {
            from: ".cook/deps/src.d".into(),
            format: "make".into(),
        };

        // Caller passes only the declared input.
        let (result, _updated) = needs_rebuild_cook(
            Some(&entry),
            &["src.c"],
            &["out.o"],
            0xc0de,
            0,
            0,
            wd,
            None,
            Some(&di),
        );

        assert!(matches!(result, RebuildResult::Skip),
            "augmented current_inputs (declared + discovered) should match the fat entry");
    }

    #[test]
    fn missing_depfile_falls_back_to_thin_inputs() {
        use cook_contracts::DiscoveredInputs;

        let dir = tempfile::tempdir().expect("tempdir");
        let wd = dir.path();
        std::fs::write(wd.join("src.c"), b"src").expect("src");
        std::fs::write(wd.join("hdr.h"), b"hdr").expect("hdr");
        std::fs::write(wd.join("out.o"), b"obj").expect("out");

        let src_hash = cook_fingerprint::hash_file(&wd.join("src.c")).unwrap();
        let hdr_hash = cook_fingerprint::hash_file(&wd.join("hdr.h")).unwrap();
        let out_hash = cook_fingerprint::hash_file(&wd.join("out.o")).unwrap();

        let entry = StepEntry {
            inputs: vec![
                FileRecord { path: "src.c".into(), mtime: 0, hash: src_hash },
                FileRecord { path: "hdr.h".into(), mtime: 0, hash: hdr_hash },
            ],
            outputs: vec![FileRecord {
                path: "out.o".into(),
                mtime: cook_fingerprint::stat_mtime(&wd.join("out.o")).unwrap_or(0),
                hash: out_hash,
            }],
            command_hash: 0xc0de,
            context_hash: 0,
            env_contribution: 0,
        };

        let di = DiscoveredInputs {
            from: ".cook/deps/src.d".into(),  // does not exist
            format: "make".into(),
        };

        let (result, _) = needs_rebuild_cook(
            Some(&entry),
            &["src.c"],
            &["out.o"],
            0xc0de,
            0,
            0,
            wd,
            None,
            Some(&di),
        );

        // Augmentation no-ops; current=[src.c] vs entry=[src.c, hdr.h] → InputSetChanged.
        assert!(matches!(result, RebuildResult::Rebuild(RebuildReason::InputSetChanged)));
    }
```

- [x] **Step 6.2: Run the new tests — verify they fail**

```bash
cd cli && cargo test -p cook-fingerprint augments_current_inputs_from_depfile_and_skips missing_depfile_falls_back_to_thin_inputs
```

Expected: both fail. The first reports `Rebuild(InputSetChanged)` because no augmentation happens yet (current is thin, entry is fat). The second already passes by accident (the no-augmentation behaviour matches what the test expects). Note that explicitly: it's the first test that should drive the implementation.

- [x] **Step 6.3: Implement pre-check augmentation**

In `cli/crates/cook-fingerprint/src/check.rs`, locate the existing line:

```rust
    let updated_inputs = match check_inputs(&entry.inputs, current_inputs, working_dir) {
        Err(reason) => return (RebuildResult::Rebuild(reason), None),
        Ok(u) => u,
    };
```

Replace it with the augmentation block:

```rust
    // Pre-check augmentation: when the unit declares discovered_inputs and
    // a prior depfile is on disk, fatten current_inputs by the discovered
    // paths so the entry's input set matches. A missing or malformed depfile
    // is no-augmentation (fallthrough to InputSetChanged → rebuild → self-heal).
    let augmented_storage: Vec<String>;
    let augmented_refs: Vec<&str>;
    let current_inputs_for_check: &[&str] = if let Some(di) = discovered_inputs {
        let source_for_skip = current_inputs.first().copied().unwrap_or("");
        match crate::__depfile_call::parse(
            &working_dir.join(&di.from),
            source_for_skip,
            working_dir,
            &di.format,
        ) {
            Ok(discovered_paths) => {
                augmented_storage = current_inputs
                    .iter()
                    .map(|s| (*s).to_string())
                    .chain(discovered_paths.into_iter())
                    .collect();
                augmented_refs = augmented_storage.iter().map(String::as_str).collect();
                &augmented_refs
            }
            Err(_) => current_inputs,
        }
    } else {
        current_inputs
    };

    let updated_inputs = match check_inputs(&entry.inputs, current_inputs_for_check, working_dir) {
        Err(reason) => return (RebuildResult::Rebuild(reason), None),
        Ok(u) => u,
    };
```

The `crate::__depfile_call::parse` indirection avoids an outright dependency from `cook-fingerprint` on `cook-cache`. Add a small adapter module at the top of `check.rs`:

```rust
mod __depfile_call {
    use std::path::Path;

    /// Function pointer the engine installs at startup. `cook-fingerprint`
    /// does not depend on `cook-cache`; the engine wires the real parser
    /// before any check fires (see cook-engine::executor).
    static PARSER: std::sync::OnceLock<
        fn(&Path, &str, &Path, &str) -> Result<Vec<String>, ()>,
    > = std::sync::OnceLock::new();

    pub fn install(parser: fn(&Path, &str, &Path, &str) -> Result<Vec<String>, ()>) {
        let _ = PARSER.set(parser);
    }

    pub fn parse(
        depfile_path: &Path,
        source_path: &str,
        working_dir: &Path,
        format: &str,
    ) -> Result<Vec<String>, ()> {
        match PARSER.get() {
            Some(p) => p(depfile_path, source_path, working_dir, format),
            None => Err(()),
        }
    }
}

pub use __depfile_call::install as install_depfile_parser;
```

This keeps the dependency graph clean (no cycle between fingerprint and cache) and lets tests install a parser.

- [x] **Step 6.4: Install the parser in the test**

Update the new tests in Step 6.1 to install the real parser before calling `needs_rebuild_cook`:

```rust
    fn install_real_parser_once() {
        use std::sync::OnceLock;
        static ONCE: OnceLock<()> = OnceLock::new();
        ONCE.get_or_init(|| {
            crate::install_depfile_parser(|p, s, wd, fmt| {
                if fmt != "make" { return Err(()); }
                cook_cache::parse_make_depfile(p, s, wd).map_err(|_| ())
            });
        });
    }
```

Add `install_real_parser_once();` as the first line of both `augments_current_inputs_from_depfile_and_skips` and `missing_depfile_falls_back_to_thin_inputs`.

The fingerprint crate now needs `cook-cache` as a dev-dependency (test-only — does not introduce a runtime cycle). Add to `cli/crates/cook-fingerprint/Cargo.toml`:

```toml
[dev-dependencies]
cook-cache = { path = "../cook-cache" }
```

- [x] **Step 6.5: Run the augmentation tests — verify they pass**

```bash
cd cli && cargo test -p cook-fingerprint augments_current_inputs_from_depfile_and_skips missing_depfile_falls_back_to_thin_inputs
```

Expected: both pass.

- [x] **Step 6.6: Run the entire fingerprint suite to confirm no regressions**

```bash
cd cli && cargo test -p cook-fingerprint
```

Expected: all green.

- [x] **Step 6.7: Commit**

```bash
cd /home/alex/dev/cook
git add cli/crates/cook-fingerprint/src/check.rs cli/crates/cook-fingerprint/Cargo.toml
git commit -m "feat(cook-fingerprint): pre-check augmentation from prior depfile

When a unit declares discovered_inputs, needs_rebuild_cook reads the
prior-run depfile, parses it, and unions discovered paths into
current_inputs before the existing check_inputs comparison. The parser
itself lives in cook-cache; fingerprint receives a function pointer to
avoid a runtime dep cycle.

Missing or malformed depfile → no augmentation → InputSetChanged →
rebuild → self-heal on the next run.

Spec: standard/specs/2026-05-04-discovered-inputs-design.md §5.1
Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 7: Read `discovered_inputs` Lua field in `cook.add_unit`

**Files:**
- Modify: `cli/crates/cook-register/src/unit_api.rs`
- Test: `cli/crates/cook-register/src/tests.rs`

- [x] **Step 7.1: Write the failing test**

In `cli/crates/cook-register/src/tests.rs`, append:

```rust
#[test]
fn add_unit_reads_discovered_inputs_table() {
    use cook_contracts::CapturedUnit;

    let lua = mlua::Lua::new();
    let captured = std::rc::Rc::new(std::cell::RefCell::new(crate::CaptureState::default()));
    let outputs = std::rc::Rc::new(std::cell::RefCell::new(
        std::collections::BTreeMap::new(),
    ));
    crate::unit_api::register_unit_api(
        &lua,
        captured.clone(),
        outputs,
        "demo".to_string(),
    ).expect("register");

    lua.load(r#"
        cook.add_unit({
            inputs = { "src/a.c" },
            output = "build/a.o",
            command = "gcc -c src/a.c -o build/a.o",
            discovered_inputs = { from = ".cook/deps/a.d", format = "make" },
        })
    "#).exec().expect("exec");

    let st = captured.borrow();
    let unit: &CapturedUnit = st.units.last().expect("one unit");
    let cm = unit.cache_meta.as_ref().expect("cache_meta");
    let di = cm.discovered_inputs.as_ref().expect("discovered_inputs");
    assert_eq!(di.from, ".cook/deps/a.d");
    assert_eq!(di.format, "make");
}
```

- [x] **Step 7.2: Run the test — verify it fails**

```bash
cd cli && cargo test -p cook-register add_unit_reads_discovered_inputs_table
```

Expected: fail. `cm.discovered_inputs` is `None` because nothing reads the field yet.

- [x] **Step 7.3: Read the table in `unit_api.rs`**

In `cli/crates/cook-register/src/unit_api.rs`, locate the `cache_meta` construction (around line 191-215). Immediately before the `let cache_meta = if cache_enabled {` block, add a discovery-table reader:

```rust
        // Read optional discovered_inputs table.
        let discovered_inputs: Option<cook_contracts::DiscoveredInputs> =
            match tbl.get::<LuaValue>("discovered_inputs") {
                Ok(LuaValue::Table(di_tbl)) => {
                    let from: String = di_tbl.get::<String>("from").map_err(|_| {
                        LuaError::RuntimeError(
                            "cook.add_unit: discovered_inputs.from is required and must be a string"
                                .into(),
                        )
                    })?;
                    let format: String = di_tbl.get::<String>("format").map_err(|_| {
                        LuaError::RuntimeError(
                            "cook.add_unit: discovered_inputs.format is required and must be a string"
                                .into(),
                        )
                    })?;
                    if from.is_empty() {
                        return Err(LuaError::RuntimeError(
                            "cook.add_unit: discovered_inputs.from must be non-empty".into(),
                        ));
                    }
                    Some(cook_contracts::DiscoveredInputs { from, format })
                }
                Ok(LuaValue::Nil) | Err(_) => None,
                Ok(_) => {
                    return Err(LuaError::RuntimeError(
                        "cook.add_unit: discovered_inputs must be a table".into(),
                    ));
                }
            };
```

Then thread it into the `CacheMeta` literal:

```rust
        let cache_meta = if cache_enabled {
            // ... existing code ...
            Some(CacheMeta {
                recipe_name: rname.clone(),
                project_id,
                cookfile_path,
                cache_key,
                input_paths: cache_input_paths,
                output_paths: output_paths.clone(),
                command_hash,
                context_hash,
                env_contribution: env_contribution_val,
                consulted_env,
                discovered_inputs,  // NEW
            })
        } else {
            None
        };
```

- [x] **Step 7.4: Run the test — verify it passes**

```bash
cd cli && cargo test -p cook-register add_unit_reads_discovered_inputs_table
```

Expected: pass.

- [x] **Step 7.5: Run the full register suite**

```bash
cd cli && cargo test -p cook-register
```

Expected: all green.

- [x] **Step 7.6: Commit**

```bash
cd /home/alex/dev/cook
git add cli/crates/cook-register/src/unit_api.rs cli/crates/cook-register/src/tests.rs
git commit -m "feat(cook-register): read discovered_inputs from cook.add_unit

When the user passes \`discovered_inputs = { from, format }\`, populate
CacheMeta.discovered_inputs. Format-validation lands in Task 8.

Spec: standard/specs/2026-05-04-discovered-inputs-design.md §4.4
Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 8: Validate `discovered_inputs` shape in `cook.add_unit`

**Files:**
- Modify: `cli/crates/cook-register/src/unit_api.rs`
- Test: `cli/crates/cook-register/src/tests.rs`

- [x] **Step 8.1: Write the failing tests**

In `cli/crates/cook-register/src/tests.rs`, append:

```rust
#[test]
fn add_unit_rejects_unsupported_discovered_inputs_format() {
    let lua = mlua::Lua::new();
    let captured = std::rc::Rc::new(std::cell::RefCell::new(crate::CaptureState::default()));
    let outputs = std::rc::Rc::new(std::cell::RefCell::new(
        std::collections::BTreeMap::new(),
    ));
    crate::unit_api::register_unit_api(
        &lua,
        captured.clone(),
        outputs,
        "demo".to_string(),
    ).expect("register");

    let result = lua.load(r#"
        cook.add_unit({
            inputs = { "x" },
            output = "y",
            command = "true",
            discovered_inputs = { from = "x.d", format = "ninja" },
        })
    "#).exec();

    let err = result.expect_err("expected error").to_string();
    assert!(err.contains("ninja"), "diagnostic must name the unsupported format; got: {err}");
    assert!(err.contains("supported"), "diagnostic must say what is supported; got: {err}");
}

#[test]
fn add_unit_rejects_absolute_discovered_from() {
    let lua = mlua::Lua::new();
    let captured = std::rc::Rc::new(std::cell::RefCell::new(crate::CaptureState::default()));
    let outputs = std::rc::Rc::new(std::cell::RefCell::new(
        std::collections::BTreeMap::new(),
    ));
    crate::unit_api::register_unit_api(
        &lua,
        captured.clone(),
        outputs,
        "demo".to_string(),
    ).expect("register");

    let result = lua.load(r#"
        cook.add_unit({
            inputs = { "x" },
            output = "y",
            command = "true",
            discovered_inputs = { from = "/etc/secrets.d", format = "make" },
        })
    "#).exec();

    let err = result.expect_err("expected error").to_string();
    assert!(err.contains("relative") || err.contains("absolute"),
        "diagnostic must explain the path constraint; got: {err}");
}

#[test]
fn add_unit_rejects_dotdot_discovered_from() {
    let lua = mlua::Lua::new();
    let captured = std::rc::Rc::new(std::cell::RefCell::new(crate::CaptureState::default()));
    let outputs = std::rc::Rc::new(std::cell::RefCell::new(
        std::collections::BTreeMap::new(),
    ));
    crate::unit_api::register_unit_api(
        &lua,
        captured.clone(),
        outputs,
        "demo".to_string(),
    ).expect("register");

    let result = lua.load(r#"
        cook.add_unit({
            inputs = { "x" },
            output = "y",
            command = "true",
            discovered_inputs = { from = "../escape.d", format = "make" },
        })
    "#).exec();

    let err = result.expect_err("expected error").to_string();
    assert!(err.contains(".."), "diagnostic must mention '..'; got: {err}");
}
```

- [x] **Step 8.2: Run the tests — verify they fail**

```bash
cd cli && cargo test -p cook-register add_unit_rejects_
```

Expected: all three fail. The invalid inputs are accepted today.

- [x] **Step 8.3: Add validation to `unit_api.rs`**

Modify the discovered-inputs reader from Task 7 to enforce the rules:

```rust
        let discovered_inputs: Option<cook_contracts::DiscoveredInputs> =
            match tbl.get::<LuaValue>("discovered_inputs") {
                Ok(LuaValue::Table(di_tbl)) => {
                    let from: String = di_tbl.get::<String>("from").map_err(|_| {
                        LuaError::RuntimeError(
                            "cook.add_unit: discovered_inputs.from is required and must be a string"
                                .into(),
                        )
                    })?;
                    let format: String = di_tbl.get::<String>("format").map_err(|_| {
                        LuaError::RuntimeError(
                            "cook.add_unit: discovered_inputs.format is required and must be a string"
                                .into(),
                        )
                    })?;
                    if from.is_empty() {
                        return Err(LuaError::RuntimeError(
                            "cook.add_unit: discovered_inputs.from must be non-empty".into(),
                        ));
                    }
                    if from.starts_with('/') {
                        return Err(LuaError::RuntimeError(format!(
                            "cook.add_unit: discovered_inputs.from must be a relative path; got absolute path {from:?}"
                        )));
                    }
                    if from.split('/').any(|seg| seg == "..") {
                        return Err(LuaError::RuntimeError(format!(
                            "cook.add_unit: discovered_inputs.from must not contain '..' segments; got {from:?}"
                        )));
                    }
                    if format != "make" {
                        return Err(LuaError::RuntimeError(format!(
                            "cook.add_unit: discovered_inputs.format = {format:?} is not supported by this implementation (supported: \"make\")"
                        )));
                    }
                    Some(cook_contracts::DiscoveredInputs { from, format })
                }
                Ok(LuaValue::Nil) | Err(_) => None,
                Ok(_) => {
                    return Err(LuaError::RuntimeError(
                        "cook.add_unit: discovered_inputs must be a table".into(),
                    ));
                }
            };
```

- [x] **Step 8.4: Run the tests — verify they pass**

```bash
cd cli && cargo test -p cook-register add_unit_rejects_
```

Expected: all three pass.

- [x] **Step 8.5: Run the full register suite**

```bash
cd cli && cargo test -p cook-register
```

Expected: all green.

- [x] **Step 8.6: Commit**

```bash
cd /home/alex/dev/cook
git add cli/crates/cook-register/src/unit_api.rs cli/crates/cook-register/src/tests.rs
git commit -m "feat(cook-register): validate discovered_inputs shape in cook.add_unit

Reject absolute paths, '..' segments, empty 'from', and any format
other than 'make'. Diagnostics name the offending value and what is
supported.

Spec: standard/specs/2026-05-04-discovered-inputs-design.md §4.4
Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 9: Append depfile to `StepEntry.outputs` in `record_completion`

**Files:**
- Modify: `cli/crates/cook-cache/src/manager.rs`
- Test: `cli/crates/cook-cache/src/manager.rs` (existing test module)

- [x] **Step 9.1: Write the failing test**

In `cli/crates/cook-cache/src/manager.rs`, append to the `mod tests` section:

```rust
    #[test]
    fn record_completion_appends_depfile_to_outputs() {
        use cook_contracts::DiscoveredInputs;
        use crate::store;

        let dir = tempfile::tempdir().expect("tempdir");
        let wd = dir.path();
        std::fs::write(wd.join("a.c"), b"src").expect("a.c");
        std::fs::write(wd.join("a.o"), b"obj").expect("a.o");
        std::fs::create_dir_all(wd.join(".cook/deps")).expect("mkdir");
        std::fs::write(wd.join(".cook/deps/a.d"), b"a.o: a.c\n").expect("dep");

        let cache_dir = wd.join(".cook/cache");
        std::fs::create_dir_all(&cache_dir).expect("cachedir");
        let mgr = ThreadSafeCacheManager::new(cache_dir.clone());

        let mut meta = make_cache_meta(vec!["a.c".into()], vec!["a.o".into()]);
        meta.discovered_inputs = Some(DiscoveredInputs {
            from: ".cook/deps/a.d".into(),
            format: "make".into(),
        });

        let entry = mgr.record_completion("rec", "k", &meta, wd).expect("rec");

        let output_paths: Vec<&str> =
            entry.outputs.iter().map(|fr| fr.path.as_str()).collect();
        assert!(output_paths.contains(&"a.o"), "user output present");
        assert!(output_paths.contains(&".cook/deps/a.d"),
            "depfile appended to outputs when discovered_inputs is set");
    }
```

This test references the existing `make_cache_meta` helper at `cli/crates/cook-cache/src/manager.rs:175`. If the helper doesn't already initialise `discovered_inputs: None`, Task 1's cascade fixed that already; verify by looking at the helper.

- [x] **Step 9.2: Run the test — verify it fails**

```bash
cd cli && cargo test -p cook-cache --lib record_completion_appends_depfile_to_outputs
```

Expected: fail. The test's last assertion fails because the depfile isn't in `entry.outputs`.

- [x] **Step 9.3: Append the depfile to `new_outputs` in `record_completion`**

In `cli/crates/cook-cache/src/manager.rs` `record_completion`, after the existing:

```rust
        let new_outputs = collect_records(&meta.output_paths, working_dir)
            .map_err(|p| RecordError::UnreadableFile(p))?;
```

Add:

```rust
        let mut new_outputs = new_outputs;
        if let Some(di) = &meta.discovered_inputs {
            // Append the depfile as an implicit output. If the file is
            // missing on disk post-execution, skip silently — the engine's
            // augmentation block (Task 10) handles the warning.
            if let Ok(records) = collect_records(
                &[di.from.clone()],
                working_dir,
            ) {
                if let Some(rec) = records.into_iter().next() {
                    new_outputs.push(rec);
                }
            }
        }
```

`new_outputs` now needs to be `mut`; the rebinding shadow is intentional and idiomatic.

- [x] **Step 9.4: Run the test — verify it passes**

```bash
cd cli && cargo test -p cook-cache --lib record_completion_appends_depfile_to_outputs
```

Expected: pass.

- [x] **Step 9.5: Run the full cache suite**

```bash
cd cli && cargo test -p cook-cache
```

Expected: all green.

- [x] **Step 9.6: Commit**

```bash
cd /home/alex/dev/cook
git add cli/crates/cook-cache/src/manager.rs
git commit -m "feat(cook-cache): record depfile as an implicit output

When a unit's CacheMeta carries discovered_inputs, record_completion
appends the depfile path to StepEntry.outputs so the existing
restore-on-hit walker (2026-05-02 spec §5.2) treats it like any other
output. Backend put + restore wiring lands in Tasks 10 and 11.

Spec: standard/specs/2026-05-04-discovered-inputs-design.md §5.2
Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 10: Post-execution input augmentation in `executor`

**Files:**
- Modify: `cli/crates/cook-engine/src/executor.rs`

This task wires the parser-pointer install (so pre-check augmentation works) and adds the post-execution amend-and-replace block to both `record_completion` call sites.

- [x] **Step 10.1: Install the depfile parser at engine startup**

In `cli/crates/cook-engine/src/executor.rs`, find the public entry point — the function that builds the executor (likely named `run_with_dag` or similar; search for the start of the main exec function). Add at the top of that function (before any node executes):

```rust
    // Install the depfile parser pointer so cook-fingerprint's pre-check
    // augmentation can call back into cook-cache without a runtime dep cycle.
    {
        use std::sync::Once;
        static INSTALL: Once = Once::new();
        INSTALL.call_once(|| {
            cook_fingerprint::install_depfile_parser(|p, src, wd, fmt| {
                if fmt != "make" { return Err(()); }
                cook_cache::parse_make_depfile(p, src, wd).map_err(|_| ())
            });
        });
    }
```

- [x] **Step 10.2: Add the post-execution augmentation block**

Locate both call sites where `record_completion` is invoked and `step_entry` is built (interactive ~1052 and worker ~1219). After each `match cm.record_completion(...)` arm that gets `Ok(step_entry)`, immediately before the existing `let mut sorted_hashes:` line that composes `cloud_key`, insert:

```rust
                                        // Post-execution augmentation: parse the just-written
                                        // depfile and append discovered FileRecords to
                                        // step_entry.inputs, then persist the augmented entry.
                                        let mut step_entry = step_entry;
                                        if let Some(di) = &meta.discovered_inputs {
                                            let working_dir = &dag.node(id).payload().working_dir;
                                            let abs_depfile = working_dir.join(&di.from);
                                            let source_for_skip = meta
                                                .input_paths
                                                .first()
                                                .map(String::as_str)
                                                .unwrap_or("");
                                            match cook_cache::parse_make_depfile(
                                                &abs_depfile,
                                                source_for_skip,
                                                working_dir,
                                            ) {
                                                Ok(discovered_paths) => {
                                                    let strs: Vec<String> = discovered_paths
                                                        .iter()
                                                        .cloned()
                                                        .collect();
                                                    match cook_cache::collect_records_public(&strs, working_dir) {
                                                        Ok(records) => {
                                                            for rec in records {
                                                                step_entry.inputs.push(rec);
                                                            }
                                                            cm.update_step(
                                                                &meta.recipe_name,
                                                                &meta.cache_key,
                                                                step_entry.clone(),
                                                            );
                                                        }
                                                        Err(p) => {
                                                            tracing::warn!(
                                                                "discovered-inputs: failed to hash discovered path '{}'", p
                                                            );
                                                        }
                                                    }
                                                }
                                                Err(e) => {
                                                    tracing::warn!(
                                                        "discovered-inputs: depfile parse failed for '{}': {e}",
                                                        di.from
                                                    );
                                                }
                                            }
                                        }
```

This duplicates between the two call sites is intentional — they live in distinct branches of the executor's match (interactive vs. worker) and the surrounding context differs. Keep them in sync; if they diverge in a future change, the integration test in Task 13 catches it.

- [x] **Step 10.3: Expose `collect_records` from `cook-cache`**

Currently `collect_records` is private to `cli/crates/cook-cache/src/manager.rs:15`. Add a thin public wrapper at the top of that file (near the existing `fn collect_records`):

```rust
/// Public wrapper for [`collect_records`] used by the engine's post-execution
/// augmentation path.
pub fn collect_records_public(
    paths: &[String],
    working_dir: &Path,
) -> Result<Vec<FileRecord>, String> {
    collect_records(paths, working_dir)
}
```

Re-export it in `cli/crates/cook-cache/src/lib.rs`:

```rust
pub use manager::collect_records_public;
```

- [x] **Step 10.4: Build and run the entire CLI test suite**

```bash
cd cli && cargo test
```

Expected: all green. The post-execution path doesn't have a unit test yet — Task 13 covers it via integration.

- [x] **Step 10.5: Commit**

```bash
cd /home/alex/dev/cook
git add cli/crates/cook-engine/src/executor.rs \
        cli/crates/cook-cache/src/manager.rs \
        cli/crates/cook-cache/src/lib.rs
git commit -m "feat(cook-engine): post-execution input augmentation

After record_completion succeeds, parse the just-written depfile,
append discovered FileRecords to step_entry.inputs, and rewrite the
local entry via update_step. Wire the depfile parser pointer at
executor startup so cook-fingerprint's pre-check augmentation has a
callback into cook-cache.

Spec: standard/specs/2026-05-04-discovered-inputs-design.md §5.2 step 1
Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 11: Upload depfile as implicit artifact

**Files:**
- Modify: `cli/crates/cook-engine/src/executor.rs`

- [x] **Step 11.1: Locate the existing per-output upload loop**

In `cli/crates/cook-engine/src/executor.rs`, find the `for (out_idx, output_path) in meta.output_paths.iter().enumerate()` loop (there are two — one in each branch, around line 1079 and around line 1241). Each loop body uploads one user-declared output via `backend.put(&artifact_k, &bytes, &artifact_meta)`.

- [x] **Step 11.2: After the loop, upload the depfile (if any)**

Immediately after each per-output loop, insert the depfile upload block:

```rust
                                        // Upload the depfile as an implicit artifact at index
                                        // outputs.len() so a future restore can pull it back.
                                        if let Some(di) = &meta.discovered_inputs {
                                            let depfile_idx = meta.output_paths.len() as u32;
                                            let working_dir = &dag.node(id).payload().working_dir;
                                            let abs_depfile = working_dir.join(&di.from);
                                            if let Ok(bytes) = std::fs::read(&abs_depfile) {
                                                let artifact_k = artifact_key(
                                                    &cloud_k,
                                                    depfile_idx,
                                                    &di.from,
                                                );
                                                let artifact_meta = ArtifactMeta {
                                                    recipe_namespace: recipe_namespace.clone(),
                                                    command_hash: meta.command_hash,
                                                    context_hash: meta.context_hash,
                                                    env_contribution: meta.env_contribution,
                                                    schema_version: CACHE_VERSION,
                                                    size_bytes: bytes.len() as u64,
                                                    tags: std::collections::BTreeSet::new(),
                                                    consulted_env_keys: meta
                                                        .consulted_env
                                                        .keys()
                                                        .cloned()
                                                        .collect(),
                                                    output_index: depfile_idx,
                                                    output_path: di.from.clone(),
                                                };
                                                if let Err(e) = cache_ctx.backend.put(
                                                    &artifact_k,
                                                    &bytes,
                                                    &artifact_meta,
                                                ) {
                                                    tracing::warn!(
                                                        "cache backend put failed for depfile {}: {e}",
                                                        di.from
                                                    );
                                                }
                                            }
                                        }
```

Verify `CACHE_VERSION` is in scope (already imported in executor.rs:9).

- [x] **Step 11.3: Build the workspace**

```bash
cd cli && cargo build
```

Expected: exit 0.

- [x] **Step 11.4: Run the whole CLI test suite**

```bash
cd cli && cargo test
```

Expected: all green.

- [x] **Step 11.5: Commit**

```bash
cd /home/alex/dev/cook
git add cli/crates/cook-engine/src/executor.rs
git commit -m "feat(cook-engine): upload depfile as implicit artifact

When a unit declares discovered_inputs, after the user-declared output
upload loop, upload the depfile under artifact_key(cloud_k,
outputs.len(), depfile_path). Failure logs and continues per the
existing fire-and-forget contract.

Spec: standard/specs/2026-05-04-discovered-inputs-design.md §5.2 step 2
Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 12: Integration test — three-run warmup at the cache layer

**Files:**
- Create: `cli/crates/cook-cache/tests/integration_discovered_inputs_warmup.rs`

- [x] **Step 12.1: Write the integration test**

Create `cli/crates/cook-cache/tests/integration_discovered_inputs_warmup.rs`:

```rust
//! Three-run warmup scenario at the cache layer (no engine).
//! Asserts: Run 1 = miss, Run 2 = hit (after augmentation), Run 3 = hit
//! after a header content edit triggers InputChanged.

use cook_cache::{parse_make_depfile, store::{FileRecord, StepEntry}};
use cook_contracts::DiscoveredInputs;
use cook_fingerprint::{install_depfile_parser, needs_rebuild_cook, RebuildReason, RebuildResult};
use std::sync::Once;

fn install_parser_once() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        install_depfile_parser(|p, src, wd, fmt| {
            if fmt != "make" { return Err(()); }
            parse_make_depfile(p, src, wd).map_err(|_| ())
        });
    });
}

fn fr(wd: &std::path::Path, rel: &str) -> FileRecord {
    FileRecord {
        path: rel.into(),
        mtime: cook_fingerprint::stat_mtime(&wd.join(rel)).unwrap_or(0),
        hash: cook_fingerprint::hash_file(&wd.join(rel)).unwrap(),
    }
}

#[test]
fn warmup_collapses_to_two_runs() {
    install_parser_once();

    let dir = tempfile::tempdir().expect("tempdir");
    let wd = dir.path();
    std::fs::write(wd.join("a.c"), b"int main(){return 0;}").expect("a.c");
    std::fs::write(wd.join("a.h"), b"#pragma once\n").expect("a.h");
    std::fs::write(wd.join("a.o"), b"obj-bytes").expect("a.o");
    std::fs::create_dir_all(wd.join(".cook/deps")).expect("mkdir");
    std::fs::write(wd.join(".cook/deps/a.d"), b"a.o: a.c a.h\n").expect("d");

    let di = DiscoveredInputs {
        from: ".cook/deps/a.d".into(),
        format: "make".into(),
    };

    // ---- Run 1: NoCacheEntry, simulated execute, store fat StepEntry ----
    let (r1, _) = needs_rebuild_cook(
        None,
        &["a.c"],
        &["a.o"],
        0xc0de,
        0,
        0,
        wd,
        None,
        Some(&di),
    );
    assert!(matches!(r1, RebuildResult::Rebuild(RebuildReason::NoCacheEntry)),
        "fresh check returns NoCacheEntry");

    // Engine post-execution augmentation: build a fat StepEntry.
    let stored_entry = StepEntry {
        inputs: vec![fr(wd, "a.c"), fr(wd, "a.h")],
        outputs: vec![fr(wd, "a.o"), fr(wd, ".cook/deps/a.d")],
        command_hash: 0xc0de,
        context_hash: 0,
        env_contribution: 0,
    };

    // ---- Run 2: pre-check augments current_inputs, equality check skips ----
    let (r2, _) = needs_rebuild_cook(
        Some(&stored_entry),
        &["a.c"],
        &["a.o"],
        0xc0de,
        0,
        0,
        wd,
        None,
        Some(&di),
    );
    assert!(matches!(r2, RebuildResult::Skip),
        "Run 2 should hit (augmented current matches fat entry); got {r2:?}");

    // ---- Run 3: edit header content; expect InputChanged ----
    std::fs::write(wd.join("a.h"), b"#pragma once\n#define X 1\n").expect("a.h v2");

    let (r3, _) = needs_rebuild_cook(
        Some(&stored_entry),
        &["a.c"],
        &["a.o"],
        0xc0de,
        0,
        0,
        wd,
        None,
        Some(&di),
    );
    assert!(matches!(r3, RebuildResult::Rebuild(RebuildReason::InputChanged(_))),
        "Run 3 should rebuild because a.h content changed; got {r3:?}");
}
```

- [x] **Step 12.2: Run the integration test**

```bash
cd cli && cargo test -p cook-cache --test integration_discovered_inputs_warmup
```

Expected: pass. (If you wrote it before the implementation, this would fail — but Tasks 4 + 6 already landed the necessary behaviour.)

- [x] **Step 12.3: Commit**

```bash
cd /home/alex/dev/cook
git add cli/crates/cook-cache/tests/integration_discovered_inputs_warmup.rs
git commit -m "test(cook-cache): three-run warmup integration test

Run 1 = NoCacheEntry → Run 2 = Skip via augmentation → Run 3 =
InputChanged after header edit. Pins the warmup-collapse contract at
the cache layer, independent of the executor.

Spec: standard/specs/2026-05-04-discovered-inputs-design.md §9.2
Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 13: Integration test — depfile restore from backend

**Files:**
- Create: `cli/crates/cook-cache/tests/integration_discovered_inputs_restore.rs`

- [x] **Step 13.1: Write the integration test**

Create `cli/crates/cook-cache/tests/integration_discovered_inputs_restore.rs`:

```rust
//! Restore-on-hit interaction with depfile-as-implicit-output.
//! Setup: a fat entry exists; backend has the .o and .d artifacts.
//! Disk state: both .o and .d are missing.
//! Expectation: needs_rebuild_cook with restore_ctx restores both
//! files and returns Skip.

use cook_cache::{
    backend::{artifact_key, cloud_key, ArtifactMeta, CacheBackend, CloudKeyInputs, LocalBackend},
    parse_make_depfile,
    store::{FileRecord, StepEntry, CACHE_VERSION},
};
use cook_contracts::DiscoveredInputs;
use cook_fingerprint::{install_depfile_parser, needs_rebuild_cook, RebuildResult, RestoreCtx};
use std::sync::Once;

fn install_parser_once() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        install_depfile_parser(|p, src, wd, fmt| {
            if fmt != "make" { return Err(()); }
            parse_make_depfile(p, src, wd).map_err(|_| ())
        });
    });
}

#[test]
fn missing_outputs_and_depfile_are_both_restored() {
    install_parser_once();

    let wd_dir = tempfile::tempdir().expect("wd");
    let wd = wd_dir.path();
    let backend_dir = tempfile::tempdir().expect("backend");
    let backend = LocalBackend::new(backend_dir.path().to_path_buf());

    // Lay out source + header + (initially) .o + .d.
    std::fs::write(wd.join("a.c"), b"src").expect("a.c");
    std::fs::write(wd.join("a.h"), b"hdr").expect("a.h");
    std::fs::write(wd.join("a.o"), b"obj-bytes").expect("a.o");
    std::fs::create_dir_all(wd.join(".cook/deps")).expect("deps");
    std::fs::write(wd.join(".cook/deps/a.d"), b"a.o: a.c a.h\n").expect("d");

    let recipe_namespace = "p/Cookfile::r".to_string();
    let entry = StepEntry {
        inputs: vec![
            FileRecord {
                path: "a.c".into(),
                mtime: 0,
                hash: cook_fingerprint::hash_file(&wd.join("a.c")).unwrap(),
            },
            FileRecord {
                path: "a.h".into(),
                mtime: 0,
                hash: cook_fingerprint::hash_file(&wd.join("a.h")).unwrap(),
            },
        ],
        outputs: vec![
            FileRecord {
                path: "a.o".into(),
                mtime: cook_fingerprint::stat_mtime(&wd.join("a.o")).unwrap_or(0),
                hash: cook_fingerprint::hash_file(&wd.join("a.o")).unwrap(),
            },
            FileRecord {
                path: ".cook/deps/a.d".into(),
                mtime: cook_fingerprint::stat_mtime(&wd.join(".cook/deps/a.d")).unwrap_or(0),
                hash: cook_fingerprint::hash_file(&wd.join(".cook/deps/a.d")).unwrap(),
            },
        ],
        command_hash: 0xc0de,
        context_hash: 0,
        env_contribution: 0,
    };

    // Compose cloud_key from the fat input set.
    let mut sorted: Vec<u64> = entry.inputs.iter().map(|fr| fr.hash).collect();
    sorted.sort();
    let cloud_k = cloud_key(&CloudKeyInputs {
        schema_version: CACHE_VERSION,
        recipe_namespace: &recipe_namespace,
        command_hash: 0xc0de,
        context_hash: 0,
        env_contribution: 0,
        sorted_input_content_hashes: &sorted,
    });

    // Pre-populate backend with both artifacts.
    let obj_bytes = std::fs::read(wd.join("a.o")).unwrap();
    let dep_bytes = std::fs::read(wd.join(".cook/deps/a.d")).unwrap();
    let obj_k = artifact_key(&cloud_k, 0, "a.o");
    let dep_k = artifact_key(&cloud_k, 1, ".cook/deps/a.d");
    let mk_meta = |idx: u32, path: &str, size: u64| ArtifactMeta {
        recipe_namespace: recipe_namespace.clone(),
        command_hash: 0xc0de,
        context_hash: 0,
        env_contribution: 0,
        schema_version: CACHE_VERSION,
        size_bytes: size,
        tags: Default::default(),
        consulted_env_keys: Default::default(),
        output_index: idx,
        output_path: path.to_string(),
    };
    backend.put(&obj_k, &obj_bytes, &mk_meta(0, "a.o", obj_bytes.len() as u64))
        .expect("put obj");
    backend.put(&dep_k, &dep_bytes, &mk_meta(1, ".cook/deps/a.d", dep_bytes.len() as u64))
        .expect("put dep");

    // Wipe .o and .d from the working tree to simulate a partial clean.
    std::fs::remove_file(wd.join("a.o")).expect("rm o");
    std::fs::remove_file(wd.join(".cook/deps/a.d")).expect("rm d");

    let restore_ctx = RestoreCtx {
        backend: &backend,
        recipe_namespace: &recipe_namespace,
    };
    let di = DiscoveredInputs {
        from: ".cook/deps/a.d".into(),
        format: "make".into(),
    };

    let (result, _) = needs_rebuild_cook(
        Some(&entry),
        &["a.c"],
        &["a.o"],
        0xc0de,
        0,
        0,
        wd,
        Some(&restore_ctx),
        Some(&di),
    );

    assert!(matches!(result, RebuildResult::Skip),
        "expected Skip after restoring both output and depfile; got {result:?}");
    assert!(wd.join("a.o").exists(), "a.o restored");
    assert!(wd.join(".cook/deps/a.d").exists(), ".cook/deps/a.d restored");
}
```

- [x] **Step 13.2: Run the test**

```bash
cd cli && cargo test -p cook-cache --test integration_discovered_inputs_restore
```

Expected: pass.

- [x] **Step 13.3: Commit**

```bash
cd /home/alex/dev/cook
git add cli/crates/cook-cache/tests/integration_discovered_inputs_restore.rs
git commit -m "test(cook-cache): depfile restore-on-hit integration

Wipe .o and .d from disk; backend has both. needs_rebuild_cook with
restore_ctx pulls both back and returns Skip. Verifies depfile-as-
implicit-output participates in 2026-05-02 spec §5.2 restore semantics.

Spec: standard/specs/2026-05-04-discovered-inputs-design.md §5.3
Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 14: Slim down `cpp.lua` (lua-build)

**Files:**
- Modify: `examples/lua-build/cook_modules/cpp.lua`

- [ ] **Step 14.1: Remove the `parse_depfile` function**

In `examples/lua-build/cook_modules/cpp.lua`, delete the entire `local function parse_depfile(...) ... end` block at lines 115–132 (function definition + closing). Verify the function is no longer referenced after the next steps.

- [ ] **Step 14.2: Replace the `cpp.compile` call site (line ~327–338)**

In `cpp.compile`, replace:

```lua
    -- Parse existing depfile for header inputs
    local inputs = { source }
    local header_deps = parse_depfile(dep_file, source)
    for _, h in ipairs(header_deps) do
        inputs[#inputs + 1] = h
    end

    cook.add_unit({
        inputs = inputs,
        output = obj_out,
        command = cmd,
    })
```

with:

```lua
    cook.add_unit({
        inputs = { source },
        output = obj_out,
        command = cmd,
        discovered_inputs = {
            from = dep_file,
            format = "make",
        },
    })
```

- [ ] **Step 14.3: Replace the C++20 BMI compile call site (around line ~575–581)**

Find:

```lua
                local bmi_inputs = { mod_src }
                local mod_deps = parse_depfile(".cook/deps/" .. name .. "/" .. mstem .. ".d", mod_src)
                for _, h in ipairs(mod_deps) do bmi_inputs[#bmi_inputs + 1] = h end
                cook.add_unit({ inputs = bmi_inputs, output = bmi_path, command = bmi_cmd })
```

Replace with:

```lua
                cook.add_unit({
                    inputs = { mod_src },
                    output = bmi_path,
                    command = bmi_cmd,
                    discovered_inputs = {
                        from = ".cook/deps/" .. name .. "/" .. mstem .. ".d",
                        format = "make",
                    },
                })
```

- [ ] **Step 14.4: Replace the second BMI compile call site (around line ~821–827)**

Find the parallel call in the second `cook.step_group` block (line ~824). Apply the same transformation as Step 14.3.

- [ ] **Step 14.5: Verify `parse_depfile` is no longer referenced**

```bash
grep -n parse_depfile examples/lua-build/cook_modules/cpp.lua
```

Expected: no output. If anything remains, it's a missed call site — remove it.

- [ ] **Step 14.6: Build the workspace**

```bash
cd cli && cargo build
```

Expected: exit 0.

- [ ] **Step 14.7: Run lua-build end-to-end (manual verification)**

This step requires running the `cook` CLI. From `examples/lua-build`:

```bash
cd examples/lua-build
cargo run --quiet --manifest-path ../../cli/Cargo.toml --bin cook -- clean
cargo run --quiet --manifest-path ../../cli/Cargo.toml --bin cook
cargo run --quiet --manifest-path ../../cli/Cargo.toml --bin cook
```

Expected: the second `cook` invocation reports `(N nodes, N cached)` for every `.o` node — collapsed warmup. If it still reports `0 cached` for the `.o` nodes, augmentation isn't reaching them; revisit Tasks 6, 7, and 10.

- [ ] **Step 14.8: Commit**

```bash
cd /home/alex/dev/cook
git add examples/lua-build/cook_modules/cpp.lua
git commit -m "refactor(cpp.lua): delegate depfile parsing to engine

Remove parse_depfile and replace its three call sites with the
discovered_inputs declaration on cook.add_unit. The engine now owns
Make-format parsing.

Verified manually: examples/lua-build's warmup collapses from three
runs to two.

Spec: standard/specs/2026-05-04-discovered-inputs-design.md §6
Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 15: Slim down sibling `cpp.lua` copies

**Files:**
- Modify: `examples/cpp-project/cook_modules/cpp.lua`
- Modify: `examples/fzf-picker/cook_modules/cpp.lua`

The two sibling copies have the same `parse_depfile` function and the same three call sites (line numbers may differ). Apply the same transformations as Task 14.

- [ ] **Step 15.1: Verify call-site count in each sibling**

```bash
grep -c parse_depfile examples/cpp-project/cook_modules/cpp.lua
grep -c parse_depfile examples/fzf-picker/cook_modules/cpp.lua
```

Expected: 4 each (1 definition + 3 call sites).

- [ ] **Step 15.2: Apply Task 14's edits to `examples/cpp-project/cook_modules/cpp.lua`**

Same removals and same `discovered_inputs` table additions. Use the line-near anchors (`local function parse_depfile`, `local mod_deps = parse_depfile(`, etc.) to locate them.

- [ ] **Step 15.3: Apply Task 14's edits to `examples/fzf-picker/cook_modules/cpp.lua`**

Same.

- [ ] **Step 15.4: Verify no `parse_depfile` references remain in any cpp.lua copy**

```bash
grep -rn parse_depfile examples/
```

Expected: no output.

- [ ] **Step 15.5: Build the workspace**

```bash
cd cli && cargo build
```

Expected: exit 0.

- [ ] **Step 15.6: Commit**

```bash
cd /home/alex/dev/cook
git add examples/cpp-project/cook_modules/cpp.lua examples/fzf-picker/cook_modules/cpp.lua
git commit -m "refactor(cpp.lua): apply depfile slim-down to sibling copies

Same edits as the lua-build copy: remove parse_depfile, declare
discovered_inputs on cook.add_unit. Keeps the three example modules
in lockstep.

Spec: standard/specs/2026-05-04-discovered-inputs-design.md §6
Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 16: Verify entire CLI test suite + lua-build smoke

**Files:** none (verification only)

- [ ] **Step 16.1: Run the whole CLI test suite**

```bash
cd cli && cargo test
```

Expected: clean.

- [ ] **Step 16.2: Run the existing cache_benchmarks verify (must remain green)**

```bash
cd examples/cache_benchmarks
./verify.sh
```

Expected: all scenarios pass.

- [ ] **Step 16.3: Run lua-build three times and inspect counters**

```bash
cd examples/lua-build
cargo run --quiet --manifest-path ../../cli/Cargo.toml --bin cook -- clean
cargo run --quiet --manifest-path ../../cli/Cargo.toml --bin cook 2>&1 | tail -1
cargo run --quiet --manifest-path ../../cli/Cargo.toml --bin cook 2>&1 | tail -1
```

Expected:

- First `cook`: `Finished in <X>s   (37 nodes, 0 cached)`
- Second `cook`: `Finished in <Y>s   (37 nodes, 37 cached)` — every node hits.

If the second run still reports anything less than 37 cached, the augmentation pipeline is broken. Diagnose by adding `tracing::debug!` calls in `cook-fingerprint::check::needs_rebuild_cook` around the augmentation block and the resulting `RebuildResult`.

- [ ] **Step 16.4: No commit (verification step only)**

---

## Task 17: Cook Standard amendment — §6 (Lua API)

**Files:**
- Modify: `standard/src/content/docs/06-cook-lua-api.mdx`

- [ ] **Step 17.1: Locate `cook.add_unit` field table**

In `standard/src/content/docs/06-cook-lua-api.mdx`, find the `cook.add_unit` field table (around lines 51-61). It currently ends with the `ingredient_groups` row.

- [ ] **Step 17.2: Append the `discovered_inputs` row**

Add a new row to the table:

```mdx
| `discovered_inputs` | table         | absent  | If present, declares a file the command writes during the execute phase that records additional input paths the command consumed (§{exec.cache.discovered-inputs}). Subfields: `from` (string, required, working-directory-relative path) and `format` (string, required; the only mandatory format is `"make"`). |
```

- [ ] **Step 17.3: Add a normative subsection after §6.2 examples**

Find the end of §6.2 (just before §6.3 starts). Add a new subsection:

```mdx
### 6.2.X. `discovered_inputs` field [#lua.add-unit-discovered-inputs]

A unit that consumes input files which are not statically known until execute time MAY declare a `discovered_inputs` table on its `cook.add_unit` call:

```lua
cook.add_unit({
    inputs  = { "src/a.c" },
    output  = "build/a.o",
    command = "gcc -c {in} -MMD -MF .cook/deps/a.d -o {out}",
    discovered_inputs = {
        from   = ".cook/deps/a.d",
        format = "make",
    },
})
```

A conforming implementation MUST:

- accept `discovered_inputs` as either absent or a Lua table; reject any non-table value with a register-phase Lua error.
- require both `from` (non-empty string) and `format` (string) when the table is present; reject missing or wrong-typed subfields with a register-phase Lua error.
- require `from` to be a relative path under the unit's working directory; reject absolute paths and paths containing `..` segments.
- support `format = "make"`; MAY support additional formats; MUST raise a register-phase Lua error if `format` names a value the implementation does not recognise. The diagnostic MUST include the offending value and the list of supported values.

The semantic effect of `discovered_inputs` on the cache is specified in §{exec.cache.discovered-inputs}. The `from` path MUST NOT also appear in `outputs[]`; the implementation tracks the file as an implicit cache artifact and surfaces it to neither the using-block `outputs` binding nor `cook.dep_output` resolution.
```

- [ ] **Step 17.4: Build the spec**

```bash
cd standard && pnpm build
```

Expected: exit 0, no `error` lines.

- [ ] **Step 17.5: Commit**

```bash
cd /home/alex/dev/cook
git add standard/src/content/docs/06-cook-lua-api.mdx
git commit -m "spec(§6): document discovered_inputs field on cook.add_unit

Adds the field row to §6.2 and a new normative subsection §6.2.X
covering the 'from'/'format' shape, validation rules, and the
implicit-output semantics. Cross-refs §{exec.cache.discovered-inputs}.

Spec: standard/specs/2026-05-04-discovered-inputs-design.md §7.1
Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 18: Cook Standard amendment — §8 (Execution model)

**Files:**
- Modify: `standard/src/content/docs/08-execution-model.mdx`

- [ ] **Step 18.1: Locate the cache subsection**

In `standard/src/content/docs/08-execution-model.mdx`, find the `§{exec.cache}` subsection. It currently covers cache keys, restore-on-hit (per 2026-05-02 spec), and per-output artifacts.

- [ ] **Step 18.2: Append the new normative subsection**

After the existing cache subsection content, add:

```mdx
### 8.X.Y. Discovered inputs [#exec.cache.discovered-inputs]

A unit whose declaration carries `discovered_inputs` (§{lua.add-unit-discovered-inputs}) participates in post-execution input discovery. When such a unit is checked against an existing cache entry, a conforming implementation:

1. **MUST** attempt to read the file at `discovered_inputs.from` from the unit's working directory before composing the input-content set passed to the cache check. If the file is present and well-formed under `discovered_inputs.format`, the implementation MUST union the discovered paths with the unit's declared `inputs[]` for the purposes of the check.
2. **MUST** treat a missing or malformed discovery file as no augmentation — the check proceeds with the thin (declared-only) input set. A subsequent rebuild MUST regenerate the discovery file.
3. **MUST**, after a successful execution, parse the discovery file at `discovered_inputs.from` and amend the recorded `StepEntry`'s input set to include the discovered paths before persisting the entry to the local cache index.
4. **MUST** treat the discovery file as an implicit cache artifact: uploaded under its own artifact key during commit, restored from the backend on a hit-with-drifted-outputs check (§{exec.cache.restore-on-hit}).

The `"make"` discovery format is the Make rule format produced by GCC's `-MMD` flag and equivalent compiler options. The conforming parser MUST: strip the leading target text up to and including the first `:`; join continuation lines (`\\\n` and `\\\r\n`); ignore entries beginning with `/`; ignore the unit's first declared input; ignore entries whose paths do not exist on disk.
```

- [ ] **Step 18.3: Build the spec**

```bash
cd standard && pnpm build
```

Expected: exit 0.

- [ ] **Step 18.4: Commit**

```bash
cd /home/alex/dev/cook
git add standard/src/content/docs/08-execution-model.mdx
git commit -m "spec(§8): formalize discovered-inputs execution semantics

New normative subsection §{exec.cache.discovered-inputs} captures the
four MUST clauses governing pre-check augmentation, missing-file
fallthrough, post-execution amendment, and depfile-as-implicit-artifact.
Pins the 'make' format's parser rules.

Spec: standard/specs/2026-05-04-discovered-inputs-design.md §7.2
Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 19: Final regression sweep

**Files:** none (verification only)

- [ ] **Step 19.1: Whole CLI test suite**

```bash
cd cli && cargo test
```

Expected: clean.

- [ ] **Step 19.2: Spec build + lint**

```bash
cd standard && pnpm build
```

Expected: exit 0, no error lines.

- [ ] **Step 19.3: cache_benchmarks regression**

```bash
cd examples/cache_benchmarks && ./verify.sh
```

Expected: all scenarios green.

- [ ] **Step 19.4: lua-build collapse confirmation**

```bash
cd examples/lua-build
cargo run --quiet --manifest-path ../../cli/Cargo.toml --bin cook -- clean
RESULT_1=$(cargo run --quiet --manifest-path ../../cli/Cargo.toml --bin cook 2>&1 | tail -1)
RESULT_2=$(cargo run --quiet --manifest-path ../../cli/Cargo.toml --bin cook 2>&1 | tail -1)
echo "Run 1: $RESULT_1"
echo "Run 2: $RESULT_2"
```

Expected:

- Run 1: `... (37 nodes, 0 cached)`
- Run 2: `... (37 nodes, 37 cached)`

- [ ] **Step 19.5: No commit (verification step only)**

If anything fails here, fix in place before declaring the plan complete; do not paper over with skipped tests.

---

## Self-review checklist

The following spec sections have a corresponding task:

| Spec section | Task |
|---|---|
| §3.1 modules touched | covered across Tasks 1–18 |
| §3.2 architectural invariants | preserved by Tasks 6, 9, 10, 11 (cache-miss > poison; backend errors never fail build) |
| §3.3 build-time flow amendments | Tasks 9 (8d-ii), 10 (post-exec augment), 11 (8d-iv) |
| §4.1 DiscoveredInputs / CacheMeta | Task 1 |
| §4.2 StepEntry shape unchanged | preserved (no `CACHE_VERSION` bump, no struct field added) |
| §4.3 depfile parser module | Tasks 2, 3, 4 |
| §4.4 register-phase reading | Tasks 7, 8 |
| §5.1 pre-check augmentation | Tasks 5 (signature), 6 (logic) |
| §5.2 post-exec augmentation + upload | Tasks 9, 10, 11 |
| §5.3 restore interaction | Task 13 (test); behavior is automatic via existing 2026-05-02 try_restore + Task 9's outputs append |
| §5.4 cloud_key composition | unchanged — handled implicitly by Task 10 (augmented step_entry.inputs feeds existing composition path at executor.rs:1056) |
| §5.5 failure modes | covered by Tasks 6, 8, 10, 11 (warning logs + register-phase errors) |
| §6 cpp.lua slim-down | Tasks 14, 15 |
| §7.1 §6 amendment | Task 17 |
| §7.2 §8 amendment | Task 18 |
| §7.3 no grammar change | preserved (no grammar files touched) |
| §8 backwards compatibility | preserved by §4.2 (no schema bump); Tasks 1, 9 use `Option<>` for additive change |
| §9.1 unit tests | Tasks 3, 6, 7, 8, 9 |
| §9.2 integration tests | Tasks 12, 13 |
| §9.3 end-to-end (cache_benchmarks + lua-build) | Tasks 14, 16, 19 |

**Placeholder scan.** No "TBD", "TODO", "implement later", or generic "add error handling". Each step has actual code or actual command.

**Type consistency.**

- `DiscoveredInputs { from, format }` — used identically in Tasks 1, 6, 7, 8, 12, 13.
- `CacheMeta.discovered_inputs: Option<DiscoveredInputs>` — same shape in Task 1 (definition), Task 5 (`meta.discovered_inputs.as_ref()`), Task 7 (`Some(cook_contracts::DiscoveredInputs { ... })`), Task 9 (`if let Some(di) = &meta.discovered_inputs`), Task 10 (`if let Some(di) = &meta.discovered_inputs`).
- `parse_make_depfile(depfile_path: &Path, source_path: &str, working_dir: &Path) -> Result<Vec<String>, DepfileError>` — Task 2 (decl), Task 4 (impl), Task 6 (call via parser pointer), Task 10 (direct call), Tasks 12 + 13 (test calls).
- `install_depfile_parser` — Task 6 (definition), Task 10 (engine install), Tasks 12 + 13 (test install).
- `collect_records_public(paths: &[String], working_dir: &Path)` — Task 10 (decl + use).
- `RebuildResult::{Skip, Rebuild(RebuildReason)}` and `RebuildReason::{NoCacheEntry, InputChanged, InputSetChanged}` — used consistently.

**Scope check.** This plan implements one focused change (discovered inputs + depfile-as-implicit-output) with no orthogonal additions.

---

## Execution handoff

Plan complete and saved to `standard/plans/2026-05-04-discovered-inputs-plan.md`. Two execution options:

1. **Subagent-Driven (recommended)** — I dispatch a fresh subagent per task, review between tasks, fast iteration.
2. **Inline Execution** — Execute tasks in this session using executing-plans, batch execution with checkpoints.

Which approach?
