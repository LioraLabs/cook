# Design: Cache cloud-readiness (per-step keying, execution context, backend seam)

**Date:** 2026-05-01
**Status:** Design — pending implementation plan
**Standard change ID:** CS-NNNN (assigned at PR time)
**Linear epic:** [SHI-140](https://linear.app/shiny-guru/issue/SHI-140) — *Cache correctness pass for cloud-readiness*
**Sub-issues folded into this design:** SHI-141, SHI-142, SHI-143, SHI-144, SHI-145, SHI-146
**Scope:** `cli/crates/cook-cache`, `cli/crates/cook-contracts`, `cli/crates/cook-register`, `cli/crates/cook-engine`, `cli/crates/cook-cli`. One additive amendment to Standard §8.6 (and a companion paragraph in Appendix B); no Cookfile grammar change, no Cook Lua API change.

## 1. Motivation

Cook's cache today is correct on a single machine because every input that the cache key elides — the host's compiler, libc, locale, the binary that `gcc` resolves to — is held constant by virtue of running on one developer's box. The moment a build artifact crosses to another machine, those elisions become poisoning vectors:

1. **Toolchain identity is not in the key.** `command_hash = hash_str(command)` (`cli/crates/cook-register/src/unit_api.rs:88`) hashes the literal command string; the cpp module emits `"gcc"` with no version, no path, no target triple. Alice's gcc-13 `parser.o` would be served to Bob's gcc-11 build. Cross-machine: catastrophic.
2. **Environment is recipe-level wipe, not per-step key.** `hash_env` is consulted only for recipe-wide invalidation (`cli/crates/cook-cache/src/manager.rs:145-158`). Two simultaneous builds with different `CXXFLAGS` need *different keys*, not a wipe of the recipe; toggling `CXXFLAGS` back to a prior value should re-hit, not re-execute.
3. **`command_hash` is non-cryptographic.** xxh3_64 is fine for local hit-checking. In a content-addressed cloud cache where artifacts are *served* by hash, an adversary with control over command shapes could induce collisions.
4. **`cache_key` defaults to first output path.** `cli/crates/cook-register/src/unit_api.rs:91` sets `cache_key = output_paths.first()` (e.g. `"build/main.o"`). Globally collision-prone in a shared bucket — every project produces a `build/main.o`.
5. **`secondary_inputs_hash` is dead code.** `hash_secondary_inputs` (`cli/crates/cook-cache/src/check.rs:46`) and `invalidate_recipe` (`cli/crates/cook-cache/src/manager.rs:37`) exist but are never called from production. The cpp module's depfile-driven per-unit `inputs[]` is the live mechanism; the unused machinery is a maintenance liability.
6. **`record_completion` swallows missing-file errors.** `unwrap_or(0)` for `stat_mtime`/`hash_file` (`cli/crates/cook-cache/src/manager.rs:175`) persists `(mtime=0, hash=0)` if a file vanished between command-run and record. Self-correcting locally; could push poisoned records to a shared cache.

The framing this design adopts is sharper than "cloud-ready": the goal is **a cache Bob can rely on without auditing what Alice produced.** That bar admits no "mostly correct cross-machine" caveats.

This design closes those gaps and locks the seam (a `CacheBackend` trait + `ArtifactMeta` value type) that Cook Cloud's R2/D1 backend (SHI-24, SHI-25) implements against. It does **not** implement the cloud backend itself; it makes the cloud backend a routine implementation exercise rather than a cache redesign.

## 2. Non-goals

- **HTTP transport, R2 schema, D1 schema, auth model.** SHI-9, SHI-10, SHI-18, SHI-19, SHI-20, SHI-24, SHI-25. v3 produces the trait surface those tickets implement against.
- **Eviction policy.** v3 ships `ArtifactMeta` with `tags` and `size_bytes` so backends *can* implement policy; v3 doesn't define the policy. Eviction is a Cook Cloud product decision.
- **CLI flags `--cloud` / `--no-cloud`.** SHI-26. v3 silently uses the in-process `LocalBackend`; the trait is in place for SHI-24 to wire `CloudBackend` behind a flag.
- **Per-step Lua API for env declaration.** A future `cook.add_unit({ env_keys = [...] })` form is the right shape for steps with diverging env needs within one recipe; v4, requires §6 amendment.
- **Wrapper-script transparency** (ccache / distcc / sccache). The binary-content hash captures the wrapper, not the wrapped compiler. Documented limitation; structured `tool = "ccache"` probes are v4.
- **Transitive binary capture.** Hashing `g++` does not capture `cc1plus`, `as`, `ld`, or libstdc++ headers. Practical: distros ship them as a unit; `libc_version` + entry-binary hash catches the typical cases. Documented limitation.
- **Pipeline / multi-binary / shell-prefix commands.** Only the first argv token is binary-hashed in v3. Pipelines (`tool1 | tool2`) capture only `tool1`'s identity. Commands that prefix the real tool with a shell builtin or wrapper (`cd subdir && gcc ...`, `env VAR=x gcc ...`) capture the prefix's identity (or the empty-binary fallback if the prefix is a shell builtin not on `$PATH`), not the real compiler. Build-system modules SHOULD emit clean tool-first commands; users with prefix patterns SHOULD restructure or accept the documented limitation.
- **Lua-chunk fine-grained env tracking.** v3 hashes the whole denylist-filtered `cook.env` map for `lua_chunk` payloads; v4 narrows to actually-accessed keys via `__index` interception.
- **Cross-tenant artifact deduplication.** R2 path is `{team_id}/{cloud_key}`; identical artifacts across teams are stored twice. Dedup is a Cook Cloud product decision.
- **Build provenance / SLSA / signed artifacts.** Out of scope; if it becomes a requirement, it's an `ArtifactMeta` extension.
- **Local cache hash function change.** Local cache continues to use xxh3_64 throughout. SHA-256 is computed only at the cloud-key boundary.

## 3. Architecture

### 3.1. Module layout

```
cli/crates/
├── cook-contracts/        existing; CacheMeta extended; ArtifactMeta added
├── cook-cache/
│   ├── lib.rs             public re-exports; hash_str, resolve_glob (unchanged)
│   ├── store.rs           StepEntry/RecipeCache schema v3
│   ├── check.rs           needs_rebuild_* extended for context_hash + env_contribution
│   ├── manager.rs         ThreadSafeCacheManager; record_completion hardened
│   ├── context.rs         NEW — ExecutionContext, machine probe, tool-binary hashing
│   ├── envkey.rs          NEW — D1 baseline denylist, env contribution computation
│   └── backend.rs         NEW — trait CacheBackend, ArtifactMeta, LocalBackend
└── cook-register/
    └── unit_api.rs        CacheMeta construction records consulted env keys
```

Touched but not rewritten: `cook-engine/src/run.rs` (drop `invalidate_if_env_changed`, thread `ExecutionContext` build-once), `cook-cli/src/dag_data.rs` and `cook-engine/src/executor.rs` (lookup keys gain context contribution).

### 3.2. Architectural invariants the design locks

1. **Local-correct ⊕ cloud-correct.** A cache entry that is correct locally is also correct to upload. `record_completion` is the sole producer of `StepEntry`; if it cannot produce a complete one, no `StepEntry` exists. (Section 7.)
2. **Schema-version partitions.** A v3 client never sees v2 cloud entries because schema version is the first field of the cloud-key SHA-256 input. Old entries become orphaned and the eviction policy reaps them on its own schedule.
3. **Tenant partitioning at the storage layer.** `team_id` is **not** in the recipe namespace and **not** in the `cloud_key`; it is the R2 path prefix only (`{team_id}/{cloud_key}`). Two teams building bit-identical artifacts produce identical `cloud_key`s — deduplicable at the R2 layer if cross-tenant dedup is ever desired — but reads and writes are scoped to a tenant's path prefix by the backend's auth layer, so tenants cannot reach each other's bytes.
4. **Eviction is monotonic.** A key in the cache is *valid forever in Cook's model.* Eviction only deletes unused entries; it never invalidates correct ones. The cache key is what guarantees correctness; eviction is purely a cost optimization.
5. **Backend errors never fail the build.** Transient errors degrade to "miss"; auth/quota errors disable the backend for the build with one log line. Cook Cloud is a *cache*, not a *dependency*.

### 3.3. Build-time flow

```
1. cook build starts
2. ExecutionContext::probe()              once per build
3. EnvDenylist::load(.cook/cloud.toml)    once per build
4. Backend::health()                      once per build; on fail, disable for build
5. Register phase produces CacheMeta for every cacheable unit
6. Compute cloud_keys for all units
7. Backend::batch_query(all_keys)         single round trip for cloud backend
8. For each unit:
     a. local hit (RecipeCache + on-disk outputs match)?      skip
     b. local entry matches but on-disk drift OR missing?     try Backend::get for each output;
                                                              if all hit, write to disk; skip
     c. else, cloud hit?                                       try Backend::get for each output;
                                                              if all hit, write to disk; skip
     d. else execute, then:
          i.   record_completion()                             local index update
          ii.  for each output, Backend::put()                 upload, fire-and-forget (errors logged)
9. ThreadSafeCacheManager::flush_all()    write recipe .bin index files
```

Steps 7, 8c, and 8d-ii are no-ops when `LocalBackend` is the only backend; they become real round-trips for `CloudBackend`. Step 8b is the local restore-on-hit path added by the [2026-05-02 addendum](./2026-05-02-cache-restore-and-dep-inputs-design.md). Step 8d-ii loops over all outputs (was: first output only) — see addendum §5.1 for the per-output `artifact_key` derivation.

## 4. Data structures

### 4.1. On-disk schema (v3)

`cli/crates/cook-cache/src/store.rs`:

```rust
pub const CACHE_VERSION: u32 = 3;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RecipeCache {
    pub version: u32,
    pub globs: BTreeMap<String, BTreeSet<String>>,
    pub steps: BTreeMap<String, StepEntry>,
    // REMOVED: secondary_inputs_hash (SHI-145)
    // REMOVED: env_hash (SHI-142 — folded into per-step keys)
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct StepEntry {
    pub inputs: Vec<FileRecord>,
    pub outputs: Vec<FileRecord>,
    pub command_hash: u64,        // unchanged: xxh3_64(rendered command)
    pub context_hash: u64,        // NEW: xxh3_64 over machine + tool identity
    pub env_contribution: u64,    // NEW: xxh3_64 over consulted (key,value) pairs after denylist
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FileRecord {
    pub path: String,
    pub mtime: u64,
    pub hash: u64,
}
```

`RecipeCache::load` returns `None` on `version != CACHE_VERSION`. Existing v2 caches on disk fail the version check and the recipe rebuilds from scratch on first v3 run. **No migration code path; the version mismatch is the migration.**

### 4.2. `ExecutionContext`

`cli/crates/cook-cache/src/context.rs`:

```rust
pub struct ExecutionContext {
    /// Build-wide: probed once per `cook build` invocation.
    pub machine: MachineIdentity,
    /// Per-step: lazily probed and cached, keyed by canonical realpath.
    tool_cache: Mutex<HashMap<PathBuf, ToolHash>>,
}

pub struct MachineIdentity {
    pub target_triple: String,         // e.g. "x86_64-unknown-linux-gnu"
    pub libc_version: Option<String>,  // e.g. "glibc 2.39"; None on musl/macOS native
    pub locale_baseline: BTreeMap<String, String>, // LANG, LC_*, TZ, SOURCE_DATE_EPOCH
}

pub struct ToolHash {
    pub content_sha256: [u8; 32],
}
```

**Probe mechanics:**

- `target_triple`: from compile-time `cfg!(target_*)` macros; no runtime cost.
- `libc_version`: best-effort. On Linux glibc, read the first line of `/lib/x86_64-linux-gnu/libc.so.6 --version` (or equivalent for the resolved arch). `None` on musl, macOS native, or any platform where the probe fails — treat as a constant absence rather than a fault.
- `locale_baseline`: read `LANG`, `LC_ALL`, every `LC_*` env (full glob), `TZ`, and `SOURCE_DATE_EPOCH` into a sorted map. These are universal output-affecting envs that bypass the denylist.
- Tool probe: `which::which(primary_tool)` → `std::fs::canonicalize` → SHA-256 over file bytes. Cached in `tool_cache` keyed by canonical path so each binary is hashed at most once per build.

**Failure modes:**

- Tool not found on `$PATH` → `tool_hash = sha256("")`. Step still cacheable; first run on a different machine where the tool resolves will miss correctly. (Empty-hash and resolved-hash differ; no false hit.)
- File unreadable → same as above. **Cache miss > cache poison** is the universal safety direction.

**Per-step composition:**

```rust
impl ExecutionContext {
    pub fn step_context_hash(&self, command: &str) -> u64 {
        let primary_tool = first_argv_token(command);
        let realpath = which::which(primary_tool)
            .ok()
            .and_then(|p| std::fs::canonicalize(p).ok());
        let tool_hash = self.tool_hash_for(realpath.as_deref());
        let mut hasher = Xxh3::new();
        hasher.update(&self.machine.encode());
        hasher.update(&tool_hash.content_sha256);
        hasher.digest()
    }
}
```

### 4.3. Env contribution and denylist

`cli/crates/cook-cache/src/envkey.rs`:

```rust
pub struct EnvDenylist {
    names: HashSet<String>,
    globs: Vec<glob::Pattern>,  // for "XDG_*", "GITHUB_*", etc.
}

impl EnvDenylist {
    /// D1: Cook-shipped baseline. See Appendix A for the full list.
    pub fn baseline() -> Self;
    /// D2: extend with .cook/cloud.toml [cache.ignore_env] entries.
    pub fn extend_from_cloud_toml(&mut self, additions: &[String]);
    pub fn is_ignored(&self, key: &str) -> bool;
}

/// Compute the env contribution for a step.
/// `consulted` is the BTreeMap<String, String> of (key, value) pairs
/// resolved during {TOKEN} substitution by cook-register, BEFORE denylist filtering.
pub fn env_contribution(
    consulted: &BTreeMap<String, String>,
    denylist: &EnvDenylist,
) -> u64 {
    let mut hasher = Xxh3::new();
    for (k, v) in consulted {
        if denylist.is_ignored(k) { continue; }
        hasher.update(k.as_bytes());
        hasher.update(b"=");
        hasher.update(v.as_bytes());
        hasher.update(b"\n");
    }
    hasher.digest()
}
```

`consulted` is built by `cook-register` during using-string substitution (§5.2): every `{TOKEN}` that fell through to `cook.env[TOKEN]` per Standard §B-rationale-291 is recorded. For `lua_chunk` payloads (where the command is a Lua block), v3 takes the conservative posture: `consulted` is the full `cook.env` map — the denylist still filters before hashing, so noise envs do not contribute, but every non-denylisted env is keyed. This over-invalidates Lua chunks; v4 narrows.

### 4.4. `CacheMeta` and `ArtifactMeta`

`cli/crates/cook-contracts/src/lib.rs`:

```rust
pub struct CacheMeta {
    pub recipe_name: String,
    pub project_id: String,                       // NEW: from .cook/cloud.toml
    pub cookfile_path: String,                    // NEW: relative to project root
    pub cache_key: String,                        // local-cache key (Section 5.1)
    pub input_paths: Vec<String>,
    pub output_paths: Vec<String>,
    pub command_hash: u64,
    pub context_hash: u64,                        // NEW
    pub env_contribution: u64,                    // NEW
    pub consulted_env: BTreeMap<String, String>,  // NEW: post-denylist
}

pub struct ArtifactMeta {
    pub recipe_namespace: String,         // see §5.2
    pub command_hash: u64,
    pub context_hash: u64,
    pub env_contribution: u64,
    pub schema_version: u32,
    pub size_bytes: u64,
    pub tags: BTreeSet<String>,           // for backend eviction policies
    /// Diagnostic only — backend SHOULD redact values for storage.
    pub consulted_env_keys: BTreeSet<String>,
}
```

`consulted_env` survives in `CacheMeta` so the cloud uploader can include the *keys* (not values) in `ArtifactMeta` for backend introspection. Values are never propagated to backend metadata.

## 5. Cache-key composition

### 5.1. Local cache key

The `RecipeCache.steps` BTreeMap is indexed by a string key. To allow two simultaneous variant builds (e.g., debug and release) to coexist in one local cache without overwrite, the local key must include the variant-discriminating fields:

```
local_cache_key = "{cookfile_path}::{recipe_name}::{step_name}::{command_hash:016x}::{context_hash:016x}::{env_contribution:016x}"
```

`step_name` is the primary output path if the step has outputs, else a register-time-stable derivation (existing fallback `"{first_input}@{command_hash}"` from `cli/crates/cook-register/src/unit_api.rs:94` — preserved). The full `(context_hash, env_contribution, command_hash, input hashes)` tuple is checked against the cached `StepEntry` content during rebuild evaluation; the key is just an index.

This means toggling `CXXFLAGS=-O2 ↔ -O3` on the same source produces two distinct entries in the local cache, and switching back rehits the prior entry — satisfying SHI-142 AC.

### 5.2. Recipe namespace

The cloud cache shares a bucket across recipes, projects, and teams. The namespace produces a globally-unique handle for *what build position a key represents,* independent of artifact content:

```
recipe_namespace = "{project_id}/{cookfile_path}::{recipe_name}"
```

Examples:
- `cook/Cookfile::build`
- `dhewm3/services/maps/Cookfile::bake`
- `cook/cli/crates/cook-lang/Cookfile::test`

`cookfile_path` is the relative path of the source Cookfile from the project root (the directory containing `.cook/cloud.toml`), forward-slashed and used verbatim.

`team_id` is **NOT** in the namespace; it is appended at upload time by the backend. Two teams building bit-identical artifacts produce identical `cloud_key`s — deduplicable at the R2 layer if cross-tenant dedup is ever desired — but stored at distinct R2 paths (`{team_id}/{cloud_key}`) so tenant isolation holds at the storage layer.

Notably **NOT** in the namespace:

- **Config name** (debug vs. release). A config that sets `CFLAGS=-O3` changes the rendered using-string → different `command_hash`; config that sets env → different `env_contribution`. The key already distinguishes; namespacing on config would be redundant.
- **Git ref / commit / branch.** Defeats the entire point of cloud sharing. Bob's CI on `feature-x` should hit Alice's `main` build if source files match. The cache key already includes input content hashes, so source state is fully in the key.
- **CLI version.** Handled separately via `schema_version` in the cloud-key composition.

### 5.3. Cloud key

```rust
pub fn cloud_key(meta: &CacheMeta, schema_version: u32) -> [u8; 32] {
    let mut h = Sha256::new();
    h.update(&schema_version.to_le_bytes());
    h.update(meta.recipe_namespace().as_bytes());
    h.update(&[0x00]); // delimiter — prevents string-injection collisions
    h.update(&meta.command_hash.to_le_bytes());
    h.update(&meta.context_hash.to_le_bytes());
    h.update(&meta.env_contribution.to_le_bytes());
    // Inputs: path-sorted; hash only contents, not paths
    // (paths can vary between machines for the same content).
    let mut sorted: Vec<&FileRecord> = meta.inputs_with_hashes().iter().collect();
    sorted.sort_by(|a, b| a.path.cmp(&b.path));
    for record in sorted {
        h.update(&record.hash.to_le_bytes());
    }
    h.finalize().into()
}
```

The `0x00` delimiter between namespace string and the rest prevents string-injection collisions: a namespace whose textual encoding would otherwise pretend to be the next field's bytes is structurally distinguished.

`xxh3_64` is preserved everywhere the local cache reads — `command_hash`, `context_hash`, `env_contribution`, file content hashes — for performance. SHA-256 is computed only when materializing the cloud key (rare: once per cacheable unit per build), so the cloud-correctness story does not regress local-cache performance.

## 6. The `CacheBackend` trait

`cli/crates/cook-cache/src/backend.rs`:

```rust
pub type CloudKey = [u8; 32];

#[derive(Debug)]
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

pub type BackendResult<T> = Result<T, BackendError>;

pub trait CacheBackend: Send + Sync {
    /// Batch existence check. Returns the subset of inputs that are hits.
    /// MUST be deterministic for the same inputs in the same backend state.
    fn batch_query(&self, keys: &[CloudKey]) -> BackendResult<BTreeSet<CloudKey>>;

    /// Fetch artifact bytes. Returns Ok(None) on miss (NOT BackendError).
    /// MAY update last_accessed_at server-side as a side effect (debounced).
    fn get(&self, key: &CloudKey) -> BackendResult<Option<Vec<u8>>>;

    /// Upload artifact bytes with metadata. Idempotent on (key, bytes):
    /// re-putting the same pair MUST succeed; re-putting (same key, different
    /// bytes) is implementation-defined (LocalBackend overwrites; CloudBackend
    /// SHOULD reject).
    fn put(&self, key: &CloudKey, bytes: &[u8], meta: &ArtifactMeta) -> BackendResult<()>;

    /// Explicit deletion. Used by admin tooling; engine never calls this during build.
    /// Idempotent: returns Ok(()) for both "deleted" and "didn't exist".
    fn delete(&self, key: &CloudKey) -> BackendResult<()>;

    /// Lightweight health check. Engine calls once at build start.
    /// On failure, engine logs and proceeds with backend disabled for the build.
    fn health(&self) -> BackendResult<()>;
}
```

The trait is **synchronous**. Cloud backends manage async I/O internally (worker thread + `reqwest::blocking`, or an internal tokio runtime) so tokio's "color" does not infect `cook-cache` or `cook-engine`.

### 6.1. Engine policy on backend errors

The build NEVER fails because the backend failed:

- `Transient(_)` from `batch_query` / `get` → step treated as a miss; local execution path runs.
- `Transient(_)` from `put` → log once, drop the upload silently; local cache still has the entry, the next successful build will retry.
- `Unauthorized(_)` from any method → log once at warn level, disable backend for the rest of the build.
- `QuotaExceeded` from `put` → log, drop the upload.
- `Other(_)` → same as `Transient(_)` for the offending call.

This is the contract that lets Cook Cloud be a *cache* and not a *dependency*. Network down → slow build, not failed build.

### 6.2. `LocalBackend` — the v3 implementation

`LocalBackend` stores artifact bytes under a configurable root (default `~/.cache/cook/cloud/`), fanned out by the first byte of the cloud key (`{root}/ab/cdef...`) to avoid one-million-files-in-one-directory pathologies. A sidecar `<key>.meta.json` file holds the `ArtifactMeta`. Writes are atomic (tmp + rename).

`LocalBackend` is **not** the same as today's `RecipeCache` on-disk store. The recipe cache file (`<recipe>.bin`) is the *index* — "what cache_key did this recipe's step produce, and what are its input hashes?" The `LocalBackend` is the *artifact store* — "given a cloud_key, give me the bytes that were produced." Today, Cook only has the index; build artifacts live in the build directory at `output_paths`. Post-spec, the artifact store is a separate concern accessed via the trait.

For local-only use, `LocalBackend` is mostly redundant with the build directory itself. It exists primarily as the **first conformance test of the trait** and as a **fallback** for users with multi-checkout workflows (artifacts at `~/.cache/cook/cloud/...` survive `git clean -fdx`). Cook Cloud's `CloudBackend` (SHI-24) replaces it as the production backend; `LocalBackend` stays as the offline default.

## 7. `record_completion` hardening (SHI-146)

`cli/crates/cook-cache/src/manager.rs`:

```rust
impl ThreadSafeCacheManager {
    pub fn record_completion(
        &self,
        recipe_name: &str,
        cache_key: &str,
        meta: &CacheMeta,
        working_dir: &Path,
    ) -> Result<(), RecordError> {
        let new_inputs = collect_records(&meta.input_paths, working_dir)?;
        let new_outputs = collect_records(&meta.output_paths, working_dir)?;

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
}

fn collect_records(paths: &[String], working_dir: &Path) -> Result<Vec<FileRecord>, RecordError> {
    let mut out = Vec::with_capacity(paths.len());
    for rel in paths {
        let abs = working_dir.join(rel);
        let mtime = stat_mtime(&abs).ok_or_else(|| RecordError::MissingFile(rel.clone()))?;
        let hash = hash_file(&abs).ok_or_else(|| RecordError::UnreadableFile(rel.clone()))?;
        out.push(FileRecord { path: rel.clone(), mtime, hash });
    }
    Ok(out)
}

#[derive(Debug)]
pub enum RecordError {
    MissingFile(String),
    UnreadableFile(String),
}
```

**Caller policy.** The engine calls `record_completion` after a step's command exits successfully. On `Err(_)`:

1. Log via `tracing::warn!("cache: skipping record for {recipe}::{cache_key}: {err}")`. Visible at default verbosity, never silenced.
2. Do **NOT** write any cache entry. The previous entry (if any) is left untouched. Specifically: not "write a partial entry" or "write zeros" — the prior valid entry survives so a subsequent run with the file present can hit on it.
3. The build continues; the failed step's output is whatever it produced on disk.
4. The next build will see no cache entry for this step and re-execute. **No poison reaches the cloud.**

`record_completion` is the sole producer of `StepEntry`. If it cannot produce a complete one, no `StepEntry` exists. This is how invariant 1 of §3.2 (Local-correct ⊕ cloud-correct) holds.

## 8. Engine integration

### 8.1. Build bootstrap

In `cli/crates/cook-engine/src/run.rs`, the build's setup phase gains four steps before the existing register-phase invocation:

```rust
fn run_build(args: &BuildArgs) -> Result<()> {
    // 1. Probe the machine. Reused by every step in this build.
    let exec_ctx = Arc::new(ExecutionContext::probe());

    // 2. Load .cook/cloud.toml (if present) for project_id and denylist.
    let cloud_config = CloudConfig::load_or_default(&project_root)?;
    let denylist = EnvDenylist::baseline()
        .extended_with(&cloud_config.cache_ignore_env);

    // 3. Construct the backend. v3 ships LocalBackend only.
    //    SHI-24 plugs in CloudBackend behind the same trait.
    let backend: Arc<dyn CacheBackend> = Arc::new(LocalBackend::new(
        cloud_config.cache_dir.unwrap_or_else(default_cloud_cache_dir)
    ));
    if let Err(e) = backend.health() {
        tracing::warn!("cache backend unavailable: {e:?}; continuing with backend disabled");
    }

    // 4. Build context bundle threaded into every register-phase invocation
    //    and every executor invocation.
    let cache_ctx = Arc::new(CacheContext { exec_ctx, denylist, backend, cloud_config });

    // ... rest of build proceeds; cache_ctx flows down ...
}
```

`CacheContext` is the single struct every cache call site needs. It is `Arc`-shared because `ThreadSafeCacheManager` runs steps in parallel (existing behavior).

### 8.2. Register-phase changes (`cook-register`)

**`{TOKEN}` substitution records consulted env.** The substitution path that today resolves `{TOKEN}` to `cook.env[TOKEN]` per Standard §B-rationale-291 gains a per-unit accumulator:

```rust
struct SubstitutionContext<'a> {
    env: &'a BTreeMap<String, String>,
    consulted: BTreeMap<String, String>, // NEW: records what was actually read
}

impl<'a> SubstitutionContext<'a> {
    fn resolve(&mut self, token: &str) -> Option<String> {
        match self.lookup_placeholder_or_recipe(token) {
            Some(v) => Some(v),
            None => {
                let value = self.env.get(token)?.clone();
                self.consulted.insert(token.to_string(), value.clone());
                Some(value)
            }
        }
    }
}
```

The `consulted` map flows into `CacheMeta` construction.

**`CacheMeta` construction populates the new fields.** In `cli/crates/cook-register/src/unit_api.rs`'s `add_unit` at the cache-meta block:

```rust
let cache_meta = if cache_enabled {
    let context_hash = cache_ctx.exec_ctx.step_context_hash(&command);
    let env_contribution = env_contribution(&consulted_env, &cache_ctx.denylist);
    let local_key = build_local_key(
        &cookfile_relpath, &rname, &step_name,
        command_hash, context_hash, env_contribution,
    );

    Some(CacheMeta {
        recipe_name: rname.clone(),
        project_id: cloud_config.project.clone(),
        cookfile_path: cookfile_relpath.clone(),
        cache_key: local_key,
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

For `lua_chunk` payloads, `consulted_env` is the full denylist-filtered `cook.env` map — see §4.3 final paragraph.

### 8.3. Lookup path (`cook-engine/src/executor.rs`, `cook-cli/src/dag_data.rs`)

Both call sites construct the lookup key and check the cached `StepEntry`. The change is purely additive — comparison now also covers `context_hash` and `env_contribution`:

```rust
if entry.context_hash != current_context_hash {
    return RebuildResult::Rebuild(RebuildReason::ContextChanged);
}
if entry.env_contribution != current_env_contribution {
    return RebuildResult::Rebuild(RebuildReason::EnvChanged);
}
// existing checks: command_hash, output presence, input set/content
```

`RebuildReason` gains two variants (Appendix B taxonomy):

```rust
pub enum RebuildReason {
    NoCacheEntry,
    CommandHashChanged,
    ContextChanged,        // NEW — tool binary, machine identity, libc, locale baseline
    EnvChanged,            // NEW — consulted env values
    OutputMissing,
    OutputChanged,
    InputSetChanged,
    InputChanged(String),
}
```

These improve diagnostics: `cook build -v` prints `"rebuild: env CFLAGS changed"` instead of an opaque `"command hash changed"`.

### 8.4. Removal of `invalidate_if_env_changed`

`ThreadSafeCacheManager::invalidate_if_env_changed` is **deleted**. The recipe-wide wipe is gone; per-step keying handles env changes correctly without nuking unaffected steps.

`cli/crates/cook-engine/src/run.rs:165-169` (the call site `let env_hash = hash_env(&units.env_vars); ... .invalidate_if_env_changed(name, env_hash)`) is also deleted. The recipe's env now flows into per-step `consulted_env` via the substitution path; no recipe-level wipe is needed.

This is a behavior improvement visible to users: changing `CXXFLAGS` no longer invalidates the recipe's *non-CXX* steps. Test step, plate steps, doc-gen — all keep their cache entries.

## 9. `.cook/cloud.toml` schema additions

```toml
[cloud]
enabled = false                        # default; --cloud overrides (SHI-26)
endpoint = "https://api.cook.dev"      # cloud backend URL (consumed by SHI-24)
project = "cook"                       # REQUIRED when cloud.enabled = true; project_id in namespace

[cache]
ignore_env = ["GITHUB_TOKEN", "NPM_TOKEN", "AWS_PROFILE"]  # D2 denylist additions
cache_dir = "~/.cache/cook/cloud"      # optional override for LocalBackend root
```

The `[cloud]` section is partly forward-defined for SHI-24 (`endpoint`, `enabled`); v3 only consumes `project`. The `[cache]` section is fully consumed in v3.

**Validation rules at build start (CLI errors):**

- `cloud.enabled = true` AND missing `project` → error.
- `cache.ignore_env` contains entries that overlap the hardcoded D1 baseline → warning, no error (idempotent).
- File missing entirely → fine; `project` defaults to the directory name of the project root, `ignore_env` is empty (D1 baseline only). The default-`project` fallback is an explicit affordance for "I'm trying Cook without setting up Cloud."

Once `cloud.enabled = true`, `project` is **required** (no implicit name) so cloud uploads have a stable identity.

## 10. Standard impact

Only §8.6 ¶3 changes. No grammar change, no §6 (Lua API) change, no new Cookfile syntax. The `.cook/cloud.toml` file is a CLI affordance — implementation-defined per Standard §B-rationale-78 ("Config selection is a CLI affordance...The Standard's general posture is to specify Cookfile-language behaviour rather than tool invocation surfaces").

### 10.1. Proposed §8.6 amendment

Current §8.6 ¶3 reads:

> A conforming implementation MAY additionally consider the content of the declared input files, the modification times of the declared output files, and the content of the recipe's ingredient set. It MUST NOT consider wall-clock time, process identifiers, or any state not derivable from the sources above.

Replacement:

> A conforming implementation MAY additionally consider the content of the declared input files, the modification times of the declared output files, the content of the recipe's ingredient set, the identity of the host machine on which the unit will execute (e.g., target triple, libc version, locale-affecting environment variables), and the resolved content of the binaries invoked by the unit's command text. It MUST NOT consider wall-clock time, process identifiers, or any state not derivable from the sources above. An implementation MAY exclude from the resolved-environment observable a set of environment variables whose values are known not to affect work-unit output (e.g., ambient session state, credentials unrelated to the build product); the specific set is implementation-defined.

Three additions, all permissive (`MAY`), all consistent with §B-rationale-225's framing of the abstract/concrete split.

### 10.2. Companion Appendix B paragraph

A short note added to `standard/src/content/docs/appendix/B-rationale.mdx` near §B-225:

> **Cross-machine cache correctness.** §8.6's enumeration of permissible cache observables now includes machine identity and tool-binary content. The motivation is the shared/team cache: an artifact produced on one machine and served to another must remain correct against differences in compiler version, libc, locale, and the resolved binary that the command text names. A Cook implementation that hashes only "command text + input files" is correct on a single machine but unsafe across machines; the additional observables let an implementation be cross-machine-safe without taking on those observables when running locally. The denylist clause acknowledges that not every difference in the resolved environment affects build output (ambient session state, secrets unrelated to the artifact), and lets implementations filter without violating the minimum-observable enumeration of ¶2.

## 11. Acceptance criteria

The spec ships when **all** of these pass.

**SHI-141 (execution context):**

- AC-141.1 Switching `CC=gcc` ↔ `CC=clang` in the same checkout produces different `context_hash` values; the entire cache misses, no false hits.
- AC-141.2 Identical machine + identical resolved tool binary → identical `context_hash`.
- AC-141.3 Differing tool binary content → different `context_hash`.
- AC-141.4 Differing target triple → different `context_hash`.
- AC-141.5 A Nix-shell build on machine A and a Nix-shell build on machine B with the same Nix derivation produce the same `context_hash` (cross-machine determinism via content-addressed binaries).

**SHI-142 (env per-step keying):**

- AC-142.1 Two builds with `CXXFLAGS=-O2` vs `-O3` get distinct cache entries; no overwrites, no false hits.
- AC-142.2 Toggling `CXXFLAGS` back to a prior value re-hits the prior entry (no over-invalidation).
- AC-142.3 Recipe-level `env_hash` no longer wipes per-step entries; `invalidate_if_env_changed` is gone.
- AC-142.4 A non-`{CXXFLAGS}`-consulting step in the same recipe keeps its cache entry across `CXXFLAGS` toggles.

**SHI-143 + SHI-144 (cloud key):**

- AC-143.1 Local cache performance unchanged — still xxh3_64 for `command_hash`, `context_hash`, `env_contribution`, file content.
- AC-143.2 `cloud_key()` is deterministic across runs and platforms (same logical inputs → same 32-byte digest).
- AC-143.3 Two `StepEntry` values that differ only in a non-key-relevant field (e.g., `mtime`) produce the same `cloud_key`.
- AC-143.4 Two `StepEntry` values differing in any key-relevant field produce different `cloud_key`.
- AC-144.1 Two recipes both producing `build/main.o` from different sources/commands produce different cloud keys.
- AC-144.2 Reordering inputs (via `BTreeMap` iteration determinism) does not change the cloud key.
- AC-144.3 Two distinct `team_id`s building the same artifact produce the same `cloud_key` (deduplicable across tenants at R2 if desired) but stored at different R2 paths.

**SHI-145 (dead code removal):**

- AC-145.1 No production code references `secondary_inputs_hash`.
- AC-145.2 `hash_secondary_inputs` and `invalidate_recipe` deleted.
- AC-145.3 v2 cache files on disk return `None` from `RecipeCache::load`; not deserialized into a half-populated struct.

**SHI-146 (record_completion hardening):**

- AC-146.1 With one input file removed before `record_completion` is called: the function does not panic, does not write a `StepEntry`, and emits a structured `tracing::warn!` line that callers can observe.
- AC-146.2 Happy-path tests pass unchanged.
- AC-146.3 Cache state for the affected step is left untouched (no partial overwrite of a previously valid entry).

**B2 backend seam:**

- AC-B2.1 `LocalBackend` passes a conformance test suite covering `batch_query` / `get` / `put` / `delete` / `health`. The same suite will run against `CloudBackend` in SHI-24.
- AC-B2.2 Backend errors in any method NEVER fail the build. Transient errors degrade to "miss"; auth/quota errors disable the backend for the build with one `tracing::warn!`.
- AC-B2.3 `put` is idempotent: re-putting the same `(key, bytes)` succeeds.

**Env denylist:**

- AC-Env.1 Hardcoded baseline (D1) excludes the names listed in Appendix A.
- AC-Env.2 `.cook/cloud.toml` `cache.ignore_env` extends D1 via union semantics.
- AC-Env.3 A `{TOKEN}` whose name is in the union denylist substitutes its value into the rendered command but does NOT contribute to `env_contribution`.

**Cross-cutting integration:**

- AC-Integ.1 Two-machine first-build depfile fixture: machine A first compile (no `.d` file → `inputs = [source]` only), uploads. Machine B with empty cache pulls. Cache hit is correct (same source content); B's subsequent builds *do* generate a depfile and pick up header changes. Closes the SHI-140 §6 thin-input concern.
- AC-Integ.2 Cross-recipe `build/main.o` collision: two recipes producing same path in same bucket → distinct cloud keys, no overwrite.
- AC-Integ.3 Toggling between two configs preserves both cache entries.

## 12. Test coverage plan

Three test categories:

1. **Unit tests in `cook-cache`.** All AC-* items directly exercise crate-internal functions (`step_context_hash`, `env_contribution`, `cloud_key`, `record_completion`, denylist application, schema-version round-trip). New file `cli/crates/cook-cache/src/context.rs` ships with its own `#[cfg(test)]` block; `envkey.rs`, `backend.rs`, and the modified `manager.rs` likewise.
2. **Integration tests in `cli/tests/`.** AC-Integ.* and AC-141.5 (the cross-machine Nix-shell scenario) require multi-process or multi-tempdir fixtures. AC-Integ.1 (the depfile thin-input fixture) is the highest-value test in this category.
3. **Conformance harness updates in `standard/conformance/`.** Add positive cases for: the `# env: {TOKEN}` hidden-env idiom (verifies the substitution captures the env into the cache key), the schema-v2-on-disk-rejection case, the `record_completion` skip-on-missing-input case.

## 13. Rollout

- One implementation branch off `main`, structured as a series of small, individually-mergeable commits per the Cook Standard pre-1.0 lockstep posture (CLAUDE.md, project memory).
- The §8.6 amendment lands in the **same commit** as the cache-correctness changes that exercise it. Per `core.hooksPath` the spec-first pre-commit hook will reject any code-side commit without a corresponding Standard delta.
- Schema bump (v2 → v3) is a one-time invalidation. Document in release notes; first build after upgrade rebuilds everything.
- No feature flag. The cache-correctness pass is strictly better than the v2 behavior on a single machine (per-step env keying eliminates the recipe-wide wipe; `record_completion` no longer poisons on missing files); shipping behind a flag would only delay the safety improvement.

## Appendix A — D1 hardcoded denylist (full list)

Names (exact match):

```
HOME, USER, LOGNAME, SHELL, PATH, PWD, OLDPWD, MAIL, HOSTNAME,
TERM, TERMINFO, COLORTERM,
DISPLAY, WAYLAND_DISPLAY, XAUTHORITY,
SSH_AUTH_SOCK, SSH_CONNECTION, SSH_CLIENT, SSH_TTY,
DBUS_SESSION_BUS_ADDRESS, DBUS_STARTER_BUS_TYPE, DBUS_STARTER_ADDRESS,
EDITOR, VISUAL, PAGER, BROWSER,
TMPDIR, TMP, TEMP,
HISTFILE, HISTSIZE, HISTCONTROL,
SHLVL, PS1, PS2, PS3, PS4
```

Glob patterns:

```
XDG_*           — desktop-environment session paths
GITHUB_*        — CI ambient metadata (also catches GITHUB_TOKEN)
RUNNER_*        — GitHub Actions runner metadata
GITLAB_CI_*     — GitLab runner metadata
CI              — universal CI marker
BUILDKITE_*     — Buildkite runner metadata
CIRCLE_*        — CircleCI runner metadata
TRAVIS_*        — Travis CI metadata
JENKINS_*       — Jenkins metadata
TEAMCITY_*      — TeamCity metadata
DRONE_*         — Drone CI metadata
```

`PATH` is on the list because tool identity is captured by the binary-content hash; PATH itself is just the lookup mechanism.

Locale variables (`LANG`, `LC_*`, `TZ`, `SOURCE_DATE_EPOCH`) are **NOT** on the denylist — they are deliberately keyed via `MachineIdentity::locale_baseline` (§4.2).

## Appendix B — RebuildReason taxonomy

All rebuild-reason variants and what each means in user-visible output:

| Variant | Meaning | Diagnostic example |
|---|---|---|
| `NoCacheEntry` | First build, no prior `StepEntry`. | `rebuild: no cache entry` |
| `CommandHashChanged` | Rendered command text differs (substituted `{TOKEN}`s changed, or recipe template changed). | `rebuild: command text changed` |
| `ContextChanged` | Tool binary content, target triple, libc version, or locale baseline differs. | `rebuild: tool binary changed (gcc)` |
| `EnvChanged` | Consulted env value differs (and not denylisted). | `rebuild: env CXXFLAGS changed` |
| `OutputMissing` | A declared output file no longer exists on disk. | `rebuild: output build/main.o missing` |
| `OutputChanged` | A declared output file's content differs from cached `FileRecord`. | `rebuild: output build/main.o tampered` |
| `InputSetChanged` | The set of declared input paths differs (e.g., new header from depfile). | `rebuild: input set changed (new headers)` |
| `InputChanged(path)` | An input file's content differs. | `rebuild: input src/main.c changed` |

`-v` verbosity prints the variant; `-vv` adds the per-field old-vs-new for `Context`/`Env` (e.g., `was: gcc 13.2 (sha:abc..), now: gcc 13.3 (sha:def..)`).
