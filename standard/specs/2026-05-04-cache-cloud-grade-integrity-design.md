# Design: Cloud-grade content integrity for the cache backend

**Date:** 2026-05-04
**Status:** Design — pending implementation plan
**Standard change ID:** CS-0054 (assigned at PR time; CS-0053 is reserved)
**Linear epic:** SHI-24 cache cloud-readiness — *content integrity primitive for the shared backend*
**Predecessors:**
  - [2026-05-01-cache-cloud-readiness-design.md](./2026-05-01-cache-cloud-readiness-design.md) (introduced the `CacheBackend` trait, `cloud_key`, and the local-only artifact store)
  - [2026-05-02-cache-restore-and-dep-inputs-design.md](./2026-05-02-cache-restore-and-dep-inputs-design.md) (introduced the multi-output `ArtifactMeta` sidecar)
  - [2026-05-04-cache-declared-tools-design.md](./2026-05-04-cache-declared-tools-design.md) (CS-0052)
**Scope:** `cli/crates/cook-fingerprint/src/backend.rs` (`ArtifactMeta`, `CacheBackend` trait), `cli/crates/cook-cache/src/backend.rs` (`LocalBackend::put` / `LocalBackend::get`), `cli/crates/cook-engine/src/executor.rs` (caller-side struct construction), and an amendment to Cook Standard §{exec.cache.integrity}.

## 1. Motivation

Cook's existing cache integrity story has two layers. At the CAS-write level the engine records `output.hash = xxh3_64(bytes)` in `StepEntry`; at the restore-on-hit path (`cli/crates/cook-fingerprint/src/check.rs:362-365`) it recomputes `xxh3_64(bytes_from_backend)` and refuses to install bytes whose hash drifted. The Standard §{exec.cache.integrity} (`standard/src/content/docs/08-execution-model.mdx:201-206`) mandates that integrity check on every restore but leaves the fingerprint function implementation-defined ("MAY use any deterministic fingerprint that meets the observability constraint").

For a single-machine cache, xxh3_64 is the right choice: hot-path cheap, deterministic, sound against accidental drift. For the multi-tenant SaaS backend SHI-24 will land, it is not. xxh3_64 is non-cryptographic — a 64-bit collision is ~2³² work under birthday bounds, well within reach for an adversary who can write to the shared bucket. A backend with bytes-only write access can craft an alternate payload that hashes to the same value as a victim's legitimate artifact and, on the next restore, the engine will install the attacker's bytes under a cache hit. The 2026-05-01 design lockstepped the trait surface; it did not lock in a cryptographic primitive because the local backend did not need one.

This spec adds a backend-self-verifying SHA-256 integrity primitive: the bytes returned by `CacheBackend::get` MUST be byte-identical to the bytes most recently `put` under that key, or `get` MUST return `None` (treat as miss, fail closed). The check is the backend's responsibility, not the engine's, so the trait contract holds equally for `LocalBackend` and any future `CloudBackend`. This complements — rather than replaces — the existing xxh3_64 check in `check.rs`: that check guards the engine→backend value contract (the bytes match the recorded `output.hash`), this check guards the backend's internal storage contract (what came out is what was put in).

## 2. Non-goals

- **Signed / authenticated artifacts.** An adversary that consistently rewrites both bytes and the meta-sidecar's `content_hash` field defeats this primitive — the SHA-256 is over bytes, not over a tenant-scoped attestation. Defending against that requires a signing root (per-tenant key, MAC, or in-toto-style attestation) and is explicitly deferred to the SLSA / signed-artifact track. CS-0054 is the bytes-only-tampering layer.
- **Cryptographic protection of the cache key itself.** `cloud_key` is already SHA-256 (`cli/crates/cook-fingerprint/src/backend.rs:115-126`); the gap was the *artifact bytes*, not the key.
- **xxh3_64 removal.** The engine-level `output.hash` in `StepEntry` (`cli/crates/cook-fingerprint/src/check.rs:362-365`) stays as-is — it is hot-path, runs against trusted local-build bytes, and is the right tool for that job. CS-0054 adds a *second* layer at the backend boundary, not a replacement.
- **Trait shape change.** `CacheBackend::get` keeps its signature (`fn get(&self, key: &CloudKey) -> BackendResult<Option<Vec<u8>>>`). The verification is internal to the implementation; tampered or unverifiable bytes surface as `Ok(None)`.
- **Streaming / chunked artifacts.** SHA-256 is over the whole byte slice in one shot. A future streaming variant is a separate design (it will need to define chunk framing and Merkle root semantics).

## 3. Architecture

### 3.1 Modules touched

```
cli/crates/
├── cook-fingerprint/
│   └── backend.rs            ArtifactMeta gains `content_hash: [u8; 32]`;
│                             CacheBackend::get / ::put trait docs amend
│                             the contract.
├── cook-cache/
│   └── backend.rs            LocalBackend::put computes SHA-256 and
│                             stamps content_hash before sidecar write;
│                             LocalBackend::get verifies before returning.
└── cook-engine/
    └── executor.rs           Caller sites (4) initialise content_hash
                              with the zero sentinel — `put` overwrites.

standard/
└── src/content/docs/08-execution-model.mdx     §8.6.1 amendment
```

No grammar change. No Lua API change. No on-disk schema-version bump (`CACHE_VERSION` unchanged); the sidecar field is `#[serde(default)]` so pre-CS-0054 sidecars deserialize with `content_hash = [0; 32]` and fail the integrity check on first read — a one-shot rebuild for any project that upgrades across the boundary, identical to the orphan behaviour CS-0052 §4.4 documented.

### 3.2 Architectural invariants preserved

1. **Trait shape stable.** `CacheBackend::get` returns `Option<Vec<u8>>`. A future cloud backend implements the same SHA-256 self-verify; the trait does not need to learn about hashes.
2. **Single source of truth on `content_hash`.** `put` is the *only* writer of `content_hash`. Callers pass the zero sentinel; `put` overwrites before persisting the sidecar. No call site computes SHA-256 on its own — that would create a "compute hash twice, hope they match" failure mode.
3. **Fail closed, not loud.** A mismatch is logged at `warn!` and returned as `Ok(None)`. The engine treats it as a miss and falls through to rebuild — same shape as a missing entry. The Standard §{exec.cache.integrity} is explicit that a corrupt restore MUST be observationally indistinguishable from "didn't have it"; CS-0054 honours that contract.
4. **Atomic sidecar writes.** The existing tmp+rename pattern for the bytes is extended to the sidecar: a partially written `meta.json` cannot coexist with fully-written bytes. Without this, a concurrent crash between bytes-write and sidecar-write would leave a permanent fail-closed entry that masquerades as tampered.

### 3.3 Threat model

Defended:
- **Bit-flip / disk corruption (local).** The sidecar's recorded SHA-256 won't match recomputed bytes; restore returns `None`.
- **Network corruption (shared backend).** Same.
- **Bytes-only tampering on a shared backend.** An adversary with write access to the byte object but no equivalent control over the meta sidecar (different bucket / different ACL / different replication path / different write window) flips bytes; the recorded SHA-256 won't match; restore returns `None`.
- **Bytes-only collision attack.** SHA-256 is collision-resistant under adversarial input (`>= 2^128` work for a second-preimage). xxh3_64's `~2^32` ceiling is closed.

Out of scope (deferred to SLSA / signed-artifact work):
- **Bytes + meta consistent rewrite.** An adversary who rewrites both `bytes` and `meta.content_hash` to a self-consistent pair defeats CS-0054. The defence is per-tenant signing of the meta — separate design.
- **Cache key forgery.** Already covered by SHA-256 in `cloud_key`; not in scope here.
- **Confidentiality.** Bytes are not encrypted; CS-0054 is about integrity, not secrecy.

## 4. Data structures

### 4.1 `ArtifactMeta` extension

`cli/crates/cook-fingerprint/src/backend.rs` (around line 43-57):

```rust
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ArtifactMeta {
    // … existing fields unchanged …
    /// SHA-256 of the artifact bytes. Computed and stamped by the backend
    /// in `CacheBackend::put`; verified against the on-disk bytes by
    /// `CacheBackend::get`. Callers SHOULD pass the all-zero sentinel
    /// `[0u8; 32]` at construction time — `put` overwrites it before
    /// persisting the sidecar.
    #[serde(default = "ArtifactMeta::zero_content_hash")]
    pub content_hash: [u8; 32],
}

impl ArtifactMeta {
    pub fn zero_content_hash() -> [u8; 32] { [0u8; 32] }
}
```

Why a fixed `[u8; 32]`, not `Vec<u8>` or an enum: SHA-256 is the contract. A future `Sha512` or `Blake3` variant is a separate trait-level decision and is not the right shape to bake into this metadata field. If the primitive ever needs to evolve, `CACHE_VERSION` bumps and the new field replaces this one — exactly the same evolution path the rest of the cache schema uses.

### 4.2 `CacheBackend` trait contract

`cli/crates/cook-fingerprint/src/backend.rs` (around line 64-69):

```rust
pub trait CacheBackend: Send + Sync {
    /// Fetch artifact bytes. Returns Ok(None) on miss (NOT an error).
    ///
    /// Implementations MUST self-verify content integrity before returning
    /// bytes. The bytes returned MUST be byte-identical to the bytes most
    /// recently `put` under this key — otherwise `Ok(None)` MUST be returned
    /// (fail closed). The reference contract is SHA-256 of bytes-on-disk
    /// equal to `ArtifactMeta::content_hash`.
    fn get(&self, key: &CloudKey) -> BackendResult<Option<Vec<u8>>>;

    /// Upload artifact bytes with metadata.
    ///
    /// Implementations MUST stamp `meta.content_hash` with the SHA-256 of
    /// `bytes` before persisting the sidecar; callers pass the zero sentinel
    /// and treat the overwrite as authoritative.
    fn put(&self, key: &CloudKey, bytes: &[u8], meta: &ArtifactMeta) -> BackendResult<()>;
}
```

## 5. Algorithms

### 5.1 `LocalBackend::put`

```
1. mkdir -p path.parent()
2. write bytes to path.with_extension("tmp"); fsync-equivalent rename → path
3. h = SHA-256(bytes)
4. stamped = ArtifactMeta { content_hash: h, ..meta.clone() }
5. write serde_json(stamped) to path.with_extension("meta.json.tmp")
6. rename → path.with_extension("meta.json")
```

Step 6 is new. Pre-CS-0054 the sidecar was a single `fs::write` — fine for the single-writer case but vulnerable to mid-write crashes producing a half-readable sidecar that subsequently looks like a tampered entry. The atomic rename keeps the integrity check decisive: the sidecar exists fully or not at all.

### 5.2 `LocalBackend::get`

```
1. read bytes from path. NotFound → Ok(None) (cache miss).
2. read meta from path.with_extension("meta.json").
   NotFound → warn! + Ok(None) (no integrity proof; fail closed).
3. parse meta. Malformed → warn! + Ok(None) (fail closed).
4. actual = SHA-256(bytes)
5. if actual != meta.content_hash:
       warn!(…) + Ok(None) (tamper or drift; fail closed).
6. Ok(Some(bytes))
```

Two design decisions worth pinning explicitly:

- **Missing sidecar is fail-closed, not "trust-on-first-use".** Without a recorded hash there is no integrity proof; we MUST NOT install the bytes. The next build re-`put`s the bytes (with a fresh sidecar) and the cache repopulates. This is the same fall-through the engine already takes for `Ok(None)`.
- **Malformed sidecar is fail-closed, not error.** A `BackendError::Other` would propagate up and the engine would treat the *whole* backend as unhealthy for the rest of the build (the trait contract for transient errors). A single corrupt entry shouldn't disable the cache; we surface it as a miss and let the engine rebuild.

## 6. Test plan

### 6.1 Unit tests (`cli/crates/cook-cache/src/backend.rs::tests`)

- `put_computes_sha256_in_meta` — round-trip put, read sidecar, assert `content_hash` equals an independent SHA-256 of the bytes; assert it is not the zero sentinel.
- `get_succeeds_when_bytes_match_meta` — round-trip put/get returns `Some` with byte-identical bytes.
- `get_fails_closed_on_byte_tamper` — put bytes; mutate the on-disk bytes file (sidecar untouched); assert `get` returns `None`.
- `get_fails_closed_on_meta_tamper` — put bytes; rewrite the sidecar's `content_hash` to a different value; assert `get` returns `None`.
- `get_fails_closed_on_missing_meta` — put bytes; `rm` the sidecar; assert `get` returns `None` (no integrity proof).

### 6.2 Existing tests touched

Every `ArtifactMeta { … }` literal in the codebase gains `content_hash: ArtifactMeta::zero_content_hash()`. The five integration tests under `cli/crates/cook-cache/tests/` (`integration_config_toggle`, `integration_multi_output_restore`, `integration_discovered_inputs_restore`, `integration_first_build_depfile`, `integration_restore_on_hit`) and the four executor.rs construction sites (around lines 1150 / 1180 / 1406 / 1436) are mechanical updates — no semantic change. They continue to pass because the backend overwrites whatever the test passes.

### 6.3 End-to-end fixtures

The `examples/cache_benchmarks/verify.sh` (31 scenarios) and `examples/cache_dep_drift/verify.sh` (3 scenarios) exercise the full `put`/`get` round-trip through real builds; their continued green status is the integration regression bar for CS-0054.

## 7. Spec amendments

### 7.1 §{exec.cache.integrity}

Append one normative sentence to the existing "MAY use any deterministic fingerprint" paragraph at `standard/src/content/docs/08-execution-model.mdx:204`:

> An implementation that exposes the artifact store to writers other than the local build (e.g., a multi-tenant shared backend) SHOULD use a cryptographically secure fingerprint (collision-resistant under adversarial input).

This is a SHOULD, not a MUST: a trusted-environment backend (e.g., a CI-only artifact store inside a single-tenant network with no untrusted writers) can legitimately keep using xxh3_64. The MUST in the existing paragraph already mandates *some* function-of-bytes check; this amendment narrows the choice for the threat model that motivates Cook Cloud.

### 7.2 No §9 (`.cook/cloud.toml` schema) change

CS-0054 introduces no user-visible configuration surface. The integrity primitive is internal to the backend implementation; users do not opt in or out.

## 8. Backwards compatibility

- **On-disk cache:** `CACHE_VERSION` unchanged. Pre-CS-0054 sidecars lack the `content_hash` field; serde defaults it to `[0; 32]` and the SHA-256 of any non-empty byte payload will mismatch on the first read — pre-CS-0054 entries surface as misses and rebuild on first access. Eviction reaps them on its own schedule. Identical orphan-on-upgrade behaviour to CS-0052.
- **Backend trait:** `CacheBackend::get` and `CacheBackend::put` signatures unchanged; the *contract* tightens (implementations MUST self-verify, MUST stamp content_hash). This is a documented breaking change for any out-of-tree implementer; the in-tree `LocalBackend` is the only implementer today.
- **Cookfile grammar:** unchanged.
- **Cook Lua API:** unchanged.
- **Configuration:** unchanged.

## 9. Open questions

1. **Should `get` differentiate "tamper detected" from "miss" in its return type?** Currently both surface as `Ok(None)` so the engine's miss-handling path Just Works. A `Result<RestoreOutcome, _>` enum (with `Miss`, `Tampered { recorded, actual }`, `Hit(bytes)` variants) would let the engine surface tamper events to telemetry separately from organic misses — useful for SaaS observability. Recommendation: defer; the `tracing::warn!` line + telemetry scrape covers the SaaS case until we have a cloud-backend signal worth differentiating.
2. **Is `[u8; 32]` the right shape, or should it be a `ContentHash` newtype?** A newtype prevents accidental mixing with other 32-byte arrays (`CloudKey`) and would be the cleaner long-run choice. Recommendation: defer to a follow-up; the in-tree shape is tight enough that the type confusion isn't a real risk yet.
3. **Should the sidecar be Merkle-tree'd for streaming / chunked artifacts?** Out of scope here (§2); flagged as a future evolution path so a v2 design has a referenced precedent.

## Appendix A. Why backend-self-verify, not engine-side verify

A simpler shape would be: extend the `CacheBackend::get` return type to `(bytes, content_hash)` and have the engine compute SHA-256 itself, comparing in `check.rs`. The tradeoffs:

- **Pro (alternate):** the engine sees the recorded hash directly and can fold it into telemetry / `events.jsonl`.
- **Con (alternate, 1):** every backend implementation now needs to expose the recorded hash through the wire format. The cloud backend's HTTP response would have to carry it as a header, breaking byte-for-byte compatibility with R2's standard GET response.
- **Con (alternate, 2):** the engine ends up with two integrity checks (the one already in `check.rs` against `output.hash`, plus a new one against `content_hash`) — the duplication invites drift.
- **Con (alternate, 3):** a buggy backend implementation that returned the wrong hash would cause silent acceptance of tampered bytes; the responsibility is misplaced.

Backend-self-verify keeps the trait surface minimal, keeps the responsibility where it belongs (the layer that knows where the bytes came from), and lets each backend choose its own verification strategy if the SHA-256 default doesn't fit (e.g., a future TPM-attested backend can verify via attestation chain instead of content hash, while keeping the same trait contract).
