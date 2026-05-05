# Design: Backend trait idempotency contract for the artifact store

**Date:** 2026-05-04
**Status:** Design — pending implementation plan
**Standard change ID:** CS-0055 (assigned at PR time; CS-0053 is reserved per CS-0054 §preamble)
**Linear epic:** SHI-24 cache cloud-readiness — *write-side soundness primitive for the shared backend*
**Predecessors:**
  - [2026-05-01-cache-cloud-readiness-design.md](./2026-05-01-cache-cloud-readiness-design.md) (introduced the `CacheBackend` trait, `cloud_key`, and the local-only artifact store)
  - [2026-05-04-cache-declared-tools-design.md](./2026-05-04-cache-declared-tools-design.md) (CS-0052)
  - [2026-05-04-cache-cloud-grade-integrity-design.md](./2026-05-04-cache-cloud-grade-integrity-design.md) (CS-0054 — the SHA-256 `content_hash` primitive this spec re-uses)
**Scope:** `cli/crates/cook-fingerprint/src/backend.rs` (`CacheBackend::put` trait-doc contract), `cli/crates/cook-cache/src/backend.rs` (`LocalBackend::put`), and an amendment to Cook Standard §{exec.cache.integrity}.

## 1. Motivation

CS-0054 closed the read-side soundness gap: `CacheBackend::get` MUST self-verify that the bytes it returns are byte-identical to the bytes most recently `put` under that key. The `content_hash: [u8; 32]` field on `ArtifactMeta` is the integrity primitive — `put` stamps it, `get` checks it.

That contract has a silent partner. The read-side check assumes the write side preserves the invariant "one key, one byte sequence". In the pre-CS-0055 code path, `LocalBackend::put` overwrites silently: `put(k, bytes_a)` followed by `put(k, bytes_b)` with `bytes_a != bytes_b` succeeds and the next `get(k)` returns `bytes_b`. On a single-developer machine this is harmless — the only writer is the local build, and a determinism bug that produces different bytes for the same `cloud_key` is a rebuild problem the user fixes locally. On a multi-tenant SaaS backend (SHI-24) it is a soundness gap. Two clients can race on the same `cloud_key`: client A writes a legitimate artifact; client B, running with a poisoned toolchain (a `gcc` not declared in `cache.tools` whose bytes drifted, an `LD_PRELOAD` shim, a compromised build environment), produces different bytes for what should be the same key. The pre-CS-0055 contract lets B's `put` silently replace A's bytes; every subsequent `get` from any tenant serves B's poisoned bytes under A's cache hit. The SHA-256 read-side check from CS-0054 doesn't help — B's bytes are self-consistent against B's recomputed `content_hash`.

This spec adds a write-side analogue of the CS-0054 read-side check. A `put` to a key that already holds an artifact MUST distinguish two cases: identical bytes (idempotent re-put — normal rebuild path, succeed) and conflicting bytes (key collision — refuse, return an error with a diagnostic naming the key). The implementation is cheap because CS-0054 already computes SHA-256 of the bytes during `put`; we additionally read the existing sidecar's `content_hash` and compare. This makes the write-side invariant the read-side check assumes — "one key, one byte sequence over the artifact's lifetime" — a normative property of the trait rather than an emergent property of the local-only deployment.

## 2. Non-goals

- **Cross-tenant attribution / signed conflict reports.** When `put` rejects a conflict, the diagnostic names the key and the differing hashes. It does not name the tenant or the build that wrote the prior artifact. Tenant-scoped attribution requires a signing root and is deferred to the SLSA / signed-artifact track (cf. CS-0054 §2).
- **Conflict resolution policy.** A rejected conflict surfaces as `BackendError::Other`. The engine's response (log + treat as `put`-failure, propagate, retry on a different key, etc.) is engine-side policy — not specified here. The trait contract is "refuse, do not overwrite"; how the engine handles refusal is orthogonal.
- **Compare-and-swap / etag semantics for the cloud backend.** The cloud backend will need an HTTP-level CAS primitive (e.g., R2's `If-Match: <etag>`) to make this contract racy-safe across concurrent writers. That implementation detail is the cloud backend's design; this spec establishes the trait contract the cloud backend MUST honour.
- **Dropping the read-side check.** CS-0054's `get` self-verify is unchanged. The two checks are complementary: write-side enforces the one-key-one-bytes invariant; read-side enforces that the bytes that came back are still the bytes that went in (defends against bit-flip, replication corruption, bytes-only tampering after the fact).
- **Trait shape change.** `CacheBackend::put` keeps its signature. The contract tightens; the type does not.

## 3. Architecture

### 3.1 Modules touched

```
cli/crates/
├── cook-fingerprint/
│   └── backend.rs            CacheBackend::put trait doc tightened —
│                             new "Idempotency contract (CS-0055)"
│                             paragraph specifies the conflict semantics.
└── cook-cache/
    └── backend.rs            LocalBackend::put pre-flight checks the
                              existing sidecar's content_hash before
                              writing; idempotent on match, errors on
                              conflict, writes through on missing/
                              malformed sidecar.

standard/
└── src/content/docs/08-execution-model.mdx     §8.6.1 amendment
```

No grammar change. No Lua API change. No on-disk schema change. No `CACHE_VERSION` bump. Existing sidecars produced by CS-0054 contain the `content_hash` the new code path needs; pre-CS-0054 sidecars (which deserialize with the zero sentinel) trigger the read-side fail-closed path on first read and rebuild — same orphan-on-upgrade behaviour CS-0054 §8 documents.

### 3.2 Architectural invariants preserved

1. **Trait shape stable.** `CacheBackend::put` signature unchanged. Only the trait-level contract tightens.
2. **Single source of truth on `content_hash`.** `put` is still the only writer of `content_hash`; the conflict check reads the sidecar of an *existing* artifact (whose `content_hash` was stamped by an earlier `put`), not a value supplied by the caller.
3. **Cheap conflict detection.** SHA-256 of the new bytes is already computed for CS-0054's stamping. The conflict check is one extra sidecar read plus one 32-byte equality test — O(1) in the artifact size beyond what CS-0054 already pays.
4. **Partial-write recovery.** A missing or unparseable sidecar — the partial-write recovery case CS-0054 §3.2 calls out — is treated as "no existing artifact", and `put` writes through. Otherwise an interrupted write between bytes-rename and sidecar-rename would brick the key permanently (every subsequent `put` would see corrupt-meta and fail). Recovery is silent at `tracing::warn!`.
5. **Idempotence on identical bytes is the common case.** A correct rebuild that deterministically produces the same bytes (the entire reason caching is sound) MUST be a no-op. Treating it as a conflict would break the cache for any project whose engine ever re-`put`s — i.e., every project.

### 3.3 Threat model (write-side)

Defended:
- **Cross-tenant key collision with bytes drift.** Tenant A's legitimate `put(k, bytes_a)` cannot be silently replaced by tenant B's `put(k, bytes_b)`. B's `put` returns an error; A's bytes remain. The next `get(k)` (from either tenant) still returns `bytes_a`.
- **Re-`put`-on-determinism-bug.** A local build that produces different bytes than a previous build for the same `cloud_key` is a determinism bug; pre-CS-0055 it silently succeeded and the user saw stale-cache symptoms downstream. Post-CS-0055 it surfaces as a `put` error at the moment the bug occurs.

Out of scope (deferred to signed-artifact / SLSA work):
- **Adversary with `delete` access.** An adversary who can call `delete(k)` followed by `put(k, bytes_b)` defeats this primitive — the conflict check sees no existing artifact. Defending against that requires authenticated `delete` (per-tenant ACLs); separate design.
- **Adversary with sidecar-only write access.** An adversary who rewrites only the sidecar's `content_hash` to match `SHA-256(bytes_b)` then calls `put(k, bytes_b)` defeats the conflict check (the existing sidecar now claims to have stored `bytes_b`'s hash). The CS-0054 read-side check catches the inconsistent sidecar-vs-bytes pair on the next `get`, so the artifact still surfaces as a miss — but the legitimate `bytes_a` are gone. Defending against this requires per-tenant signing of the sidecar; separate design.

## 4. Algorithms

### 4.1 `LocalBackend::put` — augmented flow

```
1. mkdir -p path.parent()
2. h_new = SHA-256(bytes)                                  # CS-0054, hoisted to here
3. if path exists:
3a.    read meta_bytes from path.with_extension("meta.json")
3b.    if NotFound:
           warn!("missing sidecar; treating as no prior artifact")
           # fall through to write path (4+)
3c.    elif read fails (other I/O):
           return Err(BackendError::Other("read meta: …"))
3d.    elif parse(meta_bytes) fails:
           warn!("malformed sidecar; treating as no prior artifact")
           # fall through to write path (4+)
3e.    else:
           existing = parsed ArtifactMeta
           if existing.content_hash == h_new:
               return Ok(())                                # idempotent re-put
           else:
               return Err(BackendError::Other(
                   "artifact key conflict at {key_hex}: existing content_hash differs from new bytes"
               ))
4. write bytes atomically (tmp + rename)                    # CS-0054 path, unchanged
5. stamped = ArtifactMeta { content_hash: h_new, ..meta.clone() }
6. write stamped sidecar atomically (meta.json.tmp + rename)
7. Ok(())
```

Step 2 is hoisted from its post-write position (in CS-0054) to pre-flight, so the conflict check has the new bytes' hash ready without a redundant pass.

Step 3 is the new conflict / idempotence pre-flight. The four sub-cases (missing sidecar, I/O error, parse error, hash mismatch) are exhaustive.

### 4.2 Why `path.exists()` and not just attempting the read

A separate existence check is a small race-window optimisation: if the bytes file does not exist, neither does its sidecar (the sidecar is never written first), so we skip two filesystem syscalls in the common write-fresh case. The check is advisory; the subsequent sidecar read is still authoritative — if the bytes file exists but the sidecar does not (the partial-write window CS-0054 §3.2 documents), step 3b handles it.

## 5. Test plan

### 5.1 Unit tests (`cli/crates/cook-cache/src/backend.rs::tests`)

- `put_idempotent_on_same_bytes` — `put(k, bytes)`; `put(k, bytes)` again; second call returns `Ok(())`. Round-trip `get(k)` returns the bytes.
- `put_rejects_conflict` — `put(k, bytes_a)`; `put(k, bytes_b)` with `bytes_a != bytes_b`; second call returns `BackendError::Other` whose message contains `"conflict"` and the key in hex. Round-trip `get(k)` still returns `bytes_a` (prior bytes preserved).
- `put_recovers_from_missing_meta` — `put(k, bytes)`; `rm` the sidecar; `put(k, bytes)` again; second call succeeds. Round-trip `get(k)` returns the bytes (sidecar restored).
- `put_recovers_from_corrupt_meta` — `put(k, bytes)`; overwrite the sidecar with non-JSON garbage; `put(k, bytes)` again; second call succeeds. Round-trip `get(k)` returns the bytes.

### 5.2 Existing tests touched

The pre-CS-0055 test `local_backend_put_idempotent` already passed identical bytes twice; it is preserved as written and continues to pass. No existing test puts conflicting bytes at the same key (the engine's call sites never do this in practice — `cloud_key` collapses identical inputs to identical keys), so no semantic regressions.

### 5.3 End-to-end fixtures

The `examples/cache_benchmarks/verify.sh` (31 scenarios) and `examples/cache_dep_drift/verify.sh` (3 scenarios) exercise the full `put`/`get` round-trip through real builds. Their continued green status is the integration regression bar; in steady-state builds, every `put` is either to a fresh key or an idempotent re-put with identical bytes — both succeed under the new contract.

## 6. Spec amendments

### 6.1 §{exec.cache.integrity}

Append one normative paragraph after the existing read-side fingerprint-verification text at `standard/src/content/docs/08-execution-model.mdx:204`:

> An implementation's artifact-store write path MUST treat a `put` of bytes that conflict with bytes already stored at the same key as an error. "Conflict" means the bytes differ; re-writing identical bytes is idempotent and MUST succeed. The error MUST surface to the caller; the implementation MUST NOT silently overwrite the prior bytes. This requirement is the write-side analogue of the read-side fingerprint check: it ensures that a key in the artifact store maps to one and only one byte sequence over its lifetime, which is the invariant the read-side verification relies upon.

This is a STRICT MUST. The existing flexibility ("MAY use any deterministic fingerprint that meets the observability constraint") is unaffected — the conflict check uses whatever fingerprint the implementation already uses for the read-side verification.

### 6.2 No §9 schema change

CS-0055 introduces no user-visible configuration surface. The idempotency contract is internal to the backend implementation; users do not opt in or out.

## 7. Backwards compatibility

- **On-disk cache:** `CACHE_VERSION` unchanged. Existing CS-0054 sidecars contain the `content_hash` the new code path needs. Pre-CS-0054 sidecars deserialize with `content_hash = [0; 32]`; on a re-`put` with non-empty bytes, the new bytes' SHA-256 will not match the zero sentinel, so the conflict check fires and the build sees a `BackendError::Other`. To avoid this regression, the new implementation treats a sidecar whose `content_hash` is the zero sentinel as if no prior artifact existed (write through). **Note:** CS-0054 already invalidates pre-CS-0054 sidecars on the read side (they fail the integrity check and surface as misses), so a CS-0055-conformant implementation that lands after CS-0054 sees no pre-CS-0054 sidecars in steady state — the orphans were already swept by the read-side miss. The zero-sentinel write-through is a belt-and-braces guard for the upgrade boundary.
- **Backend trait:** `CacheBackend::put` signature unchanged; the *contract* tightens. This is a documented breaking change for any out-of-tree implementer that previously overwrote silently. The in-tree `LocalBackend` is the only implementer today.
- **Cookfile grammar:** unchanged.
- **Cook Lua API:** unchanged.
- **Configuration:** unchanged.

## 8. Open questions

1. **Should the conflict error carry a structured payload (existing hash, new hash, key) instead of a `String`?** A `BackendError::Conflict { key, existing_hash, new_hash }` variant would let upstream code surface conflict telemetry without parsing the diagnostic message. Recommendation: defer; the message-with-substrings test pattern is sufficient for v1, and adding a new variant is a strict refinement when SaaS telemetry needs it.
2. **Should the conflict check also compare `meta` fields beyond `content_hash`?** Two `put`s with identical bytes but different `tags` or `consulted_env_keys` are currently treated as idempotent (the second `put` is a no-op, the first sidecar's metadata wins). Strictly the bytes are what callers care about; the metadata is diagnostic. Recommendation: keep the bytes-only check. If metadata divergence ever matters, it's a separate "MERGE on re-put" semantics design — not in scope here.
3. **Should the implementation compute `h_new` lazily (only after `path.exists()` returns true)?** The current §4.1 hoists step 2 to pre-flight unconditionally. For a fresh-key `put` (the common case) this costs one SHA-256 pass; the same pass would otherwise happen post-write for CS-0054's stamping. Net cost is zero; complexity is lower with the unconditional compute. Recommendation: keep the unconditional compute.

## Appendix A. Why write-side enforcement, not engine-side dedup

A simpler shape is: leave `put` overwriting silently and have the engine refuse to call `put` with conflicting bytes by tracking an in-memory "already-put under this key" set. The tradeoffs:

- **Pro (alternate):** trait surface unchanged in semantics; engine has authoritative knowledge of what it has put.
- **Con (alternate, 1):** the in-memory set is per-process. Two cook processes on the same machine racing on the same key would not see each other's puts; the second would overwrite without warning.
- **Con (alternate, 2):** the in-memory set is per-tenant. The cross-tenant case — the *whole* threat model this spec exists for — is not defended.
- **Con (alternate, 3):** the engine is not the only writer of the artifact store on a SaaS backend. Future tools (a cache-warming service, an admin re-uploader, an evict-and-restore orchestrator) all write through the trait; the contract MUST be enforced at the trait, not at one of its callers.

Backend-side enforcement keeps the responsibility where it belongs (the layer that owns the persistence boundary), defends the cross-tenant case by construction, and reuses the SHA-256 primitive CS-0054 already established. The same in-memory dedup remains available as an engine-side optimisation — if the engine knows it has just `put` bytes under `k`, it can skip a redundant `put` call entirely — but it is an optimisation, not a soundness primitive.
