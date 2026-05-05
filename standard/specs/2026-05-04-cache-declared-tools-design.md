# Design: Declared toolchain pinning for cross-machine cache correctness

**Date:** 2026-05-04
**Status:** Design — pending implementation plan
**Standard change ID:** CS-0052 (assigned at PR time)
**Linear epic:** SHI-140 follow-up — *Toolchain pinning for cache correctness*
**Predecessors:**
  - [2026-05-01-cache-cloud-readiness-design.md](./2026-05-01-cache-cloud-readiness-design.md) (introduced `context_hash` + first-token tool fingerprinting)
  - [2026-05-02-cache-restore-and-dep-inputs-design.md](./2026-05-02-cache-restore-and-dep-inputs-design.md)
**Scope:** `cli/crates/cook-cache`, `cli/crates/cook-fingerprint`, `cli/crates/cook-cli`, and amendments to Cook Standard §8.6 (Execution model — cache) and §9 (`.cook/cloud.toml` schema).

## 1. Motivation

The 2026-05-01 design established `context_hash` over machine identity plus the SHA-256 of every binary the engine could *automatically* identify by walking the rendered command body. That walk has a documented coverage limit (`cli/crates/cook-fingerprint/src/context.rs:164-170`): only the first executable on each newline-separated statement is fingerprinted. Tools invoked downstream of `&&`, `||`, `|`, `;`, `$(...)`, subshells, or `xargs`/`find -exec` are silently invisible. So is `gcc` inside `bash -c "gcc foo.c"`.

For a single-developer build that runs end-to-end on one host this is harmless: the elided tools are constant. For a shared cache — the entire point of the cache cloud-readiness work — it is a soundness gap. A consumer who upgrades their `strip` while their `gcc` stays pinned will silently serve stripped objects produced by the *old* strip to other machines. The fix-them-all surface is large (a real shell parser), but the operational reality is smaller: in any environment that takes cross-machine caching seriously, tools are already pinned at the environment level.

**The deployment model the cache is optimised for is the deployment model that makes pinning easy.** Dev containers, `nix-shell`, `nix develop`, Docker dev environments, and Bazel-style hermetic toolchains all swap their toolchain *atomically*: when the environment changes, every tool in it changes together. The user of such an environment does not want gcc-13's parser to survive a flake update — they want the whole cache to invalidate when the environment does. Global-fold semantics is exactly that contract.

This spec adds an explicit, opt-in declaration — `[cache] tools` in `.cook/cloud.toml` — listing executable names whose content MUST be folded into every step's `context_hash` regardless of whether they appear textually in any step's command body. Projects that don't opt in see no behavioural change; projects that do get a hash that invalidates correctly when their toolchain moves.

## 2. Non-goals

- **Per-recipe or per-step tool declaration.** `cache.tools` is project-wide. A future Cookfile-grammar form (`recipe foo { tools = [...] }`) is the right shape for monorepos with diverging toolchains per recipe but is out of scope here. Global is enough for the dev-container / nix-shell case this spec targets.
- **Shell-aware command parsing.** No pipeline / `&&` / `bash -c` / `find -exec` walk. The first-token-per-line auto-detection from 2026-05-01 stays unchanged. Declared tools are *additive*, not replacement.
- **Wrapper-script transparency.** Declaring `gcc` hashes whatever `which gcc` resolves to. If that's a ccache wrapper, the wrapper is what's hashed; the wrapped compiler is not. This is the same limitation 2026-05-01 §2 documented for auto-detected tools and is preserved deliberately.
- **Transitive binary capture.** Declaring `gcc` does not hash `cc1`, `as`, `ld`, or libstdc++ headers. Same boundary as 2026-05-01 §2; users who care can declare those binaries explicitly.
- **Allowlist-based replacement of auto-detection.** A future "cache.tools is the *only* signal" mode (where auto-detection is suppressed and only declared tools count) is out of scope. v1 always runs both signals; declared tools are unioned into the result.
- **Per-tool pinning to a specific hash or version.** Declaration is by name; the hash is whatever the resolved binary contains at build start. There is no `cache.tools = { gcc = "sha256:abcd…" }` form. If a user wants pinning to a specific binary, that's the environment's job (nix, container image, etc.).
- **Schema bump.** `CACHE_VERSION` does not change. An empty `cache.tools` produces `declared_tools_hash = 0` (sentinel), which folds neutrally — projects without the config produce byte-identical `context_hash` values to v3.

## 3. Architecture

### 3.1. Modules touched

```
cli/crates/
├── cook-cache/
│   └── cloud_config.rs       CacheSection gains `tools: Vec<String>`
├── cook-fingerprint/
│   └── context.rs            ExecutionContext gains declared_tools_hash;
│                             step_context_hash folds it in
└── cook-cli/
    └── (binding wire-up)     load CacheSection.tools at build start;
                              resolve + hash; pass into ExecutionContext

standard/
├── src/content/docs/08-execution-model.mdx     §8.6 amendment
└── src/content/docs/09-cloud-config.mdx        §9.x add `[cache] tools`
```

No Cookfile grammar change. No Cook Lua API change. No on-disk schema change. The `StepEntry.context_hash` field's *meaning* widens (it now incorporates declared tools), but its shape, type, and storage are unchanged.

### 3.2. Architectural invariants preserved

1. **Empty config = v3 identity.** With `[cache] tools = []` (or the section absent), `declared_tools_hash` evaluates to a sentinel zero and `step_context_hash` folds it in as zero bytes. The resulting `context_hash` is byte-identical to a v3 build. No existing project sees cache invalidation from upgrading to a v3.1 client.
2. **Loud failure on misdeclaration.** A declared tool that does not resolve via `which` is a build-start error — not a silent degrade. The whole point of explicit declaration is to surface mistakes; falling back to "tool not found, ignored" reproduces the silent-miss class this spec exists to close.
3. **Global hash, single resolution.** Each declared tool is resolved exactly once per build, hashed exactly once, folded once into a single `declared_tools_hash` value. That value participates in every step's `context_hash`. No per-step iteration over the declared list.
4. **Auto-detection still runs.** The first-token-per-line resolution is unchanged. A step that uses `gcc` directly fingerprints gcc twice (once via auto-detection, once via the declared-tools fold) — both folds use the same canonical-realpath cache, so it's one read of gcc's bytes. Idempotent and deterministic.
5. **Diagnostic separation.** `MachineIdentity` answers "what host." `declared_tools_hash` answers "what toolchain did the user pin." Surfacing them as sibling inputs to `step_context_hash` (rather than collapsing declared tools into `MachineIdentity`) keeps `cook --explain-cache-key` interpretable.

### 3.3. Build-time flow (amends 2026-05-01 §3.3)

```
1. cook build starts
2. ExecutionContext::probe()
2a. NEW — for each name in cloud_config.cache.tools:
       which(name) || error("declared tool not on PATH: {name}")
       canonicalize → SHA-256 contents → insert into BTreeMap<name, [u8;32]>
       compute declared_tools_hash = xxh3_64 over BTreeMap iteration
       store on ExecutionContext
3. EnvDenylist::load(.cook/cloud.toml)
4. Backend::health()
5. Register phase
6. Compute cloud_keys (each step's context_hash now folds declared_tools_hash)
... (rest unchanged)
```

Step 2a is the only new flow node. It runs once, before any step is registered. Failure short-circuits the build with a clear diagnostic; the engine does not start.

## 4. Data structures

### 4.1. `CacheSection` extension

`cli/crates/cook-cache/src/cloud_config.rs`:

```rust
#[derive(Debug, Clone, Default, Deserialize)]
pub struct CacheSection {
    #[serde(default)]
    pub ignore_env: Vec<String>,
    #[serde(default)]
    pub cache_dir: Option<String>,
    #[serde(default)]
    pub tools: Vec<String>,   // NEW
}
```

User-facing TOML:

```toml
[cache]
tools = ["gcc", "ld", "strip", "ar"]
```

Names are interpreted by `which::which`. They MAY be unqualified (`gcc`) or absolute (`/usr/bin/gcc`); both forms resolve through the same `canonicalize` step that auto-detection uses, so a name and a path that point to the same realpath produce the same hash.

### 4.2. `ExecutionContext` extension

`cli/crates/cook-fingerprint/src/context.rs`:

```rust
pub struct ExecutionContext {
    pub machine: MachineIdentity,
    pub(crate) tool_cache: Mutex<HashMap<PathBuf, ToolHash>>,
    /// Build-wide hash of declared tools, folded into every step's context_hash.
    /// Zero (the sentinel) when [cache] tools is empty or absent — preserves
    /// v3 hash identity for projects that don't opt in.
    pub declared_tools_hash: u64,                            // NEW
    /// Diagnostic record: name → resolved realpath, in BTreeMap order.
    /// Surfaced by `cook --explain-cache-key`. Not part of the hash directly;
    /// the hash is computed from (name, content_sha256) pairs.
    pub declared_tools: BTreeMap<String, PathBuf>,           // NEW
}
```

The hash is computed in `ExecutionContext::probe_with_declared_tools(names: &[String])`. The existing `probe()` function delegates to it with an empty slice for backwards compatibility.

### 4.3. Hash composition

```rust
fn compute_declared_tools_hash(
    names: &[String],
    tool_cache: &Mutex<HashMap<PathBuf, ToolHash>>,
) -> Result<(u64, BTreeMap<String, PathBuf>), DeclaredToolError> {
    if names.is_empty() {
        return Ok((0, BTreeMap::new()));
    }
    // Resolve each declared name → canonical realpath. Loud failure if any miss.
    let mut resolved: BTreeMap<String, (PathBuf, [u8; 32])> = BTreeMap::new();
    for name in names {
        let p = which::which(name)
            .map_err(|e| DeclaredToolError::NotFound { name: name.clone(), source: e })?;
        let rp = std::fs::canonicalize(&p)
            .map_err(|e| DeclaredToolError::Canonicalize { name: name.clone(), source: e })?;
        let hash = {
            let mut cache = tool_cache.lock().expect("tool_cache poisoned");
            cache
                .entry(rp.clone())
                .or_insert_with(|| ToolHash::for_resolved(Some(rp.as_path())))
                .content_sha256
        };
        resolved.insert(name.clone(), (rp, hash));
    }
    // Fold deterministically: BTreeMap iteration is sorted by (declared) name.
    let mut hasher = xxhash_rust::xxh3::Xxh3::new();
    for (name, (_rp, sha)) in &resolved {
        hasher.update(name.as_bytes());
        hasher.update(&[0x00]);
        hasher.update(sha);
    }
    let realpaths = resolved.iter().map(|(n, (rp, _))| (n.clone(), rp.clone())).collect();
    Ok((hasher.digest(), realpaths))
}
```

Folding by **declared name** (not realpath) is deliberate: two distros where `gcc` resolves to differently-named realpaths (`/usr/bin/gcc-13` vs. `/usr/bin/gcc-11`) MUST produce different hashes. The sentinel-zero return for the empty-input case keeps the v3 identity invariant from §3.2.

### 4.4. `step_context_hash` fold

`step_context_hash` (currently at `cli/crates/cook-fingerprint/src/context.rs:171-189`) gains one extra `hasher.update` call after the machine bytes and before the per-step tool fold:

```rust
hasher.update(&machine_bytes);
hasher.update(&self.declared_tools_hash.to_le_bytes());   // NEW
// ... existing per-step tool fold unchanged
```

When `declared_tools_hash` is the sentinel zero, the eight bytes folded in are `[0; 8]`. **Note on backward compatibility:** the *literal byte stream* fed to the hasher does change for v3 projects (eight extra zero bytes appear before the per-step tools), so the *resulting* `context_hash` will differ from a v3 client's. The v3 cache entries become orphaned. This is a one-shot rebuild for projects upgrading; eviction reaps the orphans. If preserving v3 identity for empty-tools projects is required, the fold MUST be conditional (`if self.declared_tools_hash != 0 { hasher.update(...) }`) — see §10 open question 1.

## 5. Algorithms

### 5.1. Build-start resolution

The CLI binding (`cli/crates/cook-cli/src/main.rs` — exact insertion point depends on where `ExecutionContext::probe()` is called today; likely `run.rs`) replaces the bare `probe()` call:

```rust
let cloud_config = CloudConfig::load_or_default(project_root)?;
let exec_ctx = ExecutionContext::probe_with_declared_tools(&cloud_config.cache.tools)
    .map_err(|e| match e {
        DeclaredToolError::NotFound { name, .. } => anyhow!(
            "declared tool `{name}` not found on PATH (.cook/cloud.toml [cache] tools). \
             Either install it, remove it from the list, or run in an environment \
             where it resolves (devcontainer, nix-shell)."
        ),
        DeclaredToolError::Canonicalize { name, source } => anyhow!(
            "declared tool `{name}` resolved but could not be canonicalized: {source}"
        ),
    })?;
```

The diagnostic is verbose by design; this is where `[cache] tools` configuration mistakes surface.

### 5.2. Diagnostic surface

`cook --explain-cache-key <recipe>` (future flag, beyond this spec's scope) MUST print declared tools and their resolved realpaths. Until that flag exists, a `tracing::debug!` line at probe time logs them:

```rust
tracing::debug!(
    "declared cache tools: {:?}",
    self.declared_tools.iter()
        .map(|(n, p)| format!("{n} -> {}", p.display()))
        .collect::<Vec<_>>()
);
```

Surfacing the realpaths (not just names) is load-bearing for debugging "why did my cache invalidate" — the realpath tells the user *which* gcc the build saw.

## 6. Test plan

### 6.1. Unit tests (`cli/crates/cook-fingerprint/src/context.rs`)

- `compute_declared_tools_hash_empty_is_sentinel_zero` — empty slice returns `(0, BTreeMap::new())`.
- `compute_declared_tools_hash_deterministic` — two calls with same inputs return same hash.
- `compute_declared_tools_hash_differs_on_content` — two builds where `gcc`'s bytes change produce different hashes (use a fake binary + chmod +x).
- `compute_declared_tools_hash_differs_on_membership` — `["gcc"]` and `["gcc", "ld"]` produce different hashes.
- `compute_declared_tools_hash_order_independent` — `["gcc", "ld"]` and `["ld", "gcc"]` produce the *same* hash (BTreeMap-sorted).
- `compute_declared_tools_hash_errors_on_missing` — `["definitely-not-on-path-12345"]` returns `DeclaredToolError::NotFound`.
- `step_context_hash_folds_declared` — same step, two `ExecutionContext` instances differing only in `declared_tools_hash`, produce different `step_context_hash` values.

### 6.2. Integration tests (`cli/crates/cook-cache/tests/`)

`integration_declared_tools.rs`:
- Cookfile with no `[cache] tools` declared, build, snapshot `context_hash`. Compare to a build with `[cache] tools = []`. Assert byte-equal — empty list is a no-op.
- Cookfile with `[cache] tools = ["sh"]` declared. Build, populate cache. Replace `which sh`'s realpath bytes (use `LD_PRELOAD` or a fake-tool fixture on a custom `PATH`). Build again. Assert `InputChanged` / cache miss on every step.
- Cookfile with `[cache] tools = ["doesnt-exist-xyz"]`. Build. Assert cook exits with the configured error message and never starts the engine.

### 6.3. End-to-end fixture extension

`examples/cache_benchmarks/Cookfile` and `verify.sh` gain a scenario:

```
--- Scenario N: declared cache.tools fold ---
clean_state
add `tools = ["gcc"]` to .cook/cloud.toml
$COOK demo                              0 cached, 4 done
$COOK demo                              4 cached, 4 done
remove `tools = ...` from cloud.toml
$COOK demo                              0 cached, 4 done   (declared_tools_hash 0 → nonzero changed every key)
```

The last assertion documents the orphan-on-toggle behaviour — adding or removing `cache.tools` is a one-shot full rebuild.

## 7. Spec amendments

### 7.1. Standard §8.6 (Execution model — cache)

Append to the `context_hash` paragraph:

> The implementation MAY additionally fold a build-wide *declared toolchain hash* into `context_hash`. The declared toolchain is the set of executables enumerated by `[cache] tools` in `.cook/cloud.toml` (§9.x). When the set is empty (or the configuration absent), the declared toolchain hash MUST be a sentinel zero such that its contribution to `context_hash` is observationally inert; otherwise it MUST be a deterministic hash over the (name, content) pairs of every declared executable, where *content* is the bytes of the file that `which(name)` resolves to via canonical realpath. A misdeclared tool — one that does not resolve via `which` — MUST cause the build to fail before any step executes.

### 7.2. Standard §9 (`.cook/cloud.toml` schema)

Add a subsection documenting the field:

> #### `[cache] tools`
>
> An optional array of executable names whose content MUST be folded into every step's `context_hash`. Names are resolved via the host's `which` mechanism at build start; missing tools cause an immediate build failure. Use this option to pin toolchain identity in environments where the engine's automatic first-token-per-line tool detection cannot see the relevant binary (pipelines, `bash -c`, `find -exec`, etc.). In hermetic environments such as dev containers and `nix-shell`, declaring the environment's compiler / linker / archiver here ensures cache hits across machines remain correct when the environment is updated.
>
> Example:
>
> ```toml
> [cache]
> tools = ["gcc", "ld", "ar", "strip"]
> ```
>
> Equivalent forms: an empty array `tools = []` and an absent `tools` key are interchangeable; both produce the sentinel hash that has no effect on `context_hash`.

## 8. Backwards compatibility

- **On-disk cache:** `StepEntry.context_hash` is a u64 whose layout and storage are unchanged. The *value* differs once `[cache] tools` is non-empty; that value also differs once the spec is implemented for projects that previously had the field empty (§4.4 — eight extra zero bytes in the hash input). Pre-amendment entries become orphaned the next time those projects build under the new client; eviction reclaims them.
- **Backend trait:** unchanged.
- **Cookfile grammar:** unchanged.
- **Cook Lua API:** unchanged.
- **Configuration:** the existing `[cache]` section gains a field with `#[serde(default)]`; pre-amendment `cloud.toml` files parse identically.
- **Open question on identity preservation:** see §10.

## 9. Cross-references

- 2026-05-01 §2 documents the auto-detection coverage limit this spec partially closes.
- 2026-05-01 §3.3 build-time flow is amended by §3.3 above (one new step, 2a).
- The future shell-aware tokenizer work (currently un-issued) supersedes the auto-detection limit fully; this spec is a forward-compatible stopgap that handles the dev-container / nix-shell path well and leaves the shell-parser path open.

## 10. Open questions

1. **Preserve v3 hash identity for empty-tools projects?** The simple implementation (§4.4) always folds the sentinel zero, which changes the literal byte stream and causes a one-shot rebuild for every existing project on upgrade. A conditional fold (`if declared_tools_hash != 0 { ... }`) preserves v3 identity exactly for non-opting projects but introduces a special case in the hash composition that is mildly harder to reason about. Recommendation: **accept the one-shot rebuild**; eviction handles the orphans and the simpler invariant ("declared_tools_hash always folds, sentinel is zero") is worth the cost.
2. **Tool-name collision with shell builtins.** A user who declares `cd` or `echo` will resolve to the binary form (`/usr/bin/echo`) even though most shells use the builtin. This is harmless (the binary still hashes deterministically) but slightly counter-intuitive. Document in §9 the realpath the resolver chooses.
3. **Path-vs-name re-declaration.** `tools = ["gcc", "/usr/bin/gcc"]` declares the same tool twice under two names. Both fold into the hash; the BTreeMap distinguishes them by name. Acceptable (the hash is correct, just slightly redundant). Could be linted later.

## Appendix A. Why global, not per-step

A per-step alternative is to walk every step's tokenized command body and fold only the *intersecting* declared tools' hashes. This preserves incrementality across mixed-toolchain projects: upgrading rustc would invalidate Rust steps but not C steps. The tradeoffs:

- **Pro:** cache survives partial toolchain upgrades.
- **Con (1):** a sound implementation requires shlex-style tokenization plus one-level-deep recursion into quoted compounds (to catch `bash -c "gcc foo.c"`). That is the shell-aware tokenizer this spec explicitly defers.
- **Con (2):** the deployment model that benefits most from cross-machine caching — dev containers, nix-shell, hermetic Docker — already swaps tools atomically. Per-step granularity solves a problem those users do not have.
- **Con (3):** declaring `gcc` and seeing only-some-steps invalidate when its bytes change is a surprising semantics for the configuration's stated purpose ("pin these tools globally").

The per-step form is a strict refinement of the global form (a shlex-walking implementation can always degrade to global when configured to do so) and remains available as a v4 evolution. v1 is deliberately the simpler shape.
