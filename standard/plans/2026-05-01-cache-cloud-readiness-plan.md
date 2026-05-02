# Cache Cloud-Readiness — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Apply the design in `standard/specs/2026-05-01-cache-cloud-readiness-design.md` (Linear: SHI-140 epic, sub-issues SHI-141..146). Make Cook's cache key correct cross-machine and lock the `CacheBackend` trait seam that Cook Cloud's R2/D1 backend will implement against.

**Architecture:** Nine phases. **Phase 0** lands additive-only foundation pieces (`ExecutionContext`, `EnvDenylist`, `CacheBackend` trait + `LocalBackend`) that don't break compilation. **Phase 1** extends `CacheMeta` in `cook-contracts`. **Phase 2** is the schema v3 cascade in `cook-cache` (bump `CACHE_VERSION`, add `context_hash`/`env_contribution` to `StepEntry`, drop `secondary_inputs_hash`/`env_hash`/`invalidate_recipe`/`invalidate_if_env_changed`, harden `record_completion`). **Phase 3** adds `.cook/cloud.toml` parsing in `cook-engine`. **Phase 4** adds consulted-env enumeration in `cook-luagen`'s template expansion (where the substitution actually lives) and emits the list on `cook.add_unit` tables. **Phase 5** consumes that list in `cook-register`. **Phase 6** wires the engine bootstrap (probe + denylist + backend + cache_ctx) and updates the executor/dag_data lookups. **Phase 7** updates the `cpp` module template with the `# env: ...` tail. **Phase 8** lands the §8.6 Standard amendment and the Appendix B companion paragraph. **Phase 9** lands cross-cutting integration tests.

**Tech Stack:** Rust 2024 (cargo workspace at `cli/`), `xxhash-rust` (existing, xxh3 for local hashing), `sha2` (new dep, SHA-256 for cloud key), `hex` (new dep, hex encoding), `which` (new dep, PATH resolution), `glob` (existing), `serde`+`bincode` (existing, schema persistence), `serde_json` (new dep, ArtifactMeta sidecar), `toml` (new dep, .cook/cloud.toml parsing), `tracing` (existing, warn diagnostics), `tempfile` (existing dev-dep). Tests via `cargo test`.

---

## Working directory and prerequisites

All paths relative to `/home/alex/dev/cook` unless noted. Run from the repo root.

Confirm spec-first hook is installed:

```bash
git -C /home/alex/dev/cook config --get core.hooksPath
# Expected: .githooks
```

If empty: `git -C /home/alex/dev/cook config core.hooksPath .githooks`.

Cargo workspace root is `cli/`. Run cargo commands as:

```bash
cd /home/alex/dev/cook/cli && cargo test -p cook-cache
```

…or chain `cd cli &&` in front of any cargo invocation.

## Per-task verification

Most tasks verify with the cook-cache tests:

```bash
cd /home/alex/dev/cook/cli && cargo test -p cook-cache
```

Cross-crate tasks (Phases 4–6) verify with the full workspace:

```bash
cd /home/alex/dev/cook/cli && cargo build -q && cargo test -q
```

Phase 8 (Standard amendment) additionally verifies:

```bash
cd /home/alex/dev/cook/standard && pnpm build
```

Conformance harness:

```bash
cd /home/alex/dev/cook/cli && cargo test -p cook-lang --test conformance
```

---

## File structure

| File | Status | Responsibility | Tasks |
|---|---|---|---|
| `cli/crates/cook-cache/Cargo.toml` | Modify | Add deps: `sha2`, `hex`, `which`, `serde_json`, `toml`, `tracing`, `target` (build-script for triple). | 0.1 |
| `cli/crates/cook-cache/build.rs` | Create | Emit `TARGET` env var into the build for `target_triple` capture. | 0.2 |
| `cli/crates/cook-cache/src/context.rs` | Create | `ExecutionContext`, `MachineIdentity`, `ToolHash`, build-once probe, per-step `step_context_hash`. | 0.2 |
| `cli/crates/cook-cache/src/envkey.rs` | Create | `EnvDenylist` (D1 baseline + D2 extensions), `env_contribution` hash. | 0.3 |
| `cli/crates/cook-cache/src/backend.rs` | Create | `CacheBackend` trait, `BackendError`, `ArtifactMeta`, `CloudKey` type, `LocalBackend`, `cloud_key()` function. | 0.4, 0.5, 0.6 |
| `cli/crates/cook-cache/src/lib.rs` | Modify | Add `pub mod context;`, `pub mod envkey;`, `pub mod backend;`; update re-exports; (Phase 2) drop `hash_secondary_inputs` from public re-exports. | 0.2, 0.3, 0.4, 2.4 |
| `cli/crates/cook-cache/src/store.rs` | Modify | Bump `CACHE_VERSION` to 3; add `context_hash` and `env_contribution` to `StepEntry`; remove `secondary_inputs_hash` and `env_hash` from `RecipeCache`; update tests. | 2.1 |
| `cli/crates/cook-cache/src/check.rs` | Modify | Update `needs_rebuild_cook` and `needs_rebuild_plate` to compare `context_hash` and `env_contribution`; new `RebuildReason::ContextChanged` and `RebuildReason::EnvChanged` variants; remove `hash_secondary_inputs` (or keep private as removable in 2.4). | 2.2 |
| `cli/crates/cook-cache/src/manager.rs` | Modify | Harden `record_completion` (return `Result<(), RecordError>`; collect_records bails on `None`); remove `invalidate_recipe`; remove `invalidate_if_env_changed`. | 2.3 |
| `cli/crates/cook-contracts/src/lib.rs` | Modify | Extend `CacheMeta` with `project_id`, `cookfile_path`, `context_hash`, `env_contribution`, `consulted_env`; update tests. | 1.1 |
| `cli/crates/cook-engine/Cargo.toml` | Modify | Add `toml` dep. | 3.1 |
| `cli/crates/cook-engine/src/cloud_config.rs` | Create | Parse `.cook/cloud.toml` into `CloudConfig`; validate; default fallbacks. | 3.1 |
| `cli/crates/cook-engine/src/lib.rs` | Modify | `pub mod cloud_config;`. | 3.1 |
| `cli/crates/cook-engine/src/run.rs` | Modify | Build bootstrap: probe `ExecutionContext`, build `EnvDenylist`, construct `LocalBackend`, run `health()`, build `CacheContext`; remove `invalidate_if_env_changed` call. | 6.1, 6.4 |
| `cli/crates/cook-engine/src/executor.rs` | Modify | Lookup path uses `context_hash` and `env_contribution`. | 6.2 |
| `cli/crates/cook-engine/src/cache_ctx.rs` | Create | `CacheContext` struct (Arc-wrapping ExecutionContext, EnvDenylist, Box<dyn CacheBackend>, CloudConfig). | 6.1 |
| `cli/crates/cook-cli/src/dag_data.rs` | Modify | Lookup path uses `context_hash` and `env_contribution`. | 6.3 |
| `cli/crates/cook-luagen/src/template.rs` | Modify | Walk template tokens during expansion; collect those falling through to `cook.env[TOKEN]`; return list alongside the expanded Lua expression. | 4.1, 4.2 |
| `cli/crates/cook-luagen/src/cook_step.rs` | Modify | Plumb consulted-env list from template expansion into the `cook.add_unit` table emission as a `consulted_env_keys` field. | 4.3 |
| `cli/crates/cook-luagen/src/plate_step.rs` | Modify | Same as cook_step.rs for plate steps. | 4.3 |
| `cli/crates/cook-luagen/src/test_step.rs` | Modify | Same as cook_step.rs for test steps. | 4.3 |
| `cli/crates/cook-register/src/unit_api.rs` | Modify | Read `consulted_env_keys` from add_unit table; look up values from current process env; populate `CacheMeta.consulted_env` and `env_contribution`; populate `context_hash` from threaded `ExecutionContext`. | 5.1 |
| `cli/crates/cook-register/src/engine.rs` | Modify | Thread `Arc<CacheContext>` from cook-engine bootstrap into the Lua VM as registry data. | 5.2 |
| `examples/cpp-project/cook_modules/cpp.lua` | Modify | Append `# env: {CPATH} {C_INCLUDE_PATH} {CPLUS_INCLUDE_PATH} {LIBRARY_PATH} {LD_LIBRARY_PATH} {PKG_CONFIG_PATH} {SDKROOT}` to compile/link command templates. | 7.1 |
| `standard/src/content/docs/08-execution-model.mdx` | Modify | Replace §8.6 ¶3 with the amended text from spec §10.1. | 8.1 |
| `standard/src/content/docs/appendix/B-rationale.mdx` | Modify | Insert the cross-machine cache correctness paragraph from spec §10.2. | 8.2 |
| `standard/src/content/docs/appendix/D-changes.mdx` | Modify | Add CS-NNNN entry for this Standard amendment. | 8.1 |
| `cli/crates/cook-cache/tests/integration_first_build_depfile.rs` | Create | AC-Integ.1: thin-input cache-entry cross-machine fixture. | 9.1 |
| `cli/crates/cook-cache/tests/integration_cross_recipe_collision.rs` | Create | AC-Integ.2: two recipes producing same path → distinct cloud keys. | 9.2 |
| `cli/crates/cook-cache/tests/integration_config_toggle.rs` | Create | AC-Integ.3: toggling between two configs preserves both cache entries. | 9.3 |

No files removed; field/function removals are tracked as in-place modifications.

---

## Phase 0 — Foundation (additive, no breaking changes)

Phase 0 lands new files in `cook-cache` that do not modify any existing public surface. Each task ends with a passing `cargo test -p cook-cache` and its own commit.

### Task 0.1: Add Cargo dependencies

**Files:**
- Modify: `cli/crates/cook-cache/Cargo.toml`

- [ ] **Step 0.1.1: Add deps to `[dependencies]`**

In `cli/crates/cook-cache/Cargo.toml`, add to the `[dependencies]` table:

```toml
sha2 = "0.10"
hex = "0.4"
which = "6"
serde_json = "1"
toml = "0.8"
tracing = "0.1"
```

The `target` triple capture uses a build script (see Task 0.2) so no extra dep is required for that.

- [ ] **Step 0.1.2: Verify cargo accepts the new deps**

Run: `cd /home/alex/dev/cook/cli && cargo build -p cook-cache -q`
Expected: builds clean (deps download; no warnings related to the new deps).

- [ ] **Step 0.1.3: Commit**

```bash
git add cli/crates/cook-cache/Cargo.toml cli/Cargo.lock
git commit -m "$(cat <<'EOF'
build(SHI-140): cook-cache deps for cloud-key, env, backend

sha2 + hex for SHA-256 cloud-key composition; which for $PATH
resolution in the tool-binary probe; serde_json for ArtifactMeta
sidecar; toml for .cook/cloud.toml; tracing for hardened
record_completion warnings.
EOF
)"
```

---

### Task 0.2: `ExecutionContext` (machine identity + tool-binary hashing)

**Files:**
- Create: `cli/crates/cook-cache/build.rs`
- Create: `cli/crates/cook-cache/src/context.rs`
- Modify: `cli/crates/cook-cache/src/lib.rs`

- [ ] **Step 0.2.1: Build script for `TARGET` env var**

Create `cli/crates/cook-cache/build.rs`:

```rust
fn main() {
    let target = std::env::var("TARGET").expect("TARGET set by cargo for build scripts");
    println!("cargo:rustc-env=COOK_TARGET_TRIPLE={}", target);
    println!("cargo:rerun-if-env-changed=TARGET");
}
```

This makes `env!("COOK_TARGET_TRIPLE")` available at compile time in the crate's source.

- [ ] **Step 0.2.2: Write the failing tests for `MachineIdentity`**

Create `cli/crates/cook-cache/src/context.rs` with the following test module at the top of the file (we'll add the implementation in subsequent steps):

```rust
//! Per-build execution context: machine identity + tool-binary hashing.
//!
//! `MachineIdentity` is build-wide (probed once per `cook build`).
//! `ToolHash` is per-binary, cached by canonical realpath for the build's lifetime.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn machine_identity_target_triple_is_set() {
        let m = MachineIdentity::probe();
        assert!(!m.target_triple.is_empty(), "target_triple must be populated");
        // We can't assert a specific triple — varies by host — but it should
        // contain at least one '-' (e.g., "x86_64-unknown-linux-gnu").
        assert!(m.target_triple.contains('-'));
    }

    #[test]
    fn machine_identity_locale_baseline_includes_lang_when_set() {
        // Run-internal sanity check: if LANG is set in the test env, it lands.
        // Use `std::env::set_var` is not safe in parallel tests; instead,
        // we assert the captured map is at least lookup-able.
        let m = MachineIdentity::probe();
        // BTreeMap is always Some if probe returned; we don't assert specific keys.
        let _ = m.locale_baseline.get("LANG");
    }

    #[test]
    fn machine_identity_encode_deterministic() {
        let m1 = MachineIdentity::probe();
        let m2 = MachineIdentity::probe();
        assert_eq!(m1.encode(), m2.encode(), "two probes on same host produce same bytes");
    }

    #[test]
    fn tool_hash_of_known_file_is_deterministic() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("fake-tool");
        std::fs::write(&path, b"fake binary contents").expect("write");

        let h1 = ToolHash::from_file(&path).expect("hash");
        let h2 = ToolHash::from_file(&path).expect("hash");
        assert_eq!(h1.content_sha256, h2.content_sha256);
    }

    #[test]
    fn tool_hash_differs_for_different_contents() {
        let dir = tempfile::tempdir().expect("tempdir");
        let p1 = dir.path().join("a"); std::fs::write(&p1, b"AAA").expect("write");
        let p2 = dir.path().join("b"); std::fs::write(&p2, b"BBB").expect("write");

        let h1 = ToolHash::from_file(&p1).expect("hash");
        let h2 = ToolHash::from_file(&p2).expect("hash");
        assert_ne!(h1.content_sha256, h2.content_sha256);
    }

    #[test]
    fn tool_hash_missing_file_returns_empty_hash() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("nonexistent");
        let h = ToolHash::for_resolved(Some(path.as_path()));
        // Empty-binary fallback: sha256 of empty bytes.
        let expected = ToolHash::empty();
        assert_eq!(h.content_sha256, expected.content_sha256);
    }

    #[test]
    fn step_context_hash_combines_machine_and_tool() {
        let ctx = ExecutionContext::probe();
        let h_a = ctx.step_context_hash("/bin/sh -c true");
        let h_b = ctx.step_context_hash("/bin/sh -c true");
        assert_eq!(h_a, h_b, "same command on same machine → same hash");
    }

    #[test]
    fn step_context_hash_differs_on_first_token() {
        let ctx = ExecutionContext::probe();
        let h_sh = ctx.step_context_hash("/bin/sh -c true");
        let h_true = ctx.step_context_hash("/usr/bin/true");
        // Both binaries exist and differ; tool-hash component differs ⇒ hashes differ.
        // Skip the assertion if either binary is missing in the test environment.
        if std::path::Path::new("/bin/sh").exists() && std::path::Path::new("/usr/bin/true").exists() {
            assert_ne!(h_sh, h_true);
        }
    }

    #[test]
    fn tool_cache_caches_per_realpath() {
        let ctx = ExecutionContext::probe();
        // Two calls with the same first token go to the same cache entry.
        let _ = ctx.step_context_hash("/bin/sh foo");
        let _ = ctx.step_context_hash("/bin/sh bar");
        // Verify the tool_cache has at most one entry for /bin/sh's realpath.
        let cache = ctx.tool_cache.lock().expect("lock");
        let bin_sh_realpath = std::fs::canonicalize("/bin/sh").ok();
        if let Some(rp) = bin_sh_realpath {
            assert_eq!(cache.get(&rp).is_some(), true, "tool_cache should contain /bin/sh's realpath");
        }
    }
}
```

Run: `cd /home/alex/dev/cook/cli && cargo test -p cook-cache --lib context -- --nocapture`
Expected: COMPILE FAILURE (`MachineIdentity`, `ToolHash`, `ExecutionContext` not yet defined).

- [ ] **Step 0.2.3: Write `MachineIdentity::probe` and `encode`**

Above the `#[cfg(test)]` block in `context.rs`, add:

```rust
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MachineIdentity {
    pub target_triple: String,
    pub libc_version: Option<String>,
    pub locale_baseline: BTreeMap<String, String>,
}

impl MachineIdentity {
    /// Probe the current machine. Cheap; suitable to call once per build.
    pub fn probe() -> Self {
        Self {
            target_triple: env!("COOK_TARGET_TRIPLE").to_string(),
            libc_version: probe_libc_version(),
            locale_baseline: probe_locale_baseline(),
        }
    }

    /// Stable byte encoding for hashing. BTreeMap iteration is sorted ⇒ deterministic.
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(self.target_triple.as_bytes());
        out.push(0x1F); // unit separator
        match &self.libc_version {
            Some(v) => out.extend_from_slice(v.as_bytes()),
            None => out.extend_from_slice(b"<none>"),
        }
        out.push(0x1F);
        for (k, v) in &self.locale_baseline {
            out.extend_from_slice(k.as_bytes());
            out.push(b'=');
            out.extend_from_slice(v.as_bytes());
            out.push(0x1E); // record separator
        }
        out
    }
}

fn probe_libc_version() -> Option<String> {
    // Best-effort: try to read the first line of `<libc> --version`.
    // On glibc, /lib/x86_64-linux-gnu/libc.so.6 prints version info when invoked.
    // On musl/macOS native, this fails; return None.
    #[cfg(target_os = "linux")]
    {
        let candidates = [
            "/lib/x86_64-linux-gnu/libc.so.6",
            "/lib64/libc.so.6",
            "/lib/aarch64-linux-gnu/libc.so.6",
            "/lib/libc.so.6",
        ];
        for path in &candidates {
            if std::path::Path::new(path).exists() {
                if let Ok(out) = std::process::Command::new(path).output() {
                    if out.status.success() {
                        let s = String::from_utf8_lossy(&out.stdout);
                        if let Some(line) = s.lines().next() {
                            return Some(line.trim().to_string());
                        }
                    }
                }
            }
        }
    }
    None
}

fn probe_locale_baseline() -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    // Universal locale-affecting envs that bypass the denylist.
    let exact_keys = ["LANG", "TZ", "SOURCE_DATE_EPOCH"];
    for k in &exact_keys {
        if let Ok(v) = std::env::var(k) {
            out.insert((*k).to_string(), v);
        }
    }
    // Glob: every LC_*
    for (k, v) in std::env::vars() {
        if k.starts_with("LC_") {
            out.insert(k, v);
        }
    }
    out
}
```

- [ ] **Step 0.2.4: Write `ToolHash` and helpers**

Append to `context.rs`:

```rust
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct ToolHash {
    pub content_sha256: [u8; 32],
}

impl ToolHash {
    /// SHA-256 of empty input — used as the "binary not found / unreadable" sentinel.
    pub fn empty() -> Self {
        let mut h = Sha256::new();
        h.update(b"");
        let digest: [u8; 32] = h.finalize().into();
        Self { content_sha256: digest }
    }

    /// Hash a known file's contents. Returns None if the file cannot be read.
    pub fn from_file(path: &Path) -> Option<Self> {
        let bytes = std::fs::read(path).ok()?;
        let mut h = Sha256::new();
        h.update(&bytes);
        let digest: [u8; 32] = h.finalize().into();
        Some(Self { content_sha256: digest })
    }

    /// For a resolved (or unresolved) realpath: read+hash the file, or return
    /// `Self::empty()` on any failure (missing file, permission denied, etc.).
    /// Cache miss > cache poison: we never propagate a non-deterministic error.
    pub fn for_resolved(realpath: Option<&Path>) -> Self {
        match realpath.and_then(Self::from_file) {
            Some(h) => h,
            None => Self::empty(),
        }
    }
}
```

- [ ] **Step 0.2.5: Write `ExecutionContext` and `step_context_hash`**

Append to `context.rs`:

```rust
pub struct ExecutionContext {
    pub machine: MachineIdentity,
    /// Per-build: lazily probed and cached, keyed by canonical realpath.
    pub tool_cache: Mutex<HashMap<PathBuf, ToolHash>>,
}

impl ExecutionContext {
    pub fn probe() -> Self {
        Self {
            machine: MachineIdentity::probe(),
            tool_cache: Mutex::new(HashMap::new()),
        }
    }

    /// Compose the per-step context_hash for the given command.
    /// Hashes the machine identity + the resolved primary tool's binary contents.
    pub fn step_context_hash(&self, command: &str) -> u64 {
        let primary_tool = first_argv_token(command);
        let realpath = primary_tool
            .and_then(|t| which::which(t).ok())
            .and_then(|p| std::fs::canonicalize(p).ok());
        let tool_hash = self.tool_hash_for(realpath.as_deref());

        let machine_bytes = self.machine.encode();
        let mut hasher = xxhash_rust::xxh3::Xxh3::new();
        hasher.update(&machine_bytes);
        hasher.update(&tool_hash.content_sha256);
        hasher.digest()
    }

    fn tool_hash_for(&self, realpath: Option<&Path>) -> ToolHash {
        let Some(rp) = realpath else { return ToolHash::empty(); };
        // Fast path: cache hit.
        {
            let cache = self.tool_cache.lock().expect("tool_cache poisoned");
            if let Some(h) = cache.get(rp) {
                return h.clone();
            }
        }
        // Miss: hash and insert.
        let computed = ToolHash::for_resolved(Some(rp));
        let mut cache = self.tool_cache.lock().expect("tool_cache poisoned");
        cache.insert(rp.to_path_buf(), computed.clone());
        computed
    }
}

/// Split off the first argv token from a command string. Returns None for empty.
/// Does NOT understand shell-builtin chains (`cd && gcc`) — those produce the
/// builtin's identity per documented limitation (spec §2 non-goals).
fn first_argv_token(command: &str) -> Option<&str> {
    command.split_whitespace().next()
}
```

- [ ] **Step 0.2.6: Register the module**

Modify `cli/crates/cook-cache/src/lib.rs`. After the existing `pub mod check;`, `pub mod manager;`, `pub mod store;` block (around line 6-8), insert:

```rust
pub mod context;
```

- [ ] **Step 0.2.7: Run tests, expect pass**

Run: `cd /home/alex/dev/cook/cli && cargo test -p cook-cache --lib context`
Expected: all tests pass. The `tool_hash_missing_file_returns_empty_hash` test passes because `for_resolved(Some(missing))` returns `empty()`.

- [ ] **Step 0.2.8: Run all cook-cache tests, expect pass (no regressions)**

Run: `cd /home/alex/dev/cook/cli && cargo test -p cook-cache`
Expected: all existing tests pass; the new context tests pass.

- [ ] **Step 0.2.9: Commit**

```bash
git add cli/crates/cook-cache/build.rs cli/crates/cook-cache/src/context.rs cli/crates/cook-cache/src/lib.rs
git commit -m "$(cat <<'EOF'
feat(SHI-141): ExecutionContext with machine identity and tool hashing

Build-wide MachineIdentity (target triple via build.rs; libc version
best-effort on Linux glibc; locale baseline from LANG/LC_*/TZ/
SOURCE_DATE_EPOCH). Per-step tool-binary SHA-256, cached by realpath
for the build's lifetime. Failures return Sha256(empty) — cache miss
> cache poison.

Wired into a step_context_hash that composes machine ⊕ tool. Not yet
threaded into StepEntry / CacheMeta (Phase 2).
EOF
)"
```

---

### Task 0.3: `EnvDenylist` and `env_contribution`

**Files:**
- Create: `cli/crates/cook-cache/src/envkey.rs`
- Modify: `cli/crates/cook-cache/src/lib.rs`

- [ ] **Step 0.3.1: Write the failing tests**

Create `cli/crates/cook-cache/src/envkey.rs`:

```rust
//! Per-step env contribution to the cache key, with a two-layer denylist.
//!
//! D1: Cook-shipped baseline (`baseline()`) — universal noisy env.
//! D2: `.cook/cloud.toml [cache] ignore_env` extensions (`extend_with`).
//! Layer 2 inference (the consulted-env capture) is in cook-luagen/cook-register.

use std::collections::{BTreeMap, HashSet};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn baseline_excludes_HOME() {
        let d = EnvDenylist::baseline();
        assert!(d.is_ignored("HOME"));
    }

    #[test]
    fn baseline_excludes_PATH() {
        let d = EnvDenylist::baseline();
        assert!(d.is_ignored("PATH"));
    }

    #[test]
    fn baseline_excludes_XDG_glob() {
        let d = EnvDenylist::baseline();
        assert!(d.is_ignored("XDG_RUNTIME_DIR"));
        assert!(d.is_ignored("XDG_CONFIG_HOME"));
    }

    #[test]
    fn baseline_excludes_GITHUB_glob() {
        let d = EnvDenylist::baseline();
        assert!(d.is_ignored("GITHUB_TOKEN"));
        assert!(d.is_ignored("GITHUB_ACTIONS"));
    }

    #[test]
    fn baseline_does_not_exclude_CFLAGS() {
        let d = EnvDenylist::baseline();
        assert!(!d.is_ignored("CFLAGS"));
        assert!(!d.is_ignored("CXXFLAGS"));
        assert!(!d.is_ignored("CPATH"));
    }

    #[test]
    fn baseline_does_not_exclude_LANG_or_LC() {
        // Locale envs are NOT on the denylist — they are keyed via locale_baseline.
        let d = EnvDenylist::baseline();
        assert!(!d.is_ignored("LANG"));
        assert!(!d.is_ignored("LC_ALL"));
        assert!(!d.is_ignored("LC_CTYPE"));
        assert!(!d.is_ignored("TZ"));
        assert!(!d.is_ignored("SOURCE_DATE_EPOCH"));
    }

    #[test]
    fn extend_with_adds_user_names() {
        let mut d = EnvDenylist::baseline();
        d.extend_with(&["MY_API_TOKEN".to_string(), "MY_SECRET".to_string()]);
        assert!(d.is_ignored("MY_API_TOKEN"));
        assert!(d.is_ignored("MY_SECRET"));
        assert!(d.is_ignored("HOME"), "baseline still applies");
    }

    #[test]
    fn extend_with_overlap_is_idempotent() {
        let mut d = EnvDenylist::baseline();
        d.extend_with(&["HOME".to_string()]); // overlaps baseline
        assert!(d.is_ignored("HOME"));
    }

    #[test]
    fn env_contribution_empty_consulted_is_constant() {
        let d = EnvDenylist::baseline();
        let consulted = BTreeMap::new();
        let h1 = env_contribution(&consulted, &d);
        let h2 = env_contribution(&consulted, &d);
        assert_eq!(h1, h2);
    }

    #[test]
    fn env_contribution_filtered_keys_excluded() {
        let d = EnvDenylist::baseline();
        let mut a = BTreeMap::new();
        a.insert("CFLAGS".to_string(), "-O2".to_string());
        // Add a denylisted key with a different value
        let mut b = a.clone();
        b.insert("HOME".to_string(), "/home/alice".to_string());
        let h_a = env_contribution(&a, &d);
        let h_b = env_contribution(&b, &d);
        assert_eq!(h_a, h_b, "denylisted HOME must not contribute");
    }

    #[test]
    fn env_contribution_kept_keys_included() {
        let d = EnvDenylist::baseline();
        let mut a = BTreeMap::new();
        a.insert("CFLAGS".to_string(), "-O2".to_string());
        let mut b = BTreeMap::new();
        b.insert("CFLAGS".to_string(), "-O3".to_string());
        let h_a = env_contribution(&a, &d);
        let h_b = env_contribution(&b, &d);
        assert_ne!(h_a, h_b, "CFLAGS value change must change hash");
    }

    #[test]
    fn env_contribution_value_change_changes_hash() {
        let d = EnvDenylist::baseline();
        let mut a = BTreeMap::new();
        a.insert("MYVAR".to_string(), "v1".to_string());
        let mut b = BTreeMap::new();
        b.insert("MYVAR".to_string(), "v2".to_string());
        assert_ne!(env_contribution(&a, &d), env_contribution(&b, &d));
    }

    #[test]
    fn env_contribution_iteration_order_independent() {
        // BTreeMap is always sorted, so this trivially holds; sanity-check anyway.
        let d = EnvDenylist::baseline();
        let mut a = BTreeMap::new();
        a.insert("Z".to_string(), "1".to_string());
        a.insert("A".to_string(), "2".to_string());
        let mut b = BTreeMap::new();
        b.insert("A".to_string(), "2".to_string());
        b.insert("Z".to_string(), "1".to_string());
        assert_eq!(env_contribution(&a, &d), env_contribution(&b, &d));
    }
}
```

Run: `cd /home/alex/dev/cook/cli && cargo test -p cook-cache --lib envkey -- --nocapture`
Expected: COMPILE FAILURE.

- [ ] **Step 0.3.2: Write `EnvDenylist`**

Above the `#[cfg(test)]` block in `envkey.rs`, add:

```rust
pub struct EnvDenylist {
    /// Exact-match names.
    names: HashSet<String>,
    /// Glob patterns like "XDG_*", "GITHUB_*". Compiled once at construction.
    globs: Vec<glob::Pattern>,
}

impl EnvDenylist {
    /// D1: Cook-shipped baseline. See spec Appendix A for the full list.
    pub fn baseline() -> Self {
        const EXACT: &[&str] = &[
            // Session/ambient
            "HOME", "USER", "LOGNAME", "SHELL", "PATH", "PWD", "OLDPWD", "MAIL", "HOSTNAME",
            "TERM", "TERMINFO", "COLORTERM",
            "DISPLAY", "WAYLAND_DISPLAY", "XAUTHORITY",
            "SSH_AUTH_SOCK", "SSH_CONNECTION", "SSH_CLIENT", "SSH_TTY",
            "DBUS_SESSION_BUS_ADDRESS", "DBUS_STARTER_BUS_TYPE", "DBUS_STARTER_ADDRESS",
            "EDITOR", "VISUAL", "PAGER", "BROWSER",
            "TMPDIR", "TMP", "TEMP",
            "HISTFILE", "HISTSIZE", "HISTCONTROL",
            "SHLVL", "PS1", "PS2", "PS3", "PS4",
            // CI universal
            "CI",
        ];
        const GLOBS: &[&str] = &[
            "XDG_*",
            "GITHUB_*", "RUNNER_*",
            "GITLAB_CI_*",
            "BUILDKITE_*",
            "CIRCLE_*",
            "TRAVIS_*",
            "JENKINS_*",
            "TEAMCITY_*",
            "DRONE_*",
        ];

        let names: HashSet<String> = EXACT.iter().map(|s| (*s).to_string()).collect();
        let globs: Vec<glob::Pattern> = GLOBS
            .iter()
            .map(|p| glob::Pattern::new(p).expect("baseline glob compiles"))
            .collect();
        Self { names, globs }
    }

    /// Extend with project-level (.cook/cloud.toml) additions. Idempotent on overlap.
    pub fn extend_with(&mut self, additions: &[String]) {
        for a in additions {
            if a.contains('*') || a.contains('?') {
                if let Ok(p) = glob::Pattern::new(a) {
                    self.globs.push(p);
                }
            } else {
                self.names.insert(a.clone());
            }
        }
    }

    pub fn is_ignored(&self, key: &str) -> bool {
        if self.names.contains(key) {
            return true;
        }
        self.globs.iter().any(|p| p.matches(key))
    }
}
```

- [ ] **Step 0.3.3: Write `env_contribution`**

Append to `envkey.rs`:

```rust
/// Compute the env contribution hash for a step.
///
/// `consulted` is the BTreeMap of (name → value) pairs that the step's
/// command consulted (per Layer 2 inference). The denylist filters
/// names whose values must not contribute to the cache key.
///
/// xxh3_64 because this is a local-cache hash; the cloud-key SHA-256
/// composition reads this field directly.
pub fn env_contribution(consulted: &BTreeMap<String, String>, denylist: &EnvDenylist) -> u64 {
    let mut hasher = xxhash_rust::xxh3::Xxh3::new();
    for (k, v) in consulted {
        if denylist.is_ignored(k) {
            continue;
        }
        hasher.update(k.as_bytes());
        hasher.update(b"=");
        hasher.update(v.as_bytes());
        hasher.update(b"\n");
    }
    hasher.digest()
}
```

- [ ] **Step 0.3.4: Register the module**

In `cli/crates/cook-cache/src/lib.rs`, after `pub mod context;`, add:

```rust
pub mod envkey;
```

- [ ] **Step 0.3.5: Run tests, expect pass**

Run: `cd /home/alex/dev/cook/cli && cargo test -p cook-cache --lib envkey`
Expected: all twelve envkey tests pass.

- [ ] **Step 0.3.6: Run all cook-cache tests**

Run: `cd /home/alex/dev/cook/cli && cargo test -p cook-cache`
Expected: all pass.

- [ ] **Step 0.3.7: Commit**

```bash
git add cli/crates/cook-cache/src/envkey.rs cli/crates/cook-cache/src/lib.rs
git commit -m "$(cat <<'EOF'
feat(SHI-142): EnvDenylist and env_contribution hash

Two-layer denylist: D1 (Cook-shipped baseline of session/ambient and
CI metadata env names + globs) and D2 (.cook/cloud.toml extensions
via extend_with). Locale envs (LANG/LC_*/TZ/SOURCE_DATE_EPOCH) are
NOT on the denylist — they are keyed via MachineIdentity.locale_baseline.

env_contribution(consulted, denylist) hashes the post-filter
(name, value) pairs with xxh3_64 in BTreeMap iteration order. Local
cache uses this hash directly; cloud key composition reads it as a
u64 component.

Not yet wired into StepEntry / CacheMeta (Phase 2).
EOF
)"
```

---

### Task 0.4: `CacheBackend` trait, `BackendError`, `ArtifactMeta`, `CloudKey`

**Files:**
- Create: `cli/crates/cook-cache/src/backend.rs` (trait + types only; impls in 0.5)
- Modify: `cli/crates/cook-cache/src/lib.rs`

- [ ] **Step 0.4.1: Create `backend.rs` skeleton with types**

Create `cli/crates/cook-cache/src/backend.rs`:

```rust
//! The CacheBackend trait — the seam Cook Cloud's R2/D1 backend implements
//! against. v3 ships LocalBackend (file-system); SHI-24 will add CloudBackend.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

/// 32-byte SHA-256 cloud cache key.
pub type CloudKey = [u8; 32];

#[derive(Debug, Clone)]
pub enum BackendError {
    /// Network/transport failure. Engine treats as miss and proceeds.
    Transient(String),
    /// Authentication/permission failure. Engine logs once, disables backend for build.
    Unauthorized(String),
    /// Quota exceeded on put. Engine logs, drops the put, build continues.
    QuotaExceeded,
    /// Unexpected backend state (corrupted response, etc.). Logged; treated as miss.
    Other(String),
}

impl std::fmt::Display for BackendError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BackendError::Transient(s) => write!(f, "transient backend error: {s}"),
            BackendError::Unauthorized(s) => write!(f, "backend unauthorized: {s}"),
            BackendError::QuotaExceeded => write!(f, "backend quota exceeded"),
            BackendError::Other(s) => write!(f, "backend error: {s}"),
        }
    }
}

impl std::error::Error for BackendError {}

pub type BackendResult<T> = Result<T, BackendError>;

/// Metadata describing one artifact, written alongside the bytes for backend
/// introspection and eviction policy. Values of consulted env are NEVER stored
/// here — only the keys, for diagnostic use.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ArtifactMeta {
    pub recipe_namespace: String,
    pub command_hash: u64,
    pub context_hash: u64,
    pub env_contribution: u64,
    pub schema_version: u32,
    pub size_bytes: u64,
    pub tags: BTreeSet<String>,
    pub consulted_env_keys: BTreeSet<String>,
}

pub trait CacheBackend: Send + Sync {
    /// Batch existence check. Returns the subset of inputs that are hits.
    /// Implementations MAY ignore order; the engine sorts before calling.
    fn batch_query(&self, keys: &[CloudKey]) -> BackendResult<BTreeSet<CloudKey>>;

    /// Fetch artifact bytes. Returns Ok(None) on miss (NOT an error).
    fn get(&self, key: &CloudKey) -> BackendResult<Option<Vec<u8>>>;

    /// Upload artifact bytes with metadata. Idempotent on (key, bytes):
    /// re-putting the same pair MUST succeed.
    fn put(&self, key: &CloudKey, bytes: &[u8], meta: &ArtifactMeta) -> BackendResult<()>;

    /// Explicit deletion. Idempotent: returns Ok(()) for both
    /// "deleted" and "didn't exist".
    fn delete(&self, key: &CloudKey) -> BackendResult<()>;

    /// Lightweight health check. Engine calls once at build start.
    fn health(&self) -> BackendResult<()>;
}
```

- [ ] **Step 0.4.2: Register the module**

In `cli/crates/cook-cache/src/lib.rs`, after `pub mod envkey;`, add:

```rust
pub mod backend;
```

- [ ] **Step 0.4.3: Build and verify**

Run: `cd /home/alex/dev/cook/cli && cargo build -p cook-cache -q`
Expected: builds clean. No tests added in this step — the trait has no behavior to test until LocalBackend implements it (Task 0.5).

- [ ] **Step 0.4.4: Commit**

```bash
git add cli/crates/cook-cache/src/backend.rs cli/crates/cook-cache/src/lib.rs
git commit -m "$(cat <<'EOF'
feat(SHI-143): CacheBackend trait, BackendError, ArtifactMeta

Trait surface for the cloud-cache seam. Synchronous (cloud impls
manage async I/O internally). BackendError taxonomy explicitly
encodes engine behavior: Transient → miss; Unauthorized → disable
backend for build; QuotaExceeded → drop put; Other → miss.

ArtifactMeta carries the structured fields any sensible eviction
policy needs (size, tags, namespace, hash components, schema_version).
Values of consulted env are never stored — only keys, for diagnostics.

LocalBackend impl in next task.
EOF
)"
```

---

### Task 0.5: `LocalBackend` implementation

**Files:**
- Modify: `cli/crates/cook-cache/src/backend.rs`

- [ ] **Step 0.5.1: Write the failing tests**

Append to `cli/crates/cook-cache/src/backend.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn sample_meta() -> ArtifactMeta {
        ArtifactMeta {
            recipe_namespace: "cook/Cookfile::build".into(),
            command_hash: 0xdead_beef,
            context_hash: 0x1111_2222,
            env_contribution: 0x3333_4444,
            schema_version: 3,
            size_bytes: 5,
            tags: BTreeSet::new(),
            consulted_env_keys: BTreeSet::new(),
        }
    }

    fn key(byte: u8) -> CloudKey {
        let mut k = [0u8; 32];
        k[0] = byte;
        k
    }

    #[test]
    fn local_backend_health_ok_on_existing_root() {
        let dir = tempfile::tempdir().expect("tempdir");
        let backend = LocalBackend::new(dir.path().to_path_buf());
        backend.health().expect("health ok");
    }

    #[test]
    fn local_backend_get_miss_returns_none() {
        let dir = tempfile::tempdir().expect("tempdir");
        let backend = LocalBackend::new(dir.path().to_path_buf());
        let k = key(0xAB);
        assert!(backend.get(&k).expect("get").is_none());
    }

    #[test]
    fn local_backend_put_get_round_trip() {
        let dir = tempfile::tempdir().expect("tempdir");
        let backend = LocalBackend::new(dir.path().to_path_buf());
        let k = key(0x01);
        backend.put(&k, b"hello", &sample_meta()).expect("put");
        let got = backend.get(&k).expect("get").expect("hit");
        assert_eq!(got, b"hello");
    }

    #[test]
    fn local_backend_put_idempotent() {
        let dir = tempfile::tempdir().expect("tempdir");
        let backend = LocalBackend::new(dir.path().to_path_buf());
        let k = key(0x02);
        backend.put(&k, b"data", &sample_meta()).expect("put 1");
        backend.put(&k, b"data", &sample_meta()).expect("put 2");
        let got = backend.get(&k).expect("get").expect("hit");
        assert_eq!(got, b"data");
    }

    #[test]
    fn local_backend_batch_query_returns_hits_subset() {
        let dir = tempfile::tempdir().expect("tempdir");
        let backend = LocalBackend::new(dir.path().to_path_buf());
        let k1 = key(0x10);
        let k2 = key(0x20);
        let k3 = key(0x30);
        backend.put(&k1, b"a", &sample_meta()).expect("put1");
        backend.put(&k3, b"c", &sample_meta()).expect("put3");
        let hits = backend.batch_query(&[k1, k2, k3]).expect("query");
        assert!(hits.contains(&k1));
        assert!(!hits.contains(&k2));
        assert!(hits.contains(&k3));
    }

    #[test]
    fn local_backend_delete_idempotent() {
        let dir = tempfile::tempdir().expect("tempdir");
        let backend = LocalBackend::new(dir.path().to_path_buf());
        let k = key(0xFF);
        backend.delete(&k).expect("delete missing ok"); // never existed
        backend.put(&k, b"x", &sample_meta()).expect("put");
        backend.delete(&k).expect("delete existing ok");
        backend.delete(&k).expect("delete missing again ok");
        assert!(backend.get(&k).expect("get").is_none());
    }

    #[test]
    fn local_backend_meta_sidecar_persisted() {
        let dir = tempfile::tempdir().expect("tempdir");
        let backend = LocalBackend::new(dir.path().to_path_buf());
        let k = key(0x55);
        let mut meta = sample_meta();
        meta.tags.insert("ci".into());
        meta.tags.insert("release:v0.5".into());
        backend.put(&k, b"x", &meta).expect("put");

        // Read the sidecar file directly to verify structure.
        let path = backend.path_for(&k);
        let meta_path = path.with_extension("meta.json");
        let bytes = std::fs::read(&meta_path).expect("read sidecar");
        let restored: ArtifactMeta = serde_json::from_slice(&bytes).expect("deserialize");
        assert_eq!(restored.tags, meta.tags);
        assert_eq!(restored.recipe_namespace, meta.recipe_namespace);
    }

    #[test]
    fn local_backend_path_for_fans_out_by_first_byte() {
        let dir = tempfile::tempdir().expect("tempdir");
        let backend = LocalBackend::new(dir.path().to_path_buf());
        let k = key(0xAB);
        let path = backend.path_for(&k);
        // First two hex chars are the parent directory; remaining 62 are the file name.
        let parent = path.parent().unwrap().file_name().unwrap().to_string_lossy();
        assert_eq!(parent, "ab");
        let file_name = path.file_name().unwrap().to_string_lossy();
        assert_eq!(file_name.len(), 62);
    }
}
```

Run: `cd /home/alex/dev/cook/cli && cargo test -p cook-cache --lib backend`
Expected: COMPILE FAILURE (`LocalBackend` not yet defined).

- [ ] **Step 0.5.2: Implement `LocalBackend`**

Append to `cli/crates/cook-cache/src/backend.rs` (above the test module — directly after the trait definition):

```rust
use std::path::PathBuf;

pub struct LocalBackend {
    root: PathBuf,
}

impl LocalBackend {
    pub fn new(root: PathBuf) -> Self {
        // Ensure root exists; ignore "already exists" errors.
        let _ = std::fs::create_dir_all(&root);
        Self { root }
    }

    /// Compute the on-disk path for a CloudKey:
    ///   {root}/{first_2_hex_chars}/{remaining_62_hex_chars}
    pub(crate) fn path_for(&self, key: &CloudKey) -> PathBuf {
        let hex = hex::encode(key);
        self.root.join(&hex[..2]).join(&hex[2..])
    }
}

impl CacheBackend for LocalBackend {
    fn batch_query(&self, keys: &[CloudKey]) -> BackendResult<BTreeSet<CloudKey>> {
        let mut hits = BTreeSet::new();
        for k in keys {
            if self.path_for(k).exists() {
                hits.insert(*k);
            }
        }
        Ok(hits)
    }

    fn get(&self, key: &CloudKey) -> BackendResult<Option<Vec<u8>>> {
        let path = self.path_for(key);
        match std::fs::read(&path) {
            Ok(bytes) => Ok(Some(bytes)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(BackendError::Other(format!("read {}: {e}", path.display()))),
        }
    }

    fn put(&self, key: &CloudKey, bytes: &[u8], meta: &ArtifactMeta) -> BackendResult<()> {
        let path = self.path_for(key);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| BackendError::Other(format!("mkdir {}: {e}", parent.display())))?;
        }
        // Atomic write via tmp + rename.
        let tmp = path.with_extension("tmp");
        std::fs::write(&tmp, bytes)
            .map_err(|e| BackendError::Other(format!("write {}: {e}", tmp.display())))?;
        std::fs::rename(&tmp, &path)
            .map_err(|e| BackendError::Other(format!("rename {}: {e}", path.display())))?;

        // Sidecar metadata.
        let meta_path = path.with_extension("meta.json");
        let meta_bytes = serde_json::to_vec(meta)
            .map_err(|e| BackendError::Other(format!("serialize meta: {e}")))?;
        std::fs::write(&meta_path, &meta_bytes)
            .map_err(|e| BackendError::Other(format!("write meta {}: {e}", meta_path.display())))?;
        Ok(())
    }

    fn delete(&self, key: &CloudKey) -> BackendResult<()> {
        let path = self.path_for(key);
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(path.with_extension("meta.json"));
        Ok(())
    }

    fn health(&self) -> BackendResult<()> {
        std::fs::metadata(&self.root)
            .map(|_| ())
            .map_err(|e| BackendError::Other(format!("root {}: {e}", self.root.display())))
    }
}
```

- [ ] **Step 0.5.3: Run tests, expect pass**

Run: `cd /home/alex/dev/cook/cli && cargo test -p cook-cache --lib backend`
Expected: all eight backend tests pass.

- [ ] **Step 0.5.4: Run all cook-cache tests**

Run: `cd /home/alex/dev/cook/cli && cargo test -p cook-cache`
Expected: all pass; no regressions.

- [ ] **Step 0.5.5: Commit**

```bash
git add cli/crates/cook-cache/src/backend.rs
git commit -m "$(cat <<'EOF'
feat(SHI-143): LocalBackend — file-system CacheBackend impl

Stores artifact bytes under {root}/{xx}/{remaining-62-hex} (fan-out
by first byte to avoid 1M-files-in-one-dir). Sidecar
<key>.meta.json holds ArtifactMeta. Atomic writes via tmp + rename.
Idempotent put and delete. Health checks the root directory exists.

This is not the same as today's RecipeCache index — it's the
artifact STORE. Cook Cloud's CloudBackend (SHI-24) will replace it
behind the same trait.
EOF
)"
```

---

### Task 0.6: `cloud_key()` composition function

**Files:**
- Modify: `cli/crates/cook-cache/src/backend.rs`

- [ ] **Step 0.6.1: Write failing tests for `cloud_key`**

Append a new section to the existing `#[cfg(test)] mod tests` block in `backend.rs`, just before the closing brace `}`:

```rust
    // ─── cloud_key composition tests ────────────────────────────────────────

    fn make_key_inputs() -> CloudKeyInputs<'static> {
        CloudKeyInputs {
            schema_version: 3,
            recipe_namespace: "cook/Cookfile::build",
            command_hash: 0xAAAA,
            context_hash: 0xBBBB,
            env_contribution: 0xCCCC,
            sorted_input_content_hashes: &[0x1111, 0x2222, 0x3333],
        }
    }

    #[test]
    fn cloud_key_deterministic() {
        let inputs = make_key_inputs();
        let k1 = cloud_key(&inputs);
        let k2 = cloud_key(&inputs);
        assert_eq!(k1, k2);
    }

    #[test]
    fn cloud_key_changes_on_command_hash_change() {
        let mut a = make_key_inputs();
        let mut b = a;
        b.command_hash = 0xFFFF;
        assert_ne!(cloud_key(&a), cloud_key(&b));
        let _ = &mut a; // silence unused-mut on a
    }

    #[test]
    fn cloud_key_changes_on_context_hash_change() {
        let mut a = make_key_inputs();
        let mut b = a;
        b.context_hash = 0xFFFF;
        assert_ne!(cloud_key(&a), cloud_key(&b));
        let _ = &mut a;
    }

    #[test]
    fn cloud_key_changes_on_env_contribution_change() {
        let mut a = make_key_inputs();
        let mut b = a;
        b.env_contribution = 0xFFFF;
        assert_ne!(cloud_key(&a), cloud_key(&b));
        let _ = &mut a;
    }

    #[test]
    fn cloud_key_changes_on_schema_version_change() {
        let mut a = make_key_inputs();
        let mut b = a;
        b.schema_version = 4;
        assert_ne!(cloud_key(&a), cloud_key(&b));
        let _ = &mut a;
    }

    #[test]
    fn cloud_key_changes_on_namespace_change() {
        let a = make_key_inputs();
        let mut b = a;
        b.recipe_namespace = "cook/Cookfile::test";
        assert_ne!(cloud_key(&a), cloud_key(&b));
    }

    #[test]
    fn cloud_key_changes_on_input_content_change() {
        let a = make_key_inputs();
        let alt_inputs = [0x1111, 0x2222, 0x9999]; // last hash differs
        let b = CloudKeyInputs { sorted_input_content_hashes: &alt_inputs, ..a };
        assert_ne!(cloud_key(&a), cloud_key(&b));
    }

    #[test]
    fn cloud_key_caller_must_sort_inputs() {
        // The function trusts its caller's sort. A caller-sorted slice produces
        // a stable hash; an unsorted slice produces a different (but stable) one.
        // This test documents that the sort is the caller's responsibility.
        let sorted = [0x1111u64, 0x2222, 0x3333];
        let unsorted = [0x3333u64, 0x1111, 0x2222];
        let a = make_key_inputs();
        let b = CloudKeyInputs { sorted_input_content_hashes: &sorted, ..a };
        let c = CloudKeyInputs { sorted_input_content_hashes: &unsorted, ..a };
        assert_ne!(cloud_key(&b), cloud_key(&c),
            "the function does not internally sort; caller responsibility");
    }

    #[test]
    fn cloud_key_returns_32_bytes() {
        let k = cloud_key(&make_key_inputs());
        assert_eq!(k.len(), 32);
    }
```

Run: `cd /home/alex/dev/cook/cli && cargo test -p cook-cache --lib backend -- --nocapture`
Expected: COMPILE FAILURE (`cloud_key`, `CloudKeyInputs` not yet defined).

- [ ] **Step 0.6.2: Implement `cloud_key` and `CloudKeyInputs`**

Append to `cli/crates/cook-cache/src/backend.rs` (above the test module):

```rust
use sha2::{Digest, Sha256};

/// Inputs to `cloud_key()`. The struct is `Copy` so callers can build it once
/// and pass it around; lifetimes track the borrowed namespace and inputs slice.
#[derive(Clone, Copy)]
pub struct CloudKeyInputs<'a> {
    pub schema_version: u32,
    pub recipe_namespace: &'a str,
    pub command_hash: u64,
    pub context_hash: u64,
    pub env_contribution: u64,
    /// Caller MUST sort by path before passing. The slice is hashed in given
    /// order; sorting is the caller's responsibility (cf. spec §5.3).
    pub sorted_input_content_hashes: &'a [u64],
}

/// Compose the SHA-256 cloud key for an artifact.
/// See spec §5.3 for the composition; the 0x00 delimiter prevents
/// string-injection collisions between the namespace and hash bytes.
pub fn cloud_key(inputs: &CloudKeyInputs<'_>) -> CloudKey {
    let mut h = Sha256::new();
    h.update(inputs.schema_version.to_le_bytes());
    h.update(inputs.recipe_namespace.as_bytes());
    h.update([0x00]); // delimiter
    h.update(inputs.command_hash.to_le_bytes());
    h.update(inputs.context_hash.to_le_bytes());
    h.update(inputs.env_contribution.to_le_bytes());
    for hash in inputs.sorted_input_content_hashes {
        h.update(hash.to_le_bytes());
    }
    h.finalize().into()
}
```

- [ ] **Step 0.6.3: Run tests**

Run: `cd /home/alex/dev/cook/cli && cargo test -p cook-cache --lib backend`
Expected: all backend tests (LocalBackend + cloud_key) pass — 17 tests total.

- [ ] **Step 0.6.4: Run all cook-cache tests**

Run: `cd /home/alex/dev/cook/cli && cargo test -p cook-cache`
Expected: all pass.

- [ ] **Step 0.6.5: Commit**

```bash
git add cli/crates/cook-cache/src/backend.rs
git commit -m "$(cat <<'EOF'
feat(SHI-143/144): cloud_key SHA-256 composition

Composition order (spec §5.3):
  schema_version || namespace || 0x00 || command || context || env || sorted_inputs

The 0x00 delimiter between the namespace string and the binary hash
fields prevents string-injection collisions. Caller is responsible
for sorting input content hashes by path before invocation; the
function does not internally sort (documented by test).
EOF
)"
```

---

## Phase 1 — `cook-contracts::CacheMeta` extension

Phase 1 extends `CacheMeta` with five new fields. `unit_api.rs` in `cook-register` is the sole production builder; we update it to populate the new fields with **placeholder zeros / empty maps**. Real values come in later phases (project_id from Phase 3, consulted_env from Phase 5, context_hash from Phase 6). After this phase, the workspace builds cleanly with the v2-like cache behavior, just carrying the new field shape.

### Task 1.1: Extend `CacheMeta` and update its sole production builder

**Files:**
- Modify: `cli/crates/cook-contracts/src/lib.rs`
- Modify: `cli/crates/cook-register/src/unit_api.rs`

- [ ] **Step 1.1.1: Update `CacheMeta` struct definition**

Open `cli/crates/cook-contracts/src/lib.rs`. Find the `pub struct CacheMeta` definition (around line 57). Replace its body to include the new fields:

```rust
#[derive(Debug, Clone, PartialEq)]
pub struct CacheMeta {
    pub recipe_name: String,
    /// NEW: from .cook/cloud.toml `project = "..."` (Phase 3 wires real values).
    pub project_id: String,
    /// NEW: relative path of the source Cookfile from the project root, forward-slashed.
    pub cookfile_path: String,
    pub cache_key: String,
    pub input_paths: Vec<String>,
    pub output_paths: Vec<String>,
    pub command_hash: u64,
    /// NEW: machine + tool identity. Phase 6 wires real values; zero until then.
    pub context_hash: u64,
    /// NEW: post-denylist env contribution. Phase 5 wires real values; zero until then.
    pub env_contribution: u64,
    /// NEW: the (key, value) pairs the command consulted post-denylist.
    /// Phase 5 wires real values; empty BTreeMap until then.
    pub consulted_env: std::collections::BTreeMap<String, String>,
}
```

- [ ] **Step 1.1.2: Update existing tests in `cook-contracts/src/lib.rs`**

The existing `#[cfg(test)] mod tests` in `lib.rs` constructs `CacheMeta` literals that now break. Find every `CacheMeta { ... }` in the file (two test functions, around lines 156 and 171). Add the new fields to each construction:

```rust
        let m = CacheMeta {
            recipe_name: "build".into(),
            project_id: String::new(),
            cookfile_path: String::new(),
            cache_key: "abc123".into(),
            input_paths: vec!["src/main.c".into()],
            output_paths: vec!["build/main.o".into()],
            command_hash: 42,
            context_hash: 0,
            env_contribution: 0,
            consulted_env: std::collections::BTreeMap::new(),
        };
```

Apply the same five-field addition to the other `CacheMeta {` constructions in the file. Also find and update the `cache_meta: Some(CacheMeta { ... })` construction (around line 269).

- [ ] **Step 1.1.3: Update `cook-register/src/unit_api.rs` to construct new fields**

Open `cli/crates/cook-register/src/unit_api.rs`. Find the `CacheMeta { ... }` construction (around line 96). Replace it with:

```rust
        let cache_meta = if cache_enabled {
            let cache_key = if let Some(first) = output_paths.first() {
                first.clone()
            } else {
                format!("{}@{:x}", inputs.first().map(|s| s.as_str()).unwrap_or(""), command_hash)
            };
            Some(CacheMeta {
                recipe_name: rname.clone(),
                // Placeholders — real values come in Phases 3, 5, 6.
                project_id: String::new(),
                cookfile_path: String::new(),
                cache_key,
                input_paths: inputs.clone(),
                output_paths: output_paths.clone(),
                command_hash,
                context_hash: 0,
                env_contribution: 0,
                consulted_env: std::collections::BTreeMap::new(),
            })
        } else {
            None
        };
```

- [ ] **Step 1.1.4: Update existing `unit_api.rs` tests**

Search `unit_api.rs` for `CacheMeta` and `cache_meta` references in the `#[cfg(test)]` section. The existing tests (around lines 214, 363) check fields like `meta.cache_key` and `meta.command_hash`. Those assertions still pass; you only need to ensure NEW assertions cover the new fields if useful. For now, no new assertions required — Phase 5 will add real consulted_env assertions.

- [ ] **Step 1.1.5: Build the workspace, expect compile success**

Run: `cd /home/alex/dev/cook/cli && cargo build -q`
Expected: builds clean. If any other call site reads/writes `CacheMeta`, the compiler will point at it; update those constructions with the new fields set to placeholder values.

- [ ] **Step 1.1.6: Run all workspace tests**

Run: `cd /home/alex/dev/cook/cli && cargo test -q`
Expected: all tests pass. The new fields are zero/empty everywhere; behavior unchanged.

- [ ] **Step 1.1.7: Commit**

```bash
git add cli/crates/cook-contracts/src/lib.rs cli/crates/cook-register/src/unit_api.rs
git commit -m "$(cat <<'EOF'
feat(SHI-140): extend CacheMeta with v3 fields (placeholders)

project_id, cookfile_path, context_hash, env_contribution,
consulted_env added to CacheMeta. unit_api.rs (the sole production
builder) populates them with empty/zero placeholders. Real values
arrive in Phase 3 (project_id from .cook/cloud.toml), Phase 5
(consulted_env from cook-luagen), and Phase 6 (context_hash from
ExecutionContext probe).

This is the additive precursor to the schema v3 cascade.
EOF
)"
```

---

## Phase 2 — Schema v3 cascade in `cook-cache` + lookup updates

Phase 2 bumps `CACHE_VERSION` to 3, adds `context_hash`/`env_contribution` to `StepEntry`, removes the dead `secondary_inputs_hash` and `env_hash` fields from `RecipeCache`, removes the dead `invalidate_recipe` and `invalidate_if_env_changed` methods, hardens `record_completion`, and updates the `executor.rs`/`dag_data.rs` lookup paths to pass the new comparison values. After this phase, the cache key shape is v3 in full; behavior is still v2-equivalent because the values flowing in are zero (until Phases 5–6).

This is a cascading change; it lands in **one commit** to keep the workspace building.

### Task 2.1: Update `cook-cache/src/store.rs` (struct shape + version + tests)

**Files:**
- Modify: `cli/crates/cook-cache/src/store.rs`

- [ ] **Step 2.1.1: Bump `CACHE_VERSION` and reshape structs**

Open `cli/crates/cook-cache/src/store.rs`. Replace the top of the file (the const + struct definitions, lines 1–28 in the existing file):

```rust
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

pub const CACHE_VERSION: u32 = 3;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RecipeCache {
    pub version: u32,
    pub globs: BTreeMap<String, BTreeSet<String>>,
    pub steps: BTreeMap<String, StepEntry>,
    // REMOVED: secondary_inputs_hash (SHI-145) — dead code path.
    // REMOVED: env_hash (SHI-142) — folded into per-step env_contribution.
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct StepEntry {
    pub inputs: Vec<FileRecord>,
    pub outputs: Vec<FileRecord>,
    pub command_hash: u64,
    pub context_hash: u64,        // NEW (SHI-141)
    pub env_contribution: u64,    // NEW (SHI-142)
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FileRecord {
    pub path: String,
    pub mtime: u64,
    pub hash: u64,
}
```

- [ ] **Step 2.1.2: Update `RecipeCache::new` and `Default`**

In `store.rs`, the `RecipeCache::new` function (around line 36) currently initializes the now-removed fields. Replace its body:

```rust
impl RecipeCache {
    pub fn new() -> Self {
        Self {
            version: CACHE_VERSION,
            globs: BTreeMap::new(),
            steps: BTreeMap::new(),
        }
    }

    pub fn load(cache_dir: &Path, recipe_name: &str) -> Option<Self> {
        let path = cache_dir.join(format!("{}.bin", recipe_name));
        let bytes = std::fs::read(&path).ok()?;
        let cache: Self = bincode::deserialize(&bytes).ok()?;
        if cache.version != CACHE_VERSION {
            return None;
        }
        Some(cache)
    }

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

- [ ] **Step 2.1.3: Rewrite the `#[cfg(test)] mod tests` block**

The existing `make_populated_cache()` and tests reference `secondary_inputs_hash` and `env_hash` (lines 73–200). Replace the entire `#[cfg(test)]` block at the bottom of `store.rs` with:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn make_populated_cache() -> RecipeCache {
        let mut cache = RecipeCache::new();

        let mut globs = BTreeMap::new();
        globs.insert(
            "src/*.c".to_string(),
            BTreeSet::from(["src/main.c".to_string(), "src/util.c".to_string()]),
        );
        cache.globs = globs;

        let step = StepEntry {
            inputs: vec![
                FileRecord {
                    path: "src/main.c".to_string(),
                    mtime: 1700000000,
                    hash: 0x1234567890abcdef,
                },
                FileRecord {
                    path: "src/util.c".to_string(),
                    mtime: 1700000001,
                    hash: 0xfedcba9876543210,
                },
            ],
            outputs: vec![FileRecord {
                path: "build/main.o".to_string(),
                mtime: 1700000100,
                hash: 0xabcdef1234567890,
            }],
            command_hash: 0x0102030405060708,
            context_hash: 0x1111111111111111,
            env_contribution: 0x2222222222222222,
        };
        cache.steps.insert("compile_main".to_string(), step);

        cache
    }

    #[test]
    fn version_is_three() {
        assert_eq!(CACHE_VERSION, 3);
    }

    #[test]
    fn round_trip_with_new_fields() {
        let original = make_populated_cache();
        let bytes = bincode::serialize(&original).expect("serialize");
        let restored: RecipeCache = bincode::deserialize(&bytes).expect("deserialize");
        assert_eq!(original, restored);
        assert_eq!(restored.version, CACHE_VERSION);
        let step = restored.steps.get("compile_main").unwrap();
        assert_eq!(step.command_hash, 0x0102030405060708);
        assert_eq!(step.context_hash, 0x1111111111111111);
        assert_eq!(step.env_contribution, 0x2222222222222222);
    }

    #[test]
    fn empty_cache_round_trip() {
        let original = RecipeCache::new();
        let bytes = bincode::serialize(&original).expect("serialize");
        let restored: RecipeCache = bincode::deserialize(&bytes).expect("deserialize");
        assert_eq!(original, restored);
    }

    #[test]
    fn plate_step_no_output() {
        let step = StepEntry {
            inputs: vec![FileRecord {
                path: "src/main.c".to_string(),
                mtime: 1700000000,
                hash: 0x1234567890abcdef,
            }],
            outputs: vec![],
            command_hash: 0xdeadbeefcafe,
            context_hash: 0xc0c0c0c0,
            env_contribution: 0xe0e0e0e0,
        };
        let bytes = bincode::serialize(&step).expect("serialize");
        let restored: StepEntry = bincode::deserialize(&bytes).expect("deserialize");
        assert_eq!(step, restored);
    }

    #[test]
    fn save_and_load() {
        let dir = tempfile::tempdir().expect("tempdir");
        let original = make_populated_cache();
        original.save(dir.path(), "my_recipe").expect("save");
        let loaded = RecipeCache::load(dir.path(), "my_recipe").expect("load");
        assert_eq!(original, loaded);
    }

    #[test]
    fn load_missing_returns_none() {
        let dir = tempfile::tempdir().expect("tempdir");
        assert!(RecipeCache::load(dir.path(), "nonexistent").is_none());
    }

    #[test]
    fn load_corrupted_returns_none() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("bad.bin"), b"not bincode").expect("write");
        assert!(RecipeCache::load(dir.path(), "bad").is_none());
    }

    #[test]
    fn load_v2_returns_none_via_version_check() {
        let dir = tempfile::tempdir().expect("tempdir");
        // Hand-craft a "v2" cache: just write a struct with version=2.
        // We use a minimal serde value that bincode would accept as the v3 layout
        // but with version=2 — the version check rejects it before any field
        // mismatch matters.
        let mut wrong_version = RecipeCache::new();
        wrong_version.version = 2;
        let bytes = bincode::serialize(&wrong_version).expect("serialize");
        std::fs::write(dir.path().join("old.bin"), &bytes).expect("write");
        assert!(RecipeCache::load(dir.path(), "old").is_none());
    }
}
```

- [ ] **Step 2.1.4: Run store tests, expect pass**

Run: `cd /home/alex/dev/cook/cli && cargo test -p cook-cache --lib store`
Expected: 8 tests pass.

The rest of the cascade lands in Tasks 2.2–2.5; do NOT commit yet — the workspace will not build until check.rs and manager.rs catch up.

---

### Task 2.2: Update `cook-cache/src/check.rs` (new RebuildReason variants + comparison)

**Files:**
- Modify: `cli/crates/cook-cache/src/check.rs`

- [ ] **Step 2.2.1: Add new `RebuildReason` variants and update `needs_rebuild_*`**

Open `cli/crates/cook-cache/src/check.rs`. Find the `RebuildReason` enum (around line 76). Replace it with:

```rust
#[derive(Debug, PartialEq)]
pub enum RebuildReason {
    NoCacheEntry,
    CommandHashChanged,
    ContextChanged,        // NEW
    EnvChanged,            // NEW
    OutputMissing,
    OutputChanged,
    InputSetChanged,
    InputChanged(String),
}
```

- [ ] **Step 2.2.2: Update `needs_rebuild_cook` signature and body**

Replace the entire `needs_rebuild_cook` function (around line 128) with:

```rust
/// Check if a cook layer (with output) needs to rebuild.
/// INVARIANT: cook.layer() calls must NOT be nested.
pub fn needs_rebuild_cook(
    entry: Option<&StepEntry>,
    current_inputs: &[&str],
    current_outputs: &[&str],
    command_hash: u64,
    context_hash: u64,
    env_contribution: u64,
    working_dir: &Path,
) -> (RebuildResult, Option<StepEntry>) {
    let entry = match entry {
        None => return (RebuildResult::Rebuild(RebuildReason::NoCacheEntry), None),
        Some(e) => e,
    };
    if entry.command_hash != command_hash {
        return (RebuildResult::Rebuild(RebuildReason::CommandHashChanged), None);
    }
    if entry.context_hash != context_hash {
        return (RebuildResult::Rebuild(RebuildReason::ContextChanged), None);
    }
    if entry.env_contribution != env_contribution {
        return (RebuildResult::Rebuild(RebuildReason::EnvChanged), None);
    }

    // Check output count matches.
    if entry.outputs.len() != current_outputs.len() {
        return (RebuildResult::Rebuild(RebuildReason::OutputMissing), None);
    }

    for (cached_out, rel_path) in entry.outputs.iter().zip(current_outputs.iter()) {
        let abs_output = working_dir.join(rel_path);
        if !abs_output.exists() {
            return (RebuildResult::Rebuild(RebuildReason::OutputMissing), None);
        }
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

    match check_inputs(&entry.inputs, current_inputs, working_dir) {
        Err(reason) => (RebuildResult::Rebuild(reason), None),
        Ok(updated_inputs) => {
            let updated = StepEntry {
                inputs: updated_inputs,
                outputs: entry.outputs.clone(),
                command_hash: entry.command_hash,
                context_hash: entry.context_hash,
                env_contribution: entry.env_contribution,
            };
            (RebuildResult::Skip, Some(updated))
        }
    }
}
```

- [ ] **Step 2.2.3: Update `needs_rebuild_plate` signature and body**

Replace `needs_rebuild_plate` (around line 181) with:

```rust
/// Check if a plate layer (no output) needs to re-run.
pub fn needs_rebuild_plate(
    entry: Option<&StepEntry>,
    current_inputs: &[&str],
    command_hash: u64,
    context_hash: u64,
    env_contribution: u64,
    working_dir: &Path,
) -> (RebuildResult, Option<StepEntry>) {
    let entry = match entry {
        None => return (RebuildResult::Rebuild(RebuildReason::NoCacheEntry), None),
        Some(e) => e,
    };
    if entry.command_hash != command_hash {
        return (RebuildResult::Rebuild(RebuildReason::CommandHashChanged), None);
    }
    if entry.context_hash != context_hash {
        return (RebuildResult::Rebuild(RebuildReason::ContextChanged), None);
    }
    if entry.env_contribution != env_contribution {
        return (RebuildResult::Rebuild(RebuildReason::EnvChanged), None);
    }
    match check_inputs(&entry.inputs, current_inputs, working_dir) {
        Err(reason) => (RebuildResult::Rebuild(reason), None),
        Ok(updated_inputs) => {
            let updated = StepEntry {
                inputs: updated_inputs,
                outputs: vec![],
                command_hash: entry.command_hash,
                context_hash: entry.context_hash,
                env_contribution: entry.env_contribution,
            };
            (RebuildResult::Skip, Some(updated))
        }
    }
}
```

- [ ] **Step 2.2.4: Remove `hash_secondary_inputs` and its tests**

In `check.rs`, delete the entire `pub fn hash_secondary_inputs(...)` function (around lines 44–68) and the three tests `test_hash_secondary_*` from the `#[cfg(test)]` block (lines around 477–510).

- [ ] **Step 2.2.5: Update existing `check.rs` tests for the new signatures**

Every test in `check.rs` that calls `needs_rebuild_cook` or `needs_rebuild_plate` needs two new args (context_hash, env_contribution). Search for `needs_rebuild_cook(` and `needs_rebuild_plate(` in `check.rs` and update each call site to insert `0u64, 0u64` between `command_hash` and `working_dir`. Each existing test exercises the v2 fields; passing zeros for the new fields keeps the test semantics. Also each `StepEntry` literal in the test module needs `context_hash: 0` and `env_contribution: 0` lines added.

Concrete edit: replace each pattern like

```rust
let (result, updated) = needs_rebuild_cook(Some(&entry), &["in.c"], &["out.o"], 0xbeef, wd);
```

with

```rust
let (result, updated) = needs_rebuild_cook(Some(&entry), &["in.c"], &["out.o"], 0xbeef, 0, 0, wd);
```

and each `StepEntry { ... }` literal in the tests now needs `context_hash: 0,` and `env_contribution: 0,` after `command_hash:`.

- [ ] **Step 2.2.6: Add new tests for context/env rebuild reasons**

Append to the `#[cfg(test)] mod tests` block in `check.rs`:

```rust
    #[test]
    fn context_hash_changed_rebuilds() {
        let dir = tempfile::tempdir().expect("tempdir");
        let wd = dir.path();
        std::fs::write(wd.join("in.c"), b"int main(){}").expect("write");
        std::fs::write(wd.join("out.o"), b"binary").expect("write");

        let in_record = make_file_record("in.c", wd);
        let out_record = make_file_record("out.o", wd);

        let entry = StepEntry {
            inputs: vec![in_record],
            outputs: vec![out_record],
            command_hash: 0xbeef,
            context_hash: 0x1111,
            env_contribution: 0,
        };

        let (result, updated) = needs_rebuild_cook(Some(&entry), &["in.c"], &["out.o"], 0xbeef, 0x9999, 0, wd);
        assert_eq!(result, RebuildResult::Rebuild(RebuildReason::ContextChanged));
        assert!(updated.is_none());
    }

    #[test]
    fn env_contribution_changed_rebuilds() {
        let dir = tempfile::tempdir().expect("tempdir");
        let wd = dir.path();
        std::fs::write(wd.join("in.c"), b"int main(){}").expect("write");
        std::fs::write(wd.join("out.o"), b"binary").expect("write");

        let in_record = make_file_record("in.c", wd);
        let out_record = make_file_record("out.o", wd);

        let entry = StepEntry {
            inputs: vec![in_record],
            outputs: vec![out_record],
            command_hash: 0xbeef,
            context_hash: 0,
            env_contribution: 0x1111,
        };

        let (result, updated) = needs_rebuild_cook(Some(&entry), &["in.c"], &["out.o"], 0xbeef, 0, 0x9999, wd);
        assert_eq!(result, RebuildResult::Rebuild(RebuildReason::EnvChanged));
        assert!(updated.is_none());
    }

    #[test]
    fn plate_context_hash_changed_rebuilds() {
        let dir = tempfile::tempdir().expect("tempdir");
        let wd = dir.path();
        std::fs::write(wd.join("in.c"), b"int main(){}").expect("write");
        let in_record = make_file_record("in.c", wd);

        let entry = StepEntry {
            inputs: vec![in_record],
            outputs: vec![],
            command_hash: 0xbeef,
            context_hash: 0x1111,
            env_contribution: 0,
        };

        let (result, updated) = needs_rebuild_plate(Some(&entry), &["in.c"], 0xbeef, 0x9999, 0, wd);
        assert_eq!(result, RebuildResult::Rebuild(RebuildReason::ContextChanged));
        assert!(updated.is_none());
    }
```

Tasks 2.3, 2.4, 2.5 (manager.rs hardening, lib.rs re-exports, executor/dag_data lookup updates) follow before the single Phase 2 commit. Continue without committing.

---

### Task 2.3: Harden `record_completion` and remove dead methods in `manager.rs`

**Files:**
- Modify: `cli/crates/cook-cache/src/manager.rs`

- [ ] **Step 2.3.1: Add `RecordError` and update `record_completion` signature**

Open `cli/crates/cook-cache/src/manager.rs`. At the top of the file, after the existing `use` block, add:

```rust
use cook_contracts::CacheMeta;
```

Above `pub struct ThreadSafeCacheManager`, insert:

```rust
#[derive(Debug, thiserror::Error)]
pub enum RecordError {
    #[error("cache record skipped: input file missing or unreadable: {0}")]
    MissingFile(String),
    #[error("cache record skipped: output file missing or unreadable: {0}")]
    UnreadableFile(String),
}
```

This requires `thiserror` as a dependency. Add to `cli/crates/cook-cache/Cargo.toml` `[dependencies]`:

```toml
thiserror = "1"
```

- [ ] **Step 2.3.2: Replace `record_completion` body**

Find the existing `record_completion` method (around line 160) and replace its entire body:

```rust
    pub fn record_completion(
        &self,
        recipe_name: &str,
        cache_key: &str,
        meta: &CacheMeta,
        working_dir: &Path,
    ) -> Result<(), RecordError> {
        let new_inputs = collect_records(&meta.input_paths, working_dir)
            .map_err(|p| RecordError::MissingFile(p))?;
        let new_outputs = collect_records(&meta.output_paths, working_dir)
            .map_err(|p| RecordError::UnreadableFile(p))?;

        self.update_step(
            recipe_name,
            cache_key,
            StepEntry {
                inputs: new_inputs,
                outputs: new_outputs,
                command_hash: meta.command_hash,
                context_hash: meta.context_hash,
                env_contribution: meta.env_contribution,
            },
        );
        Ok(())
    }
```

Add the helper function elsewhere in the file (e.g., near the top, after the `use` block):

```rust
/// Build FileRecord vec for a list of relative paths. Bails on the first
/// path whose mtime or content cannot be read. Returning Err from here
/// causes record_completion to skip the cache write entirely.
fn collect_records(paths: &[String], working_dir: &Path) -> Result<Vec<FileRecord>, String> {
    let mut out = Vec::with_capacity(paths.len());
    for rel in paths {
        let abs = working_dir.join(rel);
        let mtime = stat_mtime(&abs).ok_or_else(|| rel.clone())?;
        let hash = hash_file(&abs).ok_or_else(|| rel.clone())?;
        out.push(FileRecord { path: rel.clone(), mtime, hash });
    }
    Ok(out)
}
```

- [ ] **Step 2.3.3: Remove `invalidate_recipe` from `CacheState`**

In `manager.rs`, find `impl CacheState { ... pub fn invalidate_recipe(...) { ... } }` (around line 37). Delete the entire `invalidate_recipe` method (about 35 lines, ending where `files_per_glob` starts). The `CacheState` struct itself is preserved; only the method is removed.

The `invalidate_recipe` method also wrote to `cache.secondary_inputs_hash` and `cache.env_hash` — both fields are now gone, so any leftover reference will fail to compile, confirming the deletion is complete.

- [ ] **Step 2.3.4: Remove `invalidate_if_env_changed` from `ThreadSafeCacheManager`**

In `manager.rs`, find `impl ThreadSafeCacheManager { ... pub fn invalidate_if_env_changed(...) { ... } }` (around line 145). Delete the entire method (about 14 lines, ending just before `pub fn record_completion`).

- [ ] **Step 2.3.5: Update existing `manager.rs` tests**

In the `#[cfg(test)] mod tests` block:

a) Update `make_step_entry` (around line 210) to include the new fields:

```rust
    fn make_step_entry(command_hash: u64) -> StepEntry {
        StepEntry {
            inputs: vec![FileRecord {
                path: "src/main.c".to_string(),
                mtime: 1700000000,
                hash: 0xaabbccdd,
            }],
            outputs: vec![FileRecord {
                path: "build/main.o".to_string(),
                mtime: 1700000100,
                hash: 0x11223344,
            }],
            command_hash,
            context_hash: 0,
            env_contribution: 0,
        }
    }
```

b) Delete the two tests that exercise the now-removed `invalidate_if_env_changed`: `test_invalidate_if_env_changed_clears_steps` and `test_invalidate_if_env_changed_no_op_on_match`.

- [ ] **Step 2.3.6: Add tests for hardened `record_completion`**

Append to the `#[cfg(test)] mod tests` block in `manager.rs`:

```rust
    fn make_cache_meta(input_paths: Vec<String>, output_paths: Vec<String>) -> cook_contracts::CacheMeta {
        cook_contracts::CacheMeta {
            recipe_name: "test_recipe".into(),
            project_id: String::new(),
            cookfile_path: String::new(),
            cache_key: "step_one".into(),
            input_paths,
            output_paths,
            command_hash: 0xdeadbeef,
            context_hash: 0,
            env_contribution: 0,
            consulted_env: std::collections::BTreeMap::new(),
        }
    }

    #[test]
    fn record_completion_writes_full_step_entry() {
        let dir = tempfile::tempdir().expect("tempdir");
        let wd = dir.path();
        std::fs::write(wd.join("in.c"), b"int main(){}").expect("write");
        std::fs::write(wd.join("out.o"), b"binary").expect("write");

        let cache_dir = dir.path().join("cache");
        std::fs::create_dir_all(&cache_dir).expect("mkdir cache");
        let cm = ThreadSafeCacheManager::new(cache_dir.clone());

        let meta = make_cache_meta(vec!["in.c".into()], vec!["out.o".into()]);
        cm.record_completion("rec", "step_one", &meta, wd).expect("record ok");
        cm.flush_all().expect("flush");

        let loaded = store::RecipeCache::load(&cache_dir, "rec").expect("load");
        let entry = loaded.steps.get("step_one").expect("step");
        assert_eq!(entry.command_hash, 0xdeadbeef);
        assert_eq!(entry.inputs.len(), 1);
        assert_eq!(entry.outputs.len(), 1);
    }

    #[test]
    fn record_completion_skips_on_missing_input() {
        let dir = tempfile::tempdir().expect("tempdir");
        let wd = dir.path();
        // Do NOT create "in.c" — record_completion should skip.
        std::fs::write(wd.join("out.o"), b"binary").expect("write");

        let cache_dir = dir.path().join("cache");
        std::fs::create_dir_all(&cache_dir).expect("mkdir");
        let cm = ThreadSafeCacheManager::new(cache_dir.clone());

        let meta = make_cache_meta(vec!["in.c".into()], vec!["out.o".into()]);
        let err = cm.record_completion("rec", "step_one", &meta, wd).unwrap_err();
        assert!(matches!(err, RecordError::MissingFile(_)));

        // Verify nothing was written.
        cm.flush_all().expect("flush");
        let loaded = store::RecipeCache::load(&cache_dir, "rec");
        assert!(loaded.is_none() || loaded.unwrap().steps.is_empty());
    }

    #[test]
    fn record_completion_preserves_prior_entry_on_skip() {
        let dir = tempfile::tempdir().expect("tempdir");
        let wd = dir.path();
        std::fs::write(wd.join("in.c"), b"int main(){}").expect("write");
        std::fs::write(wd.join("out.o"), b"binary").expect("write");

        let cache_dir = dir.path().join("cache");
        std::fs::create_dir_all(&cache_dir).expect("mkdir");
        let cm = ThreadSafeCacheManager::new(cache_dir.clone());

        // First successful record.
        let meta = make_cache_meta(vec!["in.c".into()], vec!["out.o".into()]);
        cm.record_completion("rec", "step_one", &meta, wd).expect("record 1");
        cm.flush_all().expect("flush 1");

        // Now remove the input and try again — must err and leave prior entry intact.
        std::fs::remove_file(wd.join("in.c")).expect("rm");
        let err = cm.record_completion("rec", "step_one", &meta, wd).unwrap_err();
        assert!(matches!(err, RecordError::MissingFile(_)));
        cm.flush_all().expect("flush 2");

        let loaded = store::RecipeCache::load(&cache_dir, "rec").expect("load");
        let entry = loaded.steps.get("step_one").expect("prior entry survives");
        assert_eq!(entry.command_hash, 0xdeadbeef);
    }
```

---

### Task 2.4: Remove `hash_secondary_inputs` re-export from `lib.rs`

**Files:**
- Modify: `cli/crates/cook-cache/src/lib.rs`

- [ ] **Step 2.4.1: Drop the dead re-export**

Open `cli/crates/cook-cache/src/lib.rs`. The current `pub use check::{...}` re-exports `hash_secondary_inputs`. Replace with:

```rust
pub use check::{
    hash_env, hash_file, needs_rebuild_cook, needs_rebuild_plate,
    stat_mtime, RebuildReason, RebuildResult,
};
pub use manager::{CacheState, RecordError, SharedCacheState, ThreadSafeCacheManager};
pub use store::{FileRecord, RecipeCache, StepEntry, CACHE_VERSION};
```

(`hash_secondary_inputs` was deleted in Step 2.2.4; this step removes it from the re-export list. `RecordError` is added because Phase 6 callers will need it.)

---

### Task 2.5: Update `executor.rs` and `dag_data.rs` for new check signatures

**Files:**
- Modify: `cli/crates/cook-engine/src/executor.rs`
- Modify: `cli/crates/cook-cli/src/dag_data.rs`

- [ ] **Step 2.5.1: Update `executor.rs` to pass new check params**

Open `cli/crates/cook-engine/src/executor.rs`. Find the `needs_rebuild_*` call (around line 254 — right after `let entry = cache.steps.get(&meta.cache_key);`). The call currently passes `meta.command_hash`. Update to also pass the meta's context and env contribution:

```rust
            let (result, updated_entry) = needs_rebuild_cook(
                entry,
                &input_paths_refs,
                &output_paths_refs,
                meta.command_hash,
                meta.context_hash,
                meta.env_contribution,
                working_dir,
            );
```

Apply the same change to any `needs_rebuild_plate` call site in the same file (search for `needs_rebuild_plate(`).

Find any `cm.record_completion(...)` call and update to pass `&meta` instead of the legacy positional args, and to handle the new `Result`:

```rust
            if let Err(e) = cm.record_completion(&meta.recipe_name, &meta.cache_key, meta, working_dir) {
                tracing::warn!("cache: skipping record for {}::{}: {e}", meta.recipe_name, meta.cache_key);
            }
```

- [ ] **Step 2.5.2: Update `cook-cli/src/dag_data.rs` similarly**

Open `cli/crates/cook-cli/src/dag_data.rs`. Find the `needs_rebuild_*` call (around line 232–268 area; the existing read of `meta.command_hash`). Apply the same six-arg change as in 2.5.1.

If `dag_data.rs` does not record completions (it's a read-only DAG inspection path), no changes to record_completion are needed there — only the rebuild-check signature update.

- [ ] **Step 2.5.3: Build the workspace**

Run: `cd /home/alex/dev/cook/cli && cargo build -q`
Expected: builds clean. Any remaining StepEntry construction in test fixtures elsewhere that still lacks `context_hash`/`env_contribution` will fail to compile — fix each by adding the two zero fields.

- [ ] **Step 2.5.4: Run all workspace tests**

Run: `cd /home/alex/dev/cook/cli && cargo test -q`
Expected: all pass. Behavior is v2-equivalent (all new fields are zero everywhere); the new test variants (ContextChanged, EnvChanged) pass because they explicitly trigger the new comparison paths.

- [ ] **Step 2.5.5: Single Phase 2 commit**

```bash
git add cli/crates/cook-cache/Cargo.toml cli/crates/cook-cache/src/store.rs cli/crates/cook-cache/src/check.rs cli/crates/cook-cache/src/manager.rs cli/crates/cook-cache/src/lib.rs cli/crates/cook-engine/src/executor.rs cli/crates/cook-cli/src/dag_data.rs cli/Cargo.lock
git commit -m "$(cat <<'EOF'
feat(SHI-140): cache schema v3 cascade

CACHE_VERSION 2→3. StepEntry gains context_hash and env_contribution
(both u64, xxh3-keyed). RecipeCache loses secondary_inputs_hash and
env_hash; CacheState loses invalidate_recipe;
ThreadSafeCacheManager loses invalidate_if_env_changed (recipe-wide
wipe replaced by per-step keying). hash_secondary_inputs and its
tests deleted. record_completion now returns Result<(), RecordError>
and skips the cache write entirely on missing/unreadable
input/output — no more (mtime=0, hash=0) poison.

needs_rebuild_cook / needs_rebuild_plate gain context_hash and
env_contribution params and emit RebuildReason::ContextChanged /
EnvChanged. v2 cache files on disk return None from
RecipeCache::load via the version check (existing mechanism, no
migration code). cook-engine/executor.rs and cook-cli/dag_data.rs
updated to thread the new comparison values from CacheMeta.

Behavior remains v2-equivalent until Phases 5–6 wire real values
into context_hash and env_contribution; this commit is purely a
shape change with hardening.

Closes (in part): SHI-141, SHI-142, SHI-145, SHI-146.
EOF
)"
```

---

## Phase 3 — `.cook/cloud.toml` parser in `cook-engine`

Phase 3 adds the `CloudConfig` type and parser. The parser is independent — no consumer wires it in until Phase 6.

### Task 3.1: `CloudConfig` parser

**Files:**
- Modify: `cli/crates/cook-engine/Cargo.toml`
- Create: `cli/crates/cook-engine/src/cloud_config.rs`
- Modify: `cli/crates/cook-engine/src/lib.rs`

- [ ] **Step 3.1.1: Add `toml` dep to cook-engine**

In `cli/crates/cook-engine/Cargo.toml` `[dependencies]`, add:

```toml
toml = "0.8"
serde = { version = "1", features = ["derive"] }
```

(Add `serde` if it's not already there — the existing engine probably has it transitively, but be explicit.)

- [ ] **Step 3.1.2: Write the failing tests**

Create `cli/crates/cook-engine/src/cloud_config.rs` with the test module first:

```rust
//! Parse `.cook/cloud.toml` — the project-level cloud config.
//!
//! Spec §9. The file is optional; if missing or empty, defaults apply.

use std::path::{Path, PathBuf};

use serde::Deserialize;

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write_toml(dir: &Path, contents: &str) -> PathBuf {
        let cook_dir = dir.join(".cook");
        std::fs::create_dir_all(&cook_dir).expect("mkdir");
        let path = cook_dir.join("cloud.toml");
        let mut f = std::fs::File::create(&path).expect("create");
        f.write_all(contents.as_bytes()).expect("write");
        path
    }

    #[test]
    fn missing_file_returns_default() {
        let dir = tempfile::tempdir().expect("tempdir");
        let cfg = CloudConfig::load_or_default(dir.path()).expect("load");
        assert!(!cfg.cloud.enabled);
        assert_eq!(cfg.project_id_or_fallback(dir.path()), dir.path().file_name().unwrap().to_string_lossy());
        assert!(cfg.cache_ignore_env().is_empty());
    }

    #[test]
    fn cloud_disabled_no_project_required() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_toml(dir.path(), r#"
[cloud]
enabled = false
"#);
        let cfg = CloudConfig::load_or_default(dir.path()).expect("load");
        assert!(!cfg.cloud.enabled);
        // No project required when disabled.
    }

    #[test]
    fn cloud_enabled_requires_project() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_toml(dir.path(), r#"
[cloud]
enabled = true
endpoint = "https://api.cook.dev"
"#);
        let result = CloudConfig::load_or_default(dir.path());
        assert!(result.is_err(), "missing project must error when cloud.enabled=true");
    }

    #[test]
    fn cloud_enabled_with_project_ok() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_toml(dir.path(), r#"
[cloud]
enabled = true
endpoint = "https://api.cook.dev"
project = "cook"
"#);
        let cfg = CloudConfig::load_or_default(dir.path()).expect("load");
        assert!(cfg.cloud.enabled);
        assert_eq!(cfg.cloud.project.as_deref(), Some("cook"));
    }

    #[test]
    fn cache_ignore_env_parsed() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_toml(dir.path(), r#"
[cache]
ignore_env = ["GITHUB_TOKEN", "MY_API_KEY"]
"#);
        let cfg = CloudConfig::load_or_default(dir.path()).expect("load");
        let ignore = cfg.cache_ignore_env();
        assert_eq!(ignore.len(), 2);
        assert!(ignore.contains(&"GITHUB_TOKEN".to_string()));
        assert!(ignore.contains(&"MY_API_KEY".to_string()));
    }

    #[test]
    fn malformed_toml_errors() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_toml(dir.path(), "this is not valid toml === ");
        assert!(CloudConfig::load_or_default(dir.path()).is_err());
    }

    #[test]
    fn project_id_or_fallback_uses_dir_name_when_no_project() {
        let dir = tempfile::tempdir().expect("tempdir");
        let project_dir = dir.path().join("my-cool-project");
        std::fs::create_dir_all(&project_dir).expect("mkdir");
        let cfg = CloudConfig::load_or_default(&project_dir).expect("load");
        assert_eq!(cfg.project_id_or_fallback(&project_dir), "my-cool-project");
    }
}
```

Run: `cd /home/alex/dev/cook/cli && cargo test -p cook-engine --lib cloud_config -- --nocapture`
Expected: COMPILE FAILURE.

- [ ] **Step 3.1.3: Implement `CloudConfig`**

Above the test module, add:

```rust
#[derive(Debug, Clone, Default, Deserialize)]
pub struct CloudConfig {
    #[serde(default)]
    pub cloud: CloudSection,
    #[serde(default)]
    pub cache: CacheSection,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct CloudSection {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub endpoint: Option<String>,
    #[serde(default)]
    pub project: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct CacheSection {
    #[serde(default)]
    pub ignore_env: Vec<String>,
    #[serde(default)]
    pub cache_dir: Option<String>,
}

#[derive(Debug)]
pub enum CloudConfigError {
    Io(std::io::Error),
    Parse(toml::de::Error),
    MissingProject,
}

impl std::fmt::Display for CloudConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "reading .cook/cloud.toml: {e}"),
            Self::Parse(e) => write!(f, "parsing .cook/cloud.toml: {e}"),
            Self::MissingProject => write!(
                f,
                "[cloud] enabled=true but [cloud] project is missing — \
                 set `project = \"...\"` in .cook/cloud.toml or set `enabled = false`"
            ),
        }
    }
}

impl std::error::Error for CloudConfigError {}

impl CloudConfig {
    /// Load `.cook/cloud.toml` from `project_root`. Returns `Default` if absent.
    /// Validates that `project` is set when `cloud.enabled = true`.
    pub fn load_or_default(project_root: &Path) -> Result<Self, CloudConfigError> {
        let path = project_root.join(".cook").join("cloud.toml");
        let cfg = if !path.exists() {
            Self::default()
        } else {
            let bytes = std::fs::read_to_string(&path).map_err(CloudConfigError::Io)?;
            toml::from_str::<Self>(&bytes).map_err(CloudConfigError::Parse)?
        };

        if cfg.cloud.enabled && cfg.cloud.project.is_none() {
            return Err(CloudConfigError::MissingProject);
        }
        Ok(cfg)
    }

    /// Returns the configured project_id, or the project root directory name
    /// as a fallback (only valid when cloud is disabled).
    pub fn project_id_or_fallback(&self, project_root: &Path) -> String {
        if let Some(p) = &self.cloud.project {
            return p.clone();
        }
        project_root
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "unknown".to_string())
    }

    pub fn cache_ignore_env(&self) -> &[String] {
        &self.cache.ignore_env
    }

    pub fn cache_dir(&self) -> Option<&str> {
        self.cache.cache_dir.as_deref()
    }
}
```

- [ ] **Step 3.1.4: Register the module in `cook-engine/src/lib.rs`**

Open `cli/crates/cook-engine/src/lib.rs`. After the existing `pub mod` declarations, add:

```rust
pub mod cloud_config;
```

- [ ] **Step 3.1.5: Run tests, expect pass**

Run: `cd /home/alex/dev/cook/cli && cargo test -p cook-engine --lib cloud_config`
Expected: 7 tests pass.

- [ ] **Step 3.1.6: Run all workspace tests**

Run: `cd /home/alex/dev/cook/cli && cargo test -q`
Expected: all pass.

- [ ] **Step 3.1.7: Commit**

```bash
git add cli/crates/cook-engine/Cargo.toml cli/crates/cook-engine/src/cloud_config.rs cli/crates/cook-engine/src/lib.rs cli/Cargo.lock
git commit -m "$(cat <<'EOF'
feat(SHI-140): .cook/cloud.toml parser (CloudConfig)

[cloud] section: enabled (bool), endpoint (str), project (str —
required when enabled). [cache] section: ignore_env (list of names
extending the D1 baseline), cache_dir (LocalBackend root override).

Validation: enabled=true requires project (returns
CloudConfigError::MissingProject). project_id_or_fallback() uses
the project root directory name when project is unset (only valid
under enabled=false).

Not yet wired into the build bootstrap — Phase 6.
EOF
)"
```

---

## Phase 4 — `cook-luagen` consulted-env enumeration

`cook-luagen/src/template.rs` does the `{TOKEN}` → `cook.env[TOKEN]` substitution at codegen time. Phase 4 instruments those expansion sites to record the token names that fall through to env, and emits them on the generated `cook.add_unit({...})` table as a `consulted_env_keys` field. After Phase 4, every cacheable unit's add_unit table carries the static list; cook-register reads it in Phase 5.

### Task 4.1: Add a `ConsultedEnv` accumulator and thread it through template expansion

**Files:**
- Modify: `cli/crates/cook-luagen/src/template.rs`

- [ ] **Step 4.1.1: Add the `ConsultedEnv` type at the top of `template.rs`**

Open `cli/crates/cook-luagen/src/template.rs`. After the existing `use` block at the top, add:

```rust
use std::collections::BTreeSet;

/// Accumulator for env keys that fall through to `cook.env[KEY]` during
/// template expansion. Populated by every expansion path that reaches the
/// "fallback to env" branch. The set is sorted (BTreeSet) so the resulting
/// emitted Lua table is deterministic.
#[derive(Debug, Default, Clone)]
pub struct ConsultedEnv {
    pub keys: BTreeSet<String>,
}

impl ConsultedEnv {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn record(&mut self, key: &str) {
        self.keys.insert(key.to_string());
    }

    /// Render as a Lua table literal: `{"A", "B", "C"}`.
    /// Returns `"{}"` for an empty set.
    pub fn to_lua_table(&self) -> String {
        if self.keys.is_empty() {
            return "{}".to_string();
        }
        let parts: Vec<String> = self.keys.iter().map(|k| format!("\"{}\"", crate::escape_lua_string(k))).collect();
        format!("{{{}}}", parts.join(", "))
    }
}
```

If `escape_lua_string` is private to a different module, replace `crate::escape_lua_string(k)` with the inlined logic from `template.rs` (search `escape_lua_string` to find the existing function — `template.rs` already calls it, so the function is accessible at the call sites).

- [ ] **Step 4.1.2: Add a `record` parameter to every expansion function in `template.rs`**

`template.rs` contains multiple expansion functions: `expand_token` (around line 95), the unit-test helper at line ~193, the body-text expander around line ~199, the cook-step expander around line ~592, the plate/test expander around line ~577.

For **each** function whose body produces `format!("cook.env[\"{}\"]", escape_lua_string(inner))`, add an `out: &mut ConsultedEnv` parameter, and call `out.record(inner)` immediately before constructing the `cook.env[…]` string. Concretely, find every occurrence of:

```rust
format!("cook.env[\"{}\"]", escape_lua_string(inner))
```

and replace with:

```rust
{
    out.record(inner);
    format!("cook.env[\"{}\"]", escape_lua_string(inner))
}
```

This requires the enclosing function to receive `out: &mut ConsultedEnv` as a parameter. Update each function's signature and every call site in `template.rs` to pass a `&mut ConsultedEnv`.

For functions that previously had no callers needing the consulted set (e.g., pure validation walkers like `validate_plate_test_placeholders`), do NOT add the parameter — only the *expansion* paths that produce cook.env reads need the accumulator.

- [ ] **Step 4.1.3: Update `template.rs` unit tests**

Each test in `template.rs`'s `#[cfg(test)]` block that calls an expansion function now needs to pass a `&mut ConsultedEnv`. Add `let mut consulted = ConsultedEnv::new();` to each affected test, pass `&mut consulted` to the expansion call, and (where the test asserts on the generated Lua) optionally assert on `consulted.keys`.

For the existing test at line ~502 ("should expand `{{CC}}` to `cook.env[\"CC\"]`"), add an assertion:

```rust
    assert!(consulted.keys.contains("CC"), "consulted should record CC");
    assert!(consulted.keys.contains("CFLAGS"), "consulted should record CFLAGS");
```

after the existing assertions.

- [ ] **Step 4.1.4: Add new tests for ConsultedEnv**

Append to the `#[cfg(test)]` block in `template.rs`:

```rust
    #[test]
    fn consulted_env_to_lua_table_empty() {
        let c = ConsultedEnv::new();
        assert_eq!(c.to_lua_table(), "{}");
    }

    #[test]
    fn consulted_env_to_lua_table_sorted() {
        let mut c = ConsultedEnv::new();
        c.record("Z");
        c.record("A");
        c.record("M");
        assert_eq!(c.to_lua_table(), "{\"A\", \"M\", \"Z\"}");
    }

    #[test]
    fn consulted_env_record_dedups() {
        let mut c = ConsultedEnv::new();
        c.record("CFLAGS");
        c.record("CFLAGS");
        c.record("CFLAGS");
        assert_eq!(c.keys.len(), 1);
    }

    #[test]
    fn consulted_env_to_lua_table_escapes_quotes() {
        let mut c = ConsultedEnv::new();
        c.record("KEY\"WITH\"QUOTES");
        // The escape function should escape the embedded quotes.
        assert!(c.to_lua_table().contains("\\\""));
    }
```

- [ ] **Step 4.1.5: Run cook-luagen tests, expect pass**

Run: `cd /home/alex/dev/cook/cli && cargo test -p cook-luagen`
Expected: all existing tests pass; the four new ConsultedEnv tests pass.

- [ ] **Step 4.1.6: Commit (intermediate — Phase 4 not yet emitting on add_unit)**

```bash
git add cli/crates/cook-luagen/src/template.rs
git commit -m "$(cat <<'EOF'
feat(SHI-142): ConsultedEnv accumulator in template expansion

Every {TOKEN} fall-through to cook.env[TOKEN] during codegen now
records TOKEN in a ConsultedEnv accumulator threaded through the
expansion functions. The accumulator's to_lua_table() renders as a
sorted, deterministic Lua table literal.

Not yet emitted on cook.add_unit — that lands in Task 4.2.
EOF
)"
```

---

### Task 4.2: Emit `consulted_env_keys` on `cook.add_unit` for cook/plate/test steps

**Files:**
- Modify: `cli/crates/cook-luagen/src/cook_step.rs`
- Modify: `cli/crates/cook-luagen/src/plate_step.rs`
- Modify: `cli/crates/cook-luagen/src/test_step.rs`

- [ ] **Step 4.2.1: Plumb ConsultedEnv through cook_step.rs**

Open `cli/crates/cook-luagen/src/cook_step.rs`. Find the function that builds the `cook.add_unit({...})` Lua text (search for `cook.add_unit`). For shell-command-using cook steps and lua-block-using cook steps:

a) At the start of the function, allocate a fresh accumulator:

```rust
    let mut consulted = crate::template::ConsultedEnv::new();
```

b) Pass `&mut consulted` into every template-expansion call this function makes (the calls now require it after Task 4.1).

c) When emitting the `cook.add_unit({...})` Lua source, append a `consulted_env_keys = {...}` line just before the closing brace. Find the existing emission (it produces lines like `command = "...",`, `inputs = {...},`, `output = "...",` etc.) and add:

```rust
    out.push_str(&format!("    consulted_env_keys = {},\n", consulted.to_lua_table()));
```

(Adjust the indentation to match existing emission style.)

For **lua_block** payloads (`using >{ ... }`), emit `consulted_env_keys = "*"` (a string sentinel) instead of a list. Cook-register treats `"*"` as "hash everything in cook.env (denylist-filtered)", per spec §4.3 final paragraph. Emission:

```rust
    out.push_str("    consulted_env_keys = \"*\",\n");
```

- [ ] **Step 4.2.2: Same plumbing for plate_step.rs**

Apply the same emission pattern to `cli/crates/cook-luagen/src/plate_step.rs`. Plate steps produce no output but consume env the same way; emit `consulted_env_keys` identically.

- [ ] **Step 4.2.3: Same plumbing for test_step.rs**

Apply the same emission pattern to `cli/crates/cook-luagen/src/test_step.rs`.

- [ ] **Step 4.2.4: Update existing cook-luagen tests**

Tests that snapshot the generated Lua source need their snapshots updated to include the new `consulted_env_keys = {...}` line. Run the cook-luagen test suite and observe failures; update each snapshot. For any test that previously asserted a specific generated-Lua substring, also assert that `consulted_env_keys` appears with the expected list (e.g., `{"CC", "CFLAGS"}` for a template containing `{CC} {CFLAGS}`).

- [ ] **Step 4.2.5: Add a positive integration test**

Append to `cli/crates/cook-luagen/src/tests.rs`:

```rust
#[test]
fn cook_step_with_env_tokens_emits_consulted_env_keys() {
    // Recipe with `using "gcc {CFLAGS} -c {in} -o {out}"` should emit
    // consulted_env_keys = {"CFLAGS"} (in/out are placeholders, not env).
    let cookfile_text = r#"
recipe build
    ingredients "src/*.c"
    cook "build/{stem}.o" using "gcc {CFLAGS} -c {in} -o {out}"
end
"#;
    let lua = generate_lua_for_test(cookfile_text);
    assert!(
        lua.contains("consulted_env_keys = {\"CFLAGS\"}"),
        "expected consulted_env_keys with CFLAGS, got:\n{lua}"
    );
}

#[test]
fn lua_block_step_emits_star_sentinel() {
    let cookfile_text = r#"
recipe build
    ingredients "src/*.c"
    cook "build/{stem}.o" using >{ os.execute("gcc " .. cook.env.CFLAGS .. " -c " .. inputs[1] .. " -o " .. outputs[1]) }
end
"#;
    let lua = generate_lua_for_test(cookfile_text);
    assert!(
        lua.contains("consulted_env_keys = \"*\""),
        "lua_block payload should emit star sentinel, got:\n{lua}"
    );
}
```

If `generate_lua_for_test` doesn't exist, replace with the test harness function the existing tests in `tests.rs` already use (e.g., the result of `cook_luagen::generate_recipe_module(...)` — search the file for an existing `fn` that produces a Lua string from a Cookfile string).

- [ ] **Step 4.2.6: Run all cook-luagen tests**

Run: `cd /home/alex/dev/cook/cli && cargo test -p cook-luagen`
Expected: all tests pass, including the two new ones.

- [ ] **Step 4.2.7: Build the workspace, expect pass**

Run: `cd /home/alex/dev/cook/cli && cargo build -q`
Expected: builds clean. Cook-register doesn't yet read `consulted_env_keys` — it ignores unknown table fields.

- [ ] **Step 4.2.8: Commit**

```bash
git add cli/crates/cook-luagen/src/cook_step.rs cli/crates/cook-luagen/src/plate_step.rs cli/crates/cook-luagen/src/test_step.rs cli/crates/cook-luagen/src/tests.rs
git commit -m "$(cat <<'EOF'
feat(SHI-142): emit consulted_env_keys on cook.add_unit tables

For shell-template-using cook/plate/test steps, cook-luagen now
emits the BTreeSet of env names that fell through to cook.env[NAME]
during template expansion as a Lua table literal.

For lua_block payloads (using >{ ... }), emits the sentinel "*" —
cook-register will treat this as "hash everything in cook.env minus
the denylist" per spec §4.3.

Cook-register does not yet read this field — Phase 5.
EOF
)"
```

---

## Phase 5 — Engine bootstrap + cook-register populates real CacheMeta values

Phase 5 brings cross-machine correctness live. The engine probes `ExecutionContext`, builds the `EnvDenylist` from `CloudConfig`, constructs a `LocalBackend`, and threads the resulting `CacheContext` into the register-phase Lua VM. cook-register's `add_unit` reads `consulted_env_keys`, looks up current env values, and populates the real `context_hash` / `env_contribution` / `consulted_env` / `project_id` / `cookfile_path` fields on `CacheMeta`.

### Task 5.1: `CacheContext` aggregation type

**Files:**
- Create: `cli/crates/cook-engine/src/cache_ctx.rs`
- Modify: `cli/crates/cook-engine/src/lib.rs`

- [ ] **Step 5.1.1: Create `CacheContext` struct**

Create `cli/crates/cook-engine/src/cache_ctx.rs`:

```rust
//! CacheContext — single struct aggregating everything the cache layer needs:
//! machine identity, env denylist, backend, and project config. Built once
//! per `cook build` invocation in run.rs and threaded down.

use std::path::PathBuf;
use std::sync::Arc;

use cook_cache::backend::CacheBackend;
use cook_cache::context::ExecutionContext;
use cook_cache::envkey::EnvDenylist;

use crate::cloud_config::CloudConfig;

#[derive(Clone)]
pub struct CacheContext {
    pub exec_ctx: Arc<ExecutionContext>,
    pub denylist: Arc<EnvDenylist>,
    pub backend: Arc<dyn CacheBackend>,
    pub cloud_config: Arc<CloudConfig>,
    pub project_root: PathBuf,
    pub project_id: String,
}
```

- [ ] **Step 5.1.2: Register the module**

In `cli/crates/cook-engine/src/lib.rs`, add:

```rust
pub mod cache_ctx;
```

- [ ] **Step 5.1.3: Build, expect pass**

Run: `cd /home/alex/dev/cook/cli && cargo build -p cook-engine -q`
Expected: builds clean.

- [ ] **Step 5.1.4: Commit**

```bash
git add cli/crates/cook-engine/src/cache_ctx.rs cli/crates/cook-engine/src/lib.rs
git commit -m "$(cat <<'EOF'
feat(SHI-140): CacheContext aggregation type in cook-engine

Single Arc-wrapped struct carrying ExecutionContext, EnvDenylist,
CacheBackend, CloudConfig, project_root, project_id. Built once
per build in run.rs (Task 5.2); threaded into register-phase VM
(Task 5.3); read by cook-register's add_unit (Task 5.4).
EOF
)"
```

---

### Task 5.2: Engine bootstrap — probe + denylist + backend + CacheContext

**Files:**
- Modify: `cli/crates/cook-engine/src/run.rs`

- [ ] **Step 5.2.1: Find the build entry point**

Open `cli/crates/cook-engine/src/run.rs`. Find the function that begins a `cook build` (the one that today calls `invalidate_if_env_changed` around line 165–169 — that call no longer compiles after Phase 2). Search for `hash_env(&units.env_vars)`. The enclosing function is the build entry.

- [ ] **Step 5.2.2: Replace the dead `invalidate_if_env_changed` call site**

In that function, locate the lines:

```rust
            let env_hash = hash_env(&units.env_vars);
            self.thread_safe_cache_manager
                .invalidate_if_env_changed(name, env_hash);
```

Delete those three lines (the method no longer exists; per Phase 2 the recipe-wide wipe is replaced by per-step keying).

- [ ] **Step 5.2.3: Bootstrap CacheContext at build start**

Find the *outermost* build entry function (before per-recipe iteration). Insert, near the top of that function (after the recipe set is loaded but before any register-phase invocation):

```rust
    // ── Cache cloud-readiness bootstrap (spec §3.3) ──────────────────────
    use std::sync::Arc;
    use cook_cache::backend::{CacheBackend, BackendError};
    use cook_cache::context::ExecutionContext;
    use cook_cache::envkey::EnvDenylist;
    use crate::cache_ctx::CacheContext;
    use crate::cloud_config::CloudConfig;

    let cloud_config = CloudConfig::load_or_default(&project_root)
        .map_err(|e| anyhow::anyhow!("invalid .cook/cloud.toml: {e}"))?;

    let mut denylist = EnvDenylist::baseline();
    denylist.extend_with(cloud_config.cache_ignore_env());
    let denylist = Arc::new(denylist);

    let exec_ctx = Arc::new(ExecutionContext::probe());

    let cache_dir = cloud_config
        .cache_dir()
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| {
            dirs::cache_dir()
                .unwrap_or_else(|| std::env::temp_dir())
                .join("cook")
                .join("cloud")
        });
    let backend: Arc<dyn CacheBackend> = Arc::new(cook_cache::backend::LocalBackend::new(cache_dir));
    if let Err(e) = backend.health() {
        tracing::warn!("cache backend unavailable: {e}; continuing with backend disabled");
    }

    let project_id = cloud_config.project_id_or_fallback(&project_root);
    let cache_ctx = Arc::new(CacheContext {
        exec_ctx: exec_ctx.clone(),
        denylist: denylist.clone(),
        backend: backend.clone(),
        cloud_config: Arc::new(cloud_config),
        project_root: project_root.clone(),
        project_id,
    });
    // ── End bootstrap ────────────────────────────────────────────────────
```

The variable name `project_root` and the error type (`anyhow::anyhow!`) match the existing `run.rs` patterns; if your file uses different names (search for the existing project root computation), substitute them. If `dirs` is not a workspace dep, add it to `cli/crates/cook-engine/Cargo.toml`:

```toml
dirs = "5"
```

The `tracing` macro requires `tracing` to be in `cook-engine`'s deps; add if missing.

- [ ] **Step 5.2.4: Pass `cache_ctx` into per-recipe register invocations**

The same `run.rs` function iterates over recipes and invokes each recipe's register-phase Lua module. Each invocation already passes various arguments to the register call. Extend the call signature to also pass `cache_ctx.clone()` (an Arc<CacheContext>) into each register-phase invocation. The receiving signature in `cook-register` is updated in Task 5.3.

If the recipe register call is something like `cook_register::register_recipe(&module_name, recipe_lua, &thread_safe_cache_manager)`, change it to:

```rust
        cook_register::register_recipe(
            &module_name,
            recipe_lua,
            &thread_safe_cache_manager,
            cache_ctx.clone(),
        )?;
```

Adjust to match your existing function name and signature. Search for the entry point with `grep -n "register_recipe\|register_phase" cli/crates/cook-engine/src/run.rs`.

- [ ] **Step 5.2.5: Build the workspace**

Run: `cd /home/alex/dev/cook/cli && cargo build -q`
Expected: build fails — `register_recipe` (or whichever entry) does not yet take `Arc<CacheContext>`. That's expected; Task 5.3 fixes it.

- [ ] **Step 5.2.6: Defer commit until Task 5.3 lands**

Do not commit yet. The workspace will not build until Task 5.3 updates cook-register's signature.

---

### Task 5.3: Thread `CacheContext` into the cook-register Lua VM

**Files:**
- Modify: `cli/crates/cook-register/Cargo.toml`
- Modify: `cli/crates/cook-register/src/engine.rs`
- Modify: `cli/crates/cook-register/src/unit_api.rs`

- [ ] **Step 5.3.1: Add cook-engine to cook-register's deps**

Open `cli/crates/cook-register/Cargo.toml`. Add:

```toml
cook-engine = { path = "../cook-engine" }
```

Wait — this creates a circular dependency (cook-engine already depends on cook-register). Instead, move `CacheContext` into a crate they both depend on. The cleanest fix:

**Move `CacheContext` to `cook-cache`** (where ExecutionContext, EnvDenylist, CacheBackend already live). The CloudConfig field stays — it's a small enough type to move along too. Plan revision:

a) Move `cli/crates/cook-engine/src/cache_ctx.rs` to `cli/crates/cook-cache/src/cache_ctx.rs`. Adjust imports — `cook-cache` already has `ExecutionContext`, `EnvDenylist`, `CacheBackend` locally, so the imports become local.

b) Also move `cli/crates/cook-engine/src/cloud_config.rs` to `cli/crates/cook-cache/src/cloud_config.rs` (CloudConfig is needed inside CacheContext).

c) Add `pub mod cache_ctx;` and `pub mod cloud_config;` to `cli/crates/cook-cache/src/lib.rs`. Remove from `cli/crates/cook-engine/src/lib.rs`.

d) In `cook-engine/Cargo.toml`, the `toml` dep moves to `cook-cache/Cargo.toml`. Same for `dirs`.

e) cook-engine's `run.rs` imports become `use cook_cache::cache_ctx::CacheContext;` and `use cook_cache::cloud_config::CloudConfig;`.

f) cook-register can now `use cook_cache::cache_ctx::CacheContext;` without circular dependency (cook-register already depends on cook-contracts; add cook-cache as a new dep in cook-register/Cargo.toml).

Add to `cli/crates/cook-register/Cargo.toml`:

```toml
cook-cache = { path = "../cook-cache" }
```

(NOT cook-engine.)

Verify that cook-cache does not depend on cook-register: `grep cook-register cli/crates/cook-cache/Cargo.toml`. If empty, the dependency direction is fine.

- [ ] **Step 5.3.2: Update register-phase entry to accept `Arc<CacheContext>`**

Open `cli/crates/cook-register/src/engine.rs`. Find the function that today bootstraps the recipe register Lua VM (the one cook-engine's run.rs calls). Add an `Arc<CacheContext>` parameter:

```rust
use std::sync::Arc;
use cook_cache::cache_ctx::CacheContext;

pub fn register_recipe(
    // ... existing args ...
    cache_ctx: Arc<CacheContext>,
) -> /* existing return type */ {
    // ...
}
```

Inside the function, before calling `register_unit_api(...)`, store `cache_ctx` in the Lua VM's app data so add_unit can read it:

```rust
    lua.set_app_data(cache_ctx.clone());
```

If your VM is `mlua::Lua`, `set_app_data` takes the Arc directly and stores it. The `register_unit_api` function (in `unit_api.rs`) reads it back.

- [ ] **Step 5.3.3: cook-register's `add_unit` reads `CacheContext` and `consulted_env_keys`, populates real values**

Open `cli/crates/cook-register/src/unit_api.rs`. Inside the closure registered as `cook.add_unit`, near the start (after the `command` and `inputs` are parsed but before `cache_meta` is constructed), add:

```rust
        // Read CacheContext from Lua app data — set by register_recipe in Task 5.3.2.
        let cache_ctx = lua
            .app_data_ref::<std::sync::Arc<cook_cache::cache_ctx::CacheContext>>()
            .ok_or_else(|| LuaError::RuntimeError("CacheContext not set on Lua VM".into()))?
            .clone();

        // Read consulted_env_keys field from the add_unit table.
        // - A list of strings → look up each in current process env.
        // - The literal "*" → hash all of cook.env keys-and-values (denylisted).
        // - Absent → empty map (non-cacheable steps emit no consulted_env_keys).
        let mut consulted_env: std::collections::BTreeMap<String, String> = std::collections::BTreeMap::new();
        match tbl.get::<LuaValue>("consulted_env_keys") {
            Ok(LuaValue::Table(list)) => {
                for v in list.sequence_values::<String>().flatten() {
                    if let Ok(val) = std::env::var(&v) {
                        consulted_env.insert(v, val);
                    }
                }
            }
            Ok(LuaValue::String(s)) if s.to_str().ok().as_deref() == Some("*") => {
                // Sentinel: hash everything in process env.
                for (k, v) in std::env::vars() {
                    consulted_env.insert(k, v);
                }
            }
            _ => {}
        }
```

(`LuaValue` and `LuaError` are mlua imports — they're already used in this file. The `tbl` variable is the `LuaTable` argument of `add_unit`, already in scope.)

- [ ] **Step 5.3.4: Compute real `context_hash` and `env_contribution`**

Continuing in `unit_api.rs`'s `add_unit` closure, immediately before the `cache_meta = if cache_enabled { ... }` block, compute:

```rust
        let context_hash = cache_ctx.exec_ctx.step_context_hash(&command);
        let env_contribution = cook_cache::envkey::env_contribution(&consulted_env, &cache_ctx.denylist);
```

- [ ] **Step 5.3.5: Populate `cache_meta` with real values**

Replace the current `cache_meta` construction (Phase 1's placeholder zero/empty version) with:

```rust
        let cache_meta = if cache_enabled {
            let cookfile_path = cookfile_relative_path(&cache_ctx, lua);
            let cache_key = build_local_cache_key(
                &cookfile_path,
                &rname,
                &output_paths,
                &inputs,
                command_hash,
                context_hash,
                env_contribution,
            );
            Some(CacheMeta {
                recipe_name: rname.clone(),
                project_id: cache_ctx.project_id.clone(),
                cookfile_path,
                cache_key,
                input_paths: inputs.clone(),
                output_paths: output_paths.clone(),
                command_hash,
                context_hash,
                env_contribution,
                consulted_env,
            })
        } else {
            None
        };
```

Add the helper functions to `unit_api.rs` (above the `register_unit_api` function):

```rust
fn cookfile_relative_path(cache_ctx: &cook_cache::cache_ctx::CacheContext, _lua: &Lua) -> String {
    // The Cookfile path for the current recipe lives in the recipe's source
    // metadata, which the register_recipe entry knows. For v3, we read it from
    // a Lua VM key set by register_recipe. If absent, fall back to "Cookfile".
    // (Phase 5.3.6 adds the corresponding setter in register_recipe.)
    _lua.named_registry_value::<String>("__cook_cookfile_path")
        .unwrap_or_else(|_| "Cookfile".to_string())
}

fn build_local_cache_key(
    cookfile_path: &str,
    recipe: &str,
    output_paths: &[String],
    inputs: &[String],
    command_hash: u64,
    context_hash: u64,
    env_contribution: u64,
) -> String {
    let step_name = if let Some(first) = output_paths.first() {
        first.clone()
    } else {
        format!("{}@{:x}", inputs.first().map(|s| s.as_str()).unwrap_or(""), command_hash)
    };
    format!(
        "{}::{}::{}::{:016x}::{:016x}::{:016x}",
        cookfile_path, recipe, step_name, command_hash, context_hash, env_contribution
    )
}
```

- [ ] **Step 5.3.6: register_recipe sets `__cook_cookfile_path` registry value**

In `cook-register/src/engine.rs`'s `register_recipe`, immediately after `lua.set_app_data(cache_ctx.clone())`, add:

```rust
    let rel = cookfile_path_relative_to(&cache_ctx.project_root, source_cookfile_path);
    lua.set_named_registry_value("__cook_cookfile_path", rel)?;
```

…where `source_cookfile_path` is the absolute path of the source Cookfile being registered (search the function for the existing variable name; it's already in scope because the function reads the file).

Add the helper:

```rust
fn cookfile_path_relative_to(project_root: &std::path::Path, abs: &std::path::Path) -> String {
    abs.strip_prefix(project_root)
        .ok()
        .map(|p| p.to_string_lossy().replace(std::path::MAIN_SEPARATOR, "/"))
        .unwrap_or_else(|| abs.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| "Cookfile".to_string()))
}
```

- [ ] **Step 5.3.7: Build the workspace**

Run: `cd /home/alex/dev/cook/cli && cargo build -q`
Expected: builds clean.

- [ ] **Step 5.3.8: Run all workspace tests**

Run: `cd /home/alex/dev/cook/cli && cargo test -q`
Expected: all pass. The unit_api.rs tests construct `Lua` instances without setting CacheContext as app data — those tests will fail at the `app_data_ref` lookup. Fix each existing test by setting up a minimal CacheContext (with a `tempfile::tempdir()` LocalBackend) before invoking add_unit. Sample helper to add at the top of `unit_api.rs`'s test module:

```rust
    fn fake_cache_ctx() -> std::sync::Arc<cook_cache::cache_ctx::CacheContext> {
        let dir = tempfile::tempdir().expect("tempdir").keep(); // leak; tests are short-lived
        std::sync::Arc::new(cook_cache::cache_ctx::CacheContext {
            exec_ctx: std::sync::Arc::new(cook_cache::context::ExecutionContext::probe()),
            denylist: std::sync::Arc::new(cook_cache::envkey::EnvDenylist::baseline()),
            backend: std::sync::Arc::new(cook_cache::backend::LocalBackend::new(dir.clone())),
            cloud_config: std::sync::Arc::new(cook_cache::cloud_config::CloudConfig::default()),
            project_root: dir.clone(),
            project_id: "test-project".to_string(),
        })
    }
```

In each test, before invoking `register_unit_api`, set the app data:

```rust
    lua.set_app_data(fake_cache_ctx());
    lua.set_named_registry_value("__cook_cookfile_path", "Cookfile".to_string()).expect("set");
```

- [ ] **Step 5.3.9: Add a new test verifying real consulted_env capture**

Append to `unit_api.rs`'s test module:

```rust
    #[test]
    fn add_unit_populates_consulted_env_from_keys_list() {
        // The Lua test harness invokes cook.add_unit with a table that
        // includes consulted_env_keys = {"FOO_TEST_VAR_X"}. We set the env
        // var, then assert it lands in the captured CacheMeta.
        std::env::set_var("FOO_TEST_VAR_X", "the-value");
        let lua = mlua::Lua::new();
        lua.set_app_data(fake_cache_ctx());
        lua.set_named_registry_value("__cook_cookfile_path", "Cookfile".to_string()).expect("set");

        let capture_state = SharedCaptureState::new(/* ... */);
        register_unit_api(&lua, capture_state.clone(), "build").expect("register");

        lua.load(r#"
cook.add_unit({
    command = "echo hi",
    inputs = {},
    consulted_env_keys = {"FOO_TEST_VAR_X"},
})
"#).exec().expect("exec");

        let units = capture_state.borrow();
        let meta = units.units[0].cache_meta.as_ref().unwrap();
        assert_eq!(meta.consulted_env.get("FOO_TEST_VAR_X"), Some(&"the-value".to_string()));
        assert_ne!(meta.env_contribution, 0, "env_contribution must be non-zero when env was consulted");

        std::env::remove_var("FOO_TEST_VAR_X");
    }
```

(Adjust the `capture_state` construction to match the existing test pattern in the file.)

- [ ] **Step 5.3.10: Phase 5 commit**

```bash
git add -A cli/
git commit -m "$(cat <<'EOF'
feat(SHI-140/141/142): cross-machine cache correctness live

Engine bootstrap (run.rs) probes ExecutionContext, builds
EnvDenylist from CloudConfig (.cook/cloud.toml extensions union the
D1 baseline), constructs LocalBackend, runs health() best-effort.

CacheContext aggregates exec_ctx + denylist + backend + cloud_config
and is moved to cook-cache (avoiding circular dep with cook-register).
register_recipe accepts Arc<CacheContext> and threads it into the
Lua VM via app_data + named registry value.

cook-register's add_unit reads consulted_env_keys (list-of-names or
the "*" sentinel for lua_block payloads), looks up values from the
process env, computes context_hash via exec_ctx.step_context_hash,
computes env_contribution via envkey::env_contribution, and writes
real values into CacheMeta. Local cache key now embeds context_hash
and env_contribution so simultaneous variant builds (debug ↔ release,
gcc ↔ clang) coexist without overwrite.

Removes the dead invalidate_if_env_changed call site in run.rs (the
method itself was deleted in Phase 2).

Closes (in part): SHI-141, SHI-142.
EOF
)"
```

---

## Phase 6 — Update `executor.rs` and `dag_data.rs` for live cloud-key composition

After Phase 5, `CacheMeta` carries real values. Phase 6 wires the executor to actually compute `cloud_key` and (in a follow-up) call `Backend::batch_query` / `get` / `put`. v3 ships with the cloud_key computed for diagnostic purposes; the actual upload-on-completion call is gated behind `cloud.enabled` and is only meaningful with `CloudBackend` (SHI-24). For v3, the upload is a no-op against `LocalBackend` but the call sites are wired so SHI-24 just swaps the backend.

### Task 6.1: Compute `cloud_key` per cacheable unit

**Files:**
- Modify: `cli/crates/cook-engine/src/executor.rs`

- [ ] **Step 6.1.1: Compute cloud_key just before record_completion**

Open `cli/crates/cook-engine/src/executor.rs`. Find the post-execution block where `cm.record_completion(...)` is called. Just before that call, compute `cloud_key` and call `backend.put(...)` with the artifact bytes:

```rust
            // Compute cloud_key for this unit (spec §5.3).
            // Build sorted_input_content_hashes from the freshly-recorded inputs.
            let mut sorted_hashes: Vec<u64> = step_entry.inputs.iter().map(|fr| fr.hash).collect();
            sorted_hashes.sort();

            let recipe_namespace = format!(
                "{}/{}::{}",
                meta.project_id, meta.cookfile_path, meta.recipe_name
            );

            let key_inputs = cook_cache::backend::CloudKeyInputs {
                schema_version: cook_cache::store::CACHE_VERSION,
                recipe_namespace: &recipe_namespace,
                command_hash: meta.command_hash,
                context_hash: meta.context_hash,
                env_contribution: meta.env_contribution,
                sorted_input_content_hashes: &sorted_hashes,
            };
            let cloud_k = cook_cache::backend::cloud_key(&key_inputs);

            // Best-effort upload. LocalBackend always succeeds; CloudBackend
            // (SHI-24) may fail — log and continue per spec §6.1.
            if let Some(first_output) = meta.output_paths.first() {
                let abs_output = working_dir.join(first_output);
                if let Ok(bytes) = std::fs::read(&abs_output) {
                    let artifact_meta = cook_cache::backend::ArtifactMeta {
                        recipe_namespace: recipe_namespace.clone(),
                        command_hash: meta.command_hash,
                        context_hash: meta.context_hash,
                        env_contribution: meta.env_contribution,
                        schema_version: cook_cache::store::CACHE_VERSION,
                        size_bytes: bytes.len() as u64,
                        tags: std::collections::BTreeSet::new(),
                        consulted_env_keys: meta.consulted_env.keys().cloned().collect(),
                    };
                    if let Err(e) = cache_ctx.backend.put(&cloud_k, &bytes, &artifact_meta) {
                        tracing::warn!("cache backend put failed for {}: {}", first_output, e);
                    }
                }
            }
```

`step_entry`, `meta`, `working_dir`, and `cache_ctx` must be in scope at the call site. If `cache_ctx` is not currently threaded into the executor, plumb it as an additional parameter from `run.rs`'s execution loop.

- [ ] **Step 6.1.2: For multi-output steps, hash and upload only the first output in v3**

This is consistent with the spec's "v3 captures primary output" posture; multi-output coverage is a downstream concern. Document the limitation inline:

```rust
            // v3 limitation: multi-output steps upload only the first output
            // bytes. SHI-NNN (future) will produce a manifest-style artifact
            // covering all outputs.
```

- [ ] **Step 6.1.3: Build and run all workspace tests**

Run: `cd /home/alex/dev/cook/cli && cargo build -q && cargo test -q`
Expected: all pass.

- [ ] **Step 6.1.4: Commit**

```bash
git add cli/crates/cook-engine/src/executor.rs
git commit -m "$(cat <<'EOF'
feat(SHI-143/144): compute cloud_key and upload via Backend::put

After record_completion, the executor composes the cloud_key from
the StepEntry + CacheMeta and calls backend.put() with the artifact
bytes and an ArtifactMeta sidecar. For LocalBackend (v3 default)
this writes to ~/.cache/cook/cloud/. For CloudBackend (SHI-24) this
will be the wire-level upload.

Failures are logged via tracing::warn! but never propagate — per
spec §6.1, the build never fails because the backend failed.

v3 limitation: multi-output steps upload only the first output's
bytes; manifest-style multi-output upload is a follow-up.
EOF
)"
```

---

## Phase 7 — `cpp` module declares consulted env

The cpp module constructs commands as Lua strings (using `cook.env.CFLAGS` etc.), bypassing cook-luagen's template-substitution path. Layer 2 inference therefore doesn't fire for module-built commands. The cleanest fix is **explicit declaration**: the cpp module includes `consulted_env_keys = {"CPATH", ...}` directly in its `cook.add_unit({...})` calls. cook-register reads this list whether it came from cook-luagen (Phase 4) or a module (Phase 7) — same code path.

### Task 7.1: Add `consulted_env_keys` to cpp module's add_unit calls

**Files:**
- Modify: `examples/cpp-project/cook_modules/cpp.lua`

- [ ] **Step 7.1.1: Find the `cook.add_unit` calls in cpp.lua**

```bash
grep -n "cook.add_unit" /home/alex/dev/cook/examples/cpp-project/cook_modules/cpp.lua
```

There should be calls inside `cpp.compile` (around line 249–325) and `cpp.link` (around line 330+).

- [ ] **Step 7.1.2: Add the env declaration to `cpp.compile`'s add_unit call**

In `cpp.compile`, find the `cook.add_unit({...})` call at the end of the function. Add a `consulted_env_keys` field:

```lua
    cook.add_unit({
        -- ... existing fields (command, inputs, output, etc.) ...
        consulted_env_keys = {
            "CPATH", "C_INCLUDE_PATH", "CPLUS_INCLUDE_PATH",
            "LIBRARY_PATH", "LD_LIBRARY_PATH", "PKG_CONFIG_PATH",
            "SDKROOT",
        },
    })
```

- [ ] **Step 7.1.3: Add the env declaration to `cpp.link`'s add_unit call**

In `cpp.link`, the linker consults a slightly different set (the include paths don't matter at link time; link-paths and pkg-config matter):

```lua
    cook.add_unit({
        -- ... existing fields ...
        consulted_env_keys = {
            "LIBRARY_PATH", "LD_LIBRARY_PATH", "PKG_CONFIG_PATH",
            "SDKROOT",
        },
    })
```

- [ ] **Step 7.1.4: Sanity-test the cpp example builds**

```bash
cd /home/alex/dev/cook/examples/cpp-project && cargo run -p cook-cli -- build 2>&1 | tail -20
```

(Substitute the canonical "run cook from the workspace" invocation if different.)

Expected: build completes (no behavior change — the env declaration only affects cache key composition, not command execution).

- [ ] **Step 7.1.5: Verify env contribution is non-zero when env is set**

```bash
cd /home/alex/dev/cook/examples/cpp-project
CPATH=/tmp/include1 cargo run -p cook-cli -- build 2>&1 | tail -5
# Modify a source, rebuild with different CPATH; observe rebuild.
touch src/main.cpp
CPATH=/tmp/include2 cargo run -p cook-cli -- build 2>&1 | grep -E "rebuild|cache"
```

Expected: the second build shows a rebuild reason mentioning env (the diagnostic from `RebuildReason::EnvChanged`). If verbosity is needed to see it, add `-v`.

- [ ] **Step 7.1.6: Commit**

```bash
git add examples/cpp-project/cook_modules/cpp.lua
git commit -m "$(cat <<'EOF'
feat(SHI-142): cpp module declares consulted env keys

cpp.compile and cpp.link now declare the env vars their tools
consult internally (CPATH, C_INCLUDE_PATH, CPLUS_INCLUDE_PATH,
LIBRARY_PATH, LD_LIBRARY_PATH, PKG_CONFIG_PATH, SDKROOT).
cook-register reads consulted_env_keys identically whether it came
from cook-luagen template inference or a module's explicit
declaration.

Compile units declare the include-path class; link units declare
only the link/search-path class.
EOF
)"
```

---

## Phase 8 — Standard §8.6 amendment + Appendix B paragraph + D-changes entry

Phase 8 lands the spec-derived Standard amendments. Per CLAUDE.md and the project memory, this is the spec-first commit that authorizes the implementation; in pre-1.0 lockstep era, it ships in the same PR/branch series. By landing it last in the implementation sequence (rather than first), we avoid claiming Standard conformance for behavior that doesn't yet ship.

### Task 8.1: Amend §8.6 ¶3

**Files:**
- Modify: `standard/src/content/docs/08-execution-model.mdx`

- [ ] **Step 8.1.1: Replace the §8.6 ¶3 prose**

Open `standard/src/content/docs/08-execution-model.mdx`. Find paragraph 3 of §8.6 (around line 160). Replace:

> A conforming implementation MAY additionally consider the content of the declared input files, the modification times of the declared output files, and the content of the recipe's ingredient set. It MUST NOT consider wall-clock time, process identifiers, or any state not derivable from the sources above.

with:

> A conforming implementation MAY additionally consider the content of the declared input files, the modification times of the declared output files, the content of the recipe's ingredient set, the identity of the host machine on which the unit will execute (e.g., target triple, libc version, locale-affecting environment variables), and the resolved content of the binaries invoked by the unit's command text. It MUST NOT consider wall-clock time, process identifiers, or any state not derivable from the sources above. An implementation MAY exclude from the resolved-environment observable a set of environment variables whose values are known not to affect work-unit output (e.g., ambient session state, credentials unrelated to the build product); the specific set is implementation-defined.

- [ ] **Step 8.1.2: Verify the standard site builds**

```bash
cd /home/alex/dev/cook/standard && pnpm build 2>&1 | tail -30
```

Expected: clean build.

---

### Task 8.2: Add the rationale paragraph to Appendix B

**Files:**
- Modify: `standard/src/content/docs/appendix/B-rationale.mdx`

- [ ] **Step 8.2.1: Locate §B-225 (the existing cache observables paragraph)**

```bash
grep -n "Section 8.6 fixes the" /home/alex/dev/cook/standard/src/content/docs/appendix/B-rationale.mdx
```

It is around line 225. The new paragraph goes immediately after that paragraph (preserving the "boundary is deliberate" framing).

- [ ] **Step 8.2.2: Insert the new paragraph**

After the existing paragraph that ends with "the boundary is deliberate.", insert a blank line and the following paragraph:

> **Cross-machine cache correctness.** §8.6's enumeration of permissible cache observables now includes machine identity and tool-binary content. The motivation is the shared/team cache: an artifact produced on one machine and served to another must remain correct against differences in compiler version, libc, locale, and the resolved binary that the command text names. A Cook implementation that hashes only "command text + input files" is correct on a single machine but unsafe across machines; the additional observables let an implementation be cross-machine-safe without taking on those observables when running locally. The denylist clause acknowledges that not every difference in the resolved environment affects build output (ambient session state, secrets unrelated to the artifact), and lets implementations filter without violating the minimum-observable enumeration of ¶2.

- [ ] **Step 8.2.3: Verify build**

```bash
cd /home/alex/dev/cook/standard && pnpm build 2>&1 | tail -30
```

Expected: clean build.

---

### Task 8.3: Add the CS-NNNN entry to App. D

**Files:**
- Modify: `standard/src/content/docs/appendix/D-changes.mdx`

- [ ] **Step 8.3.1: Determine the next CS number**

The most recent CS number visible in commit messages and `D-changes.mdx` is **CS-0024**. The next is **CS-0025**.

- [ ] **Step 8.3.2: Add the entry**

In `standard/src/content/docs/appendix/D-changes.mdx`, after the existing CS-0024 entry block, add:

```markdown
## D.NN. CS-0025 — Cache observables: machine identity, tool-binary content, env denylist. [#changes.cs-0025]

**Date:** 2026-05-01

**Version:** v0.5 (or v0.6 — confirm before merge based on the active cut)

**Sections affected:** §{exec.cache} ¶3 (additive); App. B (rationale paragraph after §B-225, "Cross-machine cache correctness").

**Summary:** Extends the §{exec.cache} list of permissible cache observables with (i) host machine identity (target triple, libc version, locale-affecting environment), (ii) the resolved content of the binaries invoked by the unit's command text, and (iii) an implementation-defined denylist of environment variables excluded from the resolved-environment observable. All three are permissive (`MAY`); a single-machine implementation that elides them remains conforming.

**Motivation.** The shared/team cache (Cook Cloud, Linear epic SHI-140) requires that an artifact produced on one machine be correct when served to another. Today's `command_hash = hash_str(command_text)` plus input-content hashing is insufficient: Alice's `gcc-13` `parser.o` would be served to Bob's `gcc-11` build under the same command text. The amendment authorizes implementations to capture the cross-machine variation without changing language semantics. The denylist clarifies that "the resolved environment" was never intended to include ambient session state or unrelated credentials.
```

(Update the `D.NN` numbering to match the next available — the existing entries proceed sequentially; check the previous entry's number and increment.)

- [ ] **Step 8.3.3: Add CS-0025 to the Versions index**

At the top of `D-changes.mdx`, find the `## Versions` block. The line for whichever cut this CS lands in (likely the next release) should include `CS-0025`. If a cut is not yet declared, add a new entry; otherwise append `, CS-0025` to the relevant line.

- [ ] **Step 8.3.4: Verify the build**

```bash
cd /home/alex/dev/cook/standard && pnpm build 2>&1 | tail -30
```

Expected: clean build, no broken anchors. Verify the new CS-0025 anchor renders by visiting `/standard/appendix/d-changes` in the dev server (`pnpm dev`) if available.

- [ ] **Step 8.3.5: Commit Phase 8**

```bash
git add standard/src/content/docs/08-execution-model.mdx standard/src/content/docs/appendix/B-rationale.mdx standard/src/content/docs/appendix/D-changes.mdx
git commit -m "$(cat <<'EOF'
spec(CS-0025): cache observables — machine identity + tool binary + env denylist

§8.6 ¶3 amended additively to permit hashing host machine identity
(target triple, libc, locale envs), the resolved content of the
binaries the command invokes, and an implementation-defined
denylist filter on the resolved environment. App. B gains a
"Cross-machine cache correctness" rationale paragraph after §B-225;
App. D records the change as CS-0025.

All additions are permissive (MAY) — single-machine
implementations that elide them remain conforming. The amendment
authorizes the cache cloud-readiness pass landed under SHI-140
(local cache key composition unchanged for single-host use).
EOF
)"
```

---

## Phase 9 — Cross-cutting integration tests

Three tests in `cli/crates/cook-cache/tests/` close the cross-cutting acceptance criteria from spec §11. Each is its own commit.

### Task 9.1: First-build depfile thin-input fixture (AC-Integ.1)

**Files:**
- Create: `cli/crates/cook-cache/tests/integration_first_build_depfile.rs`

- [ ] **Step 9.1.1: Write the test**

Create `cli/crates/cook-cache/tests/integration_first_build_depfile.rs`:

```rust
//! AC-Integ.1: Two-machine first-build depfile fixture.
//!
//! Machine A (no .d file → inputs = [source] only) records a StepEntry
//! and uploads. Machine B (also no .d file, fresh checkout) pulls.
//! The cache hit is correct because input *content* matches; B's
//! subsequent builds generate a depfile and pick up header changes.

use std::collections::BTreeMap;

use cook_cache::backend::{cloud_key, ArtifactMeta, CacheBackend, CloudKeyInputs, LocalBackend};
use cook_cache::context::ExecutionContext;
use cook_cache::envkey::EnvDenylist;
use cook_cache::store::{FileRecord, RecipeCache, StepEntry, CACHE_VERSION};

fn make_step_with_thin_inputs(source_path: &str, source_hash: u64) -> StepEntry {
    StepEntry {
        inputs: vec![FileRecord {
            path: source_path.to_string(),
            mtime: 1700000000,
            hash: source_hash,
        }],
        outputs: vec![FileRecord {
            path: "build/main.o".to_string(),
            mtime: 1700000100,
            hash: 0xabcd_efab_cdef_abcd,
        }],
        command_hash: 0x1111,
        context_hash: 0x2222,
        env_contribution: 0x3333,
    }
}

#[test]
fn machine_a_uploads_thin_entry_machine_b_pulls_correctly() {
    // Shared "cloud" backend used by both machines.
    let shared_dir = tempfile::tempdir().expect("tempdir");
    let backend = LocalBackend::new(shared_dir.path().to_path_buf());

    // Machine A: build with no depfile, inputs=[source] only.
    let entry_a = make_step_with_thin_inputs("src/main.c", 0xc01dcafe);

    // Compose the cloud key. Machine A's namespace and key inputs.
    let mut sorted_hashes: Vec<u64> = entry_a.inputs.iter().map(|fr| fr.hash).collect();
    sorted_hashes.sort();
    let inputs_for_key = CloudKeyInputs {
        schema_version: CACHE_VERSION,
        recipe_namespace: "myproj/Cookfile::build",
        command_hash: entry_a.command_hash,
        context_hash: entry_a.context_hash,
        env_contribution: entry_a.env_contribution,
        sorted_input_content_hashes: &sorted_hashes,
    };
    let key = cloud_key(&inputs_for_key);

    // Machine A uploads the artifact bytes (object file contents).
    let artifact_bytes: Vec<u8> = (0..256u8).cycle().take(4096).collect();
    let meta = ArtifactMeta {
        recipe_namespace: "myproj/Cookfile::build".into(),
        command_hash: entry_a.command_hash,
        context_hash: entry_a.context_hash,
        env_contribution: entry_a.env_contribution,
        schema_version: CACHE_VERSION,
        size_bytes: artifact_bytes.len() as u64,
        tags: Default::default(),
        consulted_env_keys: Default::default(),
    };
    backend.put(&key, &artifact_bytes, &meta).expect("put");

    // Machine B: fresh checkout, no .d file. Same source content.
    // Its key composition produces the same key.
    let entry_b = make_step_with_thin_inputs("src/main.c", 0xc01dcafe);
    let mut sorted_b: Vec<u64> = entry_b.inputs.iter().map(|fr| fr.hash).collect();
    sorted_b.sort();
    let inputs_for_key_b = CloudKeyInputs {
        schema_version: CACHE_VERSION,
        recipe_namespace: "myproj/Cookfile::build",
        command_hash: entry_b.command_hash,
        context_hash: entry_b.context_hash,
        env_contribution: entry_b.env_contribution,
        sorted_input_content_hashes: &sorted_b,
    };
    let key_b = cloud_key(&inputs_for_key_b);
    assert_eq!(key, key_b, "same content → same cloud_key across machines");

    // Machine B pulls.
    let bytes_b = backend.get(&key_b).expect("get").expect("hit");
    assert_eq!(bytes_b, artifact_bytes, "B receives A's bytes");
}

#[test]
fn header_change_after_pull_invalidates_correctly() {
    // After Machine B pulls A's thin-input entry and runs its first build,
    // B's *next* build SHOULD generate a depfile and pick up header changes.
    // This test verifies that a build with a fattened input set (source +
    // header) produces a different cloud_key from the thin-input one — i.e.,
    // there is no false hit on the second build.

    let entry_thin = make_step_with_thin_inputs("src/main.c", 0xc01dcafe);

    let entry_with_header = StepEntry {
        inputs: vec![
            FileRecord {
                path: "src/main.c".to_string(),
                mtime: 1700000000,
                hash: 0xc01dcafe,
            },
            FileRecord {
                path: "include/widget.h".to_string(),
                mtime: 1700000050,
                hash: 0xdeadbeef,
            },
        ],
        outputs: entry_thin.outputs.clone(),
        command_hash: entry_thin.command_hash,
        context_hash: entry_thin.context_hash,
        env_contribution: entry_thin.env_contribution,
    };

    let mut h_thin: Vec<u64> = entry_thin.inputs.iter().map(|fr| fr.hash).collect();
    h_thin.sort();
    let mut h_fat: Vec<u64> = entry_with_header.inputs.iter().map(|fr| fr.hash).collect();
    h_fat.sort();

    let key_thin = cloud_key(&CloudKeyInputs {
        schema_version: CACHE_VERSION,
        recipe_namespace: "myproj/Cookfile::build",
        command_hash: entry_thin.command_hash,
        context_hash: entry_thin.context_hash,
        env_contribution: entry_thin.env_contribution,
        sorted_input_content_hashes: &h_thin,
    });
    let key_fat = cloud_key(&CloudKeyInputs {
        schema_version: CACHE_VERSION,
        recipe_namespace: "myproj/Cookfile::build",
        command_hash: entry_with_header.command_hash,
        context_hash: entry_with_header.context_hash,
        env_contribution: entry_with_header.env_contribution,
        sorted_input_content_hashes: &h_fat,
    });

    assert_ne!(key_thin, key_fat, "fattened input set → different cloud_key");
}
```

- [ ] **Step 9.1.2: Run the test**

```bash
cd /home/alex/dev/cook/cli && cargo test -p cook-cache --test integration_first_build_depfile
```

Expected: both tests pass.

- [ ] **Step 9.1.3: Commit**

```bash
git add cli/crates/cook-cache/tests/integration_first_build_depfile.rs
git commit -m "$(cat <<'EOF'
test(SHI-140 §6): two-machine thin-input depfile fixture

AC-Integ.1: Machine A's first build (no .d file) produces a thin
StepEntry (inputs = [source] only). Upload-via-LocalBackend. Machine
B with empty cache pulls. Same content → same cloud_key → correct
hit. Companion test verifies the *next* build (with a depfile-driven
fat input set) produces a distinct cloud_key, so header changes
invalidate correctly.

Closes the SHI-140 §6 thin-input concern.
EOF
)"
```

---

### Task 9.2: Cross-recipe collision test (AC-Integ.2)

**Files:**
- Create: `cli/crates/cook-cache/tests/integration_cross_recipe_collision.rs`

- [ ] **Step 9.2.1: Write the test**

```rust
//! AC-Integ.2: Two recipes producing `build/main.o` from different
//! sources/commands must produce different cloud keys.

use cook_cache::backend::{cloud_key, CloudKeyInputs};
use cook_cache::store::CACHE_VERSION;

#[test]
fn two_recipes_same_output_path_different_keys() {
    let inputs = [0u64, 1, 2];
    let key_a = cloud_key(&CloudKeyInputs {
        schema_version: CACHE_VERSION,
        recipe_namespace: "myproj/Cookfile::build",
        command_hash: 0xAA,
        context_hash: 0xBB,
        env_contribution: 0xCC,
        sorted_input_content_hashes: &inputs,
    });
    let key_b = cloud_key(&CloudKeyInputs {
        schema_version: CACHE_VERSION,
        recipe_namespace: "myproj/Cookfile::test",  // different recipe
        command_hash: 0xAA,
        context_hash: 0xBB,
        env_contribution: 0xCC,
        sorted_input_content_hashes: &inputs,
    });
    assert_ne!(key_a, key_b, "different recipe → different cloud_key");
}

#[test]
fn cross_project_same_recipe_name_different_keys() {
    let inputs = [0u64];
    let key_a = cloud_key(&CloudKeyInputs {
        schema_version: CACHE_VERSION,
        recipe_namespace: "proj-a/Cookfile::build",
        command_hash: 0xAA,
        context_hash: 0xBB,
        env_contribution: 0xCC,
        sorted_input_content_hashes: &inputs,
    });
    let key_b = cloud_key(&CloudKeyInputs {
        schema_version: CACHE_VERSION,
        recipe_namespace: "proj-b/Cookfile::build",
        command_hash: 0xAA,
        context_hash: 0xBB,
        env_contribution: 0xCC,
        sorted_input_content_hashes: &inputs,
    });
    assert_ne!(key_a, key_b, "different project → different cloud_key");
}

#[test]
fn cross_cookfile_same_recipe_name_different_keys() {
    let inputs = [0u64];
    let key_a = cloud_key(&CloudKeyInputs {
        schema_version: CACHE_VERSION,
        recipe_namespace: "proj/Cookfile::build",
        command_hash: 0xAA,
        context_hash: 0xBB,
        env_contribution: 0xCC,
        sorted_input_content_hashes: &inputs,
    });
    let key_b = cloud_key(&CloudKeyInputs {
        schema_version: CACHE_VERSION,
        recipe_namespace: "proj/services/api/Cookfile::build",
        command_hash: 0xAA,
        context_hash: 0xBB,
        env_contribution: 0xCC,
        sorted_input_content_hashes: &inputs,
    });
    assert_ne!(key_a, key_b, "different sub-Cookfile → different cloud_key");
}
```

- [ ] **Step 9.2.2: Run + commit**

```bash
cd /home/alex/dev/cook/cli && cargo test -p cook-cache --test integration_cross_recipe_collision
git add cli/crates/cook-cache/tests/integration_cross_recipe_collision.rs
git commit -m "$(cat <<'EOF'
test(SHI-144): cross-recipe / cross-project / cross-cookfile collision

AC-Integ.2: Three pairs that share command/context/env/inputs but
differ in recipe namespace dimensions produce distinct cloud_keys.
Confirms a shared bucket is collision-safe across recipes within a
project, projects within a tenant, and sub-Cookfiles within a
project.
EOF
)"
```

---

### Task 9.3: Config toggle test (AC-Integ.3)

**Files:**
- Create: `cli/crates/cook-cache/tests/integration_config_toggle.rs`

- [ ] **Step 9.3.1: Write the test**

```rust
//! AC-Integ.3: Toggling between two configs (e.g. CXXFLAGS=-O0 ↔ -O3)
//! preserves both cache entries — no overwrite, no false hits, and
//! toggling back re-hits the prior entry.

use std::collections::BTreeMap;

use cook_cache::backend::{cloud_key, ArtifactMeta, CacheBackend, CloudKeyInputs, LocalBackend};
use cook_cache::envkey::{env_contribution, EnvDenylist};
use cook_cache::store::CACHE_VERSION;

fn key_for(env_contrib: u64) -> [u8; 32] {
    cloud_key(&CloudKeyInputs {
        schema_version: CACHE_VERSION,
        recipe_namespace: "proj/Cookfile::build",
        command_hash: 0x1111,
        context_hash: 0x2222,
        env_contribution: env_contrib,
        sorted_input_content_hashes: &[0xaa, 0xbb],
    })
}

#[test]
fn toggling_cxxflags_produces_distinct_keys() {
    let denylist = EnvDenylist::baseline();

    let mut env_o2 = BTreeMap::new();
    env_o2.insert("CXXFLAGS".to_string(), "-O2".to_string());
    let env_contrib_o2 = env_contribution(&env_o2, &denylist);

    let mut env_o3 = BTreeMap::new();
    env_o3.insert("CXXFLAGS".to_string(), "-O3".to_string());
    let env_contrib_o3 = env_contribution(&env_o3, &denylist);

    assert_ne!(env_contrib_o2, env_contrib_o3);

    let key_o2 = key_for(env_contrib_o2);
    let key_o3 = key_for(env_contrib_o3);
    assert_ne!(key_o2, key_o3);
}

#[test]
fn toggling_back_rehits_prior_entry() {
    let dir = tempfile::tempdir().expect("tempdir");
    let backend = LocalBackend::new(dir.path().to_path_buf());
    let denylist = EnvDenylist::baseline();

    let mut env_o2 = BTreeMap::new();
    env_o2.insert("CXXFLAGS".to_string(), "-O2".to_string());
    let env_contrib_o2 = env_contribution(&env_o2, &denylist);
    let key_o2 = key_for(env_contrib_o2);

    let mut env_o3 = BTreeMap::new();
    env_o3.insert("CXXFLAGS".to_string(), "-O3".to_string());
    let env_contrib_o3 = env_contribution(&env_o3, &denylist);
    let key_o3 = key_for(env_contrib_o3);

    let meta_for = |env_c: u64| ArtifactMeta {
        recipe_namespace: "proj/Cookfile::build".into(),
        command_hash: 0x1111,
        context_hash: 0x2222,
        env_contribution: env_c,
        schema_version: CACHE_VERSION,
        size_bytes: 5,
        tags: Default::default(),
        consulted_env_keys: ["CXXFLAGS".to_string()].into_iter().collect(),
    };

    backend.put(&key_o2, b"O2-bytes", &meta_for(env_contrib_o2)).expect("put o2");
    backend.put(&key_o3, b"O3-bytes", &meta_for(env_contrib_o3)).expect("put o3");

    // Toggle back: O2 still hits with O2 bytes (no overwrite).
    let bytes_o2 = backend.get(&key_o2).expect("get").expect("hit");
    assert_eq!(bytes_o2, b"O2-bytes");

    let bytes_o3 = backend.get(&key_o3).expect("get").expect("hit");
    assert_eq!(bytes_o3, b"O3-bytes");
}

#[test]
fn denylisted_env_does_not_change_key() {
    let denylist = EnvDenylist::baseline();

    // HOME is denylisted; toggling its value must not change env_contribution.
    let mut env_a = BTreeMap::new();
    env_a.insert("HOME".to_string(), "/home/alice".to_string());
    let mut env_b = BTreeMap::new();
    env_b.insert("HOME".to_string(), "/home/bob".to_string());

    let h_a = env_contribution(&env_a, &denylist);
    let h_b = env_contribution(&env_b, &denylist);
    assert_eq!(h_a, h_b);
}
```

- [ ] **Step 9.3.2: Run + commit**

```bash
cd /home/alex/dev/cook/cli && cargo test -p cook-cache --test integration_config_toggle
git add cli/crates/cook-cache/tests/integration_config_toggle.rs
git commit -m "$(cat <<'EOF'
test(SHI-142): config toggle preserves both cache entries

AC-Integ.3: CXXFLAGS=-O2 ↔ -O3 produces distinct env_contribution
hashes, distinct cloud_keys, and distinct LocalBackend entries.
Toggling back re-hits the prior entry's bytes (no overwrite).
Companion test verifies denylisted envs (HOME) do NOT contribute.
EOF
)"
```

---

## Self-review checklist

Before treating the plan as ready for execution, run through these:

**Spec coverage.** Each spec acceptance criterion (AC-141.1 through AC-Integ.3) maps to a test in this plan:

| AC | Task | Step |
|---|---|---|
| AC-141.1 (CC=gcc ↔ clang) | manual integration via cpp example, Phase 7.1.5 | + AC-Integ.3 covers the env-key shape |
| AC-141.2/3/4 (context_hash determinism + tool/triple sensitivity) | Task 0.2 | Steps 0.2.4, 0.2.5 |
| AC-141.5 (Nix-shell determinism) | covered by Task 0.2's content-addressed binary hashing — manual verification |
| AC-142.1/2/4 (CXXFLAGS toggle) | Task 9.3 | Steps 9.3.1/2 |
| AC-142.3 (invalidate_if_env_changed gone) | Task 2.3 | Step 2.3.4 |
| AC-143.1 (local perf unchanged) | implicit — xxh3 still used; no benchmark in plan |
| AC-143.2/3/4 (cloud_key determinism + sensitivity) | Task 0.6 | Steps 0.6.1/2 |
| AC-144.1/2/3 (namespace-keyed) | Task 9.2 | Step 9.2.1 |
| AC-145.1/2/3 (dead code removal) | Task 2.2 (delete), 2.3 (delete invalidate_recipe), 2.4 (re-export) |
| AC-146.1/2/3 (record_completion hardening) | Task 2.3 | Steps 2.3.6 |
| AC-B2.1 (LocalBackend conformance) | Task 0.5 | Step 0.5.1 (8 tests) |
| AC-B2.2 (errors don't fail build) | Task 6.1 | Step 6.1.1 (tracing::warn, no propagation) |
| AC-B2.3 (put idempotent) | Task 0.5 | Step 0.5.1 (`local_backend_put_idempotent`) |
| AC-Env.1 (D1 baseline) | Task 0.3 | Step 0.3.1 (8 tests covering exact + glob) |
| AC-Env.2 (D2 union) | Task 0.3 | Step 0.3.1 (`extend_with_*` tests) |
| AC-Env.3 (denylisted token still substituted, not hashed) | Task 9.3 | Step 9.3.1 (`denylisted_env_does_not_change_key`) |
| AC-Integ.1 (depfile thin-input) | Task 9.1 |
| AC-Integ.2 (cross-recipe collision) | Task 9.2 |
| AC-Integ.3 (config toggle) | Task 9.3 |

**Placeholder scan.** Search the plan for `TBD`, `TODO`, `implement later`, `fill in details`, `add appropriate error handling`, `similar to Task N`. If found, replace with concrete code.

**Type / signature consistency.** Cross-checks:
- `CacheMeta` field set in Phase 1.1 matches Phase 5.3.5 construction site and the test fixture in Phase 2.3.6.
- `record_completion` signature `(&self, recipe_name, cache_key, meta: &CacheMeta, working_dir) -> Result<(), RecordError>` is the same in Phase 2.3.2 (definition) and Phase 2.5.1 (executor call site).
- `needs_rebuild_*` signature with `(context_hash, env_contribution)` extra params is the same in Phase 2.2.2 (definition) and Phase 2.5.1 (executor call site) and Phase 2.5.2 (dag_data call site).
- `EnvDenylist` API: `baseline()`, `extend_with(&[String])`, `is_ignored(&str) -> bool` consistent across Phase 0.3, Phase 5.2.3 (engine bootstrap), Phase 9.3.1 (test).
- `cloud_key` takes `&CloudKeyInputs` consistent across Phase 0.6 (definition), Phase 6.1.1 (executor), Phase 9.1.1 / 9.2.1 / 9.3.1 (tests).
- `CacheContext` lives in `cook_cache::cache_ctx` (per Phase 5.3.1's revision; was originally in cook-engine in Phase 5.1) — Phase 5.1's commit message is correct that it migrates.

**Standard impact ordering.** Phase 8 lands the §8.6 amendment AFTER the implementation that exercises it. Per CLAUDE.md and the project memory, the spec-first hook permits this ordering as long as the Standard delta lands in the same PR/branch series.

---

## Rollout summary

After all tasks complete, the implementation comprises **18 commits** across **9 phases**:

| Phase | Commits | Net effect |
|---|---|---|
| 0 — Foundation | 4 | Adds `ExecutionContext`, `EnvDenylist`, `CacheBackend` trait, `LocalBackend`, `cloud_key()` (additive). |
| 1 — CacheMeta | 1 | Extends `CacheMeta` with five new fields (placeholder values). |
| 2 — Schema v3 | 1 | Schema v2→v3 cascade + record_completion hardening + dead-code removal. |
| 3 — CloudConfig | 1 | `.cook/cloud.toml` parser. |
| 4 — luagen consulted-env | 2 | Tracks consulted env during template expansion; emits on add_unit table. |
| 5 — Engine + register | 4 | CacheContext type, engine bootstrap, register-phase consumes consulted_env_keys, computes real values. |
| 6 — Executor cloud_key | 1 | Computes cloud_key per unit; calls Backend::put best-effort. |
| 7 — cpp module | 1 | cpp.compile/cpp.link declare consulted_env_keys. |
| 8 — Standard | 1 | §8.6 amendment + Appendix B + CS-0025. |
| 9 — Integration tests | 3 | First-build depfile, cross-recipe collision, config toggle. |

**Branch shape.** One feature branch off `main` (`shi-140/cache-cloud-readiness`), commits land in phase order. Each commit passes `cargo build && cargo test` on its own. The Standard build (`pnpm build` in `standard/`) passes from Phase 8 onward.

**Schema migration cost.** First v3 build invalidates all v2 cache files (per Phase 2.1.1 — `version != CACHE_VERSION` returns `None`). One-time full rebuild on upgrade; document in release notes.

---

## Plan execution

Plan complete and saved to `standard/plans/2026-05-01-cache-cloud-readiness-plan.md`. Two execution options:

**1. Subagent-Driven (recommended)** — Dispatch a fresh subagent per task, review between tasks, fast iteration. Each task's commit is reviewed before the next is dispatched.

**2. Inline Execution** — Execute tasks in this session using executing-plans, batch execution with checkpoints for review.

Which approach?
