# Design: Streaming `CacheBackend` trait via `Box<dyn Read + Send>`

**Date:** 2026-05-04
**Status:** Design — pending implementation plan
**Standard change ID:** CS-0056 (assigned at PR time; CS-0053 reserved per CS-0054 §preamble)
**Linear epic:** SHI-24 cache cloud-readiness — *I/O-shape primitive for the shared backend*
**Predecessors:**
  - [2026-05-01-cache-cloud-readiness-design.md](./2026-05-01-cache-cloud-readiness-design.md) (introduced the `CacheBackend` trait, `cloud_key`, and the local-only artifact store)
  - [2026-05-04-cache-cloud-grade-integrity-design.md](./2026-05-04-cache-cloud-grade-integrity-design.md) (CS-0054 — SHA-256 `content_hash` integrity primitive this spec re-uses)
  - [2026-05-04-cache-backend-idempotency-design.md](./2026-05-04-cache-backend-idempotency-design.md) (CS-0055 — write-side conflict-detection contract this spec preserves)
**Scope:** `cli/crates/cook-fingerprint/src/backend.rs` (`CacheBackend::get` / `CacheBackend::put` signatures and trait-doc contracts), `cli/crates/cook-cache/src/backend.rs` (`LocalBackend::get` / `LocalBackend::put`, plus a new `VerifyingReader<R>` helper and `get_bytes` / `put_bytes` convenience free functions), `cli/crates/cook-fingerprint/src/check.rs` and `cli/crates/cook-engine/src/executor.rs` (caller-side rewrites), and an amendment to Cook Standard §{exec.cache.integrity}.

## 1. Motivation

The 2026-05-01 design lockstepped the `CacheBackend` trait surface so the future `CloudBackend` could ship without re-litigating the engine-side seam. CS-0054 hardened the integrity primitive (SHA-256 `content_hash`); CS-0055 hardened the write-side soundness (conflict rejection). Both kept the trait's I/O shape pinned at `Vec<u8>` / `&[u8]`:

```rust
fn get(&self, key: &CloudKey) -> BackendResult<Option<Vec<u8>>>;
fn put(&self, key: &CloudKey, bytes: &[u8], meta: &ArtifactMeta) -> BackendResult<()>;
```

This shape is the third soundness gap in the cloud-readiness sequence — but the surface this time is *liveness*, not integrity. Every artifact, on `get` and on `put`, is materialised into memory in full. For the typical 10–100 KB object file this is fine; for multi-GB artifacts (linked binaries, debug-info-rich shared objects, archive bundles, container layers in CI workflows) it OOMs the build. Worse, the `Vec<u8>` shape silently encodes "all bytes resident before any verification" into the contract — there is no way for an implementer to hash-as-they-stream from disk on the local backend, or hash-as-they-stream from an HTTP body on the cloud backend, without violating the trait. The HTTP-body case is the one that matters: every modern content-addressable backend (R2, S3, GCS, Cloudflare KV) hands you a streaming reader. Buffering the whole body before handing it to the engine throws away the natural shape of the wire protocol, and on a multi-GB artifact OOMs the worker.

This spec replaces the trait's I/O shape with `Box<dyn Read + Send>` / `&mut dyn Read`. The integrity primitive (SHA-256 self-verify on `get`, SHA-256 stamp on `put`) is preserved by surfacing verification *as bytes flow*: the local backend wraps `File` reads in a `VerifyingReader<R>` that tees bytes through a hasher and raises `io::Error(InvalidData)` at EOF on mismatch, which is the streaming equivalent of CS-0054's in-memory check. The CS-0055 conflict-detection contract is preserved by streaming the new bytes into a tmp file (computing hash on the fly), comparing against any existing sidecar's `content_hash`, and only renaming-into-place if the conflict check passes. Two free-function helpers (`get_bytes`, `put_bytes`) wrap the streaming surface with the old `Vec<u8>` / `&[u8]` ergonomics for callers that already have bytes in hand and don't benefit from streaming.

## 2. Non-goals

- **Chunked / resumable transfer protocol.** SHA-256 remains over the whole byte sequence in one shot; the verifier hashes incrementally but only commits at EOF. A future Merkle-tree variant for partial-restore semantics is a separate design.
- **Backpressure / async streaming.** The trait stays synchronous (`std::io::Read`). The cloud backend will need an async wrapper internally (R2 SDKs are async), but the engine-facing trait stays sync — the cloud backend will block on a runtime handle internally, same shape as today's `reqwest::blocking`.
- **Length-aware put.** `put` does not require the caller to declare a content-length up front. The streamed length is computed implicitly; `meta.size_bytes` remains caller-set (the caller knows the source size before streaming). A future "streaming put with declared length and progress callback" wrapper could layer on top, but the core trait stays minimal.
- **Bytes-and-meta consistent rewrite defence.** Out of scope (deferred to signed-artifact / SLSA work — same boundary CS-0054 §2 establishes).
- **Helper-as-trait-default-method.** The free-function `get_bytes` / `put_bytes` helpers are not provided as default trait methods because object-safe traits cannot have generic helpers, and free functions over `&dyn CacheBackend` keep the call site identical.
- **`size_bytes` recomputation on streaming put.** The implementation observes the streamed total, but does not overwrite a caller-set `meta.size_bytes`. Callers who don't know the length up front can pre-set `0` and re-fetch the sidecar after the put if observability matters; the bytes-vs-sidecar contract is unaffected (`size_bytes` is diagnostic, not part of the integrity invariant).

## 3. Architecture

### 3.1 Modules touched

```
cli/crates/
├── cook-fingerprint/
│   ├── backend.rs           CacheBackend::get returns Box<dyn Read + Send>;
│   │                        CacheBackend::put takes &mut dyn Read +
│   │                        &mut ArtifactMeta. Trait docs amend the
│   │                        contract to allow streaming verification
│   │                        and require in-place content_hash stamping.
│   └── check.rs             try_restore drains the streaming reader via
│                            read_to_end; the existing xxh3_64 outer
│                            check is preserved.
├── cook-cache/
│   └── backend.rs           VerifyingReader<R> teeing-hasher wrapper;
│                            get_bytes / put_bytes convenience free fns;
│                            LocalBackend::get returns a VerifyingReader-
│                            wrapped File; LocalBackend::put streams to
│                            tmp + hash, then commits via tmp-rename
│                            after the CS-0055 conflict check.
└── cook-engine/
    └── executor.rs          Four put-call-sites switched to put_bytes;
                             ArtifactMeta bound as `mut` so put can stamp.

standard/
└── src/content/docs/08-execution-model.mdx    §8.6.1 amendment
```

No grammar change. No Lua API change. No on-disk schema change. No `CACHE_VERSION` bump. The on-disk byte and sidecar formats are byte-identical to CS-0055; only the in-memory I/O shape changes.

### 3.2 Architectural invariants preserved

1. **Bytes-on-disk identity.** A `put`/`get` round-trip MUST produce byte-identical bytes. The streaming verifier surfaces tampering as an EOF `InvalidData` error; the bytes-as-streamed remain identical to bytes-as-written.
2. **CS-0054 self-verify is kept.** The verification still happens at the backend boundary. The shape is now "as bytes flow + finalize at EOF" instead of "buffer then compare", but the property is identical.
3. **CS-0055 conflict detection is kept.** A `put` to an occupied key with bytes that hash differently MUST refuse with `BackendError::Other` and a diagnostic naming the key in hex. The implementation streams to a tmp file first, computes the hash on EOF, then compares to the existing sidecar's `content_hash` before renaming-into-place; on conflict the tmp file is discarded.
4. **Single source of truth on `content_hash`.** `put` is still the only writer of `content_hash`. The contract additionally:
   - Stamps `meta.content_hash` in-place (via `&mut ArtifactMeta`) so the caller observes the canonical hash without re-reading the sidecar.
   - Honours a non-zero caller-supplied `content_hash` only when it matches the streamed bytes' hash; a mismatch is a caller-bug and surfaces as `BackendError::Other`.
5. **Fail-closed orphan-on-upgrade.** A sidecar whose `content_hash` is the zero sentinel is still a pre-CS-0054 orphan. The streaming `get` checks the sentinel *before* opening the bytes file and returns `Ok(None)` without ever exposing a verifier — same fail-closed semantics, just earlier in the path.

### 3.3 Threat model (delta vs. CS-0054 / CS-0055)

CS-0056 does not add a new threat-model category; it preserves CS-0054's bytes-only-tampering defence and CS-0055's cross-tenant-collision defence under a streaming I/O shape. The only new failure mode is "tampering surfaces at EOF rather than synchronously after a `read`" — which is a property of the verifier, not a soundness gap. A caller that reads partial bytes from the verifier and acts on them before EOF would see attacker-controlled bytes; the engine's restore path (and the `get_bytes` helper) drains the full reader before installing anything, so this is a discipline a caller MUST honour but is not a new vulnerability — pre-CS-0056, the same caller would have received the bytes in a `Vec<u8>` and acted on them before any verification. The streaming shape *adds* the option to verify-as-you-go; it does not remove the existing buffer-then-verify path (`get_bytes` is exactly that path, layered on top).

## 4. Data structures

### 4.1 `VerifyingReader<R>`

```rust
pub struct VerifyingReader<R: Read> {
    inner: R,
    hasher: Sha256,
    expected: [u8; 32],
    done: bool,
}

impl<R: Read> Read for VerifyingReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.done { return Ok(0); }
        let n = self.inner.read(buf)?;
        if n == 0 {
            self.done = true;
            let actual: [u8; 32] = std::mem::replace(&mut self.hasher, Sha256::new()).finalize().into();
            if actual != self.expected {
                return Err(io::Error::new(io::ErrorKind::InvalidData, "..."));
            }
            return Ok(0);
        }
        self.hasher.update(&buf[..n]);
        Ok(n)
    }
}
```

Lives in `cli/crates/cook-cache/src/backend.rs` next to `LocalBackend` (the only consumer today; the future `CloudBackend` will pull it from a shared spot or duplicate it — duplication is acceptable, the wrapper is ~30 lines). Generic over `R: Read` so callers can wrap a `File`, an HTTP-body reader, a `Cursor<Vec<u8>>`, etc., with no allocation in the hot path beyond the per-instance `Sha256` state.

The `done` field is a finalization-state guard. `Sha256::finalize()` consumes self, so we must call it exactly once; the EOF read replaces the hasher with a fresh-but-unused one to keep the struct in a valid state for any subsequent (defensive-caller) read calls — those return `Ok(0)` rather than re-finalizing. The mismatch error is raised on the EOF read; defensive callers that keep reading after an error would not see the error a second time, which is acceptable because the contract is "drain the reader; if anywhere it errors, treat as miss" and the caller must surface the first error encountered.

### 4.2 `get_bytes` / `put_bytes` free functions

```rust
pub fn get_bytes(backend: &dyn CacheBackend, key: &CloudKey) -> BackendResult<Option<Vec<u8>>> {
    let Some(mut reader) = backend.get(key)? else { return Ok(None); };
    let mut bytes = Vec::new();
    match reader.read_to_end(&mut bytes) {
        Ok(_) => Ok(Some(bytes)),
        Err(e) if e.kind() == io::ErrorKind::InvalidData => Ok(None),
        Err(e) => Err(BackendError::Other(format!("read streaming body: {e}"))),
    }
}

pub fn put_bytes(backend: &dyn CacheBackend, key: &CloudKey, bytes: &[u8], meta: &mut ArtifactMeta) -> BackendResult<()> {
    let mut cursor = Cursor::new(bytes);
    backend.put(key, &mut cursor, meta)
}
```

These exist because most engine call sites already have bytes in hand (the engine just `read`s the workspace output file into a `Vec<u8>` before uploading) and don't benefit from the streaming surface. Forcing every call site to construct a `Cursor` would be churn-without-benefit; the helpers are the migration-friendly compatibility layer. They live in `cook-cache/src/backend.rs` next to `LocalBackend` and are public.

The `get_bytes` helper folds the streaming verifier's `InvalidData` error back into `Ok(None)` — the helper is the buffer-then-verify shape, which is the shape pre-CS-0056 callers expected. Streaming callers that want the raw error should use `backend.get` directly.

## 5. Algorithms

### 5.1 `LocalBackend::get` — streaming flow

```
1. read sidecar bytes from path.with_extension("meta.json")
1a.    NotFound → Ok(None) [miss; partial-write recovery handled here]
1b.    other I/O err → Err(BackendError::Other)
1c.    parse fails → Ok(None) [malformed; fail closed]
1d.    parsed.content_hash == zero sentinel → Ok(None) [pre-CS-0054 orphan]
2. open bytes file at path
2a.    NotFound → Ok(None) [sidecar without bytes; partial replication]
2b.    other I/O err → Err(BackendError::Other)
3. return Ok(Some(Box::new(VerifyingReader::new(file, parsed.content_hash))))
```

The sidecar is read fully (it's small; the sidecar size is bounded by the meta struct, not the artifact size). The bytes file is *not* read up front — the `File` handle is wrapped in `VerifyingReader` and handed to the caller. Verification finalizes when the caller drains the reader to EOF.

### 5.2 `LocalBackend::put` — streaming flow

```
1. mkdir -p path.parent()
2. tmp = path.with_extension("tmp"); create File at tmp
3. loop: read up to 64KiB from `reader`, hash + write to tmp, accumulate total
4. on EOF: flush + drop tmp_file; finalize hasher → computed: [u8; 32]
5. caller-claimed-hash check:
   if meta.content_hash != zero && meta.content_hash != computed:
       remove tmp; Err("caller-claimed content_hash differs from streamed bytes")
6. CS-0055 conflict check (only if path exists):
   read meta_path; parse; compare existing.content_hash to computed:
     existing == computed                → remove tmp; stamp meta.content_hash; Ok(())
     existing != computed (and non-zero) → remove tmp; Err("artifact key conflict at <key_hex>: …")
     existing == zero sentinel           → fall through to write path (pre-CS-0054 entry)
     missing/malformed/non-NotFound err  → fall through to write path
7. rename tmp → path (atomic commit of bytes)
8. stamp meta.content_hash = computed; serialize; write meta.json.tmp + rename → meta.json
9. Ok(())
```

Step 3 is the streaming-equivalent of CS-0054's `Sha256::digest(bytes)` post-write. Step 4 finalizes the hash exactly once. Step 6 is unchanged in semantics from CS-0055; only the sequencing differs (the new bytes are in a tmp file, not in memory; the existing-meta read happens after the tmp write). Step 7 is the atomic commit — until rename, no reader can observe the new bytes.

The 64KiB buffer is a typical I/O block size; this is the only allocation in the hot path beyond the `Sha256` state. For a multi-GB artifact, total resident memory is `64KiB + sizeof(Sha256)` ≈ 64KiB — the OOM path is closed.

### 5.3 Why stream-to-tmp before the conflict check, not the other way around

The natural alternative is "read existing sidecar first, get `existing.content_hash`, then stream the new bytes only if existing is consistent". But this requires either (a) a streaming hash without writing — which means we'd have to stream-then-discard-then-stream-again (the caller's `&mut dyn Read` is a one-shot; we can't rewind it); or (b) buffering the new bytes in memory — defeating the entire point of CS-0056. Streaming-to-tmp first costs one O(n) write that gets discarded on conflict; conflict is the rare path (a determinism bug or a poisoned-toolchain cross-tenant attempt), so the cost is acceptable. The fresh `put`-to-empty-key case (the common case) writes once and renames; cost is one rename beyond CS-0054.

## 6. Test plan

### 6.1 Unit tests (`cli/crates/cook-cache/src/backend.rs::tests`)

New, CS-0056-specific:
- `verifying_reader_passes_through_on_match` — feed bytes whose SHA-256 matches `expected`; `read_to_end` returns Ok with all bytes.
- `verifying_reader_errors_on_mismatch` — feed bytes whose SHA-256 differs from `expected`; `read_to_end` returns `Err(InvalidData)`.
- `verifying_reader_passes_through_on_empty_match` — empty bytes vs. SHA-256 of empty; `read_to_end` returns Ok with zero bytes.
- `local_backend_get_returns_streaming_reader` — put 10 KiB; `get` returns a reader; `read_to_end` produces those bytes.
- `local_backend_get_streaming_errors_on_byte_tamper` — put bytes; mutate `.bin` on disk; `read_to_end` on the returned reader returns `InvalidData`.
- `local_backend_put_streams_with_zero_sentinel` — put with `content_hash = [0;32]`; after put, `meta.content_hash` is the SHA-256 of the bytes (in-place stamp).
- `local_backend_put_rejects_caller_hash_mismatch` — put with a non-zero `content_hash` that doesn't match; `BackendError::Other` mentioning "caller-claimed content_hash"; no artifact persisted.
- `local_backend_put_accepts_caller_hash_match` — put with a non-zero `content_hash` that DOES match; succeeds.
- `put_bytes_helper_round_trip` — convenience helper round-trips `Vec<u8>`.
- `get_bytes_helper_round_trip` — convenience helper round-trips bytes; on tamper, returns `Ok(None)` (helper folds `InvalidData` into miss).

Existing CS-0054 / CS-0055 tests preserved with semantic coverage intact, just rewritten against the streaming surface (most via the `put_bytes` / `get_bytes` helpers): `put_computes_sha256_in_meta`, `get_succeeds_when_bytes_match_meta`, `get_fails_closed_on_byte_tamper`, `get_fails_closed_on_meta_tamper`, `get_fails_closed_on_missing_meta`, `put_idempotent_on_same_bytes`, `put_rejects_conflict`, `put_recovers_from_missing_meta`, `put_recovers_from_corrupt_meta`.

### 6.2 Integration tests touched

`integration_first_build_depfile.rs`, `integration_config_toggle.rs`, `integration_multi_output_restore.rs`, `integration_discovered_inputs_restore.rs`, `integration_restore_on_hit.rs` — all switched to `put_bytes` / `get_bytes` helpers; their semantic coverage (cross-machine pull, config-toggle preservation, multi-output restore, depfile-as-implicit-output, tamper-detection) is unchanged.

### 6.3 End-to-end fixtures

`examples/cache_benchmarks/verify.sh` (31 scenarios) and `examples/cache_dep_drift/verify.sh` (3 scenarios) exercise the full `put`/`get` round-trip through real builds; their continued green status is the integration regression bar. Every steady-state `put` is either to a fresh key or an idempotent re-put with identical bytes — both succeed under the new contract; the streaming surface changes the in-memory shape, not the on-disk shape.

## 7. Spec amendments

### 7.1 §{exec.cache.integrity}

Append one sentence to the existing paragraph (the one that says "the implementation MAY use any deterministic fingerprint that meets the observability constraint") at `standard/src/content/docs/08-execution-model.mdx:204`:

> The fingerprint verification required by this section MAY be performed as bytes flow through a streaming reader, with the verification surfacing as an end-of-stream error; an implementation is not required to materialise the full artifact in memory before verifying.

This is a permissive amendment, not a new MUST. The trait shape is implementation-internal and not language-surface; the Standard already permits any deterministic fingerprint and any verification implementation. The new sentence makes it explicit that incremental / streaming verification is conformant — this matters for an implementer reading the Standard who might otherwise assume "verify the bytes" implies "all bytes resident".

### 7.2 No §9 schema change

CS-0056 introduces no user-visible configuration surface. The streaming I/O shape is internal to the backend implementation; users do not opt in or out.

## 8. Backwards compatibility

- **On-disk cache:** `CACHE_VERSION` unchanged. The `.bin` and `.meta.json` files are byte-identical to CS-0055.
- **Backend trait:** `CacheBackend::get` and `CacheBackend::put` signatures CHANGED. Out-of-tree implementers must rewrite. The in-tree `LocalBackend` is the only implementer today. The future `CloudBackend` consumes the new shape natively (it was the motivating use case).
- **Backend callers:** the `cook-engine` and `cook-fingerprint` call sites are rewritten to use `put_bytes` / `get_bytes` helpers (for sites that already have bytes in hand) or to drain the streaming reader inline (for `try_restore`, which still needs the bytes resident for the xxh3_64 outer-check). No engine logic changes.
- **`ArtifactMeta`:** the in-memory binding pattern shifts from `let meta = …` to `let mut meta = …`; the field set is unchanged.
- **Cookfile grammar:** unchanged.
- **Cook Lua API:** unchanged.
- **Configuration:** unchanged.

## 9. Open questions

1. **Should `VerifyingReader` live in a shared crate (e.g., a new `cook-cache::streaming` module) so the future `CloudBackend` can re-use it?** Recommendation: defer until the cloud backend lands. Cross-crate sharing is cheap to refactor when the second consumer arrives; speculatively factoring out a 30-line wrapper is over-engineering. The wrapper is currently in `cli/crates/cook-cache/src/backend.rs` next to `LocalBackend`.
2. **Should `put_bytes` / `get_bytes` be default methods on the trait?** The trait is object-safe today (`&dyn CacheBackend`) which precludes generic default methods that reference `Self`. Free functions over `&dyn CacheBackend` give the same ergonomics with no object-safety conflict. Keep as free functions.
3. **Should the streaming `put` enforce a maximum artifact size?** Today there is no limit; a runaway producer could fill the disk. Recommendation: defer; a size cap is a policy decision that belongs in `cloud.toml` configuration, not in the trait. The CI/CD environments that ship Cook today enforce disk quotas at the OS level.
4. **Should `meta.size_bytes` be authoritatively set by the streaming `put`?** Today it is caller-set; the `put` observes the streamed total but does not overwrite. Recommendation: keep caller-set. Overwriting could regress callers who pre-set `size_bytes` from a known-good source; surfacing a mismatch as an error is too aggressive (the field is diagnostic, not part of the integrity invariant). A future refinement could log a `tracing::warn!` when caller-set and streamed totals disagree, but that's an observability nit.
5. **Should `get` validate `expected_size` from the sidecar before opening the bytes file?** The sidecar's `size_bytes` could short-circuit a tamper that increased the artifact size beyond a sensible bound, before the streaming verifier hits EOF. Recommendation: defer. It's a defence-in-depth optimisation for a different threat (resource exhaustion via inflated artifacts), not a correctness improvement; the SHA-256 covers the bytes regardless of length.

## Appendix A. Why `Box<dyn Read + Send>` and not a generic associated type or `impl Trait`

A trait method returning `impl Read + Send` would require Rust 1.75+ RPITIT (return-position `impl Trait` in traits), which is supported but precludes object-safe `&dyn CacheBackend` — the engine holds the backend as `Arc<dyn CacheBackend>` because the choice between `LocalBackend` and `CloudBackend` is determined at build start, not at compile time. A generic associated type (`type Reader: Read + Send;`) has the same object-safety problem.

`Box<dyn Read + Send>` is the trait-object-friendly shape: one allocation per `get`, stable ABI across implementers, supports any concrete reader (`File`, HTTP body, `Cursor`, the verifier wrapper). The allocation cost is negligible compared to the I/O it gates. `Send` is required because the engine drives backends from a worker thread; `Sync` is not required because the reader is owned by one consumer at a time.

## Appendix B. Why `&mut dyn Read` for `put` and not `Box<dyn Read>`

The `get` side returns ownership (the backend opened the source; the caller drains it). The `put` side borrows the source (the caller owns it; the backend just reads it). `&mut dyn Read` is the ergonomic shape — the caller can pass `&mut some_file` or `&mut Cursor::new(bytes)` directly without transferring ownership. The trade-off is that `put` cannot retain the reader past the call (which is desired anyway — the put completes synchronously).

## Appendix C. Why the stamp is `&mut ArtifactMeta` and not a return value

CS-0054 took `meta: &ArtifactMeta` and persisted a stamped clone; the caller never observed the stamped hash unless they re-read the sidecar. CS-0056 makes `put` stamp `meta.content_hash` in-place via `&mut`. The benefits:

1. Symmetry with CS-0054's invariant ("`put` is the sole authority on `content_hash`") — the caller now visibly receives the stamp.
2. Eliminates a clone on the hot path (CS-0054 had `let stamped = ArtifactMeta { content_hash, ..meta.clone() };`).
3. Enables observability without re-reading the sidecar — telemetry / logging can read `meta.content_hash` after `put` returns.
4. Surfaces the "caller-claimed hash mismatch" check at the type level: the caller now has standing to pass a non-zero hash and have it validated, because `&mut` makes ownership of the field explicit.

The trade-off is a slightly heavier caller signature (`let mut meta = …`); given how few call sites there are (four in the engine, ~ten in tests), this is acceptable.
